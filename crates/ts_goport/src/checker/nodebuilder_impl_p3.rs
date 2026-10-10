//! Go `checker/nodebuilderimpl.go` lines 2156 to 3055: type serialization
//! for declarations, object type element lists, anonymous types,
//! conditional types and type references.
//!
//! Methods use the Relater pattern: `impl Checker` methods take `&mut self`
//! and `b: &Rc<RefCell<NodeBuilderImpl>>`. No `RefCell` borrow of `b` is held
//! across a Checker call.

use crate::prelude::*;
use crate::printer::{EmitContext, EmitFlags, SymbolAccessibility};
use crate::pseudochecker::{self, PseudoChecker, PseudoTypeKind};

// PORT: the helpers below are the only places that read the layout of
// `NodeBuilderImpl` (fields `e`, `pc`, `ctx`) and of `SymbolTracker`. If
// that layout changes, only these helpers change. They are free functions so
// they do not clash with methods of other node builder files.

/// Runs `f` with a shared borrow of `b.ctx`.
pub(super) fn nb_ctx<R>(
    b: &Rc<RefCell<NodeBuilderImpl>>,
    f: impl FnOnce(&NodeBuilderContext) -> R,
) -> R {
    // PORT: `ctx` is an `Rc<RefCell<..>>`; clone it out so no borrow of `b` is held during `f`.
    let ctx = b.borrow().ctx.clone();
    let r = f(&ctx.borrow());
    r
}

/// Runs `f` with a mutable borrow of `b.ctx`.
pub(super) fn nb_ctx_mut<R>(
    b: &Rc<RefCell<NodeBuilderImpl>>,
    f: impl FnOnce(&mut NodeBuilderContext) -> R,
) -> R {
    let ctx = b.borrow().ctx.clone();
    let r = f(&mut ctx.borrow_mut());
    r
}

/// Go `b.e`. The node factory is `nb_e(b).factory()` (Go `b.f`).
pub(super) fn nb_e(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<EmitContext> {
    b.borrow().e.clone()
}

/// Go `b.pc`.
pub(super) fn nb_pc(b: &Rc<RefCell<NodeBuilderImpl>>) -> PseudoChecker {
    b.borrow().pc
}

// PORT: `SymbolTracker` methods take the checker (see nodebuilder_types.rs),
// so these wrappers take it too. The tracker `Rc` is cloned out first, so no
// borrow of `b` is held during the call.

/// Go `b.ctx.tracker.TrackSymbol(symbol, enclosingDeclaration, meaning)`.
pub(super) fn tracker_track_symbol(
    c: &mut Checker,
    b: &Rc<RefCell<NodeBuilderImpl>>,
    symbol: SymbolId,
    enclosing_declaration: Node,
    meaning: SymbolFlags,
) -> bool {
    let tracker = nb_ctx(b, |x| x.tracker.clone());
    tracker.track_symbol(c, symbol, enclosing_declaration, meaning)
}

/// Go `b.ctx.tracker.ReportNonSerializableProperty(name)`.
pub(super) fn tracker_report_non_serializable_property(
    c: &mut Checker,
    b: &Rc<RefCell<NodeBuilderImpl>>,
    name: &str,
) {
    let tracker = nb_ctx(b, |x| x.tracker.clone());
    tracker.report_non_serializable_property(c, name);
}

/// Go `b.ctx.tracker.ReportPrivateInBaseOfClassExpression(name)`.
pub(super) fn tracker_report_private_in_base_of_class_expression(
    c: &mut Checker,
    b: &Rc<RefCell<NodeBuilderImpl>>,
    name: &str,
) {
    let tracker = nb_ctx(b, |x| x.tracker.clone());
    tracker.report_private_in_base_of_class_expression(c, name);
}

/// Go `b.ctx.tracker.ReportCyclicStructureError()`.
pub(super) fn tracker_report_cyclic_structure_error(
    c: &mut Checker,
    b: &Rc<RefCell<NodeBuilderImpl>>,
) {
    let tracker = nb_ctx(b, |x| x.tracker.clone());
    tracker.report_cyclic_structure_error(c);
}

/// Go `b.ctx.tracker.ReportInferenceFallback(node)`.
pub(super) fn tracker_report_inference_fallback(
    c: &mut Checker,
    b: &Rc<RefCell<NodeBuilderImpl>>,
    node: Node,
) {
    let tracker = nb_ctx(b, |x| x.tracker.clone());
    tracker.report_inference_fallback(c, node);
}

// Go: checker/nodebuilderimpl.go:2372 MAX_REVERSE_MAPPED_NESTING_INSPECTION_DEPTH
pub const MAX_REVERSE_MAPPED_NESTING_INSPECTION_DEPTH: usize = 3;

// Go: checker/nodebuilderimpl.go:2448 propertyNameNodeKind
// PORT: the Go type is `PropertyNameNodeKind` in `crate::flags`.

// Go: checker/nodebuilderimpl.go:2456 classifyPropertyName
pub(crate) fn classify_property_name(
    name: &str,
    string_named: bool,
    is_method: bool,
) -> PropertyNameNodeKind {
    if is_method && name == "new" {
        return PropertyNameNodeKind::STRING_LITERAL;
    }
    if is_identifier_text(name, LanguageVariant::STANDARD) {
        return PropertyNameNodeKind::IDENTIFIER;
    }
    if !string_named
        && is_numeric_literal_name(name)
        && crate::jsnum::Number::from_string(name) >= crate::jsnum::Number(0.0)
    {
        PropertyNameNodeKind::NUMERIC_LITERAL
    } else {
        PropertyNameNodeKind::STRING_LITERAL
    }
}

impl Checker {
    // Go: checker/nodebuilderimpl.go:2253 serializeTypeForDeclaration
    pub fn serialize_type_for_declaration(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        mut declaration: Node,
        mut t: TypeId,
        mut symbol: SymbolId,
        try_reuse: bool,
    ) -> Node {
        if declaration.is_nil() && symbol.is_some() {
            declaration = self.sym(symbol).value_declaration;
            if declaration.is_nil() {
                // TODO: prefer annotated declarations like in strada (but does this ever even matter in practice? All callers should supply a declaration!)
                declaration = self
                    .sym(symbol)
                    .declarations
                    .first()
                    .copied()
                    .unwrap_or(Node::NIL);
            }
        }
        if symbol.is_nil() {
            symbol = self.get_symbol_of_declaration(declaration);
        }
        if t.is_nil() {
            if symbol.is_nil() {
                if is_variable_like(declaration) {
                    t = self.get_type_for_variable_like_declaration(
                        declaration,
                        false,
                        CheckMode::NORMAL,
                    );
                } else {
                    t = self.error_type;
                }
            } else {
                // Go keys the map by `ast.GetSymbolId(symbol)`, which gives
                // the symbol its id (`ValueSymbolLinkStore`).
                get_symbol_id(&self.symbols, symbol);
                t = nb_ctx(b, |c| c.enclosing_symbol_types.get(&symbol).copied())
                    .unwrap_or(TypeId::NIL);
                if t.is_nil() {
                    let mapper = nb_ctx(b, |c| c.mapper);
                    if self.sym(symbol).flags.intersects(SymbolFlags::ACCESSOR)
                        && declaration.kind() == SyntaxKind::SetAccessor
                    {
                        let write_type = self.get_write_type_of_symbol(symbol);
                        t = self.instantiate_type(write_type, mapper);
                    } else if symbol.is_some()
                        && !self
                            .sym(symbol)
                            .flags
                            .intersects(SymbolFlags::TYPE_LITERAL | SymbolFlags::SIGNATURE)
                    {
                        let symbol_type = self.get_type_of_symbol(symbol);
                        let widened = self.get_widened_literal_type(symbol_type);
                        t = self.instantiate_type(widened, mapper);
                    } else {
                        t = self.error_type;
                    }
                }
            }
        }

        // ts#64649, Go N' nodebuilderimpl.go:2287: the checker method, not the emit resolver.
        let requires_adding_undefined = declaration.is_some()
            && (is_parameter_declaration(declaration)
                || is_property_signature_declaration(declaration)
                || is_property_declaration(declaration))
            && {
                let enclosing_declaration = nb_ctx(b, |c| c.enclosing_declaration);
                self.requires_adding_implicit_undefined(declaration, symbol, enclosing_declaration)
            };
        let add_undefined_for_parameter = requires_adding_undefined && is_parameter_declaration(declaration) /*|| ast.IsJSDocParameterTag(declaration)*/;
        if add_undefined_for_parameter {
            t = self.get_optional_type(t, false);
        }

        let restore_flags = self.save_restore_flags(b);
        let enclosing_declaration = nb_ctx(b, |c| c.enclosing_declaration);
        let enclosing_file = nb_ctx(b, |c| c.enclosing_file);
        if self.ty(t).flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL)
            && self.ty(t).symbol == symbol
            && (enclosing_declaration.is_nil()
                || self
                    .sym(symbol)
                    .declarations
                    .iter()
                    .any(|&d| get_source_file_of_node(d) == enclosing_file))
        {
            nb_ctx_mut(b, |c| {
                c.flags = c.flags | NodeBuilderFlags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
            });
        }
        let mut result = Node::NIL;
        let mut reported_inference_fallback = false;
        // !!! expandable hover support
        if !self.is_actively_expanding(b)
            && try_reuse
            && enclosing_declaration.is_some()
            && declaration.is_some()
            && (is_accessor(declaration)
                || (has_inferred_type(declaration)
                    && !node_is_synthesized(declaration)
                    && !self
                        .ty(t)
                        .object_flags()
                        .intersects(ObjectFlags::REQUIRES_WIDENING)))
        {
            let mut remove = None;
            if symbol.is_some() {
                remove = Some(self.add_symbol_type_to_context(b, symbol, t));
            }
            let pc = nb_pc(b);
            // PORT: Go `pt == nil` checks are dropped. The pseudochecker returns
            // `Rc<PseudoType>`, which is never nil (Go returns nil only after debug.Fail).
            let mut pt = if is_accessor(declaration) {
                pc.get_type_of_accessor(&self.symbols, declaration)
            } else {
                pc.get_type_of_declaration(&self.symbols, declaration)
            };
            if pt.kind == PseudoTypeKind::NO_RESULT
                && is_binary_expression(declaration)
                && symbol.is_some()
            {
                let decl = self
                    .sym(symbol)
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| has_type_annotation(d));
                if let Some(decl) = decl {
                    // Binary expressions have a first-in-wins type annotation system. The first one with an annotation supplies the type for the rest.
                    pt = pc.get_type_of_declaration(&self.symbols, decl);
                }
            }
            let report_errors = !nb_ctx(b, |c| c.suppress_report_inference_fallback);
            let is_optional_annotated = !requires_adding_undefined
                && (is_parameter_declaration(declaration)
                    || is_property_signature_declaration(declaration)
                    || is_property_declaration(declaration))
                && is_optional_declaration(declaration);
            if self.pseudo_type_equivalent_to_type(b, &pt, t, is_optional_annotated, report_errors)
            {
                // !!! TODO: If annotated type node is a reference with insufficient type arguments, we should still fall back to type serialization
                // see: canReuseTypeNodeAnnotation in strada for context
                let ptt = self.pseudo_type_to_type(b, &pt);
                if ptt.is_some()
                    && requires_adding_undefined
                    && self.contains_non_missing_undefined_type(t)
                    && !self.contains_non_missing_undefined_type(ptt)
                {
                    pt = pseudochecker::new_pseudo_type_union(vec![
                        pt,
                        pseudochecker::pseudo_type_undefined(),
                    ]);
                }
                result = self.pseudo_type_to_node_with_checker_fallback(b, &pt, t);
            } else {
                // Equivalence failed; if errors from inferred-with-errors pseudo types were
                // reported, note it so we can suppress nested errors during the fallback
                // typeToTypeNode serialization (mirroring the suppression that
                // pseudoTypeToNodeWithCheckerFallback provides).
                reported_inference_fallback = report_errors
                    && pt.kind == PseudoTypeKind::INFERRED
                    && !pt.as_pseudo_type_inferred().error_nodes.is_empty();
                let mut should_add_undefined = false;
                if requires_adding_undefined {
                    let ptt = self.pseudo_type_to_type(b, &pt);
                    if ptt.is_some() {
                        should_add_undefined = !self.contains_non_missing_undefined_type(ptt);
                    } else {
                        should_add_undefined =
                            !pseudochecker::could_already_refer_to_undefined_type(&pt);
                    }
                }
                if should_add_undefined {
                    pt = pseudochecker::new_pseudo_type_union(vec![
                        pt,
                        pseudochecker::pseudo_type_undefined(),
                    ]);
                    if self.pseudo_type_equivalent_to_type(b, &pt, t, false, report_errors) {
                        result = self.pseudo_type_to_node_with_checker_fallback(b, &pt, t);
                        reported_inference_fallback = false;
                    }
                }
            }
            if let Some(remove) = remove {
                remove();
            }
        }
        if result.is_nil() {
            if reported_inference_fallback {
                let old_suppress = nb_ctx(b, |c| c.suppress_report_inference_fallback);
                nb_ctx_mut(b, |c| c.suppress_report_inference_fallback = true);
                result = self.type_to_type_node(b, t);
                nb_ctx_mut(b, |c| c.suppress_report_inference_fallback = old_suppress);
            } else {
                result = self.type_to_type_node(b, t);
            }
        }
        restore_flags();
        if result.is_nil() {
            return nb_e(b)
                .factory()
                .new_keyword_type_node(SyntaxKind::AnyKeyword);
        }
        result
    }

    // Go: checker/nodebuilderimpl.go:2374 shouldUsePlaceholderForProperty
    pub fn should_use_placeholder_for_property(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        property_symbol: SymbolId,
    ) -> bool {
        // Reverse mapped type placeholders are for display, not declaration emit.
        // (ts#64558, Go N' nodebuilderimpl.go:2374)
        if !nb_ctx(b, |c| {
            c.flags
                .intersects(NodeBuilderFlags::ALLOW_ANONYMOUS_IDENTIFIER)
        }) {
            return false;
        }
        // Use placeholders for reverse mapped types we've either
        // (1) already descended into, or
        // (2) are nested reverse mappings within a mapping over a non-anonymous type, or
        // (3) are deeply nested properties that originate from the same mapped type.
        // Condition (2) is a restriction mostly just to
        // reduce the blowup in printback size from doing, eg, a deep reverse mapping over `Window`.
        // Since anonymous types usually come from expressions, this allows us to preserve the output
        // for deep mappings which likely come from expressions, while truncating those parts which
        // come from mappings over library functions.
        // Condition (3) limits printing of possibly infinitely deep reverse mapped types.
        if !self
            .sym(property_symbol)
            .check_flags
            .intersects(CheckFlags::REVERSE_MAPPED)
        {
            return false;
        }
        let reverse_mapped_stack = nb_ctx(b, |c| c.reverse_mapped_stack.clone());
        // (1)
        if reverse_mapped_stack.contains(&property_symbol) {
            return true;
        }
        // (2)
        if let Some(&last) = reverse_mapped_stack.last() {
            if self.reverse_mapped_symbol_links.has(last) {
                let property_type = self
                    .reverse_mapped_symbol_links
                    .try_get(last)
                    .map_or(TypeId::NIL, |l| l.property_type);
                if property_type.is_some()
                    && !self
                        .ty(property_type)
                        .object_flags
                        .intersects(ObjectFlags::ANONYMOUS)
                {
                    return true;
                }
            }
        }
        // (3) - we only inspect the last MAX_REVERSE_MAPPED_NESTING_INSPECTION_DEPTH elements of the
        // stack for approximate matches to catch tight infinite loops
        // TODO: Why? Reasoning lost to time. this could probably stand to be improved?
        if reverse_mapped_stack.len() < MAX_REVERSE_MAPPED_NESTING_INSPECTION_DEPTH {
            return false;
        }
        if !self.reverse_mapped_symbol_links.has(property_symbol) {
            return false;
        }
        let prop_mapped_type = self
            .reverse_mapped_symbol_links
            .try_get(property_symbol)
            .map_or(TypeId::NIL, |l| l.mapped_type);
        if prop_mapped_type.is_nil() || self.ty(prop_mapped_type).symbol.is_nil() {
            return false;
        }
        let prop_mapped_symbol = self.ty(prop_mapped_type).symbol;
        for i in 0..reverse_mapped_stack.len() {
            if i > MAX_REVERSE_MAPPED_NESTING_INSPECTION_DEPTH {
                break;
            }
            let prop = reverse_mapped_stack[reverse_mapped_stack.len() - 1 - i];
            if self.reverse_mapped_symbol_links.has(prop) {
                let mapped_type = self
                    .reverse_mapped_symbol_links
                    .try_get(prop)
                    .map_or(TypeId::NIL, |l| l.mapped_type);
                if mapped_type.is_some() && self.ty(mapped_type).symbol == prop_mapped_symbol {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/nodebuilderimpl.go:2433 trackComputedName
    pub fn track_computed_name(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        access_expression: Node,
        enclosing_declaration: Node,
    ) {
        // get symbol of the first identifier of the entityName
        let first_identifier = get_first_identifier(access_expression);
        let name = self.resolve_name(
            enclosing_declaration,
            first_identifier.text(),
            SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE,
            None, /*nameNotFoundMessage*/
            true, /*isUse*/
            false,
        );
        if name.is_some() {
            tracker_track_symbol(self, b, name, enclosing_declaration, SymbolFlags::VALUE);
        } else {
            // Name does not resolve at target location, track symbol at dest location (should be inaccessible)
            let fallback = self.resolve_name(
                first_identifier,
                first_identifier.text(),
                SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE,
                None, /*nameNotFoundMessage*/
                true, /*isUse*/
                false,
            );
            if fallback.is_some() {
                tracker_track_symbol(self, b, fallback, enclosing_declaration, SymbolFlags::VALUE);
            }
        }
    }

    // Go: checker/nodebuilderimpl.go:2466 createPropertyNameNodeForIdentifierOrLiteral
    pub fn create_property_name_node_for_identifier_or_literal(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        name: &str,
        single_quote: bool,
        string_named: bool,
        is_method: bool,
        symbol: SymbolId,
    ) -> Node {
        match classify_property_name(name, string_named, is_method) {
            PropertyNameNodeKind::IDENTIFIER => self.nb_new_identifier(b, name, symbol),
            PropertyNameNodeKind::NUMERIC_LITERAL => nb_e(b)
                .factory()
                .new_numeric_literal(name, TokenFlags::NONE),
            _ => nb_e(b).factory().new_string_literal(
                name,
                if single_quote {
                    TokenFlags::SINGLE_QUOTE
                } else {
                    TokenFlags::NONE
                },
            ),
        }
    }

    // Go: checker/nodebuilderimpl.go:2477 isStringNamed
    pub fn is_string_named(&mut self, _b: &Rc<RefCell<NodeBuilderImpl>>, d: Node) -> bool {
        let name = get_name_of_declaration(d);
        if name.is_nil() {
            return false;
        }
        if is_computed_property_name(name) {
            let t = self.check_expression(name.expression());
            return self.ty(t).flags.intersects(TypeFlags::STRING_LIKE);
        }
        if is_element_access_expression(name) {
            let t = self.check_expression(name.argument_expression());
            return self.ty(t).flags.intersects(TypeFlags::STRING_LIKE);
        }
        is_string_literal(name)
    }

    // Go: checker/nodebuilderimpl.go:2493 isSingleQuotedStringNamed
    pub fn is_single_quoted_string_named(
        &mut self,
        _b: &Rc<RefCell<NodeBuilderImpl>>,
        d: Node,
    ) -> bool {
        let name = get_name_of_declaration(d);
        name.is_some()
            && is_string_literal(name)
            && name.token_flags().intersects(TokenFlags::SINGLE_QUOTE)
    }

    // Go: checker/nodebuilderimpl.go:2498 getPropertyNameNodeForSymbol
    pub fn get_property_name_node_for_symbol(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> Node {
        // For hash-private names, clone the original private identifier from the declaration
        let value_declaration = self.sym(symbol).value_declaration;
        if value_declaration.is_some() {
            let decl_name = value_declaration.name();
            if decl_name.is_some() && is_private_identifier(decl_name) {
                return nb_e(b).factory().deep_clone_node(decl_name);
            }
        }
        let declarations = self.sym(symbol).declarations.clone();
        let mut string_named = !declarations.is_empty();
        for &d in &declarations {
            if !self.is_string_named(b, d) {
                string_named = false;
                break;
            }
        }
        let mut single_quote = !declarations.is_empty();
        for &d in &declarations {
            if !self.is_single_quoted_string_named(b, d) {
                single_quote = false;
                break;
            }
        }
        let is_method = self.sym(symbol).flags.intersects(SymbolFlags::METHOD);
        let from_name_type = self.get_property_name_node_for_symbol_from_name_type(
            b,
            symbol,
            enclosing_declaration,
            single_quote,
            string_named,
            is_method,
        );
        if from_name_type.is_some() {
            return from_name_type;
        }

        let mut name = self.sym(symbol).name.clone();
        let private_name_prefix = format!("{INTERNAL_SYMBOL_NAME_PREFIX}#");
        if let Some(rest) = name.strip_prefix(private_name_prefix.as_str()) {
            // symbol IDs are unstable - replace #nnn# with #private#
            let rest = rest.trim_start_matches(is_digit);
            name = format!("__#private{rest}").into();
        }

        self.create_property_name_node_for_identifier_or_literal(
            b,
            &name,
            single_quote,
            string_named,
            is_method,
            symbol,
        )
    }

    /// See getNameForSymbolFromNameType for a stringy equivalent
    // Go: checker/nodebuilderimpl.go:2527 getPropertyNameNodeForSymbolFromNameType
    pub fn get_property_name_node_for_symbol_from_name_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
        enclosing_declaration: Node,
        single_quote: bool,
        string_named: bool,
        is_method: bool,
    ) -> Node {
        if !self.value_symbol_links.has_by_id(&self.symbols, symbol) {
            return Node::NIL;
        }
        let name_type = self
            .value_symbol_links
            .try_get_by_id(&self.symbols, symbol)
            .map_or(TypeId::NIL, |l| l.name_type);
        if name_type.is_nil() {
            return Node::NIL;
        }
        let mut enum_enclosing_declaration = enclosing_declaration;
        let enclosing_file = nb_ctx(b, |c| c.enclosing_file);
        if enum_enclosing_declaration.is_nil() && enclosing_file.is_some() {
            enum_enclosing_declaration = enclosing_file;
        }
        if self.ty(name_type).flags.intersects(TypeFlags::ENUM_LITERAL) {
            let name_type_symbol = self.ty(name_type).symbol;
            let mut enum_symbol = self.sym(name_type_symbol).parent;
            if enum_symbol.is_nil() {
                enum_symbol = name_type_symbol;
            }
            if enum_enclosing_declaration.is_some()
                && self.is_symbol_accessible_by_flags(
                    enum_symbol,
                    enum_enclosing_declaration,
                    SymbolFlags::VALUE,
                )
            {
                let save_enclosing_declaration = nb_ctx(b, |c| c.enclosing_declaration);
                nb_ctx_mut(b, |c| c.enclosing_declaration = enum_enclosing_declaration);
                let expression = self.symbol_to_expression(b, name_type_symbol, SymbolFlags::VALUE);
                let result = nb_e(b).factory().new_computed_property_name(expression);
                nb_ctx_mut(b, |c| c.enclosing_declaration = save_enclosing_declaration);
                return result;
            }
        }
        let e = nb_e(b);
        let f = e.factory();
        if self
            .ty(name_type)
            .flags
            .intersects(TypeFlags::STRING_OR_NUMBER_LITERAL)
        {
            let name = match &self.ty(name_type).as_literal_type().value {
                Some(LiteralValue::Number(n)) => n.to_string(),
                Some(LiteralValue::String(s)) => s.clone(),
                _ => String::new(),
            };
            if !is_identifier_text(&name, LanguageVariant::STANDARD)
                && (string_named || !is_numeric_literal_name(&name))
            {
                let node = f.new_string_literal(
                    name,
                    if single_quote {
                        TokenFlags::SINGLE_QUOTE
                    } else {
                        TokenFlags::NONE
                    },
                );
                return node;
            }
            if is_numeric_literal_name(&name) && name.starts_with('-') {
                let literal = f.new_numeric_literal(&name[1..], TokenFlags::NONE);
                return f.new_computed_property_name(
                    f.new_prefix_unary_expression(SyntaxKind::MinusToken, literal),
                );
            }
            return self.create_property_name_node_for_identifier_or_literal(
                b,
                &name,
                single_quote,
                string_named,
                is_method,
                symbol,
            );
        }
        if self
            .ty(name_type)
            .flags
            .intersects(TypeFlags::UNIQUE_ES_SYMBOL)
        {
            // PORT: Go `nameType.AsUniqueESSymbolType().symbol` is the embedded `Type.symbol`.
            let unique_symbol = self.ty(name_type).symbol;
            // The reference was tracked in the destination scope by trackComputedName.
            // Reconstructing its spelling in the source scope must not paint that scope's declarations visible.
            let expression = self.symbol_to_expression_worker(b, unique_symbol, SymbolFlags::VALUE);
            return f.new_computed_property_name(expression);
        }
        Node::NIL
    }

    // PORT: Go appends to and returns a `[]*ast.TypeElement`; here the
    // slice is a `Vec<Node>` that is moved in and returned.
    // Go: checker/nodebuilderimpl.go:2578 addPropertyToElementList
    pub fn add_property_to_element_list(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        property_symbol: SymbolId,
        mut type_elements: Vec<Node>,
    ) -> Vec<Node> {
        let property_is_reverse_mapped = self
            .sym(property_symbol)
            .check_flags
            .intersects(CheckFlags::REVERSE_MAPPED);
        let property_type = if self.should_use_placeholder_for_property(b, property_symbol) {
            self.any_type
        } else {
            self.get_non_missing_type_of_symbol(property_symbol)
        };
        let save_enclosing_declaration = nb_ctx(b, |c| c.enclosing_declaration);
        nb_ctx_mut(b, |c| c.enclosing_declaration = Node::NIL);
        if is_late_bound_name(&self.sym(property_symbol).name) {
            if !self.sym(property_symbol).declarations.is_empty() {
                let decl = self.sym(property_symbol).declarations[0];
                if self.has_late_bindable_name(decl) {
                    if is_binary_expression(decl) {
                        let name = get_name_of_declaration(decl);
                        if name.is_some()
                            && is_element_access_expression(name)
                            && is_property_access_entity_name_expression(
                                name.argument_expression(),
                                false, /*allowJs*/
                            )
                        {
                            self.track_computed_name(
                                b,
                                name.argument_expression(),
                                save_enclosing_declaration,
                            );
                        }
                    } else {
                        self.track_computed_name(
                            b,
                            decl.name().expression(),
                            save_enclosing_declaration,
                        );
                    }
                }
            } else {
                let text = self.symbol_to_string(property_symbol);
                tracker_report_non_serializable_property(self, b, &text);
            }
        }
        let value_declaration = self.sym(property_symbol).value_declaration;
        let first_declaration = self
            .sym(property_symbol)
            .declarations
            .first()
            .copied()
            .unwrap_or(Node::NIL);
        if value_declaration.is_some() {
            nb_ctx_mut(b, |c| c.enclosing_declaration = value_declaration);
        } else if first_declaration.is_some() {
            nb_ctx_mut(b, |c| c.enclosing_declaration = first_declaration);
        } else {
            nb_ctx_mut(b, |c| c.enclosing_declaration = save_enclosing_declaration);
        }
        let property_name =
            self.get_property_name_node_for_symbol(b, property_symbol, save_enclosing_declaration);
        nb_ctx_mut(b, |c| c.enclosing_declaration = save_enclosing_declaration);
        let name_length = go_len(&symbol_name(&self.symbols, property_symbol)) as i32;
        nb_ctx_mut(b, |c| c.approximate_length += name_length + 1);

        let e = nb_e(b);
        let f = e.factory();
        if self
            .sym(property_symbol)
            .flags
            .intersects(SymbolFlags::ACCESSOR)
        {
            let write_type = self.get_write_type_of_symbol(property_symbol);
            if !self.is_error_type(property_type) && !self.is_error_type(write_type) {
                let prop_declaration = get_declaration_of_kind(
                    &self.symbols,
                    property_symbol,
                    SyntaxKind::PropertyDeclaration,
                );
                let parent = self.sym(property_symbol).parent;
                let parent_is_class =
                    parent.is_some() && self.sym(parent).flags.intersects(SymbolFlags::CLASS);
                if property_type != write_type || parent_is_class && prop_declaration.is_nil() {
                    let symbol_mapper = self
                        .value_symbol_links
                        .get_by_id(&self.symbols, property_symbol)
                        .mapper;
                    let getter_declaration = get_declaration_of_kind(
                        &self.symbols,
                        property_symbol,
                        SyntaxKind::GetAccessor,
                    );
                    if getter_declaration.is_some() {
                        let mut getter_signature =
                            self.get_signature_from_declaration(getter_declaration);
                        if symbol_mapper.is_some() {
                            getter_signature =
                                self.instantiate_signature(getter_signature, symbol_mapper);
                        }
                        let getter = self.signature_to_signature_declaration_helper(
                            b,
                            getter_signature,
                            SyntaxKind::GetAccessor,
                            Some(&SignatureToSignatureDeclarationOptions {
                                name: property_name,
                                ..Default::default()
                            }),
                        );
                        self.set_comment_range(b, getter, getter_declaration);
                        type_elements.push(getter);
                    }
                    let setter_declaration = get_declaration_of_kind(
                        &self.symbols,
                        property_symbol,
                        SyntaxKind::SetAccessor,
                    );
                    if setter_declaration.is_some() {
                        let mut setter_signature =
                            self.get_signature_from_declaration(setter_declaration);
                        if symbol_mapper.is_some() {
                            setter_signature =
                                self.instantiate_signature(setter_signature, symbol_mapper);
                        }
                        let setter = self.signature_to_signature_declaration_helper(
                            b,
                            setter_signature,
                            SyntaxKind::SetAccessor,
                            Some(&SignatureToSignatureDeclarationOptions {
                                name: property_name,
                                ..Default::default()
                            }),
                        );
                        self.set_comment_range(b, setter, setter_declaration);
                        type_elements.push(setter);
                    }
                    return type_elements;
                } else if parent_is_class
                    && prop_declaration.is_some()
                    && prop_declaration
                        .modifier_nodes()
                        .iter()
                        .any(|m| m.kind() == SyntaxKind::AccessorKeyword)
                {
                    let fake_getter_signature = self.new_signature(
                        SignatureFlags::NONE,
                        Node::NIL,
                        &[],
                        SymbolId::NIL,
                        &[],
                        property_type,
                        TypePredicateId::NIL,
                        0,
                    );
                    let fake_getter_declaration = self.signature_to_signature_declaration_helper(
                        b,
                        fake_getter_signature,
                        SyntaxKind::GetAccessor,
                        Some(&SignatureToSignatureDeclarationOptions {
                            name: property_name,
                            ..Default::default()
                        }),
                    );
                    self.set_comment_range(b, fake_getter_declaration, prop_declaration);
                    type_elements.push(fake_getter_declaration);

                    let setter_param =
                        self.new_symbol(SymbolFlags::FUNCTION_SCOPED_VARIABLE, "arg");
                    self.value_symbol_links
                        .get_by_id(&self.symbols, setter_param)
                        .resolved_type = write_type;
                    let void_type = self.void_type;
                    let fake_setter_signature = self.new_signature(
                        SignatureFlags::NONE,
                        Node::NIL,
                        &[],
                        SymbolId::NIL,
                        &[setter_param],
                        void_type,
                        TypePredicateId::NIL,
                        0,
                    );
                    let fake_setter_declaration = self.signature_to_signature_declaration_helper(
                        b,
                        fake_setter_signature,
                        SyntaxKind::SetAccessor,
                        Some(&SignatureToSignatureDeclarationOptions {
                            name: property_name,
                            ..Default::default()
                        }),
                    );
                    type_elements.push(fake_setter_declaration);
                    return type_elements;
                }
            }
        }

        let optional_token = if self
            .sym(property_symbol)
            .flags
            .intersects(SymbolFlags::OPTIONAL)
        {
            f.new_token(SyntaxKind::QuestionToken)
        } else {
            Node::NIL
        };
        if self
            .sym(property_symbol)
            .flags
            .intersects(SymbolFlags::FUNCTION | SymbolFlags::METHOD)
            && self.get_properties_of_object_type(property_type).is_empty()
            && !self.is_readonly_symbol(property_symbol)
        {
            let filtered = self.filter_type(property_type, &mut |c: &mut Checker, t: TypeId| {
                !c.ty(t).flags.intersects(TypeFlags::UNDEFINED)
            });
            let signatures = self.get_signatures_of_type(filtered, SignatureKind::CALL);
            for &signature in &signatures {
                let method_declaration = self.signature_to_signature_declaration_helper(
                    b,
                    signature,
                    SyntaxKind::MethodSignature,
                    Some(&SignatureToSignatureDeclarationOptions {
                        name: property_name,
                        question_token: optional_token,
                        ..Default::default()
                    }),
                );
                let signature_declaration = self.sig(signature).declaration;
                let range = if signature_declaration.is_some() {
                    signature_declaration
                } else {
                    self.sym(property_symbol).value_declaration
                };
                self.set_comment_range(b, method_declaration, range);
                type_elements.push(method_declaration);
            }
            if !signatures.is_empty() || optional_token.is_nil() {
                return type_elements;
            }
        }
        let property_type_node;
        if self.should_use_placeholder_for_property(b, property_symbol) {
            property_type_node = self.create_elided_information_placeholder(b);
        } else {
            if property_is_reverse_mapped {
                nb_ctx_mut(b, |c| c.reverse_mapped_stack.push(property_symbol));
            }
            if property_type.is_some() {
                property_type_node = self.serialize_type_for_declaration(
                    b,
                    Node::NIL, /*declaration*/
                    property_type,
                    property_symbol,
                    true,
                );
            } else {
                property_type_node = f.new_keyword_type_node(SyntaxKind::AnyKeyword);
            }
            if property_is_reverse_mapped {
                nb_ctx_mut(b, |c| {
                    c.reverse_mapped_stack.pop();
                });
            }
        }

        let mut modifiers = ModifierList::NIL;
        if self.is_readonly_symbol(property_symbol) {
            modifiers = f.new_modifier_list(&[f.new_modifier(SyntaxKind::ReadonlyKeyword)]);
            nb_ctx_mut(b, |c| c.approximate_length += 9);
        }
        let property_signature = f.new_property_signature_declaration(
            modifiers,
            property_name,
            optional_token,
            property_type_node,
            Node::NIL,
        );

        let value_declaration = self.sym(property_symbol).value_declaration;
        self.set_comment_range(b, property_signature, value_declaration);
        type_elements.push(property_signature);

        type_elements
    }

    // PORT: Go takes the `*StructuredType` returned by
    // `resolveStructuredTypeMembers`. Here the caller passes the resolved
    // type's `TypeId` and the members are read from it.
    // Go: checker/nodebuilderimpl.go:2719 createTypeNodesFromResolvedType
    pub fn create_type_nodes_from_resolved_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        resolved_type: TypeId,
    ) -> NodeList {
        let e = nb_e(b);
        let f = e.factory();
        if self.check_truncation_length(b) {
            if nb_ctx(b, |c| c.flags.intersects(NodeBuilderFlags::NO_TRUNCATION)) {
                let elem = f.new_not_emitted_type_element();
                return f.new_node_list(&[e.add_synthetic_trailing_comment(
                    elem,
                    SyntaxKind::MultiLineCommentTrivia,
                    "elided",
                    false, /*hasTrailingNewLine*/
                )]);
            }
            return f.new_node_list(&[f.new_property_signature_declaration(
                ModifierList::NIL,
                f.new_identifier("..."),
                Node::NIL,
                Node::NIL,
                Node::NIL,
            )]);
        }
        let (call_signatures, construct_signatures, index_infos, properties) = {
            let resolved = self.resolve_structured_type_members(resolved_type);
            (
                resolved.call_signatures().to_vec(),
                resolved.construct_signatures().to_vec(),
                resolved.index_infos_list(),
                resolved.properties.clone(),
            )
        };
        let mut type_elements: Vec<Node> = Vec::new();
        for signature in call_signatures {
            type_elements.push(self.signature_to_signature_declaration_helper(
                b,
                signature,
                SyntaxKind::CallSignature,
                None,
            ));
        }
        for signature in construct_signatures {
            if self
                .sig(signature)
                .flags
                .intersects(SignatureFlags::ABSTRACT)
            {
                continue;
            }
            type_elements.push(self.signature_to_signature_declaration_helper(
                b,
                signature,
                SyntaxKind::ConstructSignature,
                None,
            ));
        }
        for info in index_infos {
            // ts#64558 (Go N' nodebuilderimpl.go:2742): the placeholder is
            // made (and adds to approximateLength) only for a reverse mapped
            // type in display.
            let type_node = if self
                .ty(resolved_type)
                .object_flags
                .intersects(ObjectFlags::REVERSE_MAPPED)
                && nb_ctx(b, |c| {
                    c.flags
                        .intersects(NodeBuilderFlags::ALLOW_ANONYMOUS_IDENTIFIER)
                }) {
                self.create_elided_information_placeholder(b)
            } else {
                Node::NIL
            };
            let nodes = self
                .index_info_to_object_computed_names_or_signature_declaration(b, info, type_node);
            type_elements.extend(nodes);
        }

        if properties.is_empty() {
            return f.new_node_list(&type_elements);
        }

        let mut i = 0usize;
        for &property_symbol in &properties {
            if nb_ctx(b, |c| is_expanding(c))
                && self
                    .sym(property_symbol)
                    .flags
                    .intersects(SymbolFlags::PROTOTYPE)
            {
                continue;
            }
            i += 1;
            if nb_ctx(b, |c| {
                c.flags
                    .intersects(NodeBuilderFlags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL)
            }) {
                if self
                    .sym(property_symbol)
                    .flags
                    .intersects(SymbolFlags::PROTOTYPE)
                {
                    continue;
                }
                if self
                    .get_declaration_modifier_flags_from_symbol(property_symbol)
                    .intersects(ModifierFlags::PRIVATE | ModifierFlags::PROTECTED)
                {
                    let name = self.sym(property_symbol).name.clone();
                    tracker_report_private_in_base_of_class_expression(self, b, &name);
                }
                if self.is_private_identifier_symbol(property_symbol) {
                    let name = symbol_name(&self.symbols, property_symbol);
                    tracker_report_private_in_base_of_class_expression(self, b, &name);
                }
            }
            if self.check_truncation_length(b) && (i + 2 < properties.len() - 1) {
                if nb_ctx(b, |c| c.flags.intersects(NodeBuilderFlags::NO_TRUNCATION)) {
                    let last = type_elements.len() - 1;
                    type_elements[last] = e.add_synthetic_trailing_comment(
                        type_elements[last],
                        SyntaxKind::MultiLineCommentTrivia,
                        &format!("... {} more elided ...", properties.len() - i),
                        false, /*hasTrailingNewLine*/
                    );
                } else {
                    let text = format!("... {} more ...", properties.len() - i);
                    type_elements.push(f.new_property_signature_declaration(
                        ModifierList::NIL,
                        f.new_identifier(text),
                        Node::NIL,
                        Node::NIL,
                        Node::NIL,
                    ));
                }
                type_elements = self.add_property_to_element_list(
                    b,
                    properties[properties.len() - 1],
                    type_elements,
                );
                break;
            }
            type_elements = self.add_property_to_element_list(b, property_symbol, type_elements);
        }
        if !type_elements.is_empty() {
            f.new_node_list(&type_elements)
        } else {
            NodeList::NIL
        }
    }

    // Go: checker/nodebuilderimpl.go:2782 createTypeNodeFromObjectType
    pub fn create_type_node_from_object_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
    ) -> Node {
        if self.is_generic_mapped_type(t)
            || (self.ty(t).object_flags.intersects(ObjectFlags::MAPPED)
                && self.ty(t).as_mapped_type().contains_error)
        {
            return self.create_mapped_type_node_from_type(b, t);
        }

        let (call_sigs, ctor_sigs, properties, index_info_count) = {
            let resolved = self.resolve_structured_type_members(t);
            (
                resolved.call_signatures().to_vec(),
                resolved.construct_signatures().to_vec(),
                resolved.properties.clone(),
                resolved.index_infos().len(),
            )
        };
        let e = nb_e(b);
        let f = e.factory();
        if properties.is_empty() && index_info_count == 0 {
            if call_sigs.is_empty() && ctor_sigs.is_empty() {
                nb_ctx_mut(b, |c| c.approximate_length += 2);
                let result = f.new_type_literal_node(f.new_node_list(&[]));
                e.set_emit_flags(result, EmitFlags::SINGLE_LINE);
                return result;
            }

            if call_sigs.len() == 1 && ctor_sigs.is_empty() {
                let signature = call_sigs[0];
                let signature_node = self.signature_to_signature_declaration_helper(
                    b,
                    signature,
                    SyntaxKind::FunctionType,
                    None,
                );
                return signature_node;
            }

            if ctor_sigs.len() == 1 && call_sigs.is_empty() {
                let signature = ctor_sigs[0];
                let signature_node = self.signature_to_signature_declaration_helper(
                    b,
                    signature,
                    SyntaxKind::ConstructorType,
                    None,
                );
                return signature_node;
            }
        }

        let abstract_signatures: Vec<SignatureId> = ctor_sigs
            .iter()
            .copied()
            .filter(|&signature| {
                self.sig(signature)
                    .flags
                    .intersects(SignatureFlags::ABSTRACT)
            })
            .collect();
        if !abstract_signatures.is_empty() {
            let mut types: Vec<TypeId> = abstract_signatures
                .iter()
                .map(|&s| self.get_or_create_type_from_signature(s))
                .collect();
            // count the number of type elements excluding abstract constructors
            let property_count = if nb_ctx(b, |c| {
                c.flags
                    .intersects(NodeBuilderFlags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL)
            }) {
                properties
                    .iter()
                    .filter(|&&p| !self.sym(p).flags.intersects(SymbolFlags::PROTOTYPE))
                    .count()
            } else {
                properties.len()
            };
            let type_element_count = call_sigs.len()
                + (ctor_sigs.len() - abstract_signatures.len())
                + index_info_count
                + property_count;
            // don't include an empty object literal if there were no other static-side
            // properties to write, i.e. `abstract class C { }` becomes `abstract new () => {}`
            // and not `(abstract new () => {}) & {}`
            if type_element_count != 0 {
                // create a copy of the object type without any abstract construct signatures.
                // PORT: Go passes the resolved `*StructuredType`; here it is `t`.
                types.push(self.get_resolved_type_without_abstract_construct_signatures(b, t));
            }
            let intersection = self.get_intersection_type(&types);
            return self.type_to_type_node(b, intersection);
        }

        let restore_flags = self.save_restore_flags(b);
        nb_ctx_mut(b, |c| {
            c.flags = c.flags | NodeBuilderFlags::IN_OBJECT_TYPE_LITERAL
        });
        let members = self.create_type_nodes_from_resolved_type(b, t);
        restore_flags();
        let type_literal_node = f.new_type_literal_node(members);
        nb_ctx_mut(b, |c| c.approximate_length += 2);
        let multiline = nb_ctx(b, |c| {
            c.flags
                .intersects(NodeBuilderFlags::MULTILINE_OBJECT_LITERALS)
        });
        e.set_emit_flags(
            type_literal_node,
            if multiline {
                EmitFlags::default()
            } else {
                EmitFlags::SINGLE_LINE
            },
        );
        type_literal_node
    }

    // PORT: Go package function `getTypeAliasForTypeLiteral(c, t)`; a
    // Checker method here. Go checks `Declarations != nil`; an empty list is
    // treated the same as nil.
    // Go: checker/nodebuilderimpl.go:2842 getTypeAliasForTypeLiteral
    pub fn get_type_alias_for_type_literal(&mut self, t: TypeId) -> SymbolId {
        let symbol = self.ty(t).symbol;
        if symbol.is_some()
            && self.sym(symbol).flags.intersects(SymbolFlags::TYPE_LITERAL)
            && !self.sym(symbol).declarations.is_empty()
        {
            let node = walk_up_parenthesized_types(self.sym(symbol).declarations[0].parent());
            if is_type_alias_declaration(node) {
                return self.get_symbol_of_declaration(node);
            }
        }
        SymbolId::NIL
    }

    // Go: checker/nodebuilderimpl.go:2852 shouldWriteTypeOfFunctionSymbol
    pub fn should_write_type_of_function_symbol(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        mut symbol: SymbolId,
        type_id: TypeId,
    ) -> (bool, SymbolId) {
        let declarations = self.sym(symbol).declarations.clone();
        let mut is_static_method_symbol = false;
        // `typeof C.name` can only be written when the member name is a valid identifier
        if self.sym(symbol).flags.intersects(SymbolFlags::METHOD)
            && is_identifier_text(&self.sym(symbol).name, LanguageVariant::STANDARD)
        {
            for &declaration in &declarations {
                if is_static(declaration)
                    && !self.is_late_bindable_index_signature(get_name_of_declaration(declaration))
                {
                    is_static_method_symbol = true;
                    break;
                }
            }
        }
        let mut is_non_local_function_symbol = false;
        let mut is_function_expression_symbol = false;
        if self.sym(symbol).flags.intersects(SymbolFlags::FUNCTION) {
            if self.sym(symbol).parent.is_some() {
                is_non_local_function_symbol = true;
            } else {
                for &declaration in &declarations {
                    let parent_kind = declaration.parent().kind();
                    if parent_kind == SyntaxKind::SourceFile
                        || parent_kind == SyntaxKind::ModuleBlock
                    {
                        is_non_local_function_symbol = true;
                        break;
                    }
                    if is_function_expression_or_arrow_function(declaration)
                        && is_variable_declaration(declaration.parent())
                        && is_variable_declaration_list(declaration.parent().parent())
                        && is_variable_statement(declaration.parent().parent().parent())
                        && declaration.parent().parent().parent().parent().is_some()
                        && matches!(
                            declaration.parent().parent().parent().parent().kind(),
                            SyntaxKind::SourceFile | SyntaxKind::ModuleBlock
                        )
                    {
                        is_non_local_function_symbol = true;
                        is_function_expression_symbol = true;
                        break;
                    }
                }
            }
        }
        if is_static_method_symbol || is_non_local_function_symbol {
            let value_declaration = self.sym(symbol).value_declaration;
            if is_function_expression_symbol
                && value_declaration.is_some()
                && value_declaration.parent().is_some()
                && value_declaration.parent() != nb_ctx(b, |c| c.enclosing_declaration)
            {
                symbol = self.get_merged_symbol(value_declaration.parent().symbol());
            }
            // typeof is allowed only for static/non local functions
            let flags = nb_ctx(b, |c| c.flags);
            let visited = nb_ctx(b, |c| c.visited_types.contains(&type_id));
            let result = (flags.intersects(NodeBuilderFlags::USE_TYPE_OF_FUNCTION) || visited) // it is type of the symbol uses itself recursively
                && (!flags.intersects(NodeBuilderFlags::USE_STRUCTURAL_FALLBACK) || {
                    let enclosing_declaration = nb_ctx(b, |c| c.enclosing_declaration);
                    self.is_value_symbol_accessible(symbol, enclosing_declaration)
                }); // And the build is going to succeed without visibility error or there is no structural fallback allowed
            return (result, symbol);
        }
        (false, symbol)
    }

    // Go: checker/nodebuilderimpl.go:2890 createAnonymousTypeNode
    pub fn create_anonymous_type_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
    ) -> Node {
        self.create_anonymous_type_node_ex(b, t, false, false)
    }

    // Go: checker/nodebuilderimpl.go:2894 shouldEmitTypeOfSymbol
    pub fn should_emit_type_of_symbol(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        force_expansion: bool,
        force_class_expansion: bool,
        is_instance_type: SymbolFlags,
        symbol: SymbolId,
        type_id: TypeId,
    ) -> (bool, SymbolId) {
        if force_expansion {
            return (false, symbol);
        }
        let non_function_result = (self.sym(symbol).flags.intersects(SymbolFlags::CLASS)
            && !force_class_expansion
            && self.get_base_type_variable_of_class(symbol).is_nil()
            && !{
                let value_declaration = self.sym(symbol).value_declaration;
                value_declaration.is_some()
                    && is_class_like(value_declaration)
                    && nb_ctx(b, |c| {
                        c.flags
                            .intersects(NodeBuilderFlags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL)
                    })
                    && (!is_class_declaration(value_declaration) || {
                        let enclosing_declaration = nb_ctx(b, |c| c.enclosing_declaration);
                        self.is_symbol_accessible(
                            symbol,
                            enclosing_declaration,
                            is_instance_type,
                            false, /*shouldComputeAliasesToMakeVisible*/
                        )
                        .accessibility
                            != SymbolAccessibility::ACCESSIBLE
                    })
            })
            || self
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::ENUM | SymbolFlags::VALUE_MODULE);
        if non_function_result {
            return (true, symbol);
        }
        self.should_write_type_of_function_symbol(b, symbol, type_id)
    }

    // Go: checker/nodebuilderimpl.go:2905 createAnonymousTypeNodeEx
    pub fn create_anonymous_type_node_ex(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
        force_class_expansion: bool,
        force_expansion: bool,
    ) -> Node {
        let type_id = self.ty(t).id;
        let symbol = self.ty(t).symbol;
        if symbol.is_some() {
            let is_instantiation_expression_type = self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE);
            if is_instantiation_expression_type {
                let existing = self.ty(t).as_instantiation_expression_type().node;
                //  instantiationExpressionType.node is unreliable for constituents of unions and intersections.
                // declare const Err: typeof ErrImpl & (<T>() => T);
                // type ErrAlias<U> = typeof Err<U>;
                // declare const e: ErrAlias<number>;
                // ErrAlias<number> = typeof Err<number> = typeof ErrImpl & (<number>() => number)
                // The problem is each constituent of the intersection will be associated with typeof Err<number>
                // And when extracting a type for typeof ErrImpl from typeof Err<number> does not make sense.
                if is_type_query_node(existing)
                    && self.nb_get_type_from_type_node(b, existing, false) == t
                {
                    // Guard against unbounded recursion when the existing typeof node fails to be reused
                    // (e.g. its entity name isn't accessible from this scope) and the recovery boundary's
                    // fallback re-enters typeToTypeNode with the very same instantiation type, which would
                    // in turn try to reuse the same node again. Mark the type as visited around the reuse
                    // attempt so the inner recursion bottoms out via the visitedTypes guard below.
                    if nb_ctx(b, |c| c.visited_types.contains(&type_id)) {
                        return self.create_cyclic_structure_placeholder(b);
                    }
                    nb_ctx_mut(b, |c| c.visited_types.insert(type_id));
                    let type_node = self.try_reuse_existing_non_parameter_type_node(
                        b,
                        existing,
                        t,
                        Node::NIL,
                        TypeId::NIL,
                    );
                    nb_ctx_mut(b, |c| c.visited_types.remove(&type_id));
                    if type_node.is_some() {
                        return type_node;
                    }
                }
                if nb_ctx(b, |c| c.visited_types.contains(&type_id)) {
                    return self.create_cyclic_structure_placeholder(b);
                }
                return self.visit_and_transform_type(
                    b,
                    t,
                    Checker::create_type_node_from_object_type,
                );
            }
            let is_instance_type = if self.is_class_instance_side(t) {
                SymbolFlags::TYPE
            } else {
                SymbolFlags::VALUE
            };

            // !!! JS support
            // if c.isJSConstructor(symbol.ValueDeclaration) {
            // 	// Instance and static types share the same symbol; only add 'typeof' for the static side.
            // 	return b.symbolToTypeNode(symbol, isInstanceType, nil)
            // } else
            let (ok, typeof_symbol) = self.should_emit_type_of_symbol(
                b,
                force_expansion,
                force_class_expansion,
                is_instance_type,
                symbol,
                type_id,
            );
            if ok {
                if self.should_expand_type(b, t, false /*isAlias*/) {
                    nb_ctx_mut(b, |c| c.depth += 1);
                } else {
                    return self.symbol_to_type_node(
                        b,
                        typeof_symbol,
                        is_instance_type,
                        NodeList::NIL,
                    );
                }
            }
            if nb_ctx(b, |c| c.visited_types.contains(&type_id)) {
                // If type is an anonymous type literal in a type alias declaration, use type alias name
                let type_alias = self.get_type_alias_for_type_literal(t);
                if type_alias.is_some() {
                    // The specified symbol flags need to be reinterpreted as type flags
                    self.symbol_to_type_node(b, type_alias, SymbolFlags::TYPE, NodeList::NIL)
                } else {
                    self.create_cyclic_structure_placeholder(b)
                }
            } else {
                self.visit_and_transform_type(b, t, Checker::create_type_node_from_object_type)
            }
        } else if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::REVERSE_MAPPED)
            && !nb_ctx(b, |c| {
                c.flags
                    .intersects(NodeBuilderFlags::ALLOW_ANONYMOUS_IDENTIFIER)
            })
        {
            // ts#64558 (Go N' nodebuilderimpl.go:2979)
            if nb_ctx(b, |c| c.visited_types.contains(&type_id)) {
                return self.create_cyclic_structure_placeholder(b);
            }
            self.visit_and_transform_type(b, t, Checker::create_type_node_from_object_type)
        } else {
            // Reverse mapped types use property and index signature placeholders for display.
            self.create_type_node_from_object_type(b, t)
        }
    }

    // PORT: Go `b.getTypeFromTypeNode`. The name `get_type_from_type_node`
    // is already the Checker method (Go `c.getTypeFromTypeNode`), so the node
    // builder version has the `nb_` prefix.
    // Go: checker/nodebuilderimpl.go:2978 getTypeFromTypeNode
    pub fn nb_get_type_from_type_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        node: Node,
        no_mapped_types: bool,
    ) -> TypeId {
        // !!! noMappedTypes optional param support
        if node.parent().is_nil() {
            return self.error_type;
        }
        let t = self.get_type_from_type_node(node);
        let mapper = nb_ctx(b, |c| c.mapper);
        if mapper.is_nil() {
            return t;
        }

        let instantiated = self.instantiate_type(t, mapper);
        if no_mapped_types && instantiated != t {
            return TypeId::NIL;
        }
        instantiated
    }

    // Go: checker/nodebuilderimpl.go:2995 typeToTypeNodeOrCircularityElision
    pub fn type_to_type_node_or_circularity_elision(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
    ) -> Node {
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            let id = self.ty(t).id;
            if nb_ctx(b, |c| c.visited_types.contains(&id)) {
                return self.create_cyclic_structure_placeholder(b);
            }
            return self.visit_and_transform_type(b, t, Checker::type_to_type_node);
        }
        self.type_to_type_node(b, t)
    }

    // Go: checker/nodebuilderimpl.go:3017 createCyclicStructurePlaceholder (Go N', ts#64461)
    pub fn create_cyclic_structure_placeholder(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
    ) -> Node {
        if !nb_ctx(b, |c| {
            c.flags
                .intersects(NodeBuilderFlags::ALLOW_ANONYMOUS_IDENTIFIER)
        }) {
            nb_ctx_mut(b, |c| c.encountered_error = true);
            tracker_report_cyclic_structure_error(self, b);
        }
        self.create_elided_information_placeholder(b)
    }

    // Go: checker/nodebuilderimpl.go:3009 conditionalTypeToTypeNode
    pub fn conditional_type_to_type_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t_: TypeId,
    ) -> Node {
        if self.check_truncation_length(b) {
            return self.create_elided_information_placeholder(b);
        }
        let (check_type, extends_type, mapper, root) = {
            let t = self.ty(t_).as_conditional_type();
            (t.check_type, t.extends_type, t.mapper, t.root.clone())
        };
        let check_type_node = self.type_to_type_node(b, check_type);
        nb_ctx_mut(b, |c| c.approximate_length += 15);
        let e = nb_e(b);
        let f = e.factory();
        if nb_ctx(b, |c| {
            c.flags
                .intersects(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS)
        }) && root.borrow().is_distributive
            && !self
                .ty(check_type)
                .flags
                .intersects(TypeFlags::TYPE_PARAMETER)
        {
            let new_param_symbol =
                self.new_symbol(SymbolFlags::TYPE_PARAMETER, "T" /* as __String */);
            let new_param = self.new_type_parameter(new_param_symbol);
            let name = self.type_parameter_to_name(b, new_param);
            let new_type_variable = f.new_type_reference_node(name, NodeList::NIL);
            nb_ctx_mut(b, |c| c.approximate_length += 37);
            // 15 each for two added conditionals, 7 for an added infer type
            let (root_check_type, root_extends_type, root_node, root_infer_type_parameters) = {
                let r = root.borrow();
                (
                    r.check_type,
                    r.extends_type,
                    r.node,
                    r.infer_type_parameters.clone(),
                )
            };
            let new_mapper = self.prepend_type_mapping(root_check_type, new_param, mapper);
            let save_infer_type_parameters = nb_ctx(b, |c| c.infer_type_parameters.clone());
            nb_ctx_mut(b, |c| c.infer_type_parameters = root_infer_type_parameters);
            let instantiated_extends = self.instantiate_type(root_extends_type, new_mapper);
            let extends_type_node = self.type_to_type_node(b, instantiated_extends);
            nb_ctx_mut(b, |c| c.infer_type_parameters = save_infer_type_parameters);
            let true_type = self.nb_get_type_from_type_node(b, root_node.true_type(), false);
            let true_type = self.instantiate_type(true_type, new_mapper);
            let true_type_node = self.type_to_type_node_or_circularity_elision(b, true_type);
            let false_type = self.nb_get_type_from_type_node(b, root_node.false_type(), false);
            let false_type = self.instantiate_type(false_type, new_mapper);
            let false_type_node = self.type_to_type_node_or_circularity_elision(b, false_type);

            // outermost conditional makes `T` a type parameter, allowing the inner conditionals to be distributive
            // second conditional makes `T` have `T & checkType` substitution, so it is correctly usable as the checkType
            // inner conditional runs the check the user provided on the check type (distributively) and returns the result
            // checkType extends infer T ? T extends checkType ? T extends extendsType<T> ? trueType<T> : falseType<T> : never : never;
            // this is potentially simplifiable to
            // checkType extends infer T ? T extends checkType & extendsType<T> ? trueType<T> : falseType<T> : never;
            // but that may confuse users who read the output more.
            // On the other hand,
            // checkType extends infer T extends checkType ? T extends extendsType<T> ? trueType<T> : falseType<T> : never;
            // may also work with `infer ... extends ...` in, but would produce declarations only compatible with the latest TS.
            // PORT: Go `Identifier.Clone(f)` is `f.clone_node`.
            let new_id = f.clone_node(new_type_variable.type_name());
            let synthetic_extends_node = f.new_infer_type_node(f.new_type_parameter_declaration(
                ModifierList::NIL,
                new_id,
                Node::NIL,
                Node::NIL,
                Node::NIL,
            ));
            let inner_check_conditional_node = f.new_conditional_type_node(
                new_type_variable,
                extends_type_node,
                true_type_node,
                false_type_node,
            );
            let synthetic_true_node = f.new_conditional_type_node(
                f.new_type_reference_node(f.clone_node(name), NodeList::NIL),
                f.deep_clone_node(check_type_node),
                inner_check_conditional_node,
                f.new_keyword_type_node(SyntaxKind::NeverKeyword),
            );
            return f.new_conditional_type_node(
                check_type_node,
                synthetic_extends_node,
                synthetic_true_node,
                f.new_keyword_type_node(SyntaxKind::NeverKeyword),
            );
        }
        let save_infer_type_parameters = nb_ctx(b, |c| c.infer_type_parameters.clone());
        let root_infer_type_parameters = root.borrow().infer_type_parameters.clone();
        nb_ctx_mut(b, |c| c.infer_type_parameters = root_infer_type_parameters);
        let extends_type_node = self.type_to_type_node(b, extends_type);
        nb_ctx_mut(b, |c| c.infer_type_parameters = save_infer_type_parameters);
        let true_type = self.get_true_type_from_conditional_type(t_);
        let true_type_node = self.type_to_type_node_or_circularity_elision(b, true_type);
        let false_type = self.get_false_type_from_conditional_type(t_);
        let false_type_node = self.type_to_type_node_or_circularity_elision(b, false_type);
        f.new_conditional_type_node(
            check_type_node,
            extends_type_node,
            true_type_node,
            false_type_node,
        )
    }

    // PORT: Go takes `*TypeParameter`; here the type parameter's `TypeId`.
    // Go: checker/nodebuilderimpl.go:3055 getParentSymbolOfTypeParameter
    pub fn get_parent_symbol_of_type_parameter(
        &mut self,
        _b: &Rc<RefCell<NodeBuilderImpl>>,
        type_parameter: TypeId,
    ) -> SymbolId {
        let tp = get_declaration_of_kind(
            &self.symbols,
            self.ty(type_parameter).symbol,
            SyntaxKind::TypeParameter,
        );
        // !!! JSDoc support
        // if ast.IsJSDocTemplateTag(tp.Parent) {
        // 	host = getEffectiveContainerForJSDocTemplateTag(tp.Parent)
        // } else {
        let host = tp.parent();
        // }
        if host.is_nil() {
            return SymbolId::NIL;
        }
        self.get_symbol_of_node(host)
    }

    // Go: checker/nodebuilderimpl.go:3086 arrayOrTupleTypeToNode (Go N', ts#64556)
    // The array and tuple part of Go N typeReferenceToTypeNode.
    pub fn array_or_tuple_type_to_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
    ) -> Node {
        let mut type_arguments: Vec<TypeId> = self.get_type_arguments(t).to_vec();
        let target = self.ty(t).target();
        let e = nb_e(b);
        let f = e.factory();
        if target == self.global_array_type || target == self.global_readonly_array_type {
            if nb_ctx(b, |c| {
                c.flags
                    .intersects(NodeBuilderFlags::WRITE_ARRAY_AS_GENERIC_TYPE)
            }) {
                let type_argument_node = self.type_to_type_node(b, type_arguments[0]);
                let name = if target == self.global_array_type {
                    "Array"
                } else {
                    "ReadonlyArray"
                };
                let target_symbol = self.ty(target).symbol;
                let type_name = self.nb_new_identifier(b, name, target_symbol);
                return f
                    .new_type_reference_node(type_name, f.new_node_list(&[type_argument_node]));
            }
            let element_type = self.type_to_type_node(b, type_arguments[0]);
            let array_type = f.new_array_type_node(element_type);
            if target == self.global_array_type {
                array_type
            } else {
                f.new_type_operator_node(SyntaxKind::ReadonlyKeyword, array_type)
            }
        } else {
            debug_assert!(self.ty(target).object_flags.intersects(ObjectFlags::TUPLE));
            let element_infos: Vec<TupleElementInfo> =
                self.ty(target).as_tuple_type().element_infos.clone();
            let readonly = self.ty(target).as_tuple_type().readonly;
            for (i, arg) in type_arguments.iter_mut().enumerate() {
                let mut is_optional = false;
                if i < element_infos.len() {
                    is_optional = element_infos[i].flags.intersects(ElementFlags::OPTIONAL);
                }
                *arg = self.remove_missing_type(*arg, is_optional);
            }
            if !type_arguments.is_empty() {
                let arity = self.get_type_reference_arity(t) as usize;
                let tuple_constituent_nodes =
                    self.map_to_type_nodes(b, &type_arguments[0..arity], false /*isBareList*/);
                if tuple_constituent_nodes.is_some() {
                    // PORT: Go assigns into `tupleConstituentNodes.Nodes[i]`. Parsed
                    // node lists are immutable here, so the nodes are copied,
                    // changed and put into a new list.
                    let mut nodes = tuple_constituent_nodes.nodes().to_vec();
                    for i in 0..nodes.len() {
                        let flags = element_infos[i].flags;
                        let labeled_element_declaration = element_infos[i].labeled_declaration;

                        if labeled_element_declaration.is_some() {
                            // PORT: Go `core.IfElse` evaluates both arguments, so each
                            // token and array node is created before the choice.
                            let dot_dot_dot = f.new_token(SyntaxKind::DotDotDotToken);
                            let dot_dot_dot = if flags.intersects(ElementFlags::VARIABLE) {
                                dot_dot_dot
                            } else {
                                Node::NIL
                            };
                            let label = self.get_tuple_element_label(
                                element_infos[i],
                                SymbolId::NIL,
                                i as i32,
                            );
                            let name =
                                self.nb_new_identifier(b, &label, SymbolId::NIL /*symbol*/);
                            let question = f.new_token(SyntaxKind::QuestionToken);
                            let question = if flags.intersects(ElementFlags::OPTIONAL) {
                                question
                            } else {
                                Node::NIL
                            };
                            let array_type = f.new_array_type_node(nodes[i]);
                            let type_node = if flags.intersects(ElementFlags::REST) {
                                array_type
                            } else {
                                nodes[i]
                            };
                            nodes[i] =
                                f.new_named_tuple_member(dot_dot_dot, name, question, type_node);
                        } else if flags.intersects(ElementFlags::VARIABLE) {
                            let array_type = f.new_array_type_node(nodes[i]);
                            let type_node = if flags.intersects(ElementFlags::REST) {
                                array_type
                            } else {
                                nodes[i]
                            };
                            nodes[i] = f.new_rest_type_node(type_node);
                        } else if flags.intersects(ElementFlags::OPTIONAL) {
                            nodes[i] = f.new_optional_type_node(nodes[i]);
                        }
                    }
                    let tuple_type_node = f.new_tuple_type_node(f.new_node_list(&nodes));
                    e.set_emit_flags(tuple_type_node, EmitFlags::SINGLE_LINE);
                    return if readonly {
                        f.new_type_operator_node(SyntaxKind::ReadonlyKeyword, tuple_type_node)
                    } else {
                        tuple_type_node
                    };
                }
            }
            if nb_ctx(b, |c| {
                c.encountered_error || c.flags.intersects(NodeBuilderFlags::ALLOW_EMPTY_TUPLE)
            }) {
                let tuple_type_node = f.new_tuple_type_node(f.new_node_list(&[]));
                e.set_emit_flags(tuple_type_node, EmitFlags::SINGLE_LINE);
                return if readonly {
                    f.new_type_operator_node(SyntaxKind::ReadonlyKeyword, tuple_type_node)
                } else {
                    tuple_type_node
                };
            }
            nb_ctx_mut(b, |c| c.encountered_error = true);
            Node::NIL
            // TODO: GH#18217
        }
    }

    // Go: checker/nodebuilderimpl.go:3070 typeReferenceToTypeNode
    // Go N' nodebuilderimpl.go:3160: since ts#64556 the array and tuple part
    // is `array_or_tuple_type_to_node`.
    pub fn type_reference_to_type_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: TypeId,
    ) -> Node {
        let mut type_arguments: Vec<TypeId> = self.get_type_arguments(t).to_vec();
        let target = self.ty(t).target();
        let e = nb_e(b);
        let f = e.factory();
        if nb_ctx(b, |c| {
            c.flags
                .intersects(NodeBuilderFlags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL)
        }) && {
            let value_declaration = self.sym(self.ty(t).symbol).value_declaration;
            value_declaration.is_some() && is_class_like(value_declaration)
        } && {
            let symbol = self.ty(t).symbol;
            let enclosing_declaration = nb_ctx(b, |c| c.enclosing_declaration);
            !self.is_value_symbol_accessible(symbol, enclosing_declaration)
        } {
            self.create_anonymous_type_node(b, t)
        } else {
            let outer_type_parameters: Vec<TypeId> = self
                .ty(target)
                .as_interface_type()
                .outer_type_parameters()
                .to_vec();
            let mut i = 0usize;
            let mut result_type = Node::NIL;
            // PORT: Go checks `outerTypeParameters != nil`; an empty slice
            // gives the same result (the loop does not run).
            if !outer_type_parameters.is_empty() {
                let length = outer_type_parameters.len();
                while i < length {
                    // Find group of type arguments for type parameters with the same declaring container.
                    let start = i;
                    let parent =
                        self.get_parent_symbol_of_type_parameter(b, outer_type_parameters[i]);
                    loop {
                        // do-while loop
                        i += 1;
                        if !(i < length
                            && self
                                .get_parent_symbol_of_type_parameter(b, outer_type_parameters[i])
                                == parent)
                        {
                            break;
                        }
                    }
                    // When type parameters are their own type arguments for the whole group (i.e. we have
                    // the default outer type arguments), we don't show the group.

                    if outer_type_parameters[start..i] != type_arguments[start..i] {
                        let type_argument_slice = self.map_to_type_nodes(
                            b,
                            &type_arguments[start..i],
                            false, /*isBareList*/
                        );
                        let restore_flags = self.save_restore_flags(b);
                        nb_ctx_mut(b, |c| {
                            c.flags =
                                c.flags | NodeBuilderFlags::FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES
                        });
                        let ref_ = self.symbol_to_type_node(
                            b,
                            parent,
                            SymbolFlags::TYPE,
                            type_argument_slice,
                        );
                        restore_flags();
                        if result_type.is_nil() {
                            result_type = ref_;
                        } else {
                            result_type = self.append_reference_to_type(b, result_type, ref_);
                        }
                    }
                }
            }
            let mut type_argument_nodes = NodeList::NIL;
            if !type_arguments.is_empty() {
                let mut type_parameter_count = 0usize;
                let target_type = self.ty(target).as_interface_type();
                // PORT: Go `typeParams != nil` (nodebuilderimpl.go:3198 at N').
                // `TypeParameters()` (types.go:1046) is nil only when
                // `allTypeParameters` is empty. A class or interface that has only a
                // `this` type gives an empty slice that is not nil, so Go still runs the
                // global `Iterable` checks below, and they resolve (create) those types.
                let has_type_params = !target_type.all_type_parameters.is_empty();
                let type_params: Vec<TypeId> = target_type.type_parameters().to_vec();
                if has_type_params {
                    type_parameter_count = type_params.len().min(type_arguments.len());

                    // Maybe we should do this for more types, but for now we only elide type arguments that are
                    // identical to their associated type parameters' defaults for `Iterable`, `IterableIterator`,
                    // `AsyncIterable`, and `AsyncIterableIterator` to provide backwards-compatible .d.ts emit due
                    // to each now having three type parameters instead of only one.
                    let is_iterable_reference = {
                        let iterable = self.get_global_iterable_type();
                        self.is_reference_to_type(t, iterable)
                    } || {
                        let iterable_iterator = self.get_global_iterable_iterator_type();
                        self.is_reference_to_type(t, iterable_iterator)
                    } || {
                        let async_iterable = self.get_global_async_iterable_type();
                        self.is_reference_to_type(t, async_iterable)
                    } || {
                        let async_iterable_iterator =
                            self.get_global_async_iterable_iterator_type();
                        self.is_reference_to_type(t, async_iterable_iterator)
                    };
                    if is_iterable_reference {
                        let reference_node = self.ty(t).as_type_reference().node;
                        if reference_node.is_nil()
                            || !is_type_reference_node(reference_node)
                            || reference_node.type_arguments().is_empty()
                            || reference_node.type_arguments().len() < type_parameter_count
                        {
                            while type_parameter_count > 0 {
                                let type_argument = type_arguments[type_parameter_count - 1];
                                let type_parameter = type_params[type_parameter_count - 1];
                                let default_type =
                                    self.get_default_from_type_parameter(type_parameter);
                                if default_type.is_nil()
                                    || !self.is_type_identical_to(type_argument, default_type)
                                {
                                    break;
                                }
                                type_parameter_count -= 1;
                            }
                        }
                    }
                }

                type_argument_nodes = self.map_to_type_nodes(
                    b,
                    &type_arguments[i..type_parameter_count],
                    false, /*isBareList*/
                );
            }
            let restore_flags = self.save_restore_flags(b);
            nb_ctx_mut(b, |c| {
                c.flags = c.flags | NodeBuilderFlags::FORBID_INDEXED_ACCESS_SYMBOL_REFERENCES
            });
            let symbol = self.ty(t).symbol;
            let final_ref =
                self.symbol_to_type_node(b, symbol, SymbolFlags::TYPE, type_argument_nodes);
            restore_flags();
            if result_type.is_nil() {
                final_ref
            } else {
                self.append_reference_to_type(b, result_type, final_ref)
            }
        }
    }
}
