//! Port of Go `checker/checker.go` lines 2056-2967.

use crate::gostd::Context;
use crate::prelude::*;

// Go: checker/checker.go:2139 isImmediatelyUsedInInitializerOfBlockScopedVariable
pub fn is_immediately_used_in_initializer_of_block_scoped_variable(
    declaration: Node,
    usage: Node,
    decl_container: Node,
) -> bool {
    // PERF: chkA. The grandparent is read once (Go reads it twice).
    let grandparent = declaration.parent().parent();
    match grandparent.kind() {
        SyntaxKind::VariableStatement | SyntaxKind::ForStatement | SyntaxKind::ForOfStatement => {
            // variable statement/for/for-of statement case,
            // use site should not be inside variable declaration (initializer of declaration or binding element)
            if is_same_scope_descendent_of(usage, declaration, decl_container) {
                return true;
            }
        }
        _ => {}
    }
    // ForIn/ForOf case - use site should not be used in expression part
    is_for_in_or_of_statement(grandparent)
        && is_same_scope_descendent_of(usage, grandparent.expression(), decl_container)
}

// Go: checker/checker.go:2157 isSameScopeDescendentOf
// Starting from 'initial' node walk up the parent chain until 'stopAt' node is reached.
// If at any point current node is equal to 'parent' node - return true.
// If current node is an IIFE, continue walking up.
// Return false if 'stopAt' node is reached or isFunctionLike(current) === true.
// PERF: chkA. One walk up the store tables (`find_ancestor_with_kind`,
// which gives each kind), in place of a `parent()` and a `kind()` lookup per
// step. The tests and their order are Go's.
pub fn is_same_scope_descendent_of(initial: Node, parent: Node, stop_at: Node) -> bool {
    if parent.is_nil() {
        return false;
    }
    let mut found = false;
    find_ancestor_with_kind(initial, |n, kind| {
        if n == parent {
            found = true;
            return true;
        }
        n == stop_at
            || is_function_like_kind(kind)
                && (get_immediately_invoked_function_expression(n).is_nil()
                    || get_function_flags(n).intersects(FunctionFlags::ASYNC_GENERATOR))
    });
    found
}

// Go: checker/checker.go:2173 isPropertyImmediatelyReferencedWithinDeclaration
// isPropertyImmediatelyReferencedWithinDeclaration is used for detecting ES-standard class field use-before-def errors
pub fn is_property_immediately_referenced_within_declaration(
    declaration: Node,
    usage: Node,
    stop_at_any_property_declaration: bool,
) -> bool {
    // always legal if usage is after declaration
    if usage.end() > declaration.end() {
        return false;
    }
    // still might be legal if usage is deferred (e.g. x: any = () => this.x)
    // otherwise illegal if immediately referenced within the declaration (e.g. x: any = this.x)
    let mut node = usage;
    while node.is_some() && node != declaration {
        match node.kind() {
            SyntaxKind::ArrowFunction => return false,
            SyntaxKind::PropertyDeclaration => {
                // even when stopping at any property declaration, they need to come from the same class
                return stop_at_any_property_declaration
                    && ((is_property_declaration(declaration)
                        && node.parent() == declaration.parent())
                        || (is_parameter_property_declaration(declaration, declaration.parent())
                            && node.parent() == declaration.parent().parent()));
            }
            SyntaxKind::Block => match node.parent().kind() {
                SyntaxKind::MethodDeclaration
                | SyntaxKind::GetAccessor
                | SyntaxKind::SetAccessor => return false,
                _ => {}
            },
            _ => {}
        }
        node = node.parent();
    }
    true
}

// PORT: Go `node.AsTypeParameterDeclaration().Expression`. The Go `Node.Expression()`
// method panics on type parameters, and fields.rs skips the clashing field name, so
// the field is read from the astdata data directly.
fn type_parameter_declaration_expression(node: Node) -> Node {
    with_ast_data(node, |d| match d {
        crate::astdata::NodeData::TypeParameterDeclaration(d) => match d.expression {
            Some(id) => Node::new(node.file_index(), id),
            None => Node::NIL,
        },
        _ => panic!("AsTypeParameterDeclaration on {:?}", node.kind()),
    })
}

impl Checker {
    // Go: checker/checker.go:2200 getTypeOnlyAliasDeclaration
    // Return the type-only declaration node (if any) for the given alias symbol (non-transitively)
    pub fn get_type_only_alias_declaration(&mut self, symbol: SymbolId) -> Node {
        if self.sym_flags(symbol).intersects(SymbolFlags::ALIAS) {
            self.resolve_alias(symbol);
            return self.alias_symbol_links.get(symbol).type_only_declaration;
        }
        Node::NIL
    }

    // Go: checker/checker.go:2210 getTypeOnlyAliasDeclarationEx
    // Return the first type-only alias declaration node (if any) in the resolution chain that affects
    // the symbol for the given meaning
    pub fn get_type_only_alias_declaration_ex(
        &mut self,
        symbol: SymbolId,
        meaning: SymbolFlags,
    ) -> Node {
        let mut symbol = symbol;
        while self.sym_flags(symbol).intersects(SymbolFlags::ALIAS)
            && !self.sym_flags(symbol).intersects(meaning)
        {
            let resolved = self.resolve_alias(symbol);
            let type_only_declaration = self.alias_symbol_links.get(symbol).type_only_declaration;
            if type_only_declaration.is_some() {
                return type_only_declaration;
            }
            symbol = resolved;
        }
        Node::NIL
    }

    // Go: checker/checker.go:2222 getImmediateAliasedSymbol
    pub fn get_immediate_aliased_symbol(&mut self, symbol: SymbolId) -> SymbolId {
        debug_assert!(
            self.sym_flags(symbol).intersects(SymbolFlags::ALIAS),
            "Should only get Alias here."
        );
        if self
            .alias_symbol_links
            .get(symbol)
            .immediate_target
            .is_nil()
        {
            let node = self.get_declaration_of_alias_symbol(symbol);
            if node.is_nil() {
                panic!("Unexpected nil in getImmediateAliasedSymbol");
            }
            let target = self.get_target_of_alias_declaration(node);
            self.alias_symbol_links.get(symbol).immediate_target = target;
        }
        self.alias_symbol_links.get(symbol).immediate_target
    }

    // Go: checker/checker.go:2235 addTypeOnlyDeclarationRelatedInfo
    // PORT: Go mutates the `*ast.Diagnostic` in place and returns it. Here the
    // owned diagnostic is taken by value and returned with the related info added.
    pub fn add_type_only_declaration_related_info(
        &mut self,
        mut diagnostic: Diagnostic,
        type_only_declaration: Node,
        name: &str,
    ) -> Diagnostic {
        if type_only_declaration.is_nil() {
            return diagnostic;
        }
        let is_export = is_export_specifier(type_only_declaration)
            || is_export_declaration(type_only_declaration)
            || is_namespace_export(type_only_declaration);
        let message = if is_export {
            diag::X_0_was_exported_here
        } else {
            diag::X_0_was_imported_here
        };
        diagnostic.add_related_info(Some(new_diagnostic_for_node(
            type_only_declaration,
            message,
            args![name],
        )));
        diagnostic
    }

    // Go: checker/checker.go:2243 getSymbol
    // PORT: `name` is a `&str` or a `&Name` (`TableKey`).
    pub fn get_symbol<'a>(
        &mut self,
        symbols: SymbolTable,
        name: impl Into<TableKey<'a>>,
        meaning: SymbolFlags,
    ) -> SymbolId {
        self.get_symbol_key(symbols, name.into(), meaning)
    }

    /// `get_symbol` by a `TableKey`. `NameResolver` passes an interned
    /// `TableKey::Name`, which finds the entry by id with no text compare.
    pub fn get_symbol_key(
        &mut self,
        symbols: SymbolTable,
        name: TableKey<'_>,
        meaning: SymbolFlags,
    ) -> SymbolId {
        if meaning.intersects(SymbolFlags::ALL) {
            let found = self.symbols.get(symbols, name);
            let symbol = self.get_merged_symbol(found);
            if symbol.is_some() {
                if self.sym_flags(symbol).intersects(meaning) {
                    return symbol;
                }
                if self.sym_flags(symbol).intersects(SymbolFlags::ALIAS) {
                    let target_flags = self.get_symbol_flags(symbol);
                    // `targetFlags` will be `SymbolFlags.All` if an error occurred in alias resolution; this avoids cascading errors
                    if target_flags.intersects(meaning) {
                        return symbol;
                    }
                }
            }
        }
        // return nil if we can't find a symbol
        SymbolId::NIL
    }

    // Go: checker/checker.go:2263 checkSourceFile
    pub fn check_source_file(&mut self, ctx: &Context, source_file: Node, check_unused: bool) {
        self.ctx = Some(ctx.clone());
        // Go `defer tr.Push(...)()` inside the block: the event ends when the
        // function returns.
        let mut _trace: Option<crate::tracing::Pop> = None;
        if !self.source_file_links.get(source_file).type_checked {
            _trace = self.tracer.map(|tr| {
                tr.push(
                    crate::tracing::Phase::Check,
                    "checkSourceFile",
                    vec![("path", source_file_file_name(source_file).into())],
                    true,
                )
            });
            // Effect-TS/tsgo patch 002: clear any stale relation errors before type checking.
            self.effect_relation_errors.remove(&source_file);
            // Grammar checking
            self.check_grammar_source_file(source_file);
            self.renamed_binding_elements_in_types = Vec::new();
            self.check_source_elements(source_file.statements());
            self.check_deferred_nodes(source_file);
            if is_external_or_common_js_module(source_file) {
                self.check_external_module_exports(source_file);
                self.register_for_unused_identifiers_check(source_file);
            }
            if !with_source_file_info(source_file, |info| info.is_declaration_file)
                && !self.is_canceled()
            {
                self.check_unused_renamed_binding_elements();
            }
            self.produce_deferred_diagnostics();
            self.reported_unreachable_nodes.clear();
            self.source_file_links.get(source_file).type_checked = true;
        }
        // Effect-TS/tsgo patch 002 runs the Effect rules inside the block above, before the
        // unused check. Their type queries can mark a symbol referenced (for example the
        // parameter of a `x is T` type predicate), which hides TS6133. The port runs them
        // once, after the unused check, so that check reads only the TypeScript references.
        // A file with Effect rules always gets the unused check first. Without noUnusedLocals
        // and noUnusedParameters, that check only adds suggestions. A standalone API process
        // runs no Effect rules (`rulerunner::enabled_options`).
        let run_effect = crate::effect::rulerunner::enabled_options(self.compiler_options)
            .is_some()
            && !self.source_file_links.get(source_file).effect_checked;
        if (check_unused || run_effect) && !self.source_file_links.get(source_file).unused_checked {
            // The unused identifiers check relies on a full type check having first been performed
            if !with_source_file_info(source_file, |info| info.is_declaration_file)
                && !self.is_canceled()
            {
                let identifier_check_nodes = self
                    .source_file_links
                    .get(source_file)
                    .identifier_check_nodes
                    .clone();
                self.check_unused_identifiers(&identifier_check_nodes);
            }
            self.source_file_links.get(source_file).unused_checked = true;
        }
        if run_effect {
            // Set first as a guard, not a need: `get_relation_errors` checks a file only
            // when `type_checked` is false, and every rule passes this file, which is
            // already type checked here.
            self.source_file_links.get(source_file).effect_checked = true;
            crate::effect::after_check_source_file(ctx, self, source_file);
        }
        if self.is_canceled() {
            self.was_canceled = true;
        }
        self.ctx = None;
    }

    // Go: checker/checker.go:2299 checkSourceElements
    // PERF: takes the nodes by value (a `NodeSlice` of program data, or
    // copied ids), so callers do not copy a node list into a `Vec` first.
    pub fn check_source_elements(&mut self, nodes: impl IntoIterator<Item = Node>) {
        for node in nodes {
            if self.is_canceled() {
                break;
            }
            self.check_source_element(node);
        }
    }

    // Go: checker/checker.go:2308 checkSourceElement
    pub fn check_source_element(&mut self, node: Node) -> bool {
        if node.is_some() {
            let save_current_node = self.current_node;
            let save_within_unreachable_code = self.within_unreachable_code;
            self.current_node = node;
            self.instantiation_count = 0;
            self.check_source_element_worker(node);
            self.current_node = save_current_node;
            self.within_unreachable_code = save_within_unreachable_code;
        }
        false
    }

    // Go: checker/checker.go:2321 checkSourceElementWorker
    pub fn check_source_element_worker(&mut self, node: Node) {
        for jsdoc in node.eager_js_doc(Node::NIL) {
            self.check_js_doc_comments(jsdoc);
            let tags = jsdoc.tags();
            if !tags.is_nil() {
                for tag in tags.nodes() {
                    self.check_js_doc_comments(tag);
                }
            }
        }

        // PERF: chkport1 item 9. Go reads `node.Kind` as a plain field; here
        // it is an AST store read, so it is read once for the unreachable
        // test and the switch.
        let kind = node.kind();
        if !self.within_unreachable_code
            && self.compiler_options.allow_unreachable_code != Tristate::True
        {
            if self.check_source_element_unreachable(node, kind) {
                self.within_unreachable_code = true;
            }
        }

        match kind {
            SyntaxKind::TypeParameter => self.check_type_parameter(node),
            SyntaxKind::Parameter => self.check_parameter(node),
            SyntaxKind::PropertyDeclaration => self.check_property_declaration(node),
            SyntaxKind::PropertySignature => self.check_property_signature(node),
            SyntaxKind::ConstructorType
            | SyntaxKind::FunctionType
            | SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::IndexSignature => self.check_signature_declaration(node),
            SyntaxKind::MethodDeclaration | SyntaxKind::MethodSignature => {
                self.check_method_declaration(node)
            }
            SyntaxKind::ClassStaticBlockDeclaration => {
                self.check_class_static_block_declaration(node)
            }
            SyntaxKind::Constructor => self.check_constructor_declaration(node),
            SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => {
                self.check_accessor_declaration(node)
            }
            SyntaxKind::TypeReference => {
                self.check_type_reference_node(node);
            }
            SyntaxKind::TypePredicate => {
                self.check_type_predicate(node);
            }
            SyntaxKind::TypeQuery => {
                self.check_type_query(node);
            }
            SyntaxKind::TypeLiteral => {
                self.check_type_literal(node);
            }
            SyntaxKind::ArrayType => {
                self.check_array_type(node);
            }
            SyntaxKind::TupleType => {
                self.check_tuple_type(node);
            }
            SyntaxKind::UnionType | SyntaxKind::IntersectionType => {
                self.check_union_or_intersection_type(node);
            }
            SyntaxKind::ParenthesizedType | SyntaxKind::OptionalType | SyntaxKind::RestType => {
                node.for_each_child(&mut |child: Node| self.check_source_element(child));
            }
            SyntaxKind::ThisType => {
                self.check_this_type(node);
            }
            SyntaxKind::TypeOperator => {
                self.check_type_operator(node);
            }
            SyntaxKind::ConditionalType => {
                self.check_conditional_type(node);
            }
            SyntaxKind::InferType => {
                self.check_infer_type(node);
            }
            SyntaxKind::TemplateLiteralType => {
                self.check_template_literal_type(node);
            }
            SyntaxKind::ImportType => {
                self.check_import_type(node);
            }
            SyntaxKind::NamedTupleMember => {
                self.check_named_tuple_member(node);
            }
            SyntaxKind::IndexedAccessType => {
                self.check_indexed_access_type(node);
            }
            SyntaxKind::MappedType => {
                self.check_mapped_type(node);
            }
            SyntaxKind::FunctionDeclaration => {
                self.check_function_declaration(node);
            }
            SyntaxKind::Block | SyntaxKind::ModuleBlock => {
                self.check_block(node);
            }
            SyntaxKind::VariableStatement => {
                self.check_variable_statement(node);
            }
            SyntaxKind::ExpressionStatement => {
                self.check_expression_statement(node);
            }
            SyntaxKind::IfStatement => {
                self.check_if_statement(node);
            }
            SyntaxKind::DoStatement => {
                self.check_do_statement(node);
            }
            SyntaxKind::WhileStatement => {
                self.check_while_statement(node);
            }
            SyntaxKind::ForStatement => {
                self.check_for_statement(node);
            }
            SyntaxKind::ForInStatement => {
                self.check_for_in_statement(node);
            }
            SyntaxKind::ForOfStatement => {
                self.check_for_of_statement(node);
            }
            SyntaxKind::ContinueStatement | SyntaxKind::BreakStatement => {
                self.check_break_or_continue_statement(node);
            }
            SyntaxKind::ReturnStatement => {
                self.check_return_statement(node);
            }
            SyntaxKind::WithStatement => {
                self.check_with_statement(node);
            }
            SyntaxKind::SwitchStatement => {
                self.check_switch_statement(node);
            }
            SyntaxKind::LabeledStatement => {
                self.check_labeled_statement(node);
            }
            SyntaxKind::ThrowStatement => {
                self.check_throw_statement(node);
            }
            SyntaxKind::TryStatement => {
                self.check_try_statement(node);
            }
            SyntaxKind::VariableDeclaration => {
                self.check_variable_declaration(node);
            }
            SyntaxKind::BindingElement => {
                self.check_binding_element(node);
            }
            SyntaxKind::ClassDeclaration => {
                self.check_class_declaration(node);
            }
            SyntaxKind::InterfaceDeclaration => {
                self.check_interface_declaration(node);
            }
            SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration => {
                self.check_type_alias_declaration(node);
            }
            SyntaxKind::EnumDeclaration => {
                self.check_enum_declaration(node);
            }
            SyntaxKind::EnumMember => {
                self.check_enum_member(node);
            }
            SyntaxKind::ModuleDeclaration => {
                self.check_module_declaration(node);
            }
            SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
                self.check_import_declaration(node);
            }
            SyntaxKind::ImportEqualsDeclaration => {
                self.check_import_equals_declaration(node);
            }
            SyntaxKind::ExportDeclaration => {
                self.check_export_declaration(node);
            }
            SyntaxKind::ExportAssignment => {
                self.check_export_assignment(node);
            }
            SyntaxKind::EmptyStatement => {
                self.check_grammar_statement_in_ambient_context(node);
            }
            SyntaxKind::DebuggerStatement => {
                self.check_grammar_statement_in_ambient_context(node);
            }
            SyntaxKind::MissingDeclaration => {
                self.check_missing_declaration(node);
            }
            SyntaxKind::JsDocNonNullableType
            | SyntaxKind::JsDocNullableType
            | SyntaxKind::JsDocAllType
            | SyntaxKind::JsDocTypeLiteral => {
                self.check_js_doc_type(node);
            }
            _ => {}
        }
    }

    // Go: checker/checker.go:2459 checkSourceElementUnreachable
    // PERF: chkport1 item 9. `kind` is `node.Kind`, read once by the caller
    // (`is_potentially_executable_kind`). The reported set is empty in most
    // programs, so its hash lookup is skipped while it is empty.
    pub fn check_source_element_unreachable(&mut self, node: Node, kind: SyntaxKind) -> bool {
        if !is_potentially_executable_kind(node, kind) {
            return false;
        }

        if !self.reported_unreachable_nodes.is_empty()
            && self.reported_unreachable_nodes.contains(&node)
        {
            return true;
        }

        if !self.is_source_element_unreachable(node) {
            return false;
        }

        self.reported_unreachable_nodes.insert(node);

        let source_file = get_source_file_of_node(node);

        let mut start_node = node;
        let mut end_node = node;

        let parent = node.parent();
        if parent.can_have_statements() {
            let statements = parent.statements().to_vec();
            if let Some(offset) = statements.iter().position(|&s| s == node) {
                // Scan backwards to find the first unreachable unreported node;
                // this may happen when producing region diagnostics where not all nodes
                // will have been visited.
                // TODO: enable this code once we support region diagnostics again.
                let first = offset;

                let mut last = offset;
                for i in (offset + 1)..statements.len() {
                    let next_node = statements[i];
                    if !is_potentially_executable_kind(next_node, next_node.kind())
                        || !self.is_source_element_unreachable(next_node)
                    {
                        break;
                    }
                    last = i;
                    self.reported_unreachable_nodes.insert(next_node);
                }

                start_node = statements[first];
                end_node = statements[last];
            }
        }

        let start = get_token_pos_of_node(start_node, source_file, false /*includeJSDoc*/);

        let diagnostic = new_diagnostic(
            source_file,
            TextRange::new(start, end_node.end()),
            diag::Unreachable_code_detected,
            args![],
        );
        let is_error = self.compiler_options.allow_unreachable_code == Tristate::False;
        self.add_error_or_suggestion(is_error, diagnostic);

        true
    }

    // Go: checker/checker.go:2520 isSourceElementUnreachable
    pub fn is_source_element_unreachable(&mut self, node: Node) -> bool {
        // Precondition: ast.IsPotentiallyExecutableNode is true
        if node.flags().intersects(NodeFlags::UNREACHABLE) {
            // The binder has determined that this code is unreachable.
            // Ignore const enums unless preserveConstEnums is set.
            return match node.kind() {
                SyntaxKind::EnumDeclaration => {
                    !is_enum_const(node) || self.compiler_options.should_preserve_const_enums()
                }
                SyntaxKind::ModuleDeclaration => is_instantiated_module(
                    node,
                    self.compiler_options.should_preserve_const_enums(),
                ),
                _ => true,
            };
        } else {
            let flow_node = node.flow_node();
            if flow_node.is_some() {
                // For code the binder doesn't know is unreachable, use control flow / types.
                return !self.is_reachable_flow_node(flow_node);
            }
        }
        false
    }

    // Go: checker/checker.go:2551 checkNodeDeferred
    // Function and class expression bodies are checked after all statements in the enclosing body. This is
    // to ensure constructs like the following are permitted:
    //
    //	const foo = function () {
    //	   const s = foo();
    //	   return "hello";
    //	}
    //
    // Here, performing a full type check of the body of the function expression whilst in the process of
    // determining the type of foo would cause foo to be given type any because of the recursive reference.
    // Delaying the type check of the body ensures foo has been assigned a type.
    pub fn check_node_deferred(&mut self, node: Node) {
        let enclosing_file = get_source_file_of_node(node);
        let links = self.source_file_links.get(enclosing_file);
        if !links.type_checked {
            links.deferred_nodes.insert(node);
        }
    }

    // Go: checker/checker.go:2559 checkDeferredNodes
    // PORT: Go iterates the OrderedSet by index so nodes added during iteration are
    // visited too; the index loop below re-reads the set each step to match.
    pub fn check_deferred_nodes(&mut self, context: Node) {
        let mut i = 0;
        loop {
            let node = match self
                .source_file_links
                .get(context)
                .deferred_nodes
                .get_index(i)
            {
                Some(&node) => node,
                None => break,
            };
            if self.is_canceled() {
                break;
            }
            self.check_deferred_node(node);
            i += 1;
        }
        // Go (#4825) replaces the set so its storage is freed.
        self.source_file_links.get(context).deferred_nodes = IndexSet::default();
    }

    // Go: checker/checker.go:2570 checkDeferredNode
    pub fn check_deferred_node(&mut self, node: Node) {
        let _trace = self.tracer.map(|tr| {
            tr.push(
                crate::tracing::Phase::Check,
                "checkDeferredNode",
                crate::tracing::node_args(node),
                false,
            )
        });
        let save_current_node = self.current_node;
        self.current_node = node;
        self.instantiation_count = 0;
        match node.kind() {
            SyntaxKind::CallExpression
            | SyntaxKind::NewExpression
            | SyntaxKind::TaggedTemplateExpression
            | SyntaxKind::Decorator
            | SyntaxKind::JsxOpeningElement => {
                // These node kinds are deferred checked when overload resolution fails. To save on work,
                // we ensure the arguments are checked just once in a deferred way.
                self.resolve_untyped_call(node);
            }
            SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature => {
                self.check_function_expression_or_object_literal_method_deferred(node);
            }
            SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => {
                self.check_accessor_declaration(node);
            }
            SyntaxKind::ClassExpression => {
                self.check_class_expression_deferred(node);
            }
            SyntaxKind::TypeParameter => {
                self.check_type_parameter_deferred(node);
            }
            SyntaxKind::JsxSelfClosingElement => {
                self.check_jsx_self_closing_element_deferred(node);
            }
            SyntaxKind::JsxElement => {
                self.check_jsx_element_deferred(node);
            }
            SyntaxKind::TypeAssertionExpression | SyntaxKind::AsExpression => {
                self.check_assertion_deferred(node);
            }
            SyntaxKind::VoidExpression => {
                self.check_expression(node.expression());
            }
            SyntaxKind::BinaryExpression => {
                if is_instance_of_expression(node) {
                    self.resolve_untyped_call(node);
                }
            }
            SyntaxKind::ObjectLiteralExpression | SyntaxKind::JsxAttributes => {
                self.check_contextual_deprecations(node);
            }
            _ => {}
        }
        self.current_node = save_current_node;
    }

    // Go: checker/checker.go:2608 checkJSDocComments
    pub fn check_js_doc_comments(&mut self, node: Node) {
        for comment in node.comments() {
            self.check_js_doc_comment(comment);
        }
    }

    // Go: checker/checker.go:2614 checkJSDocComment
    pub fn check_js_doc_comment(&mut self, node: Node) {
        // This performs minimal checking of JSDoc nodes to ensure that @link references to entities are recorded
        // for purposes of checking unused identifiers.
        match node.kind() {
            SyntaxKind::JsDocLink | SyntaxKind::JsDocLinkCode | SyntaxKind::JsDocLinkPlain => {
                self.resolve_js_doc_member_name(node.name());
            }
            _ => {}
        }
    }

    // Go: checker/checker.go:2623 resolveJSDocMemberName
    pub fn resolve_js_doc_member_name(&mut self, name: Node) -> SymbolId {
        if name.is_some() && is_entity_name(name) {
            let meaning = SymbolFlags::TYPE | SymbolFlags::NAMESPACE | SymbolFlags::VALUE;
            let symbol = self.resolve_entity_name(
                name,
                meaning,
                true, /*ignoreErrors*/
                true, /*dontResolveAlias*/
                get_host_signature_from_js_doc(name),
            );
            if symbol.is_some() {
                return symbol;
            }
            if is_qualified_name(name) {
                let symbol = self.resolve_js_doc_member_name(name.left());
                if symbol.is_some() {
                    let mut t = TypeId::NIL;
                    if self.sym_flags(symbol).intersects(SymbolFlags::VALUE) {
                        let symbol_type = self.get_type_of_symbol(symbol);
                        let proto = self.get_property_of_type(symbol_type, "prototype");
                        if proto.is_some() {
                            t = self.get_type_of_symbol(proto);
                        }
                    }
                    if t.is_nil() {
                        t = self.get_declared_type_of_symbol(symbol);
                    }
                    return self.get_property_of_type(t, name.right().text());
                }
            }
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:2648 checkJSDocType
    pub fn check_js_doc_type(&mut self, node: Node) {
        self.check_js_doc_type_is_in_js_file(node);
        node.for_each_child(&mut |child: Node| self.check_source_element(child));
    }

    // Go: checker/checker.go:2653 checkJSDocTypeIsInJsFile
    pub fn check_js_doc_type_is_in_js_file(&mut self, node: Node) {
        if !is_in_js_file(node) {
            if is_js_doc_non_nullable_type(node) || is_js_doc_nullable_type(node) {
                let token = if is_js_doc_non_nullable_type(node) {
                    "!"
                } else {
                    "?"
                };
                let postfix = node.pos() == node.type_().pos();
                let message = if postfix {
                    diag::X_0_at_the_end_of_a_type_is_not_valid_TypeScript_syntax_Did_you_mean_to_write_1
                } else {
                    diag::X_0_at_the_start_of_a_type_is_not_valid_TypeScript_syntax_Did_you_mean_to_write_1
                };
                let mut t = self.get_type_from_type_node(node.type_());
                if is_js_doc_nullable_type(node) && t != self.never_type && t != self.void_type {
                    t = self.get_nullable_type(
                        t,
                        if postfix {
                            TypeFlags::UNDEFINED
                        } else {
                            TypeFlags::NULLABLE
                        },
                    );
                }
                let type_string = self.type_to_string_exported(t);
                self.grammar_error_on_node(node, message, args![token, type_string]);
            } else {
                self.grammar_error_on_node(
                    node,
                    diag::JSDoc_types_can_only_be_used_inside_documentation_comments,
                    args![],
                );
            }
        }
    }

    // Go: checker/checker.go:2672 checkTypeParameter
    pub fn check_type_parameter(&mut self, node: Node) {
        // Grammar Checking
        self.check_grammar_modifiers(node);
        let expr = type_parameter_declaration_expression(node);
        if expr.is_some() {
            self.grammar_error_on_first_token(expr, diag::Type_expected, args![]);
        }
        let constraint = node.constraint();
        let default_type_node = node.default_type();
        self.check_source_element(constraint);
        self.check_source_element(default_type_node);
        let symbol = self.get_symbol_of_declaration(node);
        let type_parameter = self.get_declared_type_of_type_parameter(symbol);
        // Resolve base constraint to reveal circularity errors
        self.get_base_constraint_of_type(type_parameter);
        if self.get_resolved_type_parameter_default(type_parameter) == self.circular_constraint_type
        {
            let type_string = self.type_to_string_exported(type_parameter);
            self.error(
                default_type_node,
                diag::Type_parameter_0_has_a_circular_default,
                args![type_string],
            );
        }
        let constraint_type = self.get_constraint_of_type_parameter(type_parameter);
        let default_type = self.get_default_from_type_parameter(type_parameter);
        if constraint_type.is_some() && default_type.is_some() {
            let mapper = self.new_simple_type_mapper(type_parameter, default_type);
            let instantiated = self.instantiate_type(constraint_type, mapper);
            let target = self.get_type_with_this_argument(instantiated, default_type, false);
            self.check_type_assignable_to(
                default_type,
                target,
                default_type_node,
                Some(diag::Type_0_does_not_satisfy_the_constraint_1),
            );
        }
        self.check_type_name_is_reserved(node.name(), diag::Type_parameter_name_cannot_be_0);
        self.check_node_deferred(node);
    }

    // Go: checker/checker.go:2696 checkTypeParameterDeferred
    pub fn check_type_parameter_deferred(&mut self, node: Node) {
        let parent = node.parent();
        if is_interface_declaration(parent)
            || is_class_like(parent)
            || is_type_or_js_type_alias_declaration(parent)
        {
            let tp_symbol = self.get_symbol_of_declaration(node);
            let type_parameter = self.get_declared_type_of_type_parameter(tp_symbol);
            let modifiers = self.get_type_parameter_modifiers(type_parameter)
                & (ModifierFlags::IN | ModifierFlags::OUT);
            if modifiers != ModifierFlags::NONE {
                let symbol = self.get_symbol_of_declaration(parent);
                let is_alias_without_object_type = is_type_or_js_type_alias_declaration(parent)
                    && {
                        let declared = self.get_declared_type_of_symbol(symbol);
                        !self
                            .ty(declared)
                            .object_flags
                            .intersects(ObjectFlags::ANONYMOUS | ObjectFlags::MAPPED)
                    };
                if is_alias_without_object_type {
                    self.error(
                        node,
                        diag::Variance_annotations_are_only_supported_in_type_aliases_for_object_function_constructor_and_mapped_types,
                        args![],
                    );
                } else if modifiers == ModifierFlags::IN || modifiers == ModifierFlags::OUT {
                    let _trace = self.tracer.map(|tr| {
                        let parent_type = self.get_declared_type_of_symbol(symbol);
                        tr.push(
                            crate::tracing::Phase::CheckTypes,
                            "checkTypeParameterDeferred",
                            vec![
                                ("parent", parent_type.into()),
                                ("id", type_parameter.into()),
                            ],
                            false,
                        )
                    });
                    let source_marker = if modifiers == ModifierFlags::OUT {
                        self.marker_sub_type_for_check
                    } else {
                        self.marker_super_type_for_check
                    };
                    let source = self.create_marker_type(symbol, type_parameter, source_marker);
                    let target_marker = if modifiers == ModifierFlags::OUT {
                        self.marker_super_type_for_check
                    } else {
                        self.marker_sub_type_for_check
                    };
                    let target = self.create_marker_type(symbol, type_parameter, target_marker);
                    // PORT: Go saves `typeParameter` (not the previous value) and restores it; kept as is.
                    let save_variance_type_parameter = type_parameter;
                    self.variance_type_parameter = type_parameter;
                    self.check_type_assignable_to(
                        source,
                        target,
                        node,
                        Some(diag::Type_0_is_not_assignable_to_type_1_as_implied_by_variance_annotation),
                    );
                    self.variance_type_parameter = save_variance_type_parameter;
                }
            }
        }
    }

    // Go: checker/checker.go:2719 shouldCheckErasableSyntax
    pub fn should_check_erasable_syntax(&self, node: Node) -> bool {
        self.compiler_options.erasable_syntax_only.is_true() && !is_in_js_file(node)
    }

    // Go: checker/checker.go:2723 checkParameter
    pub fn check_parameter(&mut self, node: Node) {
        // Grammar checking
        // It is a SyntaxError if the Identifier "eval" or the Identifier "arguments" occurs as the
        // Identifier in a PropertySetParameterList of a PropertyAssignment that is contained in strict code
        // or if its FunctionBody is strict code(11.1.5).
        self.check_grammar_modifiers(node);
        self.check_variable_like_declaration(node);
        let fn_ = get_containing_function(node);
        let mut param_name = "";
        if node.name().is_some() && is_identifier(node.name()) {
            param_name = node.name().text();
        }
        if has_syntactic_modifier(node, ModifierFlags::PARAMETER_PROPERTY_MODIFIER) {
            if self.should_check_erasable_syntax(node) {
                self.error(
                    node,
                    diag::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                    args![],
                );
            }
            if !(is_constructor_declaration(fn_) && node_is_present(fn_.body())) {
                self.error(
                    node,
                    diag::A_parameter_property_is_only_allowed_in_a_constructor_implementation,
                    args![],
                );
            }
            if is_constructor_declaration(fn_) && param_name == "constructor" {
                self.error(
                    node.name(),
                    diag::X_constructor_cannot_be_used_as_a_parameter_property_name,
                    args![],
                );
            }
        }
        if node.initializer().is_nil()
            && is_optional_declaration(node)
            && is_binding_pattern(node.name())
            && fn_.body().is_some()
        {
            self.error(
                node,
                diag::A_binding_pattern_parameter_cannot_be_optional_in_an_implementation_signature,
                args![],
            );
        }
        if param_name == "this" || param_name == "new" {
            if fn_.parameters().to_vec().iter().position(|&p| p == node) != Some(0) {
                self.error(
                    node,
                    diag::A_0_parameter_must_be_the_first_parameter,
                    args![param_name],
                );
            }
            if is_constructor_declaration(fn_)
                || is_construct_signature_declaration(fn_)
                || is_constructor_type_node(fn_)
            {
                self.error(
                    node,
                    diag::A_constructor_cannot_have_a_this_parameter,
                    args![],
                );
            }
            if is_arrow_function(fn_) {
                self.error(
                    node,
                    diag::An_arrow_function_cannot_have_a_this_parameter,
                    args![],
                );
            }
            if is_accessor(fn_) {
                self.error(
                    node,
                    diag::X_get_and_set_accessors_cannot_declare_this_parameters,
                    args![],
                );
            }
        }
        // Only check rest parameter type if it's not a binding pattern. Since binding patterns are
        // not allowed in a rest parameter, we already have an error from checkGrammarParameterList.
        if has_dot_dot_dot_token(node) && !is_binding_pattern(node.name()) {
            let symbol_type = self.get_type_of_symbol(node.symbol());
            let reduced = self.get_reduced_type(symbol_type);
            let any_readonly_array_type = self.any_readonly_array_type;
            if !self.is_type_assignable_to(reduced, any_readonly_array_type) {
                self.error(
                    node,
                    diag::A_rest_parameter_must_be_of_an_array_type,
                    args![],
                );
            }
        }
    }

    // Go: checker/checker.go:2770 checkPropertyDeclaration
    pub fn check_property_declaration(&mut self, node: Node) {
        // Grammar checking
        if !self.check_grammar_modifiers(node) && !self.check_grammar_property(node) {
            self.check_grammar_computed_property_name(node.name());
        }
        self.check_variable_like_declaration(node);
        self.set_node_links_for_private_identifier_scope(node);
        // property signatures already report "initializer not allowed in ambient context" elsewhere
        if has_syntactic_modifier(node, ModifierFlags::ABSTRACT) && is_property_declaration(node) {
            if node.initializer().is_some() {
                self.error(
                    node,
                    diag::Property_0_cannot_have_an_initializer_because_it_is_marked_abstract,
                    args![declaration_name_to_string(node.name())],
                );
            }
        }
    }

    // Go: checker/checker.go:2785 checkPropertySignature
    pub fn check_property_signature(&mut self, node: Node) {
        if is_private_identifier(node.name()) {
            self.error(
                node,
                diag::Private_identifiers_are_not_allowed_outside_class_bodies,
                args![],
            );
        }
        self.check_property_declaration(node);
    }

    // Go: checker/checker.go:2792 checkSignatureDeclaration
    pub fn check_signature_declaration(&mut self, node: Node) {
        // Grammar checking
        match node.kind() {
            SyntaxKind::IndexSignature => {
                self.check_grammar_index_signature(node);
            }
            SyntaxKind::FunctionType
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ConstructorType
            | SyntaxKind::CallSignature
            | SyntaxKind::Constructor
            | SyntaxKind::ConstructSignature => {
                self.check_grammar_function_like_declaration(node);
            }
            _ => {}
        }
        let function_flags = get_function_flags(node);
        if !function_flags.intersects(FunctionFlags::INVALID) {
            // Async generators prior to ES2018 require the __await and __asyncGenerator helpers
            if (function_flags & FunctionFlags::ASYNC_GENERATOR) == FunctionFlags::ASYNC_GENERATOR
                && self.language_version < LANGUAGE_FEATURE_MINIMUM_TARGET.async_generators
            {
                self.check_external_emit_helpers(
                    node,
                    ExternalEmitHelpers::ASYNC_GENERATOR_INCLUDES,
                );
            }
            if (function_flags & FunctionFlags::ASYNC_GENERATOR) == FunctionFlags::ASYNC
                && self.language_version < LANGUAGE_FEATURE_MINIMUM_TARGET.async_functions
            {
                self.check_external_emit_helpers(node, ExternalEmitHelpers::AWAITER);
            }
        }
        self.check_type_parameters(node.type_parameters());
        self.check_unmatched_js_doc_parameters(node);
        self.check_source_elements(node.parameters());
        let return_type_node = node.type_();
        if return_type_node.is_some() {
            self.check_source_element(return_type_node);
        }
        if self.no_implicit_any && return_type_node.is_nil() {
            match node.kind() {
                SyntaxKind::ConstructSignature => {
                    self.error(
                        node,
                        diag::Construct_signature_which_lacks_return_type_annotation_implicitly_has_an_any_return_type,
                        args![],
                    );
                }
                SyntaxKind::CallSignature => {
                    self.error(
                        node,
                        diag::Call_signature_which_lacks_return_type_annotation_implicitly_has_an_any_return_type,
                        args![],
                    );
                }
                _ => {}
            }
        }
        if return_type_node.is_some() {
            if (function_flags & (FunctionFlags::INVALID | FunctionFlags::GENERATOR))
                == FunctionFlags::GENERATOR
            {
                let return_type = self.get_type_from_type_node(return_type_node);
                if return_type == self.void_type {
                    self.error(
                        return_type_node,
                        diag::A_generator_cannot_have_a_void_type_annotation,
                        args![],
                    );
                } else {
                    self.check_generator_instantiation_assignability_to_return_type(
                        return_type,
                        function_flags,
                        return_type_node,
                    );
                }
            } else if (function_flags & FunctionFlags::ASYNC_GENERATOR) == FunctionFlags::ASYNC {
                self.check_async_function_return_type(node, return_type_node);
            }
        }
        if !is_index_signature_declaration(node) {
            self.register_for_unused_identifiers_check(node);
        }
    }

    // Go: checker/checker.go:2851 checkAsyncFunctionReturnType
    // Checks the return type of an async function to ensure it is a compatible
    // Promise implementation.
    //
    // This checks that an async function has a valid Promise-compatible return type.
    // An async function has a valid Promise-compatible return type if the resolved value
    // of the return type has a construct signature that takes in an `initializer` function
    // that in turn supplies a `resolve` function as one of its arguments and results in an
    // object with a callable `then` signature.
    pub fn check_async_function_return_type(&mut self, node: Node, return_type_node: Node) {
        let return_type = self.get_type_from_type_node(return_type_node);
        if self.is_error_type(return_type) {
            return;
        }
        let get_global_promise_type_checked = self.get_global_promise_type_checked.clone();
        let global_promise_type = get_global_promise_type_checked(self);
        if global_promise_type != self.empty_generic_type
            && !self.is_reference_to_type(return_type, global_promise_type)
        {
            // The promise type was not a valid type reference to the global promise type, so we
            // report an error and return the unknown type.
            let awaited = self.get_awaited_type_no_alias(return_type);
            let shown = if awaited.is_some() {
                awaited
            } else {
                self.void_type
            };
            let type_string = self.type_to_string_exported(shown);
            self.error(
                return_type_node,
                diag::The_return_type_of_an_async_function_or_method_must_be_the_global_Promise_T_type_Did_you_mean_to_write_Promise_0,
                args![type_string],
            );
            return;
        }
        self.check_awaited_type(
            return_type,
            false, /*withAlias*/
            node,
            diag::The_return_type_of_an_async_function_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member,
        );
    }

    // Go: checker/checker.go:2866 checkMethodDeclaration
    pub fn check_method_declaration(&mut self, node: Node) {
        // Grammar checking
        if !self.check_grammar_method(node) {
            self.check_grammar_computed_property_name(node.name());
            if is_method_declaration(node)
                && node.asterisk_token().is_some()
                && is_identifier(node.name())
                && node.name().text() == "constructor"
            {
                self.error(
                    node.name(),
                    diag::Class_constructor_may_not_be_a_generator,
                    args![],
                );
            }
        }
        // Grammar checking for modifiers is done inside the function checkGrammarFunctionLikeDeclaration
        self.check_function_or_method_declaration(node);
        // method signatures already report "implementation not allowed in ambient context" elsewhere
        if has_syntactic_modifier(node, ModifierFlags::ABSTRACT)
            && is_method_declaration(node)
            && node.body().is_some()
        {
            self.error(
                node,
                diag::Method_0_cannot_have_an_implementation_because_it_is_marked_abstract,
                args![declaration_name_to_string(node.name())],
            );
        }
        // Private named methods are only allowed in class declarations
        if is_private_identifier(node.name()) && get_containing_class(node).is_nil() {
            self.error(
                node,
                diag::Private_identifiers_are_not_allowed_outside_class_bodies,
                args![],
            );
        }
        self.set_node_links_for_private_identifier_scope(node);
    }

    // Go: checker/checker.go:2887 checkClassStaticBlockDeclaration
    pub fn check_class_static_block_declaration(&mut self, node: Node) {
        // Grammar checking
        self.check_grammar_modifiers(node);
        node.for_each_child(&mut |child: Node| self.check_source_element(child));
        if self.symbols.len(node.locals()) != 0 {
            self.register_for_unused_identifiers_check(node);
        }
    }

    // Go: checker/checker.go:2896 checkConstructorDeclaration
    pub fn check_constructor_declaration(&mut self, node: Node) {
        // Grammar check on signature of constructor and modifier of the constructor is done in checkSignatureDeclaration function.
        self.check_signature_declaration(node);
        // Grammar check for checking only related to constructorDeclaration
        if !self.check_grammar_constructor_type_parameters(node) {
            self.check_grammar_constructor_type_annotation(node);
        }
        self.check_source_element(node.body());
        let symbol = self.get_symbol_of_declaration(node);
        self.check_function_or_constructor_symbol(symbol);
        // exit early in the case of signature - super checks are not relevant to them
        if node_is_missing(node.body()) {
            return;
        }
        // TS 1.0 spec (April 2014): 8.3.2
        // Constructors of classes with no extends clause may not contain super calls, whereas
        // constructors of derived classes must contain at least one super call somewhere in their function body.
        let containing_class_decl = node.parent();
        if get_class_extends_heritage_element(containing_class_decl).is_nil() {
            return;
        }
        let class_extends_null = self.class_declaration_extends_null(containing_class_decl);
        let super_call = self.find_first_super_call(node.body());
        if super_call.is_some() {
            if class_extends_null {
                self.error(
                    super_call,
                    diag::A_constructor_cannot_contain_a_super_call_when_its_class_extends_null,
                    args![],
                );
            }
            // A super call must be root-level in a constructor if both of the following are true:
            // - The containing class is a derived class.
            // - The constructor declares parameter properties
            //   or the containing class declares instance member variables with initializers.
            let super_call_should_be_root_level = !self.emit_standard_class_fields
                && (node
                    .parent()
                    .members()
                    .iter()
                    .any(is_instance_property_with_initializer_or_private_identifier_property)
                    || node.parameters().iter().any(|p: Node| {
                        has_syntactic_modifier(p, ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
                    }));
            if super_call_should_be_root_level {
                // Until we have better flow analysis, it is an error to place the super call within any kind of block or conditional
                // See GH #8277
                if !super_call_is_root_level_in_constructor(super_call, node.body()) {
                    self.error(
                        super_call,
                        diag::A_super_call_must_be_a_root_level_statement_within_a_constructor_of_a_derived_class_that_contains_initialized_properties_parameter_properties_or_private_identifiers,
                        args![],
                    );
                } else {
                    let mut super_call_statement = Node::NIL;
                    for statement in node.body().statements() {
                        if is_expression_statement(statement)
                            && is_super_call(skip_outer_expressions(
                                statement.expression(),
                                OuterExpressionKinds::OEK_ALL,
                            ))
                        {
                            super_call_statement = statement;
                            break;
                        }
                        if node_immediately_references_super_or_this(statement) {
                            break;
                        }
                    }
                    // Until we have better flow analysis, it is an error to place the super call within any kind of block or conditional
                    // See GH #8277
                    if super_call_statement.is_nil() {
                        self.error(
                            node,
                            diag::A_super_call_must_be_the_first_statement_in_the_constructor_to_refer_to_super_or_this_when_a_derived_class_contains_initialized_properties_parameter_properties_or_private_identifiers,
                            args![],
                        );
                    }
                }
            }
        } else if !class_extends_null {
            self.error(
                node,
                diag::Constructors_for_derived_classes_must_contain_a_super_call,
                args![],
            );
        }
    }

    // Go: checker/checker.go:2961 findFirstSuperCall
    pub fn find_first_super_call(&self, node: Node) -> Node {
        fn visit(node: Node, super_call: &mut Node) -> bool {
            if is_super_call(node) {
                *super_call = node;
                return true;
            }
            if is_function_like(node) {
                return false;
            }
            node.for_each_child(&mut |child: Node| visit(child, super_call))
        }
        let mut super_call = Node::NIL;
        visit(node, &mut super_call);
        super_call
    }
}

// Go: checker/checker.go:2978 isInstancePropertyWithInitializerOrPrivateIdentifierProperty
pub fn is_instance_property_with_initializer_or_private_identifier_property(n: Node) -> bool {
    is_private_identifier_class_element_declaration(n)
        || is_property_declaration(n) && !is_static(n) && n.initializer().is_some()
}

// Go: checker/checker.go:2982 superCallIsRootLevelInConstructor
pub fn super_call_is_root_level_in_constructor(super_call: Node, body: Node) -> bool {
    let super_call_parent = walk_up_parenthesized_expressions(super_call.parent());
    is_expression_statement(super_call_parent) && super_call_parent.parent() == body
}

// Go: checker/checker.go:2987 nodeImmediatelyReferencesSuperOrThis
pub fn node_immediately_references_super_or_this(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::SuperKeyword | SyntaxKind::ThisKeyword => return true,
        SyntaxKind::ArrowFunction
        | SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::PropertyDeclaration => return false,
        SyntaxKind::Block => match node.parent().kind() {
            SyntaxKind::Constructor
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor => return false,
            _ => {}
        },
        _ => {}
    }
    node.for_each_child(&mut |child: Node| node_immediately_references_super_or_this(child))
}

impl Checker {
    // Go: checker/checker.go:3002 checkAccessorDeclaration
    pub fn check_accessor_declaration(&mut self, node: Node) {
        // Grammar checking accessors
        if !self.check_grammar_function_like_declaration(node) && !self.check_grammar_accessor(node)
        {
            self.check_grammar_computed_property_name(node.name());
        }
        let name = node.name();
        if is_identifier(name) && name.text() == "constructor" && is_class_like(node.parent()) {
            self.error(
                node.name(),
                diag::Class_constructor_may_not_be_an_accessor,
                args![],
            );
        }
        self.check_decorators(node);
        self.check_signature_declaration(node);
        if is_get_accessor_declaration(node) {
            if !node.flags().intersects(NodeFlags::AMBIENT)
                && node_is_present(node.body())
                && node.flags().intersects(NodeFlags::HAS_IMPLICIT_RETURN)
            {
                if !node.flags().intersects(NodeFlags::HAS_EXPLICIT_RETURN) {
                    self.error(name, diag::A_get_accessor_must_return_a_value, args![]);
                }
            }
        }
        // Do not use hasDynamicName here, because that returns false for well known symbols.
        // We want to perform checkComputedPropertyName for all computed properties, including
        // well known symbols.
        if is_computed_property_name(name) {
            self.check_computed_property_name(name);
        }
        if self.has_bindable_name(node) {
            // TypeScript 1.0 spec (April 2014): 8.4.3
            // Accessors for the same member name must specify the same accessibility.
            let symbol = self.get_symbol_of_declaration(node);
            let getter = get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::GetAccessor);
            let setter = get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::SetAccessor);
            if getter.is_some()
                && setter.is_some()
                && !self
                    .node_links
                    .get(getter)
                    .flags
                    .intersects(NodeCheckFlags::TYPE_CHECKED)
            {
                self.node_links.get(getter).flags |= NodeCheckFlags::TYPE_CHECKED;
                let getter_flags = getter.modifier_flags();
                let setter_flags = setter.modifier_flags();
                if (getter_flags & ModifierFlags::ABSTRACT)
                    != (setter_flags & ModifierFlags::ABSTRACT)
                {
                    self.error(
                        getter.name(),
                        diag::Accessors_must_both_be_abstract_or_non_abstract,
                        args![],
                    );
                    self.error(
                        setter.name(),
                        diag::Accessors_must_both_be_abstract_or_non_abstract,
                        args![],
                    );
                }
                if (getter_flags.intersects(ModifierFlags::PROTECTED)
                    && !setter_flags.intersects(ModifierFlags::PROTECTED | ModifierFlags::PRIVATE))
                    || (getter_flags.intersects(ModifierFlags::PRIVATE)
                        && !setter_flags.intersects(ModifierFlags::PRIVATE))
                {
                    self.error(
                        getter.name(),
                        diag::A_get_accessor_must_be_at_least_as_accessible_as_the_setter,
                        args![],
                    );
                    self.error(
                        setter.name(),
                        diag::A_get_accessor_must_be_at_least_as_accessible_as_the_setter,
                        args![],
                    );
                }
            }
        }
        let accessor_symbol = self.get_symbol_of_declaration(node);
        let return_type = self.get_type_of_accessors(accessor_symbol);
        if node.kind() == SyntaxKind::GetAccessor {
            self.check_all_code_paths_in_non_void_function_return_or_throw(node, return_type);
        }
        self.check_source_element(node.body());
        self.set_node_links_for_private_identifier_scope(node);
    }
}

// Go: ast/utilities.go:4351 IsPotentiallyExecutableNode
/// `is_potentially_executable_node(node)` with `kind`, the kind of `node`
/// that the caller read.
// PERF: chkport1 item 9. The ast function reads the kind up to five times.
fn is_potentially_executable_kind(node: Node, kind: SyntaxKind) -> bool {
    if (SyntaxKind::FIRST_STATEMENT as u16) <= (kind as u16)
        && (kind as u16) <= (SyntaxKind::LAST_STATEMENT as u16)
    {
        if kind == SyntaxKind::VariableStatement {
            let declaration_list = node.declaration_list();
            if get_combined_node_flags(declaration_list).intersects(NodeFlags::BLOCK_SCOPED) {
                return true;
            }
            return declaration_list
                .declarations()
                .nodes()
                .iter()
                .any(|d| d.initializer().is_some());
        }
        return true;
    }
    matches!(
        kind,
        SyntaxKind::ClassDeclaration | SyntaxKind::EnumDeclaration | SyntaxKind::ModuleDeclaration
    )
}
