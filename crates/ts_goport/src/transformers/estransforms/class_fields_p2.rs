//! Port of Go `transformers/estransforms/classfields.go` lines 1592 to the
//! end: `createPrivateIdentifierAssignment` through
//! `createAccessorPropertySetRedirector`. The first part and the free helpers
//! `flattenCommaList`, `findComputedPropertyNameCacheAssignment` and
//! `expandPreOrPostfixIncrementOrDecrementExpression` are in
//! `class_fields.rs`.

use super::class_fields::{
    CLASS_FACTS_CLASS_WAS_DECORATED, CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE,
    CLASS_FACTS_NEEDS_CLASS_SUPER_REFERENCE,
    CLASS_FACTS_NEEDS_SUBSTITUTION_FOR_THIS_IN_CLASS_STATIC_FIELD, CLASS_FACTS_NONE,
    CLASS_FACTS_WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR, ClassFacts, ClassFieldsTransformer,
    ClassLexicalEnv, ClassLexicalEnvironment, ClassLexicalEnvironmentRef, PrivateEnvironment,
    PrivateEnvironmentRef, PrivateIdentifierInfo, PrivateIdentifierInfoRef,
    SUBTREE_CONTAINS_LEXICAL_THIS_OR_SUPER, find_computed_property_name_cache_assignment,
    reserved_in_nested_scopes,
};
use super::class_this::is_class_this_assignment_block;
use super::named_evaluation::{
    class_has_explicitly_assigned_name, is_class_named_evaluation_helper_block,
    transform_named_evaluation,
};
use super::utilities::TxVisitors;
use crate::prelude::*;
use crate::printer::factory::{NodeFactory, PrivateIdentifierKind};
use crate::printer::{AutoGenerateOptions, EmitContext, EmitFlags, GeneratedIdentifierFlags};
use crate::transformers::modifier_visitor::extract_modifiers;
use crate::transformers::utilities::{
    find_super_statement_index_path, get_non_assignment_operator_for_compound_assignment,
    is_generated_identifier, is_simple_copiable_expression, is_simple_inlineable_expression,
    move_range_past_modifiers,
};

impl ClassFieldsTransformer {
    // Go: transformers/estransforms/classfields.go:1627 classFieldsTransformer.createPrivateIdentifierAssignment
    pub(super) fn create_private_identifier_assignment(
        &mut self,
        info: &PrivateIdentifierInfoRef,
        receiver: Node,
        right: Node,
        operator: SyntaxKind,
    ) -> Node {
        let mut receiver = self.visit_node(receiver);
        let mut right = self.visit_node(right);
        let ec = self.ec();
        let f = ec.factory();

        if is_compound_assignment(operator) {
            let (read_expression, initialize_expression) =
                self.create_copiable_receiver_expr(receiver);
            receiver = if initialize_expression.is_some() {
                initialize_expression
            } else {
                read_expression
            };
            right = f.new_binary_expression(
                ModifierList::NIL,
                self.create_private_identifier_access_helper(info, read_expression),
                Node::NIL,
                f.new_token(get_non_assignment_operator_for_compound_assignment(
                    operator,
                )),
                right,
            );
        }

        ec.set_comment_range(receiver, TextRange::new(-1, receiver.end()));

        let info = info.borrow();
        match info.kind {
            PrivateIdentifierKind::ACCESSOR => f.new_class_private_field_set_helper(
                receiver,
                info.brand_check_identifier,
                right,
                info.kind,
                info.setter_name,
            ),
            PrivateIdentifierKind::METHOD => f.new_class_private_field_set_helper(
                receiver,
                info.brand_check_identifier,
                right,
                info.kind,
                Node::NIL,
            ),
            PrivateIdentifierKind::FIELD => {
                let fn_ = if info.is_static {
                    info.variable_name
                } else {
                    Node::NIL
                };
                f.new_class_private_field_set_helper(
                    receiver,
                    info.brand_check_identifier,
                    right,
                    info.kind,
                    fn_,
                )
            }
            PrivateIdentifierKind::UNTRANSFORMED => crate::gostd::debug::fail(
                "Access helpers should not be created for untransformed private elements",
            ),
            _ => {
                debug_assert!(false, "Unknown private element type");
                Node::NIL
            }
        }
    }

    // Go: transformers/estransforms/classfields.go:1686 classFieldsTransformer.getPrivateInstanceMethodsAndAccessors
    pub(super) fn get_private_instance_methods_and_accessors(&self, node: Node) -> Vec<Node> {
        node.members()
            .iter()
            .filter(|&m| is_non_static_method_or_accessor_with_private_name(m))
            .collect()
    }

    // Go: transformers/estransforms/classfields.go:1695 classFieldsTransformer.memberContainsConstructorReference
    /// memberContainsConstructorReference checks if a class member's body contains an identifier
    /// that resolves to the class declaration. Replaces Strada's resolver.hasNodeCheckFlag(member,
    /// NodeCheckFlags.ContainsConstructorReference) by walking the AST with the EmitResolver.
    /// Only checks member bodies (not computed property names), since computed property names
    /// are evaluated during class definition when the binding is still correct.
    pub(super) fn member_contains_constructor_reference(
        &self,
        member: Node,
        class_decl: Node,
    ) -> bool {
        let class_original = self.emit_context.most_original(class_decl);
        let class_name = get_name_of_declaration(class_decl);
        let resolver = self.resolver.clone();
        fn check(
            n: Node,
            class_original: Node,
            class_name: Node,
            resolver: &dyn crate::transformers::transformer::TransformReferenceResolver,
        ) -> bool {
            if is_identifier(n) && n != class_name {
                let decl = resolver.get_referenced_value_declaration(n);
                if decl == class_original {
                    return true;
                }
            }
            // For PropertyAccessExpression, only check the expression, not the name.
            // The .Name() is a property access name, not a value reference to the class.
            if is_property_access_expression(n) {
                return check(n.expression(), class_original, class_name, resolver);
            }
            n.for_each_child(|c| check(c, class_original, class_name, resolver))
        }
        let check = |n: Node| check(n, class_original, class_name, &*resolver);
        // Check only the body/initializer of the member, not the name (which may be
        // a computed property name that shouldn't trigger alias substitution).
        let body = member.body();
        if body.is_some() && check(body) {
            return true;
        }
        if is_property_declaration(member) {
            let init = member.initializer();
            if init.is_some() && check(init) {
                return true;
            }
        }
        false
    }

    // Go: transformers/estransforms/classfields.go:1738 classFieldsTransformer.classContainsConstructorReference
    /// classContainsConstructorReference checks if any member of a class contains
    /// references to the class's own constructor. Replaces Strada's
    /// resolver.hasNodeCheckFlag(node, NodeCheckFlags.ContainsConstructorReference).
    pub(super) fn class_contains_constructor_reference(&self, node: Node) -> bool {
        for member in node.members().iter() {
            if self.member_contains_constructor_reference(member, node) {
                return true;
            }
        }
        false
    }

    // Go: transformers/estransforms/classfields.go:1747 classFieldsTransformer.getClassFacts
    pub(super) fn get_class_facts(&self, node: Node) -> ClassFacts {
        let ec = &self.emit_context;
        let mut facts = CLASS_FACTS_NONE;

        let original = ec.most_original(node);
        if is_class_like(original)
            && class_or_constructor_parameter_is_decorated(
                self.legacy_decorators, /*useLegacyDecorators*/
                original,
            )
        {
            facts |= CLASS_FACTS_CLASS_WAS_DECORATED;
        }

        if self.should_transform_private_elements_or_class_static_blocks
            && (class_has_class_this_assignment(ec, node)
                || class_has_explicitly_assigned_name(ec, node))
        {
            facts |= CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
        }

        let mut contains_public_instance_fields = false;
        let mut contains_initialized_public_instance_fields = false;
        let mut contains_instance_private_elements = false;
        let mut contains_instance_auto_accessors = false;

        for member in node.members().iter() {
            if is_static(member) {
                if member.name().is_some()
                    && (is_private_identifier(member.name())
                        || is_auto_accessor_property_declaration(member))
                    && self.should_transform_private_elements_or_class_static_blocks
                {
                    facts |= CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
                } else if is_auto_accessor_property_declaration(member)
                    && self.should_transform_auto_accessors
                    && node.name().is_nil()
                    && ec.class_this(node).is_nil()
                {
                    facts |= CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
                }
                if is_property_declaration(member) || is_class_static_block_declaration(member) {
                    if self.should_transform_this_in_static_initializers
                        && member
                            .subtree_facts()
                            .intersects(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_THIS)
                    {
                        facts |= CLASS_FACTS_NEEDS_SUBSTITUTION_FOR_THIS_IN_CLASS_STATIC_FIELD;
                        if facts & CLASS_FACTS_CLASS_WAS_DECORATED == 0 {
                            facts |= CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
                        }
                    }
                    if self.should_transform_super_in_static_initializers
                        && member
                            .subtree_facts()
                            .intersects(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_SUPER)
                        && facts & CLASS_FACTS_CLASS_WAS_DECORATED == 0
                    {
                        facts |= CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE
                            | CLASS_FACTS_NEEDS_CLASS_SUPER_REFERENCE;
                    }
                }
            } else if !has_abstract_modifier(ec.most_original(member)) {
                if is_auto_accessor_property_declaration(member) {
                    contains_instance_auto_accessors = true;
                    contains_instance_private_elements = contains_instance_private_elements
                        || is_private_identifier_class_element_declaration(member);
                } else if is_private_identifier_class_element_declaration(member) {
                    contains_instance_private_elements = true;
                    if self.member_contains_constructor_reference(member, node) {
                        facts |= CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE;
                    }
                } else if is_property_declaration(member) {
                    contains_public_instance_fields = true;
                    contains_initialized_public_instance_fields =
                        contains_initialized_public_instance_fields
                            || member.initializer().is_some();
                }
            }
        }

        let will_hoist_initializers_to_constructor =
            (self.should_transform_initializers_using_define && contains_public_instance_fields)
                || (self.should_transform_initializers_using_set
                    && contains_initialized_public_instance_fields)
                || (self.should_transform_private_elements_or_class_static_blocks
                    && contains_instance_private_elements)
                || (self.should_transform_private_elements_or_class_static_blocks
                    && contains_instance_auto_accessors
                    && self.should_transform_auto_accessors);

        if will_hoist_initializers_to_constructor {
            facts |= CLASS_FACTS_WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR;
        }

        facts
    }

    // Go: transformers/estransforms/classfields.go:1815 classFieldsTransformer.visitExpressionWithTypeArgumentsInHeritageClause
    pub(super) fn visit_expression_with_type_arguments_in_heritage_clause(
        &mut self,
        node: Node,
    ) -> Node {
        let facts = self
            .lexical_environment_data()
            .map_or(CLASS_FACTS_NONE, |data| data.facts);
        if facts & CLASS_FACTS_NEEDS_CLASS_SUPER_REFERENCE != 0 {
            let ec = self.ec();
            let f = ec.factory();
            let temp = f.new_temp_variable_ex(reserved_in_nested_scopes());
            ec.add_variable_declaration(temp);
            self.get_class_lexical_environment()
                .borrow_mut()
                .super_class_reference = temp;
            let expression = self.visit_node(node.expression());
            return f.update_expression_with_type_arguments(
                node,
                f.new_assignment_expression(temp, expression),
                NodeList::NIL, /*typeArguments*/
            );
        }
        self.heritage_clause_visitor_visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:1835 classFieldsTransformer.visitInNewClassLexicalEnvironment
    pub(super) fn visit_in_new_class_lexical_environment(
        &mut self,
        node: Node,
        visitor: fn(&mut Self, Node, ClassFacts) -> Node,
    ) -> Node {
        let ec = self.ec();
        let saved_current_class_container = self.current_class_container;
        let saved_pending_expressions = std::mem::take(&mut self.pending_expressions);
        let saved_lexical_environment = self.lexical_environment.clone();
        self.current_class_container = node;
        self.start_class_lexical_environment();
        let original = ec.most_original(node);
        self.enclosing_class_declarations.insert(original);

        if self.should_transform_private_elements_or_class_static_blocks
            || self.node_has_transform_private_static_elements_flag(node)
        {
            let name = get_name_of_declaration(node);
            if name.is_some() && is_identifier(name) {
                self.get_private_identifier_environment()
                    .borrow_mut()
                    .data
                    .class_name = name;
            } else {
                let assigned_name = ec.assigned_name(node);
                if assigned_name.is_some() && is_string_literal(assigned_name) {
                    // If the assigned name has a textSourceNode that is an identifier, use it directly.
                    let text_source_node = ec.text_source(assigned_name);
                    if text_source_node.is_some() && is_identifier(text_source_node) {
                        self.get_private_identifier_environment()
                            .borrow_mut()
                            .data
                            .class_name = text_source_node;
                    } else if is_identifier_text(assigned_name.text(), LanguageVariant::STANDARD) {
                        // If the text is a valid identifier, create an identifier from it.
                        let prefix_name = ec.factory().new_identifier(assigned_name.text());
                        self.get_private_identifier_environment()
                            .borrow_mut()
                            .data
                            .class_name = prefix_name;
                    }
                }
            }
        }

        if self.should_transform_private_elements_or_class_static_blocks {
            let private_instance_methods_and_accessors =
                self.get_private_instance_methods_and_accessors(node);
            if !private_instance_methods_and_accessors.is_empty() {
                let weak_set_name = self.create_hoisted_variable_for_class(
                    "instances",
                    private_instance_methods_and_accessors[0].name(),
                    "",
                );
                self.get_private_identifier_environment()
                    .borrow_mut()
                    .data
                    .weak_set_name = weak_set_name;
            }
        }

        let facts = self.get_class_facts(node);
        if facts != CLASS_FACTS_NONE {
            self.get_class_lexical_environment().borrow_mut().facts = facts;
        }

        let result = visitor(self, node, facts);
        self.enclosing_class_declarations.remove(&original);
        self.end_class_lexical_environment();
        go_assert!(
            match (&self.lexical_environment, &saved_lexical_environment) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            }
        );
        self.current_class_container = saved_current_class_container;
        self.pending_expressions = saved_pending_expressions;
        self.lexical_environment = saved_lexical_environment;
        result
    }

    // Go: transformers/estransforms/classfields.go:1889 classFieldsTransformer.visitClassDeclaration
    pub(super) fn visit_class_declaration(&mut self, node: Node) -> Node {
        self.visit_in_new_class_lexical_environment(
            node,
            Self::visit_class_declaration_in_new_class_lexical_environment,
        )
    }

    // Go: transformers/estransforms/classfields.go:1893 classFieldsTransformer.visitClassDeclarationInNewClassLexicalEnvironment
    pub(super) fn visit_class_declaration_in_new_class_lexical_environment(
        &mut self,
        node: Node,
        facts: ClassFacts,
    ) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        // If a class has private static fields, or a static field has a `this` or `super` reference,
        // then we need to allocate a temp variable to hold on to that reference.
        let mut pending_class_reference_assignment = Node::NIL;
        if facts & CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE != 0 {
            // If we aren't transforming class static blocks, then we can't reuse `_classThis` since in
            // `class C { ... static { _classThis = ... } }; _classThis = C` the outer assignment would occur *after*
            // class static blocks evaluate and would overwrite the replacement constructor produced by class
            // decorators.

            // If we are transforming class static blocks, then we can reuse `_classThis` since the assignment
            // will be evaluated *before* the transformed static blocks are evaluated and thus won't overwrite
            // the replacement constructor.

            if self.should_transform_private_elements_or_class_static_blocks
                && ec.class_this(node).is_some()
            {
                let class_this = ec.class_this(node);
                self.get_class_lexical_environment()
                    .borrow_mut()
                    .class_constructor = class_this;
                pending_class_reference_assignment =
                    f.new_assignment_expression(class_this, f.get_local_name(node));
            } else {
                let temp = f.new_temp_variable_ex(reserved_in_nested_scopes());
                ec.add_variable_declaration(temp);
                self.get_class_lexical_environment()
                    .borrow_mut()
                    .class_constructor = f.clone_node(temp);
                pending_class_reference_assignment =
                    f.new_assignment_expression(temp, f.get_local_name(node));
            }
        }

        if ec.class_this(node).is_some() {
            self.get_class_lexical_environment().borrow_mut().class_this = ec.class_this(node);
        }

        let is_class_with_constructor_reference = self.class_contains_constructor_reference(node);

        // Register class alias BEFORE visiting members (Strada registers after, since its
        // onSubstituteNode runs at emit time; we substitute eagerly during transformation).
        let alias = self
            .get_class_lexical_environment()
            .borrow()
            .class_constructor;
        if is_class_with_constructor_reference && alias.is_some() {
            self.class_aliases.insert(ec.most_original(node), alias);
        }

        let mut modifiers = self.modifier_visitor_visit_modifiers(node.modifiers());
        let heritage_clauses = self.with_visitor(Self::visit_heritage_clause, |v| {
            v.visit_nodes(node.heritage_clauses())
        });
        let (members, members_prologue) = self.transform_class_members(node);

        let mut statements: Vec<Node> = Vec::new();

        if pending_class_reference_assignment.is_some() {
            self.pending_expressions
                .insert(0, pending_class_reference_assignment);
        }

        // Write any pending expressions from elided or moved computed property names
        if !self.pending_expressions.is_empty() {
            statements
                .push(f.new_expression_statement(f.inline_expressions(&self.pending_expressions)));
        }

        // A class declaration without a name needs a generated name if it has static
        // initialized properties, since those will be moved outside the class body and
        // need to reference the class by name.
        let mut name = node.name();

        if self.should_transform_initializers_using_set
            || self.should_transform_private_elements_or_class_static_blocks
        {
            // Emit static property assignment. Because classDeclaration is lexically evaluated,
            // it is safe to emit static property assignment after classDeclaration
            // From ES6 specification:
            //   HasLexicalDeclaration (N) : Determines if the argument identifier has a binding in this environment record that was created using
            //                               a lexical declaration such as a LexicalDeclaration or a ClassDeclaration.
            let static_properties = self.get_static_properties_and_class_static_block(node);
            if !static_properties.is_empty() {
                if name.is_nil() {
                    name = f.new_generated_name_for_node(node);
                }
                statements = self.add_property_or_class_static_block_statements(
                    statements,
                    &static_properties,
                    f.get_local_name(node),
                );
            }
        }

        let is_export = has_syntactic_modifier(node, ModifierFlags::EXPORT);
        let is_default = has_syntactic_modifier(node, ModifierFlags::DEFAULT);

        if !statements.is_empty() && is_export && is_default {
            modifiers = extract_modifiers(&ec, modifiers, !ModifierFlags::EXPORT_DEFAULT);
            let export_assignment = f.new_export_assignment(
                ModifierList::NIL,
                false,     /*isExportEquals*/
                Node::NIL, /*typeNode*/
                f.get_local_name(node),
            );
            statements.push(export_assignment);
        }

        let updated_class = f.update_class_declaration(
            node,
            modifiers,
            name,
            NodeList::NIL, /*typeParameters*/
            heritage_clauses,
            members,
        );

        let mut result: Vec<Node> = Vec::with_capacity(1 + statements.len() + 1);
        if members_prologue.is_some() {
            result.push(f.new_expression_statement(members_prologue));
        }
        result.push(updated_class);
        result.extend(statements);
        f.new_syntax_list(&result)
    }

    // Go: transformers/estransforms/classfields.go:2003 classFieldsTransformer.visitClassExpression
    pub(super) fn visit_class_expression(&mut self, node: Node) -> Node {
        self.visit_in_new_class_lexical_environment(
            node,
            Self::visit_class_expression_in_new_class_lexical_environment,
        )
    }

    // Go: transformers/estransforms/classfields.go:2007 classFieldsTransformer.visitClassExpressionInNewClassLexicalEnvironment
    pub(super) fn visit_class_expression_in_new_class_lexical_environment(
        &mut self,
        node: Node,
        facts: ClassFacts,
    ) -> Node {
        let ec = self.ec();
        let f = ec.factory();

        // If this class expression is a transformation of a decorated class declaration,
        // then we want to output the pendingExpressions as statements, not as inlined
        // expressions with the class statement.
        //
        // In this case, we use pendingStatements to produce the same output as the
        // class declaration transformation. The VariableStatement visitor will insert
        // these statements after the class expression variable statement.
        let is_decorated_class_declaration = facts & CLASS_FACTS_CLASS_WAS_DECORATED != 0;

        if ec.class_this(node).is_some() {
            self.get_class_lexical_environment().borrow_mut().class_this = ec.class_this(node);
        }

        let mut temp = Node::NIL;
        if facts & CLASS_FACTS_NEEDS_CLASS_CONSTRUCTOR_REFERENCE != 0 {
            if (self.should_transform_private_elements_or_class_static_blocks
                || self.node_has_transform_private_static_elements_flag(node))
                && ec.class_this(node).is_some()
            {
                let class_this = ec.class_this(node);
                self.get_class_lexical_environment()
                    .borrow_mut()
                    .class_constructor = class_this;
                temp = class_this;
            } else {
                temp = f.new_temp_variable_ex(reserved_in_nested_scopes());
                if self.class_expression_needs_block_scoped_temp() {
                    ec.add_lexical_declaration(temp);
                } else {
                    ec.add_variable_declaration(temp);
                }
                self.get_class_lexical_environment()
                    .borrow_mut()
                    .class_constructor = f.clone_node(temp);
            }
        }

        let static_properties_or_class_static_blocks =
            self.get_static_properties_and_class_static_block(node);

        // Pre-compute whether the class expression will need a temp variable wrapper.
        // Strada registers class aliases AFTER transformClassMembers (since onSubstituteNode runs
        // at emit time), but we must predict this before visiting members since we substitute
        // eagerly. This requires pre-detecting willHavePrivatePendingExpressions.
        let mut is_class_with_constructor_reference = false;
        let mut has_transformable_statics = false;
        let mut defer_temp_declaration = false;
        if !is_decorated_class_declaration {
            is_class_with_constructor_reference = self.class_contains_constructor_reference(node);
            has_transformable_statics = (self
                .should_transform_private_elements_or_class_static_blocks
                || self.node_has_transform_private_static_elements_flag(node))
                && static_properties_or_class_static_blocks.iter().any(|&n| {
                    is_class_static_block_declaration(n)
                        || is_private_identifier_class_element_declaration(n)
                        || (self.should_transform_initializers && is_initialized_property(n))
                });

            // Private instance elements (fields, methods, accessors) transformed to
            // WeakMap/WeakSet will add initialization expressions to pendingExpressions
            // during transformClassMembers. Pre-detect this so we know whether the class
            // will be wrapped with a temp variable.
            let will_have_private_pending_expressions = self
                .should_transform_private_elements_or_class_static_blocks
                && node.members().iter().any(|n| {
                    is_private_identifier_class_element_declaration(n)
                        && !has_static_modifier(n)
                        && self.should_transform_class_element_to_weak_map(n)
                });
            let will_need_temp_wrapper =
                has_transformable_statics || will_have_private_pending_expressions;

            // Register class alias BEFORE visiting members (Strada registers after, since its
            // onSubstituteNode runs at emit time). Only register when the class will be wrapped
            // with a temp, matching Strada's conditional registration.
            if is_class_with_constructor_reference
                && will_need_temp_wrapper
                && self
                    .get_class_lexical_environment()
                    .borrow()
                    .class_constructor
                    .is_nil()
            {
                // Create temp early so the alias is available during member visiting, even though in the Strada
                // reference the temp would be created later in the pendingExpressions branch.
                temp = f.new_temp_variable_ex(reserved_in_nested_scopes());
                // Defer AddVariableDeclaration to preserve Strada's variable declaration ordering.
                defer_temp_declaration = true;
                self.get_class_lexical_environment()
                    .borrow_mut()
                    .class_constructor = f.clone_node(temp);
            }
            let alias = self
                .get_class_lexical_environment()
                .borrow()
                .class_constructor;
            if is_class_with_constructor_reference && will_need_temp_wrapper && alias.is_some() {
                self.class_aliases.insert(ec.most_original(node), alias);
            }
        }

        let modifiers = self.modifier_visitor_visit_modifiers(node.modifiers());
        let heritage_clauses = self.with_visitor(Self::visit_heritage_clause, |v| {
            v.visit_nodes(node.heritage_clauses())
        });
        let (members, members_prologue) = self.transform_class_members(node);

        if defer_temp_declaration {
            if self.class_expression_needs_block_scoped_temp() {
                ec.add_lexical_declaration(temp);
            } else {
                ec.add_variable_declaration(temp);
            }
        }

        let class_expression = f.update_class_expression(
            node,
            modifiers,
            node.name(),
            NodeList::NIL, /*typeParameters*/
            heritage_clauses,
            members,
        );

        let mut expressions: Vec<Node> = Vec::new();
        if members_prologue.is_some() {
            expressions.push(members_prologue);
        }

        if !is_decorated_class_declaration {
            if has_transformable_statics || !self.pending_expressions.is_empty() {
                if temp.is_nil() {
                    temp = f.new_temp_variable_ex(reserved_in_nested_scopes());
                    if self.class_expression_needs_block_scoped_temp() {
                        ec.add_lexical_declaration(temp);
                    } else {
                        ec.add_variable_declaration(temp);
                    }
                    self.get_class_lexical_environment()
                        .borrow_mut()
                        .class_constructor = f.clone_node(temp);
                    if is_class_with_constructor_reference {
                        let class_constructor = self
                            .get_class_lexical_environment()
                            .borrow()
                            .class_constructor;
                        self.class_aliases
                            .insert(ec.most_original(node), class_constructor);
                    }
                }

                expressions.push(f.new_assignment_expression(temp, class_expression));

                // Add any pending expressions leftover from elided or relocated computed property names
                expressions.extend_from_slice(&self.pending_expressions);

                let initialized = self
                    .generate_initialized_property_expressions_or_class_static_block(
                        &static_properties_or_class_static_blocks,
                        temp,
                    );
                expressions.extend(initialized);
                expressions.push(f.clone_node(temp));
            } else {
                expressions.push(class_expression);
            }
        } else {
            // Decorated class declaration path: emit static properties as separate statements
            // via pendingStatements, matching the class declaration output structure.

            // Write any pending expressions from elided or moved computed property names
            if !self.pending_expressions.is_empty() {
                for expr in self.pending_expressions.clone() {
                    self.pending_statements
                        .push(f.new_expression_statement(expr));
                }
            }

            // Emit static properties as statements (via pendingStatements) using the class's
            // internal name as the receiver, matching the class declaration output structure.
            if !static_properties_or_class_static_blocks.is_empty() {
                let mut class_this_or_name = ec.class_this(node);
                if class_this_or_name.is_nil() {
                    class_this_or_name = f.get_local_name(node);
                }
                let pending_statements = std::mem::take(&mut self.pending_statements);
                self.pending_statements = self.add_property_or_class_static_block_statements(
                    pending_statements,
                    &static_properties_or_class_static_blocks,
                    class_this_or_name,
                );
            }

            if temp.is_some() {
                expressions.push(f.new_assignment_expression(temp, class_expression));
            } else if self.should_transform_private_elements_or_class_static_blocks
                && ec.class_this(node).is_some()
            {
                expressions
                    .push(f.new_assignment_expression(ec.class_this(node), class_expression));
            } else {
                expressions.push(class_expression);
            }
        }

        if expressions.len() > 1 {
            ec.add_emit_flags(class_expression, EmitFlags::INDENTED);
            for &expr in &expressions {
                ec.add_emit_flags(expr, EmitFlags::START_ON_NEW_LINE);
            }
        }
        f.inline_expressions(&expressions)
    }

    // Go: transformers/estransforms/classfields.go:2181 classFieldsTransformer.visitClassStaticBlockDeclaration
    pub(super) fn visit_class_static_block_declaration(&mut self, node: Node) -> Node {
        if !self.should_transform_private_elements_or_class_static_blocks {
            return self.visit_each_child(node);
        }
        // ClassStaticBlockDeclaration for classes are transformed in visitClassDeclaration/visitClassExpression.
        Node::NIL
    }

    // Go: transformers/estransforms/classfields.go:2194 classFieldsTransformer.visitThisExpression
    /// visitThisExpression replaces Strada's substituteThisExpression / onSubstituteNode.
    /// Strada substitutes `this` at emit time; we do it eagerly during transformation.
    ///
    /// The Strada noSubstitution set (ensureDynamicThisIfNeeded) is not needed because
    /// transformAutoAccessor() passes the receiver directly rather than emitting `this`.
    pub(super) fn visit_this_expression(&mut self, node: Node) -> Node {
        if self.inside_computed_property_name && self.should_transform_this_in_static_initializers {
            if let Some(data) = self.lexical_environment_data() {
                // Don't replace `this` in computed property names for ES-decorated classes.
                // The esDecorator transformer wraps them in an arrow IIFE where `this` already
                // refers to the correct outer scope.
                if data.facts & CLASS_FACTS_CLASS_WAS_DECORATED == 0 || self.legacy_decorators {
                    let class_this = self.try_get_class_this_no_container();
                    if class_this.is_some() {
                        return class_this;
                    }
                }
            }
        }
        if self.should_transform_this_in_static_initializers
            && self.current_class_element.is_some()
            && (is_class_static_block_declaration(self.current_class_element)
                || (is_property_declaration(self.current_class_element)
                    && has_static_modifier(self.current_class_element)))
            && let Some(data) = self.lexical_environment_data()
        {
            let class_this = self.try_get_class_this_no_container();
            if class_this.is_some() {
                return class_this;
            }
            // When the class was decorated with legacy decorators and no class constructor
            // reference is available, the decorator may replace the constructor, so `this`
            // cannot reliably point to the class. Use `(void 0)` instead.
            if data.facts & CLASS_FACTS_CLASS_WAS_DECORATED != 0 && self.legacy_decorators {
                let ec = self.ec();
                let f = ec.factory();
                return f.new_parenthesized_expression(f.new_void_zero_expression());
            }
        }
        node
    }

    // Go: transformers/estransforms/classfields.go:2223 classFieldsTransformer.transformClassMembers
    /// Returns `(members, prologue)`.
    pub(super) fn transform_class_members(&mut self, node: Node) -> (NodeList, Node) {
        let ec = self.ec();
        let f = ec.factory();
        let should_transform_private_static_elements_in_class = ec
            .emit_flags(node)
            .intersects(EmitFlags::TRANSFORM_PRIVATE_STATIC_ELEMENTS);
        let mut prologue = Node::NIL;

        // Declare private names
        if self.should_transform_private_elements_or_class_static_blocks
            || self.should_transform_private_static_elements_in_file
        {
            for member in node.members().iter() {
                if is_private_identifier_class_element_declaration(member) {
                    if self.should_transform_class_element_to_weak_map(member) {
                        self.add_private_identifier_to_environment(member);
                    } else {
                        let env = self.get_private_identifier_environment();
                        self.set_private_identifier(
                            &env,
                            member.name(),
                            PrivateIdentifierInfo::new(PrivateIdentifierKind::UNTRANSFORMED),
                        );
                    }
                }
            }

            if self.should_transform_private_elements_or_class_static_blocks
                && !self
                    .get_private_instance_methods_and_accessors(node)
                    .is_empty()
            {
                self.create_brand_check_weak_set_for_private_methods();
            }

            if self.should_transform_auto_accessors_in_current_class() {
                for member in node.members().iter() {
                    if is_auto_accessor_property_declaration(member) {
                        let storage_name = f.new_generated_private_name_for_node_ex(
                            member.name(),
                            AutoGenerateOptions {
                                suffix: "_accessor_storage".to_string(),
                                ..Default::default()
                            },
                        );
                        if self.should_transform_private_elements_or_class_static_blocks
                            || should_transform_private_static_elements_in_class
                                && has_static_modifier(member)
                        {
                            self.add_private_identifier_property_declaration_to_environment(
                                member,
                                storage_name,
                            );
                        } else {
                            let env = self.get_private_identifier_environment();
                            // Only register as untransformed if it hasn't already been registered
                            // by the first loop (e.g., if esDecorators expanded a private auto-accessor
                            // into a backing field with the same generated name).
                            if self.get_private_identifier(&env, storage_name).is_none() {
                                self.set_private_identifier(
                                    &env,
                                    storage_name,
                                    PrivateIdentifierInfo::new(
                                        PrivateIdentifierKind::UNTRANSFORMED,
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }

        let mut members = self.with_visitor(Self::visit_class_element, |v| {
            v.visit_nodes(node.member_list())
        });

        // Create a synthetic constructor if necessary
        let mut synthetic_constructor = Node::NIL;
        if !members.nodes().iter().any(is_constructor_declaration) {
            synthetic_constructor = self.transform_constructor(Node::NIL, node);
        }

        // If there are pending expressions create a class static block in which to evaluate them, but only if
        // class static blocks are not also being transformed. This block will be injected at the top of the class
        // to ensure that expressions from computed property names are evaluated before any other static
        // initializers.
        let mut synthetic_static_block = Node::NIL;
        if !self.should_transform_private_elements_or_class_static_blocks
            && !self.pending_expressions.is_empty()
        {
            let mut statement =
                f.new_expression_statement(f.inline_expressions(&self.pending_expressions));
            if statement
                .subtree_facts()
                .intersects(SUBTREE_CONTAINS_LEXICAL_THIS_OR_SUPER)
            {
                // If there are `this` or `super` references from computed property names, shift the expression
                // into an arrow function to be evaluated in the outer scope so that `this` and `super` are
                // properly captured.
                let temp = f.new_temp_variable();
                ec.add_variable_declaration(temp);
                let arrow = f.new_arrow_function(
                    ModifierList::NIL,                               /*modifiers*/
                    NodeList::NIL,                                   /*typeParameters*/
                    f.new_node_list(&[]),                            /*parameters*/
                    Node::NIL,                                       /*returnType*/
                    Node::NIL,                                       /*fullSignature*/
                    f.new_token(SyntaxKind::EqualsGreaterThanToken), /*equalsGreaterThanToken*/
                    f.new_block(f.new_node_list(&[statement]), false /*multiline*/),
                );
                prologue = f.new_assignment_expression(temp, arrow);
                statement = f.new_expression_statement(f.new_call_expression(
                    temp,
                    Node::NIL,     /*questionDotToken*/
                    NodeList::NIL, /*typeArguments*/
                    f.new_node_list(&[]),
                    NodeFlags::NONE,
                ));
            }

            let block = f.new_block(f.new_node_list(&[statement]), false /*multiline*/);
            synthetic_static_block =
                f.new_class_static_block_declaration(ModifierList::NIL /*modifiers*/, block);
            self.pending_expressions = Vec::new();
        }

        // If we created a synthetic constructor or class static block, add them to the visited members
        if synthetic_constructor.is_some() || synthetic_static_block.is_some() {
            let member_nodes = members.nodes().to_vec();
            let mut members_array: Vec<Node> = Vec::with_capacity(member_nodes.len() + 2);

            // Find and preserve classThis assignment block and named evaluation helper block at the top
            let class_this_idx = member_nodes
                .iter()
                .position(|&n| is_class_this_assignment_block(&ec, n));
            let named_eval_idx = member_nodes
                .iter()
                .position(|&n| is_class_named_evaluation_helper_block(&ec, n));

            if let Some(i) = class_this_idx {
                members_array.push(member_nodes[i]);
            }
            if let Some(i) = named_eval_idx {
                members_array.push(member_nodes[i]);
            }
            if synthetic_constructor.is_some() {
                members_array.push(synthetic_constructor);
            }
            if synthetic_static_block.is_some() {
                members_array.push(synthetic_static_block);
            }

            for (i, &member) in member_nodes.iter().enumerate() {
                if Some(i) != class_this_idx && Some(i) != named_eval_idx {
                    members_array.push(member);
                }
            }
            members = f.new_node_list_with_loc(&members_array, node.member_list().loc());
        }

        (members, prologue)
    }

    // Go: transformers/estransforms/classfields.go:2348 classFieldsTransformer.createBrandCheckWeakSetForPrivateMethods
    pub(super) fn create_brand_check_weak_set_for_private_methods(&mut self) {
        let env = self.get_private_identifier_environment();
        let weak_set_name = env.borrow().data.weak_set_name;
        go_assert!(
            weak_set_name.is_some(),
            "weakSetName should be set in private identifier environment"
        );

        let ec = self.ec();
        let f = ec.factory();
        self.add_pending_expressions(&[f.new_assignment_expression(
            weak_set_name,
            f.new_new_expression(
                f.new_identifier("WeakSet"),
                NodeList::NIL, /*typeArguments*/
                f.new_node_list(&[]),
            ),
        )]);
    }

    // Go: transformers/estransforms/classfields.go:2365 classFieldsTransformer.transformConstructor
    pub(super) fn transform_constructor(&mut self, constructor: Node, container: Node) -> Node {
        // NOTE: The Strada reference pre-visits the constructor via `visitNode(constructor, visitor)` before
        // checking WillHoistInitializersToConstructor. This is not done here because Go's variable environment
        // (StartVariableEnvironment/EndAndMergeVariableEnvironment) is scoped inside transformConstructorBody.
        // Pre-visiting would hoist variables outside that scope, causing them to appear after field initializers
        // instead of before. Instead, we visit parameters and body separately within the correct scopes.
        if !self.lexical_environment_data().is_some_and(|data| {
            data.facts & CLASS_FACTS_WILL_HOIST_INITIALIZERS_TO_CONSTRUCTOR != 0
        }) {
            if constructor.is_some() {
                return self.visit_each_child(constructor);
            }
            return Node::NIL;
        }

        let extends_clause_element = get_class_extends_heritage_element(container);
        let is_derived_class = extends_clause_element.is_some()
            && skip_outer_expressions(
                extends_clause_element.expression(),
                OuterExpressionKinds::OEK_ALL,
            )
            .kind()
                != SyntaxKind::NullKeyword;

        let mut parameters = NodeList::NIL;
        if constructor.is_some() {
            parameters = self.visit_nodes(constructor.parameter_list());
        }

        let body = self.transform_constructor_body(container, constructor, is_derived_class);
        if body.is_nil() {
            if constructor.is_some() {
                return self.visit_each_child(constructor);
            }
            return Node::NIL;
        }

        let ec = self.ec();
        let f = ec.factory();
        if constructor.is_some() {
            go_assert!(parameters.is_some());
            return f.update_constructor_declaration(
                constructor,
                ModifierList::NIL, /*modifiers*/
                NodeList::NIL,     /*typeParameters*/
                parameters,
                Node::NIL, /*returnType*/
                Node::NIL, /*fullSignature*/
                body,
            );
        }

        if parameters.is_nil() {
            parameters = f.new_node_list(&[]);
        }

        let result = f.new_constructor_declaration(
            ModifierList::NIL, /*modifiers*/
            NodeList::NIL,     /*typeParameters*/
            parameters,
            Node::NIL, /*returnType*/
            Node::NIL, /*fullSignature*/
            body,
        );
        set_node_loc(result, container.loc());
        result
    }

    // Go: transformers/estransforms/classfields.go:2424 classFieldsTransformer.transformConstructorBodyWorker
    #[allow(clippy::too_many_arguments)]
    pub(super) fn transform_constructor_body_worker(
        &mut self,
        mut statements_out: Vec<Node>,
        statements_in: &[Node],
        mut statement_offset: usize,
        super_path: &[usize],
        super_path_depth: usize,
        initializer_statements: &[Node],
        constructor: Node,
    ) -> Vec<Node> {
        let ec = self.ec();
        let f = ec.factory();
        let super_statement_index = super_path[super_path_depth];
        let super_statement = statements_in[super_statement_index];

        // Visit statements before super
        let (visited, _) =
            self.visit_slice(&statements_in[statement_offset..super_statement_index]);
        statements_out.extend(visited);
        statement_offset = super_statement_index + 1;

        if is_try_statement(super_statement) {
            let try_block = super_statement.try_block();
            let try_block_statements = self.transform_constructor_body_worker(
                Vec::new(),
                &try_block.statements().to_vec(),
                0, /*statementOffset*/
                super_path,
                super_path_depth + 1,
                initializer_statements,
                constructor,
            );
            let try_statement_list =
                f.new_node_list_with_loc(&try_block_statements, try_block.statement_list().loc());

            let catch_clause = self.visit_node(super_statement.catch_clause());
            let finally_block = self.visit_node(super_statement.finally_block());

            let updated = f.update_try_statement(
                super_statement,
                f.update_block(try_block, try_statement_list, try_block.multi_line()),
                catch_clause,
                finally_block,
            );
            statements_out.push(updated);
        } else {
            let (visited, _) =
                self.visit_slice(&statements_in[super_statement_index..super_statement_index + 1]);
            statements_out.extend(visited);

            // Add the property initializers. Transforms this:
            //
            //  public x = 1;
            //
            // Into this:
            //
            //  constructor() {
            //      this.x = 1;
            //  }
            //
            // If we do useDefineForClassFields, they'll be converted elsewhere.
            // We instead *remove* them from the transformed output at this stage.

            // parameter-property assignments should occur immediately after the prologue and `super()`,
            // so only count the statements that immediately follow.
            while statement_offset < statements_in.len() {
                let stmt = statements_in[statement_offset];
                let orig = ec.most_original(stmt);
                if is_parameter_property_declaration(orig, constructor) {
                    statement_offset += 1;
                } else {
                    break;
                }
            }

            statements_out.extend_from_slice(initializer_statements);
        }

        // Visit remaining statements
        let (visited2, _) = self.visit_slice(&statements_in[statement_offset..]);
        statements_out.extend(visited2);
        statements_out
    }

    // Go: transformers/estransforms/classfields.go:2503 classFieldsTransformer.transformConstructorBody
    pub(super) fn transform_constructor_body(
        &mut self,
        container: Node,
        constructor: Node,
        is_derived_class: bool,
    ) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        let instance_properties = self.get_properties(
            container, false, /*requireInitializer*/
            false, /*isStatic*/
        );
        let mut properties = instance_properties.clone();
        if !self.compiler_options.get_use_define_for_class_fields() {
            properties.retain(|&prop| {
                prop.initializer().is_some()
                    || is_private_identifier(prop.name())
                    || has_accessor_modifier(prop)
            });
        }

        let private_methods_and_accessors =
            self.get_private_instance_methods_and_accessors(container);
        let needs_constructor_body =
            !properties.is_empty() || !private_methods_and_accessors.is_empty();

        // Only generate synthetic constructor when there are property initializers to move.
        if constructor.is_nil() && !needs_constructor_body {
            return self.visit_function_body(Node::NIL);
        }

        ec.start_variable_environment();

        let needs_synthetic_constructor = constructor.is_nil() && is_derived_class;
        let mut statements: Vec<Node> = Vec::new();

        // Add the property initializers. Transforms this:
        //
        //  public x = 1;
        //
        // Into this:
        //
        //  constructor() {
        //      this.x = 1;
        //  }
        //
        let mut initializer_statements: Vec<Node> = Vec::new();
        let receiver = f.new_this_expression();

        // private methods can be called in property initializers, they should execute first
        initializer_statements = self.add_instance_method_statements(
            initializer_statements,
            &private_methods_and_accessors,
            receiver,
        );

        if constructor.is_some() {
            let parameter_properties: Vec<Node> = instance_properties
                .iter()
                .copied()
                .filter(|&prop| {
                    is_parameter_property_declaration(ec.most_original(prop), constructor)
                })
                .collect();
            let non_parameter_properties: Vec<Node> = properties
                .iter()
                .copied()
                .filter(|&prop| {
                    !is_parameter_property_declaration(ec.most_original(prop), constructor)
                })
                .collect();
            initializer_statements = self.add_property_or_class_static_block_statements(
                initializer_statements,
                &parameter_properties,
                receiver,
            );
            initializer_statements = self.add_property_or_class_static_block_statements(
                initializer_statements,
                &non_parameter_properties,
                receiver,
            );
        } else {
            initializer_statements = self.add_property_or_class_static_block_statements(
                initializer_statements,
                &properties,
                receiver,
            );
        }

        if constructor.is_some() && constructor.body().is_some() {
            let body = constructor.body();
            let body_statements = body.statements().to_vec();

            // Copy prologue
            for &stmt in &body_statements {
                if is_prologue_directive(stmt) {
                    statements.push(stmt);
                } else {
                    break;
                }
            }
            let mut statement_offset = statements.len();

            let super_path = find_super_statement_index_path(&body_statements, statement_offset);
            if !super_path.is_empty() {
                statements = self.transform_constructor_body_worker(
                    statements,
                    &body_statements,
                    statement_offset,
                    &super_path,
                    0,
                    &initializer_statements,
                    constructor,
                );
            } else {
                // parameter-property assignments should occur immediately after the prologue and `super()`,
                // so only count the statements that immediately follow.
                while statement_offset < body_statements.len() {
                    let stmt = body_statements[statement_offset];
                    let orig = ec.most_original(stmt);
                    if is_parameter_property_declaration(orig, constructor) {
                        statement_offset += 1;
                    } else {
                        break;
                    }
                }
                statements.extend_from_slice(&initializer_statements);
                let (visited, _) = self.visit_slice(&body_statements[statement_offset..]);
                statements.extend(visited);
            }
        } else {
            if needs_synthetic_constructor {
                // Add a synthetic `super` call:
                //
                //  super(...arguments);
                //
                let super_call = f.new_expression_statement(f.new_call_expression(
                    f.new_keyword_expression(SyntaxKind::SuperKeyword),
                    Node::NIL,     /*typeArguments*/
                    NodeList::NIL, /*questionDotToken*/
                    f.new_node_list(&[f.new_spread_element(f.new_identifier("arguments"))]),
                    NodeFlags::NONE,
                ));
                statements.push(super_call);
            }
            statements.extend_from_slice(&initializer_statements);
        }

        let statements = ec.end_and_merge_variable_environment(&statements);

        if statements.is_empty() && constructor.is_nil() {
            return Node::NIL;
        }

        let multi_line = if constructor.is_some()
            && constructor.body().is_some()
            && constructor.body().statements().len() >= statements.len()
        {
            constructor.body().multi_line()
        } else {
            !statements.is_empty()
        };

        let statement_list_loc = if constructor.is_some() && constructor.body().is_some() {
            constructor.body().statement_list().loc()
        } else {
            TextRange::new(
                container.member_list().loc().pos(),
                container.member_list().loc().end(),
            )
        };
        let statement_list = f.new_node_list_with_loc(&statements, statement_list_loc);

        let block = f.new_block(statement_list, multi_line);
        if constructor.is_some() && constructor.body().is_some() {
            set_node_loc(block, constructor.body().loc());
        }
        block
    }

    // Go: transformers/estransforms/classfields.go:2637 classFieldsTransformer.addPropertyOrClassStaticBlockStatements
    /// addPropertyOrClassStaticBlockStatements generates assignment statements for property initializers.
    pub(super) fn add_property_or_class_static_block_statements(
        &mut self,
        mut statements: Vec<Node>,
        properties: &[Node],
        receiver: Node,
    ) -> Vec<Node> {
        for &property in properties {
            if is_static(property) && !self.should_transform_private_elements_or_class_static_blocks
            {
                continue;
            }
            let statement = self.transform_property_or_class_static_block(property, receiver);
            if statement.is_some() {
                statements.push(statement);
            }
        }
        statements
    }

    // Go: transformers/estransforms/classfields.go:2650 classFieldsTransformer.transformPropertyOrClassStaticBlock
    pub(super) fn transform_property_or_class_static_block(
        &mut self,
        property: Node,
        receiver: Node,
    ) -> Node {
        let expression = if is_class_static_block_declaration(property) {
            self.set_current_class_element_and(
                property,
                Self::transform_class_static_block_declaration,
                property,
            )
        } else {
            self.transform_property(property, receiver)
        };
        if expression.is_nil() {
            return Node::NIL;
        }

        let ec = self.ec();
        let f = ec.factory();
        let statement = f.new_expression_statement(expression);
        ec.set_original(statement, property);
        ec.add_emit_flags(statement, ec.emit_flags(property) & EmitFlags::NO_COMMENTS);
        ec.set_comment_range(statement, property.loc());

        let property_original_node = ec.most_original(property);
        if is_parameter_declaration(property_original_node) {
            ec.set_source_map_range(statement, property_original_node.loc());
            ec.add_emit_flags(statement, EmitFlags::NO_COMMENTS);
        } else {
            ec.set_source_map_range(statement, move_range_past_modifiers(property));
        }

        // `setOriginalNode` *copies* the `emitNode` from `property`, so now both
        // `statement` and `expression` have a copy of the synthesized comments.
        // Drop the comments from expression to avoid printing them twice.
        ec.set_synthetic_leading_comments(expression, Vec::new());
        ec.set_synthetic_trailing_comments(expression, Vec::new());

        // If the property was originally an auto-accessor, don't emit comments here since they will be attached to
        // the synthesized getter.
        if has_accessor_modifier(property_original_node) {
            ec.add_emit_flags(statement, EmitFlags::NO_COMMENTS);
        }

        statement
    }

    // Go: transformers/estransforms/classfields.go:2690 classFieldsTransformer.generateInitializedPropertyExpressionsOrClassStaticBlock
    /// generateInitializedPropertyExpressionsOrClassStaticBlock generates assignment expressions for property initializers.
    pub(super) fn generate_initialized_property_expressions_or_class_static_block(
        &mut self,
        properties_or_class_static_blocks: &[Node],
        receiver: Node,
    ) -> Vec<Node> {
        let ec = self.ec();
        let mut expressions: Vec<Node> = Vec::new();
        for &property in properties_or_class_static_blocks {
            let expression = if is_class_static_block_declaration(property) {
                self.set_current_class_element_and(
                    property,
                    Self::transform_class_static_block_declaration,
                    property,
                )
            } else {
                self.transform_property(property, receiver)
            };
            if expression.is_nil() {
                continue;
            }
            ec.set_original_ex(expression, property, true /*allowOverwrite*/);
            ec.assign_comment_and_source_map_ranges(expression, property);
            expressions.push(expression);
        }
        expressions
    }

    // Go: transformers/estransforms/classfields.go:2713 classFieldsTransformer.transformProperty
    /// transformProperty transforms a property initializer into an assignment expression.
    pub(super) fn transform_property(&mut self, property: Node, receiver: Node) -> Node {
        let ec = self.ec();
        let saved_current_class_element = self.current_class_element;
        let transformed = self.transform_property_worker(property, receiver);
        if transformed.is_some() && has_static_modifier(property) {
            ec.add_emit_flags(transformed, EmitFlags::NO_LEXICAL_THIS);
        }
        if transformed.is_some()
            && has_static_modifier(property)
            && self
                .lexical_environment_data()
                .is_some_and(|data| data.facts != 0)
        {
            // capture the lexical environment for the member
            ec.set_original(transformed, property);
            ec.set_source_map_range(transformed, ec.source_map_range(property.name()));
        }
        self.current_class_element = saved_current_class_element;
        transformed
    }

    // Go: transformers/estransforms/classfields.go:2729 classFieldsTransformer.transformPropertyWorker
    pub(super) fn transform_property_worker(&mut self, mut property: Node, receiver: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        // We generate a name here in order to reuse the value cached by the relocated computed name expression (which uses the same generated name)
        let emit_assignment = !self.compiler_options.get_use_define_for_class_fields();

        if self.is_named_evaluation_needing_assigned_name(property) {
            property = transform_named_evaluation(&ec, property, false, "");
        }

        let mut property_name = property.name();
        if has_accessor_modifier(property) {
            property_name = f.new_generated_private_name_for_node_ex(
                property.name(),
                AutoGenerateOptions {
                    suffix: "_accessor_storage".to_string(),
                    ..Default::default()
                },
            );
        } else if is_computed_property_name(property_name)
            && !is_simple_inlineable_expression(property_name.expression())
        {
            property_name = f.update_computed_property_name(
                property_name,
                f.new_generated_name_for_node(property_name),
            );
        }

        if has_static_modifier(property) {
            self.current_class_element = property;
        }

        if is_private_identifier(property_name)
            && self.should_transform_class_element_to_weak_map(property)
        {
            let info = self.access_private_identifier(property_name);
            if let Some(info) = info {
                let (kind, is_static, brand_check_identifier, variable_name) = {
                    let i = info.borrow();
                    (
                        i.kind,
                        i.is_static,
                        i.brand_check_identifier,
                        i.variable_name,
                    )
                };
                if kind == PrivateIdentifierKind::FIELD {
                    if !is_static {
                        let initializer = self.visit_node(property.initializer());
                        return create_private_instance_field_initializer(
                            f,
                            receiver,
                            initializer,
                            brand_check_identifier,
                        );
                    }
                    let initializer = self.visit_node(property.initializer());
                    return create_private_static_field_initializer(f, variable_name, initializer);
                }
                return Node::NIL;
            } else {
                crate::gostd::debug::fail("Undeclared private name for property declaration.");
            }
        }

        if (is_private_identifier(property_name) || has_static_modifier(property))
            && property.initializer().is_nil()
        {
            return Node::NIL;
        }

        // TODO: can we get rid of this original checking and better coordinate with runtimesyntax?
        if has_abstract_modifier(ec.most_original(property)) {
            return Node::NIL;
        }

        let mut initializer = self.visit_node(property.initializer());
        let property_original_node = ec.most_original(property);
        if is_parameter_property_declaration(
            property_original_node,
            property_original_node.parent(),
        ) && is_identifier(property_name)
        {
            // A parameter-property declaration always overrides the initializer. The only time a parameter-property
            // declaration *should* have an initializer is when decorators have added initializers that need to run before
            // any other initializer
            let local_name = f.clone_node(property_name);
            if initializer.is_some() {
                // unwrap `(__runInitializers(this, _instanceExtraInitializers), void 0)`
                if is_parenthesized_expression(initializer)
                    && is_comma_expression(initializer.expression())
                    && ec.is_call_to_helper(initializer.expression().left(), "__runInitializers")
                    && is_void_expression(initializer.expression().right())
                    && is_numeric_literal(initializer.expression().right().expression())
                {
                    initializer = initializer.expression().left();
                }
                initializer = f.inline_expressions(&[initializer, local_name]);
            } else {
                initializer = local_name;
            }
            ec.add_emit_flags(
                property_name,
                EmitFlags::NO_COMMENTS | EmitFlags::NO_SOURCE_MAP,
            );
            ec.set_source_map_range(local_name, property_original_node.name().loc());
            ec.add_emit_flags(local_name, EmitFlags::NO_COMMENTS);
        } else if initializer.is_nil() {
            initializer = f.new_void_zero_expression();
        }

        if emit_assignment || is_private_identifier(property_name) {
            let member_access = create_member_access_for_property_name(
                f,
                &ec,
                receiver,
                property_name,
                property_name,
            );
            ec.add_emit_flags(member_access, EmitFlags::NO_LEADING_COMMENTS);
            return f.new_assignment_expression(member_access, initializer);
        }

        // useDefineForClassFields: Object.defineProperty
        let name = if is_computed_property_name(property_name) {
            property_name.expression()
        } else if is_identifier(property_name) {
            f.new_string_literal(property_name.text(), TokenFlags::NONE)
        } else {
            property_name
        };
        let descriptor = f.new_object_literal_expression(
            f.new_node_list(&[
                f.new_property_assignment(
                    ModifierList::NIL,
                    f.new_identifier("enumerable"),
                    Node::NIL,
                    Node::NIL,
                    f.new_true_expression(),
                ),
                f.new_property_assignment(
                    ModifierList::NIL,
                    f.new_identifier("configurable"),
                    Node::NIL,
                    Node::NIL,
                    f.new_true_expression(),
                ),
                f.new_property_assignment(
                    ModifierList::NIL,
                    f.new_identifier("writable"),
                    Node::NIL,
                    Node::NIL,
                    f.new_true_expression(),
                ),
                f.new_property_assignment(
                    ModifierList::NIL,
                    f.new_identifier("value"),
                    Node::NIL,
                    Node::NIL,
                    initializer,
                ),
            ]),
            true,
        );
        f.new_object_define_property_call(receiver, name, descriptor)
    }

    // Go: transformers/estransforms/classfields.go:2836 classFieldsTransformer.addInstanceMethodStatements
    /// addInstanceMethodStatements generates brand-check initializer for private methods.
    pub(super) fn add_instance_method_statements(
        &mut self,
        mut statements: Vec<Node>,
        methods: &[Node],
        receiver: Node,
    ) -> Vec<Node> {
        if !self.should_transform_private_elements_or_class_static_blocks || methods.is_empty() {
            return statements;
        }

        let env = self.get_private_identifier_environment();
        let weak_set_name = env.borrow().data.weak_set_name;
        go_assert!(
            weak_set_name.is_some(),
            "weakSetName should be set in private identifier environment"
        );

        let ec = self.ec();
        let f = ec.factory();
        statements.push(
            f.new_expression_statement(create_private_instance_method_initializer(
                f,
                receiver,
                weak_set_name,
            )),
        );
        statements
    }

    // Go: transformers/estransforms/classfields.go:2853 classFieldsTransformer.visitInvalidSuperProperty
    pub(super) fn visit_invalid_super_property(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        if is_property_access_expression(node) {
            return f.update_property_access_expression(
                node,
                f.new_void_zero_expression(),
                Node::NIL,
                node.name(),
                node.flags(),
            );
        }
        let argument = self.visit_node(node.argument_expression());
        f.update_element_access_expression(
            node,
            f.new_void_zero_expression(),
            Node::NIL,
            argument,
            node.flags(),
        )
    }

    // Go: transformers/estransforms/classfields.go:2876 classFieldsTransformer.getPropertyNameExpressionIfNeeded
    /// getPropertyNameExpressionIfNeeded transforms a computed property name, then either returns an expression
    /// which caches the value of the result or the expression itself if the value is either unused or safe to
    /// inline into multiple locations.
    /// shouldHoist indicates whether the expression needs to be reused (i.e., for an initializer or a decorator).
    pub(super) fn get_property_name_expression_if_needed(
        &mut self,
        name: Node,
        should_hoist: bool,
    ) -> Node {
        if !is_computed_property_name(name) {
            return Node::NIL;
        }
        let ec = self.ec();
        let f = ec.factory();
        let cache_assignment = find_computed_property_name_cache_assignment(&ec, name);
        // Switch to outer lex env for computed property name expressions, matching
        // Strada reference's onEmitNode behavior for ComputedPropertyName.
        let saved_lexical_environment = self.lexical_environment.clone();
        let saved_inside_computed_property_name = self.inside_computed_property_name;
        self.inside_computed_property_name = true;
        if let Some(previous) = self
            .lexical_environment
            .as_ref()
            .and_then(|env| env.previous.clone())
        {
            self.lexical_environment = Some(previous);
        }
        let expression = self.visit_node(name.expression());
        self.lexical_environment = saved_lexical_environment;
        self.inside_computed_property_name = saved_inside_computed_property_name;
        let inner_expression = skip_partially_emitted_expressions(expression);
        let inlinable = is_simple_inlineable_expression(inner_expression);
        let already_transformed = cache_assignment.is_some()
            || (is_assignment_expression(
                inner_expression,
                true, /*excludeCompoundAssignment*/
            ) && is_identifier(inner_expression.left())
                && is_generated_identifier(&ec, inner_expression.left()));
        if !already_transformed && !inlinable && should_hoist {
            let generated_name = f.new_generated_name_for_node(name);
            if self.requires_block_scoped_var() {
                ec.add_lexical_declaration(generated_name);
            } else {
                ec.add_variable_declaration(generated_name);
            }
            return f.new_assignment_expression(generated_name, expression);
        }
        if inlinable || is_identifier(inner_expression) {
            return Node::NIL;
        }
        expression
    }

    // Go: transformers/estransforms/classfields.go:2910 classFieldsTransformer.startClassLexicalEnvironment
    pub(super) fn start_class_lexical_environment(&mut self) {
        self.lexical_environment = Some(Rc::new(ClassLexicalEnv {
            previous: self.lexical_environment.take(),
            ..Default::default()
        }));
    }

    // Go: transformers/estransforms/classfields.go:2914 classFieldsTransformer.endClassLexicalEnvironment
    pub(super) fn end_class_lexical_environment(&mut self) {
        self.lexical_environment = self
            .lexical_environment
            .as_ref()
            .and_then(|env| env.previous.clone());
    }

    // Go: transformers/estransforms/classfields.go:2918 classFieldsTransformer.getClassLexicalEnvironment
    pub(super) fn get_class_lexical_environment(&mut self) -> ClassLexicalEnvironmentRef {
        let env = self
            .lexical_environment
            .as_ref()
            // Go: debug.Assert(tx.lexicalEnvironment != nil)
            .unwrap_or_else(|| crate::gostd::debug::assert_failed(None));
        let mut data = env.data.borrow_mut();
        data.get_or_insert_with(|| Rc::new(RefCell::new(ClassLexicalEnvironment::default())))
            .clone()
    }

    // Go: transformers/estransforms/classfields.go:2926 classFieldsTransformer.getPrivateIdentifierEnvironment
    pub(super) fn get_private_identifier_environment(&mut self) -> PrivateEnvironmentRef {
        let env = self
            .lexical_environment
            .as_ref()
            // Go: debug.Assert(tx.lexicalEnvironment != nil)
            .unwrap_or_else(|| crate::gostd::debug::assert_failed(None));
        let mut private_env = env.private_env.borrow_mut();
        private_env
            .get_or_insert_with(|| Rc::new(RefCell::new(PrivateEnvironment::default())))
            .clone()
    }

    // Go: transformers/estransforms/classfields.go:2936 classFieldsTransformer.addPendingExpressions
    pub(super) fn add_pending_expressions(&mut self, exprs: &[Node]) {
        self.pending_expressions.extend_from_slice(exprs);
    }

    // Go: transformers/estransforms/classfields.go:2940 classFieldsTransformer.addPrivateIdentifierPropertyDeclarationToEnvironment
    pub(super) fn add_private_identifier_property_declaration_to_environment(
        &mut self,
        node: Node,
        name: Node,
    ) {
        let lex = self.get_class_lexical_environment();
        let env = self.get_private_identifier_environment();
        let is_static = has_static_modifier(node);
        let previous_info = self.get_private_identifier(&env, name);
        let is_valid = !self.is_reserved_private_name(name) && previous_info.is_none();

        if is_static {
            let mut brand_check_identifier = lex.borrow().class_this;
            if brand_check_identifier.is_nil() {
                brand_check_identifier = lex.borrow().class_constructor;
            }
            let variable_name = self.create_hoisted_variable_for_private_name(name, "");
            self.set_private_identifier(
                &env,
                name,
                PrivateIdentifierInfo {
                    kind: PrivateIdentifierKind::FIELD,
                    is_static: true,
                    brand_check_identifier,
                    variable_name,
                    is_valid,
                    ..PrivateIdentifierInfo::new(PrivateIdentifierKind::FIELD)
                },
            );
        } else {
            let weak_map_name = self.create_hoisted_variable_for_private_name(name, "");
            self.set_private_identifier(
                &env,
                name,
                PrivateIdentifierInfo {
                    kind: PrivateIdentifierKind::FIELD,
                    is_static: false,
                    brand_check_identifier: weak_map_name,
                    is_valid,
                    ..PrivateIdentifierInfo::new(PrivateIdentifierKind::FIELD)
                },
            );
            let ec = self.ec();
            let f = ec.factory();
            self.add_pending_expressions(&[f.new_assignment_expression(
                weak_map_name,
                f.new_new_expression(
                    f.new_identifier("WeakMap"),
                    NodeList::NIL, /*typeArguments*/
                    f.new_node_list(&[]),
                ),
            )]);
        }
    }

    /// Go: `lex.classThis`, else `lex.classConstructor` (static), else `env.data.weakSetName`.
    pub(super) fn brand_check_identifier_for(
        lex: &ClassLexicalEnvironmentRef,
        env: &PrivateEnvironmentRef,
        is_static: bool,
    ) -> Node {
        if is_static {
            let lex = lex.borrow();
            let mut brand_check_identifier = lex.class_this;
            if brand_check_identifier.is_nil() {
                brand_check_identifier = lex.class_constructor;
            }
            go_assert!(
                brand_check_identifier.is_some(),
                "classConstructor should be set in private identifier environment"
            );
            brand_check_identifier
        } else {
            env.borrow().data.weak_set_name
        }
    }

    // Go: transformers/estransforms/classfields.go:2981 classFieldsTransformer.addPrivateIdentifierMethodToEnvironment
    pub(super) fn add_private_identifier_method_to_environment(
        &mut self,
        name: Node,
        lex: &ClassLexicalEnvironmentRef,
        env: &PrivateEnvironmentRef,
        is_static: bool,
        is_valid: bool,
    ) {
        let method_name = self.create_hoisted_variable_for_private_name(name, "");
        let brand_check_identifier = Self::brand_check_identifier_for(lex, env, is_static);
        self.set_private_identifier(
            env,
            name,
            PrivateIdentifierInfo {
                method_name,
                brand_check_identifier,
                is_static,
                is_valid,
                ..PrivateIdentifierInfo::new(PrivateIdentifierKind::METHOD)
            },
        );
    }

    // Go: transformers/estransforms/classfields.go:3002 classFieldsTransformer.addPrivateIdentifierGetAccessorToEnvironment
    pub(super) fn add_private_identifier_get_accessor_to_environment(
        &mut self,
        name: Node,
        lex: &ClassLexicalEnvironmentRef,
        env: &PrivateEnvironmentRef,
        is_static: bool,
        is_valid: bool,
        previous_info: Option<PrivateIdentifierInfoRef>,
    ) {
        let getter_name = self.create_hoisted_variable_for_private_name(name, "_get");
        let brand_check_identifier = Self::brand_check_identifier_for(lex, env, is_static);
        go_assert!(
            is_static || brand_check_identifier.is_some(),
            "weakSetName should be set in private identifier environment"
        );

        if let Some(previous_info) = previous_info.as_ref().filter(|p| {
            let p = p.borrow();
            p.kind == PrivateIdentifierKind::ACCESSOR
                && p.is_static == is_static
                && p.getter_name.is_nil()
        }) {
            previous_info.borrow_mut().getter_name = getter_name;
        } else {
            self.set_private_identifier(
                env,
                name,
                PrivateIdentifierInfo {
                    getter_name,
                    brand_check_identifier,
                    is_static,
                    is_valid,
                    ..PrivateIdentifierInfo::new(PrivateIdentifierKind::ACCESSOR)
                },
            );
        }
    }

    // Go: transformers/estransforms/classfields.go:3029 classFieldsTransformer.addPrivateIdentifierSetAccessorToEnvironment
    pub(super) fn add_private_identifier_set_accessor_to_environment(
        &mut self,
        name: Node,
        lex: &ClassLexicalEnvironmentRef,
        env: &PrivateEnvironmentRef,
        is_static: bool,
        is_valid: bool,
        previous_info: Option<PrivateIdentifierInfoRef>,
    ) {
        let setter_name = self.create_hoisted_variable_for_private_name(name, "_set");
        let brand_check_identifier = Self::brand_check_identifier_for(lex, env, is_static);
        go_assert!(
            is_static || brand_check_identifier.is_some(),
            "weakSetName should be set in private identifier environment"
        );

        if let Some(previous_info) = previous_info.as_ref().filter(|p| {
            let p = p.borrow();
            p.kind == PrivateIdentifierKind::ACCESSOR
                && p.is_static == is_static
                && p.setter_name.is_nil()
        }) {
            previous_info.borrow_mut().setter_name = setter_name;
        } else {
            self.set_private_identifier(
                env,
                name,
                PrivateIdentifierInfo {
                    setter_name,
                    brand_check_identifier,
                    is_static,
                    is_valid,
                    ..PrivateIdentifierInfo::new(PrivateIdentifierKind::ACCESSOR)
                },
            );
        }
    }

    // Go: transformers/estransforms/classfields.go:3056 classFieldsTransformer.addPrivateIdentifierAutoAccessorToEnvironment
    pub(super) fn add_private_identifier_auto_accessor_to_environment(
        &mut self,
        _node: Node,
        name: Node,
        lex: &ClassLexicalEnvironmentRef,
        env: &PrivateEnvironmentRef,
        is_static: bool,
        is_valid: bool,
    ) {
        let getter_name = self.create_hoisted_variable_for_private_name(name, "_get");
        let setter_name = self.create_hoisted_variable_for_private_name(name, "_set");
        let brand_check_identifier = Self::brand_check_identifier_for(lex, env, is_static);
        go_assert!(
            is_static || brand_check_identifier.is_some(),
            "weakSetName should be set in private identifier environment"
        );

        self.set_private_identifier(
            env,
            name,
            PrivateIdentifierInfo {
                getter_name,
                setter_name,
                brand_check_identifier,
                is_static,
                is_valid,
                ..PrivateIdentifierInfo::new(PrivateIdentifierKind::ACCESSOR)
            },
        );
    }

    // Go: transformers/estransforms/classfields.go:3081 classFieldsTransformer.addPrivateIdentifierToEnvironment
    pub(super) fn add_private_identifier_to_environment(&mut self, node: Node) {
        let lex = self.get_class_lexical_environment();
        let env = self.get_private_identifier_environment();
        let name = node.name();
        let is_static = has_static_modifier(node);
        let previous_info = self.get_private_identifier(&env, name);
        let is_valid = !self.is_reserved_private_name(name) && previous_info.is_none();

        if is_auto_accessor_property_declaration(node) {
            self.add_private_identifier_auto_accessor_to_environment(
                node, name, &lex, &env, is_static, is_valid,
            );
        } else if is_property_declaration(node) {
            self.add_private_identifier_property_declaration_to_environment(node, name);
        } else if is_method_declaration(node) {
            self.add_private_identifier_method_to_environment(
                name, &lex, &env, is_static, is_valid,
            );
        } else if is_get_accessor_declaration(node) {
            self.add_private_identifier_get_accessor_to_environment(
                name,
                &lex,
                &env,
                is_static,
                is_valid,
                previous_info,
            );
        } else if is_set_accessor_declaration(node) {
            self.add_private_identifier_set_accessor_to_environment(
                name,
                &lex,
                &env,
                is_static,
                is_valid,
                previous_info,
            );
        }
    }

    // Go: transformers/estransforms/classfields.go:3102 classFieldsTransformer.setPrivateIdentifier
    pub(super) fn set_private_identifier(
        &self,
        env: &PrivateEnvironmentRef,
        name: Node,
        info: PrivateIdentifierInfo,
    ) {
        let info = Rc::new(RefCell::new(info));
        let ec = &self.emit_context;
        if ec.has_auto_generate_info(name) {
            let key = ec.get_node_for_generated_name(name);
            env.borrow_mut().generated_identifiers.insert(key, info);
        } else {
            env.borrow_mut()
                .members
                .insert(name.text().to_string(), info);
        }
    }

    // Go: transformers/estransforms/classfields.go:3113 classFieldsTransformer.getPrivateIdentifier
    pub(super) fn get_private_identifier(
        &self,
        env: &PrivateEnvironmentRef,
        name: Node,
    ) -> Option<PrivateIdentifierInfoRef> {
        let ec = &self.emit_context;
        if ec.has_auto_generate_info(name) {
            let key = ec.get_node_for_generated_name(name);
            return env.borrow().generated_identifiers.get(&key).cloned();
        }
        env.borrow().members.get(name.text()).cloned()
    }

    /// Go `if tx.requiresBlockScopedVar() { AddLexicalDeclaration } else { AddVariableDeclaration }`.
    pub(super) fn add_hoisted_declaration(&self, identifier: Node) {
        if self.requires_block_scoped_var() {
            self.emit_context.add_lexical_declaration(identifier);
        } else {
            self.emit_context.add_variable_declaration(identifier);
        }
    }

    // Go: transformers/estransforms/classfields.go:3122 classFieldsTransformer.createHoistedVariableForClass
    pub(super) fn create_hoisted_variable_for_class(
        &mut self,
        name_text: &str,
        _node: Node,
        suffix: &str,
    ) -> Node {
        let env = self.get_private_identifier_environment();
        let class_name = env.borrow().data.class_name;
        let options = AutoGenerateOptions {
            flags: GeneratedIdentifierFlags::OPTIMISTIC
                | GeneratedIdentifierFlags::RESERVED_IN_NESTED_SCOPES,
            suffix: suffix.to_string(),
            ..Default::default()
        };
        let f = self.emit_context.factory();
        let identifier = if class_name.is_some() {
            let prefix = format!("_{}_", class_name.text());
            f.new_unique_name_ex(&format!("{prefix}{name_text}"), options)
        } else {
            f.new_unique_name_ex(&format!("_{name_text}"), options)
        };
        self.add_hoisted_declaration(identifier);
        identifier
    }

    // Go: transformers/estransforms/classfields.go:3145 classFieldsTransformer.createHoistedVariableForClassFromNode
    pub(super) fn create_hoisted_variable_for_class_from_node(
        &mut self,
        name: Node,
        suffix: &str,
    ) -> Node {
        let env = self.get_private_identifier_environment();
        let class_name = env.borrow().data.class_name;
        let prefix = if class_name.is_some() {
            format!("_{}_", class_name.text())
        } else {
            "_".to_string()
        };
        let identifier = self.emit_context.factory().new_generated_name_for_node_ex(
            name,
            AutoGenerateOptions {
                flags: GeneratedIdentifierFlags::OPTIMISTIC
                    | GeneratedIdentifierFlags::RESERVED_IN_NESTED_SCOPES,
                prefix,
                suffix: suffix.to_string(),
            },
        );
        self.add_hoisted_declaration(identifier);
        identifier
    }

    // Go: transformers/estransforms/classfields.go:3166 classFieldsTransformer.createHoistedVariableForPrivateName
    pub(super) fn create_hoisted_variable_for_private_name(
        &mut self,
        name: Node,
        suffix: &str,
    ) -> Node {
        // If the name is a generated identifier (e.g., auto-accessor backing field),
        // use node-based name generation so the emitter can resolve the name properly.
        if self.emit_context.has_auto_generate_info(name) {
            return self.create_hoisted_variable_for_class_from_node(name, suffix);
        }
        let mut text = name.text();
        if let Some(stripped) = text.strip_prefix('#') {
            text = stripped; // strip leading '#'
        }
        self.create_hoisted_variable_for_class(text, name, suffix)
    }

    // Go: transformers/estransforms/classfields.go:3181 classFieldsTransformer.accessPrivateIdentifier
    /// accessPrivateIdentifier accesses an already defined PrivateIdentifier in the current
    /// PrivateIdentifierEnvironment.
    pub(super) fn access_private_identifier(&self, name: Node) -> Option<PrivateIdentifierInfoRef> {
        let mut env = self.lexical_environment.clone();
        while let Some(e) = env {
            if let Some(private_env) = e.private_env() {
                if let Some(info) = self.get_private_identifier(&private_env, name) {
                    if info.borrow().kind == PrivateIdentifierKind::UNTRANSFORMED {
                        return None;
                    }
                    return Some(info);
                }
            }
            env = e.previous.clone();
        }
        None
    }

    // Go: transformers/estransforms/classfields.go:3195 classFieldsTransformer.wrapPrivateIdentifierForDestructuringTarget
    pub(super) fn wrap_private_identifier_for_destructuring_target(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        let parameter = f.new_generated_name_for_node(node);
        let info = self.access_private_identifier(node.name());
        let Some(info) = info else {
            return self.visit_each_child(node);
        };
        let mut receiver = node.expression();
        // We cannot copy `this` or `super` into the function because they will be bound
        // differently inside the function.
        let is_this_or_super_property = node.expression().kind() == SyntaxKind::ThisKeyword
            || node.expression().kind() == SyntaxKind::SuperKeyword;
        if is_this_or_super_property || !is_simple_copiable_expression(node.expression()) {
            receiver = f.new_temp_variable_ex(reserved_in_nested_scopes());
            ec.add_variable_declaration(receiver);
            let visited = self.visit_node(node.expression());
            self.pending_expressions
                .push(f.new_assignment_expression(receiver, visited));
        }
        let assign_expr = self.create_private_identifier_assignment(
            &info,
            receiver,
            parameter,
            SyntaxKind::EqualsToken,
        );
        f.new_assignment_target_wrapper(parameter, assign_expr)
    }

    // Go: transformers/estransforms/classfields.go:3220 classFieldsTransformer.visitAssignmentElement
    pub(super) fn visit_assignment_element(&mut self, mut node: Node) -> Node {
        // 13.15.5.5 RS: IteratorDestructuringAssignmentEvaluation
        //   AssignmentElement : DestructuringAssignmentTarget Initializer?
        //     ...
        //     4. If |Initializer| is present and _value_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) and IsIdentifierRef of |DestructuringAssignmentTarget| are both *true*, then
        //           i. Let _v_ be ? NamedEvaluation of |Initializer| with argument _lref_.[[ReferencedName]].
        //     ...

        if self.is_named_evaluation_needing_assigned_name(node) {
            node = transform_named_evaluation(
                &self.ec(),
                node,
                false, /*ignoreEmptyStringLiteral*/
                "",    /*assignedName*/
            );
        }
        if is_assignment_expression(node, true /*excludeCompoundAssignment*/) {
            let left = self.visit_destructuring_assignment_target(node.left());
            let right = self.visit_node(node.right());
            return self.ec().factory().update_binary_expression(
                node,
                ModifierList::NIL,
                left,
                Node::NIL,
                node.operator_token(),
                right,
            );
        }
        self.visit_destructuring_assignment_target(node)
    }

    // Go: transformers/estransforms/classfields.go:3247 classFieldsTransformer.visitAssignmentRestElement
    pub(super) fn visit_assignment_rest_element(&mut self, node: Node) -> Node {
        if is_left_hand_side_expression(node.expression()) {
            let expr = self.visit_destructuring_assignment_target(node.expression());
            return self.ec().factory().update_spread_element(node, expr);
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:3256 classFieldsTransformer.visitArrayAssignmentElement
    pub(super) fn visit_array_assignment_element(&mut self, node: Node) -> Node {
        if is_array_binding_or_assignment_element(node) {
            if is_spread_element(node) {
                return self.visit_assignment_rest_element(node);
            }
            if node.kind() != SyntaxKind::OmittedExpression {
                return self.visit_assignment_element(node);
            }
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:3268 classFieldsTransformer.visitAssignmentProperty
    pub(super) fn visit_assignment_property(&mut self, node: Node) -> Node {
        // AssignmentProperty : PropertyName `:` AssignmentElement
        // AssignmentElement : DestructuringAssignmentTarget Initializer?

        // 13.15.5.6 RS: KeyedDestructuringAssignmentEvaluation
        //   AssignmentElement : DestructuringAssignmentTarget Initializer?
        //     ...
        //     3. If |Initializer| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousfunctionDefinition(|Initializer|) and IsIdentifierRef of |DestructuringAssignmentTarget| are both *true*, then
        //           i. Let _rhsValue_ be ? NamedEvaluation of |Initializer| with argument _lref_.[[ReferencedName]].
        //     ...

        let name = self.visit_node(node.name());
        let init = node.initializer();
        if is_assignment_expression(init, true /*excludeCompoundAssignment*/) {
            let assign_elem = self.visit_assignment_element(init);
            return self.ec().factory().update_property_assignment(
                node,
                ModifierList::NIL,
                name,
                Node::NIL,
                Node::NIL,
                assign_elem,
            );
        }
        if is_left_hand_side_expression(init) {
            let target = self.visit_destructuring_assignment_target(init);
            return self.ec().factory().update_property_assignment(
                node,
                ModifierList::NIL,
                name,
                Node::NIL,
                Node::NIL,
                target,
            );
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:3294 classFieldsTransformer.visitShorthandAssignmentProperty
    pub(super) fn visit_shorthand_assignment_property(&mut self, mut node: Node) -> Node {
        // AssignmentProperty : IdentifierReference Initializer?

        // 13.15.5.3 RS: PropertyDestructuringAssignmentEvaluation
        //   AssignmentProperty : IdentifierReference Initializer?
        //     ...
        //     4. If |Initializer?| is present and _v_ is *undefined*, then
        //        a. If IsAnonymousFunctionDefinition(|Initializer|) is *true*, then
        //           i. Set _v_ to ? NamedEvaluation of |Initializer| with argument _P_.
        //     ...

        if self.is_named_evaluation_needing_assigned_name(node) {
            node = transform_named_evaluation(
                &self.ec(),
                node,
                false, /*ignoreEmptyStringLiteral*/
                "",    /*assignedName*/
            );
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:3311 classFieldsTransformer.visitAssignmentRestProperty
    pub(super) fn visit_assignment_rest_property(&mut self, node: Node) -> Node {
        if is_left_hand_side_expression(node.expression()) {
            let expr = self.visit_destructuring_assignment_target(node.expression());
            return self.ec().factory().update_spread_assignment(node, expr);
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:3320 classFieldsTransformer.visitObjectAssignmentElement
    pub(super) fn visit_object_assignment_element(&mut self, node: Node) -> Node {
        go_assert!(node.is_some() && is_object_literal_element(node));
        if is_spread_assignment(node) {
            return self.visit_assignment_rest_property(node);
        }
        if is_shorthand_property_assignment(node) {
            return self.visit_shorthand_assignment_property(node);
        }
        if is_property_assignment(node) {
            return self.visit_assignment_property(node);
        }
        self.visit_each_child(node)
    }

    // Go: transformers/estransforms/classfields.go:3334 classFieldsTransformer.visitAssignmentPattern
    pub(super) fn visit_assignment_pattern(&mut self, node: Node) -> Node {
        let ec = self.ec();
        let f = ec.factory();
        if is_array_literal_expression(node) {
            // Transforms private names in destructuring assignment array bindings.
            // Transforms SuperProperty assignments in destructuring assignment array bindings in static initializers.
            //
            // Source:
            // ([ this.#myProp ] = [ "hello" ]);
            //
            // Transformation:
            // [ { set value(x) { this.#myProp = x; } }.value ] = [ "hello" ];
            let elements = self.with_visitor(Self::visit_array_assignment_element, |v| {
                v.visit_nodes(node.element_list())
            });
            return f.update_array_literal_expression(node, elements, node.multi_line());
        }
        // Transforms private names in destructuring assignment object bindings.
        // Transforms SuperProperty assignments in destructuring assignment object bindings in static initializers.
        //
        // Source:
        // ({ stringProperty: this.#myProp } = { stringProperty: "hello" });
        //
        // Transformation:
        // ({ stringProperty: { set value(x) { this.#myProp = x; } }.value }) = { stringProperty: "hello" };
        let properties = self.with_visitor(Self::visit_object_assignment_element, |v| {
            v.visit_nodes(node.property_list())
        });
        f.update_object_literal_expression(node, properties, node.multi_line())
    }

    // Go: transformers/estransforms/classfields.go:3391 classFieldsTransformer.isReservedPrivateName
    pub(super) fn is_reserved_private_name(&self, node: Node) -> bool {
        !(is_private_identifier(node) && self.emit_context.has_auto_generate_info(node))
            && node.text() == "#constructor"
    }

    // Go: transformers/estransforms/classfields.go:3400 classFieldsTransformer.getProperties
    pub(super) fn get_properties(
        &self,
        node: Node,
        require_initializer: bool,
        is_static: bool,
    ) -> Vec<Node> {
        let mut result: Vec<Node> = Vec::new();
        for member in node.members().iter() {
            if is_property_declaration(member)
                && (!require_initializer || member.initializer().is_some())
                && has_static_modifier(member) == is_static
            {
                result.push(member);
            }
        }
        result
    }

    // Go: transformers/estransforms/classfields.go:3412 classFieldsTransformer.getStaticPropertiesAndClassStaticBlock
    pub(super) fn get_static_properties_and_class_static_block(&self, node: Node) -> Vec<Node> {
        let mut result: Vec<Node> = Vec::new();
        for member in node.members().iter() {
            if is_class_static_block_declaration(member)
                || (is_property_declaration(member) && has_static_modifier(member))
            {
                result.push(member);
            }
        }
        result
    }

    // Go: transformers/estransforms/classfields.go:3457 classFieldsTransformer.createCallBinding
    /// Returns `(thisArg, target)`.
    pub(super) fn create_call_binding(&self, node: Node) -> (Node, Node) {
        let ec = &self.emit_context;
        let f = ec.factory();
        if is_super_property(node) {
            return (f.new_this_expression(), node);
        }
        if is_property_access_expression(node) {
            if should_be_captured_in_temp_variable(node.expression()) {
                let this_arg = f.new_temp_variable();
                ec.add_variable_declaration(this_arg);
                let target = f.new_property_access_expression(
                    f.new_parenthesized_expression(
                        // TODO: do we even need these?
                        f.new_assignment_expression(this_arg, node.expression()),
                    ),
                    Node::NIL,
                    node.name(),
                    NodeFlags::NONE,
                );
                return (this_arg, target);
            }
            return (node.expression(), node);
        }
        let this_arg = f.new_void_zero_expression();
        let target = node;
        (this_arg, target)
    }

    // Go: transformers/estransforms/classfields.go:3493 classFieldsTransformer.createAccessorPropertyGetRedirector
    pub(super) fn create_accessor_property_get_redirector(
        &self,
        node: Node,
        modifiers: ModifierList,
        name: Node,
        receiver: Node,
    ) -> Node {
        let f = self.emit_context.factory();
        let backing_field_name = f.new_generated_private_name_for_node_ex(
            node.name(),
            AutoGenerateOptions {
                suffix: "_accessor_storage".to_string(),
                ..Default::default()
            },
        );
        let return_expr = f.new_property_access_expression(
            receiver,
            Node::NIL,
            backing_field_name,
            NodeFlags::NONE,
        );
        let return_stmt = f.new_return_statement(return_expr);
        let body = f.new_block(f.new_node_list(&[return_stmt]), false);
        f.new_get_accessor_declaration(
            modifiers,
            name,
            NodeList::NIL, /*typeParameters*/
            f.new_node_list(&[]),
            Node::NIL, /*returnType*/
            Node::NIL, /*fullSignature*/
            body,
        )
    }

    // Go: transformers/estransforms/classfields.go:3514 classFieldsTransformer.createAccessorPropertySetRedirector
    pub(super) fn create_accessor_property_set_redirector(
        &self,
        node: Node,
        modifiers: ModifierList,
        name: Node,
        receiver: Node,
    ) -> Node {
        let f = self.emit_context.factory();
        let backing_field_name = f.new_generated_private_name_for_node_ex(
            node.name(),
            AutoGenerateOptions {
                suffix: "_accessor_storage".to_string(),
                ..Default::default()
            },
        );
        let value_param = f.new_parameter_declaration(
            ModifierList::NIL, /*modifiers*/
            Node::NIL,         /*dotDotDotToken*/
            f.new_identifier("value"),
            Node::NIL, /*questionToken*/
            Node::NIL, /*typeNode*/
            Node::NIL, /*initializer*/
        );
        let assign_expr = f.new_assignment_expression(
            f.new_property_access_expression(
                receiver,
                Node::NIL,
                backing_field_name,
                NodeFlags::NONE,
            ),
            f.new_identifier("value"),
        );
        let expr_stmt = f.new_expression_statement(assign_expr);
        let body = f.new_block(f.new_node_list(&[expr_stmt]), false);
        f.new_set_accessor_declaration(
            modifiers,
            name,
            NodeList::NIL, /*typeParameters*/
            f.new_node_list(&[value_param]),
            Node::NIL, /*returnType*/
            Node::NIL, /*fullSignature*/
            body,
        )
    }
}

// Go: transformers/estransforms/classfields.go:3365 createPrivateStaticFieldInitializer
fn create_private_static_field_initializer(
    factory: &NodeFactory,
    variable_name: Node,
    mut initializer: Node,
) -> Node {
    if initializer.is_nil() {
        initializer = factory.new_void_zero_expression();
    }
    factory.new_assignment_expression(
        variable_name,
        factory.new_object_literal_expression(
            factory.new_node_list(&[factory.new_property_assignment(
                ModifierList::NIL,
                factory.new_identifier("value"),
                Node::NIL,
                Node::NIL,
                initializer,
            )]),
            false,
        ),
    )
}

// Go: transformers/estransforms/classfields.go:3380 createPrivateInstanceFieldInitializer
fn create_private_instance_field_initializer(
    factory: &NodeFactory,
    receiver: Node,
    mut initializer: Node,
    weak_map_name: Node,
) -> Node {
    if initializer.is_nil() {
        initializer = factory.new_void_zero_expression();
    }
    factory.new_method_call(
        weak_map_name,
        factory.new_identifier("set"),
        &[receiver, initializer],
    )
}

// Go: transformers/estransforms/classfields.go:3387 createPrivateInstanceMethodInitializer
fn create_private_instance_method_initializer(
    factory: &NodeFactory,
    receiver: Node,
    weak_set_name: Node,
) -> Node {
    factory.new_method_call(weak_set_name, factory.new_identifier("add"), &[receiver])
}

// Go: transformers/estransforms/classfields.go:3423 classHasClassThisAssignment
/// classHasClassThisAssignment checks if a class has a static block that is a class-this assignment.
pub(super) fn class_has_class_this_assignment(emit_context: &EmitContext, node: Node) -> bool {
    for member in node.members().iter() {
        if is_class_this_assignment_block(emit_context, member) {
            return true;
        }
    }
    false
}

// Go: transformers/estransforms/classfields.go:3432 isNonStaticMethodOrAccessorWithPrivateName
fn is_non_static_method_or_accessor_with_private_name(member: Node) -> bool {
    !is_static(member)
        && (is_method_or_accessor(member) || is_auto_accessor_property_declaration(member))
        && is_private_identifier(member.name())
}

// Go: transformers/estransforms/classfields.go:3438 createMemberAccessForPropertyName
pub(super) fn create_member_access_for_property_name(
    factory: &NodeFactory,
    emit_context: &EmitContext,
    receiver: Node,
    name: Node,
    location: Node,
) -> Node {
    if is_computed_property_name(name) {
        let expression = factory.new_element_access_expression(
            receiver,
            Node::NIL,
            name.expression(),
            NodeFlags::NONE,
        );
        set_node_loc(expression, location.loc());
        return expression;
    }
    let expression = if is_identifier(name) || is_private_identifier(name) {
        factory.new_property_access_expression(receiver, Node::NIL, name, NodeFlags::NONE)
    } else {
        // string or numeric literal
        factory.new_element_access_expression(receiver, Node::NIL, name, NodeFlags::NONE)
    };
    emit_context.set_comment_range(expression, name.loc());
    emit_context.set_source_map_range(expression, name.loc());
    emit_context.add_emit_flags(expression, EmitFlags::NO_NESTED_SOURCE_MAPS);
    expression
}

// Go: transformers/estransforms/classfields.go:3483 shouldBeCapturedInTempVariable
pub(super) fn should_be_captured_in_temp_variable(node: Node) -> bool {
    let target = skip_parentheses(node);
    !matches!(
        target.kind(),
        SyntaxKind::Identifier
            | SyntaxKind::ThisKeyword
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::StringLiteral
    )
}
