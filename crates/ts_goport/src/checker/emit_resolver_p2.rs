//! Go `checker/emitresolver.go` from line 329 (`isOptionalParameter`) to the end, and the
//! implicit undefined helpers of `checker/emitsupport.go` (ts#64649, `impl Checker` at the
//! end of this file). The `EmitResolver` struct, `EmitResolverLinks` and `newEmitResolver`
//! are in `emit_resolver_p1.rs`.
//!
//! PORT: Go methods read `r.checker`. A Rust resolver cannot hold `&mut Checker`, so:
//! - A Go unexported method takes the checker as an explicit `c: &mut Checker`
//!   parameter (`c` is Go `r.checker`).
//! - A Go exported method has the `crate::printer::EmitResolver` trait signature (the
//!   trait impl in `emit_resolver_p1.rs` calls it). A method that locks
//!   (`r.checkerMu.Lock(); defer r.checkerMu.Unlock()`) gets the checker with
//!   `self.with_checker(|c| ...)`. An exported method that Go does not lock also uses
//!   `with_checker`, because the trait signature has no checker, and has a `_worker`
//!   twin that takes `c` for callers that already hold the checker.
//! - Go exported and unexported methods with the same snake name: the exported one has
//!   the `_exported` suffix (PORTING.md).

use crate::prelude::*;

use crate::binder::reference_resolver::{
    ReferenceResolver, ReferenceResolverHooks, new_reference_resolver,
};
use crate::checker::emit_resolver_p1::{EmitResolver, is_common_js_module_exports};
use crate::printer::{
    SymbolAccessibility, SymbolAccessibilityResult, TypeReferenceSerializationKind,
};

impl EmitResolver {
    // Go: checker/emitresolver.go:329 isOptionalParameter
    pub fn is_optional_parameter(&self, c: &mut Checker, node: Node) -> bool {
        c.is_optional_parameter(node)
    }

    // Go: checker/emitresolver.go:643 IsLiteralConstDeclaration
    pub fn is_literal_const_declaration(&self, node: Node) -> bool {
        // node = r.emitContext.ParseNode(node)
        if !is_parse_tree_node(node) {
            return false;
        }
        if is_declaration_readonly(node) || is_variable_declaration(node) && is_var_const(node) {
            return self.with_checker(|c| {
                let s = c.get_symbol_of_declaration(node);
                if s.is_nil() {
                    return false;
                }
                let t = c.get_type_of_symbol(s);
                c.is_fresh_literal_type(t)
            });
        }
        false
    }

    // Go: checker/emitresolver.go:660 IsExpandoFunctionDeclarationUnsafe
    // PORT: Go takes no lock because its callers already hold it. The trait signature has
    // no checker, so this borrows it. Callers that hold the checker use the `_worker` twin.
    pub fn is_expando_function_declaration_unsafe(&self, node: Node) -> bool {
        self.with_checker(|c| self.is_expando_function_declaration_unsafe_worker(c, node))
    }

    // Go: checker/emitresolver.go:660 IsExpandoFunctionDeclarationUnsafe
    // PORT: body of Go `IsExpandoFunctionDeclarationUnsafe` with the checker passed in.
    pub fn is_expando_function_declaration_unsafe_worker(
        &self,
        c: &mut Checker,
        node: Node,
    ) -> bool {
        // node = r.emitContext.ParseNode(node)
        if !is_parse_tree_node(node) {
            return false;
        }
        // this is substantially different from strada, but so is expando property checking
        let props = self.get_properties_of_container_function_worker(c, node);
        for p in props {
            if is_expando_property_declaration(c.sym(p).value_declaration) {
                return true;
            }
        }
        false
    }

    // Go: checker/emitresolver.go:675 IsExpandoFunctionDeclaration
    pub fn is_expando_function_declaration(&self, node: Node) -> bool {
        self.with_checker(|c| self.is_expando_function_declaration_unsafe_worker(c, node))
    }

    // Go: checker/emitresolver.go:681 isSymbolAccessible
    pub fn is_symbol_accessible(
        &self,
        c: &mut Checker,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        should_compute_alias_to_mark_visible: bool,
    ) -> SymbolAccessibilityResult {
        c.is_symbol_accessible(
            symbol,
            enclosing_declaration,
            meaning,
            should_compute_alias_to_mark_visible,
        )
    }

    // Go: checker/emitresolver.go:685 IsSymbolAccessible
    // PORT: Go takes no lock. The trait signature has no checker, so this borrows it.
    // Callers that hold the checker call `is_symbol_accessible` (or the checker) directly.
    pub fn is_symbol_accessible_exported(
        &self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        should_compute_alias_to_mark_visible: bool,
    ) -> SymbolAccessibilityResult {
        // TODO: Split into locking and non-locking API methods - only current usage is the symbol tracker, which is non-locking,
        // as all tracker calls happen within a CreateX call below, which already holds a lock
        // r.checkerMu.Lock()
        // defer r.checkerMu.Unlock()
        self.with_checker(|c| {
            self.is_symbol_accessible(
                c,
                symbol,
                enclosing_declaration,
                meaning,
                should_compute_alias_to_mark_visible,
            )
        })
    }
}

// Go: checker/emitresolver.go:693 isConstEnumOrConstEnumOnlyModule
// PORT: already ported as `Checker::is_const_enum_or_const_enum_only_module` in extras.rs.

impl EmitResolver {
    // Go: checker/emitresolver.go:697 IsReferencedAliasDeclaration
    pub fn is_referenced_alias_declaration(&self, node: Node) -> bool {
        // PORT: Go reads `r.checker.canCollectSymbolAliasAccessibilityData` before the lock.
        // Reading a checker field needs the checker, so it is read under `with_checker`.
        self.with_checker(|c| {
            if !c.can_collect_symbol_alias_accessibility_data || !is_parse_tree_node(node) {
                return true;
            }

            if is_alias_symbol_declaration(node) {
                let symbol = c.get_symbol_of_declaration(node);
                if symbol.is_some() {
                    let alias_links = c.alias_symbol_links.get(symbol);
                    if alias_links.referenced {
                        return true;
                    }
                    let target = alias_links.alias_target;
                    if target.is_some()
                        && node.modifier_flags().intersects(ModifierFlags::EXPORT)
                        && c.get_symbol_flags(target).intersects(SymbolFlags::VALUE)
                        && (c.compiler_options.should_preserve_const_enums()
                            || !c.is_const_enum_or_const_enum_only_module(target))
                    {
                        return true;
                    }
                }
            }
            false
        })
    }

    // Go: checker/emitresolver.go:723 IsValueAliasDeclaration
    pub fn is_value_alias_declaration(&self, node: Node) -> bool {
        // PORT: see `is_referenced_alias_declaration` for the early field read.
        self.with_checker(|c| {
            if !c.can_collect_symbol_alias_accessibility_data || !is_parse_tree_node(node) {
                return true;
            }

            self.is_value_alias_declaration_worker(c, node)
        })
    }

    // Go: checker/emitresolver.go:735 isValueAliasDeclarationWorker
    pub fn is_value_alias_declaration_worker(&self, c: &mut Checker, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::ImportEqualsDeclaration => {
                let symbol = c.get_symbol_of_declaration(node);
                return self
                    .is_alias_resolved_to_value(c, symbol, false /*excludeTypeOnlyValues*/);
            }
            SyntaxKind::ImportClause
            | SyntaxKind::NamespaceImport
            | SyntaxKind::ImportSpecifier
            | SyntaxKind::ExportSpecifier => {
                let symbol = c.get_symbol_of_declaration(node);
                return symbol.is_some()
                    && self.is_alias_resolved_to_value(
                        c, symbol, true, /*excludeTypeOnlyValues*/
                    );
            }
            SyntaxKind::ExportDeclaration => {
                let export_clause = node.export_clause();
                // PORT: Go passes the func field `r.isValueAliasDeclaration`, which
                // `newEmitResolver` sets to `isValueAliasDeclarationWorker`; call the worker.
                return export_clause.is_some()
                    && (is_namespace_export(export_clause)
                        || export_clause
                            .elements()
                            .iter()
                            .any(|e| self.is_value_alias_declaration_worker(c, e)));
            }
            SyntaxKind::ExportAssignment => {
                if node.expression().is_some() && node.expression().kind() == SyntaxKind::Identifier
                {
                    let symbol = c.get_symbol_of_declaration(node);
                    return self.is_alias_resolved_to_value(
                        c, symbol, true, /*excludeTypeOnlyValues*/
                    );
                }
                return true;
            }
            SyntaxKind::BinaryExpression => {
                if is_common_js_module_exports(node) && is_identifier(node.right()) {
                    let symbol = c.get_symbol_of_declaration(node);
                    return self.is_alias_resolved_to_value(
                        c, symbol, true, /*excludeTypeOnlyValues*/
                    );
                }
            }
            _ => {}
        }
        false
    }

    // Go: checker/emitresolver.go:764 isAliasResolvedToValue
    pub fn is_alias_resolved_to_value(
        &self,
        c: &mut Checker,
        symbol: SymbolId,
        exclude_type_only_values: bool,
    ) -> bool {
        if symbol.is_nil() {
            return false;
        }
        let value_declaration = c.sym(symbol).value_declaration;
        if value_declaration.is_some() {
            let container = get_source_file_of_node(value_declaration);
            if container.is_some() {
                let file_symbol = c.get_symbol_of_declaration(container);
                // Ensures cjs export assignment is setup, since this symbol may point at, and merge with, the file itself.
                // If we don't, the merge may not have yet occurred, and the flags check below will be missing flags that
                // are added as a result of the merge.
                c.resolve_external_module_symbol(file_symbol, false /*dontResolveAlias*/);
            }
        }
        let resolved = c.resolve_alias(symbol);
        let target = c.get_export_symbol_of_value_symbol_if_exported(resolved);
        if target == c.unknown_symbol {
            return !exclude_type_only_values || c.get_type_only_alias_declaration(symbol).is_nil();
        }
        // const enums and modules that contain only const enums are not considered values from the emit perspective
        // unless 'preserveConstEnums' option is set to true
        c.get_symbol_flags_ex(
            symbol,
            exclude_type_only_values,
            true, /*excludeLocalMeanings*/
        )
        .intersects(SymbolFlags::VALUE)
            && (c.compiler_options.should_preserve_const_enums()
                || !c.is_const_enum_or_const_enum_only_module(target))
    }

    // Go: checker/emitresolver.go:789 IsTopLevelValueImportEqualsWithEntityName
    pub fn is_top_level_value_import_equals_with_entity_name(&self, node: Node) -> bool {
        // PORT: see `is_referenced_alias_declaration` for the early field read. The checks
        // before Go's lock read only the AST, so running them under the lock is the same.
        self.with_checker(|c| {
            if !c.can_collect_symbol_alias_accessibility_data {
                return true;
            }
            if !is_parse_tree_node(node)
                || node.kind() != SyntaxKind::ImportEqualsDeclaration
                || node.parent().kind() != SyntaxKind::SourceFile
            {
                return false;
            }
            if is_import_equals_declaration(node)
                && (node_is_missing(node.module_reference())
                    || node.module_reference().kind() == SyntaxKind::ExternalModuleReference)
            {
                return false;
            }

            let symbol = c.get_symbol_of_declaration(node);
            self.is_alias_resolved_to_value(c, symbol, false /*excludeTypeOnlyValues*/)
        })
    }

    // Go: checker/emitresolver.go:808 MarkLinkedReferencesRecursively
    pub fn mark_linked_references_recursively(&self, file: Node) {
        if !is_parse_tree_node(file) {
            return;
        }
        self.with_checker(|c| {
            if file.is_some() {
                file.for_each_child(|n: Node| mark_linked_references_recursively_visit(c, n));
            }
        });
    }

    // Go: checker/emitresolver.go:832 GetExternalModuleFileFromDeclaration
    pub fn get_external_module_file_from_declaration(&self, declaration: Node) -> Node {
        if !is_parse_tree_node(declaration) {
            return Node::NIL;
        }

        self.with_checker(|c| c.get_external_module_file_from_declaration(declaration))
    }

    // Go: checker/emitresolver.go:842 getReferenceResolver
    // PORT: Go caches the resolver in `r.referenceResolver`. The resolver only
    // holds the options, the hooks and a lazy name resolver that is used only
    // when `ResolveName` is nil. All hooks are set here, so a new resolver per
    // call behaves the same, and `EmitResolver` needs no cache field.
    pub fn get_reference_resolver(&self, c: &Checker) -> ReferenceResolver {
        new_reference_resolver(
            c.compiler_options,
            ReferenceResolverHooks {
                resolve_name: Some(
                    |c,
                     location,
                     name,
                     meaning,
                     name_not_found_message,
                     is_use,
                     exclude_globals| {
                        c.resolve_name(
                            location,
                            name,
                            meaning,
                            name_not_found_message,
                            is_use,
                            exclude_globals,
                        )
                    },
                ),
                get_resolved_symbol: Some(|c, node| c.get_resolved_symbol_or_nil(node)),
                get_merged_symbol: Some(|c, symbol| c.get_merged_symbol(symbol)),
                get_parent_of_symbol: Some(|c, symbol| c.get_parent_of_symbol(symbol)),
                get_symbol_of_declaration: Some(|c, declaration| {
                    c.get_symbol_of_declaration(declaration)
                }),
                get_type_only_alias_declaration: Some(|c, symbol, include| {
                    c.get_type_only_alias_declaration_ex(symbol, include)
                }),
                get_export_symbol_of_value_symbol_if_exported: Some(|c, symbol| {
                    c.get_export_symbol_of_value_symbol_if_exported(symbol)
                }),
                get_element_access_expression_name: Some(|c, expression| {
                    let (name, ok) = c.try_get_element_access_expression_name(expression);
                    (name.into_owned(), ok)
                }),
            },
        )
    }

    // Go: checker/emitresolver.go:858 GetReferencedExportContainer
    pub fn get_referenced_export_container(&self, node: Node, prefix_locals: bool) -> Node /*SourceFile|ModuleDeclaration|EnumDeclaration*/
    {
        if !is_parse_tree_node(node) {
            return Node::NIL;
        }

        self.with_checker(|c| {
            self.get_reference_resolver(c)
                .get_referenced_export_container(c, node, prefix_locals)
        })
    }

    // Go: checker/emitresolver.go:559 SetReferencedImportDeclaration
    pub fn set_referenced_import_declaration(&self, node: Node, ref_: Node) {
        self.with_checker(|c| {
            c.emit_resolver_links.jsx_links.get(node).import_ref = ref_;
        });
    }

    // Go: checker/emitresolver.go:565 GetReferencedImportDeclaration
    pub fn get_referenced_import_declaration(&self, node: Node) -> Node {
        self.with_checker(|c| {
            if !is_parse_tree_node(node) {
                return c.emit_resolver_links.jsx_links.get(node).import_ref;
            }

            let symbol = c.get_referenced_value_or_alias_symbol(node);
            if is_non_local_alias(&c.symbols, symbol, SymbolFlags::VALUE)
                && c.get_type_only_alias_declaration_ex(symbol, SymbolFlags::VALUE)
                    .is_nil()
            {
                return c.get_declaration_of_alias_symbol(symbol);
            }
            Node::NIL
        })
    }

    // Go: checker/emitresolver.go:889 GetReferencedValueDeclaration
    pub fn get_referenced_value_declaration(&self, node: Node) -> Node {
        if !is_parse_tree_node(node) {
            return Node::NIL;
        }

        self.with_checker(|c| {
            self.get_reference_resolver(c)
                .get_referenced_value_declaration(c, node)
        })
    }

    // Go: checker/emitresolver.go:900 GetReferencedValueDeclarationUnsafe
    // PORT: Go takes no lock because its callers already hold it. The trait signature has
    // no checker, so this borrows it. Callers that hold the checker use the `_worker` twin.
    pub fn get_referenced_value_declaration_unsafe(&self, node: Node) -> Node {
        self.with_checker(|c| self.get_referenced_value_declaration_unsafe_worker(c, node))
    }

    // Go: checker/emitresolver.go:900 GetReferencedValueDeclarationUnsafe
    // PORT: body of Go `GetReferencedValueDeclarationUnsafe` with the checker passed in.
    pub fn get_referenced_value_declaration_unsafe_worker(
        &self,
        c: &mut Checker,
        node: Node,
    ) -> Node {
        self.get_reference_resolver(c)
            .get_referenced_value_declaration(c, node)
    }

    // Go: checker/emitresolver.go:904 GetReferencedValueDeclarations
    pub fn get_referenced_value_declarations(&self, node: Node) -> Vec<Node> {
        if !is_parse_tree_node(node) {
            return Vec::new();
        }

        self.with_checker(|c| {
            self.get_reference_resolver(c)
                .get_referenced_value_declarations(c, node)
        })
    }

    // Go: checker/emitresolver.go:916 IsNameResolvable
    // IsNameResolvable returns `true` if the given `name` resolves to any symbol at `location`
    pub fn is_name_resolvable(&self, location: Node, name: &str) -> bool {
        self.with_checker(|c| {
            let symbol = c.resolve_name(
                location,
                name,
                SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
                None,  /*nameNotFoundMessage*/
                false, /*isUse*/
                false, /*excludeGlobals*/
            );
            symbol.is_some()
        })
    }

    // Go: checker/emitresolver.go:924 GetElementAccessExpressionName
    pub fn get_element_access_expression_name(&self, expression: Node) -> String {
        if !is_parse_tree_node(expression) {
            return String::new();
        }

        self.with_checker(|c| {
            self.get_reference_resolver(c)
                .get_element_access_expression_name(c, expression)
        })
    }

    // Go: checker/emitresolver.go:935 GetReferencedMemberValueDeclaration
    pub fn get_referenced_member_value_declaration(&self, node: Node) -> Node {
        if !is_parse_tree_node(node) {
            return Node::NIL;
        }

        self.with_checker(|c| {
            self.get_reference_resolver(c)
                .get_referenced_member_value_declaration(c, node)
        })
    }

    // TODO: the emit resolver being responsible for some amount of node construction is a very leaky abstraction,
    // and requires giving it access to a lot of context it's otherwise not required to have, which also further complicates the API
    // and likely reduces performance. There's probably some refactoring that could be done here to simplify this.

    // Go: checker/emitresolver.go:950 CreateReturnTypeOfSignatureDeclaration
    pub fn create_return_type_of_signature_declaration(
        &self,
        emit_context: &EmitContext,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        let original = emit_context.parse_node(signature_declaration);
        if original.is_nil() {
            return emit_context
                .factory
                .new_keyword_type_node(SyntaxKind::AnyKeyword);
        }

        self.with_checker(|c| {
            let request_node_builder = self.node_builder(c, emit_context);
            c.node_builder_serialize_return_type_for_signature(
                &request_node_builder,
                original,
                enclosing_declaration,
                flags,
                internal_flags,
                tracker,
            )
        })
    }

    // Go: checker/emitresolver.go:962 CreateTypeParametersOfSignatureDeclaration
    pub fn create_type_parameters_of_signature_declaration(
        &self,
        emit_context: &EmitContext,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Vec<Node> {
        let original = emit_context.parse_node(signature_declaration);
        if original.is_nil() {
            return Vec::new();
        }

        self.with_checker(|c| {
            let request_node_builder = self.node_builder(c, emit_context);
            c.node_builder_serialize_type_parameters_for_signature(
                &request_node_builder,
                original,
                enclosing_declaration,
                flags,
                internal_flags,
                tracker,
            )
        })
    }

    // Go: checker/emitresolver.go:974 CreateTypeOfDeclaration
    pub fn create_type_of_declaration(
        &self,
        emit_context: &EmitContext,
        declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        let original = emit_context.parse_node(declaration);
        if original.is_nil() {
            return emit_context
                .factory
                .new_keyword_type_node(SyntaxKind::AnyKeyword);
        }

        self.with_checker(|c| {
            let request_node_builder = self.node_builder(c, emit_context);
            // // Get type of the symbol if this is the valid symbol otherwise get type at location
            let symbol = c.get_symbol_of_declaration(declaration);
            c.node_builder_serialize_type_for_declaration(
                &request_node_builder,
                declaration,
                symbol,
                enclosing_declaration,
                flags | NodeBuilderFlags::MULTILINE_OBJECT_LITERALS,
                internal_flags,
                tracker,
            )
        })
    }

    // Go: checker/emitresolver.go:988 CreateLiteralConstValue
    pub fn create_literal_const_value(
        &self,
        emit_context: &EmitContext,
        node: Node,
        tracker: EmitSymbolTracker,
    ) -> Node {
        let node = emit_context.parse_node(node);
        let t = self.with_checker(|c| {
            let symbol = c.get_symbol_of_declaration(node);
            c.get_type_of_symbol(symbol)
        });
        if t.is_nil() {
            return Node::NIL; // TODO: How!? Maybe this should be a panic. All symbols should have a type.
        }

        let mut enum_result = Node::NIL;
        let (t_flags, t_symbol, true_type, false_type) =
            self.with_checker(|c| (c.ty(t).flags, c.ty(t).symbol, c.true_type, c.false_type));
        if t_flags.intersects(TypeFlags::ENUM_LIKE) {
            enum_result = self.with_checker(|c| {
                let request_node_builder = self.node_builder(c, emit_context);
                c.node_builder_symbol_to_expression(
                    &request_node_builder,
                    t_symbol,
                    SymbolFlags::VALUE,
                    node,
                    NodeBuilderFlags::NONE,
                    InternalNodeBuilderFlags::NONE,
                    tracker,
                )
            });
            // What about regularTrueType/regularFalseType - since those aren't fresh, we never make initializers from them
            // TODO: handle those if this function is ever used for more than initializers in declaration emit
        } else if t == true_type {
            enum_result = emit_context
                .factory
                .new_keyword_expression(SyntaxKind::TrueKeyword);
        } else if t == false_type {
            enum_result = emit_context
                .factory
                .new_keyword_expression(SyntaxKind::FalseKeyword);
        }
        if enum_result.is_some() {
            return enum_result;
        }
        if !t_flags.intersects(TypeFlags::LITERAL) {
            return Node::NIL; // non-literal type
        }
        let value = self.with_checker(|c| c.ty(t).as_literal_type().value.clone());
        let factory = &emit_context.factory;
        match value {
            Some(LiteralValue::String(value)) => {
                factory.new_string_literal(value, TokenFlags::NONE)
            }
            Some(LiteralValue::Number(value)) => {
                if value.is_infinite() {
                    if value > crate::jsnum::Number(0.0) {
                        return factory.new_identifier("Infinity");
                    }
                    return factory.new_prefix_unary_expression(
                        SyntaxKind::MinusToken,
                        factory.new_identifier("Infinity"),
                    );
                }
                if value.is_nan() {
                    return factory.new_identifier("NaN");
                }
                if value.abs() != value {
                    // negative
                    return factory.new_prefix_unary_expression(
                        SyntaxKind::MinusToken,
                        factory.new_numeric_literal(
                            value.to_string()[1..].to_string(),
                            TokenFlags::NONE,
                        ),
                    );
                }
                factory.new_numeric_literal(value.to_string(), TokenFlags::NONE)
            }
            Some(LiteralValue::PseudoBigInt(value)) => factory
                .new_big_int_literal(pseudo_big_int_to_string(&value) + "n", TokenFlags::NONE),
            Some(LiteralValue::Bool(value)) => {
                let mut kind = SyntaxKind::FalseKeyword;
                if value {
                    kind = SyntaxKind::TrueKeyword;
                }
                factory.new_keyword_expression(kind)
            }
            None => panic!("unhandled literal const value kind"),
        }
    }

    // Go: checker/emitresolver.go:1049 CreateTypeOfExpression
    pub fn create_type_of_expression(
        &self,
        emit_context: &EmitContext,
        expression: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        let expression = emit_context.parse_node(expression);
        if expression.is_nil() {
            return emit_context
                .factory
                .new_keyword_type_node(SyntaxKind::AnyKeyword);
        }

        self.with_checker(|c| {
            let request_node_builder = self.node_builder(c, emit_context);
            c.node_builder_serialize_type_for_expression(
                &request_node_builder,
                expression,
                enclosing_declaration,
                flags | NodeBuilderFlags::MULTILINE_OBJECT_LITERALS,
                internal_flags,
                tracker,
            )
        })
    }

    // Go: checker/emitresolver.go:1061 CreateLateBoundIndexSignatures
    pub fn create_late_bound_index_signatures(
        &self,
        emit_context: &EmitContext,
        container: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Vec<Node> {
        let container = emit_context.parse_node(container);
        self.with_checker(|c| {
            let sym = container.symbol();
            let sym_type = c.get_type_of_symbol(sym);
            let static_infos = c.get_index_infos_of_type(sym_type).to_vec();
            let instance_index_symbol = c.get_index_symbol(sym);
            let mut instance_infos: Vec<IndexInfoId> = Vec::new();
            if instance_index_symbol.is_some() {
                // PORT: Go collects `maps.Values` of a Go map, whose order is random. This
                // uses the symbol table's own order.
                let members = c.get_members_of_symbol(sym);
                let sibling_symbols = c.symbols.values(members);
                instance_infos =
                    c.get_index_infos_of_index_symbol(instance_index_symbol, &sibling_symbols);
            }

            let request_node_builder = self.node_builder(c, emit_context);
            let factory = &emit_context.factory;

            let mut result: Vec<Node> = Vec::new();
            for (i, info_list) in [static_infos, instance_infos].into_iter().enumerate() {
                let mut is_static = true;
                if i > 0 {
                    is_static = false;
                }
                if info_list.is_empty() {
                    continue;
                }
                for info in info_list {
                    if c.index_info(info).declaration.is_some() {
                        continue;
                    }
                    if info == c.any_base_type_index_info {
                        continue; // inherited, but looks like a late-bound signature because it has no declarations
                    }
                    let components = c.index_info(info).components.clone();
                    if !components.is_empty() {
                        // !!! TODO: Complete late-bound index info support - getObjectLiteralIndexInfo does not yet add late bound components to index signatures
                        let all_component_computed_names_serializable = enclosing_declaration
                            .is_some()
                            && components.iter().all(|&comp| {
                                comp.name().is_some()
                                    && is_computed_property_name(comp.name())
                                    && is_entity_name_expression(comp.name().expression())
                                    && c.is_entity_name_visible(
                                        comp.name().expression(),
                                        enclosing_declaration,
                                        false,
                                    )
                                    .accessibility
                                        == SymbolAccessibility::ACCESSIBLE
                            });
                        if all_component_computed_names_serializable {
                            for &comp in &components {
                                if c.has_late_bindable_name(comp) {
                                    // skip late bound props that contribute to the index signature - they'll be preserved via other means
                                    continue;
                                }

                                let first_identifier =
                                    get_first_identifier(comp.name().expression());
                                let name = c.resolve_name(
                                    first_identifier,
                                    first_identifier.text(),
                                    SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE,
                                    None,  /*nameNotFoundMessage*/
                                    true,  /*isUse*/
                                    false, /*excludeGlobals*/
                                );
                                if name.is_some() {
                                    // PORT: Go calls the interface directly, so a nil tracker panics.
                                    tracker.as_ref().expect("nil SymbolTracker").track_symbol(
                                        c,
                                        name,
                                        enclosing_declaration,
                                        SymbolFlags::VALUE,
                                    );
                                }

                                let mut mods: Option<Vec<Node>> = if is_static {
                                    Some(vec![factory.new_modifier(SyntaxKind::StaticKeyword)])
                                } else {
                                    None
                                };
                                if c.index_info(info).is_readonly {
                                    mods.get_or_insert_with(Vec::new)
                                        .push(factory.new_modifier(SyntaxKind::ReadonlyKeyword));
                                }

                                let comp_symbol = comp.symbol();
                                let comp_type = c.get_type_of_symbol(comp_symbol);
                                let type_node = c.node_builder_type_to_type_node(
                                    &request_node_builder,
                                    comp_type,
                                    enclosing_declaration,
                                    flags,
                                    internal_flags,
                                    tracker.clone(),
                                );
                                let decl = factory.new_property_declaration(
                                    match &mods {
                                        Some(mods) => factory.new_modifier_list(mods),
                                        None => ModifierList::NIL,
                                    },
                                    comp.name(),
                                    comp.question_token(),
                                    type_node,
                                    Node::NIL,
                                );
                                result.push(decl);
                            }
                            continue;
                        }
                    }
                    let mut node = c.node_builder_index_info_to_index_signature_declaration(
                        &request_node_builder,
                        info,
                        enclosing_declaration,
                        flags,
                        internal_flags,
                        tracker.clone(),
                    );
                    if node.is_some() && is_static {
                        let mut mod_nodes = vec![factory.new_modifier(SyntaxKind::StaticKeyword)];
                        mod_nodes.extend(node.modifier_nodes().iter());
                        let mods = factory.new_modifier_list(&mod_nodes);
                        node = factory.update_index_signature_declaration(
                            node,
                            mods,
                            node.parameter_list(),
                            node.type_(),
                        );
                    }
                    if node.is_some() {
                        result.push(node);
                    }
                }
            }
            result
        })
    }

    // Go: checker/emitresolver.go:1151 GetEffectiveDeclarationFlags
    pub fn get_effective_declaration_flags(
        &self,
        node: Node,
        flags: ModifierFlags,
    ) -> ModifierFlags {
        // node = emitContext.ParseNode(node)
        self.with_checker(|c| c.get_effective_declaration_flags(node, flags))
    }

    // Go: checker/emitresolver.go:1158 GetConstantValue
    pub fn get_constant_value(&self, node: Node) -> Option<LiteralValue> {
        // node = emitContext.ParseNode(node)
        self.with_checker(|c| c.get_constant_value(node))
    }

    // Go: checker/emitresolver.go:1165 GetTypeReferenceSerializationKind
    pub fn get_type_reference_serialization_kind(
        &self,
        type_name: Node,
        location: Node,
    ) -> TypeReferenceSerializationKind {
        // typeName = emitContext.ParseNode(typeName)
        // location = emitContext.ParseNode(location)
        self.with_checker(|c| {
            if type_name.is_nil() || location.is_nil() {
                return TypeReferenceSerializationKind::UNKNOWN;
            }

            // Resolve the symbol as a value to ensure the type can be reached at runtime during emit.
            let mut is_type_only = false;
            if is_qualified_name(type_name) {
                let root_value_symbol = c.resolve_entity_name(
                    get_first_identifier(type_name),
                    SymbolFlags::VALUE,
                    true,
                    true,
                    location,
                );

                if root_value_symbol.is_some() && !c.sym(root_value_symbol).declarations.is_empty()
                {
                    is_type_only = c
                        .sym(root_value_symbol)
                        .declarations
                        .iter()
                        .all(|&d| is_type_only_import_or_export_declaration(d));
                }
            }
            let value_symbol =
                c.resolve_entity_name(type_name, SymbolFlags::VALUE, true, true, location);
            let mut resolved_value_symbol = value_symbol;
            if value_symbol.is_some() && c.sym(value_symbol).flags.intersects(SymbolFlags::ALIAS) {
                resolved_value_symbol = c.resolve_alias(value_symbol);
            }

            is_type_only = is_type_only
                || (value_symbol.is_some()
                    && c.get_type_only_alias_declaration_ex(value_symbol, SymbolFlags::VALUE)
                        .is_some());

            // Resolve the symbol as a type so that we can provide a more useful hint for the type serializer.
            let type_symbol =
                c.resolve_entity_name(type_name, SymbolFlags::TYPE, true, true, location);
            let mut resolved_type_symbol = type_symbol;
            if type_symbol.is_some() && c.sym(type_symbol).flags.intersects(SymbolFlags::ALIAS) {
                resolved_type_symbol = c.resolve_alias(type_symbol);
            }
            // In case the value symbol can't be resolved (e.g. because of missing declarations), use type symbol for reachability check.
            is_type_only = is_type_only
                || (type_symbol.is_some()
                    && c.get_type_only_alias_declaration_ex(type_symbol, SymbolFlags::TYPE)
                        .is_some());

            if resolved_value_symbol.is_some() && resolved_value_symbol == resolved_type_symbol {
                let global_promise_symbol = c.get_global_promise_constructor_symbol();
                if global_promise_symbol.is_some() && resolved_value_symbol == global_promise_symbol
                {
                    return TypeReferenceSerializationKind::PROMISE;
                }

                let constructor_type = c.get_type_of_symbol(resolved_value_symbol);
                if constructor_type.is_some() && c.is_constructor_type(constructor_type) {
                    if is_type_only {
                        return TypeReferenceSerializationKind::TYPE_WITH_CALL_SIGNATURE;
                    }
                    return TypeReferenceSerializationKind::TYPE_WITH_CONSTRUCT_SIGNATURE_AND_VALUE;
                }
            }

            // We might not be able to resolve type symbol so use unknown type in that case (eg error case)
            if resolved_type_symbol.is_nil() {
                if is_type_only {
                    return TypeReferenceSerializationKind::OBJECT_TYPE;
                }
                return TypeReferenceSerializationKind::UNKNOWN;
            }

            let type_ = c.get_declared_type_of_symbol(resolved_type_symbol);
            if c.is_error_type(type_) {
                if is_type_only {
                    return TypeReferenceSerializationKind::OBJECT_TYPE;
                }
                return TypeReferenceSerializationKind::UNKNOWN;
            }

            if c.ty(type_).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                TypeReferenceSerializationKind::OBJECT_TYPE
            } else if c.is_type_assignable_to_kind(
                type_,
                TypeFlags::VOID | TypeFlags::NULLABLE | TypeFlags::NEVER,
            ) {
                TypeReferenceSerializationKind::VOID_NULLABLE_OR_NEVER_TYPE
            } else if c.is_type_assignable_to_kind(type_, TypeFlags::BOOLEAN_LIKE) {
                TypeReferenceSerializationKind::BOOLEAN_TYPE
            } else if c.is_type_assignable_to_kind(type_, TypeFlags::NUMBER_LIKE) {
                TypeReferenceSerializationKind::NUMBER_LIKE_TYPE
            } else if c.is_type_assignable_to_kind(type_, TypeFlags::BIG_INT_LIKE) {
                TypeReferenceSerializationKind::BIG_INT_LIKE_TYPE
            } else if c.is_type_assignable_to_kind(type_, TypeFlags::STRING_LIKE) {
                TypeReferenceSerializationKind::STRING_LIKE_TYPE
            } else if c.is_tuple_type(type_) {
                TypeReferenceSerializationKind::ARRAY_LIKE_TYPE
            } else if c.is_type_assignable_to_kind(type_, TypeFlags::ES_SYMBOL_LIKE) {
                TypeReferenceSerializationKind::ES_SYMBOL_TYPE
            } else if c.is_function_type(type_) {
                TypeReferenceSerializationKind::TYPE_WITH_CALL_SIGNATURE
            } else if c.is_array_type(type_) {
                TypeReferenceSerializationKind::ARRAY_LIKE_TYPE
            } else {
                TypeReferenceSerializationKind::OBJECT_TYPE
            }
        })
    }

    // Go: checker/emitresolver.go:1257 GetPropertiesOfContainerFunction
    // PORT: see `is_expando_function_declaration_unsafe`.
    pub fn get_properties_of_container_function(&self, node: Node) -> Vec<SymbolId> {
        self.with_checker(|c| self.get_properties_of_container_function_worker(c, node))
    }

    // Go: checker/emitresolver.go:1257 GetPropertiesOfContainerFunction
    // PORT: body of Go `GetPropertiesOfContainerFunction` with the checker passed in.
    pub fn get_properties_of_container_function_worker(
        &self,
        c: &mut Checker,
        node: Node,
    ) -> Vec<SymbolId> {
        // This is explicitly _not locked_ because it is only called via error reporters invoked via node builder calls
        // to the symbol tracker already within locked contexts.
        // r.checkerMu.Lock()
        // defer r.checkerMu.Unlock()
        if node.is_nil() {
            return Vec::new();
        }
        let s = c.get_symbol_of_declaration(node);
        if s.is_nil() {
            return Vec::new();
        }
        let t = c.get_type_of_symbol(s);
        c.get_properties_of_type(t).to_vec()
    }

    // Go: checker/emitresolver.go:1272 TryJSTypeNodeToTypeNode
    pub fn try_js_type_node_to_type_node(
        &self,
        emit_context: &EmitContext,
        type_node: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        let type_node = emit_context.parse_node(type_node);
        self.with_checker(|c| {
            let request_node_builder = self.node_builder(c, emit_context);
            c.node_builder_try_js_type_node_to_type_node(
                &request_node_builder,
                type_node,
                enclosing_declaration,
                flags,
                internal_flags,
                tracker,
            )
        })
    }

    // Go: checker/emitresolver.go:1292 IsThisPropertyAssignmentDeclarationRedundant
    /// IsThisPropertyAssignmentDeclarationRedundant reports whether a JS `this.<name> = ...` expando
    /// assignment should be omitted from declaration emit because the member it would synthesize is
    /// already provided by an `extends` base type. This mirrors the skip condition in the checker's
    /// serializePropertySymbol: an inherited member is redundant when it is identical to the assigned
    /// one (same readonly-ness, optionality and type). Inherited accessors and methods are always
    /// treated as redundant here, since accessors merge oddly with value assignments (and run via the
    /// accessor at runtime), and `this`-expando props carry the ReplaceableByMethod contract, so a
    /// rebind such as `this.method = this.method.bind(this)` must not override the base method.
    ///
    /// Only `extends` base types are considered. Members coming from `implements` clauses are not
    /// inherited, so the class must redeclare them, and they are always emitted.
    pub fn is_this_property_assignment_declaration_redundant(&self, node: Node) -> bool {
        if node.is_nil() {
            return false;
        }

        self.with_checker(|c| {
            let s = c.get_symbol_of_declaration(node);
            if s.is_nil() || c.sym(s).parent.is_nil() {
                return false;
            }
            let parent = c.sym(s).parent;
            let parent_type = c.get_declared_type_of_symbol(parent);
            if parent_type.is_nil() {
                return false;
            }
            let name = c.sym(s).name.clone();
            for base in c.get_base_types(parent_type) {
                let base_prop = c.get_property_of_type(base, &name);
                if base_prop.is_nil() {
                    continue;
                }
                if c.sym(base_prop)
                    .flags
                    .intersects(SymbolFlags::ACCESSOR | SymbolFlags::METHOD | SymbolFlags::FUNCTION)
                {
                    return true;
                }
                if c.is_readonly_symbol(base_prop) == c.is_readonly_symbol(s)
                    && (c.sym(s).flags & SymbolFlags::OPTIONAL)
                        == (c.sym(base_prop).flags & SymbolFlags::OPTIONAL)
                {
                    let s_type = c.get_type_of_symbol(s);
                    let base_type = c.get_type_of_symbol(base_prop);
                    if c.is_type_identical_to(s_type, base_type) {
                        return true;
                    }
                }
            }
            false
        })
    }
}

// Go: checker/emitresolver.go:804 MarkLinkedReferencesRecursively.func1 (visit)
// PORT: the Go closure is recursive (it passes itself to `ForEachChild`), so it is a named fn.
fn mark_linked_references_recursively_visit(c: &mut Checker, n: Node) -> bool {
    if is_import_equals_declaration(n) && !n.modifier_flags().intersects(ModifierFlags::EXPORT) {
        return false; // These are deferred and marked in a chain when referenced
    }
    if is_import_declaration(n) {
        return false; // likewise, these are ultimately what get marked by calls on other nodes - we want to skip them
    }
    c.mark_linked_references(
        n,
        ReferenceHint::UNSPECIFIED,
        SymbolId::NIL, /*propSymbol*/
        TypeId::NIL,   /*parentType*/
    );
    n.for_each_child(|child: Node| mark_linked_references_recursively_visit(c, child));
    false
}

impl Checker {
    // Go: checker/services.go:870 GetConstantValue
    // PORT: Go `any` result is `Option<LiteralValue>` (`None` is Go nil). The
    // Go function lives in services.go; it is here because the emit resolver
    // is its only non-language-service caller in this crate.
    pub fn get_constant_value(&mut self, node: Node) -> Option<LiteralValue> {
        if node.kind() == SyntaxKind::EnumMember {
            return self.get_enum_member_value(node).value;
        }

        if self.symbol_node_links.get(node).resolved_symbol.is_nil() {
            self.check_expression_cached(node); // ensure cached resolved symbol is set
        }
        let mut symbol = self.symbol_node_links.get(node).resolved_symbol;
        if symbol.is_nil() && is_entity_name_expression(node) {
            symbol = self.resolve_entity_name(
                node,
                SymbolFlags::VALUE,
                true,      /*ignoreErrors*/
                false,     /*dontResolveAlias*/
                Node::NIL, /*location*/
            );
        }
        if symbol.is_some() && self.sym(symbol).flags.intersects(SymbolFlags::ENUM_MEMBER) {
            // inline property\index accesses only for const enums
            let member = self.sym(symbol).value_declaration;
            if is_enum_const(member.parent()) {
                return self.get_enum_member_value(member).value;
            }
        }

        None
    }
}

// Go: checker/emitsupport.go (ts#64649): the implicit undefined helpers that
// ts#64649 moved from the emit resolver to the checker.
impl Checker {
    // Go: checker/emitsupport.go:304 requiresAddingImplicitUndefinedWorker (ts#64649 moved it from the emit resolver)
    pub fn requires_adding_implicit_undefined_worker(
        &mut self,
        parameter: Node,
        enclosing_declaration: Node,
    ) -> bool {
        (self.is_required_initialized_parameter(parameter, enclosing_declaration)
            || self.is_optional_uninitialized_parameter_property(parameter))
            && !self.declared_parameter_type_contains_undefined(parameter)
    }

    // Go: checker/emitsupport.go:308 declaredParameterTypeContainsUndefined (ts#64649 moved it from the emit resolver)
    pub fn declared_parameter_type_contains_undefined(&mut self, parameter: Node) -> bool {
        // typeNode := getNonlocalEffectiveTypeAnnotationNode(parameter); // !!! JSDoc Support
        let type_node = parameter.type_();
        if type_node.is_nil() {
            return false;
        }
        let t = self.get_type_from_type_node(type_node);
        // allow error type here to avoid confusing errors that the annotation has to contain undefined when it does in cases like this:
        //
        // export function fn(x?: Unresolved | undefined): void {}
        self.is_error_type(t) || self.contains_undefined_type(t)
    }

    // Go: checker/emitsupport.go:321 isOptionalUninitializedParameterProperty (ts#64649 moved it from the emit resolver)
    pub fn is_optional_uninitialized_parameter_property(&mut self, parameter: Node) -> bool {
        self.strict_null_checks
            && self.is_optional_parameter(parameter)
            && ( /*isJSDocParameterTag(parameter) ||*/parameter.initializer().is_nil()) // !!! TODO: JSDoc support
            && has_syntactic_modifier(parameter, ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
    }

    // Go: checker/emitsupport.go:328 isRequiredInitializedParameter (ts#64649 moved it from the emit resolver)
    pub fn is_required_initialized_parameter(
        &mut self,
        parameter: Node,
        enclosing_declaration: Node,
    ) -> bool {
        if !self.strict_null_checks || self.is_optional_parameter(parameter) || /*isJSDocParameterTag(parameter) ||*/ parameter.initializer().is_nil()
        {
            // !!! TODO: JSDoc Support
            return false;
        }
        if has_syntactic_modifier(parameter, ModifierFlags::PARAMETER_PROPERTY_MODIFIER) {
            return enclosing_declaration.is_some()
                && is_function_like_declaration(enclosing_declaration);
        }
        true
    }
}
