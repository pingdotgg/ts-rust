//! Port of Go `parser/jsdoc.go`: the JSDoc parts of `Parser` (unit U10).
//!
//! Every function is an `impl Parser` method, except the package-level
//! helpers. The `Parser` struct and the shared helpers (`mark`, `rewind`,
//! `look_ahead`, `finish_node`, `new_node_list`, the error helpers) are in
//! the other parser units.
//!
//! PORT: Go reuses slices across calls (`nodeSliceArena`, `stringSliceArena`,
//! `jsdocCommentsSpace`, `jsdocCommentRangesSpace`, `jsdocTagCommentsSpace`,
//! `jsdocTagCommentsPartsSpace`). They only save allocations. Here the
//! comment text parsers reuse `jsdoc_comments_space`,
//! `jsdoc_tag_comments_space` and `jsdoc_tag_comments_parts_space` as Go
//! does, and the other calls use a new `Vec`.
//!
//! PERF: the comment text parts are slices of the scanner text, as Go's
//! substrings, not a `String` per token (jsdocstr1; astchunks0 section 7).

use std::borrow::Cow;

use crate::frontend::prelude::*;

// Go: jsdoc.go:13 init
// PORT: Go registers `parseJSDocForNode` with `ast.SetParseJSDocForNode`.
// Rust has no init hooks. The AST JSDoc lookup calls `parse_js_doc_for_node`
// directly.

// Go: jsdoc.go:19 parseJSDocForNode
/// Lazily parses the JSDoc of `node` in a TS file. Go calls it on the first
/// `Node.JSDoc()` access for a non-JS source file.
// PORT: Go reads the parse options, text and script kind from `sourceFile`.
// The caller passes them, because the Rust source file data is not a Go
// `ast.SourceFile`. The file store is frozen after the parse, so the parser
// uses the synthetic factory target for these nodes (Go allocates them on
// the heap like any other node). Go `getParser`/`putParser` pooling is not
// ported.
pub fn parse_js_doc_for_node(
    opts: &SourceFileParseOptions,
    source_text: &str,
    script_kind: ScriptKind,
    node: Node,
) -> Vec<Node> {
    let jsdoc_cut_text = std::cell::OnceCell::new();
    let mut p = new_parser();
    p.initialize_state(opts, source_text, script_kind);
    p.jsdoc_cut_text = Some(&jsdoc_cut_text);
    p.factory = NodeFactory::new();
    let ranges = get_js_doc_comment_ranges(&p.factory, Vec::new(), node, p.source_text);
    if ranges.is_empty() {
        return Vec::new();
    }
    let mut jsdoc: Vec<Node> = Vec::with_capacity(ranges.len());
    let mut pos = node.pos();
    for comment in &ranges {
        let parsed = p.parse_js_doc_comment(node, comment.pos(), comment.end(), pos);
        if !parsed.is_nil() {
            set_node_parent(parsed, node);
            jsdoc.push(parsed);
            pos = parsed.end();
        }
    }
    jsdoc
}

/// Go `jsdocState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsdocState {
    BeginningOfLine,
    SawAsterisk,
    SavingComments,
    SavingBackticks,
}

/// Go `propertyLikeParse` (a bit set).
pub type PropertyLikeParse = i32;

pub const PROPERTY_LIKE_PARSE_PROPERTY: PropertyLikeParse = 1 << 0;
pub const PROPERTY_LIKE_PARSE_PARAMETER: PropertyLikeParse = 1 << 1;
pub const PROPERTY_LIKE_PARSE_CALLBACK_PARAMETER: PropertyLikeParse = 1 << 2;

/// Go `s[i:]` on a byte offset.
// PORT: Go strings are bytes, so `s[i:]` can split a UTF-8 sequence. Rust
// `&str` slicing panics there. `s` is whitespace, which has no port form unit
// (see `scanner_util::GO_STRING_MARKER`), so the split bytes become invalid
// byte units, the Go bytes: a new text. Otherwise it is a slice of `s`.
fn byte_suffix(s: &str, i: usize) -> Cow<'_, str> {
    match s.get(i..) {
        Some(t) => Cow::Borrowed(t),
        None => Cow::Owned(crate::scanner_util::go_string_from_bytes(
            s.as_bytes()[i..].to_vec(),
        )),
    }
}

/// The text of a JSDocText or JSDocLink node made of its parts.
// PORT: Go keeps the parts (substrings of the comment) and joins them in
// `Text()` (`ast.go:300`). The port node data owns `Vec<String>`, and every
// reader joins it (`Node::text`), so the parser joins the parts once here
// and the node keeps one part, as the API decoder makes it.
fn joined_parts<S: std::borrow::Borrow<str>>(parts: &[S]) -> Vec<String> {
    if parts.is_empty() {
        Vec::new()
    } else {
        vec![parts.concat()]
    }
}

/// Go `strings.TrimRightFunc(part, f)`: a shorter slice, or a shorter owned
/// text.
fn trim_part_end(part: &mut Cow<'_, str>, f: fn(char) -> bool) {
    match part {
        Cow::Borrowed(s) => *s = (*s).trim_end_matches(f),
        Cow::Owned(s) => s.truncate(s.trim_end_matches(f).len()),
    }
}

/// Go `text[:end-2]` in `parseJSDocComment`: the comment text without its
/// closing `*/`. Returns the port offset of the cut and the Go bytes that
/// the cut keeps of a char that it splits (empty when it splits none).
// PORT: Go cuts 2 bytes. Before a `*/` they are 2 ASCII bytes. An
// unterminated comment runs to the end of the file and can end in any char,
// so the cut can split a char or a unit of the port form (see
// `scanner_util::GO_STRING_MARKER`). Go keeps the bytes of that char before
// the cut, and its scanner reads each one as a RuneError of size 1. Then the
// offset is the start of the split char.
// PERF: this runs for every JSDoc comment. A closed comment ends in `*/`,
// and an ASCII byte is a unit of its own in both forms, so 2 ASCII bytes
// before `end` are Go's 2 bytes: no walk and no allocation. A cut at a unit
// boundary keeps no bytes and allocates nothing either.
fn jsdoc_text_cut(text: &str, end: usize) -> (usize, Vec<u8>) {
    let bytes = text.as_bytes();
    if bytes[end - 1].is_ascii() && bytes[end - 2].is_ascii() {
        return (end - 2, Vec::new());
    }
    let mut cut = end;
    let mut drop = 2;
    loop {
        let (unit, size) = crate::scanner_util::go_unit_before(text, cut);
        cut -= size;
        let len = unit.go_len();
        if len == drop {
            return (cut, Vec::new());
        }
        if len > drop {
            let mut kept = Vec::new();
            unit.push_go_bytes(&mut kept);
            kept.truncate(len - drop);
            return (cut, kept);
        }
        drop -= len;
    }
}

/// The size of an invalid byte unit of the port form: the marker U+FDD0 (3
/// bytes) and a char of U+10F780..U+10F7FF (4 bytes).
const INVALID_BYTE_UNIT_LEN: i32 = 7;

/// Not in Go (jsdocmem1): the call site of a speculative JSDoc tag parse
/// (`Parser::speculate_js_doc_tag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsDocSpeculation {
    /// The tag after a JSDoc signature (Go `parseJSDocSignature`).
    SignatureTag,
    /// A child tag of this target (Go `parseChildParameterOrPropertyTag`).
    ChildTag(PropertyLikeParse),
}

/// Not in Go (jsdocmem1): the parser state where a speculative JSDoc tag
/// parse starts (`Parser::speculate_js_doc_tag`). A parser parses one file,
/// and a position of it is in one comment, so two parses with the same key
/// read the same text in the same state: they make the same nodes and
/// counts.
///
/// A tag can parse an arrow function (a `[x = (a) => b]` default, a computed
/// key in a type literal), and that parse reads `notParenthesizedArrow` (Go
/// parser.go:4570 `parsePossibleParenthesizedArrowFunctionExpression`).
/// That set only grows during a parse (its only write is the `Add` at
/// parser.go:4577), so its size is a version of it: an equal size is an
/// equal set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JsDocDiscardKey {
    site: JsDocSpeculation,
    indent: i32,
    pos: i32,
    full_start_pos: i32,
    token_start: i32,
    token: SyntaxKind,
    token_flags: TokenFlags,
    skip_js_doc_leading_asterisks: i32,
    context_flags: NodeFlags,
    parsing_contexts: ParsingContexts,
    not_parenthesized_arrows: usize,
}

/// Not in Go (jsdocmem1): a speculative JSDoc tag parse whose result its
/// caller discarded, and what the parse left after the caller's rewind (Go
/// keeps these too: the factory counts, `identifierCount` and
/// `hasDeprecatedTag`).
#[derive(Clone, Copy, Debug)]
pub struct JsDocDiscard {
    /// The discarded result. No live node points to it. A replay gives it
    /// to the caller's check again.
    result: Node,
    node_count: usize,
    text_count: usize,
    identifier_count: i64,
    sets_deprecated_tag: bool,
}

/// True when the caller of the speculative parse `site` discards its result
/// `tag` and rewinds (`name`: the name argument of a child tag parse).
fn js_doc_tag_is_discarded(site: JsDocSpeculation, tag: Node, name: Node) -> bool {
    match site {
        JsDocSpeculation::SignatureTag => tag.kind() != SyntaxKind::JsDocReturnTag,
        JsDocSpeculation::ChildTag(_) => tag.is_nil() || is_other_child_tag(tag, name),
    }
}

/// Go `parseChildParameterOrPropertyTag`: true when `child` is a parameter
/// or property tag that is not a child of `name`.
fn is_other_child_tag(child: Node, name: Node) -> bool {
    !child.is_nil()
        && !name.is_nil()
        && (child.kind() == SyntaxKind::JsDocParameterTag
            || child.kind() == SyntaxKind::JsDocPropertyTag)
        && (is_identifier(child.name()) || !texts_equal(name, child.name().left()))
}

impl<'a> Parser<'a> {
    /// The file position of the position `pos` of a JSDoc text with a cut
    /// tail (see `parse_js_doc_comment`).
    // PORT: after the split char starts at `tail`, a position of the cut
    // text is `tail + 7k`: k invalid byte units of 7 bytes. Go's position
    // is `tail + k`, k bytes into the split char, and the port's file
    // offsets of that char are its Go bytes. A mapped position
    // (`tail + 1..=3`) is before `tail + 7`. The only other file position
    // is the comment end, which can equal `tail + 7` (after a real U+FDD0,
    // 6 bytes in the port form, and 1 more byte). The worker gives it to
    // the JSDoc node only, without this map.
    #[cold]
    pub(crate) fn jsdoc_tail_pos(&self, pos: i32) -> i32 {
        if pos >= self.jsdoc_tail_first {
            let tail = self.jsdoc_tail_first - INVALID_BYTE_UNIT_LEN;
            tail + (pos - tail) / INVALID_BYTE_UNIT_LEN
        } else {
            pos
        }
    }

    // Go: jsdoc.go:56 withJSDoc
    pub(crate) fn with_js_doc(&mut self, node: Node, info: JsdocScannerInfo) -> Vec<Node> {
        if info & JSDOC_SCANNER_INFO_HAS_JS_DOC == 0 {
            return Vec::new();
        }

        // For TS/TSX files, defer JSDoc parsing to first access, unless the comment
        // contains @see/@link (needed for unused-identifier checks).
        // @deprecated is detected via cheap text scan to set PossiblyContainsDeprecatedTag;
        // callers must confirm via JSDoc lookup.
        if !self.is_javascript() {
            set_node_flags(node, node.flags() | NodeFlags::HAS_JS_DOC);
            if info & JSDOC_SCANNER_INFO_HAS_DEPRECATED != 0 {
                set_node_flags(
                    node,
                    node.flags() | NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG,
                );
            }
            if info & JSDOC_SCANNER_INFO_HAS_SEE_OR_LINK == 0 {
                return Vec::new();
            }
            // Fall through to eager parse for @see/@link
        }

        let ranges = get_js_doc_comment_ranges(&self.factory, Vec::new(), node, self.source_text);

        // Should only be called once per node
        self.has_deprecated_tag = false;
        let mut jsdoc: Vec<Node> = Vec::with_capacity(ranges.len());
        let mut pos = node.pos();
        for comment in &ranges {
            let parsed = self.parse_js_doc_comment(node, comment.pos(), comment.end(), pos);
            if !parsed.is_nil() {
                set_node_parent(parsed, node);
                jsdoc.push(parsed);
                pos = parsed.end();
            }
        }
        if !jsdoc.is_empty() {
            if !node.flags().intersects(NodeFlags::HAS_JS_DOC) {
                set_node_flags(node, node.flags() | NodeFlags::HAS_JS_DOC);
            }
            if self.has_deprecated_tag {
                self.has_deprecated_tag = false;
                set_node_flags(
                    node,
                    node.flags() | NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG,
                );
            }
            if self.is_javascript() {
                self.reparse_tags(node, &jsdoc);
            }
            self.jsdoc_infos.push(JsDocInfo {
                parent: node,
                js_docs: jsdoc.clone(),
            });
            return jsdoc;
        }
        Vec::new()
    }

    // Go: jsdoc.go:107 parseJSDocTypeExpression
    pub(crate) fn parse_js_doc_type_expression(&mut self, may_omit_braces: bool) -> Node {
        let pos = self.node_pos();
        let has_brace = if may_omit_braces {
            self.parse_optional(SyntaxKind::OpenBraceToken)
        } else {
            self.parse_expected(SyntaxKind::OpenBraceToken)
        };
        let save_context_flags = self.context_flags;
        self.set_context_flags(NodeFlags::JS_DOC, true);
        let t = self.parse_js_doc_type();
        self.context_flags = save_context_flags;
        if has_brace {
            self.parse_expected_js_doc(SyntaxKind::CloseBraceToken);
        }

        let n = self.factory.new_js_doc_type_expression(t);
        self.finish_node(n, pos)
    }

    // Go: jsdoc.go:126 parseJSDocNameReference
    pub(crate) fn parse_js_doc_name_reference(&mut self) -> Node {
        let pos = self.node_pos();
        let has_brace = self.parse_optional(SyntaxKind::OpenBraceToken);
        let entity_name = self.parse_js_doc_link_name();
        if has_brace {
            self.parse_expected_js_doc(SyntaxKind::CloseBraceToken);
        }
        let full_start = self.scanner.token_full_start();
        self.scanner.reset_pos(full_start);
        self.next_token_js_doc();
        let n = self.factory.new_js_doc_name_reference(entity_name);
        self.finish_node(n, pos)
    }

    // Go: jsdoc.go:139 parseJSDocComment
    /// Pass `end = -1` to parse the text to the end.
    pub(crate) fn parse_js_doc_comment(
        &mut self,
        parent: Node,
        start: i32,
        end: i32,
        full_start: i32,
    ) -> Node {
        let _ = parent;
        let end = if end == -1 {
            self.source_text.len() as i32
        } else {
            end
        };
        // Check for /** (JSDoc opening part)
        if !super::utilities::is_js_doc_like_text(&self.source_text[start as usize..]) {
            // TODO: This should be a panic, unless parseSingleJSDocComment is calling this (not ported yet)
            return Node::NIL;
        }

        let save_source_text = self.source_text;
        let save_token = self.token;
        let save_context_flags = self.context_flags;
        let save_parsing_contexts = self.parsing_contexts;
        let save_scanner_state = self.scanner.mark();
        // Not in Go (parsefast2): `set_text` clears it.
        let save_text_has_no_go_string_marker = self.scanner.text_has_no_go_string_marker;
        // Go: jsdoc.go:153 Mark saves the `commentDirectives` slice header,
        // SetText (jsdoc.go:164, scanner.go:400) starts a nil slice, so the
        // JSDoc scan appends to a new array, and Rewind (jsdoc.go:181) gives
        // back the outer slice unchanged. A directive inside the JSDoc is
        // dropped. PORT: the scanner state keeps only a length into the
        // scanner's one `Vec`, so the save also takes that `Vec` (issue #49).
        let save_comment_directives = std::mem::take(&mut self.scanner.comment_directives);
        let save_diagnostics_length = self.diagnostics.borrow().diagnostics.len();
        let save_has_parse_error = self.has_parse_error();
        let save_has_await_identifier = self.statement_has_await_identifier;

        // initial indent is start+4 to account for leading `/** `
        // + 1 because \n is one character before the first character in the line and,
        // if there is no \n before start, -1 is one index before the first character in the string
        let last_newline = self.source_text[..start as usize]
            .rfind('\n')
            .map_or(-1, |i| i as i32);
        let initial_indent = start + 4 - (last_newline + 1);
        // -2 for trailing `*/`
        // PORT: see `jsdoc_text_cut`. A text that ends in the kept bytes of
        // a split char is a new string, with one invalid byte unit for each
        // kept byte. Its owner is `jsdoc_cut_text`, which outlives the
        // parse. The positions after the split char differ from Go's, so
        // the nodes, lists and diagnostics map them (`jsdoc_tail_pos`).
        let (cut, kept) = jsdoc_text_cut(save_source_text, end as usize);
        let cut_has_no_tail = kept.is_empty();
        self.source_text = if kept.is_empty() {
            &save_source_text[..cut]
        } else {
            self.jsdoc_tail_first = cut as i32 + INVALID_BYTE_UNIT_LEN;
            let text = || {
                let mut text = save_source_text[..cut].to_string();
                text.push_str(&crate::scanner_util::go_string_from_bytes(kept));
                text
            };
            match self.jsdoc_cut_text {
                // Only an unterminated comment, which ends the file, has a
                // tail, so a parse cuts one text at most.
                Some(owner) => owner.get_or_init(text).as_str(),
                // PORT: a parse that gives no owner leaks the text.
                None => Box::leak(text().into_boxed_str()),
            }
        };
        self.scanner.set_text(self.source_text);
        // A cut of the text with no kept tail is a prefix of the text; a
        // tail holds invalid byte units, which are markers.
        self.scanner
            .set_text_has_no_go_string_marker(cut_has_no_tail && save_text_has_no_go_string_marker);
        // +3 for leading `/**`
        self.scanner.reset_pos(start + 3);
        self.set_context_flags(NodeFlags::JS_DOC, true);
        self.parsing_contexts |= 1 << (ParsingContext::JsDocComment as i32);

        let comment = self.parse_js_doc_comment_worker(start, end, full_start, initial_indent);
        // move jsdoc diagnostics to jsdocDiagnostics -- for JS files only
        let mut moved = self
            .diagnostics
            .borrow_mut()
            .diagnostics
            .split_off(save_diagnostics_length);
        if self.jsdoc_tail_first != i32::MAX {
            for d in &mut moved {
                d.pos = self.jsdoc_tail_pos(d.pos);
                d.end = self.jsdoc_tail_pos(d.end);
            }
            self.jsdoc_tail_first = i32::MAX;
        }
        if self.context_flags.intersects(NodeFlags::JAVA_SCRIPT_FILE) {
            self.jsdoc_diagnostics.extend(moved);
        }

        self.source_text = save_source_text;
        self.scanner.set_text(self.source_text);
        self.scanner
            .set_text_has_no_go_string_marker(save_text_has_no_go_string_marker);
        self.parsing_contexts = save_parsing_contexts;
        self.context_flags = save_context_flags;
        self.scanner.comment_directives = save_comment_directives;
        self.scanner.rewind(save_scanner_state);
        self.token = save_token;
        self.set_has_parse_error(save_has_parse_error);
        self.statement_has_await_identifier = save_has_await_identifier;

        comment
    }

    // Go: jsdoc.go:193 parseJSDocCommentWorker
    /// `start` is the offset in the containing file. `indent` is the number of
    /// spaces to consider as the margin (applies to non-first lines only).
    fn parse_js_doc_comment_worker(
        &mut self,
        start: i32,
        end: i32,
        full_start: i32,
        mut indent: i32,
    ) -> Node {
        // Initially we can parse out a tag.  We also have seen a starting asterisk.
        // This is so that /** * @type */ doesn't parse.
        let mut tags: Vec<Node> = Vec::new();
        let mut tags_pos = -1;
        let mut tags_end = -1;
        let mut state = JsdocState::SawAsterisk;
        let mut backtick_count = 0;
        let mut in_fenced_code_block = false;
        let mut comment_parts: Vec<Node> = Vec::new();
        // Go: `comments := p.jsdocCommentsSpace`. Taken while in use.
        let mut comments = std::mem::take(&mut self.jsdoc_comments_space);
        let mut comments_pos = -1;
        let mut link_end = start;
        let mut margin = -1;
        // PORT: Go `pushComment` is a closure over these locals. A closure
        // would hold them borrowed for the whole loop, so it is a macro.
        macro_rules! push_comment {
            ($text:expr) => {{
                let text: &'a str = $text;
                if margin == -1 {
                    margin = indent;
                }
                indent += text.len() as i32;
                comments.push(Cow::Borrowed(text));
            }};
        }

        self.next_token_js_doc();
        while self.parse_optional_jsdoc(SyntaxKind::WhitespaceTrivia) {}
        if self.parse_optional_jsdoc(SyntaxKind::NewLineTrivia) {
            state = JsdocState::BeginningOfLine;
            indent = 0;
        }
        loop {
            // Detect fenced code blocks by counting consecutive backtick tokens.
            // Three or more consecutive backticks toggle the fenced code block state.
            if self.token != SyntaxKind::BacktickToken && backtick_count > 0 {
                if backtick_count >= 3 {
                    in_fenced_code_block = !in_fenced_code_block;
                }
                backtick_count = 0;
            }
            match self.token {
                SyntaxKind::AtToken => {
                    if in_fenced_code_block || !self.scanner.can_follow_js_doc_at() {
                        state = if in_fenced_code_block {
                            JsdocState::SavingBackticks
                        } else {
                            JsdocState::SavingComments
                        };
                        push_comment!(self.scanner.token_text());
                    } else {
                        remove_trailing_whitespace(&mut comments);
                        if comments_pos == -1 {
                            comments_pos = self.node_pos();
                        }
                        let tag = self.parse_tag(&tags, indent);
                        if tags_pos == -1 {
                            tags_pos = tag.pos();
                        }
                        tags.push(tag);
                        tags_end = tag.end();
                        // NOTE: According to usejsdoc.org, a tag goes to end of line, except the last tag.
                        // Real-world comments may break this rule, so "BeginningOfLine" will not be a real line beginning
                        // for malformed examples like `/** @param {string} x @returns {number} the length */`
                        state = JsdocState::BeginningOfLine;
                        margin = -1;
                    }
                }
                SyntaxKind::NewLineTrivia => {
                    comments.push(Cow::Borrowed(self.scanner.token_text()));
                    state = JsdocState::BeginningOfLine;
                    indent = 0;
                }
                SyntaxKind::AsteriskToken => {
                    let asterisk = self.scanner.token_text();
                    if state == JsdocState::SawAsterisk {
                        // If we've already seen an asterisk, then we can no longer parse a tag on this line
                        state = JsdocState::SavingComments;
                        push_comment!(asterisk);
                    } else {
                        assert!(
                            state == JsdocState::BeginningOfLine,
                            "state must be BeginningOfLine"
                        );
                        // Ignore the first asterisk on a line
                        state = JsdocState::SawAsterisk;
                        indent += asterisk.len() as i32;
                    }
                }
                SyntaxKind::WhitespaceTrivia => {
                    assert!(
                        state != JsdocState::SavingComments && state != JsdocState::SavingBackticks,
                        "whitespace shouldn't come from the scanner while saving top-level comment text"
                    );
                    // only collect whitespace if we're already saving comments or have just crossed the comment indent margin
                    let whitespace = self.scanner.token_text();
                    let len = whitespace.len() as i32;
                    if margin > -1 && indent + len > margin {
                        let mut existing_indent = margin - indent;
                        if existing_indent < 0 {
                            existing_indent += len;
                        }
                        if existing_indent < 0 {
                            existing_indent = 0;
                        }
                        comments.push(byte_suffix(whitespace, existing_indent as usize));
                    }
                    indent += len;
                }
                SyntaxKind::EndOfFile => break,
                SyntaxKind::JsDocCommentTextToken => {
                    if state != JsdocState::SavingBackticks {
                        state = if in_fenced_code_block {
                            JsdocState::SavingBackticks
                        } else {
                            JsdocState::SavingComments
                        };
                    }
                    push_comment!(self.scanner.token_value());
                }
                SyntaxKind::BacktickToken => {
                    backtick_count += 1;
                    state = if state == JsdocState::SavingBackticks {
                        JsdocState::SavingComments
                    } else {
                        JsdocState::SavingBackticks
                    };
                    push_comment!(self.scanner.token_text());
                }
                _ => {
                    // PORT: Go `case KindOpenBraceToken` ends in `fallthrough`
                    // to `default`; `fall_through` models it.
                    let mut fall_through = true;
                    if self.token == SyntaxKind::OpenBraceToken {
                        if in_fenced_code_block {
                            state = JsdocState::SavingBackticks;
                            push_comment!(self.scanner.token_text());
                            fall_through = false;
                        } else {
                            state = JsdocState::SavingComments;
                            let comment_end = self.scanner.token_full_start();
                            let link_start = self.scanner.token_end() - 1;
                            let link = self.parse_js_doc_link(link_start);
                            if !link.is_nil() {
                                if link_end == start {
                                    remove_leading_newlines(&mut comments);
                                }
                                let text = self.factory.new_js_doc_text(joined_parts(&comments));
                                comments.clear();
                                let jsdoc_text =
                                    self.finish_node_with_end(text, link_end, comment_end);
                                comment_parts.push(jsdoc_text);
                                comment_parts.push(link);
                                link_end = self.scanner.token_end();
                                fall_through = false;
                            }
                        }
                    }
                    if fall_through {
                        // Anything else is doc comment text. We just save it. Because it
                        // wasn't a tag, we can no longer parse a tag on this line until we hit the next
                        // line break.
                        if state != JsdocState::SavingBackticks {
                            state = if in_fenced_code_block {
                                JsdocState::SavingBackticks
                            } else {
                                JsdocState::SavingComments
                            };
                        }
                        push_comment!(self.scanner.token_text());
                    }
                }
            }
            if state == JsdocState::SavingComments || state == JsdocState::SavingBackticks {
                self.next_js_doc_comment_text_token(state == JsdocState::SavingBackticks);
            } else {
                self.next_token_js_doc();
            }
        }

        if comments_pos == -1 {
            comments_pos = self.scanner.token_full_start();
        }

        if let Some(last) = comments.last_mut() {
            trim_part_end(last, char::is_whitespace);
            let text = self.factory.new_js_doc_text(joined_parts(&comments));
            let jsdoc_text = self.finish_node_with_end(text, link_end, comments_pos);
            comment_parts.push(jsdoc_text);
        }
        // Go: `p.jsdocCommentsSpace = comments[:0]`.
        comments.clear();
        self.jsdoc_comments_space = comments;

        assert!(
            !(!comment_parts.is_empty() && !tags.is_empty() && comments_pos == -1),
            "having parsed tags implies that the end of the comment span should be set"
        );

        let tags_node_list = if tags_pos != -1 {
            self.new_node_list(TextRange::new(tags_pos, tags_end), &tags)
        } else {
            NodeList::NIL
        };

        let comment_list = self.new_node_list(TextRange::new(start, comments_pos), &comment_parts);
        let jsdoc_comment = self.factory.new_js_doc(comment_list, tags_node_list);
        // PORT: `end` is a file position, not a position of a cut text, so
        // it is not mapped (see `jsdoc_tail_pos`).
        let tail_first = std::mem::replace(&mut self.jsdoc_tail_first, i32::MAX);
        let jsdoc_comment = self.finish_node_with_end(jsdoc_comment, full_start, end);
        self.jsdoc_tail_first = tail_first;
        jsdoc_comment
    }

    // Go: jsdoc.go:406 isNextNonwhitespaceTokenEndOfFile
    fn is_next_nonwhitespace_token_end_of_file(&mut self) -> bool {
        // We must use infinite lookahead, as there could be any number of newlines :(
        loop {
            self.next_token_js_doc();
            if self.token == SyntaxKind::EndOfFile {
                return true;
            }
            if !(self.token == SyntaxKind::WhitespaceTrivia
                || self.token == SyntaxKind::NewLineTrivia)
            {
                return false;
            }
        }
    }

    // Go: jsdoc.go:419 skipWhitespace
    pub(crate) fn skip_whitespace(&mut self) {
        if (self.token == SyntaxKind::WhitespaceTrivia || self.token == SyntaxKind::NewLineTrivia)
            && self.look_ahead(Self::is_next_nonwhitespace_token_end_of_file)
        {
            // Don't skip whitespace prior to EoF (or end of comment) - that shouldn't be included in any node's range
            return;
        }
        while self.token == SyntaxKind::WhitespaceTrivia || self.token == SyntaxKind::NewLineTrivia
        {
            self.next_token_js_doc();
        }
    }

    // Go: jsdoc.go:431 skipWhitespaceOrAsterisk
    // PERF: Go joins the texts of the tokens after the last newline. The
    // JSDoc tokens after it are contiguous (`scan_js_doc_token` starts each
    // one at the end of the one before), so the join is the slice of the
    // scanner text from the end of that newline to the full start of the
    // current token: no allocation.
    pub(crate) fn skip_whitespace_or_asterisk(&mut self) -> &'a str {
        if (self.token == SyntaxKind::WhitespaceTrivia || self.token == SyntaxKind::NewLineTrivia)
            && self.look_ahead(Self::is_next_nonwhitespace_token_end_of_file)
        {
            // Don't skip whitespace prior to EoF (or end of comment) - that shouldn't be included in any node's range
            return "";
        }

        let mut preceding_line_break = self.scanner.has_preceding_line_break();
        // The end of the last newline token (Go: `seenLineBreak` and the
        // start of `indents`).
        let mut indents_start: Option<i32> = None;
        while (preceding_line_break && self.token == SyntaxKind::AsteriskToken)
            || self.token == SyntaxKind::WhitespaceTrivia
            || self.token == SyntaxKind::NewLineTrivia
        {
            if self.token == SyntaxKind::NewLineTrivia {
                preceding_line_break = true;
                indents_start = Some(self.scanner.token_end());
            } else if self.token == SyntaxKind::AsteriskToken {
                preceding_line_break = false;
            }
            self.next_token_js_doc();
        }
        match indents_start {
            Some(start) => {
                &self.scanner.text[start as usize..self.scanner.token_full_start() as usize]
            }
            None => "",
        }
    }

    // Go: jsdoc.go:460 parseTag
    pub(crate) fn parse_tag(&mut self, tags: &[Node], margin: i32) -> Node {
        assert!(
            self.token == SyntaxKind::AtToken,
            "should be called only at the start of a tag"
        );
        let start = self.scanner.token_start();
        self.next_token_js_doc();

        let tag_name = self.parse_js_doc_identifier_name(Some(diag::Identifier_expected));
        let indent_text = self.skip_whitespace_or_asterisk();

        let tag = match tag_name.text() {
            "implements" => self.parse_implements_tag(start, tag_name, margin, indent_text),
            "augments" | "extends" => self.parse_augments_tag(start, tag_name, margin, indent_text),
            "public" => self.parse_simple_tag(
                start,
                NodeFactory::new_js_doc_public_tag,
                tag_name,
                margin,
                indent_text,
            ),
            "private" => self.parse_simple_tag(
                start,
                NodeFactory::new_js_doc_private_tag,
                tag_name,
                margin,
                indent_text,
            ),
            "protected" => self.parse_simple_tag(
                start,
                NodeFactory::new_js_doc_protected_tag,
                tag_name,
                margin,
                indent_text,
            ),
            "readonly" => self.parse_simple_tag(
                start,
                NodeFactory::new_js_doc_readonly_tag,
                tag_name,
                margin,
                indent_text,
            ),
            "override" => self.parse_simple_tag(
                start,
                NodeFactory::new_js_doc_override_tag,
                tag_name,
                margin,
                indent_text,
            ),
            "deprecated" => {
                self.has_deprecated_tag = true;
                self.parse_simple_tag(
                    start,
                    NodeFactory::new_js_doc_deprecated_tag,
                    tag_name,
                    margin,
                    indent_text,
                )
            }
            "this" => self.parse_this_tag(start, tag_name, margin, indent_text),
            "arg" | "argument" | "param" => self.parse_parameter_or_property_tag(
                start,
                tag_name,
                PROPERTY_LIKE_PARSE_PARAMETER,
                margin,
            ),
            "return" | "returns" => {
                self.parse_return_tag(tags, start, tag_name, margin, indent_text)
            }
            "template" => self.parse_template_tag(start, tag_name, margin, indent_text),
            "type" => self.parse_type_tag(tags, start, tag_name, margin, indent_text),
            "typedef" => self.parse_typedef_tag(start, tag_name, margin, indent_text),
            "callback" => self.parse_callback_tag(start, tag_name, margin, indent_text),
            "overload" => self.parse_overload_tag(start, tag_name, margin, indent_text),
            "satisfies" => self.parse_satisfies_tag(start, tag_name, margin, indent_text),
            "see" => self.parse_see_tag(start, tag_name, margin, indent_text),
            "exception" | "throws" => self.parse_throws_tag(start, tag_name, margin, indent_text),
            "import" => self.parse_import_tag(start, tag_name, margin, indent_text),
            _ => self.parse_unknown_tag(start, tag_name, margin, indent_text),
        };
        assert!(!tag.is_nil(), "tag should not be nil");
        tag
    }

    // Go: jsdoc.go:534 parseTrailingTagComments
    fn parse_trailing_tag_comments(
        &mut self,
        pos: i32,
        end: i32,
        mut margin: i32,
        indent_text: &'a str,
    ) -> NodeList {
        // some tags, like typedef and callback, have already parsed their comments earlier
        if indent_text.is_empty() {
            margin += end - pos;
        }
        let initial_margin = if (margin as usize) < indent_text.len() {
            byte_suffix(indent_text, margin as usize)
        } else {
            Cow::Borrowed("")
        };
        self.parse_tag_comments(margin, Some(initial_margin))
    }

    // Go: jsdoc.go:546 parseTagComments
    fn parse_tag_comments(
        &mut self,
        mut indent: i32,
        initial_margin: Option<Cow<'a, str>>,
    ) -> NodeList {
        let comments_pos = self.node_pos();
        // Go: `comments := p.jsdocTagCommentsSpace` and `parts :=
        // p.jsdocTagCommentsPartsSpace`, set to nil while in use.
        let mut comments = std::mem::take(&mut self.jsdoc_tag_comments_space);
        let mut parts = std::mem::take(&mut self.jsdoc_tag_comments_parts_space);
        let mut link_end = -1;
        let mut state = JsdocState::BeginningOfLine;
        let mut backtick_count = 0;
        let mut in_fenced_code_block = false;
        assert!(indent >= 0, "indent must be a natural number");
        let mut margin = -1;
        // PORT: Go `pushComment` closure, as a macro (see parseJSDocCommentWorker).
        macro_rules! push_comment {
            ($text:expr) => {{
                let text: Cow<'a, str> = ($text).into();
                if margin == -1 {
                    margin = indent;
                }
                indent += text.len() as i32;
                comments.push(text);
            }};
        }

        if let Some(initial_margin) = initial_margin {
            // jump straight to saving comments if there is some initial indentation
            if !initial_margin.is_empty() {
                push_comment!(initial_margin);
            }
            state = JsdocState::SawAsterisk;
        }
        let mut tok = self.token;
        loop {
            // Detect fenced code blocks by counting consecutive backtick tokens.
            // Three or more consecutive backticks toggle the fenced code block state.
            if tok != SyntaxKind::BacktickToken && backtick_count > 0 {
                if backtick_count >= 3 {
                    in_fenced_code_block = !in_fenced_code_block;
                }
                backtick_count = 0;
            }
            match tok {
                SyntaxKind::NewLineTrivia => {
                    state = JsdocState::BeginningOfLine;
                    // don't use pushComment here because we want to keep the margin unchanged
                    comments.push(Cow::Borrowed(self.scanner.token_text()));
                    indent = 0;
                }
                SyntaxKind::AtToken => {
                    if !in_fenced_code_block && self.scanner.can_follow_js_doc_at() {
                        let pos = self.scanner.token_end() - 1;
                        self.scanner.reset_pos(pos);
                        break;
                    }
                    state = if in_fenced_code_block {
                        JsdocState::SavingBackticks
                    } else {
                        JsdocState::SavingComments
                    };
                    push_comment!(self.scanner.token_text());
                }
                SyntaxKind::EndOfFile => {
                    // Done
                    break;
                }
                SyntaxKind::WhitespaceTrivia => {
                    assert!(
                        state != JsdocState::SavingComments && state != JsdocState::SavingBackticks,
                        "whitespace shouldn't come from the scanner while saving comment text"
                    );
                    let whitespace = self.scanner.token_text();
                    let len = whitespace.len() as i32;
                    // if the whitespace crosses the margin, take only the whitespace that passes the margin
                    if margin > -1 && indent + len > margin {
                        comments.push(byte_suffix(whitespace, (margin - indent).max(0) as usize));
                        state = if in_fenced_code_block {
                            JsdocState::SavingBackticks
                        } else {
                            JsdocState::SavingComments
                        };
                    }
                    indent += len;
                }
                SyntaxKind::OpenBraceToken => {
                    if in_fenced_code_block {
                        state = JsdocState::SavingBackticks;
                        push_comment!(self.scanner.token_text());
                    } else {
                        state = JsdocState::SavingComments;
                        let comment_end = self.scanner.token_full_start();
                        let link_start = self.scanner.token_end() - 1;
                        let link = self.parse_js_doc_link(link_start);
                        if !link.is_nil() {
                            let comment_start = if link_end > -1 {
                                link_end
                            } else {
                                comments_pos
                            };
                            let t = self.factory.new_js_doc_text(joined_parts(&comments));
                            comments.clear();
                            let text = self.finish_node_with_end(t, comment_start, comment_end);
                            parts.push(text);
                            parts.push(link);
                            link_end = self.scanner.token_end();
                        } else {
                            push_comment!(self.scanner.token_text());
                        }
                    }
                }
                SyntaxKind::BacktickToken => {
                    backtick_count += 1;
                    state = if state == JsdocState::SavingBackticks {
                        JsdocState::SavingComments
                    } else {
                        JsdocState::SavingBackticks
                    };
                    push_comment!(self.scanner.token_text());
                }
                SyntaxKind::JsDocCommentTextToken => {
                    if state != JsdocState::SavingBackticks {
                        state = if in_fenced_code_block {
                            JsdocState::SavingBackticks
                        } else {
                            JsdocState::SavingComments
                        };
                        // leading identifiers start recording as well
                    }
                    push_comment!(self.scanner.token_value());
                }
                // leading asterisks start recording on the *next* (non-whitespace) token
                SyntaxKind::AsteriskToken if state == JsdocState::BeginningOfLine => {
                    state = JsdocState::SawAsterisk;
                    indent += 1;
                }
                // Go: `case KindAsteriskToken` otherwise records the * as a
                // comment (`fallthrough` to `default`).
                _ => {
                    if state != JsdocState::SavingBackticks {
                        state = if in_fenced_code_block {
                            JsdocState::SavingBackticks
                        } else {
                            JsdocState::SavingComments
                        };
                        // leading identifiers start recording as well
                    }
                    push_comment!(self.scanner.token_text());
                }
            }
            tok = if state == JsdocState::SavingComments || state == JsdocState::SavingBackticks {
                self.next_js_doc_comment_text_token(state == JsdocState::SavingBackticks)
            } else {
                self.next_token_js_doc()
            };
        }

        remove_leading_newlines(&mut comments);
        remove_trailing_whitespace(&mut comments);
        if !comments.is_empty() {
            let comment_start = if link_end > -1 {
                link_end
            } else {
                comments_pos
            };
            let t = self.factory.new_js_doc_text(joined_parts(&comments));
            let text = self.finish_node(t, comment_start);
            parts.push(text);
        }
        // Go: `p.jsdocTagCommentsSpace = comments[:0]`.
        comments.clear();
        self.jsdoc_tag_comments_space = comments;

        let list = if !parts.is_empty() {
            let end = self.scanner.token_end();
            self.new_node_list(TextRange::new(comments_pos, end), &parts)
        } else {
            NodeList::NIL
        };
        // Go: `p.jsdocTagCommentsPartsSpace = parts[:0]`.
        parts.clear();
        self.jsdoc_tag_comments_parts_space = parts;
        list
    }

    // Go: jsdoc.go:714 parseJSDocLink
    fn parse_js_doc_link(&mut self, start: i32) -> Node {
        let state = self.mark();
        let Some(link_type) = self.parse_js_doc_link_prefix() else {
            self.rewind(state);
            return Node::NIL;
        };
        self.next_token_js_doc();
        // start at token after link, then skip any whitespace
        self.skip_whitespace();
        let name = self.parse_js_doc_link_name();
        // PERF: Go appends each token text. The tokens after the first are
        // JSDoc tokens, which start at the end of the one before, so the
        // parts are the slice of the text from the start of the first token
        // to the end of the last one.
        let text_start = self.scanner.token_start();
        let mut text_end = text_start;
        while self.token != SyntaxKind::CloseBraceToken
            && self.token != SyntaxKind::NewLineTrivia
            && self.token != SyntaxKind::EndOfFile
        {
            text_end = self.scanner.token_end();
            self.next_token_js_doc(); // Couldn't this be nextTokenCommentJSDoc?
        }
        // The one-part node text (see `joined_parts`), or no part when the
        // loop took no token.
        let text = if text_end > text_start {
            vec![self.scanner.text[text_start as usize..text_end as usize].to_string()]
        } else {
            Vec::new()
        };
        let create = match link_type {
            "link" => self.factory.new_js_doc_link(name, text),
            "linkcode" => self.factory.new_js_doc_link_code(name, text),
            _ => self.factory.new_js_doc_link_plain(name, text),
        };
        let end = self.scanner.token_end();
        self.finish_node_with_end(create, start, end)
    }

    // Go: jsdoc.go:742 parseJSDocLinkName
    fn parse_js_doc_link_name(&mut self) -> Node {
        if token_is_identifier_or_keyword(self.token) {
            let pos = self.node_pos();
            let mut name = self.parse_identifier_name();
            while self.parse_optional(SyntaxKind::DotToken) {
                let right = if self.token == SyntaxKind::PrivateIdentifier {
                    self.create_missing_identifier()
                } else {
                    self.parse_identifier_name()
                };
                let q = self.factory.new_qualified_name(name, right);
                name = self.finish_node(q, pos);
            }
            while self.token == SyntaxKind::PrivateIdentifier {
                self.scanner.re_scan_hash_token();
                self.next_token_js_doc();
                let right = self.parse_identifier();
                let q = self.factory.new_qualified_name(name, right);
                name = self.finish_node(q, pos);
            }
            return name;
        }
        Node::NIL
    }

    // Go: jsdoc.go:765 parseJSDocLinkPrefix
    // PORT: Go returns `(string, bool)`; `false` (with the kind "NONE") is `None`.
    fn parse_js_doc_link_prefix(&mut self) -> Option<&'a str> {
        self.skip_whitespace_or_asterisk();
        if self.token == SyntaxKind::OpenBraceToken
            && self.next_token_js_doc() == SyntaxKind::AtToken
            && token_is_identifier_or_keyword(self.next_token_js_doc())
        {
            let kind = self.scanner.token_value();
            if is_js_doc_link_tag(kind) {
                return Some(kind);
            }
        }
        None
    }

    // Go: jsdoc.go:780 parseUnknownTag
    fn parse_unknown_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        indent: i32,
        indent_text: &'a str,
    ) -> Node {
        let pos = self.node_pos();
        let comment = self.parse_trailing_tag_comments(start, pos, indent, indent_text);
        let n = self.factory.new_js_doc_unknown_tag(tag_name, comment);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:784 tryParseTypeExpression
    fn try_parse_type_expression(&mut self) -> Node {
        self.skip_whitespace_or_asterisk();
        if self.token == SyntaxKind::OpenBraceToken {
            self.parse_js_doc_type_expression(false /*mayOmitBraces*/)
        } else {
            Node::NIL
        }
    }

    // Go: jsdoc.go:793 parseBracketNameInPropertyAndParamTag
    /// Returns `(name, is_bracketed)`.
    fn parse_bracket_name_in_property_and_param_tag(
        &mut self,
        target: PropertyLikeParse,
    ) -> (Node, bool) {
        // Looking for something like '[foo]', 'foo', '[foo.bar]' or 'foo.bar'
        let is_bracketed = self.parse_optional_jsdoc(SyntaxKind::OpenBracketToken);
        if is_bracketed {
            self.skip_whitespace();
        }
        // a markdown-quoted name: `arg` is not legal jsdoc, but occurs in the wild
        let is_backquoted = self.parse_optional_jsdoc(SyntaxKind::BacktickToken);
        let name = self.parse_js_doc_entity_name(if target == PROPERTY_LIKE_PARSE_PARAMETER {
            None
        } else {
            Some(diag::Identifier_expected)
        });
        if is_backquoted {
            self.parse_expected_token_js_doc(SyntaxKind::BacktickToken);
        }
        if is_bracketed {
            self.skip_whitespace();
            // May have an optional default, e.g. '[foo = 42]'
            if !self.parse_optional_token(SyntaxKind::EqualsToken).is_nil() {
                self.parse_expression();
            }

            self.parse_expected(SyntaxKind::CloseBracketToken);
        }

        (name, is_bracketed)
    }

    // Go: jsdoc.go:833 parseParameterOrPropertyTag
    fn parse_parameter_or_property_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        target: PropertyLikeParse,
        indent: i32,
    ) -> Node {
        let mut type_expression = self.try_parse_type_expression();
        let mut is_name_first = type_expression.is_nil();
        self.skip_whitespace_or_asterisk();

        let (name, is_bracketed) = self.parse_bracket_name_in_property_and_param_tag(target);
        let indent_text = self.skip_whitespace_or_asterisk();

        if is_name_first
            && self.look_ahead(|p: &mut Parser<'a>| p.parse_js_doc_link_prefix().is_none())
        {
            type_expression = self.try_parse_type_expression();
        }

        let pos = self.node_pos();
        let comment = self.parse_trailing_tag_comments(start, pos, indent, indent_text);

        let nested_type_literal =
            self.parse_nested_type_literal(type_expression, name, target, indent);
        if !nested_type_literal.is_nil() {
            type_expression = nested_type_literal;
            is_name_first = true;
        }
        let kind = if target == PROPERTY_LIKE_PARSE_PROPERTY {
            SyntaxKind::JsDocPropertyTag
        } else {
            SyntaxKind::JsDocParameterTag
        };
        let result = self.factory.new_js_doc_parameter_or_property_tag(
            kind,
            tag_name,
            name,
            is_bracketed,
            type_expression,
            is_name_first,
            comment,
        );
        self.finish_node(result, start)
    }

    // Go: jsdoc.go:858 parseNestedTypeLiteral
    fn parse_nested_type_literal(
        &mut self,
        type_expression: Node,
        name: Node,
        target: PropertyLikeParse,
        indent: i32,
    ) -> Node {
        if !type_expression.is_nil()
            && is_object_or_object_array_type_reference(type_expression.type_())
        {
            let pos = self.node_pos();
            let mut children: Vec<Node> = Vec::new();
            loop {
                let state = self.mark();
                let child = self.parse_child_parameter_or_property_tag(target, indent, name);
                if child.is_nil() {
                    self.rewind(state);
                    break;
                }
                match child.kind() {
                    SyntaxKind::JsDocParameterTag | SyntaxKind::JsDocPropertyTag => {
                        children.push(child)
                    }
                    SyntaxKind::JsDocTemplateTag => {
                        self.parse_error_at_range(
                            child.tag_name().loc(),
                            diag::A_JSDoc_template_tag_may_not_follow_a_typedef_callback_or_overload_tag,
                            vec![],
                        );
                    }
                    _ => {}
                }
            }
            // PORT: Go checks `children != nil`; it is non-nil only after an append.
            if !children.is_empty() {
                let is_array_type = type_expression.type_().kind() == SyntaxKind::ArrayType;
                let l = self
                    .factory
                    .new_js_doc_type_literal(&children, is_array_type);
                let literal = self.finish_node(l, pos);
                let e = self.factory.new_js_doc_type_expression(literal);
                return self.finish_node(e, pos);
            }
        }
        Node::NIL
    }

    // Go: jsdoc.go:884 parseReturnTag
    fn parse_return_tag(
        &mut self,
        previous_tags: &[Node],
        start: i32,
        tag_name: Node,
        indent: i32,
        indent_text: &'a str,
    ) -> Node {
        if previous_tags.iter().any(|&t| is_js_doc_return_tag(t)) {
            let end = self.scanner.token_start();
            self.parse_error_at(
                tag_name.pos(),
                end,
                diag::X_0_tag_already_specified,
                args![tag_name.text()],
            );
        }

        let type_expression = self.try_parse_type_expression();
        let pos = self.node_pos();
        let comment = self.parse_trailing_tag_comments(start, pos, indent, indent_text);
        let n = self
            .factory
            .new_js_doc_return_tag(tag_name, type_expression, comment);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:894 parseTypeTag
    /// Pass `indent = -1` to skip parsing trailing comments (as when a type tag
    /// is nested in a typedef).
    fn parse_type_tag(
        &mut self,
        previous_tags: &[Node],
        start: i32,
        tag_name: Node,
        indent: i32,
        indent_text: &'a str,
    ) -> Node {
        if previous_tags.iter().any(|&t| is_js_doc_type_tag(t)) {
            let end = self.scanner.token_start();
            self.parse_error_at(
                tag_name.pos(),
                end,
                diag::X_0_tag_already_specified,
                args![tag_name.text()],
            );
        }

        let type_expression = self.parse_js_doc_type_expression(true);
        let mut comments = NodeList::NIL;
        if indent != -1 {
            let pos = self.node_pos();
            comments = self.parse_trailing_tag_comments(start, pos, indent, indent_text);
        }
        let n = self
            .factory
            .new_js_doc_type_tag(tag_name, type_expression, comments);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:907 parseSeeTag
    fn parse_see_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        indent: i32,
        indent_text: &'a str,
    ) -> Node {
        let has_name_reference = (self.is_identifier()
            && !self.source_text[self.scanner.token_end() as usize..].starts_with("://"))
            || (self.token == SyntaxKind::OpenBraceToken
                && self.look_ahead(Self::next_token_is_identifier_or_keyword));
        let mut name_expression = Node::NIL;
        if has_name_reference {
            name_expression = self.parse_js_doc_name_reference();
        }
        let pos = self.node_pos();
        let comments = self.parse_trailing_tag_comments(start, pos, indent, indent_text);
        let n = self
            .factory
            .new_js_doc_see_tag(tag_name, name_expression, comments);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:918 parseImplementsTag
    fn parse_implements_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        margin: i32,
        indent_text: &'a str,
    ) -> Node {
        let class_name = self.parse_expression_with_type_arguments_for_augments();
        let pos = self.node_pos();
        let comment = self.parse_trailing_tag_comments(start, pos, margin, indent_text);
        let n = self
            .factory
            .new_js_doc_implements_tag(tag_name, class_name, comment);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:923 parseAugmentsTag
    fn parse_augments_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        margin: i32,
        indent_text: &'a str,
    ) -> Node {
        let class_name = self.parse_expression_with_type_arguments_for_augments();
        let pos = self.node_pos();
        let comment = self.parse_trailing_tag_comments(start, pos, margin, indent_text);
        let n = self
            .factory
            .new_js_doc_augments_tag(tag_name, class_name, comment);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:928 parseSatisfiesTag
    fn parse_satisfies_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        margin: i32,
        indent_text: &'a str,
    ) -> Node {
        let type_expression = self.parse_js_doc_type_expression(false);
        let pos = self.node_pos();
        let comments = self.parse_trailing_tag_comments(start, pos, margin, indent_text);
        let n = self
            .factory
            .new_js_doc_satisfies_tag(tag_name, type_expression, comments);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:934 parseThrowsTag
    fn parse_throws_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        margin: i32,
        indent_text: &'a str,
    ) -> Node {
        let type_expression = self.try_parse_type_expression();
        let pos = self.node_pos();
        let comment = self.parse_trailing_tag_comments(start, pos, margin, indent_text);
        let n = self
            .factory
            .new_js_doc_throws_tag(tag_name, type_expression, comment);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:940 parseImportTag
    fn parse_import_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        margin: i32,
        indent_text: &'a str,
    ) -> Node {
        let after_import_tag_pos = self.scanner.token_full_start();

        let mut identifier = Node::NIL;
        if self.is_identifier() {
            identifier = self.parse_identifier();
        }

        let import_clause = self.try_parse_import_clause(
            identifier,
            after_import_tag_pos,
            SyntaxKind::TypeKeyword,
            true, /*skipJSDocLeadingAsterisks*/
        );
        let module_specifier = self.parse_module_specifier();
        let attributes = self.try_parse_import_attributes();

        let pos = self.node_pos();
        let comments = self.parse_trailing_tag_comments(start, pos, margin, indent_text);
        let n = self.factory.new_js_doc_import_tag(
            tag_name,
            import_clause,
            module_specifier,
            attributes,
            comments,
        );
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:956 parseExpressionWithTypeArgumentsForAugments
    fn parse_expression_with_type_arguments_for_augments(&mut self) -> Node {
        let used_brace = self.parse_optional(SyntaxKind::OpenBraceToken);
        let pos = self.node_pos();
        let expression = self.parse_property_access_entity_name_expression();
        self.scanner.set_skip_js_doc_leading_asterisks(true);
        let type_arguments = self.parse_type_arguments();
        self.scanner.set_skip_js_doc_leading_asterisks(false);
        let n = self
            .factory
            .new_expression_with_type_arguments(expression, type_arguments);
        let node = self.finish_node(n, pos);
        if used_brace {
            self.skip_whitespace();
            self.parse_expected(SyntaxKind::CloseBraceToken);
        }
        node
    }

    // Go: jsdoc.go:971 parsePropertyAccessEntityNameExpression
    fn parse_property_access_entity_name_expression(&mut self) -> Node {
        let pos = self.node_pos();
        let mut node = self.parse_js_doc_identifier_name(Some(diag::Identifier_expected));
        while self.parse_optional(SyntaxKind::DotToken) {
            let name = self.parse_js_doc_identifier_name(Some(diag::Identifier_expected));
            let n =
                self.factory
                    .new_property_access_expression(node, Node::NIL, name, NodeFlags::NONE);
            node = self.finish_node(n, pos);
        }
        node
    }

    // Go: jsdoc.go:981 parseSimpleTag
    // PORT: Go passes a closure over `p.factory`. Here `create_tag` is a
    // `NodeFactory` method, called with `self.factory`.
    fn parse_simple_tag(
        &mut self,
        start: i32,
        create_tag: fn(&NodeFactory, Node, NodeList) -> Node,
        tag_name: Node,
        margin: i32,
        indent_text: &'a str,
    ) -> Node {
        let pos = self.node_pos();
        let comment = self.parse_trailing_tag_comments(start, pos, margin, indent_text);
        let n = create_tag(&self.factory, tag_name, comment);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:985 parseThisTag
    fn parse_this_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        margin: i32,
        indent_text: &'a str,
    ) -> Node {
        let type_expression = self.parse_js_doc_type_expression(true);
        self.skip_whitespace();
        let pos = self.node_pos();
        let comment = self.parse_trailing_tag_comments(start, pos, margin, indent_text);
        let result = self
            .factory
            .new_js_doc_this_tag(tag_name, type_expression, comment);
        self.finish_node(result, start)
    }

    // Go: jsdoc.go:992 parseJSDocTypeNameWithNamespace
    fn parse_js_doc_type_name_with_namespace(&mut self, nested: bool) -> Node {
        let start = self.scanner.token_start();
        if !token_is_identifier_or_keyword(self.token) {
            return Node::NIL;
        }
        let type_name_or_namespace_name = self.parse_js_doc_identifier_name(None);
        if self.parse_optional_jsdoc(SyntaxKind::DotToken) {
            let body = self.parse_js_doc_type_name_with_namespace(true /*nested*/);
            let js_doc_namespace_node = self.factory.new_module_declaration(
                ModifierList::NIL,            /*modifiers*/
                SyntaxKind::NamespaceKeyword, /*keyword*/
                type_name_or_namespace_name,
                Node::NIL, /*attributes*/
                body,
            );
            if nested {
                set_node_flags(
                    js_doc_namespace_node,
                    js_doc_namespace_node.flags() | NodeFlags::NESTED_NAMESPACE,
                );
            }
            return self.finish_node(js_doc_namespace_node, start);
        }
        if nested {
            set_node_flags(
                type_name_or_namespace_name,
                type_name_or_namespace_name.flags() | NodeFlags::IDENTIFIER_IS_IN_JS_DOC_NAMESPACE,
            );
        }
        type_name_or_namespace_name
    }

    // Go: jsdoc.go:1018 parseTypedefTag
    fn parse_typedef_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        indent: i32,
        indent_text: &'a str,
    ) -> Node {
        let mut type_expression = self.try_parse_type_expression();
        self.skip_whitespace_or_asterisk();
        let mut full_name = self.parse_js_doc_type_name_with_namespace(false /*nested*/);
        if full_name.is_nil() {
            full_name = self.parse_js_doc_identifier_name(Some(diag::Identifier_expected));
        }
        self.skip_whitespace();
        let mut comment = self.parse_tag_comments(indent, None);

        let mut end = -1;
        let mut has_children = false;
        if type_expression.is_nil()
            || is_object_or_object_array_type_reference(type_expression.type_())
        {
            let mut child_type_tag = Node::NIL;
            let mut jsdoc_property_tags: Vec<Node> = Vec::new();
            loop {
                let state = self.mark();
                let child = self.parse_child_property_tag(indent);
                if child.is_nil() {
                    self.rewind(state);
                    break;
                }
                has_children = true;
                match child.kind() {
                    SyntaxKind::JsDocTemplateTag => {
                        self.parse_error_at_range(
                            child.tag_name().loc(),
                            diag::A_JSDoc_template_tag_may_not_follow_a_typedef_callback_or_overload_tag,
                            vec![],
                        );
                    }
                    SyntaxKind::JsDocTypeTag => {
                        if child_type_tag.is_nil() {
                            child_type_tag = child;
                        } else {
                            // PORT: Go gets the appended `*Diagnostic` back and
                            // adds related info through the pointer. Here the
                            // new diagnostic is the last one in the shared sink.
                            let before = self.diagnostics.borrow().diagnostics.len();
                            self.parse_error_at_current_token(
                                diag::A_JSDoc_typedef_comment_may_not_contain_multiple_type_tags,
                                vec![],
                            );
                            let mut sink = self.diagnostics.borrow_mut();
                            if sink.diagnostics.len() > before {
                                let related = new_diagnostic(
                                    Node::NIL,
                                    TextRange::new(0, 0),
                                    diag::The_tag_was_first_specified_here,
                                    vec![],
                                );
                                if let Some(last_error) = sink.diagnostics.last_mut() {
                                    last_error.add_related_info(Some(related));
                                }
                            }
                        }
                    }
                    _ => jsdoc_property_tags.push(child),
                }
            }
            if has_children {
                let is_array_type = !type_expression.is_nil()
                    && type_expression.type_().kind() == SyntaxKind::ArrayType;
                let jsdoc_type_literal = self
                    .factory
                    .new_js_doc_type_literal(&jsdoc_property_tags, is_array_type);
                if !child_type_tag.is_nil()
                    && !child_type_tag.type_expression().is_nil()
                    && !is_object_or_object_array_type_reference(
                        child_type_tag.type_expression().type_(),
                    )
                {
                    type_expression = child_type_tag.type_expression();
                } else {
                    // !!! This differs from Strada but prevents a crash
                    let pos = if let Some(first) = jsdoc_property_tags.first() {
                        first.pos()
                    } else {
                        start
                    };
                    type_expression = self.finish_node(jsdoc_type_literal, pos);
                }
                end = type_expression.end();
            }
        }

        // Only include the characters between the name end and the next token if a comment was actually parsed out - otherwise it's just whitespace
        if end == -1 {
            end = if has_children && !type_expression.is_nil() {
                type_expression.end()
            } else if !comment.is_nil() {
                self.node_pos()
            } else if !full_name.is_nil() {
                full_name.end()
            } else if !type_expression.is_nil() {
                type_expression.end()
            } else {
                tag_name.end()
            };
        }

        if comment.is_nil() {
            comment = self.parse_trailing_tag_comments(start, end, indent, indent_text);
        }

        let n = self
            .factory
            .new_js_doc_typedef_tag(tag_name, type_expression, full_name, comment);
        let typedef_tag = self.finish_node_with_end(n, start, end);
        if !type_expression.is_nil() {
            // forcibly overwrite parent potentially set by inner type expression parse
            set_node_parent(type_expression, typedef_tag);
        }
        typedef_tag
    }

    // Go: jsdoc.go:1102 parseCallbackTagParameters
    fn parse_callback_tag_parameters(&mut self, indent: i32) -> NodeList {
        let mut parameters: Vec<Node> = Vec::new();
        let pos = self.node_pos();
        loop {
            let state = self.mark();
            let child = self.parse_child_parameter_or_property_tag(
                PROPERTY_LIKE_PARSE_CALLBACK_PARAMETER,
                indent,
                Node::NIL,
            );
            if child.is_nil() {
                self.rewind(state);
                break;
            }
            if child.kind() == SyntaxKind::JsDocTemplateTag {
                self.parse_error_at_range(
                    child.tag_name().loc(),
                    diag::A_JSDoc_template_tag_may_not_follow_a_typedef_callback_or_overload_tag,
                    vec![],
                );
            } else {
                parameters.push(child);
            }
        }
        let end = self.node_pos();
        self.new_node_list(TextRange::new(pos, end), &parameters)
    }

    /// The speculative tag parse `site` at the current token (`indent`
    /// and `name` are the arguments of the call site), or its replay. The
    /// caller discards the result and rewinds to its mark when
    /// `js_doc_tag_is_discarded`.
    // PERF: jsdocmem1. Go parses the tag after a JSDoc signature
    // (`parseJSDocSignature`) and a child tag of an object type
    // (`parseChildParameterOrPropertyTag`) speculatively, and that parse
    // does the same for the tag after it. So a comment with N such tags
    // parses O(N^2) tags. Go's GC frees the discarded nodes, but here they
    // stay in the file's store and the AST arena (2.8 GB for 4,000
    // `@overload` tags, Go 14 MB). The first parse at a state is real. When
    // its result is discarded, its counts are kept here, and a later parse
    // at the same state adds the counts and gives the same discarded
    // result, without new nodes. The nodes, lists, positions and
    // diagnostics of the live tree are Go's: the caller discards the result
    // and rewinds. Go keeps only these effects after the rewind: the node
    // and text counts of the factory, `identifierCount` and
    // `hasDeprecatedTag`; the replay adds them. A parse that changes a
    // parser cache (`notParenthesizedArrow`, `possibleAwaitSpans`) is not
    // kept: a later Go parse at that state reads the cache and can make
    // fewer nodes. The key holds the size of `notParenthesizedArrow`, so a
    // record is not replayed after another parse (other context flags) adds
    // to it. Out of line, so the rest of the JSDoc parser compiles as
    // before.
    #[inline(never)]
    fn speculate_js_doc_tag(&mut self, site: JsDocSpeculation, indent: i32, name: Node) -> Node {
        let scanner = self.scanner.mark();
        let key = JsDocDiscardKey {
            site,
            indent,
            pos: scanner.pos,
            full_start_pos: scanner.full_start_pos,
            token_start: scanner.token_start,
            token: scanner.token,
            token_flags: scanner.token_flags,
            skip_js_doc_leading_asterisks: scanner.skip_js_doc_leading_asterisks,
            context_flags: self.context_flags,
            parsing_contexts: self.parsing_contexts,
            not_parenthesized_arrows: self.not_parenthesized_arrow.len(),
        };
        #[cfg(test)]
        let replay = !tests::NO_REPLAY.get();
        #[cfg(not(test))]
        let replay = true;
        if replay
            && let Some(d) = self.jsdoc_discards.get(&key).copied()
            && js_doc_tag_is_discarded(site, d.result, name)
        {
            self.factory.add_counts(d.node_count, d.text_count);
            self.identifier_count += d.identifier_count;
            self.has_deprecated_tag |= d.sets_deprecated_tag;
            return d.result;
        }
        let node_count = self.factory.node_count();
        let text_count = self.factory.text_count();
        let identifier_count = self.identifier_count;
        let arrows = key.not_parenthesized_arrows;
        let await_spans = self.possible_await_spans.len();
        // Only a `@deprecated` tag sets it, and only `with_js_doc` reads
        // it: cleared here, it tells whether this parse sets it.
        let had_deprecated_tag = std::mem::replace(&mut self.has_deprecated_tag, false);
        let result = match site {
            JsDocSpeculation::SignatureTag => self.parse_tag(&[], indent),
            JsDocSpeculation::ChildTag(target) => self.try_parse_child_tag(target, indent),
        };
        let sets_deprecated_tag = self.has_deprecated_tag;
        self.has_deprecated_tag |= had_deprecated_tag;
        if replay
            && js_doc_tag_is_discarded(site, result, name)
            && self.not_parenthesized_arrow.len() == arrows
            && self.possible_await_spans.len() == await_spans
        {
            self.jsdoc_discards.insert(
                key,
                JsDocDiscard {
                    result,
                    node_count: self.factory.node_count() - node_count,
                    text_count: self.factory.text_count() - text_count,
                    identifier_count: self.identifier_count - identifier_count,
                    sets_deprecated_tag,
                },
            );
        }
        result
    }

    // Go: jsdoc.go:1122 parseJSDocSignature
    fn parse_js_doc_signature(&mut self, start: i32, indent: i32) -> Node {
        let parameters = self.parse_callback_tag_parameters(indent);
        let mut return_tag = Node::NIL;
        let state = self.mark();
        if self.parse_optional_jsdoc(SyntaxKind::AtToken) {
            let tag = self.speculate_js_doc_tag(JsDocSpeculation::SignatureTag, indent, Node::NIL);
            if tag.kind() == SyntaxKind::JsDocReturnTag {
                return_tag = tag;
            }
        }
        if return_tag.is_nil() {
            self.rewind(state);
        }
        let n = self
            .factory
            .new_js_doc_signature(NodeList::NIL, parameters, return_tag);
        self.finish_node(n, start)
    }

    // Go: jsdoc.go:1138 parseCallbackTag
    fn parse_callback_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        indent: i32,
        indent_text: &'a str,
    ) -> Node {
        let mut full_name = self.parse_js_doc_type_name_with_namespace(false /*nested*/);
        if full_name.is_nil() {
            full_name = self.parse_js_doc_identifier_name(Some(diag::Identifier_expected));
        }
        self.skip_whitespace();
        let mut comment = self.parse_tag_comments(indent, None);
        let sig_pos = self.node_pos();
        let type_expression = self.parse_js_doc_signature(sig_pos, indent);
        if comment.is_nil() {
            let pos = self.node_pos();
            comment = self.parse_trailing_tag_comments(start, pos, indent, indent_text);
        }
        let end = if !comment.is_nil() {
            self.node_pos()
        } else {
            type_expression.end()
        };
        let n = self
            .factory
            .new_js_doc_callback_tag(tag_name, type_expression, full_name, comment);
        self.finish_node_with_end(n, start, end)
    }

    // Go: jsdoc.go:1158 parseOverloadTag
    fn parse_overload_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        indent: i32,
        indent_text: &'a str,
    ) -> Node {
        self.skip_whitespace();
        let mut comment = self.parse_tag_comments(indent, None);
        let type_expression = self.parse_js_doc_signature(start, indent);
        if comment.is_nil() {
            let pos = self.node_pos();
            comment = self.parse_trailing_tag_comments(start, pos, indent, indent_text);
        }
        let end = if !comment.is_nil() {
            self.node_pos()
        } else {
            type_expression.end()
        };
        let n = self
            .factory
            .new_js_doc_overload_tag(tag_name, type_expression, comment);
        self.finish_node_with_end(n, start, end)
    }

    // Go: jsdoc.go:1186 parseChildPropertyTag
    fn parse_child_property_tag(&mut self, indent: i32) -> Node {
        self.parse_child_parameter_or_property_tag(PROPERTY_LIKE_PARSE_PROPERTY, indent, Node::NIL)
    }

    // Go: jsdoc.go:1190 parseChildParameterOrPropertyTag
    fn parse_child_parameter_or_property_tag(
        &mut self,
        target: PropertyLikeParse,
        indent: i32,
        name: Node,
    ) -> Node {
        let mut can_parse_tag = true;
        let mut seen_asterisk = false;
        loop {
            match self.next_token_js_doc() {
                SyntaxKind::AtToken => {
                    if can_parse_tag && self.scanner.can_follow_js_doc_at() {
                        // Every caller rewinds to its mark when this returns nil.
                        let child = self.speculate_js_doc_tag(
                            JsDocSpeculation::ChildTag(target),
                            indent,
                            name,
                        );
                        if is_other_child_tag(child, name) {
                            return Node::NIL;
                        }
                        return child;
                    }
                    seen_asterisk = false;
                }
                SyntaxKind::NewLineTrivia => {
                    can_parse_tag = true;
                    seen_asterisk = false;
                }
                SyntaxKind::AsteriskToken => {
                    if seen_asterisk {
                        can_parse_tag = false;
                    }
                    seen_asterisk = true;
                }
                SyntaxKind::Identifier => {
                    can_parse_tag = false;
                }
                SyntaxKind::EndOfFile => {
                    return Node::NIL;
                }
                _ => {}
            }
        }
    }

    // Go: jsdoc.go:1222 tryParseChildTag
    fn try_parse_child_tag(&mut self, target: PropertyLikeParse, indent: i32) -> Node {
        assert!(
            self.token == SyntaxKind::AtToken,
            "should only be called when at @"
        );
        let start = self.scanner.token_full_start();
        self.next_token_js_doc();

        let tag_name = self.parse_js_doc_identifier_name(Some(diag::Identifier_expected));
        let indent_text = self.skip_whitespace_or_asterisk();
        let t: PropertyLikeParse;
        match tag_name.text() {
            "type" => {
                if target == PROPERTY_LIKE_PARSE_PROPERTY {
                    return self.parse_type_tag(&[], start, tag_name, -1, "");
                }
                // Go: a `type` tag in another target leaves `t` at zero.
                t = 0;
            }
            "prop" | "property" => t = PROPERTY_LIKE_PARSE_PROPERTY,
            "arg" | "argument" | "param" => {
                t = PROPERTY_LIKE_PARSE_PARAMETER | PROPERTY_LIKE_PARSE_CALLBACK_PARAMETER
            }
            "template" => return self.parse_template_tag(start, tag_name, indent, indent_text),
            "this" => return self.parse_this_tag(start, tag_name, indent, indent_text),
            _ => return Node::NIL,
        }
        if (target & t) == 0 {
            return Node::NIL;
        }
        self.parse_parameter_or_property_tag(start, tag_name, target, indent)
    }

    // Go: jsdoc.go:1254 parseTemplateTagTypeParameter
    fn parse_template_tag_type_parameter(&mut self) -> Node {
        let type_parameter_pos = self.node_pos();
        let is_bracketed = self.parse_optional_jsdoc(SyntaxKind::OpenBracketToken);
        if is_bracketed {
            self.skip_whitespace();
        }

        let modifiers = self.parse_modifiers_ex(false, true /*permitConstAsModifier*/, false);
        let name = self.parse_js_doc_identifier_name(Some(
            diag::Unexpected_token_A_type_parameter_name_was_expected_without_curly_braces,
        ));
        let mut default_type = Node::NIL;
        if is_bracketed {
            self.skip_whitespace();
            self.parse_expected(SyntaxKind::EqualsToken);
            let save_context_flags = self.context_flags;
            self.set_context_flags(NodeFlags::JS_DOC, true);
            default_type = self.parse_js_doc_type();
            self.context_flags = save_context_flags;
            self.parse_expected(SyntaxKind::CloseBracketToken);
        }

        if node_is_missing(name) {
            return Node::NIL;
        }
        let n = self.factory.new_type_parameter_declaration(
            modifiers,
            name,
            Node::NIL, /*constraint*/
            Node::NIL, /*expression*/
            default_type,
        );
        self.finish_node(n, type_parameter_pos)
    }

    // Go: jsdoc.go:1280 parseTemplateTagTypeParameters
    // PORT: Go builds a zero-value `ast.TypeParameterList{}` (not through the
    // factory), so its `Loc` is {0, 0}. The list is made at the end with that
    // range.
    fn parse_template_tag_type_parameters(&mut self) -> NodeList {
        let mut nodes: Vec<Node> = Vec::new();
        loop {
            // do-while loop
            self.skip_whitespace();
            let node = self.parse_template_tag_type_parameter();
            if !node.is_nil() {
                nodes.push(node);
            }
            self.skip_whitespace_or_asterisk();
            if !self.parse_optional_jsdoc(SyntaxKind::CommaToken) {
                break;
            }
        }
        self.new_node_list(TextRange::new(0, 0), &nodes)
    }

    // Go: jsdoc.go:1293 parseTemplateTag
    fn parse_template_tag(
        &mut self,
        start: i32,
        tag_name: Node,
        indent: i32,
        indent_text: &'a str,
    ) -> Node {
        // The template tag looks like one of the following:
        //   @template T,U,V
        //   @template {Constraint} T
        //
        // According to the [closure docs](https://github.com/google/closure-compiler/wiki/Generic-Types#multiple-bounded-template-types):
        //   > Multiple bounded generics cannot be declared on the same line. For the sake of clarity, if multiple templates share the same
        //   > type bound they must be declared on separate lines.
        //
        // TODO: Determine whether we should enforce this in the checker.
        // TODO: Consider moving the `constraint` to the first type parameter as we could then remove `getEffectiveConstraintOfTypeParameter`.
        // TODO: Consider only parsing a single type parameter if there is a constraint.
        let mut constraint = Node::NIL;
        if self.token == SyntaxKind::OpenBraceToken {
            constraint = self.parse_js_doc_type_expression(false);
        }
        let type_parameters = self.parse_template_tag_type_parameters();
        let pos = self.node_pos();
        let comment = self.parse_trailing_tag_comments(start, pos, indent, indent_text);
        let result =
            self.factory
                .new_js_doc_template_tag(tag_name, constraint, type_parameters, comment);
        self.finish_node(result, start)
    }

    // Go: jsdoc.go:1314 parseOptionalJsdoc
    pub(crate) fn parse_optional_jsdoc(&mut self, t: SyntaxKind) -> bool {
        if self.token == t {
            self.next_token_js_doc();
            return true;
        }
        false
    }

    // Go: jsdoc.go:1322 parseJSDocEntityName
    fn parse_js_doc_entity_name(&mut self, diagnostic_message: Option<&'static Message>) -> Node {
        let mut entity = self.parse_js_doc_identifier_name(diagnostic_message);
        if self.parse_optional(SyntaxKind::OpenBracketToken) {
            self.parse_expected(SyntaxKind::CloseBracketToken);
            // Note that y[] is accepted as an entity name, but the postfix brackets are not saved for checking.
            // Technically usejsdoc.org requires them for specifying a property of a type equivalent to Array<{ x: ...}>
            // but it's not worth it to enforce that restriction.
        }
        while self.parse_optional(SyntaxKind::DotToken) {
            let name = self.parse_js_doc_identifier_name(Some(diag::Identifier_expected));
            if self.parse_optional(SyntaxKind::OpenBracketToken) {
                self.parse_expected(SyntaxKind::CloseBracketToken);
            }
            let pos = entity.pos();
            let q = self.factory.new_qualified_name(entity, name);
            entity = self.finish_node(q, pos);
        }
        entity
    }

    // Go: jsdoc.go:1341 parseJSDocIdentifierName
    fn parse_js_doc_identifier_name(
        &mut self,
        diagnostic_message: Option<&'static Message>,
    ) -> Node {
        if !token_is_identifier_or_keyword(self.token) {
            if let Some(diagnostic_message) = diagnostic_message {
                self.parse_error_at_current_token(diagnostic_message, vec![]);
            } else if is_reserved_word(self.token) {
                let text: String = self.scanner.token_text().to_string();
                self.parse_error_at_current_token(
                    diag::Identifier_expected_0_is_a_reserved_word_that_cannot_be_used_here,
                    args![text],
                );
            }
            let n = self.new_identifier("");
            let pos = self.node_pos();
            return self.finish_node(n, pos);
        }
        let pos = self.scanner.token_start();
        let end = self.scanner.token_end();
        let text = self.scanner.token_value();
        self.next_token_js_doc();
        let n = self.new_identifier(text);
        self.finish_node_with_end(n, pos, end)
    }
}

// Go: jsdoc.go:380 removeLeadingNewlines
// PORT: Go returns the subslice `comments[i:]`; this drops the prefix in place.
fn remove_leading_newlines(comments: &mut Vec<Cow<'_, str>>) {
    let mut i = 0;
    while i < comments.len() && comments[i].trim_start_matches(['\r', '\n']).is_empty() {
        i += 1;
    }
    comments.drain(..i);
}

// Go: jsdoc.go:392 removeTrailingWhitespace
// PORT: Go returns the subslice `comments[:end]`; this truncates in place.
// A part that trims to empty is dropped, so trimming it first is the same.
// Go `trimEnd` (jsdoc.go:388) is `trim_part_end` with `is_white_space_like`.
fn remove_trailing_whitespace(comments: &mut Vec<Cow<'_, str>>) {
    let mut end = comments.len();
    for i in (0..comments.len()).rev() {
        trim_part_end(&mut comments[i], is_white_space_like);
        if comments[i].is_empty() {
            end = i;
        } else {
            break;
        }
    }
    comments.truncate(end);
}

// Go: jsdoc.go:776 isJSDocLinkTag
fn is_js_doc_link_tag(kind: &str) -> bool {
    kind == "link" || kind == "linkcode" || kind == "linkplain"
}

// Go: jsdoc.go:818 isObjectOrObjectArrayTypeReference
fn is_object_or_object_array_type_reference(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ObjectKeyword => true,
        SyntaxKind::ArrayType => is_object_or_object_array_type_reference(node.element_type()),
        _ => {
            if is_type_reference_node(node) {
                let type_name = node.type_name();
                return is_identifier(type_name)
                    && type_name.text() == "Object"
                    && node.type_argument_list().is_nil();
            }
            false
        }
    }
}

// Go: jsdoc.go:1174 textsEqual
fn texts_equal(mut a: Node, mut b: Node) -> bool {
    while !is_identifier(a) || !is_identifier(b) {
        if !is_identifier(a) && !is_identifier(b) && a.right().text() == b.right().text() {
            a = a.left();
            b = b.left();
        } else {
            return false;
        }
    }
    a.text() == b.text()
}

#[cfg(test)]
mod tests {
    use super::jsdoc_text_cut;
    use crate::frontend::parser::{ParsedSourceFile, SourceFileParseOptions, parse_source_file};
    use crate::frontend::prelude::*;
    use crate::frontend::tspath::Path;
    use crate::scanner_util::go_string_from_bytes;
    use std::cell::Cell;

    thread_local! {
        /// Set: `speculate_js_doc_tag` always parses, as Go.
        pub(super) static NO_REPLAY: Cell<bool> = const { Cell::new(false) };
    }

    /// The node tree of `n` (kind, range, flags, lists) with its eager
    /// JSDoc, as the astdump tool writes it.
    fn dump(file: &ParsedSourceFile, n: Node, depth: usize, out: &mut String) {
        let mut lists = String::new();
        let mut children = Vec::new();
        n.for_each_child_and_lists(
            &mut |c| {
                children.push(c);
                false
            },
            &mut |l, is_mod| {
                let tc = if l.has_trailing_comma() { ",tc" } else { "" };
                let m = if is_mod { "M" } else { "L" };
                lists += &format!(" {m}{}-{}:{}{tc}", l.pos(), l.end(), l.nodes().len());
            },
        );
        let (pos, end, flags) = (n.pos(), n.end(), n.flags().0);
        *out += &format!("{depth} {:?} {pos} {end} {flags:x}{lists}\n", n.kind());
        for &j in file.jsdoc_cache.get(&n).into_iter().flatten() {
            *out += "@";
            dump(file, j, depth + 1, out);
        }
        for c in children {
            dump(file, c, depth + 1, out);
        }
    }

    /// The tree, the diagnostics and the Go counts of a parse of `text`.
    fn parse(text: &str, replay: bool) -> String {
        NO_REPLAY.set(!replay);
        let file = parse_source_file(
            &SourceFileParseOptions {
                file_name: "/a.js".to_string(),
                path: Path("/a.js".to_string()),
                ..Default::default()
            },
            &*Box::leak(text.into()),
            ScriptKind::JS,
        );
        NO_REPLAY.set(false);
        let mut out = format!(
            "identifiers {} nodes {} texts {}\n",
            file.identifier_count, file.node_count, file.text_count
        );
        let all = [
            &file.diagnostics,
            &file.js_diagnostics,
            &file.jsdoc_diagnostics,
        ];
        for d in all.into_iter().flatten() {
            out += &format!("diag {} {} {} {:?}\n", d.pos, d.end, d.code, d.message_args);
        }
        dump(&file, file.root, 0, &mut out);
        out
    }

    /// jsdocmem1: a replayed speculative tag parse gives Go's tree,
    /// diagnostics and counts (Go parses each speculative tag again).
    #[test]
    fn replayed_speculative_jsdoc_tags_match_a_full_parse() {
        let tags = |tag: &str, n: usize| (0..n).map(|_| tag).collect::<Vec<_>>().join(" ");
        let cases = [
            format!("/** {} */\nfunction f() {{}}\n", tags("@overload", 7)),
            format!("/** {} */\nfunction f() {{}}\n", tags("@callback C", 7)),
            format!(
                "/** {} */\nfunction f(a) {{}}\n",
                tags("@param {Object} a", 7)
            ),
            format!(
                "/**\n * {} */\nfunction f(a) {{}}\n",
                tags("@param {Object} a\n *", 7)
            ),
            format!(
                "/** @typedef {{Object}} T {} */\n",
                tags("@property {Object} a", 7)
            ),
            format!(
                "/** {} @returns {{number}} */\nfunction f() {{}}\n",
                tags("@overload @param {string} x", 5)
            ),
            format!(
                "/** {} @overload @deprecated */\nfunction f() {{}}\n",
                tags("@overload", 5)
            ),
            "/** @param {Object} a @param {Object} a.b @param {Object} a.b.c @param {Object} b \
             @param {Object} b.c @param {Object} b.c.d @param {Object} a @param {Object} a.b */\n\
             function f(a, b) {}\n"
                .to_string(),
            format!(
                "/** {} */\nfunction f() {{}}\n",
                tags("@overload @param {{[(x) => 1]: number}} p", 5)
            ),
            format!(
                "/** {} */\nfunction f() {{}}\n",
                tags("@overload @param {Object} @template T @param {", 5)
            ),
        ];
        for text in &cases {
            assert_eq!(parse(text, true), parse(text, false), "{text}");
        }
    }

    /// jsdocmem1: one comment parsed 3 times: `in` allowed, then not
    /// allowed, then allowed again. The second parse fails the arrow
    /// function of the `[z = ...]` default and adds it to
    /// `notParenthesizedArrow`. So the third parse must not replay the
    /// first: Go skips that arrow then. Without the set size in the key,
    /// the replay gave 1 more identifier than Go (R237 `tsgo
    /// --extendedDiagnostics`: 71, Go 70).
    #[test]
    fn replay_reads_the_not_parenthesized_arrow_set() {
        // Parse 1: the true side of `c ? ... : ...` allows `in`. The arrow
        // `(y): U => ...` parses its body, then fails at the `;`. Parse 2:
        // the false side keeps the DisallowIn of the `for` initializer, and
        // the arrow `(r = ..., w = s in 1) => 0` fails at `in 1`. Parse 3:
        // that text as a parenthesized expression allows `in`. The
        // parameters of `h` give the 3 parses the same parsing contexts.
        let text = "function h(k = () => { for (c ? (y): U => (r = T => {\n\
            /** @overload @overload @param {number} [z = (a = b in 1) => g] */\n\
            function f() {}\n\
            }, w = s in 1) => 0 ;;); }) {}\n";
        assert_eq!(parse(text, true), parse(text, false));
    }

    /// The cut of a closed comment keeps no bytes and allocates nothing
    /// (the R159 reviewer: it ran 2 walks and 1 allocation per JSDoc
    /// comment). A cut inside a char keeps Go's bytes of it.
    #[test]
    fn jsdoc_text_cut_allocates_only_for_a_cut_char() {
        let text = "/** a */";
        let (cut, kept) = jsdoc_text_cut(text, text.len());
        assert_eq!((cut, kept.capacity()), (text.len() - 2, 0));
        // `日` is 3 Go bytes: the cut keeps its first byte.
        let text = "/** \u{65E5}";
        assert_eq!(jsdoc_text_cut(text, text.len()), (4, vec![0xE6]));
        // The invalid byte FF and `y` (1 Go byte each, 8 port bytes): the cut
        // is at the unit, with nothing kept.
        let text = go_string_from_bytes(b"/** \xFFy".to_vec());
        let (cut, kept) = jsdoc_text_cut(&text, text.len());
        assert_eq!((cut, kept.capacity()), (4, 0));
    }
}
