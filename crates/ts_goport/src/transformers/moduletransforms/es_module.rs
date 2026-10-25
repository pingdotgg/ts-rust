//! Port of Go `transformers/moduletransforms/esmodule.go`.
//!
//! PORT: Go `tx.Visitor()` is the root visitor built by `NewTransformer` with
//! `EmitContext.NewNodeVisitor(tx.visit)`. Here `with_visitor` builds that
//! visitor over the transformer, as the declaration transform does. Its
//! callback `visit_in` gets the running visitor, so the default arm and plain
//! call expressions visit their children on it instead of building a new
//! visitor for each node. The visitor has no state of its own, so this is the
//! same as Go's one shared `tx.Visitor()`.

use super::external_module_info::create_external_helpers_import_declaration_if_needed;
use super::utilities::{
    create_empty_imports, get_external_module_name_literal, is_declaration_file_of,
    is_external_module_file, rewrite_module_specifier,
};
use crate::ast::visitor::NodeVisitor;
use crate::prelude::*;
use crate::transformers::transformer::{
    TransformOptions, TransformReferenceResolver, Transformer, TransformerBox,
};
use crate::transformers::utilities::single_or_many;

// Go: transformers/moduletransforms/esmodule.go:14 ESModuleTransformer
pub struct ESModuleTransformer {
    emit_context: Rc<EmitContext>,
    compiler_options: &'static CompilerOptions,
    #[allow(dead_code)]
    resolver: Rc<dyn TransformReferenceResolver>,
    get_emit_module_format_of_file: Rc<dyn Fn(Node) -> ModuleKind>,
    /// ts#64581: the output extension rewrites of the content mappers.
    content_mapper_extension_rewrites: Vec<crate::frontend::tspath::ExtensionRewrite>,
    /// ts#64581: Go `caseSensitivity`.
    use_case_sensitive_file_names: bool,
    current_source_file: Node,
    import_require_statements: Option<ImportRequireStatements>,
    #[allow(dead_code)]
    helper_name_substitutions: FxHashMap<String, Node>,
}

// Go: transformers/moduletransforms/esmodule.go:26 importRequireStatements
struct ImportRequireStatements {
    statements: Vec<Node>,
    require_helper_name: Node,
}

// Go: transformers/moduletransforms/esmodule.go:31 NewESModuleTransformer
pub fn new_es_module_transformer(opts: &TransformOptions) -> TransformerBox {
    let compiler_options = opts.compiler_options;
    Box::new(ESModuleTransformer {
        emit_context: opts.context.clone(),
        compiler_options,
        resolver: opts.resolver.clone(),
        get_emit_module_format_of_file: opts.get_emit_module_format_of_file.clone(),
        content_mapper_extension_rewrites: opts.content_mapper_extension_rewrites.clone(),
        use_case_sensitive_file_names: opts.use_case_sensitive_file_names,
        current_source_file: Node::NIL,
        import_require_statements: None,
        helper_name_substitutions: FxHashMap::default(),
    })
}

impl Transformer for ESModuleTransformer {
    fn emit_context(&self) -> &Rc<EmitContext> {
        &self.emit_context
    }

    // Go: transformers/transformer.go:39 Transformer.TransformSourceFile
    fn transform_source_file(&mut self, file: Node) -> Node {
        self.with_visitor(|v| v.visit_source_file(file))
    }
}

impl ESModuleTransformer {
    /// Runs `f` with Go `tx.Visitor()`.
    fn with_visitor<R>(
        &mut self,
        f: impl FnOnce(&mut NodeVisitor<'_, &mut ESModuleTransformer>) -> R,
    ) -> R {
        let emit_context = self.emit_context.clone();
        let mut visitor = emit_context.new_node_visitor(visit_in, self);
        f(&mut visitor)
    }

    // Go: transformers/moduletransforms/esmodule.go:58 ESModuleTransformer.visitSourceFile
    fn visit_source_file(&mut self, node: Node) -> Node {
        if is_declaration_file_of(node)
            || !(is_external_module_file(node) || self.compiler_options.get_isolated_modules())
        {
            return node;
        }

        self.current_source_file = node;
        self.import_require_statements = None;

        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut result = self.with_visitor(|v| v.visit_each_child(node));
        ec.add_emit_helper(result, &ec.read_emit_helpers());

        let external_helpers_import_declaration =
            create_external_helpers_import_declaration_if_needed(
                &ec,
                result,
                self.compiler_options,
                (self.get_emit_module_format_of_file)(node),
                false, /*hasExportStarsToExportValues*/
                false, /*hasImportStar*/
                false, /*hasImportDefault*/
            );
        if external_helpers_import_declaration.is_some() || self.import_require_statements.is_some()
        {
            let result_statements = result.statements().to_vec();
            let (prologue, rest) = f.split_standard_prologue(&result_statements);
            let (custom, rest) = f.split_custom_prologue(rest);
            let mut statements: Vec<Node> = prologue.to_vec();
            statements.extend_from_slice(custom);
            if external_helpers_import_declaration.is_some() {
                // The helpers import must be visited so that `import x = require("tslib")`
                // (TypeScript-only syntax) is transformed to `const x = require("tslib")`
                // for CJS output files via visitImportEqualsDeclaration.
                let visited =
                    self.with_visitor(|v| v.visit_node(external_helpers_import_declaration));
                statements.push(visited);
            }
            if let Some(irs) = &self.import_require_statements {
                statements.extend_from_slice(&irs.statements);
            }
            statements.extend_from_slice(rest);
            let statement_list =
                f.new_node_list_with_loc(&statements, result.statement_list().loc());
            result = f.update_source_file(result, statement_list, node.end_of_file_token());
        }

        if is_external_module_file(result)
            && self.compiler_options.get_emit_module_kind() != ModuleKind::PRESERVE
            && !result.statements().iter().any(is_external_module_indicator)
        {
            let mut statements = result.statements().to_vec();
            statements.push(create_empty_imports(f));
            let statement_list =
                f.new_node_list_with_loc(&statements, result.statement_list().loc());
            result = f.update_source_file(result, statement_list, node.end_of_file_token());
        }

        self.import_require_statements = None;
        self.current_source_file = Node::NIL;
        result
    }

    // Go: transformers/moduletransforms/esmodule.go:106 ESModuleTransformer.visitImportDeclaration
    fn visit_import_declaration(&mut self, node: Node) -> Node {
        if !self
            .compiler_options
            .rewrite_relative_import_extensions
            .is_true()
        {
            return node;
        }
        let ec = self.emit_context.clone();
        let updated_module_specifier = rewrite_module_specifier(
            &ec,
            node.module_specifier(),
            self.compiler_options,
            &self.content_mapper_extension_rewrites,
            self.use_case_sensitive_file_names,
        );
        let import_clause = self.with_visitor(|v| v.visit_node(node.import_clause()));
        let attributes = self.with_visitor(|v| v.visit_node(node.attributes()));
        ec.factory().update_import_declaration(
            node,
            ModifierList::NIL, /*modifiers*/
            import_clause,
            updated_module_specifier,
            attributes,
        )
    }

    // Go: transformers/moduletransforms/esmodule.go:120 ESModuleTransformer.visitImportEqualsDeclaration
    fn visit_import_equals_declaration(&mut self, node: Node) -> Node {
        // Though an error in es2020 modules, in node-flavor es2020 modules, we can helpfully transform this to a synthetic `require` call
        // To give easy access to a synchronous `require` in node-flavor esm. We do the transform even in scenarios where we error, but `import.meta.url`
        // is available, just because the output is reasonable for a node-like runtime.
        if self.compiler_options.get_emit_module_kind() < ModuleKind::NODE16 {
            return Node::NIL;
        }

        if !is_external_module_import_equals_declaration(node) {
            panic!(
                "import= for internal module references should be handled in an earlier transformer."
            );
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();
        let require_call = self.create_require_call(node);
        let var_statement = f.new_variable_statement(
            ModifierList::NIL, /*modifiers*/
            f.new_variable_declaration_list(
                f.new_node_list(&[f.new_variable_declaration(
                    f.clone_node(node.name()),
                    Node::NIL, /*exclamationToken*/
                    Node::NIL, /*type*/
                    require_call,
                )]),
                NodeFlags::CONST,
            ),
        );
        ec.set_original(var_statement, node);
        ec.assign_comment_and_source_map_ranges(var_statement, node);

        let mut statements = vec![var_statement];
        statements = self.append_exports_of_import_equals_declaration(statements, node);
        single_or_many(Some(&statements), f)
    }

    // Go: transformers/moduletransforms/esmodule.go:155 ESModuleTransformer.appendExportsOfImportEqualsDeclaration
    fn append_exports_of_import_equals_declaration(
        &mut self,
        mut statements: Vec<Node>,
        node: Node,
    ) -> Vec<Node> {
        if has_syntactic_modifier(node, ModifierFlags::EXPORT) {
            let f = self.emit_context.factory();
            statements.push(f.new_export_declaration(
                ModifierList::NIL, /*modifiers*/
                false,             /*isTypeOnly*/
                f.new_named_exports(f.new_node_list(&[f.new_export_specifier(
                    false,     /*isTypeOnly*/
                    Node::NIL, /*propertyName*/
                    f.clone_node(node.name()),
                )])),
                Node::NIL, /*moduleSpecifier*/
                Node::NIL, /*attributes*/
            ));
        }
        statements
    }

    // Go: transformers/moduletransforms/esmodule.go:176 ESModuleTransformer.visitExportAssignment
    fn visit_export_assignment(&mut self, node: Node) -> Node {
        if !node.is_export_equals() {
            return self.with_visitor(|v| v.visit_each_child(node));
        }
        if self.compiler_options.get_emit_module_kind() != ModuleKind::PRESERVE {
            // Elide `export=` as it is not legal with --module ES6
            return Node::NIL;
        }
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let expression = self.with_visitor(|v| v.visit_node(node.expression()));
        let statement = f.new_expression_statement(f.new_assignment_expression(
            f.new_property_access_expression(
                f.new_identifier("module"),
                Node::NIL, /*questionDotToken*/
                f.new_identifier("exports"),
                NodeFlags::NONE,
            ),
            expression,
        ));
        ec.set_original(statement, node);
        statement
    }

    // Go: transformers/moduletransforms/esmodule.go:199 ESModuleTransformer.visitExportDeclaration
    fn visit_export_declaration(&mut self, node: Node) -> Node {
        if node.module_specifier().is_nil() {
            return node;
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();
        let updated_module_specifier = rewrite_module_specifier(
            &ec,
            node.module_specifier(),
            self.compiler_options,
            &self.content_mapper_extension_rewrites,
            self.use_case_sensitive_file_names,
        );
        if self.compiler_options.module > ModuleKind::ES2015
            || node.export_clause().is_nil()
            || !is_namespace_export(node.export_clause())
        {
            // Either ill-formed or don't need to be transformed.
            let attributes = self.with_visitor(|v| v.visit_node(node.attributes()));
            return f.update_export_declaration(
                node,
                ModifierList::NIL, /*modifiers*/
                false,             /*isTypeOnly*/
                node.export_clause(),
                updated_module_specifier,
                attributes,
            );
        }

        let old_identifier = node.export_clause().name();
        let synth_name = f.new_generated_name_for_node(old_identifier);
        let attributes = self.with_visitor(|v| v.visit_node(node.attributes()));
        let import_decl = f.new_import_declaration(
            ModifierList::NIL, /*modifiers*/
            f.new_import_clause(
                SyntaxKind::Unknown, /*phaseModifier*/
                Node::NIL,           /*name*/
                f.new_namespace_import(synth_name),
            ),
            updated_module_specifier,
            attributes,
        );
        ec.set_original(import_decl, node.export_clause());

        let export_decl = if is_export_namespace_as_default_declaration(node) {
            f.new_export_assignment(
                ModifierList::NIL, /*modifiers*/
                false,             /*isExportEquals*/
                Node::NIL,         /*typeNode*/
                synth_name,
            )
        } else {
            f.new_export_declaration(
                ModifierList::NIL, /*modifiers*/
                false,             /*isTypeOnly*/
                f.new_named_exports(f.new_node_list(&[f.new_export_specifier(
                    false, /*isTypeOnly*/
                    synth_name,
                    old_identifier,
                )])),
                Node::NIL, /*moduleSpecifier*/
                Node::NIL, /*attributes*/
            )
        };
        ec.set_original(export_decl, node);
        single_or_many(Some(&[import_decl, export_decl]), f)
    }

    // Go: transformers/moduletransforms/esmodule.go:261 ESModuleTransformer.visitImportOrRequireCall
    fn visit_import_or_require_call(&mut self, node: Node) -> Node {
        let args = node.arguments().to_vec();
        if args.is_empty() {
            return self.with_visitor(|v| v.visit_each_child(node));
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();
        let expression = self.with_visitor(|v| v.visit_node(node.expression()));

        let argument = if is_string_literal_like(args[0]) {
            rewrite_module_specifier(
                &ec,
                args[0],
                self.compiler_options,
                &self.content_mapper_extension_rewrites,
                self.use_case_sensitive_file_names,
            )
        } else {
            f.new_rewrite_relative_import_extensions_helper(
                args[0],
                self.compiler_options.jsx == JsxEmit::PRESERVE,
                &self.content_mapper_extension_rewrites,
                self.use_case_sensitive_file_names,
            )
        };

        let mut arguments = vec![argument];

        let (rest, _) = self.with_visitor(|v| v.visit_slice(&args[1..]));
        arguments.extend(rest);

        let argument_list = f.new_node_list_with_loc(&arguments, node.argument_list().loc());
        f.update_call_expression(
            node,
            expression,
            node.question_dot_token(),
            NodeList::NIL, /*typeArguments*/
            argument_list,
            node.flags(),
        )
    }

    // Go: transformers/moduletransforms/esmodule.go:293 ESModuleTransformer.createRequireCall
    fn create_require_call(
        &mut self,
        node: Node, /*ImportDeclaration | ImportEqualsDeclaration | ExportDeclaration*/
    ) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let module_name = get_external_module_name_literal(
            f,
            node,
            self.current_source_file,
            None, /*emitResolver*/
            self.compiler_options,
        );

        let mut args: Vec<Node> = Vec::new();
        if module_name.is_some() {
            args.push(rewrite_module_specifier(
                &ec,
                module_name,
                self.compiler_options,
                &self.content_mapper_extension_rewrites,
                self.use_case_sensitive_file_names,
            ));
        }

        if self.compiler_options.get_emit_module_kind() == ModuleKind::PRESERVE {
            return f.new_call_expression(
                f.new_identifier("require"),
                Node::NIL,     /*questionDotToken*/
                NodeList::NIL, /*typeArguments*/
                f.new_node_list(&args),
                NodeFlags::NONE,
            );
        }

        if self.import_require_statements.is_none() {
            let create_require_name = f.new_unique_name_ex(
                "_createRequire",
                AutoGenerateOptions {
                    flags: GeneratedIdentifierFlags::OPTIMISTIC
                        | GeneratedIdentifierFlags::FILE_LEVEL,
                    ..Default::default()
                },
            );
            let import_statement = f.new_import_declaration(
                ModifierList::NIL, /*modifiers*/
                f.new_import_clause(
                    SyntaxKind::Unknown, /*phaseModifier*/
                    Node::NIL,           /*name*/
                    f.new_named_imports(f.new_node_list(&[f.new_import_specifier(
                        false, /*isTypeOnly*/
                        f.new_identifier("createRequire"),
                        create_require_name,
                    )])),
                ),
                f.new_string_literal("module", TokenFlags::NONE),
                Node::NIL, /*attributes*/
            );
            ec.add_emit_flags(import_statement, EmitFlags::CUSTOM_PROLOGUE);

            let require_helper_name = f.new_unique_name_ex(
                "__require",
                AutoGenerateOptions {
                    flags: GeneratedIdentifierFlags::OPTIMISTIC
                        | GeneratedIdentifierFlags::FILE_LEVEL,
                    ..Default::default()
                },
            );
            let require_statement = f.new_variable_statement(
                ModifierList::NIL, /*modifiers*/
                f.new_variable_declaration_list(
                    f.new_node_list(&[f.new_variable_declaration(
                        require_helper_name,
                        Node::NIL, /*exclamationToken*/
                        Node::NIL, /*type*/
                        f.new_call_expression(
                            f.clone_node(create_require_name),
                            Node::NIL,     /*questionDotToken*/
                            NodeList::NIL, /*typeArguments*/
                            f.new_node_list(&[f.new_property_access_expression(
                                f.new_meta_property(
                                    SyntaxKind::ImportKeyword,
                                    f.new_identifier("meta"),
                                ),
                                Node::NIL, /*questionDotToken*/
                                f.new_identifier("url"),
                                NodeFlags::NONE,
                            )]),
                            NodeFlags::NONE,
                        ),
                    )]),
                    NodeFlags::CONST,
                ),
            );
            ec.add_emit_flags(require_statement, EmitFlags::CUSTOM_PROLOGUE);
            self.import_require_statements = Some(ImportRequireStatements {
                statements: vec![import_statement, require_statement],
                require_helper_name,
            });
        }

        let require_helper_name = self
            .import_require_statements
            .as_ref()
            .map(|irs| irs.require_helper_name)
            .unwrap_or(Node::NIL);
        f.new_call_expression(
            f.clone_node(require_helper_name),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            f.new_node_list(&args),
            NodeFlags::NONE,
        )
    }
}

// Go: transformers/moduletransforms/esmodule.go:38 ESModuleTransformer.visit
/// Visits source elements that are not top-level or top-level nested statements.
/// This is the callback of the root visitor `v` (Go `tx.Visitor()`). `v.ctx` is `tx`.
fn visit_in(node: Node, v: &mut NodeVisitor<'_, &mut ESModuleTransformer>) -> Node {
    match node.kind() {
        SyntaxKind::SourceFile => v.ctx.visit_source_file(node),
        SyntaxKind::ImportDeclaration => v.ctx.visit_import_declaration(node),
        SyntaxKind::ImportEqualsDeclaration => v.ctx.visit_import_equals_declaration(node),
        SyntaxKind::ExportAssignment => v.ctx.visit_export_assignment(node),
        SyntaxKind::ExportDeclaration => v.ctx.visit_export_declaration(node),
        SyntaxKind::CallExpression => visit_call_expression_in(node, v),
        _ => v.visit_each_child(node),
    }
}

// Go: transformers/moduletransforms/esmodule.go:251 ESModuleTransformer.visitCallExpression
/// Runs on the running root visitor `v` (`v.ctx` is `tx`), so a plain call
/// visits its children without a new visitor.
fn visit_call_expression_in(node: Node, v: &mut NodeVisitor<'_, &mut ESModuleTransformer>) -> Node {
    if v.ctx
        .compiler_options
        .rewrite_relative_import_extensions
        .is_true()
        && ((is_import_call(node) && !node.arguments().is_empty())
            || (is_in_js_file(node)
                && is_require_call(node, false /*requireStringLiteralLikeArgument*/)))
    {
        return v.ctx.visit_import_or_require_call(node);
    }
    v.visit_each_child(node)
}
