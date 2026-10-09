//! Port of Go `ls/findallreferences.go`, lines 1-1245: the reference types,
//! entry points and definitions. The search half (Go lines 1246-2654) is
//! `findallreferences_p2.rs`.
//!
//! PORT notes for the whole file:
//! - Go `*ReferenceEntry` and `*SymbolAndEntries` are shared and changed in
//!   place (`resolveEntry` fills the ranges, the search appends references),
//!   so they are `Rc<RefCell<..>>`.
//! - Go reads `*ast.Symbol` fields without a checker. Symbol data lives in a
//!   checker arena here. Free functions that read a symbol take
//!   `symbols: &SymbolArena` first, as the `ast` helpers do. Language service
//!   methods where Go reads a symbol without a checker get the request
//!   checker (`ls_program::get_type_checker`) for the read. The pool returns
//!   the checker of the request, which made the symbols.
//! - `ls_program::get_type_checker` returns the same `Rc<RefCell<Checker>>`
//!   for nested calls in one request (Go gets the same checker again). A
//!   `borrow_mut` is held only where no nested call can borrow it again.
//! - The search methods are generic over the program (`ProgramView`, see
//!   `program_view.rs`), so that a cross-project request can run them on a
//!   search thread. They get the checker with `ProgramView::get_type_checker`,
//!   which is `ls_program::get_type_checker` on the dispatch thread.

use crate::ls::prelude::*;

use crate::astnav;
use crate::flags_macros::go_enum;
use crate::frontend::tspath;
use crate::gostd::{Context, GoError};
use crate::ls::lsconv;
use crate::lsp::lsproto;
use crate::lsp::lsproto::{HasTextDocumentPosition, HasTextDocumentURI};
use crate::program::ls_program;
use crate::spanmap::Feature;
use std::cell::OnceCell;
use std::collections::VecDeque;

// === types for settings ===
// Go: ls/findallreferences.go:29 referenceUse
go_enum!(ReferenceUse, i32 {
    NONE = 0;
    OTHER = 1;
    REFERENCES = 2;
    RENAME = 3;
});

// Go: ls/findallreferences.go:38 refOptions
// PORT: Go field `use` is a Rust keyword: `use_`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefOptions {
    pub find_in_strings: bool,
    pub find_in_comments: bool,
    pub use_: ReferenceUse, // other, references, rename
    pub implementations: bool,
    pub use_aliases_for_rename: bool, // renamed from providePrefixAndSuffixTextForRename. default: true
}

// === types for results ===

// Go: ls/findallreferences.go:48 refInfo
// PORT: Go `*ast.SourceFile` is the file root `Node`; Go
// `*ast.FileReference` (nil in the module specifier case) is an `Option`.
#[derive(Clone, Debug, Default)]
pub struct RefInfo {
    pub file: Node,
    pub file_name: String,
    pub reference: Option<FileReference>,
    pub unverified: bool,
}

// Go: ls/findallreferences.go:55 SymbolAndEntries
// PORT: Go `*Definition` is never changed after it is made, so the
// definition is held by value (`Option` for nil).
#[derive(Clone, Debug, Default)]
pub struct SymbolAndEntries {
    pub definition: Option<Definition>,
    pub references: Vec<Rc<RefCell<ReferenceEntry>>>,
}

// Go: ls/findallreferences.go:60 NewSymbolAndEntries
pub fn new_symbol_and_entries(
    kind: DefinitionKind,
    node: Node,
    symbol: SymbolId,
    references: Vec<Rc<RefCell<ReferenceEntry>>>,
) -> Rc<RefCell<SymbolAndEntries>> {
    Rc::new(RefCell::new(SymbolAndEntries {
        definition: Some(Definition {
            kind,
            node,
            symbol,
            triple_slash_file_ref: None,
        }),
        references,
    }))
}

// Go: ls/findallreferences.go:71 DefinitionKind
go_enum!(DefinitionKind, i32 {
    SYMBOL = 0;
    LABEL = 1;
    KEYWORD = 2;
    THIS = 3;
    STRING = 4;
    TRIPLE_SLASH_REFERENCE = 5;
});

// Go: ls/findallreferences.go:82 Definition
#[derive(Clone, Debug, Default)]
pub struct Definition {
    pub kind: DefinitionKind,
    pub symbol: SymbolId,
    pub node: Node,
    pub triple_slash_file_ref: Option<TripleSlashDefinition>,
}

// Go: ls/findallreferences.go:88 tripleSlashDefinition
// PORT: Go `reference *ast.FileReference` can be nil (a module specifier
// reference from `getReferenceAtPosition`), so it is an `Option`.
#[derive(Clone, Debug, Default)]
pub struct TripleSlashDefinition {
    pub reference: Option<FileReference>,
    pub file: Node,
}

// Go: ls/findallreferences.go:93 entryKind
go_enum!(EntryKind, i32 {
    NONE = 0;
    RANGE = 1;
    NODE = 2;
    STRING_LITERAL = 3;
    SEARCHED_LOCAL_FOUND_PROPERTY = 4;
    SEARCHED_PROPERTY_FOUND_LOCAL = 5;
});

// Go: ls/findallreferences.go:104 ReferenceEntry
// PORT: Go `*core.TextRange` and `*lsproto.Location` are `Option`s. Go
// `*ast.SourceFile` is the file root `Node` (nil is `Node::NIL`).
#[derive(Clone, Debug, Default)]
pub struct ReferenceEntry {
    pub kind: EntryKind,
    pub node: Node,
    pub context: Node, // !!! ContextWithStartAndEndNode, optional
    pub source_file: Node,
    pub text_range: Option<TextRange>,
    pub lsp_range: Option<lsproto::Location>,
    pub unmappable: bool,
}

impl ReferenceEntry {
    // Go: ls/findallreferences.go:115 Node
    // Node returns the AST node for this reference entry.
    pub fn node(&self) -> Node {
        self.node
    }

    // Go: ls/findallreferences.go:120 IsNodeEntry
    // IsNodeEntry returns true if this is a node-backed reference entry.
    pub fn is_node_entry(&self) -> bool {
        self.node.is_some()
    }
}

impl SymbolAndEntries {
    // Go: ls/findallreferences.go:125 References
    // References returns the reference entries for this symbol.
    pub fn references(&self) -> &[Rc<RefCell<ReferenceEntry>>] {
        &self.references
    }

    // Go: ls/findallreferences.go:130 DefinitionNode
    // DefinitionNode returns the defining AST node for this symbol, if any.
    // PORT: reading `symbol.Declarations` takes the symbol arena.
    pub fn definition_node(&self, symbols: &SymbolArena) -> Node {
        let Some(definition) = &self.definition else {
            return Node::NIL;
        };
        if definition.node.is_some() {
            return definition.node;
        }
        if definition.symbol.is_some() && !symbols.sym(definition.symbol).declarations.is_empty() {
            return symbols.sym(definition.symbol).declarations[0];
        }
        Node::NIL
    }

    // Go: ls/findallreferences.go:143 DefinitionSymbol
    pub fn definition_symbol(&self) -> SymbolId {
        let Some(definition) = &self.definition else {
            return SymbolId::NIL;
        };
        definition.symbol
    }

    // Go: ls/findallreferences.go:150 canUseDefinitionSymbol
    pub fn can_use_definition_symbol(&self) -> bool {
        let Some(definition) = &self.definition else {
            return false;
        };

        match definition.kind {
            DefinitionKind::SYMBOL | DefinitionKind::THIS => definition.symbol.is_some(),
            DefinitionKind::TRIPLE_SLASH_REFERENCE => {
                // !!! TODO : need to find file reference instead?
                // May need to return true to indicate this to be file search instead and might need to do for import stuff as well
                // For now
                false
            }
            _ => false,
        }
    }
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/findallreferences.go:168 getRangeOfEntry
    pub fn get_range_of_entry(&self, entry: &Rc<RefCell<ReferenceEntry>>) -> lsproto::Range {
        self.resolve_entry(entry)
            .borrow()
            .lsp_range
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .range
    }

    // Go: ls/findallreferences.go:172 getRangeOfEntryForFeature
    pub fn get_range_of_entry_for_feature(
        &self,
        entry: &Rc<RefCell<ReferenceEntry>>,
        feature: Feature,
    ) -> (lsproto::Range, bool) {
        let (location, ok) = self.get_location_of_entry_for_feature(entry, feature);
        (location.range, ok)
    }

    // Go: ls/findallreferences.go:177 getFileNameOfEntry
    pub fn get_file_name_of_entry(
        &self,
        entry: &Rc<RefCell<ReferenceEntry>>,
    ) -> lsproto::DocumentUri {
        self.resolve_entry(entry)
            .borrow()
            .lsp_range
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .uri
            .clone()
    }

    // Go: ls/findallreferences.go:181 getLocationOfEntryForFeature
    pub fn get_location_of_entry_for_feature(
        &self,
        entry: &Rc<RefCell<ReferenceEntry>>,
        feature: Feature,
    ) -> (lsproto::Location, bool) {
        self.resolve_entry_source(entry);
        let (source_file, text_range) = {
            let e = entry.borrow();
            (
                e.source_file,
                e.text_range
                    .unwrap_or_else(|| crate::core::go_nil_dereference()),
            )
        };
        let (location, fidelity) =
            self.source_file_range_to_lsp_location_for_feature(source_file, text_range, feature);
        (location, fidelity.is_single_segment())
    }

    // Go: ls/findallreferences.go:187 resolveEntrySource
    pub fn resolve_entry_source(&self, entry: &Rc<RefCell<ReferenceEntry>>) {
        let mut e = entry.borrow_mut();
        if e.source_file.is_nil() {
            crate::go_assert!(
                e.node.is_some(),
                "reference entry must have a node or source file"
            );
            e.source_file = get_source_file_of_node(e.node);
        }
        if e.text_range.is_none() {
            let text_range = get_range_of_node(e.node, e.source_file, Node::NIL /*endNode*/);
            e.text_range = Some(text_range);
        }
    }

    // Go: ls/findallreferences.go:198 resolveEntry
    // PORT: Go returns the same pointer; here a clone of the same handle.
    pub fn resolve_entry(
        &self,
        entry: &Rc<RefCell<ReferenceEntry>>,
    ) -> Rc<RefCell<ReferenceEntry>> {
        self.resolve_entry_source(entry);
        {
            let mut e = entry.borrow_mut();
            if e.lsp_range.is_none() {
                let text_range = e
                    .text_range
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                let (location, fidelity) =
                    self.source_file_range_to_lsp_location(e.source_file, text_range);
                e.lsp_range = Some(location);
                e.unmappable = !fidelity.is_single_segment();
            }
        }
        Rc::clone(entry)
    }
}

// Go: ls/findallreferences.go:208 newNodeEntryWithKind
pub fn new_node_entry_with_kind(node: Node, kind: EntryKind) -> Rc<RefCell<ReferenceEntry>> {
    let e = new_node_entry(node);
    e.borrow_mut().kind = kind;
    e
}

// Go: ls/findallreferences.go:214 newNodeEntry
pub fn new_node_entry(node: Node) -> Rc<RefCell<ReferenceEntry>> {
    // creates nodeEntry with `kind == entryKindNode`
    let name = node.name();
    Rc::new(RefCell::new(ReferenceEntry {
        kind: EntryKind::NODE,
        node: if name.is_some() { name } else { node },
        context: get_context_node_for_node_entry(node),
        ..Default::default()
    }))
}

// Go: ls/findallreferences.go:223 getContextNodeForNodeEntry
pub fn get_context_node_for_node_entry(node: Node) -> Node {
    if is_declaration(node) {
        return get_context_node(node);
    }

    if node.parent().is_nil() {
        return Node::NIL;
    }

    if !is_declaration(node.parent()) && !is_export_assignment(node.parent()) {
        // Special property assignment in javascript
        if is_in_js_file(node) {
            // !!! jsdoc: check if branch still needed
            let mut binary_expression = Node::NIL;
            if is_binary_expression(node.parent()) {
                binary_expression = node.parent();
            } else if is_access_expression(node.parent())
                && is_binary_expression(node.parent().parent())
                && node.parent().parent().left() == node.parent()
            {
                binary_expression = node.parent().parent();
            }
            if binary_expression.is_some()
                && get_assignment_declaration_kind(binary_expression) != JSDeclarationKind::NONE
            {
                return get_context_node(binary_expression);
            }
        }

        // Jsx Tags
        match node.parent().kind() {
            SyntaxKind::JsxOpeningElement | SyntaxKind::JsxClosingElement => {
                return node.parent().parent();
            }
            SyntaxKind::JsxSelfClosingElement
            | SyntaxKind::LabeledStatement
            | SyntaxKind::BreakStatement
            | SyntaxKind::ContinueStatement => {
                return node.parent();
            }
            SyntaxKind::StringLiteral | SyntaxKind::NoSubstitutionTemplateLiteral => {
                let valid_import = try_get_import_from_module_specifier(node);
                if valid_import.is_some() {
                    // PORT: the Go callback ignores its argument and tests
                    // `node`; kept as in Go.
                    let decl_or_statement = find_ancestor(valid_import, |_| {
                        is_declaration(node) || is_statement(node) || is_js_doc_tag(node)
                    });
                    if is_declaration(decl_or_statement) {
                        return get_context_node(decl_or_statement);
                    }
                    return decl_or_statement;
                }
            }
            _ => {}
        }

        // Handle computed property name
        let property_name = find_ancestor(node, is_computed_property_name);
        if property_name.is_some() {
            return get_context_node(property_name.parent());
        }
        return Node::NIL;
    }

    if node.parent().name() == node // node is name of declaration, use parent
        || node.parent().kind() == SyntaxKind::Constructor
        || node.parent().kind() == SyntaxKind::ExportAssignment
        // Property name of the import export specifier or binding pattern, use parent
        || ((is_import_or_export_specifier(node.parent())
            || node.parent().kind() == SyntaxKind::BindingElement)
            && node.parent().property_name() == node)
        // Is default export
        || (node.kind() == SyntaxKind::DefaultKeyword
            && has_syntactic_modifier(node.parent(), ModifierFlags::EXPORT_DEFAULT))
    {
        return get_context_node(node.parent());
    }

    Node::NIL
}

// Go: ls/findallreferences.go:286 getContextNode
pub fn get_context_node(node: Node) -> Node {
    if node.is_nil() {
        return Node::NIL;
    }
    match node.kind() {
        SyntaxKind::VariableDeclaration => {
            if !is_variable_declaration_list(node.parent())
                || node.parent().declarations().nodes().len() != 1
            {
                node
            } else if is_variable_statement(node.parent().parent()) {
                node.parent().parent()
            } else if is_for_in_or_of_statement(node.parent().parent()) {
                get_context_node(node.parent().parent())
            } else {
                node.parent()
            }
        }

        SyntaxKind::BindingElement => get_context_node(node.parent().parent()),

        SyntaxKind::ImportSpecifier => node.parent().parent().parent(),

        SyntaxKind::ExportSpecifier | SyntaxKind::NamespaceImport => node.parent().parent(),

        SyntaxKind::ImportClause | SyntaxKind::NamespaceExport => node.parent(),

        SyntaxKind::BinaryExpression => {
            if node.parent().kind() == SyntaxKind::ExpressionStatement {
                node.parent()
            } else {
                node
            }
        }

        SyntaxKind::ForOfStatement | SyntaxKind::ForInStatement => {
            // !!! not implemented
            Node::NIL
        }

        SyntaxKind::PropertyAssignment | SyntaxKind::ShorthandPropertyAssignment => {
            if is_array_literal_or_object_literal_destructuring_pattern(node.parent()) {
                return get_context_node(find_ancestor(node.parent(), |node| {
                    node.kind() == SyntaxKind::BinaryExpression || is_for_in_or_of_statement(node)
                }));
            }
            node
        }
        SyntaxKind::SwitchStatement => {
            // !!! not implemented
            Node::NIL
        }
        _ => node,
    }
}

// Go: ls/findallreferences.go:335 getRangeOfNode
pub fn get_range_of_node(node: Node, mut source_file: Node, end_node: Node) -> TextRange {
    if source_file.is_nil() {
        source_file = get_source_file_of_node(node);
    }
    let mut start = get_token_pos_of_node(node, source_file, false /*includeJsDoc*/);
    let mut end = if end_node.is_some() { end_node } else { node }.end();
    // PORT: Go counts the length and steps `end` back in Go bytes. An
    // unterminated literal can end in a marker unit (see
    // `GO_STRING_MARKER`), which has more port bytes than Go bytes, so a
    // port length of 2 or less is also a Go length of 2 or less.
    // PERF: `go_len_of_literal` reads only the literal, not the file text
    // before it.
    if is_string_literal_like(node) && end - start > 2 {
        let text = source_file_text(source_file);
        if go_len_of_literal(&text, start as usize, end as usize) > 2 {
            if end_node.is_some() {
                crate::core::go_panic("endNode is not nil for stringLiteralLike".to_string());
            }
            start += 1;
            end = go_offset_before(&text, end);
        }
    }
    if end_node.is_some() && end_node.kind() == SyntaxKind::CaseBlock {
        end = end_node.pos();
    }
    TextRange::new(start, end)
}

/// Go `end - start` for the literal at port offsets `start` to `end` of the
/// port form `text`: its length in Go bytes. `start` is the literal's quote.
/// `end` can be inside a char: Go cuts an unterminated JSDoc comment at the
/// end of the file 2 bytes early (parser/jsdoc.go:163), so a literal in it
/// can end in the first bytes of the file's last char or unit
/// (`jsdoc_text_cut`). An end `k` bytes into a unit of `g` Go bytes counts
/// `min(k, g)` of them, as `go_byte_offset` does.
fn go_len_of_literal(text: &str, start: usize, end: usize) -> usize {
    match crate::scanner_util::go_unit_cut_at(text, end) {
        Some((at, unit, _)) => go_len(&text[start..at]) + (end - at).min(unit.go_len()),
        None => go_len(&text[start..end]),
    }
}

// Go: ls/findallreferences.go:354 isValidReferencePosition
pub fn is_valid_reference_position(node: Node, search_symbol_name: &str) -> bool {
    match node.kind() {
        SyntaxKind::PrivateIdentifier => {
            // !!!
            // if (isJSDocMemberName(node.Parent)) {
            // 	return true;
            // }
            node.text().len() == search_symbol_name.len()
        }
        SyntaxKind::Identifier => node.text().len() == search_symbol_name.len(),
        SyntaxKind::NoSubstitutionTemplateLiteral | SyntaxKind::StringLiteral => {
            node.text().len() == search_symbol_name.len()
                && (is_literal_name_of_property_declaration_or_index_access(node)
                    || is_name_of_module_declaration(node)
                    || is_expression_of_external_module_import_equals_declaration(node)
                    || is_call_expression(node.parent())
                        && is_bindable_object_define_property_call(node.parent())
                        && node.parent().arguments().get(1) == node
                    || is_import_or_export_specifier(node.parent()))
        }
        SyntaxKind::NumericLiteral => {
            is_literal_name_of_property_declaration_or_index_access(node)
                && node.text().len() == search_symbol_name.len()
        }
        SyntaxKind::DefaultKeyword => "default".len() == search_symbol_name.len(),
        _ => false,
    }
}

// Go: ls/findallreferences.go:378 isForRenameWithPrefixAndSuffixText
pub fn is_for_rename_with_prefix_and_suffix_text(options: RefOptions) -> bool {
    options.use_ == ReferenceUse::RENAME && options.use_aliases_for_rename
}

// Go: ls/findallreferences.go:382 skipPastExportOrImportSpecifierOrUnion
pub fn skip_past_export_or_import_specifier_or_union(
    symbol: SymbolId,
    node: Node,
    checker: &mut Checker,
    use_local_symbol_for_export_specifier: bool,
) -> SymbolId {
    if node.is_nil() {
        return SymbolId::NIL;
    }
    let parent = node.parent();
    if parent.kind() == SyntaxKind::ExportSpecifier && use_local_symbol_for_export_specifier {
        return get_local_symbol_for_export_specifier(node, symbol, parent, checker);
    }
    // If the symbol is declared as part of a declaration like `{ type: "a" } | { type: "b" }`, use the property on the union type to get more references.
    // Go: core.FirstNonNil(symbol.Declarations, ...)
    let declarations = checker.sym(symbol).declarations.to_vec();
    for decl in declarations {
        let result = if decl.parent().is_nil() {
            // Ignore UMD module and global merge and CJS module end exports symbols
            if checker
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::TRANSIENT | SymbolFlags::MODULE_EXPORTS)
            {
                SymbolId::NIL
            } else {
                // Assertions for GH#21814. We should be handling SourceFile symbols in `getReferencedSymbolsForModule` instead of getting here.
                crate::core::go_panic(format!(
                    "Unexpected symbol at {}: {}",
                    crate::gostd::debug::kind_string(node.kind()),
                    checker.sym(symbol).name.as_str()
                ));
            }
        } else if decl.parent().kind() == SyntaxKind::TypeLiteral
            && decl.parent().parent().kind() == SyntaxKind::UnionType
        {
            let t = checker.get_type_from_type_node_exported(decl.parent().parent());
            let name = checker.sym(symbol).name.as_str();
            checker.get_property_of_type_exported(t, name)
        } else {
            SymbolId::NIL
        };
        if result.is_some() {
            return result;
        }
    }
    SymbolId::NIL
}

// Go: ls/findallreferences.go:407 getSymbolScope
// PORT: Go reads the symbol without a checker and calls the package
// function `checker.IsExternalModuleSymbol`, which is a `Checker` method
// here. The checker is the first parameter.
pub fn get_symbol_scope(c: &Checker, symbol: SymbolId) -> Node {
    let s = c.sym(symbol);
    // If this is the symbol of a named function expression or named class expression,
    // then named references are limited to its own scope.
    let value_declaration = s.value_declaration;
    if value_declaration.is_some()
        && (value_declaration.kind() == SyntaxKind::FunctionExpression
            || value_declaration.kind() == SyntaxKind::ClassExpression)
    {
        return value_declaration;
    }

    if s.declarations.is_empty() {
        return Node::NIL;
    }

    let declarations: &[Node] = &s.declarations;
    // If this is private property or method, the scope is the containing class
    if s.flags
        .intersects(SymbolFlags::PROPERTY | SymbolFlags::METHOD)
    {
        let private_declaration = declarations.iter().copied().find(|&d| {
            has_modifier(d, ModifierFlags::PRIVATE)
                || is_private_identifier_class_element_declaration(d)
        });
        if let Some(private_declaration) = private_declaration {
            return find_ancestor_kind(private_declaration, SyntaxKind::ClassDeclaration);
        }
        // Else this is a public property and could be accessed from anywhere.
        return Node::NIL;
    }

    // If symbol is of object binding pattern element without property name we would want to
    // look for property too and that could be anywhere
    if declarations
        .iter()
        .any(|&d| is_object_binding_element_without_property_name(d))
    {
        return Node::NIL;
    }

    /*
        If the symbol has a parent, it's globally visible unless:
        - It's a private property (handled above).
        - It's a type parameter.
        - The parent is an external module: then we should only search in the module (and recurse on the export later).
        - But if the parent has `export as namespace`, the symbol is globally visible through that namespace.
    */
    let exposed_by_parent = s.parent.is_some() && !s.flags.intersects(SymbolFlags::TYPE_PARAMETER);
    if exposed_by_parent
        && !(c.is_external_module_symbol(s.parent)
            && !is_source_file_with_global_exports(c.sym(s.parent).value_declaration))
    {
        return Node::NIL;
    }

    let mut scope = Node::NIL;
    for &declaration in declarations {
        let container = get_container_node(declaration);
        if scope.is_some() && scope != container {
            // Different declarations have different containers, bail out
            return Node::NIL;
        }

        if container.is_nil()
            || (container.kind() == SyntaxKind::SourceFile
                && !is_external_or_common_js_module(container))
        {
            // This is a global variable and not an external module, any declaration defined
            // within this scope is visible outside the file
            return Node::NIL;
        }

        scope = container;
    }

    // If symbol.parent, this means we are in an export of an external module. (Otherwise we would have returned `undefined` above.)
    // For an export of a module, we may be in a declaration file, and it may be accessed elsewhere. E.g.:
    //     declare module "a" { export type T = number; }
    //     declare module "b" { import { T } from "a"; export const x: T; }
    // So we must search the whole source file. (Because we will mark the source file as seen, we we won't return to it when searching for imports.)
    if exposed_by_parent {
        return get_source_file_of_node(scope);
    }
    scope // TODO: GH#18217
}

// === functions on (*ls) ===

// Go: ls/findallreferences.go:480 position
// PORT: Go `position`; the Rust type name is `Position`. Write
// `lsproto::Position` qualified in ls code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub uri: lsproto::DocumentUri,
    pub pos: lsproto::Position,
}

// Go: ls/findallreferences.go:479 `var _ lsproto.HasTextDocumentPosition = (*position)(nil)`

impl HasTextDocumentURI for Position {
    // Go: ls/findallreferences.go:487 TextDocumentURI
    fn text_document_uri(&self) -> lsproto::DocumentUri {
        self.uri.clone()
    }
}

impl HasTextDocumentPosition for Position {
    // Go: ls/findallreferences.go:488 TextDocumentPosition
    fn text_document_position(&self) -> lsproto::Position {
        self.pos
    }
}

// Go: ls/findallreferences.go:490 nonLocalDefinition
// PORT: Go embeds `position`; here the field `position`, and the two
// `HasTextDocument*` impls below forward to it. The Go `sync.OnceValue`
// functions close over the language service, so the struct borrows it
// (`'l`); each function caches its result in a `OnceCell`. Go returns a
// `lsproto.HasTextDocumentPosition` that is a `*position` or nil:
// `Option<Position>`.
pub struct NonLocalDefinition<'l> {
    pub position: Position,
    pub get_source_position: Box<dyn Fn() -> Option<Position> + 'l>,
    pub get_generated_position: Box<dyn Fn() -> Option<Position> + 'l>,
}

impl HasTextDocumentURI for NonLocalDefinition<'_> {
    // Go: promoted from the embedded `position`.
    fn text_document_uri(&self) -> lsproto::DocumentUri {
        self.position.text_document_uri()
    }
}

impl HasTextDocumentPosition for NonLocalDefinition<'_> {
    // Go: promoted from the embedded `position`.
    fn text_document_position(&self) -> lsproto::Position {
        self.position.text_document_position()
    }
}

// Go: ls/findallreferences.go:496 getFileAndStartPosFromDeclaration
// PORT: Go `core.TextPos` is `i32`.
pub fn get_file_and_start_pos_from_declaration(declaration: Node) -> (Node, i32) {
    let file = get_source_file_of_node(declaration);
    let name = {
        let name = get_name_of_declaration(declaration);
        if name.is_some() { name } else { declaration }
    };
    let text_range = get_range_of_node(name, file, Node::NIL /*endNode*/);

    (file, text_range.pos())
}

impl LanguageService {
    // Go: ls/findallreferences.go:504 getNonLocalDefinition
    pub fn get_non_local_definition(
        &self,
        ctx: &Context,
        entry: &Rc<RefCell<SymbolAndEntries>>,
    ) -> Option<NonLocalDefinition<'_>> {
        if !entry.borrow().can_use_definition_symbol() {
            return None;
        }

        let program = self.get_program();
        let (checker, _done) = ls_program::get_type_checker(program, ctx);
        let checker = &mut *checker.borrow_mut();
        // ts#64649: a new emit resolver for a new emit context (Go N'
        // findallreferences.go:512); `GetEmitResolver` is gone.
        let emit_resolver = checker.new_emit_resolver(new_emit_context());
        let symbol = entry
            .borrow()
            .definition
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .symbol;
        let declarations = checker.sym(symbol).declarations.to_vec();
        for d in declarations {
            if is_definition_visible(&emit_resolver, checker, d) {
                let (file, start_pos) = get_file_and_start_pos_from_declaration(d);
                let file_name = source_file_file_name(file).to_string();
                let (lsp_position, fidelity) = self.converters.to_lsp_position(&file, start_pos);
                if fidelity.is_none() {
                    continue;
                }
                let source_file_name = file_name.clone();
                let generated_file_name = file_name.clone();
                let source_position: OnceCell<Option<Position>> = OnceCell::new();
                let generated_position: OnceCell<Option<Position>> = OnceCell::new();
                return Some(NonLocalDefinition {
                    position: Position {
                        uri: lsconv::file_name_to_document_uri(&file_name),
                        pos: lsp_position,
                    },
                    get_source_position: Box::new(move || {
                        source_position
                            .get_or_init(|| {
                                let mapped =
                                    self.try_get_source_position(&source_file_name, start_pos);
                                if let Some(mapped) = mapped {
                                    let script = self.get_script(&mapped.file_name);
                                    let (mapped_position, mapped_fidelity) =
                                        self.converters.to_lsp_position(&script, mapped.pos);
                                    if mapped_fidelity.is_none() {
                                        return None;
                                    }
                                    return Some(Position {
                                        uri: lsconv::file_name_to_document_uri(&mapped.file_name),
                                        pos: mapped_position,
                                    });
                                }
                                None
                            })
                            .clone()
                    }),
                    get_generated_position: Box::new(move || {
                        generated_position
                            .get_or_init(|| {
                                let mapped = self
                                    .try_get_generated_position(&generated_file_name, start_pos);
                                if let Some(mapped) = mapped {
                                    let script = self.get_script(&mapped.file_name);
                                    let (mapped_position, mapped_fidelity) =
                                        self.converters.to_lsp_position(&script, mapped.pos);
                                    if mapped_fidelity.is_none() {
                                        return None;
                                    }
                                    return Some(Position {
                                        uri: lsconv::file_name_to_document_uri(&mapped.file_name),
                                        pos: mapped_position,
                                    });
                                }
                                None
                            })
                            .clone()
                    }),
                });
            }
        }
        None
    }
}

// Go: ls/findallreferences.go:561 isDefinitionVisible
// This is special handling to determine if we should load up more projects and find location in other projects
// By default arrows (and such other ast kinds) are not visible as declaration emitter doesnt need them
// But we want to handle them specially so that they are visible if their parent is visible
// PORT: Go `emitResolver.IsDeclarationVisible` locks the resolver's checker
// and calls `isDeclarationVisible`. The caller already holds that checker
// (`c`), so this calls the inner method with it; the Rust exported form
// borrows the compile-pool checker of a worker thread.
pub fn is_definition_visible(
    emit_resolver: &crate::checker::emit_resolver_p1::EmitResolver,
    c: &mut Checker,
    declaration: Node,
) -> bool {
    if emit_resolver.is_declaration_visible(c, declaration) {
        return true;
    }
    if declaration.parent().is_nil() {
        return false;
    }

    // Variable initializers are visible if variable is visible
    if has_initializer(declaration.parent()) && declaration.parent().initializer() == declaration {
        return is_definition_visible(emit_resolver, c, declaration.parent());
    }

    // Handle some exceptions here like arrow function, members of class and object literal expression which are technically not visible but we want the definition to be determined by its parent
    match declaration.kind() {
        SyntaxKind::PropertyDeclaration
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::MethodDeclaration => {
            // Private/protected properties/methods are not visible
            if has_modifier(declaration, ModifierFlags::PRIVATE)
                || is_private_identifier(declaration.name())
            {
                return false;
            }
            // Public properties/methods are visible if its parents are visible, so:
            // falls through
            is_definition_visible(emit_resolver, c, declaration.parent())
        }
        SyntaxKind::Constructor
        | SyntaxKind::PropertyAssignment
        | SyntaxKind::ShorthandPropertyAssignment
        | SyntaxKind::ObjectLiteralExpression
        | SyntaxKind::ClassExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::FunctionExpression => {
            is_definition_visible(emit_resolver, c, declaration.parent())
        }
        _ => false,
    }
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/findallreferences.go:600 forEachOriginalDefinitionLocation
    pub fn for_each_original_definition_location(
        &self,
        ctx: &Context,
        entry: &Rc<RefCell<SymbolAndEntries>>,
        cb: &mut dyn FnMut(lsproto::DocumentUri, lsproto::Position),
    ) {
        if !entry.borrow().can_use_definition_symbol() {
            return;
        }

        let program = self.get_program();
        // PORT: Go reads `symbol.Declarations` directly; see the file header.
        let declarations = {
            let symbol = entry
                .borrow()
                .definition
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .symbol;
            let (checker, _done) = program.get_type_checker(ctx);
            let declarations = checker.borrow().sym(symbol).declarations.to_vec();
            declarations
        };
        for d in declarations {
            let (file, start_pos) = get_file_and_start_pos_from_declaration(d);
            let file_name = source_file_file_name(file);
            if tspath::is_declaration_file_name(file_name) {
                // Map to ts position
                let mapped = self.try_get_source_position(source_file_file_name(file), start_pos);
                if let Some(mapped) = mapped {
                    let script = self.get_script(&mapped.file_name);
                    let (lsp_position, fidelity) =
                        self.converters.to_lsp_position(&script, mapped.pos);
                    if !fidelity.is_none() {
                        cb(
                            lsconv::file_name_to_document_uri(&mapped.file_name),
                            lsp_position,
                        );
                    }
                }
            } else if program.is_source_from_project_reference(&self.to_path(file_name)) {
                let (lsp_position, fidelity) = self.converters.to_lsp_position(&file, start_pos);
                if !fidelity.is_none() {
                    cb(lsconv::file_name_to_document_uri(file_name), lsp_position);
                }
            }
        }
    }
}

// Go: ls/findallreferences.go:631 symbolEntryTransformOptions
#[derive(Clone, Copy, Debug, Default)]
pub struct SymbolEntryTransformOptions {
    // Force the result to be Location objects.
    pub require_locations_result: bool,
    // Omit node(s) containing the original position.
    pub drop_origin_nodes: bool,
}

// Go: ls/findallreferences.go:638 SymbolAndEntriesData
#[derive(Clone, Debug, Default)]
pub struct SymbolAndEntriesData {
    pub original_node: Node,
    pub symbols_and_entries: Vec<Rc<RefCell<SymbolAndEntries>>>,
    pub position: i32,
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/findallreferences.go:644 provideSymbolsAndEntries
    // PORT: Go passes the URI by value; here by reference.
    pub fn provide_symbols_and_entries(
        &self,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
        document_position: lsproto::Position,
        is_rename: bool,
        implementations: bool,
    ) -> (SymbolAndEntriesData, bool) {
        // `findReferencedSymbols` except only computes the information needed to return reference locations
        let (program, source_file) = self.get_program_and_file(uri);
        let mut feature = Feature::REFERENCES;
        if implementations {
            feature = Feature::IMPLEMENTATION;
        } else if is_rename {
            feature = Feature::RENAME;
        }
        let positions = lsconv::from_lsp_position_for_source_file(
            &self.converters,
            source_file,
            document_position,
            feature,
        );
        if positions.is_empty() {
            return (SymbolAndEntriesData::default(), false);
        }
        let mut combined = SymbolAndEntriesData::default();
        let mut ok = false;
        for mapped in positions {
            if !mapped.fidelity.is_single_segment() {
                continue;
            }
            let (data, found) = self.provide_symbols_and_entries_at_position(
                ctx,
                program,
                mapped.script,
                mapped.position,
                is_rename,
                implementations,
            );
            if !found {
                continue;
            }
            if !ok {
                combined.original_node = data.original_node;
                combined.position = data.position;
                ok = true;
            }
            combined
                .symbols_and_entries
                .extend(data.symbols_and_entries);
        }
        (combined, ok)
    }

    // Go: ls/findallreferences.go:677 provideSymbolsAndEntriesAtPosition
    pub fn provide_symbols_and_entries_at_position(
        &self,
        ctx: &Context,
        program: &P,
        source_file: Node,
        position: i32,
        is_rename: bool,
        implementations: bool,
    ) -> (SymbolAndEntriesData, bool) {
        let mut node = astnav::get_touching_property_name(source_file, position);
        if is_rename {
            // Adjust modifier/keyword nodes to the declaration name, matching Strada's findRenameLocations.
            node = get_adjusted_location(node, true /*forRename*/, source_file);
        }
        if is_rename && !node_is_eligible_for_rename(node)
            || implementations && is_source_file(node)
        {
            return (
                SymbolAndEntriesData {
                    original_node: node,
                    position,
                    ..Default::default()
                },
                false,
            );
        }

        let entries =
            self.get_symbol_and_entries(ctx, position, node, program, is_rename, implementations);
        if !implementations {
            return (
                SymbolAndEntriesData {
                    original_node: node,
                    symbols_and_entries: entries,
                    position,
                },
                true,
            );
        }

        let mut implementation_entries: Vec<Rc<RefCell<SymbolAndEntries>>> = Vec::new();
        let mut queue: VecDeque<Rc<RefCell<ReferenceEntry>>> = VecDeque::new();
        let mut seen_nodes: FxHashSet<Node> = FxHashSet::default();
        let mut seen_definitions: FxHashSet<SymbolId> = FxHashSet::default();
        // Go: addToQueue
        // PORT: the Go closure captures the locals; here they are parameters.
        let add_to_queue =
            |implementation_entries: &mut Vec<Rc<RefCell<SymbolAndEntries>>>,
             queue: &mut VecDeque<Rc<RefCell<ReferenceEntry>>>,
             seen_nodes: &mut FxHashSet<Node>,
             seen_definitions: &mut FxHashSet<SymbolId>,
             symbol_and_entries: Vec<Rc<RefCell<SymbolAndEntries>>>| {
                for s in &symbol_and_entries {
                    let s = s.borrow();
                    let mut new_references: Vec<Rc<RefCell<ReferenceEntry>>> = Vec::new();
                    for ref_ in &s.references {
                        if seen_nodes.insert(ref_.borrow().node) {
                            queue.push_back(ref_.clone());
                            new_references.push(ref_.clone());
                        }
                    }
                    if !new_references.is_empty()
                        || s.definition.is_none()
                        || seen_definitions.insert(s.definition.as_ref().unwrap().symbol)
                    {
                        implementation_entries.push(Rc::new(RefCell::new(SymbolAndEntries {
                            definition: s.definition.clone(),
                            references: new_references,
                        })));
                    }
                }
            };

        add_to_queue(
            &mut implementation_entries,
            &mut queue,
            &mut seen_nodes,
            &mut seen_definitions,
            entries,
        );
        while let Some(entry) = queue.front().cloned() {
            if ctx.err().is_some() {
                return (SymbolAndEntriesData::default(), false);
            }

            queue.pop_front();
            let entry_node = entry.borrow().node;
            if entry_node.is_some() {
                let found = self.get_symbol_and_entries(
                    ctx,
                    entry_node.pos(),
                    entry_node,
                    program,
                    is_rename,
                    implementations,
                );
                add_to_queue(
                    &mut implementation_entries,
                    &mut queue,
                    &mut seen_nodes,
                    &mut seen_definitions,
                    found,
                );
            }
        }
        (
            SymbolAndEntriesData {
                original_node: node,
                symbols_and_entries: implementation_entries,
                position,
            },
            true,
        )
    }

    // Go: ls/findallreferences.go:726 getSymbolAndEntries
    pub fn get_symbol_and_entries(
        &self,
        ctx: &Context,
        position: i32,
        node: Node,
        program: &P,
        is_rename: bool,
        implementations: bool,
    ) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
        let mut options = RefOptions::default();
        if !is_rename {
            options.use_ = ReferenceUse::REFERENCES;
            if implementations {
                options.implementations = true;
            }
        } else {
            options.use_ = ReferenceUse::RENAME;
            options.use_aliases_for_rename = self
                .user_preferences()
                .use_aliases_for_rename
                .is_true_or_unknown();
        }
        // PORT: Go `*ast.SourceFile` is the file root `Node`.
        let source_files: Vec<Node> = program.source_file_roots();
        self.get_referenced_symbols_for_node(ctx, position, node, program, &source_files, options)
    }
}

/// `ProvideReferences` as a `CrossProjectSearch`: its searches in other
/// projects run on search threads.
pub struct ReferencesSearch;

impl CrossProjectSearch for ReferencesSearch {
    type Req = lsproto::ReferenceParams;
    type Resp = lsproto::ReferencesResponse;

    fn to_resp<P: ProgramView>(
        ls: &LanguageService<P>,
        ctx: &Context,
        params: &Self::Req,
        data: SymbolAndEntriesData,
        options: SymbolEntryTransformOptions,
    ) -> Result<Self::Resp, GoError> {
        ls.symbol_and_entries_to_references(ctx, params, data, options)
    }
}

/// `ProvideImplementations` as a `CrossProjectSearch`.
pub struct ImplementationsSearch;

impl CrossProjectSearch for ImplementationsSearch {
    type Req = lsproto::ImplementationParams;
    type Resp = lsproto::ImplementationResponse;

    fn to_resp<P: ProgramView>(
        ls: &LanguageService<P>,
        ctx: &Context,
        params: &Self::Req,
        data: SymbolAndEntriesData,
        options: SymbolEntryTransformOptions,
    ) -> Result<Self::Resp, GoError> {
        ls.symbol_and_entries_to_implementations(ctx, params, data, options)
    }
}

impl LanguageService {
    // Go: ls/findallreferences.go:747 ProvideReferences
    pub fn provide_references(
        &self,
        ctx: &Context,
        params: &lsproto::ReferenceParams,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
    ) -> Result<lsproto::ReferencesResponse, GoError> {
        handle_cross_project(
            self,
            ctx,
            params,
            orchestrator,
            LanguageService::symbol_and_entries_to_references,
            Some(start_search::<ReferencesSearch>),
            combine_references,
            false, /*isRename*/
            false, /*implementations*/
            SymbolEntryTransformOptions::default(),
            None, /*defaultProjectData*/
        )
    }

    // Go: ls/findallreferences.go:761 provideReferencesFromData
    pub fn provide_references_from_data(
        &self,
        ctx: &Context,
        params: &lsproto::ReferenceParams,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
        data: SymbolAndEntriesData,
    ) -> Result<lsproto::ReferencesResponse, GoError> {
        handle_cross_project(
            self,
            ctx,
            params,
            orchestrator,
            LanguageService::symbol_and_entries_to_references,
            Some(start_search::<ReferencesSearch>),
            combine_references,
            false, /*isRename*/
            false, /*implementations*/
            SymbolEntryTransformOptions::default(),
            Some(data),
        )
    }

    // Go: ls/findallreferences.go:775 ProvideVSReferences
    pub fn provide_vs_references(
        &self,
        ctx: &Context,
        params: &lsproto::ReferenceParams,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
    ) -> Result<lsproto::VSReferencesResponse, GoError> {
        handle_cross_project(
            self,
            ctx,
            params,
            orchestrator,
            LanguageService::symbol_and_entries_to_vs_references,
            None, /*search*/
            combine_vs_references,
            false, /*isRename*/
            false, /*implementations*/
            SymbolEntryTransformOptions::default(),
            None, /*defaultProjectData*/
        )
    }
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/findallreferences.go:789 symbolAndEntriesToReferences
    pub fn symbol_and_entries_to_references(
        &self,
        ctx: &Context,
        params: &lsproto::ReferenceParams,
        data: SymbolAndEntriesData,
        _options: SymbolEntryTransformOptions,
    ) -> Result<lsproto::ReferencesResponse, GoError> {
        // `findReferencedSymbols` except only computes the information needed to return reference locations
        let mut locations: Vec<lsproto::Location> = Vec::new();
        let mut seen_locations: FxHashSet<lsproto::Location> = FxHashSet::default();
        for symbol in &data.symbols_and_entries {
            let symbol_locations = self.convert_symbol_and_entries_to_locations(
                ctx,
                symbol,
                params
                    .context
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .include_declaration,
                Feature::REFERENCES,
            );
            locations = combine_location_array(locations, &symbol_locations, &mut seen_locations);
        }
        Ok(lsproto::LocationsOrNull {
            locations: Some(locations),
        })
    }
}

impl LanguageService {
    // Go: ls/findallreferences.go:800 symbolAndEntriesToVSReferences
    pub fn symbol_and_entries_to_vs_references(
        &self,
        ctx: &Context,
        _params: &lsproto::ReferenceParams,
        data: SymbolAndEntriesData,
        _options: SymbolEntryTransformOptions,
    ) -> Result<lsproto::VSReferencesResponse, GoError> {
        let caps = lsproto::get_client_capabilities(ctx);
        let vs_capability = caps.vs_supports_visual_studio_extensions;
        let mut items: Vec<lsproto::VSReferenceItem> = Vec::new();
        let mut id: i32 = 0;
        let project_name = self.project_id.string();

        for s in &data.symbols_and_entries {
            let (definition, references) = {
                let s = s.borrow();
                (s.definition.clone(), s.references.clone())
            };
            let Some(definition) = definition else {
                continue;
            };

            // Convert definition to info
            let def_info = self.definition_to_referenced_symbol_definition_info(
                ctx,
                &definition,
                data.original_node,
                vs_capability,
                Feature::REFERENCES,
            );
            let Some(def_info) = def_info else {
                continue;
            };

            // Create the definition item
            let definition_id = id;
            let empty_str = String::new();
            let def_item = lsproto::VSReferenceItem {
                vs_id: definition_id,
                vs_location: def_info.location,
                vs_definition_text: def_info.display_text,
                vs_kind: Some(vec![lsproto::VSReferenceKind::UNKNOWN]),
                vs_project_name: Some(project_name.clone()),
                vs_containing_type: Some(empty_str),
                ..Default::default()
            };
            items.push(def_item);
            id += 1;

            // PORT: Go reads `symbol.Declarations` directly; see the file header.
            let (checker, _done) = ls_program::get_type_checker(self.get_program(), ctx);

            // Create reference items grouped under the definition
            for ref_ in &references {
                let (ref_node, ref_kind) = {
                    let r = ref_.borrow();
                    (r.node, r.kind)
                };
                // Skip the declaration itself (already represented by the definition item)
                if definition.symbol.is_some()
                    && is_declaration_of_symbol(
                        &checker.borrow().symbols,
                        ref_node,
                        definition.symbol,
                    )
                {
                    continue;
                }

                let (ref_location, ok) =
                    self.get_location_of_entry_for_feature(ref_, Feature::REFERENCES);
                if !ok {
                    continue;
                }

                // Determine read/write kind
                let mut kind = lsproto::VSReferenceKind::READ;
                if ref_kind != EntryKind::RANGE
                    && ref_node.is_some()
                    && is_write_access_for_reference(ref_node)
                {
                    kind = lsproto::VSReferenceKind::WRITE;
                }

                let ref_item = lsproto::VSReferenceItem {
                    vs_id: id,
                    vs_definition_id: Some(definition_id),
                    vs_location: ref_location,
                    vs_kind: Some(vec![kind]),
                    vs_project_name: Some(project_name.clone()),
                    ..Default::default()
                };
                items.push(ref_item);
                id += 1;
            }
        }

        Ok(lsproto::VSReferenceItemsOrNull {
            vs_reference_items: Some(items),
        })
    }
}

// Go: ls/findallreferences.go:866 referencedSymbolDefinitionInfo
// referencedSymbolDefinitionInfo holds the computed info for a definition
#[derive(Clone, Debug, Default)]
pub struct ReferencedSymbolDefinitionInfo {
    pub node: Node,
    pub location: lsproto::Location,
    pub display_text: Option<lsproto::VSClassifiedTextElement>,
}

impl LanguageService {
    // Go: ls/findallreferences.go:873 definitionToReferencedSymbolDefinitionInfo
    // definitionToReferencedSymbolDefinitionInfo converts a Definition to display info
    pub fn definition_to_referenced_symbol_definition_info(
        &self,
        ctx: &Context,
        def: &Definition,
        original_node: Node,
        vs_capability: bool,
        feature: Feature,
    ) -> Option<ReferencedSymbolDefinitionInfo> {
        match def.kind {
            DefinitionKind::SYMBOL => {
                let symbol = def.symbol;
                if symbol.is_nil() {
                    return None;
                }
                // Get display parts
                let element = self.get_definition_kind_and_display_parts(
                    ctx,
                    symbol,
                    original_node,
                    vs_capability,
                );

                // Get the definition node
                // PORT: Go reads `symbol.Declarations` directly; see the file header.
                let declarations = {
                    let (checker, _done) = ls_program::get_type_checker(self.get_program(), ctx);
                    let declarations = checker.borrow().sym(symbol).declarations.to_vec();
                    declarations
                };
                let node = if !declarations.is_empty() {
                    let decl = declarations[0];
                    let name = decl.name();
                    if name.is_some() { name } else { decl }
                } else {
                    original_node
                };

                let (loc, ok) = self.get_location_of_entry_for_feature(
                    &Rc::new(RefCell::new(ReferenceEntry {
                        kind: EntryKind::NODE,
                        node,
                        ..Default::default()
                    })),
                    feature,
                );
                if !ok {
                    return None;
                }
                Some(ReferencedSymbolDefinitionInfo {
                    node,
                    location: loc,
                    display_text: Some(element),
                })
            }

            DefinitionKind::LABEL => {
                let node = def.node;
                if node.is_nil() {
                    return None;
                }
                let (loc, ok) = self.get_location_of_entry_for_feature(
                    &Rc::new(RefCell::new(ReferenceEntry {
                        kind: EntryKind::NODE,
                        node,
                        ..Default::default()
                    })),
                    feature,
                );
                if !ok {
                    return None;
                }
                Some(ReferencedSymbolDefinitionInfo {
                    node,
                    location: loc,
                    display_text: Some(lsproto::VSClassifiedTextElement {
                        runs: vec![Some(lsproto::VSClassifiedTextRun {
                            text: node.text().to_string(),
                            classification_type_name: lsproto::ClassificationTypeName::TEXT
                                .0
                                .to_string(),
                            ..Default::default()
                        })],
                        ..Default::default()
                    }),
                })
            }

            DefinitionKind::KEYWORD => {
                let node = def.node;
                if node.is_nil() {
                    return None;
                }
                let name = token_to_string(node.kind());
                let (loc, ok) = self.get_location_of_entry_for_feature(
                    &Rc::new(RefCell::new(ReferenceEntry {
                        kind: EntryKind::NODE,
                        node,
                        ..Default::default()
                    })),
                    feature,
                );
                if !ok {
                    return None;
                }
                Some(ReferencedSymbolDefinitionInfo {
                    node,
                    location: loc,
                    display_text: Some(lsproto::VSClassifiedTextElement {
                        runs: vec![Some(lsproto::VSClassifiedTextRun {
                            text: name.to_string(),
                            classification_type_name: lsproto::ClassificationTypeName::KEYWORD
                                .0
                                .to_string(),
                            ..Default::default()
                        })],
                        ..Default::default()
                    }),
                })
            }

            DefinitionKind::THIS => {
                let node = def.node;
                if node.is_nil() {
                    return None;
                }
                let symbol = def.symbol;
                if symbol.is_nil() {
                    return None;
                }
                let element =
                    self.get_definition_kind_and_display_parts(ctx, symbol, node, vs_capability);
                let (loc, ok) = self.get_location_of_entry_for_feature(
                    &Rc::new(RefCell::new(ReferenceEntry {
                        kind: EntryKind::NODE,
                        node,
                        ..Default::default()
                    })),
                    feature,
                );
                if !ok {
                    return None;
                }
                Some(ReferencedSymbolDefinitionInfo {
                    node,
                    location: loc,
                    display_text: Some(element),
                })
            }

            DefinitionKind::STRING => {
                let node = def.node;
                if node.is_nil() {
                    return None;
                }
                let (loc, ok) = self.get_location_of_entry_for_feature(
                    &Rc::new(RefCell::new(ReferenceEntry {
                        kind: EntryKind::NODE,
                        node,
                        ..Default::default()
                    })),
                    feature,
                );
                if !ok {
                    return None;
                }
                Some(ReferencedSymbolDefinitionInfo {
                    node,
                    location: loc,
                    display_text: Some(lsproto::VSClassifiedTextElement {
                        runs: vec![Some(lsproto::VSClassifiedTextRun {
                            text: node.text().to_string(),
                            classification_type_name: lsproto::ClassificationTypeName::STRING
                                .0
                                .to_string(),
                            ..Default::default()
                        })],
                        ..Default::default()
                    }),
                })
            }

            DefinitionKind::TRIPLE_SLASH_REFERENCE => {
                let Some(triple_slash_file_ref) = &def.triple_slash_file_ref else {
                    return None;
                };
                if triple_slash_file_ref.file.is_nil() {
                    return None;
                }
                let node = triple_slash_file_ref.file;
                let (loc, ok) = self.get_location_of_entry_for_feature(
                    &Rc::new(RefCell::new(ReferenceEntry {
                        kind: EntryKind::NODE,
                        node,
                        ..Default::default()
                    })),
                    feature,
                );
                if !ok {
                    return None;
                }
                let reference_file_name = &triple_slash_file_ref
                    .reference
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .file_name;
                Some(ReferencedSymbolDefinitionInfo {
                    node,
                    location: loc,
                    display_text: Some(lsproto::VSClassifiedTextElement {
                        runs: vec![Some(lsproto::VSClassifiedTextRun {
                            text: format!("\"{reference_file_name}\""),
                            classification_type_name: lsproto::ClassificationTypeName::STRING
                                .0
                                .to_string(),
                            ..Default::default()
                        })],
                        ..Default::default()
                    }),
                })
            }

            _ => None,
        }
    }

    // Go: ls/findallreferences.go:997 getDefinitionKindAndDisplayParts
    // getDefinitionKindAndDisplayParts returns the classified display text for a symbol definition.
    // PORT: Go returns a non-nil `*lsproto.VSClassifiedTextElement`; here
    // the value.
    pub fn get_definition_kind_and_display_parts(
        &self,
        ctx: &Context,
        symbol: SymbolId,
        original_node: Node,
        vs_capability: bool,
    ) -> lsproto::VSClassifiedTextElement {
        let program = self.get_program();
        let (c, _done) = ls_program::get_type_checker(program, ctx);
        let c = &mut *c.borrow_mut();

        let meaning = get_intersecting_meaning_from_declarations(
            &c.symbols,
            original_node,
            symbol,
            SemanticMeaning::ALL,
        );

        let info = get_quick_info_and_declaration_at_location(
            c,
            symbol,
            original_node,
            None,
            vs_capability,
            meaning,
        );

        if vs_capability {
            return lsproto::VSClassifiedTextElement {
                runs: info
                    .display_parts
                    .borrow()
                    .get_runs(c)
                    .into_iter()
                    .map(Some)
                    .collect(),
                ..Default::default()
            };
        }
        // Fallback: single unclassified run with the full text
        let text = info.display_parts.borrow().string();
        lsproto::VSClassifiedTextElement {
            runs: vec![Some(lsproto::VSClassifiedTextRun {
                text,
                classification_type_name: lsproto::ClassificationTypeName::TEXT.0.to_string(),
                ..Default::default()
            })],
            ..Default::default()
        }
    }

    // Go: ls/findallreferences.go:1016 ProvideImplementations
    pub fn provide_implementations(
        &self,
        ctx: &Context,
        params: &lsproto::ImplementationParams,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
    ) -> Result<lsproto::ImplementationResponse, GoError> {
        self.provide_implementations_ex(
            ctx,
            params,
            SymbolEntryTransformOptions::default(),
            orchestrator,
        )
    }

    // Go: ls/findallreferences.go:1020 provideImplementationsEx
    pub fn provide_implementations_ex(
        &self,
        ctx: &Context,
        params: &lsproto::ImplementationParams,
        options: SymbolEntryTransformOptions,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
    ) -> Result<lsproto::ImplementationResponse, GoError> {
        handle_cross_project(
            self,
            ctx,
            params,
            orchestrator,
            LanguageService::symbol_and_entries_to_implementations,
            Some(start_search::<ImplementationsSearch>),
            combine_implementations,
            false, /*isRename*/
            true,  /*implementations*/
            options,
            None, /*defaultProjectData*/
        )
    }

    // Go: ls/findallreferences.go:1034 provideImplementationsFromData
    pub fn provide_implementations_from_data(
        &self,
        ctx: &Context,
        params: &lsproto::ImplementationParams,
        options: SymbolEntryTransformOptions,
        orchestrator: Option<&dyn CrossProjectOrchestrator>,
        data: SymbolAndEntriesData,
    ) -> Result<lsproto::ImplementationResponse, GoError> {
        handle_cross_project(
            self,
            ctx,
            params,
            orchestrator,
            LanguageService::symbol_and_entries_to_implementations,
            Some(start_search::<ImplementationsSearch>),
            combine_implementations,
            false, /*isRename*/
            true,  /*implementations*/
            options,
            Some(data),
        )
    }
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/findallreferences.go:1048 symbolAndEntriesToImplementations
    pub fn symbol_and_entries_to_implementations(
        &self,
        ctx: &Context,
        _params: &lsproto::ImplementationParams,
        data: SymbolAndEntriesData,
        options: SymbolEntryTransformOptions,
    ) -> Result<lsproto::ImplementationResponse, GoError> {
        let mut seen_nodes: FxHashSet<Node> = FxHashSet::default();
        let mut entries: Vec<Rc<RefCell<ReferenceEntry>>> = Vec::new();
        for entry in &data.symbols_and_entries {
            let references = entry.borrow().references.clone();
            for ref_ in references {
                let ref_node = ref_.borrow().node;
                if seen_nodes.insert(ref_node)
                    && (!options.drop_origin_nodes
                        || !ref_node.loc().contains_inclusive(data.position))
                {
                    entries.push(ref_);
                }
            }
        }

        if !options.require_locations_result
            && lsproto::get_client_capabilities(ctx)
                .text_document
                .implementation
                .link_support
        {
            let links = self.convert_entries_to_location_links(&entries, Feature::IMPLEMENTATION);
            return Ok(lsproto::LocationOrLocationsOrDefinitionLinksOrNull {
                definition_links: Some(links),
                ..Default::default()
            });
        }
        let locations = self.convert_entries_to_locations(&entries, Feature::IMPLEMENTATION);
        Ok(lsproto::LocationOrLocationsOrDefinitionLinksOrNull {
            locations: Some(locations),
            ..Default::default()
        })
    }

    // == functions for conversions ==
    // Go: ls/findallreferences.go:1068 convertSymbolAndEntriesToLocations
    // PORT: takes `ctx` to read the definition symbol through the request
    // checker (see the file header).
    pub fn convert_symbol_and_entries_to_locations(
        &self,
        ctx: &Context,
        s: &Rc<RefCell<SymbolAndEntries>>,
        include_declarations: bool,
        feature: Feature,
    ) -> Vec<lsproto::Location> {
        let (definition, mut references) = {
            let s = s.borrow();
            (s.definition.clone(), s.references.clone())
        };

        // !!! includeDeclarations
        if !include_declarations && let Some(definition) = &definition {
            let (checker, _done) = self.get_program().get_type_checker(ctx);
            let checker = checker.borrow();
            references.retain(|entry| {
                !is_declaration_of_symbol(&checker.symbols, entry.borrow().node, definition.symbol)
            });
        }

        self.convert_entries_to_locations(&references, feature)
    }
}

// Go: ls/findallreferences.go:1081 isDeclarationOfSymbol
// PORT: reading `target.Declarations` takes the symbol arena.
pub fn is_declaration_of_symbol(symbols: &SymbolArena, node: Node, target: SymbolId) -> bool {
    if node.is_nil() || target.is_nil() {
        return false;
    }

    let source;
    let decl = get_declaration_from_name(node);
    if decl.is_some() {
        source = decl;
    } else if node.kind() == SyntaxKind::DefaultKeyword {
        source = node.parent();
    } else if is_literal_computed_property_declaration_name(node) {
        source = node.parent().parent();
    } else if node.kind() == SyntaxKind::ConstructorKeyword
        && is_constructor_declaration(node.parent())
    {
        source = node.parent().parent();
    } else {
        source = Node::NIL;
    }

    // !!!
    // const commonjsSource = source && isBinaryExpression(source) ? source.left as unknown as Declaration : undefined;

    source.is_some()
        && symbols
            .sym(target)
            .declarations
            .iter()
            .any(|&decl| decl == source)
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/findallreferences.go:1105 convertEntriesToLocations
    pub fn convert_entries_to_locations(
        &self,
        entries: &[Rc<RefCell<ReferenceEntry>>],
        feature: Feature,
    ) -> Vec<lsproto::Location> {
        let mut locations = Vec::with_capacity(entries.len());
        for entry in entries {
            let (location, ok) = self.get_location_of_entry_for_feature(entry, feature);
            if ok {
                locations.push(location);
            }
        }
        locations
    }

    // Go: ls/findallreferences.go:1116 convertEntriesToLocationLinks
    pub fn convert_entries_to_location_links(
        &self,
        entries: &[Rc<RefCell<ReferenceEntry>>],
        feature: Feature,
    ) -> Vec<lsproto::LocationLink> {
        let mut links = Vec::with_capacity(entries.len());
        for entry in entries {
            // Get the selection range (the actual reference)
            let (loc, ok) = self.get_location_of_entry_for_feature(entry, feature);
            if !ok {
                continue;
            }
            let target_selection_range = loc.range;
            let mut target_range = target_selection_range;

            let (entry_node, entry_text_range, entry_source_file, entry_context) = {
                let e = entry.borrow();
                (e.node, e.text_range, e.source_file, e.context)
            };

            // For entries with nodes, compute ranges directly from the node
            if entry_node.is_some() {
                // Get the context range (broader scope including declaration context)
                let context_text_range =
                    to_context_range(entry_text_range, entry_source_file, entry_context);
                if let Some(context_text_range) = context_text_range {
                    let (context_location, fidelity) = self
                        .source_file_range_to_lsp_location_for_feature(
                            entry_source_file,
                            context_text_range,
                            feature,
                        );
                    if !fidelity.is_none() && context_location.uri == loc.uri {
                        target_range = context_location.range;
                    }
                }
            }

            links.push(lsproto::LocationLink {
                target_uri: lsconv::file_name_to_document_uri(source_file_original_file_name(
                    entry_source_file,
                )),
                target_range,
                target_selection_range,
                ..Default::default()
            });
        }
        links
    }

    // Go: ls/findallreferences.go:1149 mergeReferences
    // PORT: Go variadic `referencesToMerge ...[]*SymbolAndEntries` is a
    // `Vec` of lists (a nil list is empty). Go returns a non-nil slice.
    pub fn merge_references(
        &self,
        program: &P,
        references_to_merge: Vec<Vec<Rc<RefCell<SymbolAndEntries>>>>,
    ) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
        let mut result: Vec<Rc<RefCell<SymbolAndEntries>>> = Vec::new();
        let get_source_file_index_of_entry =
            |program: &P, entry: &Rc<RefCell<ReferenceEntry>>| -> i32 {
                self.resolve_entry_source(entry);
                let source_file = entry.borrow().source_file;
                program.source_file_index(source_file)
            };

        for references in references_to_merge {
            if references.is_empty() {
                continue;
            }
            if result.is_empty() {
                result = references;
                continue;
            }
            for entry in references {
                let symbol = {
                    let e = entry.borrow();
                    match &e.definition {
                        Some(definition) if definition.kind == DefinitionKind::SYMBOL => {
                            Some(definition.symbol)
                        }
                        _ => None,
                    }
                };
                let Some(symbol) = symbol else {
                    result.push(entry);
                    continue;
                };
                let ref_index = result.iter().position(|r| {
                    r.borrow().definition.as_ref().is_some_and(|definition| {
                        definition.kind == DefinitionKind::SYMBOL && definition.symbol == symbol
                    })
                });
                let Some(ref_index) = ref_index else {
                    result.push(entry);
                    continue;
                };

                let reference = Rc::clone(&result[ref_index]);
                let mut sorted_refs = reference.borrow().references.clone();
                sorted_refs.extend(entry.borrow().references.iter().cloned());
                // Go: ls/findallreferences.go:1179 slices.SortStableFunc(sortedRefs, ...)
                crate::gostd::slices::sort_stable_func(&mut sorted_refs, |entry1, entry2| {
                    let entry1_file = get_source_file_index_of_entry(program, entry1);
                    let entry2_file = get_source_file_index_of_entry(program, entry2);
                    if entry1_file != entry2_file {
                        return entry1_file.cmp(&entry2_file) as i32;
                    }

                    lsproto::compare_ranges(
                        self.get_range_of_entry(entry1),
                        self.get_range_of_entry(entry2),
                    )
                });
                let definition = reference.borrow().definition.clone();
                result[ref_index] = Rc::new(RefCell::new(SymbolAndEntries {
                    definition,
                    references: sorted_refs,
                }));
            }
        }
        result
    }

    // Go: ls/findallreferences.go:1202 GetReferencedSymbolsForNode
    // GetReferencedSymbolsForNode returns all referenced symbols and their reference entries for the given node.
    // It returns all referenced symbols and their reference entries for the given node across the provided source files.
    // PORT: the unexported `getReferencedSymbolsForNode` has the same snake
    // name, so this one ends in `_exported`.
    pub fn get_referenced_symbols_for_node_exported(
        &self,
        ctx: &Context,
        position: i32,
        node: Node,
        source_files: &[Node],
    ) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
        self.get_referenced_symbols_for_node(
            ctx,
            position,
            node,
            &self.program,
            source_files,
            RefOptions {
                use_: ReferenceUse::REFERENCES,
                ..Default::default()
            },
        )
    }
}

// Go: ls/findallreferences.go:1210 SignatureUsage
// SignatureUsage represents a single usage of a signature declaration,
// pairing the reference name node with its containing call expression (if any).
#[derive(Clone, Copy, Debug, Default)]
pub struct SignatureUsage {
    pub name: Node, // The identifier reference node
    pub call: Node, // The containing call expression, or nil if not a call usage
}

impl LanguageService {
    // Go: ls/findallreferences.go:1218 GetSignatureUsages
    // GetSignatureUsages returns all usages of a signature declaration as name-call pairs.
    // For each reference to the signature's name, it returns the reference node and
    // the call expression it appears in (nil if the reference is not in a call position).
    pub fn get_signature_usages(&self, ctx: &Context, signature_decl: Node) -> Vec<SignatureUsage> {
        let name = signature_decl.name();
        if name.is_nil() || !is_identifier(name) {
            return Vec::new();
        }

        // PORT: Go `*ast.SourceFile` is the file root `Node`.
        let source_files: Vec<Node> = self
            .program
            .get_source_files()
            .iter()
            .map(|f| f.root)
            .collect();
        let entries =
            self.get_referenced_symbols_for_node_exported(ctx, name.pos(), name, &source_files);

        // Collect all declaration name nodes for the target symbol so we can
        // filter them out — the caller wants usages, not declarations.
        let mut decl_names: FxHashMap<Node, bool> = FxHashMap::default();
        {
            // PORT: Go reads `symbol.Declarations` directly; see the file header.
            let (checker, _done) = ls_program::get_type_checker(&self.program, ctx);
            let checker = checker.borrow();
            for entry in &entries {
                let entry = entry.borrow();
                if let Some(definition) = &entry.definition
                    && definition.symbol.is_some()
                {
                    for &decl in checker.sym(definition.symbol).declarations.iter() {
                        let n = decl.name();
                        if n.is_some() {
                            decl_names.insert(n, true);
                        }
                    }
                }
            }
        }

        let mut result: Vec<SignatureUsage> = Vec::new();
        for entry in &entries {
            let references = entry.borrow().references().to_vec();
            for ref_ in &references {
                let ref_ = ref_.borrow();
                if !ref_.is_node_entry() {
                    continue;
                }
                let node = ref_.node();
                if node.is_nil() || decl_names.get(&node).copied().unwrap_or(false) {
                    continue;
                }

                let called = climb_past_property_access(node);

                let mut call_expr = Node::NIL;
                if called.parent().is_some()
                    && is_call_expression(called.parent())
                    && called.parent().expression() == called
                {
                    call_expr = called.parent();
                }

                result.push(SignatureUsage {
                    name: node,
                    call: call_expr,
                });
            }
        }
        result
    }
}

impl<P: ProgramView> LanguageService<P> {
    // === functions for find all ref implementation ===

    // Go: ls/findallreferences.go:1269 getReferencedSymbolsForNode
    // PORT: Go holds the checker for the whole function. Here the checker is
    // borrowed around each use, and `getReferencedSymbolsForModule` gets it
    // as its first argument (ts#64543).
    pub fn get_referenced_symbols_for_node(
        &self,
        ctx: &Context,
        position: i32,
        mut node: Node,
        program: &P,
        source_files: &[Node],
        options: RefOptions,
    ) -> Vec<Rc<RefCell<SymbolAndEntries>>> {
        // !!! cancellationToken
        let mut source_files_set: FxHashSet<String> =
            FxHashSet::with_capacity_and_hasher(source_files.len(), Default::default());
        for &file in source_files {
            source_files_set.insert(source_file_file_name(file).to_string());
        }

        if options.use_ == ReferenceUse::REFERENCES || options.use_ == ReferenceUse::RENAME {
            node = get_adjusted_location(
                node,
                options.use_ == ReferenceUse::RENAME,
                get_source_file_of_node(node),
            );
        }

        let (checker, _done) = program.get_type_checker(ctx);

        if node.kind() == SyntaxKind::SourceFile {
            let resolved_ref = program.reference_at_position(node, position);
            let Some(resolved_ref) = resolved_ref else {
                return Vec::new();
            };
            if resolved_ref.file.is_nil() {
                return Vec::new();
            }

            let module_symbol = checker
                .borrow()
                .get_merged_symbol_exported(resolved_ref.file.symbol());
            if module_symbol.is_some() {
                return self.get_referenced_symbols_for_module(
                    &mut checker.borrow_mut(),
                    program,
                    module_symbol, /*excludeImportTypeOfExportEquals*/
                    false,
                    source_files,
                    &source_files_set,
                );
            }

            // !!! not implemented
            // fileIncludeReasons := program.getFileIncludeReasons();
            // if (!fileIncludeReasons) {
            // 	return nil
            // }
            return vec![Rc::new(RefCell::new(SymbolAndEntries {
                definition: Some(Definition {
                    kind: DefinitionKind::TRIPLE_SLASH_REFERENCE,
                    triple_slash_file_ref: Some(TripleSlashDefinition {
                        reference: resolved_ref.reference.clone(),
                        file: Node::NIL,
                    }),
                    ..Default::default()
                }),
                references: get_references_for_non_module(
                    resolved_ref.file,
                    program, /*fileIncludeReasons,*/
                ),
            }))];
        }

        if !options.implementations {
            // !!! cancellationToken
            let special = get_referenced_symbols_special(node, source_files);
            // PORT: Go returns nil or a non-empty slice here.
            if !special.is_empty() {
                return special;
            }
        }

        // constructors should use the class symbol, detected by name, if present
        let location = if node.kind() == SyntaxKind::Constructor && node.parent().name().is_some() {
            node.parent().name()
        } else {
            node
        };
        let symbol = checker
            .borrow_mut()
            .get_symbol_at_location_exported(location);
        // Could not find a symbol e.g. unknown identifier
        if symbol.is_nil() {
            // String literal might be a property (and thus have a symbol), so do this here rather than in getReferencedSymbolsSpecial.
            if !options.implementations && is_string_literal_like(node) {
                if is_module_specifier_like(node) {
                    // !!! not implemented
                    // fileIncludeReasons := program.GetFileIncludeReasons()
                    // if referencedFile := program.GetResolvedModuleFromModuleSpecifier(node, nil /*sourceFile*/); referencedFile != nil {
                    // return []*SymbolAndEntries{{
                    // 	definition: &Definition{Kind: definitionKindString, node: node},
                    // 	references: getReferencesForNonModule(referencedFile, program /*fileIncludeReasons,*/),
                    // }}
                    // }
                    // Fall through to string literal references. This is not very likely to return
                    // anything useful, but I guess it's better than nothing, and there's an existing
                    // test that expects this to happen (fourslash/cases/untypedModuleImport.ts).
                }
                return self.get_references_for_string_literal(
                    ctx,
                    node,
                    source_files,
                    &mut checker.borrow_mut(),
                );
            }
            return Vec::new();
        }

        let (symbol_name_is_export_equals, symbol_parent) = {
            let c = checker.borrow();
            let s = c.sym(symbol);
            (s.name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS, s.parent)
        };
        if symbol_name_is_export_equals {
            if symbol_parent.is_nil() {
                return Vec::new();
            }
            return self.get_referenced_symbols_for_module(
                &mut checker.borrow_mut(),
                program,
                symbol_parent,
                false, /*excludeImportTypeOfExportEquals*/
                source_files,
                &source_files_set,
            );
        }

        // PORT: Go tells nil from a non-nil empty slice here (a module with
        // no references returns an empty non-nil slice), so the result is an
        // `Option`.
        let module_references = self.get_referenced_symbols_for_module_if_declared_by_source_file(
            ctx,
            symbol,
            program,
            source_files,
            &mut checker.borrow_mut(),
            options,
            &source_files_set,
        );
        if let Some(module_references) = &module_references
            && !checker
                .borrow()
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::TRANSIENT)
        {
            return module_references.clone();
        }

        let aliased_symbol = get_merged_aliased_symbol_of_namespace_export_declaration(
            node,
            symbol,
            &mut checker.borrow_mut(),
        );
        let module_references_of_export_target = self
            .get_referenced_symbols_for_module_if_declared_by_source_file(
                ctx,
                aliased_symbol,
                program,
                source_files,
                &mut checker.borrow_mut(),
                options,
                &source_files_set,
            );

        let references = get_referenced_symbols_for_symbol(
            ctx,
            program,
            symbol,
            node,
            source_files,
            &source_files_set,
            &mut checker.borrow_mut(),
            options,
        );
        self.merge_references(
            program,
            vec![
                module_references.unwrap_or_default(),
                references,
                module_references_of_export_target.unwrap_or_default(),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
    use crate::frontend::tspath::Path;
    use crate::scanner_util::go_string_from_bytes;
    use std::fmt::Write;

    // PORT: no Go test. `getRangeOfNode` counts the length of a literal and
    // steps its end back in Go bytes. An unterminated literal can end in a
    // marker unit (an invalid byte, a byte of a WTF-8 surrogate or a real
    // U+FDD0), which has more port bytes than Go bytes. The expected lines
    // are from a Go test at pin N with Go 1.27.1 (`followups17/tools/gomodel`
    // in the lane dir, an overlay test file in package `ls`) that prints, for
    // each string literal like node in `ForEachChild` order, its pos and end
    // and the range of `getRangeOfNode`, in Go bytes.
    #[test]
    fn range_of_a_string_literal_counts_go_bytes() {
        let go = "\
R0 17-20 18-20
R1 17-21 19-20
R2 2-6 3-5
R3 2-7 3-6
R4 2-6 3-5
R5 2-7 3-6
R6 2-5 3-4
R7 2-5 3-4
R8 2-7 3-6
R9 2-7 3-6
R9 12-14 12-14
R9 19-22 20-21
R10 2-4 2-4
R11 2-7 3-6
R12 14-17 15-17
";
        let texts: [&[u8]; 13] = [
            b"import { v } from \"\xff\n",
            b"import { v } from \"a\xff\n",
            b"o[\"\xed\xa0\x80\n];\n",
            b"o[\"p\xed\xa0\x80\n];\n",
            b"o[\"\xef\xb7\x90\n];\n",
            b"o[\"p\xef\xb7\x90\n];\n",
            b"o[\"\\\xff\n];\n",
            b"o['\xc3\xa9\n];\n",
            b"o['\xf0\x9f\x98\x80\n];\n",
            b"o[\"a\xffb\"];\no[\"\"];\no[\"x\"];\n",
            b"o[`\xff",
            b"o[`p\xef\xb7\x90",
            b"declare module \"\xff\n{ }\n",
        ];
        let mut port = String::new();
        for (i, bytes) in texts.into_iter().enumerate() {
            let text = go_string_from_bytes(bytes.to_vec());
            let file = parse_source_file(
                &SourceFileParseOptions {
                    file_name: "/a.ts".to_string(),
                    path: Path("/a.ts".to_string()),
                    ..Default::default()
                },
                crate::ast::FileText::new(text.clone(), false),
                ScriptKind::TS,
            );
            let go = |pos: i32| go_byte_offset(&text, pos);
            fn visit(node: Node, f: &mut dyn FnMut(Node)) {
                f(node);
                node.for_each_child(|child| {
                    visit(child, f);
                    false
                });
            }
            file.root.for_each_child(|child| {
                visit(child, &mut |node| {
                    if is_string_literal_like(node) {
                        let range = get_range_of_node(node, file.root, Node::NIL);
                        writeln!(
                            port,
                            "R{i} {}-{} {}-{}",
                            go(node.pos()),
                            go(node.end()),
                            go(range.pos()),
                            go(range.end())
                        )
                        .unwrap();
                    }
                });
                false
            });
        }
        assert_eq!(port, go);
    }
}
