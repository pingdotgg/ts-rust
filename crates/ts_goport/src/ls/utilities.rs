//! Port of Go `ls/utilities.go`.
//!
//! PORT: Go package-private helpers here are called from other files of
//! package `ls`, so they are `pub`. Helpers that read symbol data and have no
//! checker parameter in Go take `symbols: &SymbolArena` first (the arena that
//! holds the symbol, normally `&checker.symbols`), as the ast rule in
//! PORTING.md does. Go `func(*ast.Symbol) *ast.Symbol` callbacks that run
//! while a checker is borrowed take the checker as their first argument.

use crate::ls::prelude::*;

use crate::frontend::parser::ParsedSourceFile;
use crate::spanmap::{Feature, Fidelity};
use std::borrow::Cow;
use std::cell::Cell;

// Go: ls/utilities.go:26 quoteReplacer
// PORT: Go `strings.NewReplacer("'", `\'`, `\"`, `"`)`. The pairs are kept in
// Go argument order; `quote_replacer_replace` is `Replace`.
const QUOTE_REPLACER: [(&str, &str); 2] = [("'", "\\'"), ("\\\"", "\"")];

/// Go `quoteReplacer.Replace(s)`: scans left to right; at each position the
/// first pair (in argument order) whose old string matches wins. Matches do
/// not overlap.
fn quote_replacer_replace(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut copied_from = 0;
    'outer: while i < bytes.len() {
        for (old, new) in QUOTE_REPLACER {
            if bytes[i..].starts_with(old.as_bytes()) {
                out.push_str(&s[copied_from..i]);
                out.push_str(new);
                i += old.len();
                copied_from = i;
                continue 'outer;
            }
        }
        i += 1;
    }
    // The old strings are ASCII, so `copied_from` is always a char boundary.
    out.push_str(&s[copied_from..]);
    out
}

// Go: ls/utilities.go:28 IsInString
pub fn is_in_string(source_file: Node, position: i32, previous_token: Node) -> bool {
    if previous_token.is_some() && is_string_text_containing_node(previous_token) {
        let start =
            astnav::get_start_of_node(previous_token, source_file, false /*includeJSDoc*/);
        let end = previous_token.end();

        // To be "in" one of these literals, the position has to be:
        //   1. entirely within the token text.
        //   2. at the end position of an unterminated token.
        //   3. at the end of a regular expression (due to trailing flags like '/foo/g').
        if start < position && position < end {
            return true;
        }

        if position == end {
            return is_unterminated_literal(previous_token);
        }
    }
    false
}

// Go: ls/utilities.go:48 isModuleSpecifierLike
pub fn is_module_specifier_like(node: Node) -> bool {
    if !is_string_literal_like(node) {
        return false;
    }

    if is_require_call(
        node.parent(),
        false, /*requireStringLiteralLikeArgument*/
    ) || is_import_call(node.parent())
    {
        return node.parent().arguments().get(0) == node;
    }

    node.parent().kind() == SyntaxKind::ExternalModuleReference
        || node.parent().kind() == SyntaxKind::ImportDeclaration
        || node.parent().kind() == SyntaxKind::JsImportDeclaration
}

// Go: ls/utilities.go:62 getNonModuleSymbolOfMergedModuleSymbol
pub fn get_non_module_symbol_of_merged_module_symbol(
    symbols: &SymbolArena,
    symbol: SymbolId,
) -> SymbolId {
    let s = symbols.sym(symbol);
    if s.declarations.is_empty()
        || !s
            .flags
            .intersects(SymbolFlags::MODULE | SymbolFlags::TRANSIENT)
    {
        return SymbolId::NIL;
    }

    if let Some(decl) = s
        .declarations
        .iter()
        .copied()
        .find(|&d| !is_source_file(d) && !is_module_declaration(d))
    {
        return decl.symbol();
    }
    SymbolId::NIL
}

// Go: ls/utilities.go:73 getLocalSymbolForExportSpecifier
// PORT: Go `*ast.ExportSpecifier` is the `Node`.
pub fn get_local_symbol_for_export_specifier(
    reference_location: Node,
    reference_symbol: SymbolId,
    export_specifier: Node,
    ch: &mut Checker,
) -> SymbolId {
    if is_export_specifier_alias(reference_location, export_specifier) {
        let symbol = ch.get_export_specifier_local_target_symbol(export_specifier);
        if symbol.is_some() {
            return symbol;
        }
    }
    reference_symbol
}

// Go: ls/utilities.go:82 isExportSpecifierAlias
pub fn is_export_specifier_alias(reference_location: Node, export_specifier: Node) -> bool {
    debug_assert!(
        export_specifier.property_name() == reference_location
            || export_specifier.name() == reference_location,
        "referenceLocation is not export specifier name or property name"
    );
    let property_name = export_specifier.property_name();
    if property_name.is_some() {
        // Given `export { foo as bar } [from "someModule"]`: It's an alias at `foo`, but at `bar` it's a new symbol.
        property_name == reference_location
    } else {
        // `export { foo } from "foo"` is a re-export.
        // `export { foo };` is not a re-export, it creates an alias for the local variable `foo`.
        export_specifier
            .parent()
            .parent()
            .module_specifier()
            .is_nil()
    }
}

// Go: ls/utilities.go:95 isInComment
// PORT: Go `*ast.CommentRange` is `Option<CommentRange>`.
pub fn is_in_comment(file: Node, position: i32, token_at_position: Node) -> Option<CommentRange> {
    get_range_of_enclosing_comment(
        file,
        position,
        astnav::find_preceding_token(file, position),
        token_at_position,
    )
}

// Go: ls/utilities.go:99 positionBelongsToNode
pub fn position_belongs_to_node(candidate: Node, position: i32, file: Node) -> bool {
    lsutil::position_belongs_to_node(candidate, position, file)
}

// Go: ls/utilities.go:103 PossibleTypeArgumentInfo
// PORT: the Go fields are package-private; other `ls` files read them.
#[derive(Clone, Copy, Debug, Default)]
pub struct PossibleTypeArgumentInfo {
    pub called: Node,
    pub n_type_arguments: i32,
}

// Go: ls/utilities.go:109 getPossibleTypeArgumentsInfo
// Get info for an expression like `f <` that may be the start of type arguments.
// PORT: Go returns `*PossibleTypeArgumentInfo`; nil is `None`.
pub fn get_possible_type_arguments_info(
    token_in: Node,
    source_file: Node,
) -> Option<PossibleTypeArgumentInfo> {
    // This is a rare case, but one that saves on a _lot_ of work if true - if the source file has _no_ `<` character,
    // then there obviously can't be any type arguments - no expensive brace-matching backwards scanning required
    if source_file_text(source_file).rfind('<').is_none() {
        return None;
    }

    let mut token = token_in;
    // This function determines if the node could be a type argument position
    // When editing, it is common to have an incomplete type argument list (e.g. missing ">"),
    // so the tree can have any shape depending on the tokens before the current node.
    // Instead, scanning for an identifier followed by a "<" before current node
    // will typically give us better results than inspecting the tree.
    // Note that we also balance out the already provided type arguments, arrays, object literals while doing so.
    let mut remaining_less_than_tokens = 0;
    let mut n_type_arguments = 0;
    while token.is_some() {
        match token.kind() {
            SyntaxKind::LessThanToken => {
                // Found the beginning of the generic argument expression
                token = astnav::find_preceding_token(source_file, token.pos());
                if token.is_some() && token.kind() == SyntaxKind::QuestionDotToken {
                    token = astnav::find_preceding_token(source_file, token.pos());
                }
                if token.is_nil() || !is_identifier(token) {
                    return None;
                }
                if remaining_less_than_tokens == 0 {
                    if is_declaration_name(token) {
                        return None;
                    }
                    return Some(PossibleTypeArgumentInfo {
                        called: token,
                        n_type_arguments,
                    });
                }
                remaining_less_than_tokens -= 1;
            }
            SyntaxKind::GreaterThanGreaterThanGreaterThanToken => {
                remaining_less_than_tokens += 3;
            }
            SyntaxKind::GreaterThanGreaterThanToken => {
                remaining_less_than_tokens += 2;
            }
            SyntaxKind::GreaterThanToken => {
                remaining_less_than_tokens += 1;
            }
            SyntaxKind::CloseBraceToken => {
                // This can be object type, skip until we find the matching open brace token
                // Skip until the matching open brace token
                token =
                    find_preceding_matching_token(token, SyntaxKind::OpenBraceToken, source_file);
                if token.is_nil() {
                    return None;
                }
            }
            SyntaxKind::CloseParenToken => {
                // This can be object type, skip until we find the matching open brace token
                // Skip until the matching open brace token
                token =
                    find_preceding_matching_token(token, SyntaxKind::OpenParenToken, source_file);
                if token.is_nil() {
                    return None;
                }
            }
            SyntaxKind::CloseBracketToken => {
                // This can be object type, skip until we find the matching open brace token
                // Skip until the matching open brace token
                token =
                    find_preceding_matching_token(token, SyntaxKind::OpenBracketToken, source_file);
                if token.is_nil() {
                    return None;
                }
            }
            SyntaxKind::CommaToken => {
                // Valid tokens in a type name. Skip.
                n_type_arguments += 1;
            }
            SyntaxKind::EqualsGreaterThanToken
            | SyntaxKind::Identifier
            | SyntaxKind::StringLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::TypeOfKeyword
            | SyntaxKind::ExtendsKeyword
            | SyntaxKind::KeyOfKeyword
            | SyntaxKind::DotToken
            | SyntaxKind::BarToken
            | SyntaxKind::QuestionToken
            | SyntaxKind::ColonToken => {
                // do nothing
            }
            _ => {
                if !is_type_node(token) {
                    // Invalid token in type
                    return None;
                }
            }
        }
        token = astnav::find_preceding_token(source_file, token.pos());
    }
    None
}

// Go: ls/utilities.go:191 isNameOfModuleDeclaration
pub fn is_name_of_module_declaration(node: Node) -> bool {
    if node.parent().kind() != SyntaxKind::ModuleDeclaration {
        return false;
    }
    node.parent().name() == node
}

// Go: ls/utilities.go:198 isExpressionOfExternalModuleImportEqualsDeclaration
pub fn is_expression_of_external_module_import_equals_declaration(node: Node) -> bool {
    is_external_module_import_equals_declaration(node.parent().parent())
        && get_external_module_import_equals_declaration_expression(node.parent().parent()) == node
}

// Go: ls/utilities.go:202 isNamespaceReference
pub fn is_namespace_reference(node: Node) -> bool {
    is_qualified_name_namespace_reference(node) || is_property_access_namespace_reference(node)
}

// Go: ls/utilities.go:206 isQualifiedNameNamespaceReference
pub fn is_qualified_name_namespace_reference(node: Node) -> bool {
    let mut root = node;
    let mut is_last_clause = true;
    if root.parent().kind() == SyntaxKind::QualifiedName {
        while root.parent().is_some() && root.parent().kind() == SyntaxKind::QualifiedName {
            root = root.parent();
        }

        is_last_clause = root.right() == node;
    }

    root.parent().kind() == SyntaxKind::TypeReference && !is_last_clause
}

// Go: ls/utilities.go:220 isPropertyAccessNamespaceReference
pub fn is_property_access_namespace_reference(node: Node) -> bool {
    let mut root = node;
    let mut is_last_clause = true;
    if root.parent().kind() == SyntaxKind::PropertyAccessExpression {
        while root.parent().is_some()
            && root.parent().kind() == SyntaxKind::PropertyAccessExpression
        {
            root = root.parent();
        }

        is_last_clause = root.name() == node;
    }

    if !is_last_clause
        && root.parent().kind() == SyntaxKind::ExpressionWithTypeArguments
        && root.parent().parent().kind() == SyntaxKind::HeritageClause
    {
        let decl = root.parent().parent().parent();
        return (decl.kind() == SyntaxKind::ClassDeclaration
            && root.parent().parent().token() == SyntaxKind::ImplementsKeyword)
            || (decl.kind() == SyntaxKind::InterfaceDeclaration
                && root.parent().parent().token() == SyntaxKind::ExtendsKeyword);
    }

    false
}

// Go: ls/utilities.go:240 isThis
pub fn is_this(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ThisKeyword => {
            // case ast.KindThisType: TODO: GH#9267
            true
        }
        SyntaxKind::Identifier => {
            // 'this' as a parameter
            node.text() == "this" && node.parent().kind() == SyntaxKind::Parameter
        }
        _ => false,
    }
}

// Go: ls/utilities.go:253 isTypeReference
pub fn is_type_reference(node: Node) -> bool {
    let mut node = node;
    if is_right_side_of_qualified_name_or_property_access(node) {
        node = node.parent();
    }

    match node.kind() {
        SyntaxKind::ThisKeyword => return !is_expression_node(node),
        SyntaxKind::ThisType => return true,
        _ => {}
    }

    match node.parent().kind() {
        SyntaxKind::TypeReference => return true,
        SyntaxKind::ImportType => return !node.parent().is_type_of(),
        SyntaxKind::ExpressionWithTypeArguments => return is_part_of_type_node(node.parent()),
        _ => {}
    }

    false
}

// Go: ls/utilities.go:277 isInRightSideOfInternalImportEqualsDeclaration
pub fn is_in_right_side_of_internal_import_equals_declaration(node: Node) -> bool {
    let mut node = node;
    if node.parent().is_nil() {
        return false;
    }
    while node.parent().kind() == SyntaxKind::QualifiedName {
        node = node.parent();
    }

    is_internal_module_import_equals_declaration(node.parent())
        && node.parent().module_reference() == node
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/utilities.go:288 createLspRangeFromNode
    pub fn create_lsp_range_from_node(&self, node: Node, file: Node) -> (lsproto::Range, Fidelity) {
        self.create_lsp_range_from_bounds(
            get_token_pos_of_node(node, file, false /*includeJSDoc*/),
            node.end(),
            file,
        )
    }

    // Go: ls/utilities.go:292 createLspRangeFromNodeForFeature
    pub fn create_lsp_range_from_node_for_feature(
        &self,
        node: Node,
        file: Node,
        feature: Feature,
    ) -> (lsproto::Range, Fidelity) {
        self.converters
            .to_lsp_range_for_feature(&file, create_range_from_node(node, file), feature)
    }
}

// Go: ls/utilities.go:296 createRangeFromNode
pub fn create_range_from_node(node: Node, file: Node) -> TextRange {
    TextRange::new(
        get_token_pos_of_node(node, file, false /*includeJSDoc*/),
        node.end(),
    )
}

impl<P: ProgramView> LanguageService<P> {
    // Go: ls/utilities.go:300 createLspRangeFromBounds
    // PORT: Go passes the `*ast.SourceFile` as an `lsconv.Script`.
    pub fn create_lsp_range_from_bounds(
        &self,
        start: i32,
        end: i32,
        file: Node,
    ) -> (lsproto::Range, Fidelity) {
        self.converters
            .to_lsp_range(&file, TextRange::new(start, end))
    }

    // Go: ls/utilities.go:304 createLspRangeFromRange
    pub fn create_lsp_range_from_range(
        &self,
        text_range: TextRange,
        script: &dyn lsconv::Script,
    ) -> (lsproto::Range, Fidelity) {
        self.converters.to_lsp_range(script, text_range)
    }

    // Go: ls/utilities.go:308 createLspPosition
    pub fn create_lsp_position(&self, position: i32, file: Node) -> (lsproto::Position, Fidelity) {
        self.converters.to_lsp_position(&file, position)
    }
}

// Go: ls/utilities.go:312 quote
// PORT: Go passes the preferences by value; here by reference.
pub fn quote(file: Node, preferences: &lsutil::UserPreferences, text: &str) -> String {
    // Editors can pass in undefined or empty string - we want to infer the preference in those cases.
    let quote_preference = lsutil::get_quote_preference(file, preferences);
    // PORT: Go `core.StringifyJson(text, "", "")` ignores the error, which
    // leaves "" on failure.
    let mut quoted =
        crate::frontend::json::json_marshal_indent(text, "" /*prefix*/, "" /*indent*/)
            .unwrap_or_default();
    if quote_preference == lsutil::QuotePreference::SINGLE {
        quoted = "'".to_string() + &quote_replacer_replace(&strip_quotes(&quoted)) + "'";
    }
    quoted
}

// Go: ls/utilities.go:322 typeKeywords
// PORT: Go `*collections.Set[ast.Kind]`; membership is all that is read.
pub static TYPE_KEYWORDS: [SyntaxKind; 20] = [
    SyntaxKind::AnyKeyword,
    SyntaxKind::AssertsKeyword,
    SyntaxKind::BigIntKeyword,
    SyntaxKind::BooleanKeyword,
    SyntaxKind::FalseKeyword,
    SyntaxKind::InferKeyword,
    SyntaxKind::KeyOfKeyword,
    SyntaxKind::NeverKeyword,
    SyntaxKind::NullKeyword,
    SyntaxKind::NumberKeyword,
    SyntaxKind::ObjectKeyword,
    SyntaxKind::ReadonlyKeyword,
    SyntaxKind::StringKeyword,
    SyntaxKind::SymbolKeyword,
    SyntaxKind::TypeOfKeyword,
    SyntaxKind::TrueKeyword,
    SyntaxKind::VoidKeyword,
    SyntaxKind::UndefinedKeyword,
    SyntaxKind::UniqueKeyword,
    SyntaxKind::UnknownKeyword,
];

// Go: ls/utilities.go:345 isTypeKeyword
pub fn is_type_keyword(kind: SyntaxKind) -> bool {
    TYPE_KEYWORDS.contains(&kind)
}

// Go: ls/utilities.go:349 isSeparator
pub fn is_separator(node: Node, candidate: Node) -> bool {
    candidate.is_some()
        && node.parent().is_some()
        && (candidate.kind() == SyntaxKind::CommaToken
            || (candidate.kind() == SyntaxKind::SemicolonToken
                && node.parent().kind() == SyntaxKind::ObjectLiteralExpression))
}

// Go: ls/utilities.go:353 isLiteralNameOfPropertyDeclarationOrIndexAccess
pub fn is_literal_name_of_property_declaration_or_index_access(node: Node) -> bool {
    // utilities
    match node.parent().kind() {
        SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::PropertyAssignment
        | SyntaxKind::EnumMember
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::ModuleDeclaration => get_name_of_declaration(node.parent()) == node,
        SyntaxKind::ElementAccessExpression => node.parent().argument_expression() == node,
        SyntaxKind::ComputedPropertyName => true,
        SyntaxKind::LiteralType => node.parent().parent().kind() == SyntaxKind::IndexedAccessType,
        _ => false,
    }
}

// Go: ls/utilities.go:377 isObjectBindingElementWithoutPropertyName
pub fn is_object_binding_element_without_property_name(binding_element: Node) -> bool {
    binding_element.kind() == SyntaxKind::BindingElement
        && binding_element.parent().kind() == SyntaxKind::ObjectBindingPattern
        && binding_element.name().kind() == SyntaxKind::Identifier
        && binding_element.property_name().is_nil()
}

// Go: ls/utilities.go:384 isRightSideOfPropertyAccess
pub fn is_right_side_of_property_access(node: Node) -> bool {
    node.parent().is_some()
        && node.parent().kind() == SyntaxKind::PropertyAccessExpression
        && node.parent().name() == node
}

// Go: ls/utilities.go:388 isStaticSymbol
pub fn is_static_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    let value_declaration = symbols.sym(symbol).value_declaration;
    if value_declaration.is_nil() {
        return false;
    }
    let modifier_flags = value_declaration.modifier_flags();
    modifier_flags.intersects(ModifierFlags::STATIC)
}

// Go: ls/utilities.go:396 isImplementation
pub fn is_implementation(node: Node) -> bool {
    if node.flags().intersects(NodeFlags::AMBIENT) {
        return !(node.kind() == SyntaxKind::InterfaceDeclaration
            || node.kind() == SyntaxKind::TypeAliasDeclaration);
    }
    // PORT: Go `ast.IsVariableLike`. The ls prelude picks the ls
    // `is_variable_like` (callhierarchy.go), so the ast one is named in full.
    if crate::ast::is_variable_like(node) {
        return has_initializer(node);
    }
    if is_function_like_declaration(node) {
        return node.body().is_some();
    }
    is_class_like(node) || is_module_or_enum_declaration(node)
}

// Go: ls/utilities.go:409 isImplementationExpression
pub fn is_implementation_expression(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ParenthesizedExpression => is_implementation_expression(node.expression()),
        SyntaxKind::ArrowFunction
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ObjectLiteralExpression
        | SyntaxKind::ClassExpression
        | SyntaxKind::ArrayLiteralExpression => true,
        _ => false,
    }
}

// Go: ls/utilities.go:420 isReadonlyTypeOperator
pub fn is_readonly_type_operator(node: Node) -> bool {
    node.kind() == SyntaxKind::ReadonlyKeyword
        && node.parent().kind() == SyntaxKind::TypeOperator
        && node.parent().operator() == SyntaxKind::ReadonlyKeyword
}

// Go: ls/utilities.go:424 isJumpStatementTarget
pub fn is_jump_statement_target(node: Node) -> bool {
    node.kind() == SyntaxKind::Identifier
        && is_break_or_continue_statement(node.parent())
        && node.parent().label() == node
}

// Go: ls/utilities.go:428 isLabelOfLabeledStatement
pub fn is_label_of_labeled_statement(node: Node) -> bool {
    node.kind() == SyntaxKind::Identifier
        && node.parent().kind() == SyntaxKind::LabeledStatement
        && node.parent().label() == node
}

// Go: ls/utilities.go:432 findReferenceInPosition
// PORT: Go returns the `*ast.FileReference` element; nil is `None`.
pub fn find_reference_in_position(refs: &[FileReference], pos: i32) -> Option<&FileReference> {
    refs.iter().find(|r| r.range.contains_inclusive(pos))
}

// Go: ls/utilities.go:436 getContainingNodeIfInHeritageClause
pub fn get_containing_node_if_in_heritage_clause(node: Node) -> Node {
    if node.kind() == SyntaxKind::Identifier
        || node.kind() == SyntaxKind::QualifiedName
        || node.kind() == SyntaxKind::PropertyAccessExpression
    {
        return get_containing_node_if_in_heritage_clause(node.parent());
    }
    if (node.kind() == SyntaxKind::ExpressionWithTypeArguments
        || node.kind() == SyntaxKind::TypeReference)
        && is_heritage_clause(node.parent())
        && (is_class_like(node.parent().parent())
            || node.parent().parent().kind() == SyntaxKind::InterfaceDeclaration)
    {
        return node.parent().parent();
    }
    Node::NIL
}

// Go: ls/utilities.go:448 getContainerNode
pub fn get_container_node(node: Node) -> Node {
    let mut parent = node.parent();
    while parent.is_some() {
        match parent.kind() {
            SyntaxKind::SourceFile
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::ModuleDeclaration => return parent,
            _ => {}
        }
        parent = parent.parent();
    }
    Node::NIL
}

// Go: ls/utilities.go:459 getAdjustedLocation
// PORT: Go `sourceFile` may be nil (`Node::NIL`).
pub fn get_adjusted_location(node: Node, for_rename: bool, source_file: Node) -> Node {
    // todo: check if this function needs to be changed for jsdoc updates

    let mut source_file = source_file;
    let parent = node.parent();
    // /**/<modifier> [|name|] ...
    // /**/<modifier> <class|interface|type|enum|module|namespace|function|get|set> [|name|] ...
    // /**/<class|interface|type|enum|module|namespace|function|get|set> [|name|] ...
    // /**/import [|name|] = ...
    //
    // NOTE: If the node is a modifier, we don't adjust its location if it is the `default` modifier as that is handled
    // specially by `getSymbolAtLocation`.
    let is_modifier = |node: Node| -> bool {
        if is_modifier(node) && (for_rename || node.kind() != SyntaxKind::DefaultKeyword) {
            return can_have_modifiers(parent) && parent.modifier_nodes().iter().any(|m| m == node);
        }
        match node.kind() {
            // PORT: Go checks `ast.IsClassExpression(node)` (the keyword),
            // not the parent. Kept as is.
            SyntaxKind::ClassKeyword => is_class_declaration(parent) || is_class_expression(node),
            // PORT: Go checks `ast.IsFunctionExpression(node)`. Kept as is.
            SyntaxKind::FunctionKeyword => {
                is_function_declaration(parent) || is_function_expression(node)
            }
            SyntaxKind::InterfaceKeyword => is_interface_declaration(parent),
            SyntaxKind::EnumKeyword => is_enum_declaration(parent),
            SyntaxKind::TypeKeyword => is_type_alias_declaration(parent),
            SyntaxKind::NamespaceKeyword | SyntaxKind::ModuleKeyword => {
                is_module_declaration(parent)
            }
            SyntaxKind::ImportKeyword => is_import_equals_declaration(parent),
            SyntaxKind::GetKeyword => is_get_accessor_declaration(parent),
            SyntaxKind::SetKeyword => is_set_accessor_declaration(parent),
            _ => false,
        }
    };
    if is_modifier(node) {
        if source_file.is_nil() {
            source_file = get_source_file_of_node(node);
        }
        let location = get_adjusted_location_for_declaration(parent, for_rename, source_file);
        if location.is_some() {
            return location;
        }
    }

    // /**/<var|let|const> [|name|] ...
    if (node.kind() == SyntaxKind::VarKeyword
        || node.kind() == SyntaxKind::ConstKeyword
        || node.kind() == SyntaxKind::LetKeyword)
        && is_variable_declaration_list(parent)
        && parent.declarations().nodes().len() == 1
    {
        let declaration = parent.declarations().nodes().get(0);
        if is_identifier(declaration.name()) {
            return declaration.name();
        }
    }

    if node.kind() == SyntaxKind::TypeKeyword {
        // import /**/type [|name|] from ...;
        // import /**/type { [|name|] } from ...;
        // import /**/type { propertyName as [|name|] } from ...;
        // import /**/type ... from "[|module|]";
        if is_import_clause(parent) && parent.is_type_only() {
            let location =
                get_adjusted_location_for_import_declaration(parent.parent(), for_rename);
            if location.is_some() {
                return location;
            }
        }
        // export /**/type { [|name|] } from ...;
        // export /**/type { propertyName as [|name|] } from ...;
        // export /**/type * from "[|module|]";
        // export /**/type * as ... from "[|module|]";
        if is_export_declaration(parent) && parent.is_type_only() {
            let location = get_adjusted_location_for_export_declaration(parent, for_rename);
            if location.is_some() {
                return location;
            }
        }
    }

    // import { propertyName /**/as [|name|] } ...
    // import * /**/as [|name|] ...
    // export { propertyName /**/as [|name|] } ...
    // export * /**/as [|name|] ...
    if node.kind() == SyntaxKind::AsKeyword {
        if parent.kind() == SyntaxKind::ImportSpecifier && parent.property_name().is_some()
            || parent.kind() == SyntaxKind::ExportSpecifier && parent.property_name().is_some()
            || parent.kind() == SyntaxKind::NamespaceImport
            || parent.kind() == SyntaxKind::NamespaceExport
        {
            return parent.name();
        }
        if parent.kind() == SyntaxKind::ExportDeclaration {
            let export_clause = parent.export_clause();
            if export_clause.is_some() && export_clause.kind() == SyntaxKind::NamespaceExport {
                return export_clause.name();
            }
        }
    }

    // /**/import [|name|] from ...;
    // /**/import { [|name|] } from ...;
    // /**/import { propertyName as [|name|] } from ...;
    // /**/import ... from "[|module|]";
    // /**/import "[|module|]";
    if node.kind() == SyntaxKind::ImportKeyword && parent.kind() == SyntaxKind::ImportDeclaration {
        let location = get_adjusted_location_for_import_declaration(parent, for_rename);
        if location.is_some() {
            return location;
        }
    }

    if node.kind() == SyntaxKind::ExportKeyword {
        // /**/export { [|name|] } ...;
        // /**/export { propertyName as [|name|] } ...;
        // /**/export * from "[|module|]";
        // /**/export * as ... from "[|module|]";
        if parent.kind() == SyntaxKind::ExportDeclaration {
            let location = get_adjusted_location_for_export_declaration(parent, for_rename);
            if location.is_some() {
                return location;
            }
        }
        // NOTE: We don't adjust the location of the `default` keyword as that is handled specially by `getSymbolAtLocation`.
        // /**/export default [|name|];
        // /**/export = [|name|];
        if parent.kind() == SyntaxKind::ExportAssignment {
            return skip_outer_expressions(parent.expression(), OuterExpressionKinds::OEK_ALL);
        }
    }
    // import name = /**/require("[|module|]");
    if node.kind() == SyntaxKind::RequireKeyword
        && parent.kind() == SyntaxKind::ExternalModuleReference
    {
        return parent.expression();
    }
    // import ... /**/from "[|module|]";
    // export ... /**/from "[|module|]";
    if node.kind() == SyntaxKind::FromKeyword
        && (parent.kind() == SyntaxKind::ImportDeclaration
            || parent.kind() == SyntaxKind::ExportDeclaration)
        && parent.module_specifier().is_some()
    {
        return parent.module_specifier();
    }
    // class ... /**/extends [|name|] ...
    // class ... /**/implements [|name|] ...
    // class ... /**/implements name1, name2 ...
    // interface ... /**/extends [|name|] ...
    // interface ... /**/extends name1, name2 ...
    if (node.kind() == SyntaxKind::ExtendsKeyword || node.kind() == SyntaxKind::ImplementsKeyword)
        && parent.kind() == SyntaxKind::HeritageClause
        && parent.token() == node.kind()
    {
        let get_adjusted_location_for_heritage_clause = |node: Node| -> Node {
            // /**/extends [|name|]
            // /**/implements [|name|]
            if node.types().nodes().len() == 1 {
                return get_heritage_clause_element_name(node.types().nodes().get(0));
            }

            // fall through `getAdjustedLocation`
            //    /**/extends name1, name2 ...
            //    /**/implements name1, name2 ...
            Node::NIL
        };

        let location = get_adjusted_location_for_heritage_clause(parent);
        if location.is_some() {
            return location;
        }
    }
    if node.kind() == SyntaxKind::ExtendsKeyword {
        // ... <T /**/extends [|U|]> ...
        if parent.kind() == SyntaxKind::TypeParameter {
            let constraint = parent.constraint();
            if constraint.is_some() && constraint.kind() == SyntaxKind::TypeReference {
                return constraint.type_name();
            }
        }
        // ... T /**/extends [|U|] ? ...
        if parent.kind() == SyntaxKind::ConditionalType {
            let extends_type = parent.extends_type();
            if extends_type.is_some() && extends_type.kind() == SyntaxKind::TypeReference {
                return extends_type.type_name();
            }
        }
    }
    // ... T extends /**/infer [|U|] ? ...
    if node.kind() == SyntaxKind::InferKeyword && parent.kind() == SyntaxKind::InferType {
        return parent.type_parameter().name();
    }
    // { [ [|K|] /**/in keyof T]: ... }
    if node.kind() == SyntaxKind::InKeyword
        && parent.kind() == SyntaxKind::TypeParameter
        && parent.parent().kind() == SyntaxKind::MappedType
    {
        return parent.name();
    }
    // /**/keyof [|T|]
    if node.kind() == SyntaxKind::KeyOfKeyword
        && parent.kind() == SyntaxKind::TypeOperator
        && parent.operator() == SyntaxKind::KeyOfKeyword
    {
        let parent_type = parent.type_();
        if parent_type.is_some() && parent_type.kind() == SyntaxKind::TypeReference {
            return parent_type.type_name();
        }
    }
    // /**/readonly [|name|][]
    if node.kind() == SyntaxKind::ReadonlyKeyword
        && parent.kind() == SyntaxKind::TypeOperator
        && parent.operator() == SyntaxKind::ReadonlyKeyword
    {
        let parent_type = parent.type_();
        if parent_type.is_some()
            && parent_type.kind() == SyntaxKind::ArrayType
            && parent_type.element_type().kind() == SyntaxKind::TypeReference
        {
            return parent_type.element_type().type_name();
        }
    }

    if !for_rename {
        // /**/new [|name|]
        // /**/void [|name|]
        // /**/void obj.[|name|]
        // /**/typeof [|name|]
        // /**/typeof obj.[|name|]
        // /**/await [|name|]
        // /**/await obj.[|name|]
        // /**/yield [|name|]
        // /**/yield obj.[|name|]
        // /**/delete obj.[|name|]
        if node.kind() == SyntaxKind::NewKeyword && parent.kind() == SyntaxKind::NewExpression
            || node.kind() == SyntaxKind::VoidKeyword && parent.kind() == SyntaxKind::VoidExpression
            || node.kind() == SyntaxKind::TypeOfKeyword
                && parent.kind() == SyntaxKind::TypeOfExpression
            || node.kind() == SyntaxKind::AwaitKeyword
                && parent.kind() == SyntaxKind::AwaitExpression
            || node.kind() == SyntaxKind::YieldKeyword
                && parent.kind() == SyntaxKind::YieldExpression
            || node.kind() == SyntaxKind::DeleteKeyword
                && parent.kind() == SyntaxKind::DeleteExpression
        {
            let expr = parent.expression();
            if expr.is_some() {
                return skip_outer_expressions(expr, OuterExpressionKinds::OEK_ALL);
            }
        }

        // left /**/in [|name|]
        // left /**/instanceof [|name|]
        if (node.kind() == SyntaxKind::InKeyword || node.kind() == SyntaxKind::InstanceOfKeyword)
            && parent.kind() == SyntaxKind::BinaryExpression
            && parent.operator_token() == node
        {
            return skip_outer_expressions(parent.right(), OuterExpressionKinds::OEK_ALL);
        }

        // left /**/as [|name|]
        if node.kind() == SyntaxKind::AsKeyword && parent.kind() == SyntaxKind::AsExpression {
            let as_expr_type = parent.type_();
            if as_expr_type.is_some() && as_expr_type.kind() == SyntaxKind::TypeReference {
                return as_expr_type.type_name();
            }
        }

        // for (... /**/in [|name|])
        // for (... /**/of [|name|])
        if node.kind() == SyntaxKind::InKeyword && parent.kind() == SyntaxKind::ForInStatement
            || node.kind() == SyntaxKind::OfKeyword && parent.kind() == SyntaxKind::ForOfStatement
        {
            return skip_outer_expressions(parent.expression(), OuterExpressionKinds::OEK_ALL);
        }
    }

    node
}

// Go: ls/utilities.go:696 getAdjustedLocationForDeclaration
pub fn get_adjusted_location_for_declaration(
    node: Node,
    for_rename: bool,
    source_file: Node,
) -> Node {
    if node.name().is_some() {
        return node.name();
    }
    if for_rename {
        return Node::NIL;
    }
    match node.kind() {
        SyntaxKind::ClassDeclaration | SyntaxKind::FunctionDeclaration => {
            // for class and function declarations, use the `default` modifier
            // when the declaration is unnamed.
            // PORT: the Go predicate reads `node.Kind` (the declaration), not
            // the modifier. Kept as is.
            node.modifier_nodes()
                .iter()
                .find(|_| node.kind() == SyntaxKind::DefaultKeyword)
                .unwrap_or(Node::NIL)
        }
        SyntaxKind::ClassExpression => {
            // for class expressions, use the `class` keyword when the class is unnamed
            astnav::find_child_of_kind(node, SyntaxKind::ClassKeyword, source_file)
        }
        SyntaxKind::FunctionExpression => {
            // for function expressions, use the `function` keyword when the function is unnamed
            astnav::find_child_of_kind(node, SyntaxKind::FunctionKeyword, source_file)
        }
        SyntaxKind::Constructor => node,
        _ => Node::NIL,
    }
}

// Go: ls/utilities.go:720 getAdjustedLocationForImportDeclaration
// PORT: Go `*ast.ImportDeclaration` is the `Node`.
pub fn get_adjusted_location_for_import_declaration(node: Node, for_rename: bool) -> Node {
    let import_clause = node.import_clause();
    if import_clause.is_some() {
        let name = import_clause.name();
        if name.is_some() {
            if import_clause.named_bindings().is_some() {
                // do not adjust if we have both a name and named bindings
                return Node::NIL;
            }
            // /**/import [|name|] from ...;
            // import /**/type [|name|] from ...;
            return import_clause.name();
        }

        // /**/import { [|name|] } from ...;
        // /**/import { propertyName as [|name|] } from ...;
        // /**/import * as [|name|] from ...;
        // import /**/type { [|name|] } from ...;
        // import /**/type { propertyName as [|name|] } from ...;
        // import /**/type * as [|name|] from ...;
        let named_bindings = import_clause.named_bindings();
        if named_bindings.is_some() {
            match named_bindings.kind() {
                SyntaxKind::NamedImports => {
                    // do nothing if there is more than one binding
                    let elements = named_bindings.elements();
                    if elements.len() != 1 {
                        return Node::NIL;
                    }
                    return elements.get(0).name();
                }
                SyntaxKind::NamespaceImport => {
                    return named_bindings.name();
                }
                _ => {}
            }
        }
    }
    if !for_rename {
        // /**/import "[|module|]";
        // /**/import ... from "[|module|]";
        // import /**/type ... from "[|module|]";
        return node.module_specifier();
    }
    Node::NIL
}

// Go: ls/utilities.go:763 getAdjustedLocationForExportDeclaration
// PORT: Go `*ast.ExportDeclaration` is the `Node`.
pub fn get_adjusted_location_for_export_declaration(node: Node, for_rename: bool) -> Node {
    let export_clause = node.export_clause();
    if export_clause.is_some() {
        // /**/export { [|name|] } ...
        // /**/export { propertyName as [|name|] } ...
        // /**/export * as [|name|] ...
        // export /**/type { [|name|] } from ...
        // export /**/type { propertyName as [|name|] } from ...
        // export /**/type * as [|name|] ...
        match export_clause.kind() {
            SyntaxKind::NamedExports => {
                // do nothing if there is more than one binding
                let elements = export_clause.elements();
                if elements.len() != 1 {
                    return Node::NIL;
                }
                return elements.get(0).name();
            }
            SyntaxKind::NamespaceExport => {
                return export_clause.name();
            }
            _ => {}
        }
    }
    if !for_rename {
        // /**/export * from "[|module|]";
        // export /**/type * from "[|module|]";
        return node.module_specifier();
    }
    Node::NIL
}

// Go: ls/utilities.go:791 symbolFlagsHaveMeaning
pub fn symbol_flags_have_meaning(flags: SymbolFlags, meaning: SemanticMeaning) -> bool {
    if meaning == SemanticMeaning::ALL {
        return true;
    }
    if meaning.intersects(SemanticMeaning::VALUE) {
        return flags.intersects(SymbolFlags::VALUE);
    }
    if meaning.intersects(SemanticMeaning::TYPE) {
        return flags.intersects(SymbolFlags::TYPE);
    }
    if meaning.intersects(SemanticMeaning::NAMESPACE) {
        return flags.intersects(SymbolFlags::NAMESPACE);
    }
    false
}

// Go: ls/utilities.go:807 getMeaningFromLocation
pub fn get_meaning_from_location(node: Node) -> SemanticMeaning {
    // todo: check if this function needs to be changed for jsdoc updates
    let node = get_adjusted_location(
        get_reparsed_node_for_node(node),
        false, /*forRename*/
        Node::NIL,
    );
    let parent = node.parent();
    if is_source_file(node) {
        SemanticMeaning::VALUE
    } else if node_kind_is(
        parent,
        &[
            SyntaxKind::ExportAssignment,
            SyntaxKind::ExportSpecifier,
            SyntaxKind::ExternalModuleReference,
            SyntaxKind::ImportSpecifier,
            SyntaxKind::ImportClause,
        ],
    ) || parent.kind() == SyntaxKind::ImportEqualsDeclaration && node == parent.name()
    {
        SemanticMeaning::ALL
    } else if is_in_right_side_of_internal_import_equals_declaration(node) {
        //     import a = |b|; // Namespace
        //     import a = |b.c|; // Value, type, namespace
        //     import a = |b.c|.d; // Namespace
        let mut name = node;
        if node.kind() != SyntaxKind::QualifiedName {
            name = if node.parent().kind() == SyntaxKind::QualifiedName
                && node.parent().right() == node
            {
                node.parent()
            } else {
                Node::NIL
            };
        }
        if name.is_some() && name.parent().kind() == SyntaxKind::ImportEqualsDeclaration {
            return SemanticMeaning::ALL;
        }
        SemanticMeaning::NAMESPACE
    } else if is_declaration_name(node) {
        get_meaning_from_declaration(parent)
    } else if is_entity_name(node) && is_js_doc_name_reference_context(node) {
        SemanticMeaning::ALL
    } else if is_type_reference(node) {
        SemanticMeaning::TYPE
    } else if is_namespace_reference(node) {
        SemanticMeaning::NAMESPACE
    } else if is_type_parameter_declaration(parent) {
        SemanticMeaning::TYPE
    } else if is_literal_type_node(parent) {
        // This might be T["name"], which is actually referencing a property and not a type. So allow both meanings.
        SemanticMeaning::TYPE | SemanticMeaning::VALUE
    } else {
        SemanticMeaning::VALUE
    }
}

// Go: ls/utilities.go:846 getMeaningFromDeclaration
pub fn get_meaning_from_declaration(node: Node) -> SemanticMeaning {
    match node.kind() {
        SyntaxKind::VariableDeclaration
        | SyntaxKind::Parameter
        | SyntaxKind::BindingElement
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::PropertyAssignment
        | SyntaxKind::ShorthandPropertyAssignment
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::Constructor
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::CatchClause
        | SyntaxKind::JsxAttribute => SemanticMeaning::VALUE,

        SyntaxKind::TypeParameter
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::TypeLiteral => SemanticMeaning::TYPE,

        SyntaxKind::EnumMember | SyntaxKind::ClassDeclaration => {
            SemanticMeaning::VALUE | SemanticMeaning::TYPE
        }

        SyntaxKind::ModuleDeclaration => {
            if is_ambient_module(node) {
                SemanticMeaning::NAMESPACE | SemanticMeaning::VALUE
            } else if get_module_instance_state(node) == ModuleInstanceState::INSTANTIATED {
                SemanticMeaning::NAMESPACE | SemanticMeaning::VALUE
            } else {
                SemanticMeaning::NAMESPACE
            }
        }

        SyntaxKind::EnumDeclaration
        | SyntaxKind::NamedImports
        | SyntaxKind::ImportSpecifier
        | SyntaxKind::ImportEqualsDeclaration
        | SyntaxKind::ImportDeclaration
        | SyntaxKind::JsImportDeclaration
        | SyntaxKind::ExportAssignment
        | SyntaxKind::ExportDeclaration => SemanticMeaning::ALL,

        // An external module can be a Value
        SyntaxKind::SourceFile => SemanticMeaning::NAMESPACE | SemanticMeaning::VALUE,

        _ => SemanticMeaning::ALL,
    }
}

// Go: ls/utilities.go:881 getIntersectingMeaningFromDeclarations
pub fn get_intersecting_meaning_from_declarations(
    symbols: &SymbolArena,
    node: Node,
    symbol: SymbolId,
    default_meaning: SemanticMeaning,
) -> SemanticMeaning {
    if node.is_nil() {
        return default_meaning;
    }

    let mut meaning = get_meaning_from_location(node);
    let declarations = &symbols.sym(symbol).declarations;
    if declarations.is_empty() {
        return meaning;
    }

    let mut last_iteration_meaning = meaning;

    // !!! TODO check if the port is correct and the for loop is needed
    let iteration = |mut m: SemanticMeaning| -> SemanticMeaning {
        for &declaration in declarations.iter() {
            let declaration_meaning = get_meaning_from_declaration(declaration);

            if declaration_meaning.intersects(m) {
                m |= declaration_meaning;
            }
        }
        m
    };
    meaning = iteration(meaning);

    while meaning != last_iteration_meaning {
        // The result is order-sensitive, for instance if initialMeaning == Namespace, and declarations = [class, instantiated module]
        // we need to consider both as the initialMeaning intersects with the module in the namespace space, and the module
        // intersects with the class in the value space.
        // To achieve that we will keep iterating until the result stabilizes.

        // Remember the last meaning
        last_iteration_meaning = meaning;
        meaning = iteration(meaning);
    }

    meaning
}

// Go: ls/utilities.go:922 getAllSuperTypeNodes
// Returns the node in an `extends` or `implements` clause of a class or interface.
// PORT: Go returns `[]*ast.HeritageClauseElement` (ExpressionWithTypeArguments
// or TypeReference nodes, tsgo#4797).
pub fn get_all_super_type_nodes(node: Node) -> Vec<Node> {
    if is_interface_declaration(node) {
        return get_heritage_elements(node, SyntaxKind::ExtendsKeyword);
    }
    if is_class_like(node) {
        // Go: append(core.SingleElementSlice(extends), implements...)
        let mut result = Vec::new();
        let extends = get_class_extends_heritage_element(node);
        if extends.is_some() {
            result.push(extends);
        }
        result.extend(get_implements_heritage_clause_elements(node));
        return result;
    }
    Vec::new()
}

// Go: ls/utilities.go:935 getParentSymbolsOfPropertyAccess
pub fn get_parent_symbols_of_property_access(
    location: Node,
    symbol: SymbolId,
    ch: &mut Checker,
) -> Vec<SymbolId> {
    if !is_right_side_of_property_access(location) {
        return Vec::new();
    }
    let lhs_type = ch.get_type_at_location(location.parent().expression());
    if lhs_type.is_nil() {
        return Vec::new();
    }
    let possible_symbols: Vec<TypeId> = if ch
        .ty(lhs_type)
        .flags
        .intersects(TypeFlags::UNION_OR_INTERSECTION)
    {
        ch.ty(lhs_type).types().to_vec()
    } else if ch.ty(lhs_type).symbol != ch.sym(symbol).parent {
        vec![lhs_type]
    } else {
        Vec::new()
    };
    possible_symbols
        .into_iter()
        .filter_map(|t| {
            let t_symbol = ch.ty(t).symbol;
            if t_symbol.is_some()
                && ch
                    .sym(t_symbol)
                    .flags
                    .intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE)
            {
                return Some(t_symbol);
            }
            None
        })
        .collect()
}

// Go: ls/utilities.go:963 getPropertySymbolsFromBaseTypes
// Find symbol of the given property-name and add the symbol to the given result array
// @param symbol a symbol to start searching for the given propertyName
// @param propertyName a name of property to search for
// @param cb a cache of symbol from previous iterations of calling this function to prevent infinite revisiting of the same symbol.
//
//	The value of previousIterationSymbol is undefined when the function is first called.
//
// PORT: `cb` gets the checker as its first argument, because this function
// holds it (PORTING.md func params). The Go `recur` closure is the inner fn.
pub fn get_property_symbols_from_base_types(
    symbol: SymbolId,
    property_name: &str,
    checker: &mut Checker,
    cb: &mut dyn FnMut(&mut Checker, SymbolId) -> SymbolId,
) -> SymbolId {
    fn recur(
        symbol: SymbolId,
        property_name: &str,
        checker: &mut Checker,
        cb: &mut dyn FnMut(&mut Checker, SymbolId) -> SymbolId,
        seen: &mut FxHashSet<SymbolId>,
    ) -> SymbolId {
        // Use `addToSeen` to ensure we don't infinitely recurse in this situation:
        //      interface C extends C {
        //          /*findRef*/propName: string;
        //      }
        if !checker
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE)
            || !seen.insert(symbol)
        {
            return SymbolId::NIL;
        }
        let declarations: Vec<Node> = checker.sym(symbol).declarations.to_vec();
        for declaration in declarations {
            for type_reference in get_all_super_type_nodes(declaration) {
                let property_type = checker.get_type_at_location(type_reference);
                if property_type.is_some() && checker.ty(property_type).symbol.is_some() {
                    // Visit the typeReference as well to see if it directly or indirectly uses that property
                    let property_symbol =
                        checker.get_property_of_type_exported(property_type, property_name);
                    if property_symbol.is_some() {
                        for root_symbol in checker.get_root_symbols(property_symbol) {
                            let result = cb(checker, root_symbol);
                            if result.is_some() {
                                return result;
                            }
                        }
                    }
                    let property_type_symbol = checker.ty(property_type).symbol;
                    let result = recur(property_type_symbol, property_name, checker, cb, seen);
                    if result.is_some() {
                        return result;
                    }
                }
            }
        }
        SymbolId::NIL
    }
    let mut seen: FxHashSet<SymbolId> = FxHashSet::default();
    recur(symbol, property_name, checker, cb, &mut seen)
}

// Go: ls/utilities.go:996 getPropertySymbolFromBindingElement
pub fn get_property_symbol_from_binding_element(
    checker: &mut Checker,
    binding_element: Node,
) -> SymbolId {
    let type_of_pattern = checker.get_type_at_location(binding_element.parent());
    if type_of_pattern.is_some() {
        return checker
            .get_property_of_type_exported(type_of_pattern, binding_element.name().text());
    }
    SymbolId::NIL
}

// Go: ls/utilities.go:1003 getPropertySymbolOfObjectBindingPatternWithoutPropertyName
pub fn get_property_symbol_of_object_binding_pattern_without_property_name(
    symbol: SymbolId,
    checker: &mut Checker,
) -> SymbolId {
    let binding_element =
        get_declaration_of_kind(&checker.symbols, symbol, SyntaxKind::BindingElement);
    if binding_element.is_some() && is_object_binding_element_without_property_name(binding_element)
    {
        return get_property_symbol_from_binding_element(checker, binding_element);
    }
    SymbolId::NIL
}

// Go: ls/utilities.go:1011 getTargetLabel
pub fn get_target_label(reference_node: Node, label_name: &str) -> Node {
    let mut reference_node = reference_node;
    // todo: rewrite as `ast.FindAncestor`
    while reference_node.is_some() {
        if reference_node.kind() == SyntaxKind::LabeledStatement
            && reference_node.label().text() == label_name
        {
            return reference_node.label();
        }
        reference_node = reference_node.parent();
    }
    Node::NIL
}

// Go: ls/utilities.go:1022 skipConstraint
pub fn skip_constraint(t: TypeId, type_checker: &mut Checker) -> TypeId {
    if type_checker.ty(t).is_type_parameter() {
        let c = type_checker.get_base_constraint_of_type_exported(t);
        if c.is_some() {
            return c;
        }
    }
    t
}

// Go: ls/utilities.go:1032 caseClauseTrackerState
// PORT: Go `collections.Set[jsnum.Number]` compares keys with float `==`:
// -0 and +0 are one key and NaN is never found. `NumberKey` makes -0 and +0
// one key; `has_value` returns false for NaN. `jsnum.PseudoBigInt` is not
// `Hash`, so its key is `PseudoBigIntKey` (the struct fields, as Go `==`).
#[derive(Clone, Debug, Default)]
pub struct CaseClauseTrackerState {
    pub existing_strings: FxHashSet<String>,
    pub existing_numbers: FxHashSet<NumberKey>,
    pub existing_big_ints: FxHashSet<PseudoBigIntKey>,
}

// Go: ls/utilities.go:1039 trackerAddValue
// string | jsnum.Number
// PORT: Go `any`; the literal values are `LiteralValue`.
pub type TrackerAddValue = LiteralValue;

// Go: ls/utilities.go:1042 trackerHasValue
// string | jsnum.Number | jsnum.PseudoBigInt
pub type TrackerHasValue = LiteralValue;

// Go: ls/utilities.go:1044 caseClauseTracker
// PORT: the values are passed by reference, as other `LiteralValue`
// parameters in package `ls` are.
pub trait CaseClauseTracker {
    fn add_value(&mut self, value: &TrackerAddValue);
    fn has_value(&self, value: &TrackerHasValue) -> bool;
}

impl CaseClauseTracker for CaseClauseTrackerState {
    // Go: ls/utilities.go:1049 addValue
    fn add_value(&mut self, value: &TrackerAddValue) {
        match value {
            LiteralValue::String(v) => {
                self.existing_strings.insert(v.clone());
            }
            LiteralValue::Number(v) => {
                self.existing_numbers.insert(NumberKey::from(*v));
            }
            // PORT: Go `%T` of the value.
            LiteralValue::PseudoBigInt(_) => {
                crate::core::go_panic("Unsupported type: jsnum.PseudoBigInt".to_string())
            }
            LiteralValue::Bool(_) => crate::core::go_panic("Unsupported type: bool".to_string()),
        }
    }

    // Go: ls/utilities.go:1060 hasValue
    fn has_value(&self, value: &TrackerHasValue) -> bool {
        match value {
            LiteralValue::String(v) => self.existing_strings.contains(v),
            LiteralValue::Number(v) => {
                // PORT: Go float `==` never matches NaN (see the struct).
                !v.is_nan() && self.existing_numbers.contains(&NumberKey::from(*v))
            }
            LiteralValue::PseudoBigInt(v) => {
                self.existing_big_ints.contains(&PseudoBigIntKey::from(v))
            }
            // PORT: Go `%T` of the value.
            LiteralValue::Bool(_) => crate::core::go_panic("Unsupported type: bool".to_string()),
        }
    }
}

// Go: ls/utilities.go:1073 newCaseClauseTracker
// PORT: Go returns the `caseClauseTracker` interface.
pub fn new_case_clause_tracker(
    type_checker: &mut Checker,
    clauses: &[Node],
) -> Box<dyn CaseClauseTracker> {
    let mut c = CaseClauseTrackerState {
        existing_strings: FxHashSet::default(),
        existing_numbers: FxHashSet::default(),
        existing_big_ints: FxHashSet::default(),
    };
    for &clause in clauses {
        if !is_default_clause(clause) {
            let expression = skip_parentheses(clause.expression());
            if is_literal_expression(expression) {
                match expression.kind() {
                    SyntaxKind::NoSubstitutionTemplateLiteral | SyntaxKind::StringLiteral => {
                        c.existing_strings.insert(expression.text().to_string());
                    }
                    SyntaxKind::NumericLiteral => {
                        c.existing_numbers
                            .insert(NumberKey::from(crate::jsnum::from_string(
                                expression.text(),
                            )));
                    }
                    SyntaxKind::BigIntLiteral => {
                        c.existing_big_ints.insert(PseudoBigIntKey::from(
                            &crate::jsnum::PseudoBigInt::parse_valid(expression.text()),
                        ));
                    }
                    _ => {}
                }
            } else {
                let symbol = type_checker.get_symbol_at_location_exported(clause.expression());
                if symbol.is_some() {
                    let value_declaration = type_checker.sym(symbol).value_declaration;
                    if value_declaration.is_some() && is_enum_member(value_declaration) {
                        let enum_value = type_checker.get_constant_value(value_declaration);
                        if let Some(enum_value) = enum_value {
                            c.add_value(&enum_value);
                        }
                    }
                }
            }
        }
    }
    Box::new(c)
}

// Go: ls/utilities.go:1105 RangeContainsRange
pub fn range_contains_range(r1: TextRange, r2: TextRange) -> bool {
    start_end_contains_range(r1.pos(), r1.end(), r2)
}

// Go: ls/utilities.go:1109 startEndContainsRange
pub fn start_end_contains_range(start: i32, end: i32, text_range: TextRange) -> bool {
    start <= text_range.pos() && end >= text_range.end()
}

// Go: ls/utilities.go:1113 getPossibleGenericSignatures
pub fn get_possible_generic_signatures(
    called: Node,
    type_argument_count: i32,
    c: &mut Checker,
) -> Vec<SignatureId> {
    let mut type_at_location = c.get_type_at_location(called);
    if is_optional_chain(called.parent()) {
        type_at_location = remove_optionality(
            type_at_location,
            is_optional_chain_root(called.parent()),
            true, /*isOptionalChain*/
            c,
        );
    }
    let signatures = if is_new_expression(called.parent()) {
        c.get_signatures_of_type_exported(type_at_location, SignatureKind::CONSTRUCT)
    } else {
        c.get_signatures_of_type_exported(type_at_location, SignatureKind::CALL)
    };
    // PORT: Go `s.TypeParameters() != nil`. A signature stores nil and a
    // non-nil empty slice both as an empty `Vec`. `type_parameters_origin`
    // is the Go slice identity, so an empty list with a nonzero origin is a
    // non-nil empty slice (a non-generic class's `LocalTypeParameters()`).
    // This needs `Checker::class_type_parameters_origin` to give an empty
    // class list an origin; while it returns 0 the class case reads as nil.
    signatures
        .into_iter()
        .filter(|&s| {
            let sig = c.sig(s);
            (!sig.type_parameters().is_empty() || sig.type_parameters_origin != 0)
                && sig.type_parameters().len() as i32 >= type_argument_count
        })
        .collect()
}

// Go: ls/utilities.go:1129 removeOptionality
pub fn remove_optionality(
    t: TypeId,
    is_optional_expression: bool,
    is_optional_chain: bool,
    c: &mut Checker,
) -> TypeId {
    if is_optional_expression {
        return c.get_non_nullable_type(t);
    } else if is_optional_chain {
        return c.get_non_optional_type(t);
    }
    t
}

// Go: ls/utilities.go:1138 isNoSubstitutionTemplateLiteral
pub fn is_no_substitution_template_literal(node: Node) -> bool {
    node.kind() == SyntaxKind::NoSubstitutionTemplateLiteral
}

// Go: ls/utilities.go:1142 isTaggedTemplateExpression
pub fn is_tagged_template_expression(node: Node) -> bool {
    node.kind() == SyntaxKind::TaggedTemplateExpression
}

// Go: ls/utilities.go:1146 isInsideTemplateLiteral
pub fn is_inside_template_literal(node: Node, position: i32, source_file: Node) -> bool {
    is_template_literal_kind(node.kind())
        && (get_token_pos_of_node(node, source_file, false) < position && position < node.end()
            || (is_unterminated_literal(node) && position == node.end()))
}

// Go: ls/utilities.go:1151 isTemplateHead
// Pseudo-literals
pub fn is_template_head(node: Node) -> bool {
    node.kind() == SyntaxKind::TemplateHead
}

// Go: ls/utilities.go:1155 isTemplateTail
pub fn is_template_tail(node: Node) -> bool {
    node.kind() == SyntaxKind::TemplateTail
}

// Go: ls/utilities.go:1159 findPrecedingMatchingToken
pub fn find_preceding_matching_token(
    token: Node,
    matching_token_kind: SyntaxKind,
    source_file: Node,
) -> Node {
    let mut token = token;
    let close_token_text = token_to_string(token.kind());
    let matching_token_text = token_to_string(matching_token_kind);
    let text = source_file_text(source_file);
    // Text-scan based fast path - can be bamboozled by comments and other trivia, but often provides
    // a good, fast approximation without too much extra work in the cases where it fails.
    // PORT: Go `strings.LastIndex` byte offsets; -1 when absent.
    let best_guess_index = text.rfind(matching_token_text).map_or(-1, |i| i as i32);
    if best_guess_index == -1 {
        return Node::NIL; // if the token text doesn't appear in the file, there can't be a match - super fast bail
    }
    // we can only use the textual result directly if we didn't have to count any close tokens within the range
    if text.rfind(close_token_text).map_or(-1, |i| i as i32) < best_guess_index {
        let node_at_guess = astnav::find_preceding_token(source_file, best_guess_index + 1);
        if node_at_guess.is_some() && node_at_guess.kind() == matching_token_kind {
            return node_at_guess;
        }
    }
    let token_kind = token.kind();
    let mut remaining_matching_tokens = 0;
    loop {
        let preceding = astnav::find_preceding_token(source_file, token.pos());
        if preceding.is_nil() {
            return Node::NIL;
        }
        token = preceding;
        if token.kind() == matching_token_kind {
            if remaining_matching_tokens == 0 {
                return token;
            }
            remaining_matching_tokens -= 1;
        } else if token.kind() == token_kind {
            remaining_matching_tokens += 1;
        }
    }
}

// Go: ls/utilities.go:1195 findContainingList
// PORT: Go returns `*ast.NodeList`; nil is `NodeList::NIL`.
pub fn find_containing_list(node: Node, file: Node) -> NodeList {
    // The node might be a list element (nonsynthetic) or a comma (synthetic). Either way, it will
    // be parented by the container of the SyntaxList, not the SyntaxList itself.
    let list: Cell<NodeList> = Cell::new(NodeList::NIL);
    let visit_node = |n: Node, _visitor: &mut NodeVisitor<'_, ()>| -> Node { n };
    let visit_nodes = |nodes: NodeList, _visitor: &mut NodeVisitor<'_, ()>| -> NodeList {
        if nodes.is_some() && range_contains_range(nodes.loc(), node.loc()) {
            list.set(nodes);
        }
        nodes
    };
    astnav::visit_each_child_and_js_doc(node.parent(), file, Some(&visit_node), Some(&visit_nodes));
    list.get()
}

// Go: ls/utilities.go:1212 getLeadingCommentRangesOfNode
// PORT: Go returns an `iter.Seq` (nil for JSX text); here the collected
// ranges.
pub fn get_leading_comment_ranges_of_node(node: Node, file: Node) -> Vec<CommentRange> {
    if node.kind() == SyntaxKind::JsxText {
        return Vec::new();
    }
    crate::frontend::scanner::get_leading_comment_ranges(
        &NodeFactory::default(),
        &source_file_text(file),
        node.pos(),
    )
}

// Go: ls/utilities.go:1220 getChildrenFromNonJSDocNode
// Equivalent to Strada's `node.getChildren()` for non-JSDoc nodes.
pub fn get_children_from_non_js_doc_node(node: Node, source_file: Node) -> Vec<Node> {
    let mut child_nodes: Vec<Node> = Vec::new();
    node.for_each_child(|child| {
        child_nodes.push(child);
        false
    });

    // If the node has no children, don't scan for tokens.
    // This prevents creating tokens for leaf nodes' own text.
    if child_nodes.is_empty() {
        return Vec::new();
    }

    let mut children: Vec<Node> = Vec::new();
    let mut pos = node.pos();
    let sf_text = source_file_text(source_file);
    for child in child_nodes {
        let mut scanner = scanner_ls::get_scanner_for_source_file(source_file, &sf_text, pos);
        while pos < child.pos() {
            let token = scanner.token();
            let token_full_start = scanner.token_full_start();
            let token_end = scanner.token_end();
            children.push(source_file_get_or_create_token(
                source_file,
                token,
                token_full_start,
                token_end,
                node,
                scanner.token_flags(),
            ));
            pos = token_end;
            scanner.scan();
        }
        children.push(child);
        pos = child.end();
    }
    let mut scanner = scanner_ls::get_scanner_for_source_file(source_file, &sf_text, pos);
    while pos < node.end() {
        let token = scanner.token();
        let token_full_start = scanner.token_full_start();
        let token_end = scanner.token_end();
        children.push(source_file_get_or_create_token(
            source_file,
            token,
            token_full_start,
            token_end,
            node,
            scanner.token_flags(),
        ));
        pos = token_end;
        scanner.scan();
    }
    children
}

// Go: ls/utilities.go:1261 getContainingObjectLiteralElement
// Returns the containing object literal property declaration given a possible name node, e.g. "a" in x = { "a": 1 }
pub fn get_containing_object_literal_element(node: Node) -> Node {
    let element = get_containing_object_literal_element_worker(node);
    if element.is_some()
        && (is_object_literal_expression(element.parent()) || is_jsx_attributes(element.parent()))
    {
        return element;
    }
    Node::NIL
}

// Go: ls/utilities.go:1269 getContainingObjectLiteralElementWorker
pub fn get_containing_object_literal_element_worker(node: Node) -> Node {
    // Go: the `case ast.KindIdentifier` body, which the literal kinds reach
    // through `fallthrough`.
    let identifier_case = || -> Node {
        if is_object_literal_or_jsx_element(node.parent())
            && (node.parent().parent().kind() == SyntaxKind::ObjectLiteralExpression
                || node.parent().parent().kind() == SyntaxKind::JsxAttributes)
            && node.parent().name() == node
        {
            return node.parent();
        }
        Node::NIL
    };
    match node.kind() {
        SyntaxKind::StringLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::NumericLiteral => {
            if node.parent().kind() == SyntaxKind::ComputedPropertyName {
                if is_object_literal_or_jsx_element(node.parent().parent()) {
                    return node.parent().parent();
                }
                return Node::NIL;
            }
            // fallthrough
            identifier_case()
        }
        SyntaxKind::Identifier | SyntaxKind::JsxNamespacedName => identifier_case(),
        _ => Node::NIL,
    }
}

// Go: ls/utilities.go:1287 isObjectLiteralOrJsxElement
pub fn is_object_literal_or_jsx_element(node: Node) -> bool {
    is_object_literal_element(node) || is_jsx_attribute(node) || is_jsx_spread_attribute(node)
}

// Go: ls/utilities.go:1292 nodeSeenTracker
// Return a function that returns true if the given node has not been seen
pub fn node_seen_tracker() -> Box<dyn FnMut(Node) -> bool> {
    let mut seen: FxHashSet<Node> = FxHashSet::default();
    Box::new(move |node: Node| seen.insert(node))
}

// Go: ls/utilities.go:1300 toContextRange
// FindAllReferences.toContextSpan
// PORT: Go `*core.TextRange` param and result are `Option<TextRange>`. Go
// returns the same pointer when `context` is nil.
pub fn to_context_range(
    text_range: Option<TextRange>,
    context_file: Node,
    context: Node,
) -> Option<TextRange> {
    if context.is_nil() {
        return text_range;
    }
    // !!! isContextWithStartAndEndNode
    let context_range = get_range_of_node(context, context_file, Node::NIL /*endNode*/);
    // PORT: Go dereferences `textRange` here.
    let text_range = text_range.unwrap_or_else(|| crate::core::go_nil_dereference());
    if context_range.pos() != text_range.pos() || context_range.end() != text_range.end() {
        return Some(context_range);
    }
    None
}

/// Go passes the `*ast.SourceFile` to `*compiler.Program` methods that read
/// its parser fields.
// PORT: `NewProgram` methods take the program's `ParsedSourceFile`; the file
// root `Node` maps to the program file with the same path.
fn parsed_source_file(program: &compiler::NewProgram, file: Node) -> Rc<ParsedSourceFile> {
    program
        .get_source_file_by_path(&tspath::Path(source_file_info(file).path.clone()))
        .expect("source file is not in the program")
}

// Go: ls/utilities.go:1312 getReferenceAtPosition
// PORT: Go returns `*refInfo`; nil is `None`.
pub fn get_reference_at_position(
    source_file: Node,
    position: i32,
    program: &compiler::NewProgram,
) -> Option<RefInfo> {
    let info = source_file_info(source_file);
    if let Some(reference_path) = find_reference_in_position(&info.referenced_files, position) {
        let origin = parsed_source_file(program, source_file);
        if let Some(file) = program.get_source_file_from_reference(&origin, reference_path) {
            return Some(RefInfo {
                reference: Some(reference_path.clone()),
                file_name: source_file_file_name(file.root).to_string(),
                file: file.root,
                unverified: false,
            });
        }
        return None;
    }

    if let Some(type_reference_directive) =
        find_reference_in_position(&info.type_reference_directives, position)
    {
        let origin = parsed_source_file(program, source_file);
        if let Some(reference) = program
            .get_resolved_type_reference_directive_from_type_reference_directive(
                type_reference_directive,
                &origin,
            )
        {
            if let Some(file) = program.get_source_file(&reference.resolved_file_name) {
                return Some(RefInfo {
                    reference: Some(type_reference_directive.clone()),
                    file_name: source_file_file_name(file.root).to_string(),
                    file: file.root,
                    unverified: false,
                });
            }
        }
        return None;
    }

    if let Some(lib_reference_directive) =
        find_reference_in_position(&info.lib_reference_directives, position)
    {
        if let Some(file) = program.get_lib_file_from_reference(lib_reference_directive) {
            return Some(RefInfo {
                reference: Some(lib_reference_directive.clone()),
                file_name: source_file_file_name(file.root).to_string(),
                file: file.root,
                unverified: false,
            });
        }
        return None;
    }

    if info.imports.is_empty() && info.module_augmentations.is_empty() {
        return None;
    }

    let node = astnav::get_touching_token(source_file, position);
    if !is_module_specifier_like(node) || !tspath::is_external_module_name_relative(node.text()) {
        return None;
    }

    // PORT: Go passes the `*ast.SourceFile` as an `ast.HasFileName`.
    if let Some(resolution) = program.get_resolved_module_from_module_specifier(
        &new_has_file_name(source_file_file_name(source_file), &info.path),
        node,
    ) {
        let verified_file_name = resolution.resolved_file_name.clone();
        let mut file_name = resolution.resolved_file_name.clone();
        if file_name.is_empty() {
            // ts#64159: `sourceFile.FileName().Directory().ResolveFile(node.Text())`
            // (Go N' utilities.go:1349): normalized, no trailing separator.
            file_name = tspath::get_normalized_absolute_path(
                node.text(),
                &tspath::get_directory_path(source_file_file_name(source_file)),
            );
        }
        return Some(RefInfo {
            file: program
                .get_source_file(&file_name)
                .map_or(Node::NIL, |file| file.root),
            file_name,
            reference: None,
            unverified: !verified_file_name.is_empty(),
        });
    }

    None
}

// Go: ls/utilities.go:1362 getContextualTypeFromParent
pub fn get_contextual_type_from_parent(
    node: Node,
    type_checker: &mut Checker,
    context_flags: ContextFlags,
) -> TypeId {
    let parent = walk_up_parenthesized_expressions(node.parent());
    match parent.kind() {
        SyntaxKind::NewExpression => {
            type_checker.get_contextual_type_exported(parent, context_flags)
        }
        SyntaxKind::BinaryExpression => {
            if is_equality_operator_kind(parent.operator_token().kind()) {
                return type_checker.get_type_at_location(if node == parent.right() {
                    parent.left()
                } else {
                    parent.right()
                });
            }
            type_checker.get_contextual_type_exported(node, context_flags)
        }
        SyntaxKind::CaseClause => get_switched_type(parent, type_checker),
        _ => type_checker.get_contextual_type_exported(node, context_flags),
    }
}

// Go: ls/utilities.go:1381 getContextualTypeFromParentOrAncestorTypeNode
pub fn get_contextual_type_from_parent_or_ancestor_type_node(
    node: Node,
    type_checker: &mut Checker,
) -> TypeId {
    if node.flags().intersects(NodeFlags::JS_DOC)
        && !node.flags().intersects(NodeFlags::JAVA_SCRIPT_FILE)
    {
        return TypeId::NIL;
    }

    let contextual_type = get_contextual_type_from_parent(node, type_checker, ContextFlags::NONE);
    if contextual_type.is_some() {
        return contextual_type;
    }

    let ancestor_type_node = get_ancestor_type_node(node);
    if ancestor_type_node.is_some() {
        return type_checker.get_type_at_location(ancestor_type_node);
    }

    TypeId::NIL
}

// Go: ls/utilities.go:1398 getAncestorTypeNode
pub fn get_ancestor_type_node(node: Node) -> Node {
    let mut last_type_node = Node::NIL;
    find_ancestor(node, |n| {
        if is_type_node(n) {
            last_type_node = n;
        }
        !is_qualified_name(n.parent()) && !is_type_node(n.parent()) && !is_type_element(n.parent())
    });
    last_type_node
}

// Go: ls/utilities.go:1409 isSourceFileWithGlobalExports
pub fn is_source_file_with_global_exports(node: Node) -> bool {
    node.is_some() && is_source_file(node) && file_bind_data(node).global_exports.is_some()
}

/// Go `text[lo:hi]` on the port form `text` (see `GO_STRING_MARKER`): the
/// bounds check of `go_check_slice_bounds`, then `go_cut_slice`, which keeps
/// the Go bytes of a char that a bound cuts.
pub fn go_text_slice(text: &str, lo: i32, hi: i32) -> Cow<'_, str> {
    go_check_slice_bounds(text, lo, hi);
    crate::scanner_util::go_cut_slice(text, lo as usize, hi as usize)
}

/// The bounds check of Go `text[lo:hi]` on the port form `text`: an out of
/// range bound panics with the Go runtime text.
// PORT: Go checks `hi` against the length first, then `lo` against `hi`; a
// negative bound is printed alone. The panic names Go's numbers. Go's length
// is `go_len`, and a position past the text (from `UTF16ToUTF8`) is Go's
// position plus the port bytes of the text that Go does not have. A
// content-mapped file reaches the JSDoc snippet checks with empty text in Go
// at B too, and the LSP error response carries the text.
pub fn go_check_slice_bounds(text: &str, lo: i32, hi: i32) {
    let len = text.len();
    if hi < 0 {
        crate::core::go_panic(format!("runtime error: slice bounds out of range [:{hi}]"));
    }
    if hi as usize > len {
        let go_len = crate::scanner_util::go_len(text);
        let go_hi = i64::from(hi) - (len - go_len) as i64;
        crate::core::go_panic(format!(
            "runtime error: slice bounds out of range [:{go_hi}] with length {go_len}"
        ));
    }
    if lo < 0 {
        crate::core::go_panic(format!("runtime error: slice bounds out of range [{lo}:]"));
    }
    if lo > hi {
        crate::core::go_panic(format!(
            "runtime error: slice bounds out of range [{lo}:{hi}]"
        ));
    }
}

#[cfg(test)]
mod go_text_slice_tests {
    use super::go_text_slice;
    use crate::scanner_util::{go_byte_offset, go_string_bytes, go_string_from_bytes};

    // R151 reviewer: a `lo` inside the first marker of a real U+FDD0 kept
    // the second marker, which then read as another U+FDD0. Each slice must
    // be Go's slice of the Go bytes at the Go offsets of its bounds.
    #[test]
    fn go_text_slice_is_go_slice_of_go_bytes() {
        let mut go = b"a\xE2\x82\xACb".to_vec(); // a, U+20AC, b
        go.extend_from_slice("\u{FDD0}".as_bytes()); // a real U+FDD0
        go.extend_from_slice(b"c\xFFd\xED\xA0\x80e"); // invalid byte, WTF-8 surrogate
        go.extend_from_slice("\u{1F600}\u{FDD0}".as_bytes());
        let text = go_string_from_bytes(go.clone());
        assert_eq!(go_string_bytes(&text).as_ref(), go.as_slice());
        for lo in 0..=text.len() {
            for hi in lo..=text.len() {
                let (go_lo, go_hi) = (
                    go_byte_offset(&text, lo as i32) as usize,
                    go_byte_offset(&text, hi as i32) as usize,
                );
                let want = go_string_from_bytes(go[go_lo..go_hi].to_vec());
                assert_eq!(
                    go_text_slice(&text, lo as i32, hi as i32),
                    want,
                    "{text:?}[{lo}:{hi}]"
                );
            }
        }
        // `lo` one byte into a real U+FDD0: Go keeps its last 2 bytes.
        let at = text.find('\u{FDD0}').expect("marker");
        assert_eq!(
            go_text_slice(&text, at as i32 + 1, at as i32 + 7),
            go_string_from_bytes(b"\xB7\x90c".to_vec())
        );
    }
}
