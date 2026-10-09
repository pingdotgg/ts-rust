//! Port of Go `ls/symbols.go`.
//!
//! PORT: Go builds document symbols as `[]*lsproto.DocumentSymbol`, and
//! `mergeExpandos` shares symbol pointers between merge targets and mutates
//! them in place. The generated `lsproto::DocumentSymbol` holds its children
//! by value, so the tree is built with `DocSymbol` (`Rc<RefCell<>>`, the Go
//! pointer) and copied into lsproto values at the end. Go `Children` (a
//! `*[]*DocumentSymbol`) is never shared between two symbols here:
//! `newDocumentSymbol` and `mergeExpandos` always make a new one, and the
//! `target.Children = source.Children` branch of `mergeChildren` only runs
//! for a nil target list, which `newDocumentSymbol` never makes. So the list
//! is an owned `Option<Vec<DocSymbol>>`.

use crate::ls::prelude::*;

use crate::frontend::parser::ParsedSourceFile;
use crate::frontend::stringutil_ls;
use crate::gostd::unicode;
use crate::spanmap::Feature;

impl LanguageService {
    // Go: ls/symbols.go:26 ProvideDocumentSymbols
    pub fn provide_document_symbols(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
    ) -> Result<lsproto::DocumentSymbolResponse, GoError> {
        let (_, file) = self.get_program_and_file(document_uri);
        let projections = std::iter::once(file)
            .chain(source_file_supplemental_source_files(file).iter().copied());
        let mut symbols: Vec<DocSymbol> = Vec::new();
        // PORT: Go keys `seen` by an anonymous struct {name, kind, rng}; a tuple
        // here.
        let mut seen: FxHashSet<(String, lsproto::SymbolKind, lsproto::Range)> =
            FxHashSet::default();
        for projection in projections {
            for symbol in self.get_document_symbols_for_children(ctx, projection, projection) {
                let key = {
                    let s = symbol.borrow();
                    (s.name.clone(), s.kind, s.range)
                };
                if seen.insert(key) {
                    symbols.push(symbol);
                }
            }
        }
        let symbols = doc_symbols_to_lsp(&symbols);
        if lsproto::get_client_capabilities(ctx)
            .text_document
            .document_symbol
            .hierarchical_document_symbol_support
        {
            return Ok(lsproto::SymbolInformationsOrDocumentSymbolsOrNull {
                document_symbols: Some(symbols),
                ..Default::default()
            });
        }
        // Client doesn't support hierarchical document symbols, return flat SymbolInformation array
        let symbol_infos = flatten_document_symbols(&symbols, document_uri);
        Ok(lsproto::SymbolInformationsOrDocumentSymbolsOrNull {
            symbol_informations: Some(symbol_infos),
            ..Default::default()
        })
    }

    // Go: ls/symbols.go:60 getDocumentSymbolInformations
    // getDocumentSymbolInformations converts hierarchical DocumentSymbols to a flat SymbolInformation array
    pub fn get_document_symbol_informations(
        &self,
        ctx: &Context,
        file: Node,
        document_uri: &lsproto::DocumentUri,
    ) -> Vec<lsproto::SymbolInformation> {
        // First get hierarchical symbols
        let doc_symbols =
            doc_symbols_to_lsp(&self.get_document_symbols_for_children(ctx, file, file));
        flatten_document_symbols(&doc_symbols, document_uri)
    }

    // Go: ls/symbols.go:95 getDocumentSymbolsForChildren
    fn get_document_symbols_for_children(
        &self,
        ctx: &Context,
        node: Node,
        file: Node,
    ) -> Vec<DocSymbol> {
        // PORT: the Go closures (`addSymbolForNode`, `getSymbolsForChildren`,
        // `startNode`, `getSymbolsForNode`, `visit`) and the variables they
        // capture (`symbols`, `expandoTargets`) are `DocumentSymbolsVisitor`.
        let mut visitor = DocumentSymbolsVisitor {
            ls: self,
            ctx,
            file,
            symbols: Vec::new(),
            expando_targets: FxHashSet::default(),
        };
        node.for_each_child(|child| visitor.visit(child));
        merge_expandos(visitor.symbols)
    }
}

// Go: ls/symbols.go:66 flattenDocumentSymbols
pub fn flatten_document_symbols(
    doc_symbols: &[lsproto::DocumentSymbol],
    document_uri: &lsproto::DocumentUri,
) -> Vec<lsproto::SymbolInformation> {
    // Flatten the hierarchy
    // PORT: the recursive Go closure `flatten` is a nested fn that takes
    // the captured `result` and `documentURI`.
    fn flatten(
        result: &mut Vec<lsproto::SymbolInformation>,
        document_uri: &lsproto::DocumentUri,
        symbols: &[lsproto::DocumentSymbol],
        container_name: Option<&String>,
    ) {
        for symbol in symbols {
            let info = lsproto::SymbolInformation {
                name: symbol.name.clone(),
                kind: symbol.kind,
                location: lsproto::Location {
                    uri: document_uri.clone(),
                    range: symbol.range,
                },
                container_name: container_name.cloned(),
                tags: symbol.tags.clone(),
                deprecated: symbol.deprecated,
            };
            result.push(info);

            // Recursively flatten children with this symbol as container
            if let Some(children) = &symbol.children {
                if !children.is_empty() {
                    flatten(result, document_uri, children, Some(&symbol.name));
                }
            }
        }
    }
    let mut result: Vec<lsproto::SymbolInformation> = Vec::new();
    flatten(&mut result, document_uri, doc_symbols, None);
    result
}

// PORT: Go `*lsproto.DocumentSymbol` while the tree is built and merged (see
// the module comment).
type DocSymbol = Rc<RefCell<DocumentSymbolNode>>;

// PORT: the fields of `lsproto::DocumentSymbol`, with shared children.
struct DocumentSymbolNode {
    name: String,
    detail: Option<String>,
    kind: lsproto::SymbolKind,
    tags: Option<Vec<lsproto::SymbolTag>>,
    deprecated: Option<bool>,
    range: lsproto::Range,
    selection_range: lsproto::Range,
    children: Option<Vec<DocSymbol>>,
}

// PORT: copies the finished `DocSymbol` tree into lsproto values (the JSON
// writer reads the Go pointers the same way).
fn doc_symbols_to_lsp(symbols: &[DocSymbol]) -> Vec<lsproto::DocumentSymbol> {
    symbols
        .iter()
        .map(|symbol| {
            let s = symbol.borrow();
            lsproto::DocumentSymbol {
                name: s.name.clone(),
                detail: s.detail.clone(),
                kind: s.kind,
                tags: s.tags.clone(),
                deprecated: s.deprecated,
                range: s.range,
                selection_range: s.selection_range,
                children: s.children.as_ref().map(|c| doc_symbols_to_lsp(c)),
            }
        })
        .collect()
}

// Go: ls/symbols.go:94 (the state of the closures in getDocumentSymbolsForChildren)
struct DocumentSymbolsVisitor<'a> {
    ls: &'a LanguageService,
    ctx: &'a Context,
    file: Node,
    symbols: Vec<DocSymbol>,
    expando_targets: FxHashSet<String>,
}

// PORT: the saved state that the Go `startNode` closure captures for the
// `func()` it returns. `None` is the no-op func for a nil node.
struct EndNode {
    node: Node,
    name: Node,
    save_expando_targets: FxHashSet<String>,
    save_symbols: Vec<DocSymbol>,
}

impl DocumentSymbolsVisitor<'_> {
    // Go: ls/symbols.go:97 addSymbolForNode
    fn add_symbol_for_node(&mut self, node: Node, name: Node, children: Vec<DocSymbol>) {
        if !node.flags().intersects(NodeFlags::REPARSED) {
            let symbol = self.ls.new_document_symbol(node, name, children);
            if let Some(symbol) = symbol {
                self.symbols.push(symbol);
            }
        }
    }

    // Go: ls/symbols.go:106 getSymbolsForChildren
    fn get_symbols_for_children(&mut self, node: Node) -> Vec<DocSymbol> {
        let mut result: Vec<DocSymbol> = Vec::new();
        if node.is_some() {
            let save_expando_targets = std::mem::take(&mut self.expando_targets);
            let save_symbols = std::mem::take(&mut self.symbols);
            node.for_each_child(|child| self.visit(child));
            result = std::mem::replace(&mut self.symbols, save_symbols);
            self.expando_targets = save_expando_targets;
        }
        result
    }

    // Go: ls/symbols.go:120 startNode
    fn start_node(&mut self, node: Node, name: Node) -> Option<EndNode> {
        if node.is_nil() {
            return None;
        }
        let save_expando_targets = std::mem::take(&mut self.expando_targets);
        let save_symbols = std::mem::take(&mut self.symbols);
        Some(EndNode {
            node,
            name,
            save_expando_targets,
            save_symbols,
        })
    }

    // Go: ls/symbols.go:128 (the func returned by startNode)
    fn end_node(&mut self, end: Option<EndNode>) {
        let Some(end) = end else {
            return;
        };
        let result = std::mem::replace(&mut self.symbols, end.save_symbols);
        self.expando_targets = end.save_expando_targets;
        self.add_symbol_for_node(end.node, end.name, result);
    }

    // Go: ls/symbols.go:135 getSymbolsForNode
    fn get_symbols_for_node(&mut self, node: Node) -> Vec<DocSymbol> {
        let mut result: Vec<DocSymbol> = Vec::new();
        if node.is_some() {
            let save_symbols = std::mem::take(&mut self.symbols);
            self.visit(node);
            result = std::mem::replace(&mut self.symbols, save_symbols);
        }
        result
    }

    // Go: ls/symbols.go:146 visit
    fn visit(&mut self, node: Node) -> bool {
        if self.ctx.err().is_some() {
            return true;
        }
        if !node.flags().intersects(NodeFlags::REPARSED) {
            let jsdocs = node.js_doc(self.file);
            if !jsdocs.is_empty() {
                for jsdoc in jsdocs {
                    let tag_list = jsdoc.tags();
                    if tag_list.is_some() {
                        for tag in tag_list.nodes() {
                            if is_js_doc_typedef_tag(tag) || is_js_doc_callback_tag(tag) {
                                self.add_symbol_for_node(
                                    tag,
                                    Node::NIL,  /*name*/
                                    Vec::new(), /*children*/
                                );
                            }
                        }
                    }
                }
            }
        }
        // ts#64160: top-level imports are not document symbols (Go N'
        // symbols.go:164).
        if node.parent().kind() == SyntaxKind::SourceFile
            && is_import_or_import_equals_declaration(node)
        {
            return false;
        }
        match node.kind() {
            SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::EnumDeclaration => {
                if is_class_like(node) && !get_declaration_name(node).is_empty() {
                    self.expando_targets.insert(get_declaration_name(node));
                }
                let children = self.get_symbols_for_children(node);
                self.add_symbol_for_node(node, Node::NIL /*name*/, children);
            }
            SyntaxKind::ModuleDeclaration => {
                let children = self.get_symbols_for_children(get_interior_module(node));
                self.add_symbol_for_node(node, Node::NIL /*name*/, children);
            }
            SyntaxKind::Constructor => {
                let children = self.get_symbols_for_children(node.body());
                self.add_symbol_for_node(node, Node::NIL /*name*/, children);
                for param in node.parameters() {
                    if is_parameter_property_declaration(param, node) {
                        self.add_symbol_for_node(
                            param,
                            Node::NIL,  /*name*/
                            Vec::new(), /*children*/
                        );
                    }
                }
            }
            SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor => {
                let decl_name = get_declaration_name(node);
                if !decl_name.is_empty() {
                    self.expando_targets.insert(decl_name);
                }
                let children = self.get_symbols_for_children(node.body());
                self.add_symbol_for_node(node, Node::NIL /*name*/, children);
            }
            SyntaxKind::VariableDeclaration
            | SyntaxKind::BindingElement
            | SyntaxKind::PropertyAssignment
            | SyntaxKind::PropertyDeclaration => {
                let node_name = node.name();
                if node_name.is_some() {
                    if is_binding_pattern(node_name) {
                        self.visit(node_name);
                    } else {
                        let children = self.get_symbols_for_children(node.initializer());
                        self.add_symbol_for_node(node, Node::NIL /*name*/, children);
                    }
                }
            }
            SyntaxKind::SpreadAssignment => {
                self.add_symbol_for_node(node, node.expression(), Vec::new() /*children*/);
            }
            SyntaxKind::MethodSignature
            | SyntaxKind::PropertySignature
            | SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::IndexSignature
            | SyntaxKind::EnumMember
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ExportSpecifier => {
                self.add_symbol_for_node(
                    node,
                    Node::NIL,  /*name*/
                    Vec::new(), /*children*/
                );
            }
            SyntaxKind::ImportClause => {
                // Handle default import case e.g.:
                //    import d from "mod";
                if node.name().is_some() {
                    self.add_symbol_for_node(
                        node.name(),
                        node.name(),
                        Vec::new(), /*children*/
                    );
                }
                // Handle named bindings in imports e.g.:
                //    import * as NS from "mod";
                //    import {a, b as B} from "mod";
                let named_bindings = node.named_bindings();
                if named_bindings.is_some() {
                    if named_bindings.kind() == SyntaxKind::NamespaceImport {
                        self.add_symbol_for_node(
                            named_bindings,
                            Node::NIL,  /*name*/
                            Vec::new(), /*children*/
                        );
                    } else {
                        for element in named_bindings.elements() {
                            self.add_symbol_for_node(
                                element,
                                Node::NIL,  /*name*/
                                Vec::new(), /*children*/
                            );
                        }
                    }
                }
            }
            SyntaxKind::BinaryExpression | SyntaxKind::CallExpression => {
                let assignment_kind = get_assignment_declaration_kind(node);
                match assignment_kind {
                    // `module.exports = ...`` should be reparsed into a JSExportAssignment,
                    // and `exports.a = ...`` into a CommonJSExport.
                    JSDeclarationKind::NONE
                    | JSDeclarationKind::THIS_PROPERTY
                    | JSDeclarationKind::MODULE_EXPORTS
                    | JSDeclarationKind::EXPORTS_PROPERTY
                    | JSDeclarationKind::OBJECT_DEFINE_PROPERTY_EXPORTS => {
                        node.for_each_child(|child| self.visit(child));
                    }
                    JSDeclarationKind::PROPERTY
                    | JSDeclarationKind::OBJECT_DEFINE_PROPERTY_VALUE => {
                        let target: Node;
                        let mut target_function: Node;
                        let definition: Node;
                        let property_name: Node;
                        // `A.b = ... ` or `A.prototype.b = ...`
                        if is_binary_expression(node) {
                            let binary_expr = node;
                            target = binary_expr.left();
                            target_function = target.expression();
                            definition = binary_expr.right();
                            // `A.b` or `A.prototype.b`
                            if is_property_access_expression(target) {
                                property_name = target.name();
                            } else {
                                // `A["b"]` or `A.prototype["b"]`
                                property_name = target.argument_expression();
                            }
                        } else {
                            // `Object.defineProperty(A, "b", {...})`
                            let args = node.arguments();
                            target_function = args.get(0);
                            target = args.get(1);
                            property_name = target;
                            definition = args.get(2);
                        }
                        if is_prototype_expando(target_function) {
                            target_function = target_function.expression();
                            // If we see a prototype assignment, start tracking the target as an expando target.
                            if is_identifier(target_function) {
                                self.expando_targets
                                    .insert(target_function.text().to_string());
                            }
                        }
                        if is_identifier(target_function)
                            && self.expando_targets.contains(target_function.text())
                        {
                            let end_node = self.start_node(node, target_function);
                            let children = self.get_symbols_for_node(definition);
                            self.add_symbol_for_node(target, property_name, children);
                            self.end_node(end_node);
                        } else {
                            node.for_each_child(|child| self.visit(child));
                        }
                    }
                    _ => {}
                }
            }
            SyntaxKind::ExportAssignment => {
                if node.is_export_equals() {
                    let children = self.get_symbols_for_node(node.expression());
                    self.add_symbol_for_node(node, Node::NIL /*name*/, children);
                } else {
                    node.for_each_child(|child| self.visit(child));
                }
            }
            _ => {
                node.for_each_child(|child| self.visit(child));
            }
        }
        false
    }
}

// Go: ls/symbols.go:286 isPrototypeExpando
// Target is `f.prototype`.
pub fn is_prototype_expando(target: Node) -> bool {
    if is_access_expression(target) {
        let access_name = get_element_or_property_access_name(target);
        return access_name.is_some() && access_name.text() == "prototype";
    }
    false
}

// Go: ls/symbols.go:294 maxLength
const MAX_LENGTH: i32 = 150;

impl LanguageService {
    // Go: ls/symbols.go:296 newDocumentSymbol
    // PORT: Go returns a nil `*lsproto.DocumentSymbol` as `None`.
    fn new_document_symbol(
        &self,
        node: Node,
        name: Node,
        children: Vec<DocSymbol>,
    ) -> Option<DocSymbol> {
        let mut name = name;
        let file = get_source_file_of_node(node);
        let node_start_pos = skip_trivia(&source_file_text(file), node.pos());
        if name.is_nil() {
            name = get_name_of_declaration(node);
        }
        let mut text: String;
        let name_start_pos: i32;
        let name_end_pos: i32;
        if is_module_declaration(node) && !is_ambient_module(node) {
            text = get_module_name(node);
            name_start_pos = skip_trivia(&source_file_text(file), name.pos());
            name_end_pos = get_interior_module(node).name().end();
        } else if is_any_export_assignment(node) && node.is_export_equals() {
            text = "export=".to_string();
            if !node_is_missing(name) {
                name_start_pos = skip_trivia(&source_file_text(file), name.pos());
                name_end_pos = name.end();
            } else {
                name_start_pos = node_start_pos;
                name_end_pos = node.end();
            }
        } else if name.is_some() {
            text = get_text_of_name(name);
            name_start_pos = i32::max(
                skip_trivia(&source_file_text(file), name.pos()),
                node_start_pos,
            );
            name_end_pos = i32::max(name.end(), node_start_pos);
        } else {
            text = get_unnamed_node_label(node);
            name_start_pos = node_start_pos;
            name_end_pos = node_start_pos;
        }
        if text.is_empty() {
            return None;
        }
        let truncated_text = stringutil_ls::truncate_by_runes(&text, MAX_LENGTH);
        if truncated_text.len() < text.len() {
            text = truncated_text + "...";
        }
        let (selection_range, selection_fidelity) = self.converters.to_lsp_range_for_feature(
            &file,
            TextRange::new(name_start_pos, name_end_pos),
            Feature::DOCUMENT_SYMBOLS,
        );
        if !selection_fidelity.is_single_segment() {
            return None;
        }
        let (mut range, range_fidelity) = self.converters.to_lsp_range_for_feature(
            &file,
            TextRange::new(node_start_pos, node.end()),
            Feature::DOCUMENT_SYMBOLS,
        );
        if range_fidelity.is_none() {
            range = selection_range;
        }
        // PORT: Go turns a nil `children` into an empty slice; an empty `Vec`
        // is both here.
        Some(Rc::new(RefCell::new(DocumentSymbolNode {
            name: text,
            detail: None,
            kind: get_symbol_kind_from_node(node),
            tags: None,
            deprecated: None,
            range,
            selection_range,
            children: Some(children),
        })))
    }
}

// Go: ls/symbols.go:355 mergeExpandos
// Merges expando symbols into their target symbols, and namespaces of same name.
// Modifies the input slice.
// PORT: Go sets merged entries of the input slice to nil; here the input is
// moved into a `Vec<Option<DocSymbol>>`.
fn merge_expandos(symbols: Vec<DocSymbol>) -> Vec<DocSymbol> {
    let mut merged_symbols: Vec<DocSymbol> = Vec::with_capacity(symbols.len());
    let mut symbols: Vec<Option<DocSymbol>> = symbols.into_iter().map(Some).collect();
    // Collect symbols that can be an expando target.
    // PORT: Go `collections.MultiMap[string, int]`.
    let mut name_to_expando_target_index: IndexMap<String, Vec<i32>> = IndexMap::default();
    // Collect namespaces.
    let mut name_to_namespace_index: FxHashMap<String, i32> = FxHashMap::default();
    for (i, symbol) in symbols.iter().enumerate() {
        let symbol = symbol
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        let symbol = symbol.borrow();
        if is_anonymous_name(&symbol.name) {
            continue;
        }
        if symbol.kind == lsproto::SymbolKind::CLASS
            || symbol.kind == lsproto::SymbolKind::FUNCTION
            || symbol.kind == lsproto::SymbolKind::VARIABLE
        {
            name_to_expando_target_index
                .entry(symbol.name.clone())
                .or_default()
                .push(i as i32);
        }
        if symbol.kind == lsproto::SymbolKind::NAMESPACE {
            name_to_namespace_index
                .entry(symbol.name.clone())
                .or_insert(i as i32);
        }
    }
    for i in 0..symbols.len() {
        // PORT: Go reads `symbols[i]` at the start of iteration i; only
        // iteration i sets it to nil, so it is never nil here.
        let symbol = symbols[i]
            .clone()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        let children = symbol.borrow_mut().children.take();
        if let Some(children) = children {
            let children = merge_expandos(children);
            symbol.borrow_mut().children = Some(children);
        }

        let (symbol_name, symbol_kind) = {
            let s = symbol.borrow();
            (s.name.clone(), s.kind)
        };

        // Anonymous symbols never merge.
        if is_anonymous_name(&symbol_name) {
            continue;
        }

        // Merge expandos.
        if symbol_kind == lsproto::SymbolKind::PROPERTY {
            let symbols_with_same_name: Vec<i32> = name_to_expando_target_index
                .get(&symbol_name)
                .cloned()
                .unwrap_or_default();
            for j in (0..symbols_with_same_name.len()).rev() {
                let target_index = symbols_with_same_name[j];
                let target_symbol = symbols[target_index as usize]
                    .clone()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                merge_children(&target_symbol, &symbol);
                // Mark this symbol as merged.
                symbols[i] = None;
            }
        }
        // Merge namespaces.
        if symbol_kind == lsproto::SymbolKind::NAMESPACE {
            if let Some(&target_index) = name_to_namespace_index.get(&symbol_name) {
                if target_index != i as i32 {
                    let target_symbol = symbols[target_index as usize]
                        .clone()
                        .unwrap_or_else(|| crate::core::go_nil_dereference());
                    merge_children(&target_symbol, &symbol);
                    // Mark this symbol as merged.
                    symbols[i] = None;
                }
            }
        }
    }
    for symbol in symbols.into_iter().flatten() {
        merged_symbols.push(symbol);
    }
    merged_symbols
}

// Go: ls/symbols.go:414 mergeChildren
fn merge_children(target: &DocSymbol, source: &DocSymbol) {
    // PORT: copy the child lists out so no `RefCell` borrow is held while
    // `merge_expandos` borrows the children.
    let source_children = source.borrow().children.clone();
    if let Some(source_children) = source_children {
        let target_children = target.borrow_mut().children.take();
        match target_children {
            None => {
                target.borrow_mut().children = Some(source_children);
            }
            Some(mut target_children) => {
                target_children.extend(source_children);
                let mut merged = merge_expandos(target_children);
                gostd::slices::sort_func(&mut merged, |a, b| {
                    lsproto::compare_ranges(a.borrow().range, b.borrow().range)
                });
                target.borrow_mut().children = Some(merged);
            }
        }
    }
}

// Go: ls/symbols.go:428 isAnonymousName
// See `getUnnamedNodeLabel`.
pub fn is_anonymous_name(name: &str) -> bool {
    name == "<function>"
        || name == "<class>"
        || name == "export="
        || name == "default"
        || name == "constructor"
        || name == "()"
        || name == "new()"
        || name == "[]"
        || name.ends_with(") callback")
}

// Go: ls/symbols.go:433 getTextOfName
pub fn get_text_of_name(node: Node) -> String {
    match node.kind() {
        SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier | SyntaxKind::NumericLiteral => {
            return node.text().to_string();
        }
        SyntaxKind::StringLiteral => {
            return "\"".to_string() + &escape_string(node.text(), QuoteChar::DOUBLE_QUOTE) + "\"";
        }
        SyntaxKind::NoSubstitutionTemplateLiteral => {
            return "`".to_string() + &escape_string(node.text(), QuoteChar::BACKTICK) + "`";
        }
        SyntaxKind::ComputedPropertyName => {
            if is_string_or_numeric_literal_like(node.expression()) {
                return get_text_of_name(node.expression());
            }
        }
        _ => {}
    }
    get_text_of_node(node)
}

// Go: ls/symbols.go:449 getUnnamedNodeLabel
pub fn get_unnamed_node_label(node: Node) -> String {
    let parent = walk_up_parenthesized_expressions(node.parent());
    if parent.is_some() && is_export_assignment(parent) {
        if parent.is_export_equals() {
            return "export=".to_string();
        }
        return "default".to_string();
    }
    match node.kind() {
        SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction => {
            if node.modifier_flags().intersects(ModifierFlags::DEFAULT) {
                return "default".to_string();
            }
            if is_call_expression(node.parent()) {
                let mut name = get_call_expression_name(node.parent().expression());
                if !name.is_empty() {
                    name = clean_callback_text(&name);
                    if name.len() as i32 > MAX_LENGTH {
                        return name + " callback";
                    }
                    let args =
                        clean_callback_text(&get_call_expression_literal_args(node.parent()));
                    return name + "(" + &args + ") callback";
                }
            }
            "<function>".to_string()
        }
        SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => {
            if node.modifier_flags().intersects(ModifierFlags::DEFAULT) {
                return "default".to_string();
            }
            "<class>".to_string()
        }
        SyntaxKind::Constructor => "constructor".to_string(),
        SyntaxKind::CallSignature => "()".to_string(),
        SyntaxKind::ConstructSignature => "new()".to_string(),
        SyntaxKind::IndexSignature => "[]".to_string(),
        _ => String::new(),
    }
}

// Go: ls/symbols.go:490 getCallExpressionName
pub fn get_call_expression_name(node: Node) -> String {
    match node.kind() {
        SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier => {
            return node.text().to_string();
        }
        SyntaxKind::PropertyAccessExpression => {
            let left = get_call_expression_name(node.expression());
            let right = get_call_expression_name(node.name());
            if !left.is_empty() {
                return left + "." + &right;
            }
            return right;
        }
        _ => {}
    }
    String::new()
}

// Go: ls/symbols.go:505 getCallExpressionLiteralArgs
pub fn get_call_expression_literal_args(call_expr: Node) -> String {
    let mut parts: Vec<String> = Vec::new();
    for arg in call_expr.arguments() {
        if is_string_literal_like(arg) || is_template_expression(arg) {
            parts.push(get_text_of_node(arg));
        }
    }
    parts.join(", ")
}

// Go: ls/symbols.go:515 cleanCallbackText
pub fn clean_callback_text(text: &str) -> String {
    let mut text = text.to_string();
    let truncated = stringutil_ls::truncate_by_runes(&text, MAX_LENGTH);
    if truncated.len() < text.len() {
        text = truncated + "...";
    }
    // PORT: Go `strings.Map` drops the runes for which the func returns -1.
    text.chars().filter(|&r| !is_line_break(r)).collect()
}

// Go: ls/symbols.go:528 getInteriorModule
pub fn get_interior_module(node: Node) -> Node {
    let mut node = node;
    while node.body().is_some() && is_module_declaration(node.body()) {
        node = node.body();
    }
    node
}

// Go: ls/symbols.go:535 getModuleName
pub fn get_module_name(node: Node) -> String {
    let mut node = node;
    let mut result = node.name().text().to_string();
    while node.body().is_some() && is_module_declaration(node.body()) {
        node = node.body();
        result = result + "." + node.name().text();
    }
    result
}

// Go: ls/symbols.go:544 DeclarationInfo
#[derive(Clone, Debug)]
pub struct DeclarationInfo {
    pub name: String,
    pub declaration: Node,
    pub match_score: i32,
}

// Go: ls/symbols.go:550 ProvideWorkspaceSymbols
// PORT: Go `[]*compiler.Program` is `&[Rc<compiler::NewProgram>]`, Go
// `*lsconv.Converters` is `&lsconv::Converters`, and the preferences are
// passed by reference.
pub fn provide_workspace_symbols(
    ctx: &Context,
    programs: &[Rc<compiler::NewProgram>],
    converters: &lsconv::Converters,
    preferences: &lsutil::UserPreferences,
    query: &str,
) -> Result<lsproto::WorkspaceSymbolResponse, GoError> {
    let exclude_library_symbols = preferences.exclude_library_symbols_in_nav_to.is_true();
    // Obtain set of non-declaration source files from all active programs.
    // PORT: Go iterates this map in random order; IndexMap keeps insertion
    // order. The infos are sorted below, so only ties can differ from Go.
    // The value is the Go `*ast.SourceFile`, the root node of the parsed file.
    let mut source_files: IndexMap<tspath::Path, Node> = IndexMap::default();
    for program in programs {
        for source_file in program.source_files() {
            if (program.has_ts_file() || !source_file.is_declaration_file)
                && !should_exclude_file(source_file, program, exclude_library_symbols)
            {
                source_files.insert(source_file.path().clone(), source_file.root);
            }
        }
    }
    // Create DeclarationInfos for all declarations in the source files.
    let mut infos: Vec<DeclarationInfo> = Vec::new();
    for (_, &source_file) in &source_files {
        if ctx.err().is_some() {
            return Ok(lsproto::SymbolInformationsOrWorkspaceSymbolsOrNull::default());
        }
        // PORT: Go iterates the declaration map in random order; the infos
        // are sorted below.
        let declaration_map = &*source_file_get_declaration_map(source_file);
        for (name, declarations) in declaration_map {
            let score = get_match_score(name, query);
            if score >= 0 {
                for &declaration in declarations {
                    infos.push(DeclarationInfo {
                        name: name.clone(),
                        declaration,
                        match_score: score,
                    });
                }
            }
        }
    }
    // Sort the DeclarationInfos and return the top 256 matches.
    gostd::slices::sort_func(&mut infos, compare_declaration_infos);
    let count = infos.len().min(256);
    let mut symbols: Vec<lsproto::SymbolInformation> = Vec::with_capacity(count);
    for info in &infos[0..count] {
        let node = info.declaration;
        let source_file = get_source_file_of_node(node);
        let container = get_container_node(info.declaration);
        let mut container_name: Option<String> = None;
        if container.is_some() {
            container_name = str_ptr_to(&get_declaration_name(container));
        }
        // Use the name node's span so that VS selects just the symbol name (matching
        // the TS5 navto behaviour). GetNameOfDeclaration is always non-nil here because
        // computeDeclarationMap only adds declarations whose GetDeclarationName (string
        // form) is non-empty, which implies a name node exists.
        let name_node = get_name_of_declaration(node);
        let name_start =
            astnav::get_start_of_node(name_node, source_file, false /*includeJsDoc*/);
        let name_range = TextRange::new(name_start, name_node.end());
        let (location, fidelity) = converters.to_lsp_location_for_feature(
            &source_file,
            name_range,
            Feature::DOCUMENT_SYMBOLS,
        );
        if !fidelity.is_single_segment() {
            // The name has no counterpart in the original text, so there is nothing to navigate to.
            continue;
        }
        let mut symbol = lsproto::SymbolInformation::default();
        symbol.name = info.name.clone();
        symbol.kind = get_symbol_kind_from_node(info.declaration);
        symbol.location = location;
        symbol.container_name = container_name;
        symbols.push(symbol);
    }

    Ok(lsproto::SymbolInformationsOrWorkspaceSymbolsOrNull {
        symbol_informations: Some(symbols),
        ..Default::default()
    })
}

// Go: ls/symbols.go:619 shouldExcludeFile
// PORT: Go takes the `*ast.SourceFile`; `NewProgram::is_lib_file` takes the
// parsed file, so this does too.
pub fn should_exclude_file(
    file: &ParsedSourceFile,
    program: &compiler::NewProgram,
    exclude_library_symbols: bool,
) -> bool {
    exclude_library_symbols
        && (is_inside_node_modules(file.file_name()) || program.is_lib_file(file))
}

// Go: ls/symbols.go:623 isInsideNodeModules
pub fn is_inside_node_modules(file_name: &str) -> bool {
    file_name.contains("/node_modules/")
}

// Go: ls/symbols.go:632 getMatchScore
// Return a score for matching `s` against `pattern`. In order to match, `s` must contain each of the characters in
// `pattern` in the same order. Upper case characters in `pattern` must match exactly, whereas lower case characters
// in `pattern` match either case in `s`. If `s` doesn't match, -1 is returned. Otherwise, the returned score is the
// number of characters in `s` that weren't matched. Thus, zero represents an exact match, and higher values represent
// increasingly less specific partial matches.
// PORT: Rust strings are valid UTF-8, so Go's `RuneError` for invalid bytes
// does not occur.
pub fn get_match_score(s: &str, pattern: &str) -> i32 {
    let mut s = s;
    let mut score = 0;
    for p in pattern.chars() {
        let exact = unicode_is_upper(p);
        loop {
            let Some(c) = s.chars().next() else {
                return -1;
            };
            s = &s[c.len_utf8()..];
            if exact && c == p || !exact && unicode::to_lower(c) == unicode::to_lower(p) {
                break;
            }
            score += 1;
        }
    }
    score
}

/// Go `unicode.IsUpper`: general category Lu.
// PORT: the Rust `Uppercase` property is Lu plus `Other_Uppercase`; the
// `Other_Uppercase` ranges are removed so the result is Go's category test
// (the same helper as in `ls/autoimport/util.rs`).
fn unicode_is_upper(c: char) -> bool {
    if !c.is_uppercase() {
        return false;
    }
    !matches!(
        c as u32,
        0x2160..=0x216F | 0x24B6..=0x24CF | 0x1F130..=0x1F149 | 0x1F150..=0x1F169 | 0x1F170..=0x1F189
    )
}

// Go: ls/symbols.go:653 compareDeclarationInfos
// Sort DeclarationInfos by ascending match score, then ascending case insensitive name, then
// ascending case sensitive name, and finally by source file name and position.
pub fn compare_declaration_infos(d1: &DeclarationInfo, d2: &DeclarationInfo) -> i32 {
    if d1.match_score != d2.match_score {
        return d1.match_score - d2.match_score;
    }
    let c = stringutil_ls::compare_strings_case_insensitive(&d1.name, &d2.name);
    if c != 0 {
        return c;
    }
    let c = d1.name.as_str().cmp(d2.name.as_str()) as i32;
    if c != 0 {
        return c;
    }
    let s1 = get_source_file_of_node(d1.declaration);
    let s2 = get_source_file_of_node(d2.declaration);
    if s1 != s2 {
        return source_file_info(s1)
            .path
            .as_str()
            .cmp(source_file_info(s2).path.as_str()) as i32;
    }
    d1.declaration.pos() - d2.declaration.pos()
}

// Go: ls/symbols.go:673 getSymbolKindFromNode
// getSymbolKindFromNode converts an AST node to an LSP SymbolKind.
// Combines getNodeKind with VS Code's fromProtocolScriptElementKind.
pub fn get_symbol_kind_from_node(node: Node) -> lsproto::SymbolKind {
    match node.kind() {
        SyntaxKind::SourceFile => {
            if is_external_module(node) {
                return lsproto::SymbolKind::MODULE;
            }
            return lsproto::SymbolKind::FILE;
        }
        SyntaxKind::ModuleDeclaration => return lsproto::SymbolKind::NAMESPACE,
        SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => {
            return lsproto::SymbolKind::CLASS;
        }
        SyntaxKind::InterfaceDeclaration => return lsproto::SymbolKind::INTERFACE,
        SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsDocTypedefTag
        | SyntaxKind::JsDocCallbackTag => return lsproto::SymbolKind::CLASS,
        SyntaxKind::EnumDeclaration => return lsproto::SymbolKind::ENUM,
        SyntaxKind::VariableDeclaration => return lsproto::SymbolKind::VARIABLE,
        SyntaxKind::ArrowFunction
        | SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression => return lsproto::SymbolKind::FUNCTION,
        SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => {
            return lsproto::SymbolKind::PROPERTY;
        }
        SyntaxKind::MethodDeclaration | SyntaxKind::MethodSignature => {
            return lsproto::SymbolKind::METHOD;
        }
        SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::PropertyAssignment
        | SyntaxKind::ShorthandPropertyAssignment
        | SyntaxKind::SpreadAssignment
        | SyntaxKind::IndexSignature => return lsproto::SymbolKind::PROPERTY,
        SyntaxKind::CallSignature => return lsproto::SymbolKind::METHOD,
        SyntaxKind::ConstructSignature => return lsproto::SymbolKind::CONSTRUCTOR,
        SyntaxKind::Constructor | SyntaxKind::ClassStaticBlockDeclaration => {
            return lsproto::SymbolKind::CONSTRUCTOR;
        }
        SyntaxKind::TypeParameter => return lsproto::SymbolKind::TYPE_PARAMETER,
        SyntaxKind::EnumMember => return lsproto::SymbolKind::ENUM_MEMBER,
        SyntaxKind::Parameter => {
            if has_syntactic_modifier(node, ModifierFlags::PARAMETER_PROPERTY_MODIFIER) {
                return lsproto::SymbolKind::PROPERTY;
            }
            return lsproto::SymbolKind::VARIABLE;
        }
        SyntaxKind::BinaryExpression | SyntaxKind::CallExpression => {
            let kind = get_assignment_declaration_kind(node);
            match kind {
                JSDeclarationKind::THIS_PROPERTY
                | JSDeclarationKind::PROPERTY
                | JSDeclarationKind::OBJECT_DEFINE_PROPERTY_VALUE => {
                    return lsproto::SymbolKind::PROPERTY;
                }
                _ => {}
            }
        }
        SyntaxKind::StringLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::NumericLiteral => {
            // String literals used as property names (e.g., in Object.defineProperty)
            return lsproto::SymbolKind::PROPERTY;
        }
        _ => {}
    }
    lsproto::SymbolKind::VARIABLE
}

#[cfg(test)]
mod match_score_tests {
    use super::get_match_score;

    // Go `getMatchScore` (ls/symbols.go:628) lowers both runes with
    // `unicode.ToLower`. At pin N that is go1.27.1 (Unicode 17.0.0), where
    // U+A7CB lowers to U+0264, so the lower case pattern matches the capital
    // in a string-named declaration. An upper case pattern rune must match
    // exactly.
    #[test]
    fn match_score_uses_unicode_17_case() {
        assert_eq!(get_match_score("\u{A7CB}ab", "\u{264}ab"), 0);
        assert_eq!(get_match_score("x\u{10D50}", "\u{10D70}"), 1);
        assert_eq!(get_match_score("\u{264}ab", "\u{A7CB}"), -1);
    }
}
