//! Port of `transformers/declarations/transform.go` lines 1 to 752: the
//! transformer state, its constructor, `GetDiagnostics`, the `@internal`
//! checks, the root `visit` dispatch, `transformSourceFile` with its helpers,
//! `setupDiagnosticContext`, `visitDeclarationSubtree`, `checkName`,
//! `transformMappedTypeNode` and `transformHeritageClause`.
//!
//! PORT: Go `tx.Visitor()` is the root visitor. Its `Visit` field is
//! `tx.visit`, so Go `tx.Visitor().Visit(n)` is `self.visit(n)` here. The
//! other root visitor methods go through `with_visitor` (transform_p2.rs).
//! The other Go visitor fields (`cjsExportAssignmentVisitor`,
//! `expressionVisitor`, ...) are not fields; see `transform_source_file`.

use super::DeclarationEmitHost;
use super::diagnostics::{
    GetSymbolAccessibilityDiagnostic, SymbolAccessibilityDiagnostic, bound_symbol_declarations,
    create_diagnostic_for_node, create_get_symbol_accessibility_diagnostic_for_node,
    create_get_symbol_accessibility_diagnostic_for_node_name,
};
use super::tracker::{SymbolTrackerImpl, SymbolTrackerSharedState, new_symbol_tracker};
use super::util::{
    can_produce_diagnostics_kind, is_declaration_and_not_visible, is_enclosing_declaration_kind,
    needs_scope_marker,
};
use crate::ast::visitor::syntax_list_children;
use crate::checker::nodebuilder_types::{InternalNodeBuilderFlags, NodeBuilderFlags};
use crate::frontend::tspath;
use crate::prelude::*;
use crate::printer::{CommentRange, EmitContext, EmitResolver, SymbolAccessibilityResult};
use crate::transformers::utilities::is_original_node_single_line;

// Go: transformers/declarations/transform.go:23 ReferencedFilePair
#[derive(Clone)]
pub struct ReferencedFilePair {
    pub file: Node,
    pub r#ref: FileReference,
}

// Go: transformers/declarations/transform.go:46 thisPropertyAssignmentKey
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct ThisPropertyAssignmentKey {
    pub(crate) name: String,
    pub(crate) node: Node,
    pub(crate) is_static: bool,
    pub(crate) is_private: bool,
}

// Go: transformers/declarations/transform.go:53 getThisPropertyAssignmentKey
pub(crate) fn get_this_property_assignment_key(
    name: Node,
    node: Node,
    is_static: bool,
) -> ThisPropertyAssignmentKey {
    let is_private = is_private_identifier(name);
    if name.is_some() && !is_dynamic_name(name) {
        let (name_text, ok) = try_get_text_of_property_name(name);
        if ok {
            return ThisPropertyAssignmentKey {
                name: name_text,
                node: Node::NIL,
                is_static,
                is_private,
            };
        }
    }
    ThisPropertyAssignmentKey {
        name: String::new(),
        node,
        is_static,
        is_private,
    }
}

// Go: transformers/declarations/transform.go:63 DeclarationTransformer
// PORT: Go embeds `transformers.Transformer` (emit context, factory and root
// visitor). The emit context is the `emit_context` field; the factory is
// `emit_context.factory()`; the root visitor is built by `with_visitor`
// (transform_p2.rs). Go also builds six `*ast.NodeVisitor` fields in the
// constructor (bindingNameVisitor, expressionVisitor,
// cjsExportAssignmentVisitor, exportStrippingVisitor, thisPropertyVisitor,
// declareStrippingVisitor). They are not fields: each visitor holds its
// callback's receiver, so the methods that use them build them on demand.
// Go maps keyed by `ast.NodeId` are keyed by `Node`.
pub struct DeclarationTransformer {
    pub(crate) emit_context: Rc<EmitContext>,
    pub(crate) host: Rc<dyn DeclarationEmitHost>,
    pub(crate) compiler_options: &'static CompilerOptions,
    pub(crate) tracker: Rc<SymbolTrackerImpl>,
    pub(crate) state: Rc<RefCell<SymbolTrackerSharedState>>,
    pub(crate) resolver: Rc<dyn EmitResolver>,
    pub(crate) declaration_file_path: String,
    pub(crate) declaration_map_path: String,

    pub(crate) needs_declare: bool,
    pub(crate) needs_scope_fix_marker: bool,
    pub(crate) result_has_scope_marker: bool,
    pub(crate) enclosing_declaration: Node,
    pub(crate) result_has_external_module_indicator: bool,
    pub(crate) suppress_new_diagnostic_contexts: bool,
    pub(crate) witnessed_cjs_exports: FxHashSet<String>,
    pub(crate) late_statement_replacement_map: FxHashMap<Node, Node>,
    // store the result of transforming expando hosts so they can be inserted later if the host is actually referenced
    pub(crate) expando_hosts: FxHashMap<Node, Node>,
    // store any found expando _members_ after transforming them so *if* the host is referenced, they can be emitted alongside it
    pub(crate) expando_members: FxHashMap<Node, Vec<Node>>,
    // expando assignments whose host wasn't visible when collected, processed if the host is late-marked visible
    pub(crate) deferred_expando_assignments: FxHashMap<Node, Vec<Node>>,
    pub(crate) seen_properties: FxHashSet<ThisPropertyAssignmentKey>,
    pub(crate) this_property_assignments_collected: Vec<Node>,
    pub(crate) raw_referenced_files: Vec<ReferencedFilePair>,
    pub(crate) raw_type_reference_directives: Vec<FileReference>,
    pub(crate) raw_lib_reference_directives: Vec<FileReference>,

    pub(crate) cjs_export_assignment: Node,
    pub(crate) cjs_export_members: Vec<Node>,
    // tracks the name node used for `export =` in CJS module.exports assignments
    pub(crate) cjs_export_assignment_name: Node,
    // true when serializing members of a class expression kept as a class declaration
    pub(crate) in_class_expression_declaration: bool,
}

// Go: transformers/declarations/transform.go:99 NewDeclarationTransformer
// TODO: Convert to transformers.TransformerFactory signature to allow more automatic composition with other transforms
// ts#64649: takes the emit resolver, and the emit context is the resolver's.
// Go panics when the resolver or its context is nil; neither is nil here.
// PORT: Go stores `reportExpandoFunctionErrors` as a closure on the state;
// it is `SymbolTrackerSharedState::report_expando_function_errors`. The Go
// visitor fields are built on demand (see the struct comment).
pub fn new_declaration_transformer(
    host: Rc<dyn DeclarationEmitHost>,
    resolver: Rc<dyn EmitResolver>,
    compiler_options: &'static CompilerOptions,
    declaration_file_path: &str,
    declaration_map_path: &str,
) -> DeclarationTransformer {
    let emit_context = resolver.emit_context().clone();
    let state = Rc::new(RefCell::new(SymbolTrackerSharedState {
        late_marked_statements: Vec::new(),
        diagnostics: Vec::new(),
        get_symbol_accessibility_diagnostic: None,
        error_name_node: Node::NIL,
        isolated_declarations: compiler_options.isolated_declarations.is_true(),
        strip_internal: compiler_options.strip_internal.is_true(),
        current_source_file: Node::NIL,
        resolver: resolver.clone(),
    }));
    let tracker = Rc::new(new_symbol_tracker(
        host.clone(),
        resolver.clone(),
        state.clone(),
    ));
    // TODO: Use new host GetOutputPathsFor method instead of passing in entrypoint paths (which will also better support bundled emit)
    DeclarationTransformer {
        emit_context,
        host,
        compiler_options,
        tracker,
        state,
        resolver,
        declaration_file_path: declaration_file_path.to_string(),
        declaration_map_path: declaration_map_path.to_string(),
        needs_declare: false,
        needs_scope_fix_marker: false,
        result_has_scope_marker: false,
        enclosing_declaration: Node::NIL,
        result_has_external_module_indicator: false,
        suppress_new_diagnostic_contexts: false,
        witnessed_cjs_exports: FxHashSet::default(),
        late_statement_replacement_map: FxHashMap::default(),
        expando_hosts: FxHashMap::default(),
        expando_members: FxHashMap::default(),
        deferred_expando_assignments: FxHashMap::default(),
        seen_properties: FxHashSet::default(),
        this_property_assignments_collected: Vec::new(),
        raw_referenced_files: Vec::new(),
        raw_type_reference_directives: Vec::new(),
        raw_lib_reference_directives: Vec::new(),
        cjs_export_assignment: Node::NIL,
        cjs_export_members: Vec::new(),
        cjs_export_assignment_name: Node::NIL,
        in_class_expression_declaration: false,
    }
}

/// Go `declarationEmitNodeBuilderFlags`.
// Go: transformers/declarations/transform.go:215 declarationEmitNodeBuilderFlags
#[allow(dead_code)]
pub(crate) const DECLARATION_EMIT_NODE_BUILDER_FLAGS: NodeBuilderFlags =
    NodeBuilderFlags::MULTILINE_OBJECT_LITERALS
        .union(NodeBuilderFlags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL)
        .union(NodeBuilderFlags::USE_TYPE_OF_FUNCTION)
        .union(NodeBuilderFlags::USE_STRUCTURAL_FALLBACK)
        .union(NodeBuilderFlags::ALLOW_EMPTY_TUPLE)
        .union(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS)
        .union(NodeBuilderFlags::NO_TRUNCATION);

/// Go `declarationEmitInternalNodeBuilderFlags`.
// Go: transformers/declarations/transform.go:223 declarationEmitInternalNodeBuilderFlags
#[allow(dead_code)]
pub(crate) const DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS: InternalNodeBuilderFlags =
    InternalNodeBuilderFlags::ALLOW_UNRESOLVED_NAMES;

/// The Go func that `setupDiagnosticContext` returns. Run it with
/// `cleanup.run(self)` where Go runs `defer cleanupDiagnosticContext()`.
// PORT: a Go closure over `tx` cannot borrow the transformer here, so the
// saved values are a struct and `run` takes the transformer.
pub(crate) struct CleanupDiagnosticContext {
    old_diag: Option<GetSymbolAccessibilityDiagnostic>,
    old_name: Node,
    old_within_object_literal_type: bool,
}

impl CleanupDiagnosticContext {
    // Go: transformers/declarations/transform.go:551 setupDiagnosticContext cleanup func
    pub(crate) fn run(self, tx: &mut DeclarationTransformer) {
        {
            let mut state = tx.state.borrow_mut();
            state.get_symbol_accessibility_diagnostic = self.old_diag;
            state.error_name_node = self.old_name;
        }
        tx.suppress_new_diagnostic_contexts = self.old_within_object_literal_type;
    }
}

impl DeclarationTransformer {
    // Go: transformers/transformer.go:39 Transformer.TransformSourceFile
    pub fn transform_source_file_root(&mut self, file: Node) -> Node {
        self.with_visitor(|v| v.visit_source_file(file))
    }

    // Go: transformers/declarations/transform.go:142 DeclarationTransformer.GetDiagnostics
    pub fn get_diagnostics(&self) -> Vec<Diagnostic> {
        self.state.borrow().diagnostics.clone()
    }

    // Go: transformers/declarations/transform.go:146 DeclarationTransformer.shouldStripInternal
    pub(crate) fn should_strip_internal(&self, node: Node) -> bool {
        let (strip_internal, current_source_file) = {
            let state = self.state.borrow();
            (state.strip_internal, state.current_source_file)
        };
        strip_internal && node.is_some() && self.is_internal_declaration(node, current_source_file)
    }

    // Go: transformers/declarations/transform.go:150 DeclarationTransformer.isInternalDeclaration
    fn is_internal_declaration(&self, node: Node, source_file: Node) -> bool {
        if node.is_nil() {
            return false;
        }
        let parse_tree_node = self.emit_context.most_original(node);
        if !is_parse_tree_node(parse_tree_node) {
            return false;
        }
        if parse_tree_node.kind() == SyntaxKind::Parameter {
            let params = parse_tree_node.parent().parameters();
            let param_idx = params.iter().position(|p| p == parse_tree_node);
            let mut previous_sibling = Node::NIL;
            if let Some(idx) = param_idx {
                if idx > 0 {
                    previous_sibling = params.get(idx - 1);
                }
            }

            let text = source_file_text(source_file);
            let mut comment_ranges: Vec<CommentRange> = Vec::new();

            if previous_sibling.is_some() {
                // to handle
                // ... parameters, /** @internal */
                // public param: string
                let trailing_pos = skip_trivia_ex(
                    &text,
                    previous_sibling.end() + 1,
                    Some(&SkipTriviaOptions {
                        stop_at_comments: true,
                        ..Default::default()
                    }),
                );
                comment_ranges.extend(get_trailing_comment_ranges(&text, trailing_pos));
                comment_ranges.extend(get_leading_comment_ranges(&text, node.pos()));
            } else {
                let trailing_pos = skip_trivia_ex(
                    &text,
                    node.pos(),
                    Some(&SkipTriviaOptions {
                        stop_at_comments: true,
                        ..Default::default()
                    }),
                );
                comment_ranges.extend(get_trailing_comment_ranges(&text, trailing_pos));
            }

            if let Some(last) = comment_ranges.last() {
                return has_internal_annotation(last, source_file);
            }
            return false;
        }

        for comment_range in self.get_leading_comment_ranges_of_node(parse_tree_node, source_file) {
            if has_internal_annotation(&comment_range, source_file) {
                return true;
            }
        }
        false
    }

    // Go: transformers/declarations/transform.go:203 DeclarationTransformer.getLeadingCommentRangesOfNode
    fn get_leading_comment_ranges_of_node(
        &self,
        node: Node,
        source_file: Node,
    ) -> Vec<CommentRange> {
        if node.is_nil() || node.kind() == SyntaxKind::JsxText {
            return Vec::new();
        }
        get_leading_comment_ranges(&source_file_text(source_file), node.pos())
    }

    // Go: transformers/declarations/transform.go:226 DeclarationTransformer.visit
    // functions as both `visitDeclarationStatements` and `transformRoot`, utilitzing SyntaxList nodes
    pub fn visit(&mut self, node: Node) -> Node {
        if node.is_nil() {
            return Node::NIL;
        }
        match node.kind() {
            SyntaxKind::SourceFile => self.visit_source_file(node),
            // statements we keep but do something to
            SyntaxKind::FunctionDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::VariableStatement
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::ExportDeclaration
            | SyntaxKind::ExportAssignment => self.visit_declaration_statements(node),
            // statements we elide
            SyntaxKind::BreakStatement
            | SyntaxKind::ContinueStatement
            | SyntaxKind::DebuggerStatement
            | SyntaxKind::DoStatement
            | SyntaxKind::EmptyStatement
            | SyntaxKind::ForInStatement
            | SyntaxKind::ForOfStatement
            | SyntaxKind::ForStatement
            | SyntaxKind::IfStatement
            | SyntaxKind::LabeledStatement
            | SyntaxKind::ReturnStatement
            | SyntaxKind::SwitchStatement
            | SyntaxKind::ThrowStatement
            | SyntaxKind::TryStatement
            | SyntaxKind::WhileStatement
            | SyntaxKind::WithStatement
            | SyntaxKind::NotEmittedStatement
            | SyntaxKind::Block
            | SyntaxKind::MissingDeclaration
            | SyntaxKind::ExpressionStatement => Node::NIL,
            // parts of things, things we just visit children of
            kind => self.visit_declaration_subtree(node, kind),
        }
    }

    // Go: transformers/declarations/transform.go:280 DeclarationTransformer.visitSourceFile
    fn visit_source_file(&mut self, node: Node) -> Node {
        self.cjs_export_assignment_name = Node::NIL;
        if source_file_info(node).is_declaration_file {
            return node;
        }

        self.needs_declare = true;
        self.needs_scope_fix_marker = false;
        self.result_has_scope_marker = false;
        self.enclosing_declaration = node;
        self.state.borrow_mut().get_symbol_accessibility_diagnostic = Some(throw_diagnostic());
        self.result_has_external_module_indicator = false;
        self.suppress_new_diagnostic_contexts = false;
        self.state.borrow_mut().late_marked_statements = Vec::new();
        self.late_statement_replacement_map = FxHashMap::default();
        self.expando_hosts = FxHashMap::default();
        self.expando_members = FxHashMap::default();
        self.deferred_expando_assignments = FxHashMap::default();
        self.raw_referenced_files = Vec::new();
        self.raw_type_reference_directives = Vec::new();
        self.raw_lib_reference_directives = Vec::new();
        self.witnessed_cjs_exports.clear();
        self.state.borrow_mut().current_source_file = node;
        self.collect_file_references(node);
        let original = self.emit_context.most_original(node);
        self.resolver
            .precalculate_declaration_emit_visibility(original);
        let updated = self.transform_source_file(node);
        self.state.borrow_mut().current_source_file = Node::NIL;
        updated
    }

    // Go: transformers/declarations/transform.go:310 DeclarationTransformer.collectFileReferences
    fn collect_file_references(&mut self, source_file: Node) {
        let info = source_file_info(source_file);
        self.raw_referenced_files
            .extend(info.referenced_files.iter().map(|r| ReferencedFilePair {
                file: source_file,
                r#ref: r.clone(),
            }));
        self.raw_type_reference_directives
            .extend(info.type_reference_directives.iter().cloned());
        self.raw_lib_reference_directives
            .extend(info.lib_reference_directives.iter().cloned());
    }

    // Go: transformers/declarations/transform.go:327 DeclarationTransformer.appendCjsExports
    fn append_cjs_exports(&self, combined_statements: NodeList) -> NodeList {
        let mut result: Vec<Node> = Vec::new();
        if self.cjs_export_assignment.is_some() {
            result.push(self.cjs_export_assignment);
        }
        result.extend(self.cjs_export_members.iter().copied());
        result.extend(combined_statements.nodes().iter());
        let statement_nodes = flatten_syntax_lists(&result);
        if statement_nodes.len() != combined_statements.nodes().len() {
            return self.emit_context.factory().new_node_list(&statement_nodes);
        }
        combined_statements
    }

    // Go: transformers/declarations/transform.go:339 DeclarationTransformer.transformSourceFile
    fn transform_source_file(&mut self, node: Node) -> Node {
        self.cjs_export_assignment = Node::NIL;
        self.cjs_export_assignment_name = Node::NIL;
        self.cjs_export_members = Vec::new();
        // PORT: Go resets the three cjs fields again in a `defer`. The body
        // has one exit; the reset is at the end. A panic does not need it.
        //
        // PORT: Go calls `tx.cjsExportAssignmentVisitor.VisitNode(file)` and
        // `tx.expressionVisitor.VisitNode(file)` and drops the results.
        // `VisitNode` calls the callback, then only lifts a SyntaxList result.
        // Both callbacks return a SourceFile for a SourceFile, so the callbacks
        // are called directly.
        //
        // PERF: the two walks act only on CommonJS exports, which need a
        // `common_js_module_indicator`, and on expando assignments that the
        // binder gave an `ASSIGNMENT` symbol (`transform_expando_assignment`
        // returns early for any other node). Elsewhere they only walk: an
        // unchanged tree makes no node, and each diagnostic context is
        // restored. So a parsed file with neither skips them. A factory
        // SourceFile always walks.
        let (cjs_walk, expando_walk) = if is_synthetic_node(node) {
            (true, true)
        } else {
            let cjs = source_file_info(node).common_js_module_indicator.is_some();
            let expando = node
                .go_file()
                .file_bind
                .get()
                .is_none_or(|bind| bind.has_expando_assignments);
            (cjs, cjs || expando)
        };
        if cjs_walk {
            self.visit_cjs_export_assignments(node); // collect nested module.exports= assignments
        }
        if expando_walk {
            self.visit_nested_expression(node); // collect expando members (requires any export assignment be located in advance)
        }
        let source_statements = node.statement_list();
        let statements = self.with_visitor(|v| v.visit_nodes(source_statements));
        let mut combined_statements =
            self.transform_and_replace_late_painted_statements(statements);
        combined_statements = self.append_cjs_exports(combined_statements);
        // setTextRange
        // PORT: a synthetic list fixes its `Loc` at creation, so Go
        // `combinedStatements.Loc = statements.Loc` makes the list again. The
        // list is always new here, so no other holder sees the change.
        combined_statements =
            new_synthetic_node_list(&combined_statements.nodes().to_vec(), statements.loc());
        if is_external_or_common_js_module(node) {
            if is_in_js_file(node) {
                let export_equals = {
                    let symbols = crate::program::bound_symbols();
                    symbols.get(
                        symbols.sym(node.symbol()).exports,
                        INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
                    )
                };
                if export_equals.is_some() {
                    let declarations = bound_symbol_declarations(export_equals);
                    if declarations.len() > 1 {
                        for node in declarations {
                            self.state.borrow_mut().add_diagnostic(create_diagnostic_for_node(
                                node,
                                diag::Multiple_module_exports_assignments_cannot_be_serialized_for_declaration_emit,
                                args![],
                            ));
                        }
                    }
                }
            }
            if !self.result_has_external_module_indicator
                || (self.needs_scope_fix_marker && !self.result_has_scope_marker)
            {
                let marker = create_empty_exports(self.emit_context.factory().as_node_factory());
                let mut new_list = combined_statements.nodes().to_vec();
                new_list.push(marker);
                // PORT: Go `NewNodeList(newList)` then `withMarker.Loc = combinedStatements.Loc`.
                let with_marker = new_synthetic_node_list(&new_list, combined_statements.loc());
                combined_statements = with_marker;
            }
        }
        // ts#64159 (transform.go:375): Go `tx.declarationFilePath.Directory()`.
        let output_file_path = tspath::get_directory_path(&self.declaration_file_path);
        let result = self.emit_context.factory().update_source_file(
            node,
            combined_statements,
            node.end_of_file_token(),
        );
        let lib_reference_directives = self.get_lib_references();
        let type_reference_directives = self.get_type_references();
        let referenced_files = self.get_referenced_files(&output_file_path);
        // PORT: the result is always a new factory SourceFile (the statement
        // list above is always new), so the Go field writes go to its slot.
        update_synthetic_source_file(result, |d| {
            d.lib_reference_directives = lib_reference_directives;
            d.type_reference_directives = type_reference_directives;
            d.is_declaration_file = true;
            d.referenced_files = referenced_files;
        });
        // Go: defer
        self.cjs_export_assignment = Node::NIL;
        self.cjs_export_assignment_name = Node::NIL;
        self.cjs_export_members = Vec::new();
        result
    }

    // Go: transformers/declarations/transform.go:386 DeclarationTransformer.transformAndReplaceLatePaintedStatements
    pub(crate) fn transform_and_replace_late_painted_statements(
        &mut self,
        statements: NodeList,
    ) -> NodeList {
        // This is a `while` loop because `handleSymbolAccessibilityError` can see additional import aliases marked as visible during
        // error handling which must now be included in the output and themselves checked for errors.
        // For example:
        // ```
        // module A {
        //   export module Q {}
        //   import B = Q;
        //   import C = B;
        //   export import D = C;
        // }
        // ```
        // In such a scenario, only Q and D are initially visible, but we don't consider imports as private names - instead we say they if they are referenced they must
        // be recorded. So while checking D's visibility we mark C as visible, then we must check C which in turn marks B, completing the chain of
        // dependent imports and allowing a valid declaration file output. Today, this dependent alias marking only happens for internal import aliases.
        loop {
            let next = {
                let mut state = self.state.borrow_mut();
                if state.late_marked_statements.is_empty() {
                    break;
                }
                state.late_marked_statements.remove(0)
            };

            let save_needs_declare = self.needs_declare;
            self.needs_declare = next.parent().is_some() && is_source_file(next.parent());

            let result = self.transform_top_level_declaration(next);

            self.needs_declare = save_needs_declare;
            let original = self.emit_context.most_original(next);
            self.late_statement_replacement_map.insert(original, result);
        }

        // And lastly, we need to get the final form of all those indetermine import declarations from before and add them to the output list
        // (and remove them from the set to examine for outter declarations)
        let mut results: Vec<Node> = Vec::with_capacity(statements.nodes().len());
        for statement in statements.nodes().iter() {
            if !is_late_visibility_painted_statement(statement) {
                results.push(statement);
                continue;
            }
            let original = self.emit_context.most_original(statement);
            let Some(&replacement) = self.late_statement_replacement_map.get(&original) else {
                results.push(statement);
                continue; // not replaced
            };
            if replacement.is_nil() {
                continue; // deleted
            }
            if replacement.kind() == SyntaxKind::SyntaxList {
                let children = syntax_list_children(replacement);
                if !self.needs_scope_fix_marker || !self.result_has_external_module_indicator {
                    for &elem in &children {
                        if needs_scope_marker(elem) {
                            self.needs_scope_fix_marker = true;
                        }
                        if is_source_file(statement.parent()) && is_external_module_indicator(elem)
                        {
                            self.result_has_external_module_indicator = true;
                        }
                    }
                }
                results.extend(children);
            } else {
                if needs_scope_marker(replacement) {
                    self.needs_scope_fix_marker = true;
                }
                if is_source_file(statement.parent()) && is_external_module_indicator(replacement) {
                    self.result_has_external_module_indicator = true;
                }
                results.push(replacement);
            }
        }

        self.emit_context.factory().new_node_list(&results)
    }

    // Go: transformers/declarations/transform.go:461 DeclarationTransformer.getReferencedFiles
    fn get_referenced_files(&self, output_file_path: &str) -> Vec<FileReference> {
        let mut results = Vec::new();
        // Handle path rewrites for triple slash ref comments
        for pair in &self.raw_referenced_files {
            let source_file = pair.file;
            let r#ref = &pair.r#ref;

            if !r#ref.preserve {
                continue;
            }

            let file = self.host.get_source_file_from_reference(source_file, r#ref);
            if file.is_nil() {
                continue;
            }

            let mut decl_file_name: String;
            if source_file_info(file).is_declaration_file {
                decl_file_name = source_file_file_name(file).to_string();
            } else {
                let paths = self.host.get_output_paths_for(file, true);
                // Try to use output path for referenced file, or output js path if that doesn't exist, or the input path if all else fails
                decl_file_name = paths.declaration_file_path();
                if decl_file_name.is_empty() {
                    decl_file_name = paths.js_file_path();
                }
                if decl_file_name.is_empty() {
                    decl_file_name = source_file_file_name(file).to_string();
                }
            }
            // Should only be missing if the source file is missing a fileName (at which point we can't name a reference to it anyway)
            // TODO: Shouldn't this be a crash or assert instead of a silent continue?
            if decl_file_name.is_empty() {
                continue;
            }

            // ts#64159 (transform.go:496): both paths are rooted, so Go passes no
            // current directory.
            let file_name = tspath::get_relative_path_to_directory_or_url(
                output_file_path,
                &decl_file_name,
                false,
                &tspath::ComparePathsOptions {
                    use_case_sensitive_file_names: self.host.use_case_sensitive_file_names(),
                    current_directory: String::new(),
                },
            );

            results.push(FileReference {
                range: TextRange::new(-1, -1),
                file_name,
                resolution_mode: r#ref.resolution_mode,
                preserve: r#ref.preserve,
            });
        }
        results
    }

    // Go: transformers/declarations/transform.go:519 DeclarationTransformer.getLibReferences
    fn get_lib_references(&self) -> Vec<FileReference> {
        // clone retained references
        retained_references(&self.raw_lib_reference_directives)
    }

    // Go: transformers/declarations/transform.go:535 DeclarationTransformer.getTypeReferences
    fn get_type_references(&self) -> Vec<FileReference> {
        // clone retained references
        retained_references(&self.raw_type_reference_directives)
    }

    // Go: transformers/declarations/transform.go:551 DeclarationTransformer.setupDiagnosticContext
    pub(crate) fn setup_diagnostic_context(
        &mut self,
        input: Node,
    ) -> (bool, CleanupDiagnosticContext) {
        self.setup_diagnostic_context_of_kind(input, input.kind())
    }

    /// `setup_diagnostic_context` of a node of kind `kind`.
    // PERF: emitast2. The caller has read the kind (`visit_declaration_subtree`).
    fn setup_diagnostic_context_of_kind(
        &mut self,
        input: Node,
        kind: SyntaxKind,
    ) -> (bool, CleanupDiagnosticContext) {
        let can_produce_diagnostic = can_produce_diagnostics_kind(kind);
        let old_within_object_literal_type = self.suppress_new_diagnostic_contexts;
        // PERF: pure kind tests; read each kind once. The parent kind is still
        // read only for a type literal or mapped type, as in Go.
        let should_enter_suppress_new_diagnostics_context_context =
            matches!(kind, SyntaxKind::TypeLiteral | SyntaxKind::MappedType)
                && !matches!(
                    input.parent().kind(),
                    SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration
                );

        let old_diag = self
            .state
            .borrow()
            .get_symbol_accessibility_diagnostic
            .clone();
        if can_produce_diagnostic && !self.suppress_new_diagnostic_contexts {
            self.state.borrow_mut().get_symbol_accessibility_diagnostic =
                Some(create_get_symbol_accessibility_diagnostic_for_node(input));
        }
        let old_name = self.state.borrow().error_name_node;

        if should_enter_suppress_new_diagnostics_context_context {
            self.suppress_new_diagnostic_contexts = true;
        }

        (
            can_produce_diagnostic,
            CleanupDiagnosticContext {
                old_diag,
                old_name,
                old_within_object_literal_type,
            },
        )
    }

    // Go: transformers/declarations/transform.go:573 DeclarationTransformer.visitDeclarationSubtree
    // PERF: emitast2. `kind` is the kind of `input`, which `visit` has read.
    // The kind tests below use it: a kind read of a factory node goes to the
    // synthetic arena each time.
    fn visit_declaration_subtree(&mut self, input: Node, kind: SyntaxKind) -> Node {
        if self.should_strip_internal(input) {
            return Node::NIL;
        }
        // Go `ast.IsDeclaration(input)`.
        let is_declaration = if kind == SyntaxKind::TypeParameter {
            input.parent().is_some()
        } else {
            is_declaration_node(input)
        };
        if is_declaration {
            if is_declaration_and_not_visible(&self.emit_context, &*self.resolver, input) {
                return Node::NIL;
            }
            if has_dynamic_name(input) {
                let isolated_declarations = self.state.borrow().isolated_declarations;
                if isolated_declarations {
                    // Classes and object literals usually elide properties with computed names that are not of a literal type
                    // In isolated declarations TSC needs to error on these as we don't know the type in a DTE.
                    if !self
                        .resolver
                        .is_definitely_reference_to_global_symbol_object(input.name().expression())
                    {
                        if is_class_declaration(input.parent())
                            || is_object_literal_expression(input.parent())
                        {
                            self.state.borrow_mut().add_diagnostic(create_diagnostic_for_node(
                                input,
                                diag::Computed_property_names_on_class_or_object_literals_cannot_be_inferred_with_isolatedDeclarations,
                                args![],
                            ));
                            return Node::NIL;
                        } else if (is_interface_declaration(input.parent())
                            || is_type_literal_node(input.parent()))
                            && !is_entity_name_expression(input.name().expression())
                        {
                            // Type declarations just need to double-check that the input computed name is an entity name expression
                            self.state.borrow_mut().add_diagnostic(create_diagnostic_for_node(
                                input,
                                diag::Computed_properties_must_be_number_or_string_literals_variables_or_dotted_expressions_with_isolatedDeclarations,
                                args![],
                            ));
                            return Node::NIL;
                        }
                    }
                } else if !self
                    .resolver
                    .is_late_bound(self.emit_context.parse_node(input))
                    || !is_entity_name_expression(input.name().expression())
                {
                    return Node::NIL;
                }
            }
        }

        // Elide implementation signatures from overload sets
        if is_function_like_kind(kind) && self.resolver.is_implementation_of_overload(input) {
            return Node::NIL;
        }

        if kind == SyntaxKind::SemicolonClassElement {
            return Node::NIL;
        }

        if kind == SyntaxKind::HeritageClause {
            let types = input.types().nodes();
            if types.len() == 0 || (types.len() == 1 && node_is_missing(types.get(0))) {
                return Node::NIL;
            }
        }

        let previous_enclosing_declaration = self.enclosing_declaration;
        if is_enclosing_declaration_kind(kind) {
            self.enclosing_declaration = input;
        }

        let (can_produce_diagnostic, cleanup_diagnostic_context) =
            self.setup_diagnostic_context_of_kind(input, kind);

        let result = match kind {
            SyntaxKind::MappedType => self.transform_mapped_type_node(input),
            SyntaxKind::HeritageClause => self.transform_heritage_clause(input),
            SyntaxKind::MethodSignature => self.transform_method_signature_declaration(input),
            SyntaxKind::MethodDeclaration => self.transform_method_declaration(input),
            SyntaxKind::ConstructSignature => self.transform_construct_signature_declaration(input),
            SyntaxKind::Constructor => self.transform_constructor_declaration(input),
            SyntaxKind::GetAccessor => self.transform_get_accesor_declaration(input),
            SyntaxKind::SetAccessor => self.transform_set_accessor_declaration(input),
            SyntaxKind::PropertyDeclaration => self.transform_property_declaration(input),
            SyntaxKind::PropertySignature => self.transform_property_signature_declaration(input),
            SyntaxKind::CallSignature => self.transform_call_signature_declaration(input),
            SyntaxKind::IndexSignature => self.transform_index_signature_declaration(input),
            SyntaxKind::VariableDeclaration => self.transform_variable_declaration(input),
            SyntaxKind::TypeParameter => self.transform_type_parameter_declaration(input),
            SyntaxKind::ExpressionWithTypeArguments => {
                self.transform_expression_with_type_arguments(input)
            }
            SyntaxKind::TypeReference => self.transform_type_reference(input),
            SyntaxKind::ConditionalType => self.transform_conditional_type_node(input),
            SyntaxKind::FunctionType => self.transform_function_type_node(input),
            SyntaxKind::ConstructorType => self.transform_constructor_type_node(input),
            SyntaxKind::ImportType => self.transform_import_type_node(input),
            SyntaxKind::TypeQuery => {
                let enclosing_declaration = self.enclosing_declaration;
                self.check_entity_name_visibility(input.expr_name(), enclosing_declaration);
                self.with_visitor(|v| v.visit_each_child(input))
            }
            SyntaxKind::QualifiedName => {
                if input.right().kind() == SyntaxKind::PrivateIdentifier {
                    self.state.borrow_mut().add_diagnostic(create_diagnostic_for_node(
                        input,
                        diag::Declaration_emit_elides_private_members_but_0_refers_to_a_private_member_Write_an_explicit_type_here,
                        args![input.right().text()],
                    ));
                }
                self.with_visitor(|v| v.visit_each_child(input))
            }
            SyntaxKind::TupleType => {
                let result = self.with_visitor(|v| v.visit_each_child(input));
                if result.is_some() && is_original_node_single_line(&self.emit_context, input) {
                    self.emit_context
                        .add_emit_flags(result, EmitFlags::SINGLE_LINE);
                }
                result
            }
            SyntaxKind::JsDocTypeExpression => self.transform_js_doc_type_expression(input),
            SyntaxKind::JsDocTypeLiteral => self.transform_js_doc_type_literal(input),
            SyntaxKind::JsDocPropertyTag => self.transform_js_doc_property_tag(input),
            SyntaxKind::JsDocAllType => self.transform_js_doc_all_type(input),
            SyntaxKind::JsDocNullableType => self.transform_js_doc_nullable_type(input),
            SyntaxKind::JsDocNonNullableType => self.transform_js_doc_non_nullable_type(input),
            SyntaxKind::JsDocOptionalType => self.transform_js_doc_optional_type(input),
            SyntaxKind::JsDocVariadicType => self.transform_js_doc_variadic_type(input),
            _ => self.with_visitor(|v| v.visit_each_child(input)),
        };

        if result.is_some() && can_produce_diagnostic && has_dynamic_name(input) {
            self.check_name(input);
        }

        self.enclosing_declaration = previous_enclosing_declaration;
        // Go: defer cleanupDiagnosticContext()
        cleanup_diagnostic_context.run(self);
        result
    }

    // Go: transformers/declarations/transform.go:708 DeclarationTransformer.checkName
    pub(crate) fn check_name(&mut self, node: Node) {
        let old_diag = self
            .state
            .borrow()
            .get_symbol_accessibility_diagnostic
            .clone();
        if !self.suppress_new_diagnostic_contexts {
            self.state.borrow_mut().get_symbol_accessibility_diagnostic = Some(
                create_get_symbol_accessibility_diagnostic_for_node_name(node),
            );
        }
        self.state.borrow_mut().error_name_node = node.name();
        go_assert!(has_dynamic_name(node)); // Should only be called with dynamic names
        let entity_name = node.name().expression();
        let enclosing_declaration = self.enclosing_declaration;
        self.check_entity_name_visibility(entity_name, enclosing_declaration);
        if !self.suppress_new_diagnostic_contexts {
            self.state.borrow_mut().get_symbol_accessibility_diagnostic = old_diag;
        }
        self.state.borrow_mut().error_name_node = Node::NIL;
    }

    // Go: transformers/declarations/transform.go:723 DeclarationTransformer.transformMappedTypeNode
    fn transform_mapped_type_node(&mut self, input: Node) -> Node {
        // handle missing template type nodes, since the printer does not
        let type_node = if input.type_().is_nil() {
            self.emit_context
                .factory()
                .new_keyword_type_node(SyntaxKind::AnyKeyword)
        } else {
            self.visit(input.type_())
        };
        let type_parameter = self.visit(input.type_parameter());
        let name_type = self.visit(input.name_type());
        self.emit_context.factory().update_mapped_type_node(
            input,
            input.readonly_token(),
            type_parameter,
            name_type,
            input.question_token(),
            type_node,
            NodeList::NIL,
        )
    }

    // Go: transformers/declarations/transform.go:742 DeclarationTransformer.transformHeritageClause
    fn transform_heritage_clause(&mut self, clause: Node) -> Node {
        let types = clause.types().nodes();
        let retained_clauses: Vec<Node> = types
            .iter()
            .filter(|&t| {
                // Go: the element is an ExpressionWithTypeArguments or a TypeReference (tsgo#4797).
                let name = get_heritage_clause_element_name(t);
                is_entity_name(name)
                    || is_entity_name_expression(name)
                    || (clause.token() == SyntaxKind::ExtendsKeyword
                        && is_expression_with_type_arguments(t)
                        && t.expression().kind() == SyntaxKind::NullKeyword)
            })
            .collect();
        if retained_clauses.is_empty() {
            return Node::NIL; // elide empty clause
        }
        if retained_clauses.len() == types.len() {
            return self.with_visitor(|v| v.visit_each_child(clause));
        }
        let retained = self.emit_context.factory().new_node_list(&retained_clauses);
        let types = self.with_visitor(|v| v.visit_nodes(retained));
        self.emit_context
            .factory()
            .update_heritage_clause(clause, clause.token(), types)
    }
}

// Go: transformers/declarations/transform.go:210 hasInternalAnnotation
fn has_internal_annotation(comment_range: &CommentRange, source_file: Node) -> bool {
    let comment =
        &source_file_text(source_file)[comment_range.pos() as usize..comment_range.end() as usize];
    comment.contains("@internal")
}

// Go: transformers/declarations/transform.go:276 throwDiagnostic
fn throw_diagnostic() -> GetSymbolAccessibilityDiagnostic {
    Rc::new(
        |_result: &SymbolAccessibilityResult| -> Option<SymbolAccessibilityDiagnostic> {
            panic!("Diagnostic emitted without context")
        },
    )
}

// Go: transformers/declarations/transform.go:316 nodeOrSyntaxListChildren
pub(crate) fn node_or_syntax_list_children(node: Node) -> Vec<Node> {
    if is_syntax_list(node) {
        return syntax_list_children(node);
    }
    vec![node]
}

// Go: transformers/declarations/transform.go:323 flattenSyntaxLists
pub(crate) fn flatten_syntax_lists(nodes: &[Node]) -> Vec<Node> {
    nodes
        .iter()
        .flat_map(|&n| node_or_syntax_list_children(n))
        .collect()
}

// Go: transformers/declarations/transform.go:382 createEmptyExports
pub(crate) fn create_empty_exports(factory: &NodeFactory) -> Node {
    factory.new_export_declaration(
        ModifierList::NIL,
        false, /*isTypeOnly*/
        factory.new_named_exports(factory.new_node_list(&[])),
        Node::NIL,
        Node::NIL,
    )
}

// Go: transformers/declarations/transform.go:519 getLibReferences loop body
// PORT: `getLibReferences` and `getTypeReferences` have the same loop over a
// different field; it is shared here.
fn retained_references(refs: &[FileReference]) -> Vec<FileReference> {
    let mut result = Vec::new();
    for r#ref in refs {
        if !r#ref.preserve {
            continue;
        }
        result.push(FileReference {
            range: TextRange::new(-1, -1),
            file_name: r#ref.file_name.clone(),
            resolution_mode: r#ref.resolution_mode,
            preserve: r#ref.preserve,
        });
    }
    result
}

// Go: scanner/scanner.go:2799 GetLeadingCommentRanges
// PORT: Go takes the node factory only to allocate the ranges; it is dropped.
// The Go iterator is collected into a Vec; every caller reads all of it.
fn get_leading_comment_ranges(text: &str, pos: i32) -> Vec<CommentRange> {
    iterate_comment_ranges(text, pos, false)
}

// Go: scanner/scanner.go:2803 GetTrailingCommentRanges
// PORT: see get_leading_comment_ranges.
fn get_trailing_comment_ranges(text: &str, pos: i32) -> Vec<CommentRange> {
    iterate_comment_ranges(text, pos, true)
}

/// Go `utf8.DecodeRuneInString(text[pos:])` for `pos < len(text)`. A
/// position that is not a char boundary gives `(RuneError, 1)`, like Go on
/// invalid UTF-8.
fn decode_rune(text: &str, pos: usize) -> (char, usize) {
    match text.get(pos..).and_then(|rest| rest.chars().next()) {
        Some(ch) => (ch, ch.len_utf8()),
        None => (char::REPLACEMENT_CHARACTER, 1),
    }
}

// Go: scanner/scanner.go:2474 isShebangTrivia
// PORT: the scanner_util copy is private.
fn is_shebang_trivia(text: &str, pos: usize) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    assert!(
        pos == 0,
        "Shebangs check must only be done at the start of the file"
    );
    bytes[0] == b'#' && bytes[1] == b'!'
}

// Go: scanner/scanner.go:2484 scanShebangTrivia
// PORT: the scanner_util copy is private.
fn scan_shebang_trivia(text: &str, pos: usize) -> usize {
    let mut pos = pos + 2;
    while pos < text.len() {
        let (ch, size) = decode_rune(text, pos);
        if is_line_break(ch) {
            break;
        }
        pos += size;
    }
    pos
}

// Go: scanner/scanner.go:2813 iterateCommentRanges
/*
Returns an iterator over each comment range following the provided position.

Single-line comment ranges include the leading double-slash characters but not the ending
line break. Multi-line comment ranges include the leading slash-asterisk and trailing
asterisk-slash characters.
*/
// PORT: Go yields lazily; this collects the ranges into a Vec.
fn iterate_comment_ranges(text: &str, pos: i32, trailing: bool) -> Vec<CommentRange> {
    let mut out = Vec::new();
    let new_comment_range =
        |kind: SyntaxKind, pos: usize, end: usize, has_trailing_new_line: bool| CommentRange {
            text_range: TextRange::new(pos as i32, end as i32),
            kind,
            has_trailing_new_line,
        };
    if pos < 0 {
        return out;
    }
    let mut pos = pos as usize;
    let bytes = text.as_bytes();
    let mut pending_pos: usize = 0;
    let mut pending_end: usize = 0;
    let mut pending_kind = SyntaxKind::Unknown;
    let mut pending_has_trailing_new_line = false;
    let mut has_pending_comment_range = false;
    let mut collecting = trailing;
    if pos == 0 {
        collecting = true;
        if is_shebang_trivia(text, pos) {
            pos = scan_shebang_trivia(text, pos);
        }
    }
    'scan: while pos < text.len() {
        let (ch, size) = decode_rune(text, pos);
        match ch {
            '\r' | '\n' => {
                if ch == '\r' && pos + 1 < text.len() && bytes[pos + 1] == b'\n' {
                    pos += 1;
                }
                pos += 1;
                if trailing {
                    break 'scan;
                }

                collecting = true;
                if has_pending_comment_range {
                    pending_has_trailing_new_line = true;
                }

                continue;
            }
            '\t' | '\u{000B}' | '\u{000C}' | ' ' => {
                pos += 1;
                continue;
            }
            '/' => {
                let next_char = if pos + 1 < text.len() {
                    bytes[pos + 1]
                } else {
                    0
                };
                let mut has_trailing_new_line = false;
                if next_char == b'/' || next_char == b'*' {
                    let kind = if next_char == b'/' {
                        SyntaxKind::SingleLineCommentTrivia
                    } else {
                        SyntaxKind::MultiLineCommentTrivia
                    };

                    let start_pos = pos;
                    pos += 2;
                    if next_char == b'/' {
                        while pos < text.len() {
                            let (c, s) = decode_rune(text, pos);
                            if is_line_break(c) {
                                has_trailing_new_line = true;
                                break;
                            }
                            pos += s;
                        }
                    } else if let Some(i) = bytes[pos..].windows(2).position(|w| w == b"*/") {
                        pos += i + 2;
                    } else {
                        pos = text.len();
                    }

                    if collecting {
                        if has_pending_comment_range {
                            out.push(new_comment_range(
                                pending_kind,
                                pending_pos,
                                pending_end,
                                pending_has_trailing_new_line,
                            ));
                        }

                        pending_pos = start_pos;
                        pending_end = pos;
                        pending_kind = kind;
                        pending_has_trailing_new_line = has_trailing_new_line;
                        has_pending_comment_range = true;
                    }

                    continue;
                }
                break 'scan;
            }
            _ => {
                if !ch.is_ascii() && is_white_space_like(ch) {
                    if has_pending_comment_range && is_line_break(ch) {
                        pending_has_trailing_new_line = true;
                    }
                    pos += size;
                    continue;
                }
                break 'scan;
            }
        }
    }

    if has_pending_comment_range {
        out.push(new_comment_range(
            pending_kind,
            pending_pos,
            pending_end,
            pending_has_trailing_new_line,
        ));
    }
    out
}
