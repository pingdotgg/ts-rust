//! Port of `checker/checker.go` lines 29672-30606: contextual types for
//! object literal elements, array elements, conditionals and templates,
//! effective call arguments, decorator call signatures, contextual property
//! types, apparent contextual types and object literal discrimination.

use crate::jsnum::Number;
use crate::prelude::*;

/// The result of `get_effective_call_arguments`: Go `[]*ast.Node`.
// PERF: callcopy1. Go returns the call's own argument list without a copy.
// `Slice` is that list; it is `Copy`, so a caller that needs only a length
// or a position reads it in place. The other cases build a new list.
#[derive(Debug)]
pub enum EffectiveArgs {
    /// The call's arguments, with no spread (`node.arguments()`).
    Slice(NodeSlice),
    /// Synthetic arguments: spreads of tuple types, tagged templates, JSX,
    /// decorators and `instanceof`.
    Owned(Vec<Node>),
}

impl EffectiveArgs {
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Slice(args) => args.len(),
            Self::Owned(args) => args.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Go `args[i]`. Panics when `i` is out of range, like Go.
    #[must_use]
    pub fn get(&self, i: usize) -> Node {
        match self {
            Self::Slice(args) => args.get(i),
            Self::Owned(args) => args[i],
        }
    }

    #[must_use]
    pub fn iter(&self) -> EffectiveArgsIter<'_> {
        match self {
            Self::Slice(args) => EffectiveArgsIter::Slice(args.iter()),
            Self::Owned(args) => EffectiveArgsIter::Owned(args.iter()),
        }
    }
}

/// Iterator over `EffectiveArgs`. Yields `Node` by value.
pub enum EffectiveArgsIter<'a> {
    Slice(NodeSliceIter),
    Owned(std::slice::Iter<'a, Node>),
}

impl Iterator for EffectiveArgsIter<'_> {
    type Item = Node;

    #[inline]
    fn next(&mut self) -> Option<Node> {
        match self {
            Self::Slice(iter) => iter.next(),
            Self::Owned(iter) => iter.next().copied(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Slice(iter) => iter.size_hint(),
            Self::Owned(iter) => iter.size_hint(),
        }
    }
}

impl ExactSizeIterator for EffectiveArgsIter<'_> {}

impl Checker {
    // Go: checker/checker.go:30404 getContextualTypeForObjectLiteralElement
    pub fn get_contextual_type_for_object_literal_element(
        &mut self,
        element: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let t = element.type_();
        if t.is_some() && !is_object_literal_method(element) {
            return self.get_type_from_type_node(t);
        }
        let object_literal = element.parent();
        let t = self.get_apparent_type_of_contextual_type(object_literal, context_flags);
        if t.is_some() {
            if self.has_bindable_name(element) {
                // For a (non-symbol) computed property, there is no reason to look up the name
                // in the type. It will just be "__computed", which does not appear in any
                // SymbolTable.
                let symbol = self.get_symbol_of_declaration(element);
                let name = self.sym(symbol).name.clone();
                let name_type = self
                    .value_symbol_links
                    .get_by_id(&self.symbols, symbol)
                    .name_type;
                return self.get_type_of_property_of_contextual_type_ex(t, &name, name_type);
            }
            if has_dynamic_name(element) {
                let name = get_name_of_declaration(element);
                if name.is_some() && is_computed_property_name(name) {
                    let expr_type = self.check_expression(name.expression());
                    if self.is_type_usable_as_property_name(expr_type) {
                        let prop_name = self.get_property_name_from_type(expr_type);
                        let prop_type = self.get_type_of_property_of_contextual_type(t, &prop_name);
                        if prop_type.is_some() {
                            return prop_type;
                        }
                    }
                }
            }
            if element.name().is_some() {
                let name_type = self.get_literal_type_from_property_name(element.name());
                // We avoid calling getApplicableIndexInfo here because it performs potentially expensive intersection reduction.
                return self.map_type_ex(
                    t,
                    &mut |c: &mut Checker, t: TypeId| -> TypeId {
                        let infos = c.get_index_infos_of_structured_type(t);
                        let index_info = c.find_applicable_index_info(&infos, name_type);
                        if index_info.is_nil() {
                            return TypeId::NIL;
                        }
                        c.index_info(index_info).value_type
                    },
                    true, /*noReductions*/
                );
            }
        }
        TypeId::NIL
    }

    // In an object literal contextually typed by a type T, the contextual type of a property assignment is the type of
    // the matching property in T, if one exists. Otherwise, it is the type of the numeric index signature in T, if one
    // exists. Otherwise, it is the type of the string index signature in T, if one exists.
    // Go: checker/checker.go:30448 getContextualTypeForObjectLiteralMethod
    pub fn get_contextual_type_for_object_literal_method(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        if node.flags().intersects(NodeFlags::IN_WITH_STATEMENT) {
            // We cannot answer semantic questions within a with block, do not proceed any further
            return TypeId::NIL;
        }
        self.get_contextual_type_for_object_literal_element(node, context_flags)
    }

    // Go: checker/checker.go:30456 getContextualTypeForElementExpression
    pub fn get_contextual_type_for_element_expression(
        &mut self,
        t: TypeId,
        index: i32,
        length: i32,
        first_spread_index: i32,
        last_spread_index: i32,
    ) -> TypeId {
        if t.is_nil() {
            return TypeId::NIL;
        }
        self.map_type_ex(
            t,
            &mut |c: &mut Checker, t: TypeId| -> TypeId {
                if c.is_tuple_type(t) {
                    // If index is before any spread element and within the fixed part of the contextual tuple type, return
                    // the type of the contextual tuple element.
                    if (first_spread_index < 0 || index < first_spread_index)
                        && index < c.target_tuple_type(t).fixed_length
                    {
                        let type_arg = c.type_arguments_of(t)[index as usize];
                        let is_optional = c.target_tuple_type(t).element_infos[index as usize]
                            .flags
                            .intersects(ElementFlags::OPTIONAL);
                        return c.remove_missing_type(type_arg, is_optional);
                    }
                    // When the length is known and the index is after all spread elements we compute the offset from the element
                    // to the end and the number of ending fixed elements in the contextual tuple type.
                    let mut offset = 0;
                    if length >= 0 && (last_spread_index < 0 || index > last_spread_index) {
                        offset = length - index;
                    }
                    let mut fixed_end_length = 0;
                    if offset > 0
                        && c.target_tuple_type(t)
                            .combined_flags
                            .intersects(ElementFlags::VARIABLE)
                    {
                        fixed_end_length =
                            get_end_element_count(c.target_tuple_type(t), ElementFlags::FIXED);
                    }
                    // If the offset is within the ending fixed part of the contextual tuple type, return the type of the contextual
                    // tuple element.
                    if offset > 0 && offset <= fixed_end_length {
                        let arity = c.get_type_reference_arity(t);
                        return c.type_arguments_of(t)[(arity - offset) as usize];
                    }
                    // Return a union of the possible contextual element types with no subtype reduction.
                    let mut tuple_index = c.target_tuple_type(t).fixed_length;
                    if first_spread_index >= 0 {
                        tuple_index = tuple_index.min(first_spread_index);
                    }
                    let mut end_skip_count = fixed_end_length;
                    if length >= 0 && last_spread_index >= 0 {
                        end_skip_count = fixed_end_length.min(length - last_spread_index);
                    }
                    return c.get_element_type_of_slice_of_tuple_type(
                        t,
                        tuple_index,
                        end_skip_count,
                        false, /*writing*/
                        true,  /*noReductions*/
                    );
                }
                // If element index is known and a contextual property with that name exists, return it. Otherwise return the
                // iterated or element type of the contextual type.
                if first_spread_index < 0 || index < first_spread_index {
                    let prop_type =
                        c.get_type_of_property_of_contextual_type(t, &index.to_string());
                    if prop_type.is_some() {
                        return prop_type;
                    }
                }
                let undefined_type = c.undefined_type;
                c.get_iterated_type_or_element_type(
                    IterationUse::ELEMENT,
                    t,
                    undefined_type,
                    Node::NIL, /*errorNode*/
                    false,     /*checkAssignability*/
                )
            },
            true, /*noReductions*/
        )
    }

    // In a contextually typed conditional expression, the true/false expressions are contextually typed by the same type.
    // Go: checker/checker.go:30506 getContextualTypeForConditionalOperand
    pub fn get_contextual_type_for_conditional_operand(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let conditional = node.parent();
        if node == conditional.when_true() || node == conditional.when_false() {
            return self.get_contextual_type(node.parent(), context_flags);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30514 getContextualTypeForSubstitutionExpression
    pub fn get_contextual_type_for_substitution_expression(
        &mut self,
        template: Node,
        substitution_expression: Node,
    ) -> TypeId {
        if is_tagged_template_expression(template.parent()) {
            return self
                .get_contextual_type_for_argument(template.parent(), substitution_expression);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30521 getContextualImportAttributeType
    pub fn get_contextual_import_attribute_type(&mut self, node: Node) -> TypeId {
        let get_global_import_attributes_type = self.get_global_import_attributes_type.clone();
        let import_attributes_type = get_global_import_attributes_type(self);
        self.get_type_of_property_of_contextual_type(import_attributes_type, node.name().text())
    }

    // Returns the effective arguments for an expression that works like a function invocation.
    // Go: checker/checker.go:30526 getEffectiveCallArguments
    pub fn get_effective_call_arguments(&mut self, node: Node) -> EffectiveArgs {
        if is_jsx_opening_fragment(node) {
            // This attributes Type does not include a children property yet, the same way a fragment created with <React.Fragment> does not at this stage
            let empty_fresh_jsx_object_type = self.empty_fresh_jsx_object_type;
            return EffectiveArgs::Owned(vec![self.create_synthetic_expression(
                node,
                empty_fresh_jsx_object_type,
                false,
                Node::NIL,
            )]);
        } else if is_tagged_template_expression(node) {
            let template = node.template();
            let get_global_template_strings_array_type =
                self.get_global_template_strings_array_type.clone();
            let template_strings_array_type = get_global_template_strings_array_type(self);
            let first_arg = self.create_synthetic_expression(
                template,
                template_strings_array_type,
                false,
                Node::NIL,
            );
            if !is_template_expression(template) {
                return EffectiveArgs::Owned(vec![first_arg]);
            }
            let spans = template.template_spans().nodes();
            let mut args = Vec::with_capacity(spans.len() + 1);
            args.push(first_arg);
            for span in spans.iter() {
                args.push(span.expression());
            }
            return EffectiveArgs::Owned(args);
        } else if is_decorator(node) {
            return EffectiveArgs::Owned(self.get_effective_decorator_arguments(node));
        } else if is_binary_expression(node) {
            // Handles instanceof operator
            return EffectiveArgs::Owned(vec![node.left()]);
        } else if is_jsx_opening_like_element(node) {
            if node.attributes().properties().len() != 0
                || (is_jsx_opening_element(node) && node.parent().children().nodes().len() != 0)
            {
                return EffectiveArgs::Owned(vec![node.attributes()]);
            }
            return EffectiveArgs::Owned(Vec::new());
        }
        // PERF: callcopy1. Without a spread the result is the argument list
        // itself (`NodeSlice` is program data), like Go, which returns
        // `node.Arguments()` without a copy. The spread search is
        // `get_spread_argument_index` on the slice.
        let args = node.arguments();
        if let Some(spread_index) = args.iter().position(is_spread_argument) {
            // Create synthetic arguments from spreads of tuple types.
            let mut effective_args: Vec<Node> = args.iter().take(spread_index).collect();
            for arg in args.iter().skip(spread_index) {
                let mut spread_type = TypeId::NIL;
                // We can call checkExpressionCached because spread expressions never have a contextual type.
                if is_spread_element(arg) {
                    if !self.flow_loop_stack.is_empty() {
                        spread_type = self.check_expression(arg.expression());
                    } else {
                        spread_type = self.check_expression_cached(arg.expression());
                    }
                }
                if spread_type.is_some() && self.is_tuple_type(spread_type) {
                    let element_types = self.get_element_types(spread_type);
                    for (i, t) in element_types.into_iter().enumerate() {
                        let flags = self.target_tuple_type(spread_type).element_infos[i].flags;
                        let labeled_declaration = self.target_tuple_type(spread_type).element_infos
                            [i]
                            .labeled_declaration;
                        let mut synthetic_type = t;
                        if flags.intersects(ElementFlags::REST) {
                            synthetic_type = self.create_array_type(t);
                        }
                        let synthetic_arg = self.create_synthetic_expression(
                            arg,
                            synthetic_type,
                            flags.intersects(ElementFlags::VARIABLE),
                            labeled_declaration,
                        );
                        effective_args.push(synthetic_arg);
                    }
                } else {
                    effective_args.push(arg);
                }
            }
            return EffectiveArgs::Owned(effective_args);
        }
        EffectiveArgs::Slice(args)
    }

    // Go: checker/checker.go:30592 getSpreadArgumentIndex
    pub fn get_spread_argument_index(&self, args: &[Node]) -> i32 {
        match args.iter().position(|&arg| is_spread_argument(arg)) {
            Some(i) => i as i32,
            None => -1,
        }
    }
}

// Go: checker/checker.go:30596 isSpreadArgument
pub fn is_spread_argument(arg: Node) -> bool {
    is_spread_element(arg) || is_synthetic_expression(arg) && arg.is_spread()
}

impl Checker {
    // Go: checker/checker.go:30600 createSyntheticExpression
    pub fn create_synthetic_expression(
        &mut self,
        parent: Node,
        t: TypeId,
        is_spread: bool,
        tuple_name_source: Node,
    ) -> Node {
        let result = self
            .factory
            .new_synthetic_expression(t, is_spread, tuple_name_source);
        set_node_loc(result, parent.loc());
        set_node_parent(result, parent);
        result
    }

    // Go: checker/checker.go:30607 getSpreadIndices
    pub fn get_spread_indices(&mut self, node: Node) -> (i32, i32) {
        if !self.array_literal_links.get(node).indices_computed {
            let (mut first, mut last) = (-1i32, -1i32);
            for (i, element) in node.elements().iter().enumerate() {
                if is_spread_element(element) {
                    if first < 0 {
                        first = i as i32;
                    }
                    last = i as i32;
                }
            }
            let links = self.array_literal_links.get(node);
            links.first_spread_index = first;
            links.last_spread_index = last;
            links.indices_computed = true;
        }
        let links = self.array_literal_links.get(node);
        (links.first_spread_index, links.last_spread_index)
    }

    // Returns the synthetic argument list for a decorator invocation.
    // Go: checker/checker.go:30626 getEffectiveDecoratorArguments
    pub fn get_effective_decorator_arguments(&mut self, node: Node) -> Vec<Node> {
        let expr = node.expression();
        let signature = self.get_decorator_call_signature(node);
        if signature.is_some() {
            let parameters = self.sig(signature).parameters.clone();
            let mut args = Vec::with_capacity(parameters.len());
            for param in parameters {
                let param_type = self.get_type_of_symbol(param);
                args.push(self.create_synthetic_expression(expr, param_type, false, Node::NIL));
            }
            return args;
        }
        panic!("Decorator signature not found")
    }

    // Go: checker/checker.go:30639 getDecoratorCallSignature
    pub fn get_decorator_call_signature(&mut self, decorator: Node) -> SignatureId {
        if self.legacy_decorators {
            return self.get_legacy_decorator_call_signature(decorator);
        }
        self.get_es_decorator_call_signature(decorator)
    }

    // Go: checker/checker.go:30646 getLegacyDecoratorCallSignature
    pub fn get_legacy_decorator_call_signature(&mut self, decorator: Node) -> SignatureId {
        let node = decorator.parent();
        if self.signature_links.get(node).decorator_signature.is_nil() {
            let any_signature = self.any_signature;
            self.signature_links.get(node).decorator_signature = any_signature;
            match node.kind() {
                SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => {
                    // For a class decorator, the `target` is the type of the class (e.g. the
                    // "static" or "constructor" side of the class).
                    let symbol = self.get_symbol_of_declaration(node);
                    let target_type = self.get_type_of_symbol(symbol);
                    let target_param = self.new_parameter("target", target_type);
                    let void_type = self.void_type;
                    let return_type = self.get_union_type(&[target_type, void_type]);
                    let sig =
                        self.new_call_signature(&[], SymbolId::NIL, &[target_param], return_type);
                    self.signature_links.get(node).decorator_signature = sig;
                }
                SyntaxKind::Parameter => 'arm: {
                    if !is_constructor_declaration(node.parent())
                        && !(is_method_declaration(node.parent())
                            || is_set_accessor_declaration(node.parent())
                                && is_class_like(node.parent().parent()))
                    {
                        break 'arm;
                    }
                    if get_this_parameter(node.parent()) == node {
                        break 'arm;
                    }
                    let position = node
                        .parent()
                        .parameters()
                        .iter()
                        .position(|p| p == node)
                        .map_or(-1, |i| i as i32);
                    let index = position
                        - if get_this_parameter(node.parent()).is_some() {
                            1
                        } else {
                            0
                        };
                    go_assert!(index >= 0);
                    // A parameter declaration decorator will have three arguments (see `ParameterDecorator` in
                    // core.d.ts).
                    let target_type;
                    let key_type;
                    if is_constructor_declaration(node.parent()) {
                        let symbol = self.get_symbol_of_declaration(node.parent().parent());
                        target_type = self.get_type_of_symbol(symbol);
                        key_type = self.undefined_type;
                    } else {
                        target_type = self.get_parent_type_of_class_element(node.parent());
                        key_type = self.get_class_element_property_key_type(node.parent());
                    }
                    let index_type = self.get_number_literal_type(Number::from(index as f64));
                    let target_param = self.new_parameter("target", target_type);
                    let key_param = self.new_parameter("propertyKey", key_type);
                    let index_param = self.new_parameter("parameterIndex", index_type);
                    let void_type = self.void_type;
                    let sig = self.new_call_signature(
                        &[],
                        SymbolId::NIL,
                        &[target_param, key_param, index_param],
                        void_type,
                    );
                    self.signature_links.get(node).decorator_signature = sig;
                }
                SyntaxKind::MethodDeclaration
                | SyntaxKind::GetAccessor
                | SyntaxKind::SetAccessor
                | SyntaxKind::PropertyDeclaration => 'arm: {
                    if !is_class_like(node.parent()) {
                        break 'arm;
                    }
                    // A method or accessor declaration decorator will have either two or three arguments (see
                    // `PropertyDecorator` and `MethodDecorator` in core.d.ts).
                    let target_type = self.get_parent_type_of_class_element(node);
                    let target_param = self.new_parameter("target", target_type);
                    let key_type = self.get_class_element_property_key_type(node);
                    let key_param = self.new_parameter("propertyKey", key_type);
                    let mut return_type = self.void_type;
                    if !is_property_declaration(node) {
                        let node_type = self.get_type_of_node(node);
                        return_type = self.new_typed_property_descriptor_type(node_type);
                    }
                    let has_prop_desc =
                        !is_property_declaration(node) || has_accessor_modifier(node);
                    let void_type = self.void_type;
                    if has_prop_desc {
                        let node_type = self.get_type_of_node(node);
                        let descriptor_type = self.new_typed_property_descriptor_type(node_type);
                        let descriptor_param = self.new_parameter("descriptor", descriptor_type);
                        let union = self.get_union_type(&[return_type, void_type]);
                        let sig = self.new_call_signature(
                            &[],
                            SymbolId::NIL,
                            &[target_param, key_param, descriptor_param],
                            union,
                        );
                        self.signature_links.get(node).decorator_signature = sig;
                    } else {
                        let union = self.get_union_type(&[return_type, void_type]);
                        let sig = self.new_call_signature(
                            &[],
                            SymbolId::NIL,
                            &[target_param, key_param],
                            union,
                        );
                        self.signature_links.get(node).decorator_signature = sig;
                    }
                }
                _ => {}
            }
        }
        let decorator_signature = self.signature_links.get(node).decorator_signature;
        if decorator_signature == self.any_signature {
            return SignatureId::NIL;
        }
        decorator_signature
    }

    // Go: checker/checker.go:30713 getESDecoratorCallSignature
    pub fn get_es_decorator_call_signature(&mut self, decorator: Node) -> SignatureId {
        // We are considering a future change that would allow the type of a decorator to affect the type of the
        // class and its members, such as a `@Stringify` decorator changing the type of a `number` field to `string`, or
        // a `@Callable` decorator adding a call signature to a `class`. The type arguments for the various context
        // types may eventually change to reflect such mutations.
        //
        // In some cases we describe such potential mutations as coming from a "prior decorator application". It is
        // important to note that, while decorators are *evaluated* left to right, they are *applied* right to left
        // to preserve f ৹ g -> f(g(x)) application order. In these cases, a "prior" decorator usually means the
        // next decorator following this one in document order.
        //
        // The "original type" of a class or member is the type it was declared as, or the type we infer from
        // initializers, before _any_ decorators are applied.
        //
        // The type of a class or member that is a result of a prior decorator application represents the
        // "current type", i.e., the type for the declaration at the time the decorator is _applied_.
        //
        // The type of a class or member that is the result of the application of *all* relevant decorators is the
        // "final type".
        //
        // Any decorator that allows mutation or replacement will also refer to an "input type" and an
        // "output type". The "input type" corresponds to the "current type" of the declaration, while the
        // "output type" will become either the "input type/current type" for a subsequent decorator application,
        // or the "final type" for the decorated declaration.
        //
        // It is important to understand decorator application order as it relates to how the "current", "input",
        // "output", and "final" types will be determined:
        //
        //  @E2 @E1 class SomeClass {
        //      @A2 @A1 static f() {}
        //      @B2 @B1 g() {}
        //      @C2 @C1 static x;
        //      @D2 @D1 y;
        //  }
        //
        // Per [the specification][1], decorators are applied in the following order:
        //
        // 1. For each static method (incl. get/set methods and `accessor` fields), in document order:
        //    a. Apply each decorator for that method, in reverse order (`A1`, `A2`).
        // 2. For each instance method (incl. get/set methods and `accessor` fields), in document order:
        //    a. Apply each decorator for that method, in reverse order (`B1`, `B2`).
        // 3. For each static field (excl. auto-accessors), in document order:
        //    a. Apply each decorator for that field, in reverse order (`C1`, `C2`).
        // 4. For each instance field (excl. auto-accessors), in document order:
        //    a. Apply each decorator for that field, in reverse order (`D1`, `D2`).
        // 5. Apply each decorator for the class, in reverse order (`E1`, `E2`).
        //
        // As a result, "current" types at each decorator application are as follows:
        // - For `A1`, the "current" types of the class and method are their "original" types.
        // - For `A2`, the "current type" of the method is the "output type" of `A1`, and the "current type" of the
        //   class is the type of `SomeClass` where `f` is the "output type" of `A1`. This becomes the "final type"
        //   of `f`.
        // - For `B1`, the "current type" of the method is its "original type", and the "current type" of the class
        //   is the type of `SomeClass` where `f` now has its "final type".
        // - etc.
        //
        // [1]: https://arai-a.github.io/ecma262-compare/?pr=2417&id=sec-runtime-semantics-classdefinitionevaluation
        //
        // This seems complicated at first glance, but is not unlike our existing inference for functions:
        //
        //  declare function pipe<Original, A1, A2, B1, B2, C1, C2, D1, D2, E1, E2>(
        //      original: Original,
        //      a1: (input: Original, context: Context<E2>) => A1,
        //      a2: (input: A1, context: Context<E2>) => A2,
        //      b1: (input: A2, context: Context<E2>) => B1,
        //      b2: (input: B1, context: Context<E2>) => B2,
        //      c1: (input: B2, context: Context<E2>) => C1,
        //      c2: (input: C1, context: Context<E2>) => C2,
        //      d1: (input: C2, context: Context<E2>) => D1,
        //      d2: (input: D1, context: Context<E2>) => D2,
        //      e1: (input: D2, context: Context<E2>) => E1,
        //      e2: (input: E1, context: Context<E2>) => E2,
        //  ): E2;

        // When a decorator is applied, it is passed two arguments: "target", which is a value representing the
        // thing being decorated (constructors for classes, functions for methods/accessors, `undefined` for fields,
        // and a `{ get, set }` object for auto-accessors), and "context", which is an object that provides
        // reflection information about the decorated element, as well as the ability to add additional "extra"
        // initializers. In most cases, the "target" argument corresponds to the "input type" in some way, and the
        // return value similarly corresponds to the "output type" (though if the "output type" is `void` or
        // `undefined` then the "output type" is the "input type").
        let node = decorator.parent();
        if self.signature_links.get(node).decorator_signature.is_nil() {
            let any_signature = self.any_signature;
            self.signature_links.get(node).decorator_signature = any_signature;
            match node.kind() {
                SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => {
                    // Class decorators have a `context` of `ClassDecoratorContext<Class>`, where the `Class` type
                    // argument will be the "final type" of the class after all decorators are applied.
                    let symbol = self.get_symbol_of_declaration(node);
                    let target_type = self.get_type_of_symbol(symbol);
                    let context_type = self.new_class_decorator_context_type(target_type);
                    let sig = self.new_es_decorator_call_signature(
                        target_type,
                        context_type,
                        target_type,
                    );
                    self.signature_links.get(node).decorator_signature = sig;
                }
                SyntaxKind::MethodDeclaration
                | SyntaxKind::GetAccessor
                | SyntaxKind::SetAccessor => 'arm: {
                    if !is_class_like(node.parent()) {
                        break 'arm;
                    }
                    // Method decorators have a `context` of `ClassMethodDecoratorContext<This, Value>`, where the
                    // `Value` type argument corresponds to the "final type" of the method.
                    //
                    // Getter decorators have a `context` of `ClassGetterDecoratorContext<This, Value>`, where the
                    // `Value` type argument corresponds to the "final type" of the value returned by the getter.
                    //
                    // Setter decorators have a `context` of `ClassSetterDecoratorContext<This, Value>`, where the
                    // `Value` type argument corresponds to the "final type" of the parameter of the setter.
                    //
                    // In all three cases, the `This` type argument is the "final type" of either the class or
                    // instance, depending on whether the member was `static`.
                    let value_type = if is_method_declaration(node) {
                        let sig = self.get_signature_from_declaration(node);
                        self.get_or_create_type_from_signature(sig)
                    } else {
                        self.get_type_of_node(node)
                    };
                    let this_type = if has_static_modifier(node) {
                        let symbol = self.get_symbol_of_declaration(node.parent());
                        self.get_type_of_symbol(symbol)
                    } else {
                        let symbol = self.get_symbol_of_declaration(node.parent());
                        self.get_declared_type_of_class_or_interface(symbol)
                    };
                    // We wrap the "input type", if necessary, to match the decoration target. For getters this is
                    // something like `() => inputType`, for setters it's `(value: inputType) => void` and for
                    // methods it is just the input type.
                    let target_type = if is_get_accessor_declaration(node) {
                        self.new_getter_function_type(value_type)
                    } else if is_set_accessor_declaration(node) {
                        self.new_setter_function_type(value_type)
                    } else {
                        value_type
                    };
                    let context_type = self.new_class_member_decorator_context_type_for_node(
                        node, this_type, value_type,
                    );
                    let sig = self.new_es_decorator_call_signature(
                        target_type,
                        context_type,
                        target_type,
                    );
                    self.signature_links.get(node).decorator_signature = sig;
                }
                SyntaxKind::PropertyDeclaration => 'arm: {
                    if !is_class_like(node.parent()) {
                        break 'arm;
                    }
                    // Field decorators have a `context` of `ClassFieldDecoratorContext<This, Value>` and
                    // auto-accessor decorators have a `context` of `ClassAccessorDecoratorContext<This, Value>. In
                    // both cases, the `This` type argument is the "final type" of either the class or instance,
                    // depending on whether the member was `static`, and the `Value` type argument corresponds to
                    // the "final type" of the value stored in the field.
                    let value_type = self.get_type_of_node(node);
                    let this_type = if has_static_modifier(node) {
                        let symbol = self.get_symbol_of_declaration(node.parent());
                        self.get_type_of_symbol(symbol)
                    } else {
                        let symbol = self.get_symbol_of_declaration(node.parent());
                        self.get_declared_type_of_class_or_interface(symbol)
                    };
                    // The `target` of an auto-accessor decorator is a `{ get, set }` object, representing the
                    // runtime-generated getter and setter that are added to the class/prototype. The `target` of a
                    // regular field decorator is always `undefined` as it isn't installed until it is initialized.
                    let target_type = if has_accessor_modifier(node) {
                        self.new_class_accessor_decorator_target_type(this_type, value_type)
                    } else {
                        self.undefined_type
                    };
                    // We wrap the "output type" depending on the declaration. For auto-accessors, we wrap the
                    // "output type" in a `ClassAccessorDecoratorResult<This, In, Out>` type, which allows for
                    // mutation of the runtime-generated getter and setter, as well as the injection of an
                    // initializer mutator. For regular fields, we wrap the "output type" in an initializer mutator.
                    let return_type = if has_accessor_modifier(node) {
                        self.new_class_accessor_decorator_result_type(this_type, value_type)
                    } else {
                        self.new_class_field_decorator_initializer_mutator_type(
                            this_type, value_type,
                        )
                    };
                    let context_type = self.new_class_member_decorator_context_type_for_node(
                        node, this_type, value_type,
                    );
                    let sig = self.new_es_decorator_call_signature(
                        target_type,
                        context_type,
                        return_type,
                    );
                    self.signature_links.get(node).decorator_signature = sig;
                }
                _ => {}
            }
        }
        let decorator_signature = self.signature_links.get(node).decorator_signature;
        if decorator_signature == self.any_signature {
            return SignatureId::NIL;
        }
        decorator_signature
    }

    // Go: checker/checker.go:30891 newClassDecoratorContextType
    pub fn new_class_decorator_context_type(&mut self, class_type: TypeId) -> TypeId {
        let f = self.get_global_class_decorator_context_type.clone();
        let target = f(self);
        self.try_create_type_reference(target, &[class_type])
    }

    // Go: checker/checker.go:30895 newClassMethodDecoratorContextType
    pub fn new_class_method_decorator_context_type(
        &mut self,
        class_type: TypeId,
        value_type: TypeId,
    ) -> TypeId {
        let f = self.get_global_class_method_decorator_context_type.clone();
        let target = f(self);
        self.try_create_type_reference(target, &[class_type, value_type])
    }

    // Go: checker/checker.go:30899 newClassGetterDecoratorContextType
    pub fn new_class_getter_decorator_context_type(
        &mut self,
        class_type: TypeId,
        value_type: TypeId,
    ) -> TypeId {
        let f = self.get_global_class_getter_decorator_context_type.clone();
        let target = f(self);
        self.try_create_type_reference(target, &[class_type, value_type])
    }

    // Go: checker/checker.go:30903 newClassSetterDecoratorContextType
    pub fn new_class_setter_decorator_context_type(
        &mut self,
        class_type: TypeId,
        value_type: TypeId,
    ) -> TypeId {
        let f = self.get_global_class_setter_decorator_context_type.clone();
        let target = f(self);
        self.try_create_type_reference(target, &[class_type, value_type])
    }

    // Go: checker/checker.go:30907 newClassAccessorDecoratorContextType
    pub fn new_class_accessor_decorator_context_type(
        &mut self,
        this_type: TypeId,
        value_type: TypeId,
    ) -> TypeId {
        let f = self
            .get_global_class_accessor_decorator_context_type
            .clone();
        let target = f(self);
        self.try_create_type_reference(target, &[this_type, value_type])
    }

    // Go: checker/checker.go:30911 newClassFieldDecoratorContextType
    pub fn new_class_field_decorator_context_type(
        &mut self,
        this_type: TypeId,
        value_type: TypeId,
    ) -> TypeId {
        let f = self.get_global_class_field_decorator_context_type.clone();
        let target = f(self);
        self.try_create_type_reference(target, &[this_type, value_type])
    }

    // Gets a type like `{ name: "foo", private: false, static: true }` that is used to provided member-specific
    // details that will be intersected with a decorator context type.
    // Go: checker/checker.go:30917 getClassMemberDecoratorContextOverrideType
    pub fn get_class_member_decorator_context_override_type(
        &mut self,
        name_type: TypeId,
        is_private: bool,
        is_static: bool,
    ) -> TypeId {
        let kind = if is_private {
            if is_static {
                CachedTypeKind::DECORATOR_CONTEXT_PRIVATE_STATIC
            } else {
                CachedTypeKind::DECORATOR_CONTEXT_PRIVATE
            }
        } else if is_static {
            CachedTypeKind::DECORATOR_CONTEXT_STATIC
        } else {
            CachedTypeKind::DECORATOR_CONTEXT
        };
        let key = CachedTypeKey {
            kind,
            type_id: name_type,
        };
        if let Some(&override_type) = self.cached_types.get(&key) {
            if override_type.is_some() {
                return override_type;
            }
        }
        let members = self.symbols.new_table();
        let name_prop = self.new_property("name", name_type);
        self.symbols.set(members, "name", name_prop);
        let private_type = if is_private {
            self.true_type
        } else {
            self.false_type
        };
        let private_prop = self.new_property("private", private_type);
        self.symbols.set(members, "private", private_prop);
        let static_type = if is_static {
            self.true_type
        } else {
            self.false_type
        };
        let static_prop = self.new_property("static", static_type);
        self.symbols.set(members, "static", static_prop);
        let override_type = self.new_anonymous_type(SymbolId::NIL, members, &[], &[], &[]);
        self.cached_types.insert(key, override_type);
        override_type
    }

    // Go: checker/checker.go:30936 newClassMemberDecoratorContextTypeForNode
    pub fn new_class_member_decorator_context_type_for_node(
        &mut self,
        node: Node,
        this_type: TypeId,
        value_type: TypeId,
    ) -> TypeId {
        let is_static = has_static_modifier(node);
        let is_private = is_private_identifier(node.name());
        let name_type = if is_private {
            self.get_string_literal_type(node.name().text())
        } else {
            self.get_literal_type_from_property_name(node.name())
        };
        let context_type = if is_method_declaration(node) {
            self.new_class_method_decorator_context_type(this_type, value_type)
        } else if is_get_accessor_declaration(node) {
            self.new_class_getter_decorator_context_type(this_type, value_type)
        } else if is_set_accessor_declaration(node) {
            self.new_class_setter_decorator_context_type(this_type, value_type)
        } else if is_auto_accessor_property_declaration(node) {
            self.new_class_accessor_decorator_context_type(this_type, value_type)
        } else if is_property_declaration(node) {
            self.new_class_field_decorator_context_type(this_type, value_type)
        } else {
            panic!("Unhandled case in createClassMemberDecoratorContextTypeForNode")
        };
        let override_type =
            self.get_class_member_decorator_context_override_type(name_type, is_private, is_static);
        self.get_intersection_type(&[context_type, override_type])
    }

    // Go: checker/checker.go:30964 newClassAccessorDecoratorTargetType
    pub fn new_class_accessor_decorator_target_type(
        &mut self,
        this_type: TypeId,
        value_type: TypeId,
    ) -> TypeId {
        let f = self.get_global_class_accessor_decorator_target_type.clone();
        let target = f(self);
        self.try_create_type_reference(target, &[this_type, value_type])
    }

    // Go: checker/checker.go:30968 newClassAccessorDecoratorResultType
    pub fn new_class_accessor_decorator_result_type(
        &mut self,
        this_type: TypeId,
        value_type: TypeId,
    ) -> TypeId {
        let f = self.get_global_class_accessor_decorator_result_type.clone();
        let target = f(self);
        self.try_create_type_reference(target, &[this_type, value_type])
    }

    // Go: checker/checker.go:30972 newClassFieldDecoratorInitializerMutatorType
    pub fn new_class_field_decorator_initializer_mutator_type(
        &mut self,
        this_type: TypeId,
        value_type: TypeId,
    ) -> TypeId {
        let this_param = self.new_parameter("this", this_type);
        let value_param = self.new_parameter("value", value_type);
        self.new_function_type(&[], this_param, &[value_param], value_type)
    }

    // Creates a call signature for an ES Decorator. This method is used by the semantics of
    // `getESDecoratorCallSignature`, which you should probably be using instead.
    // Go: checker/checker.go:30980 newESDecoratorCallSignature
    pub fn new_es_decorator_call_signature(
        &mut self,
        target_type: TypeId,
        context_type: TypeId,
        non_optional_return_type: TypeId,
    ) -> SignatureId {
        let target_param = self.new_parameter("target", target_type);
        let context_param = self.new_parameter("context", context_type);
        let void_type = self.void_type;
        let return_type = self.get_union_type(&[non_optional_return_type, void_type]);
        self.new_call_signature(
            &[],
            SymbolId::NIL, /*thisParameter*/
            &[target_param, context_param],
            return_type,
        )
    }

    // Creates a synthetic `FunctionType`
    // Go: checker/checker.go:30988 newFunctionType
    pub fn new_function_type(
        &mut self,
        type_parameters: &[TypeId],
        this_parameter: SymbolId,
        parameters: &[SymbolId],
        return_type: TypeId,
    ) -> TypeId {
        let signature =
            self.new_call_signature(type_parameters, this_parameter, parameters, return_type);
        self.get_or_create_type_from_signature(signature)
    }

    // Go: checker/checker.go:30993 newGetterFunctionType
    pub fn new_getter_function_type(&mut self, t: TypeId) -> TypeId {
        self.new_function_type(&[], SymbolId::NIL /*thisParameter*/, &[], t)
    }

    // Go: checker/checker.go:30997 newSetterFunctionType
    pub fn new_setter_function_type(&mut self, t: TypeId) -> TypeId {
        let value_param = self.new_parameter("value", t);
        let void_type = self.void_type;
        self.new_function_type(
            &[],
            SymbolId::NIL, /*thisParameter*/
            &[value_param],
            void_type,
        )
    }

    // Creates a synthetic `Signature` corresponding to a call signature.
    // Go: checker/checker.go:31003 newCallSignature
    pub fn new_call_signature(
        &mut self,
        type_parameters: &[TypeId],
        this_parameter: SymbolId,
        parameters: &[SymbolId],
        return_type: TypeId,
    ) -> SignatureId {
        let any_keyword = self.factory.new_keyword_type_node(SyntaxKind::AnyKeyword);
        let decl = self
            .factory
            .new_function_type_node(NodeList::NIL, NodeList::NIL, any_keyword);
        self.new_signature(
            SignatureFlags::NONE,
            decl,
            type_parameters,
            this_parameter,
            parameters,
            return_type,
            TypePredicateId::NIL,
            parameters.len() as i32,
        )
    }

    // Go: checker/checker.go:31008 newTypedPropertyDescriptorType
    pub fn new_typed_property_descriptor_type(&mut self, property_type: TypeId) -> TypeId {
        let f = self.get_global_typed_property_descriptor_type.clone();
        let generic_global_type = f(self);
        self.create_type_from_generic_global_type(generic_global_type, &[property_type])
    }

    // Go: checker/checker.go:31012 getParentTypeOfClassElement
    pub fn get_parent_type_of_class_element(&mut self, node: Node) -> TypeId {
        let class_symbol = self.get_symbol_of_node(node.parent());
        if is_static(node) {
            return self.get_type_of_symbol(class_symbol);
        }
        self.get_declared_type_of_symbol(class_symbol)
    }

    // Go: checker/checker.go:31020 getClassElementPropertyKeyType
    pub fn get_class_element_property_key_type(&mut self, element: Node) -> TypeId {
        let name = element.name();
        match name.kind() {
            SyntaxKind::Identifier | SyntaxKind::NumericLiteral | SyntaxKind::StringLiteral => {
                return self.get_string_literal_type(name.text());
            }
            SyntaxKind::ComputedPropertyName => {
                let name_type = self.check_computed_property_name(name);
                if self.is_type_assignable_to_kind(name_type, TypeFlags::ES_SYMBOL_LIKE) {
                    return name_type;
                }
                return self.string_type;
            }
            _ => {}
        }
        self.error_type
    }

    // Go: checker/checker.go:31035 getTypeOfPropertyOfContextualType
    pub fn get_type_of_property_of_contextual_type(&mut self, t: TypeId, name: &str) -> TypeId {
        self.get_type_of_property_of_contextual_type_ex(t, name, TypeId::NIL)
    }

    // Go: checker/checker.go:31039 getTypeOfPropertyOfContextualTypeEx
    pub fn get_type_of_property_of_contextual_type_ex(
        &mut self,
        t: TypeId,
        name: &str,
        name_type: TypeId,
    ) -> TypeId {
        self.map_type_ex(
            t,
            &mut |c: &mut Checker, t: TypeId| -> TypeId {
                if c.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
                    let mut types: Vec<TypeId> = Vec::new();
                    let mut index_info_candidates: Vec<TypeId> = Vec::new();
                    let mut ignore_index_infos = false;
                    for i in 0..c.ty(t).types().len() {
                        let constituent_type = c.type_at(t, i);
                        if !c.ty(constituent_type).flags.intersects(TypeFlags::OBJECT) {
                            continue;
                        }
                        if c.is_generic_mapped_type(constituent_type)
                            && c.get_mapped_type_name_type_kind(constituent_type)
                                != MappedTypeNameTypeKind::REMAPPING
                        {
                            let substituted_type = c
                                .get_indexed_mapped_type_substituted_type_of_contextual_type(
                                    constituent_type,
                                    name,
                                    name_type,
                                );
                            types = c.append_contextual_property_type_constituent(
                                types,
                                substituted_type,
                            );
                            continue;
                        }
                        let property_type = c.get_type_of_concrete_property_of_contextual_type(
                            constituent_type,
                            name,
                        );
                        if property_type.is_nil() {
                            if !ignore_index_infos {
                                index_info_candidates.push(constituent_type);
                            }
                            continue;
                        }
                        ignore_index_infos = true;
                        index_info_candidates = Vec::new();
                        types = c.append_contextual_property_type_constituent(types, property_type);
                    }
                    for candidate in index_info_candidates {
                        let index_info_type = c.get_type_from_index_infos_of_contextual_type(
                            candidate, name, name_type,
                        );
                        types =
                            c.append_contextual_property_type_constituent(types, index_info_type);
                    }
                    if types.is_empty() {
                        return TypeId::NIL;
                    }
                    if types.len() == 1 {
                        return types[0];
                    }
                    return c.get_intersection_type(&types);
                }
                if !c.ty(t).flags.intersects(TypeFlags::OBJECT) {
                    return TypeId::NIL;
                }
                if c.is_generic_mapped_type(t)
                    && c.get_mapped_type_name_type_kind(t) != MappedTypeNameTypeKind::REMAPPING
                {
                    return c.get_indexed_mapped_type_substituted_type_of_contextual_type(
                        t, name, name_type,
                    );
                }
                let result = c.get_type_of_concrete_property_of_contextual_type(t, name);
                if result.is_some() {
                    return result;
                }
                c.get_type_from_index_infos_of_contextual_type(t, name, name_type)
            },
            true, /*noReductions*/
        )
    }

    // Go: checker/checker.go:31091 getIndexedMappedTypeSubstitutedTypeOfContextualType
    pub fn get_indexed_mapped_type_substituted_type_of_contextual_type(
        &mut self,
        t: TypeId,
        name: &str,
        name_type: TypeId,
    ) -> TypeId {
        let mut property_name_type = name_type;
        if property_name_type.is_nil() {
            property_name_type = self.get_string_literal_type(name);
        }
        let constraint = self.get_constraint_type_from_mapped_type(t);
        // special case for conditional types pretending to be negated types
        let mapped_name_type = self.ty(t).as_mapped_type().name_type;
        if mapped_name_type.is_some()
            && self.is_excluded_mapped_property_name(mapped_name_type, property_name_type)
            || self.is_excluded_mapped_property_name(constraint, property_name_type)
        {
            return TypeId::NIL;
        }
        let constraint_of_constraint = self.get_base_constraint_or_type(constraint);
        if !self.is_type_assignable_to(property_name_type, constraint_of_constraint) {
            return TypeId::NIL;
        }
        self.substitute_indexed_mapped_type(t, property_name_type)
    }

    // Go: checker/checker.go:31108 isExcludedMappedPropertyName
    pub fn is_excluded_mapped_property_name(
        &mut self,
        t: TypeId,
        property_name_type: TypeId,
    ) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::CONDITIONAL) {
            let true_type = self.get_true_type_from_conditional_type(t);
            let reduced_true_type = self.get_reduced_type(true_type);
            if !self
                .ty(reduced_true_type)
                .flags
                .intersects(TypeFlags::NEVER)
            {
                return false;
            }
            let false_type = self.get_false_type_from_conditional_type(t);
            let check_type = self.ty(t).as_conditional_type().check_type;
            if self.get_actual_type_variable(false_type)
                != self.get_actual_type_variable(check_type)
            {
                return false;
            }
            let extends_type = self.ty(t).as_conditional_type().extends_type;
            return self.is_type_assignable_to(property_name_type, extends_type);
        }
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            for i in 0..self.ty(t).types().len() {
                let t = self.type_at(t, i);
                if self.is_excluded_mapped_property_name(t, property_name_type) {
                    return true;
                }
            }
            return false;
        }
        false
    }

    // Go: checker/checker.go:31122 getTypeOfConcretePropertyOfContextualType
    pub fn get_type_of_concrete_property_of_contextual_type(
        &mut self,
        t: TypeId,
        name: &str,
    ) -> TypeId {
        let prop = self.get_property_of_type(t, name);
        if prop.is_nil() || self.is_circular_mapped_property(prop) {
            return TypeId::NIL;
        }
        let prop_type = self.get_type_of_symbol(prop);
        let is_optional = self.sym(prop).flags.intersects(SymbolFlags::OPTIONAL);
        self.remove_missing_type(prop_type, is_optional)
    }

    // Go: checker/checker.go:31130 getTypeFromIndexInfosOfContextualType
    pub fn get_type_from_index_infos_of_contextual_type(
        &mut self,
        t: TypeId,
        name: &str,
        name_type: TypeId,
    ) -> TypeId {
        if self.is_tuple_type(t)
            && is_numeric_literal_name(name)
            && crate::jsnum::from_string(name) >= Number::new(0.0)
        {
            let fixed_length = self.target_tuple_type(t).fixed_length;
            let rest_type = self.get_element_type_of_slice_of_tuple_type(
                t,
                fixed_length,
                0,     /*endSkipCount*/
                false, /*writing*/
                true,  /*noReductions*/
            );
            if rest_type.is_some() {
                return rest_type;
            }
        }
        let mut name_type = name_type;
        if name_type.is_nil() {
            name_type = self.get_string_literal_type(name);
        }
        let infos = self.get_index_infos_of_structured_type(t);
        let index_info = self.find_applicable_index_info(&infos, name_type);
        if index_info.is_nil() {
            return TypeId::NIL;
        }
        self.index_info(index_info).value_type
    }

    // Go: checker/checker.go:31147 isCircularMappedProperty
    pub fn is_circular_mapped_property(&mut self, symbol: SymbolId) -> bool {
        if self.sym(symbol).check_flags.intersects(CheckFlags::MAPPED) {
            let resolved_type = self
                .value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .resolved_type;
            return resolved_type.is_nil()
                && self.find_resolution_cycle_start_index(
                    TypeSystemEntity::Symbol(symbol),
                    TypeSystemPropertyName::TYPE,
                ) >= 0;
        }
        false
    }

    // Go: checker/checker.go:31155 appendContextualPropertyTypeConstituent
    pub fn append_contextual_property_type_constituent(
        &self,
        mut types: Vec<TypeId>,
        t: TypeId,
    ) -> Vec<TypeId> {
        // any doesn't provide any contextual information but could spoil the overall result by nullifying contextual information
        // provided by other intersection constituents so it gets replaced with `unknown` as `T & unknown` is just `T` and all
        // types computed based on the contextual information provided by other constituens are still assignable to any
        if t.is_nil() {
            return types;
        }
        if self.ty(t).flags.intersects(TypeFlags::ANY) {
            types.push(self.unknown_type);
            return types;
        }
        types.push(t);
        types
    }

    // Return the contextual type for a given expression node. During overload resolution, a contextual type may temporarily
    // be "pushed" onto a node using the contextualType property.
    // Go: checker/checker.go:31170 getApparentTypeOfContextualType
    pub fn get_apparent_type_of_contextual_type(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let contextual_type = if is_object_literal_method(node) {
            self.get_contextual_type_for_object_literal_method(node, context_flags)
        } else {
            self.get_contextual_type(node, context_flags)
        };
        let instantiated_type =
            self.instantiate_contextual_type(contextual_type, node, context_flags);
        if instantiated_type.is_some()
            && !(context_flags.intersects(ContextFlags::NO_CONSTRAINTS)
                && self
                    .ty(instantiated_type)
                    .flags
                    .intersects(TypeFlags::TYPE_VARIABLE))
        {
            let apparent_type = self.map_type_ex(
                instantiated_type,
                &mut |c: &mut Checker, t: TypeId| -> TypeId {
                    if c.ty(t).object_flags.intersects(ObjectFlags::MAPPED) {
                        return t;
                    }
                    c.get_apparent_type(t)
                },
                true,
            );
            if self.ty(apparent_type).flags.intersects(TypeFlags::UNION)
                && is_object_literal_expression(node)
            {
                return self.discriminate_contextual_type_by_object_members(node, apparent_type);
            } else if self.ty(apparent_type).flags.intersects(TypeFlags::UNION)
                && is_jsx_attributes(node)
            {
                return self.discriminate_contextual_type_by_jsx_attributes(node, apparent_type);
            } else {
                return apparent_type;
            }
        }
        TypeId::NIL
    }
}

// Go: checker/checker.go:31197 ObjectLiteralDiscriminator
// PORT: Go keeps a `c *Checker` field. The Rust `Discriminator` trait
// methods receive the checker instead, so the struct only holds the items.
#[derive(Clone, Debug, Default)]
pub struct ObjectLiteralDiscriminator {
    pub props: Vec<Node>,
    pub members: Vec<SymbolId>,
}

impl Discriminator for ObjectLiteralDiscriminator {
    // Go: checker/checker.go:31203 ObjectLiteralDiscriminator.len
    fn len(&self) -> i32 {
        (self.props.len() + self.members.len()) as i32
    }

    // Go: checker/checker.go:31207 ObjectLiteralDiscriminator.name
    fn name(&self, c: &Checker, index: i32) -> String {
        let index = index as usize;
        if index < self.props.len() {
            return c.sym(self.props[index].symbol()).name.to_string();
        }
        c.sym(self.members[index - self.props.len()])
            .name
            .to_string()
    }

    // Go: checker/checker.go:31214 ObjectLiteralDiscriminator.matches
    fn matches(&mut self, c: &mut Checker, index: i32, t: TypeId) -> bool {
        let index = index as usize;
        let prop_type;
        if index < self.props.len() {
            let prop = self.props[index];
            if is_property_assignment(prop) || is_jsx_attribute(prop) {
                let initializer = prop.initializer();
                if initializer.is_some() {
                    prop_type = c.get_context_free_type_of_expression(prop.initializer());
                } else {
                    prop_type = c.true_type; // JsxAttribute without initializer is always true
                }
            } else {
                prop_type = c.get_context_free_type_of_expression(prop.name());
            }
        } else {
            prop_type = c.undefined_type;
        }
        for s in c.ty(prop_type).distributed() {
            if c.is_type_assignable_to(s, t) {
                return true;
            }
        }
        false
    }
}

impl Checker {
    // Go: checker/checker.go:31239 discriminateContextualTypeByObjectMembers
    pub fn discriminate_contextual_type_by_object_members(
        &mut self,
        node: Node,
        contextual_type: TypeId,
    ) -> TypeId {
        let key = DiscriminatedContextualTypeKey {
            node_id: node,
            type_id: contextual_type,
        };
        if let Some(&discriminated) = self.discriminated_contextual_types.get(&key) {
            if discriminated.is_some() {
                return discriminated;
            }
        }
        let mut discriminated =
            self.get_matching_union_constituent_for_object_literal(contextual_type, node);
        if discriminated.is_nil() {
            let mut discriminant_properties: Vec<Node> = Vec::new();
            for p in node.properties().iter() {
                let symbol = p.symbol();
                let keep = if symbol.is_nil() {
                    false
                } else if is_property_assignment(p) {
                    let name = self.sym(symbol).name.clone();
                    self.is_possibly_discriminant_value(p.initializer())
                        && self.is_discriminant_property_key(contextual_type, TableKey::Name(&name))
                } else if is_shorthand_property_assignment(p) {
                    let name = self.sym(symbol).name.clone();
                    self.is_discriminant_property_key(contextual_type, TableKey::Name(&name))
                } else {
                    false
                };
                if keep {
                    discriminant_properties.push(p);
                }
            }
            let mut discriminant_members: Vec<SymbolId> = Vec::new();
            let node_members = self.sym(node.symbol()).members;
            for s in self.get_properties_of_type(contextual_type) {
                let name = self.sym(s).name.clone();
                if self.sym(s).flags.intersects(SymbolFlags::OPTIONAL)
                    && self.symbols.get(node_members, &name).is_nil()
                    && self.is_discriminant_property_key(contextual_type, TableKey::Name(&name))
                {
                    discriminant_members.push(s);
                }
            }
            let mut discriminator = ObjectLiteralDiscriminator {
                props: discriminant_properties,
                members: discriminant_members,
            };
            discriminated =
                self.discriminate_type_by_discriminable_items(contextual_type, &mut discriminator);
        }
        self.discriminated_contextual_types
            .insert(key, discriminated);
        discriminated
    }

    // Go: checker/checker.go:31269 getMatchingUnionConstituentForObjectLiteral
    pub fn get_matching_union_constituent_for_object_literal(
        &mut self,
        union_type: TypeId,
        node: Node,
    ) -> TypeId {
        let key_property_name = self.get_key_property_name(union_type);
        if !key_property_name.is_empty() {
            let mut prop_node = Node::NIL;
            for p in node.properties().iter() {
                if p.symbol().is_some()
                    && is_property_assignment(p)
                    && self.sym(p.symbol()).name == key_property_name
                    && self.is_possibly_discriminant_value(p.initializer())
                {
                    prop_node = p;
                    break;
                }
            }
            if prop_node.is_some() {
                let prop_type = self.get_context_free_type_of_expression(prop_node.initializer());
                return self.get_constituent_type_for_key_type(union_type, prop_type);
            }
        }
        TypeId::NIL
    }

    // Return true if the given expression is possibly a discriminant value. We limit the kinds of
    // expressions we check to those that don't depend on their contextual type in order not to cause
    // recursive (and possibly infinite) invocations of getContextualType.
    // Go: checker/checker.go:31286 isPossiblyDiscriminantValue
    pub fn is_possibly_discriminant_value(&self, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::StringLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateExpression
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::Identifier
            | SyntaxKind::UndefinedKeyword => return true,
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ParenthesizedExpression => {
                return self.is_possibly_discriminant_value(node.expression());
            }
            SyntaxKind::JsxExpression => {
                return node.expression().is_nil()
                    || self.is_possibly_discriminant_value(node.expression());
            }
            _ => {}
        }
        false
    }

    // If the given contextual type contains instantiable types and if a mapper representing
    // return type inferences is available, instantiate those types using that mapper.
    // Go: checker/checker.go:31301 instantiateContextualType
    pub fn instantiate_contextual_type(
        &mut self,
        contextual_type: TypeId,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        if contextual_type.is_some()
            && self.maybe_type_of_kind(contextual_type, TypeFlags::INSTANTIABLE)
        {
            let inference_context = self.get_inference_context(node);
            // If no inferences have been made, and none of the type parameters for which we are inferring
            // specify default types, nothing is gained from instantiating as type parameters would just be
            // replaced with their constraints similar to the apparent type.
            if inference_context.is_some() {
                if context_flags.intersects(ContextFlags::SIGNATURE) {
                    let inference_count =
                        self.inference_context(inference_context).inferences.len();
                    let mut has_candidates_or_default = false;
                    for i in 0..inference_count {
                        if self.has_inference_candidates_or_default(inference_context, i) {
                            has_candidates_or_default = true;
                            break;
                        }
                    }
                    if has_candidates_or_default {
                        // For contextual signatures we incorporate all inferences made so far, e.g. from return
                        // types as well as arguments to the left in a function call.
                        let non_fixing_mapper =
                            self.inference_context(inference_context).non_fixing_mapper;
                        let t =
                            self.instantiate_instantiable_types(contextual_type, non_fixing_mapper);
                        if !self.ty(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                            return t;
                        }
                    }
                }
                let return_mapper = self.inference_context(inference_context).return_mapper();
                if return_mapper.is_some() {
                    // For other purposes (e.g. determining whether to produce literal types) we only
                    // incorporate inferences made from the return type in a function call. We remove
                    // the 'boolean' type from the contextual type such that contextually typed boolean
                    // literals actually end up widening to 'boolean' (see #48363).
                    let t = self.instantiate_instantiable_types(contextual_type, return_mapper);
                    if !self.ty(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                        if self.ty(t).flags.intersects(TypeFlags::UNION) {
                            let types = self.ty(t).types_list();
                            let (regular_false_type, regular_true_type) =
                                (self.regular_false_type, self.regular_true_type);
                            if self.contains_type(&types, regular_false_type)
                                && self.contains_type(&types, regular_true_type)
                            {
                                return self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
                                    t != c.regular_false_type && t != c.regular_true_type
                                });
                            }
                        }
                        return t;
                    }
                }
            }
        }
        contextual_type
    }
}
