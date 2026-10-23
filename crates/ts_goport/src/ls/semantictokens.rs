use crate::ls::prelude::*;

// Go `internal/ls/semantictokens.go`: textDocument/semanticTokens.

use crate::flags_macros::{go_enum, go_flags};
use crate::spanmap::{Feature, Fidelity};

// Go: ls/semantictokens.go:21 tokenTypes
// tokenTypes defines the order of token types for encoding
static TOKEN_TYPES: [lsproto::SemanticTokenType; 23] = [
    lsproto::SemanticTokenType::NAMESPACE,
    lsproto::SemanticTokenType::CLASS,
    lsproto::SemanticTokenType::ENUM,
    lsproto::SemanticTokenType::INTERFACE,
    lsproto::SemanticTokenType::STRUCT,
    lsproto::SemanticTokenType::TYPE_PARAMETER,
    lsproto::SemanticTokenType::TYPE,
    lsproto::SemanticTokenType::PARAMETER,
    lsproto::SemanticTokenType::VARIABLE,
    lsproto::SemanticTokenType::PROPERTY,
    lsproto::SemanticTokenType::ENUM_MEMBER,
    lsproto::SemanticTokenType::DECORATOR,
    lsproto::SemanticTokenType::EVENT,
    lsproto::SemanticTokenType::FUNCTION,
    lsproto::SemanticTokenType::METHOD,
    lsproto::SemanticTokenType::MACRO,
    lsproto::SemanticTokenType::LABEL,
    lsproto::SemanticTokenType::COMMENT,
    lsproto::SemanticTokenType::STRING,
    lsproto::SemanticTokenType::KEYWORD,
    lsproto::SemanticTokenType::NUMBER,
    lsproto::SemanticTokenType::REGEXP,
    lsproto::SemanticTokenType::OPERATOR,
];

// Go: ls/semantictokens.go:48 tokenModifiers
// tokenModifiers defines the order of token modifiers for encoding
static TOKEN_MODIFIERS: [lsproto::SemanticTokenModifier; 11] = [
    lsproto::SemanticTokenModifier::DECLARATION,
    lsproto::SemanticTokenModifier::DEFINITION,
    lsproto::SemanticTokenModifier::READONLY,
    lsproto::SemanticTokenModifier::STATIC,
    lsproto::SemanticTokenModifier::DEPRECATED,
    lsproto::SemanticTokenModifier::ABSTRACT,
    lsproto::SemanticTokenModifier::ASYNC,
    lsproto::SemanticTokenModifier::MODIFICATION,
    lsproto::SemanticTokenModifier::DOCUMENTATION,
    lsproto::SemanticTokenModifier::DEFAULT_LIBRARY,
    lsproto::SemanticTokenModifier(std::borrow::Cow::Borrowed("local")),
];

// Go: ls/semantictokens.go:62 tokenType
go_enum!(TokenType, i32 {
    NAMESPACE = 0;
    CLASS = 1;
    ENUM = 2;
    INTERFACE = 3;
    STRUCT = 4;
    TYPE_PARAMETER = 5;
    TYPE = 6;
    PARAMETER = 7;
    VARIABLE = 8;
    PROPERTY = 9;
    ENUM_MEMBER = 10;
    DECORATOR = 11;
    EVENT = 12;
    FUNCTION = 13;
    METHOD = 14; // Previously called "member" in TypeScript
    MACRO = 15;
    LABEL = 16;
    COMMENT = 17;
    STRING = 18;
    KEYWORD = 19;
    NUMBER = 20;
    REGEXP = 21;
    OPERATOR = 22;
});

// Go: ls/semantictokens.go:90 tokenModifier
go_flags!(TokenModifier, i32 {
    DECLARATION = 1 << 0;
    DEFINITION = 1 << 1;
    READONLY = 1 << 2;
    STATIC = 1 << 3;
    DEPRECATED = 1 << 4;
    ABSTRACT = 1 << 5;
    ASYNC = 1 << 6;
    MODIFICATION = 1 << 7;
    DOCUMENTATION = 1 << 8;
    DEFAULT_LIBRARY = 1 << 9;
    LOCAL = 1 << 10;
});

// Go: ls/semantictokens.go:109 SemanticTokensLegend
// SemanticTokensLegend returns the legend describing the token types and modifiers.
// It filters the legend to only include types and modifiers that the client supports,
// as indicated by clientCapabilities.
// PORT: Go returns a `*lsproto.SemanticTokensLegend` that is never nil; the
// Rust result is `Some` always, the shape of the `legend` field it fills.
pub fn semantic_tokens_legend(
    client_capabilities: &lsproto::ResolvedSemanticTokensClientCapabilities,
) -> Option<lsproto::SemanticTokensLegend> {
    let mut types = Vec::with_capacity(TOKEN_TYPES.len());
    for t in &TOKEN_TYPES {
        if client_capabilities
            .token_types
            .iter()
            .any(|x| x.as_str() == &*t.0)
        {
            types.push(t.0.to_string());
        }
    }
    let mut modifiers = Vec::with_capacity(TOKEN_MODIFIERS.len());
    for m in &TOKEN_MODIFIERS {
        if client_capabilities
            .token_modifiers
            .iter()
            .any(|x| x.as_str() == &*m.0)
        {
            modifiers.push(m.0.to_string());
        }
    }
    Some(lsproto::SemanticTokensLegend {
        token_types: types,
        token_modifiers: modifiers,
    })
}

impl LanguageService {
    // Go: ls/semantictokens.go:128 ProvideSemanticTokens
    pub fn provide_semantic_tokens(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
    ) -> Result<lsproto::SemanticTokensResponse, GoError> {
        let (program, file) = self.get_program_and_file(document_uri);

        let supplemental = source_file_supplemental_source_files(file);
        let mut files: Vec<Node> = Vec::with_capacity(1 + supplemental.len());
        files.push(file);
        files.extend_from_slice(supplemental);
        let mut tokens: Vec<SemanticToken> = Vec::with_capacity(files.len());
        for projection in files {
            let (checker, done) = ls_program::get_type_checker_for_file(program, ctx, projection);
            for mut token in
                self.collect_semantic_tokens(ctx, &mut checker.borrow_mut(), projection, program)
            {
                token.file = projection;
                tokens.push(token);
            }
            drop(done);
        }
        sort_semantic_tokens(&mut tokens, &self.converters);

        if tokens.is_empty() {
            return Ok(lsproto::SemanticTokensOrNull::default());
        }

        // Convert to LSP format (relative encoding)
        let encoded = encode_semantic_tokens(ctx, &tokens, &self.converters);

        Ok(lsproto::SemanticTokensOrNull {
            semantic_tokens: Some(lsproto::SemanticTokens {
                data: encoded,
                ..Default::default()
            }),
        })
    }

    // Go: ls/semantictokens.go:160 ProvideSemanticTokensRange
    pub fn provide_semantic_tokens_range(
        &self,
        ctx: &Context,
        document_uri: &lsproto::DocumentUri,
        rng: lsproto::Range,
    ) -> Result<lsproto::SemanticTokensRangeResponse, GoError> {
        let (program, file) = self.get_program_and_file(document_uri);

        let mapped_ranges = lsconv::from_lsp_range_intersecting_for_source_file(
            &self.converters,
            file,
            rng,
            Feature::SEMANTIC_TOKENS,
        );
        let mut tokens: Vec<SemanticToken> = Vec::with_capacity(mapped_ranges.len());
        let mut seen: FxHashSet<SemanticToken> = FxHashSet::default();
        for mapped in &mapped_ranges {
            let projection = mapped.script;
            let (checker, done) = ls_program::get_type_checker_for_file(program, ctx, projection);
            for mut token in self.collect_semantic_tokens_in_range(
                ctx,
                &mut checker.borrow_mut(),
                projection,
                program,
                mapped.span.pos(),
                mapped.span.end(),
            ) {
                token.file = projection;
                if seen.insert(token) {
                    tokens.push(token);
                }
            }
            drop(done);
        }
        sort_semantic_tokens(&mut tokens, &self.converters);

        if tokens.is_empty() {
            return Ok(lsproto::SemanticTokensOrNull::default());
        }

        // Convert to LSP format (relative encoding)
        let encoded = encode_semantic_tokens(ctx, &tokens, &self.converters);

        Ok(lsproto::SemanticTokensOrNull {
            semantic_tokens: Some(lsproto::SemanticTokens {
                data: encoded,
                ..Default::default()
            }),
        })
    }

    // Go: ls/semantictokens.go:222 collectSemanticTokens
    fn collect_semantic_tokens(
        &self,
        ctx: &Context,
        c: &mut Checker,
        file: Node,
        program: &compiler::NewProgram,
    ) -> Vec<SemanticToken> {
        self.collect_semantic_tokens_in_range(ctx, c, file, program, file.pos(), file.end())
    }

    // Go: ls/semantictokens.go:226 collectSemanticTokensInRange
    // PORT: the Go recursive closure `visit` and the locals it shares
    // (`tokens`, `inJSXElement`) are the `SemanticTokenCollector` below.
    fn collect_semantic_tokens_in_range(
        &self,
        ctx: &Context,
        c: &mut Checker,
        file: Node,
        program: &compiler::NewProgram,
        span_start: i32,
        span_end: i32,
    ) -> Vec<SemanticToken> {
        let mut collector = SemanticTokenCollector {
            ctx,
            c,
            file,
            program,
            span_start,
            span_end,
            tokens: Vec::new(),
            in_jsx_element: false,
        };

        collector.visit(file);

        // Check for cancellation after collection
        if ctx.err().is_some() {
            return Vec::new();
        }

        collector.tokens
    }
}

// Go: ls/semantictokens.go:193 sortSemanticTokens
// PERF: Go computes both LSP ranges in each comparison. A token's range is
// the same on each call, so its start is computed once here. The comparisons
// give the same results, so the same pdqsort gives the same order.
fn sort_semantic_tokens(tokens: &mut [SemanticToken], converters: &lsconv::Converters) {
    let mut keyed: Vec<(lsproto::Position, SemanticToken)> = tokens
        .iter()
        .map(|&token| (semantic_token_lsp_range(&token, converters).0.start, token))
        .collect();
    gostd::slices::sort_func(&mut keyed, |(a_start, a), (b_start, b)| {
        let result = a_start.line.cmp(&b_start.line) as i32;
        if result != 0 {
            return result;
        }
        let result = a_start.character.cmp(&b_start.character) as i32;
        if result != 0 {
            return result;
        }
        // PORT: Go `a.file.Path()` is `source_file_info(file).path`.
        let result = source_file_info(a.file)
            .path
            .cmp(&source_file_info(b.file).path) as i32;
        if result != 0 {
            return result;
        }
        a.node.pos().cmp(&b.node.pos()) as i32
    });
    for (slot, (_, token)) in tokens.iter_mut().zip(keyed) {
        *slot = token;
    }
}

// Go: ls/semantictokens.go:210 semanticTokenLSPRange
fn semantic_token_lsp_range(
    token: &SemanticToken,
    converters: &lsconv::Converters,
) -> (lsproto::Range, Fidelity) {
    let start = get_token_pos_of_node(token.node, token.file, false);
    converters.to_lsp_range_for_feature(
        &token.file,
        TextRange::new(start, token.node.end()),
        Feature::SEMANTIC_TOKENS,
    )
}

// Go: ls/semantictokens.go:215 semanticToken
// PORT: Go compares the struct by value in the `seen` set of
// ProvideSemanticTokensRange; `Eq` and `Hash` cover all fields, as Go does.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SemanticToken {
    node: Node,
    file: Node,
    token_type: TokenType,
    token_modifier: TokenModifier,
}

// PORT: the state that the Go closure `visit` in collectSemanticTokensInRange
// captures.
struct SemanticTokenCollector<'a> {
    ctx: &'a Context,
    c: &'a mut Checker,
    file: Node,
    program: &'a compiler::NewProgram,
    span_start: i32,
    span_end: i32,
    tokens: Vec<SemanticToken>,
    in_jsx_element: bool,
}

impl SemanticTokenCollector<'_> {
    // Go: ls/semantictokens.go:232 visit (closure in collectSemanticTokensInRange)
    fn visit(&mut self, node: Node) -> bool {
        // Check for cancellation
        if self.ctx.err().is_some() {
            return false;
        }

        if node.is_nil() {
            return false;
        }
        if node.flags().intersects(NodeFlags::REPARSED) {
            return false;
        }
        let node_end = node.end();
        if node.pos() >= self.span_end || node_end <= self.span_start {
            return false;
        }

        let prev_in_jsx_element = self.in_jsx_element;
        if is_jsx_element(node) || is_jsx_self_closing_element(node) {
            self.in_jsx_element = true;
        } else if is_jsx_expression(node) {
            self.in_jsx_element = false;
        }

        if (is_identifier(node) || is_private_identifier(node))
            && !node.text().is_empty()
            && !self.in_jsx_element
            && !is_in_import_clause(node)
            && !is_infinity_or_nan_string(node.text())
        {
            let mut symbol = self.c.get_symbol_at_location_exported(node);
            if symbol.is_some() {
                // Resolve aliases
                if self.c.sym_flags(symbol).intersects(SymbolFlags::ALIAS) {
                    symbol = self.c.get_aliased_symbol(symbol);
                }

                let (mut token_type, ok) =
                    classify_symbol(&self.c.symbols, symbol, get_meaning_from_location(node));
                if ok {
                    let mut token_modifier = TokenModifier(0);

                    // Check if this is a declaration
                    let parent = node.parent();
                    if parent.is_some() {
                        let parent_is_declaration = is_binding_element(parent)
                            || token_from_declaration_mapping(parent.kind()) == token_type;
                        if parent_is_declaration && parent.name() == node {
                            token_modifier |= TokenModifier::DECLARATION;
                        }
                    }

                    // Property declaration in constructor: reclassify parameters as properties in property access context
                    if token_type == TokenType::PARAMETER
                        && is_right_side_of_qualified_name_or_property_access(node)
                    {
                        token_type = TokenType::PROPERTY;
                    }

                    // Type-based reclassification
                    token_type = reclassify_by_type(self.c, node, token_type);

                    // Get the value declaration to check modifiers
                    let decl = self.c.sym(symbol).value_declaration;
                    if decl.is_some() {
                        let modifiers = get_combined_modifier_flags(decl);
                        let node_flags = get_combined_node_flags(decl);

                        if modifiers.intersects(ModifierFlags::STATIC) {
                            token_modifier |= TokenModifier::STATIC;
                        }
                        if modifiers.intersects(ModifierFlags::ASYNC) {
                            token_modifier |= TokenModifier::ASYNC;
                        }
                        if token_type != TokenType::CLASS && token_type != TokenType::INTERFACE {
                            if modifiers.intersects(ModifierFlags::READONLY)
                                || node_flags.intersects(NodeFlags::CONST)
                                || self
                                    .c
                                    .sym_flags(symbol)
                                    .intersects(SymbolFlags::ENUM_MEMBER)
                            {
                                token_modifier |= TokenModifier::READONLY;
                            }
                        }
                        if (token_type == TokenType::VARIABLE || token_type == TokenType::FUNCTION)
                            && is_local_declaration(decl, self.file)
                        {
                            token_modifier |= TokenModifier::LOCAL;
                        }
                        let decl_source_file = get_source_file_of_node(decl);
                        // PORT: Go `declSourceFile.Path()` is `source_file_info(file).path`.
                        if decl_source_file.is_some()
                            && self.program.is_source_file_default_library(&tspath::Path(
                                source_file_info(decl_source_file).path.clone(),
                            ))
                        {
                            token_modifier |= TokenModifier::DEFAULT_LIBRARY;
                        }
                    } else if !self.c.sym(symbol).declarations.is_empty() {
                        // PORT: Go `symbol.Declarations != nil`; a Rust
                        // `Declarations` has no nil, and an empty list adds nothing.
                        for decl in self.c.sym(symbol).declarations.clone() {
                            let decl_source_file = get_source_file_of_node(decl);
                            if decl_source_file.is_some()
                                && self.program.is_source_file_default_library(&tspath::Path(
                                    source_file_info(decl_source_file).path.clone(),
                                ))
                            {
                                token_modifier |= TokenModifier::DEFAULT_LIBRARY;
                                break;
                            }
                        }
                    }

                    self.tokens.push(SemanticToken {
                        node,
                        file: Node::NIL,
                        token_type,
                        token_modifier,
                    });
                }
            }
        }

        node.for_each_child(|child| self.visit(child));
        self.in_jsx_element = prev_in_jsx_element;
        false
    }
}

// Go: ls/semantictokens.go:342 classifySymbol
// PORT: symbols live in the checker arena, so it is a parameter (PORTING
// "Names": functions that take a symbol get `symbols: &SymbolArena`).
fn classify_symbol(
    symbols: &SymbolArena,
    symbol: SymbolId,
    meaning: SemanticMeaning,
) -> (TokenType, bool) {
    let flags = symbols.sym_flags(symbol);
    if flags.intersects(SymbolFlags::CLASS) {
        return (TokenType::CLASS, true);
    }
    if flags.intersects(SymbolFlags::ENUM) {
        return (TokenType::ENUM, true);
    }
    if flags.intersects(SymbolFlags::TYPE_ALIAS) {
        return (TokenType::TYPE, true);
    }
    if flags.intersects(SymbolFlags::INTERFACE) {
        if meaning.intersects(SemanticMeaning::TYPE) {
            return (TokenType::INTERFACE, true);
        }
    }
    if flags.intersects(SymbolFlags::TYPE_PARAMETER) {
        return (TokenType::TYPE_PARAMETER, true);
    }

    // Check the value declaration
    let mut decl = symbols.sym(symbol).value_declaration;
    if decl.is_nil() && !symbols.sym(symbol).declarations.is_empty() {
        decl = symbols.sym(symbol).declarations[0];
    }
    if decl.is_some() {
        if is_binding_element(decl) {
            decl = get_declaration_for_binding_element(decl);
        }
        let token_type = token_from_declaration_mapping(decl.kind());
        if token_type.0 >= 0 {
            return (token_type, true);
        }
    }

    (TokenType(0), false)
}

// Go: ls/semantictokens.go:379 tokenFromDeclarationMapping
fn token_from_declaration_mapping(kind: SyntaxKind) -> TokenType {
    match kind {
        SyntaxKind::VariableDeclaration => TokenType::VARIABLE,
        SyntaxKind::Parameter => TokenType::PARAMETER,
        SyntaxKind::PropertyDeclaration => TokenType::PROPERTY,
        SyntaxKind::ModuleDeclaration => TokenType::NAMESPACE,
        SyntaxKind::EnumDeclaration => TokenType::ENUM,
        SyntaxKind::EnumMember => TokenType::ENUM_MEMBER,
        SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => TokenType::CLASS,
        SyntaxKind::MethodDeclaration => TokenType::METHOD,
        SyntaxKind::FunctionDeclaration | SyntaxKind::FunctionExpression => TokenType::FUNCTION,
        SyntaxKind::MethodSignature => TokenType::METHOD,
        SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => TokenType::PROPERTY,
        SyntaxKind::PropertySignature => TokenType::PROPERTY,
        SyntaxKind::InterfaceDeclaration => TokenType::INTERFACE,
        SyntaxKind::TypeAliasDeclaration => TokenType::TYPE,
        SyntaxKind::TypeParameter => TokenType::TYPE_PARAMETER,
        SyntaxKind::PropertyAssignment | SyntaxKind::ShorthandPropertyAssignment => {
            TokenType::PROPERTY
        }
        _ => TokenType(-1),
    }
}

// Go: ls/semantictokens.go:418 reclassifyByType
fn reclassify_by_type(c: &mut Checker, node: Node, tt: TokenType) -> TokenType {
    // Type-based reclassification for variables, properties, and parameters
    if tt == TokenType::VARIABLE || tt == TokenType::PROPERTY || tt == TokenType::PARAMETER {
        let typ = c.get_type_at_location(node);
        if typ.is_some() {
            // PORT: the Go closure `test` reads the checker through
            // `condition`; here the checker is passed to both.
            let test = |c: &mut Checker,
                        condition: &mut dyn FnMut(&mut Checker, TypeId) -> bool|
             -> bool {
                if condition(c, typ) {
                    return true;
                }
                if c.ty(typ).flags().intersects(TypeFlags::UNION) {
                    let types = c
                        .ty(typ)
                        .as_union_type()
                        .union_or_intersection
                        .types()
                        .to_vec();
                    // PORT: Go `slices.ContainsFunc`.
                    if types.iter().any(|&t| condition(c, t)) {
                        return true;
                    }
                }
                false
            };

            // Check for constructor signatures (class-like)
            if tt != TokenType::PARAMETER
                && test(c, &mut |c: &mut Checker, t: TypeId| {
                    !c.get_signatures_of_type_exported(t, SignatureKind::CONSTRUCT)
                        .is_empty()
                })
            {
                return TokenType::CLASS;
            }

            // Check for call signatures (function-like)
            // Must have call signatures AND (no properties OR be used in call context)
            let has_call_signatures = test(c, &mut |c: &mut Checker, t: TypeId| {
                !c.get_signatures_of_type_exported(t, SignatureKind::CALL)
                    .is_empty()
            });
            if has_call_signatures {
                let has_no_properties = !test(c, &mut |c: &mut Checker, t: TypeId| {
                    // PORT: Go `t.AsObjectType()` is nil for non-object type data.
                    let obj_type = c.ty(t).data.as_object_type();
                    obj_type.is_some_and(|o| !o.structured.properties().is_empty())
                });
                if has_no_properties || is_expression_in_call_expression(node) {
                    if tt == TokenType::PROPERTY {
                        return TokenType::METHOD;
                    }
                    return TokenType::FUNCTION;
                }
            }
        }
    }
    tt
}

// Go: ls/semantictokens.go:464 isLocalDeclaration
fn is_local_declaration(decl: Node, source_file: Node) -> bool {
    let mut decl = decl;
    if is_binding_element(decl) {
        decl = get_declaration_for_binding_element(decl);
    }
    if is_variable_declaration(decl) {
        let parent = decl.parent();
        // Check if this is a catch clause parameter
        if parent.is_some() && is_catch_clause(parent) {
            return get_source_file_of_node(decl) == source_file;
        }
        if parent.is_some() && is_variable_declaration_list(parent) {
            let grandparent = parent.parent();
            if grandparent.is_some() {
                let great_grandparent = grandparent.parent();
                return (!is_source_file(great_grandparent) || is_catch_clause(grandparent))
                    && get_source_file_of_node(decl) == source_file;
            }
        }
    } else if is_function_declaration(decl) {
        let parent = decl.parent();
        return parent.is_some()
            && !is_source_file(parent)
            && get_source_file_of_node(decl) == source_file;
    }
    false
}

// Go: ls/semantictokens.go:489 getDeclarationForBindingElement
fn get_declaration_for_binding_element(element: Node) -> Node {
    let mut element = element;
    loop {
        let parent = element.parent();
        if parent.is_some() && is_binding_pattern(parent) {
            let grandparent = parent.parent();
            if grandparent.is_some() && is_binding_element(grandparent) {
                element = grandparent;
                continue;
            }
            return parent.parent();
        }
        return element;
    }
}

// Go: ls/semantictokens.go:504 isInImportClause
fn is_in_import_clause(node: Node) -> bool {
    let parent = node.parent();
    parent.is_some()
        && (is_import_clause(parent) || is_import_specifier(parent) || is_namespace_import(parent))
}

// Go: ls/semantictokens.go:509 isExpressionInCallExpression
fn is_expression_in_call_expression(node: Node) -> bool {
    let mut node = node;
    while is_right_side_of_qualified_name_or_property_access(node) {
        node = node.parent();
    }
    let parent = node.parent();
    parent.is_some() && is_call_expression(parent) && parent.expression() == node
}

// Go: ls/semantictokens.go:517 isInfinityOrNaNString
// PORT: the ls-local helper (no "-Infinity"); it shadows the checker helper
// of the same name, as the Go package-level function does.
fn is_infinity_or_nan_string(text: &str) -> bool {
    text == "Infinity" || text == "NaN"
}

// Go: ls/semantictokens.go:523 encodeSemanticTokens
// encodeSemanticTokens encodes tokens into the LSP format using relative positioning.
// It filters tokens based on client capabilities, only including types and modifiers that the client supports.
fn encode_semantic_tokens(
    ctx: &Context,
    tokens: &[SemanticToken],
    converters: &lsconv::Converters,
) -> Vec<u32> {
    // Build mapping from server token types/modifiers to client indices
    let mut type_mapping: FxHashMap<TokenType, u32> = FxHashMap::default();
    let mut modifier_mapping: FxHashMap<lsproto::SemanticTokenModifier, u32> = FxHashMap::default();

    let caps = lsproto::get_client_capabilities(ctx);
    let client_capabilities = &caps.text_document.semantic_tokens;

    // Map server token types to client-supported indices
    let mut client_idx: u32 = 0;
    for (i, server_type) in TOKEN_TYPES.iter().enumerate() {
        if client_capabilities
            .token_types
            .iter()
            .any(|x| x.as_str() == &*server_type.0)
        {
            type_mapping.insert(TokenType(i as i32), client_idx);
            client_idx += 1;
        }
    }

    // Map server token modifiers to client-supported bit positions
    let mut client_bit: u32 = 0;
    for server_modifier in &TOKEN_MODIFIERS {
        if client_capabilities
            .token_modifiers
            .iter()
            .any(|x| x.as_str() == &*server_modifier.0)
        {
            modifier_mapping.insert(server_modifier.clone(), client_bit);
            client_bit += 1;
        }
    }

    // Each token encodes 5 uint32 values: deltaLine, deltaChar, length, tokenType, tokenModifiers
    let mut encoded: Vec<u32> = Vec::with_capacity(tokens.len() * 5);
    let mut prev_line: u32 = 0;
    let mut prev_char: u32 = 0;

    for token in tokens {
        // Skip tokens with types not supported by the client
        let Some(&client_type_idx) = type_mapping.get(&token.token_type) else {
            continue;
        };

        // Map modifiers to client-supported bit mask
        let mut client_modifier_mask: u32 = 0;
        for (i, server_modifier) in TOKEN_MODIFIERS.iter().enumerate() {
            if token.token_modifier.0 & (1 << i) != 0 {
                if let Some(&client_bit) = modifier_mapping.get(server_modifier) {
                    client_modifier_mask |= 1 << client_bit;
                }
            }
        }

        // Semantic tokens must describe one concrete source segment; synthesized and cross-segment
        // tokens do not identify a coherent token in the original text.
        let (lsp_range, fidelity) = semantic_token_lsp_range(token, converters);
        if !fidelity.is_exact() {
            continue;
        }
        let start_pos = lsp_range.start;
        let end_pos = lsp_range.end;

        // Length is the character difference when on the same line
        let token_length: u32;
        if start_pos.line == end_pos.line {
            token_length = end_pos.character - start_pos.character;
        } else {
            crate::core::go_panic(format!(
                "semantic tokens: token spans multiple lines: start=({},{}) end=({},{}) for token at offset {}",
                start_pos.line,
                start_pos.character,
                end_pos.line,
                end_pos.character,
                token.node.pos()
            ));
        }

        let line = start_pos.line;
        let char = start_pos.character;

        // Multiple virtual projections can describe the same original token; LSP requires one entry per
        // start position, so retain the first after sorting.
        if !encoded.is_empty() && line == prev_line && char == prev_char {
            continue;
        }
        if !encoded.is_empty() && (line < prev_line || line == prev_line && char < prev_char) {
            crate::core::go_panic(format!(
                "semantic tokens: positions must be strictly increasing: prev=({},{}) current=({},{}) for token at offset {}",
                prev_line,
                prev_char,
                line,
                char,
                token.node.pos()
            ));
        }

        // Encode as: [deltaLine, deltaChar, length, tokenType, tokenModifiers]
        let delta_line = line - prev_line;
        let delta_char: u32;
        if delta_line == 0 {
            delta_char = char - prev_char;
        } else {
            delta_char = char;
        }

        encoded.extend_from_slice(&[
            delta_line,
            delta_char,
            token_length,
            client_type_idx,
            client_modifier_mask,
        ]);

        prev_line = line;
        prev_char = char;
    }

    encoded
}
