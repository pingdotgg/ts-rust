use crate::format::prelude::*;

use crate::flags_macros::go_enum;
use crate::frontend::core_textchange::TextChange;
use crate::frontend::scanner::scanner_p1::{rune_to_char, utf8_decode_rune_in_string};
use std::cell::Cell;

// Go: format/span.go:20 findEnclosingNode
/** find node that fully contains given text range */
pub fn find_enclosing_node(r: TextRange, source_file: Node) -> Node {
    // PORT: Go `find` is a recursive local closure.
    fn find(n: Node, r: TextRange, source_file: Node) -> Node {
        let mut candidate = Node::NIL;
        n.for_each_child(|c| {
            if c.flags().intersects(NodeFlags::REPARSED) {
                return false;
            }
            if r.contained_by(with_token_start(c, source_file)) {
                candidate = c;
                return true;
            }
            false
        });
        if candidate.is_some() {
            let result = find(candidate, r, source_file);
            if result.is_some() {
                return result;
            }
        }

        n
    }
    find(source_file, r, source_file)
}

// Go: format/span.go:51 getScanStartPosition
/*
 * Start of the original range might fall inside the comment - scanner will not yield appropriate results
 * This function will look for token that is located before the start of target range
 * and return its end as start position for the scanner.
 */
pub fn get_scan_start_position(
    enclosing_node: Node,
    original_range: TextRange,
    source_file: Node,
) -> i32 {
    let adjusted = with_token_start(enclosing_node, source_file);
    let start = adjusted.pos();
    if start == original_range.pos() && enclosing_node.end() == original_range.end() {
        return start;
    }

    // exclude JSDoc so the scan never starts inside a JSDoc comment
    let preceding_token = astnav::find_preceding_token_ex(
        source_file,
        original_range.pos(),
        Node::NIL, /*startNode*/
        true,      /*excludeJSDoc*/
    );
    if preceding_token.is_nil() {
        // no preceding token found - start from the beginning of enclosing node
        return enclosing_node.pos();
    }

    // preceding token ends after the start of original range (i.e when originalRange.pos falls in the middle of literal)
    // start from the beginning of enclosingNode to handle the entire 'originalRange'
    if preceding_token.end() >= original_range.pos() {
        return enclosing_node.pos();
    }

    preceding_token.end()
}

// Go: format/span.go:89 getOwnOrInheritedDelta
/*
 * For cases like
 * if (a ||
 *     b ||$
 *     c) {...}
 * If we hit Enter at $ we want line '    b ||' to be indented.
 * Formatting will be applied to the last two lines.
 * Node that fully encloses these lines is binary expression 'a ||...'.
 * Initial indentation for this node will be 0.
 * Binary expressions don't introduce new indentation scopes, however it is possible
 * that some parent node on the same line does - like if statement in this case.
 * Note that we are considering parents only from the same line with initial node -
 * if parent is on the different line - its delta was already contributed
 * to the initial indentation.
 */
pub fn get_own_or_inherited_delta(
    mut n: Node,
    options: &lsutil::FormatCodeSettings,
    source_file: Node,
) -> i32 {
    let mut previous_line = -1;
    let mut child = Node::NIL;
    while n.is_some() {
        let line = get_ecma_line_of_position(source_file, with_token_start(n, source_file).pos());
        if previous_line != -1 && line != previous_line {
            break;
        }

        if should_indent_child_node(options, n, child, source_file, &[]) {
            return options.editor_settings.indent_size; // !!! nil check???
        }

        previous_line = line;
        child = n;
        n = n.parent();
    }
    0
}

// Go: format/span.go:109 rangeHasNoErrors
pub fn range_has_no_errors(_: TextRange) -> bool {
    false
}

// Go: format/span.go:113 prepareRangeContainsErrorFunction
// PORT: Go returns `func(r core.TextRange) bool`. The closure advances
// `index`, so it is `FnMut`. It keeps the ranges of the errors it picks,
// not the errors, so it does not borrow `errors` (a file version guard).
pub fn prepare_range_contains_error_function(
    errors: &[Diagnostic],
    original_range: TextRange,
) -> Box<dyn FnMut(TextRange) -> bool> {
    if errors.is_empty() {
        return Box::new(range_has_no_errors);
    }

    // pick only errors that fall in range
    let mut sorted: Vec<TextRange> = errors
        .iter()
        .map(Diagnostic::loc)
        .filter(|loc| original_range.overlaps(*loc))
        .collect();
    if sorted.is_empty() {
        return Box::new(range_has_no_errors);
    }
    // Go: format/span.go:125 slices.SortStableFunc(sorted, a.Pos() - b.Pos())
    crate::gostd::slices::sort_stable_func(&mut sorted, |a, b| a.pos().cmp(&b.pos()) as i32);

    let mut index = 0;
    Box::new(move |r: TextRange| -> bool {
        // in current implementation sequence of arguments [r1, r2...] is monotonically increasing.
        // 'index' tracks the index of the most recent error that was checked.
        loop {
            if index >= sorted.len() {
                // all errors in the range were already checked -> no error in specified range
                return false;
            }

            let err = sorted[index];

            if r.end() <= err.pos() {
                // specified range ends before the error referred by 'index' - no error in range
                return false;
            }

            if r.overlaps(err) {
                // specified range overlaps with error range
                return true;
            }

            index += 1;
        }
    })
}

// Go: format/span.go:155 formatSpanWorker
// PORT: Go keeps the NodeVisitor built in `execute` in `w.visitor`. The Rust
// visitor holds `&mut FormatSpanWorker<'t>` as its ctx, so it cannot live in the
// worker; `execute_process_node_visitor` builds one per call with the same
// callbacks (`new_format_span_visitor`). The worker owns the formatting
// scanner (see `new_formatting_scanner`), which borrows the file text
// (`'t`).
pub struct FormatSpanWorker<'t> {
    pub original_range: TextRange,
    pub enclosing_node: Node,
    pub initial_indentation: i32,
    pub delta: i32,
    pub request_kind: FormatRequestKind,
    pub range_contains_error: Box<dyn FnMut(TextRange) -> bool>,
    pub source_file: Node,

    pub ctx: Context,

    pub formatting_scanner: Option<FormattingScanner<'t>>,
    pub formatting_context: Option<FormattingContext>,

    pub edits: Vec<TextChange>,
    pub previous_range: TextRangeWithKind,
    pub previous_range_trivia_end: i32,
    pub previous_parent: Node,
    pub previous_range_start_line: i32,

    pub child_context_node: Node,
    pub last_indented_line: i32,
    pub indentation_on_last_indented_line: i32,

    pub visiting_node: Node,
    pub visiting_indenter: Option<Rc<DynamicIndenter>>,
    pub visiting_node_start_line: i32,
    pub visiting_undecorated_node_start_line: i32,

    pub current_rules: Vec<&'static RuleImpl>,
}

// Go: format/span.go:188 newFormatSpanWorker
// PORT: `range_contains_error` is 'static: every caller passes a closure over
// the file's parse diagnostics (`&'static`) or none.
pub fn new_format_span_worker<'t>(
    ctx: &Context,
    original_range: TextRange,
    enclosing_node: Node,
    initial_indentation: i32,
    delta: i32,
    request_kind: FormatRequestKind,
    range_contains_error: Box<dyn FnMut(TextRange) -> bool>,
    source_file: Node,
) -> FormatSpanWorker<'t> {
    FormatSpanWorker {
        ctx: ctx.clone(),
        original_range,
        enclosing_node,
        initial_indentation,
        delta,
        request_kind,
        range_contains_error,
        source_file,
        formatting_scanner: None,
        formatting_context: None,
        edits: Vec::new(),
        previous_range: TextRangeWithKind::default(),
        previous_range_trivia_end: 0,
        previous_parent: Node::NIL,
        previous_range_start_line: 0,
        child_context_node: Node::NIL,
        last_indented_line: 0,
        indentation_on_last_indented_line: 0,
        visiting_node: Node::NIL,
        visiting_indenter: None,
        visiting_node_start_line: 0,
        visiting_undecorated_node_start_line: 0,
        current_rules: Vec::with_capacity(32), // increaseInsertionIndex should assert there are no more than 32 rules in a given bucket
    }
}

// Go: format/span.go:211 getNonDecoratorTokenPosOfNode
pub fn get_non_decorator_token_pos_of_node(node: Node, mut file: Node) -> i32 {
    let mut last_decorator = Node::NIL;
    if has_decorators(node) {
        last_decorator = node
            .modifier_nodes()
            .iter()
            .rev()
            .find(|&m| is_decorator(m))
            .unwrap_or(Node::NIL);
    }
    if file.is_nil() {
        file = get_source_file_of_node(node);
    }
    if last_decorator.is_nil() {
        return with_token_start(node, file).pos();
    }
    skip_trivia(&source_file_text(file), last_decorator.end())
}

/// Go `w.visitor` (built in `execute`).
pub type FormatSpanVisitor<'a, 't> = NodeVisitor<'a, &'a mut FormatSpanWorker<'t>>;

// Go: format/span.go:231 (the visitor built in execute)
// PORT: see `FormatSpanWorker`. Go passes `&ast.NodeFactory{}`; `None` is the
// visitor's own default factory.
pub fn new_format_span_visitor<'a, 't>(
    w: &'a mut FormatSpanWorker<'t>,
) -> FormatSpanVisitor<'a, 't> {
    let hooks: NodeVisitorHooks<'a, &'a mut FormatSpanWorker<'t>> = NodeVisitorHooks {
        visit_nodes: Some(Rc::new(
            |nodes: NodeList, v: &mut FormatSpanVisitor<'a, 't>| -> NodeList {
                if nodes.is_nil() {
                    return nodes;
                }
                let w = &mut *v.ctx;
                let visiting_node = w.visiting_node;
                let visiting_indenter = w.visiting_indenter.clone();
                let visiting_node_start_line = w.visiting_node_start_line;
                let visiting_undecorated_node_start_line = w.visiting_undecorated_node_start_line;
                w.process_child_nodes(
                    visiting_node,
                    visiting_indenter.as_ref(),
                    visiting_node_start_line,
                    visiting_undecorated_node_start_line,
                    nodes,
                    visiting_node,
                    visiting_node_start_line,
                    visiting_indenter.as_ref(),
                );
                nodes
            },
        )),
        ..NodeVisitorHooks::default()
    };
    new_node_visitor(
        |child: Node, v: &mut FormatSpanVisitor<'a, 't>| -> Node {
            if child.is_nil() {
                return child;
            }
            let w = &mut *v.ctx;
            let visiting_node = w.visiting_node;
            let visiting_indenter = w.visiting_indenter.clone();
            let visiting_node_start_line = w.visiting_node_start_line;
            let visiting_undecorated_node_start_line = w.visiting_undecorated_node_start_line;
            w.process_child_node(
                visiting_node,
                visiting_indenter.as_ref(),
                visiting_node_start_line,
                visiting_undecorated_node_start_line,
                child,
                -1,
                visiting_node,
                visiting_indenter.as_ref(),
                visiting_node_start_line,
                visiting_undecorated_node_start_line,
                false,
                false,
            );
            child
        },
        None,
        hooks,
        w,
    )
}

impl<'t> FormatSpanWorker<'t> {
    /// Go `w.formattingScanner` (set in `execute`).
    pub fn formatting_scanner(&mut self) -> &mut FormattingScanner<'t> {
        self.formatting_scanner
            .as_mut()
            .expect("nil formattingScanner")
    }

    /// Go `w.formattingContext.Options`.
    pub fn options(&self) -> &lsutil::FormatCodeSettings {
        &self
            .formatting_context
            .as_ref()
            .expect("nil formattingContext")
            .options
    }

    // Go: format/span.go:225 execute
    pub fn execute(&mut self, s: FormattingScanner<'t>) -> Vec<TextChange> {
        self.formatting_scanner = Some(s);
        self.indentation_on_last_indented_line = -1;
        self.last_indented_line = -1;
        let opt = get_format_code_settings_from_context(&self.ctx);
        self.formatting_context = Some(new_formatting_context(
            self.source_file,
            self.request_kind,
            (*opt).clone(),
        ));
        // formatting context is used by rules provider
        // PORT: the Go visitor is built here; see `new_format_span_visitor`.

        self.formatting_scanner().advance();

        if self.formatting_scanner().is_on_token() {
            let start_line = get_ecma_line_of_position(
                self.source_file,
                with_token_start(self.enclosing_node, self.source_file).pos(),
            );
            let mut undecorated_start_line = start_line;
            if has_decorators(self.enclosing_node) {
                undecorated_start_line = get_ecma_line_of_position(
                    self.source_file,
                    get_non_decorator_token_pos_of_node(self.enclosing_node, self.source_file),
                );
            }

            self.process_node(
                self.enclosing_node,
                self.enclosing_node,
                start_line,
                undecorated_start_line,
                self.initial_indentation,
                self.delta,
            );
        }

        // Leading trivia items get attached to and processed with the token that proceeds them. If the
        // range ends in the middle of some leading trivia, the token that proceeds them won't be in the
        // range and thus won't get processed. So we process those remaining trivia items here.
        let remaining_trivia = self
            .formatting_scanner()
            .get_current_leading_trivia()
            .to_vec();
        if !remaining_trivia.is_empty() {
            let mut indentation = self.initial_indentation;
            if node_will_indent_child(
                self.options(),
                self.enclosing_node,
                Node::NIL,
                self.source_file,
                false,
            ) {
                indentation += opt.editor_settings.indent_size; // !!! TODO: nil check???
            }

            self.indent_trivia_items(
                &remaining_trivia,
                indentation,
                true,
                &mut |w: &mut FormatSpanWorker<'t>, item: TextRangeWithKind| {
                    let (start_line, start_char) =
                        get_ecma_line_and_byte_offset_of_position(w.source_file, item.loc.pos());
                    w.process_range(
                        item,
                        start_line,
                        start_char,
                        w.enclosing_node,
                        w.enclosing_node,
                        None,
                    );
                    w.insert_indentation(item.loc.pos(), indentation, false);
                },
            );

            if opt.editor_settings.trim_trailing_whitespace.is_true() {
                self.trim_trailing_whitespaces_for_remaining_range(&remaining_trivia);
            }
        }

        if self.previous_range != new_text_range_with_kind(0, 0, SyntaxKind::Unknown)
            && self.formatting_scanner().get_token_full_start() >= self.original_range.end()
        {
            // Formatting edits happen by looking at pairs of contiguous tokens (see `processPair`),
            // typically inserting or deleting whitespace between them. The recursive `processNode`
            // logic above bails out as soon as it encounters a token that is beyond the end of the
            // range we're supposed to format (or if we reach the end of the file). But this potentially
            // leaves out an edit that would occur *inside* the requested range but cannot be discovered
            // without looking at one token *beyond* the end of the range: consider the line `x = { }`
            // with a selection from the beginning of the line to the space inside the curly braces,
            // inclusive. We would expect a format-selection would delete the space (if rules apply),
            // but in order to do that, we need to process the pair ["{", "}"], but we stopped processing
            // just before getting there. This block handles this trailing edit.
            let mut token_info = TextRangeWithKind::default();
            if self.formatting_scanner().is_on_eof() {
                token_info = self.formatting_scanner().read_eof_token_range();
            } else if self.formatting_scanner().is_on_token() {
                let enclosing_node = self.enclosing_node;
                token_info = self
                    .formatting_scanner()
                    .read_token_info(enclosing_node)
                    .token;
            }

            if token_info.loc.pos() == self.previous_range_trivia_end {
                // We need to check that tokenInfo and previousRange are contiguous: the `originalRange`
                // may have ended in the middle of a token, which means we will have stopped formatting
                // on that token, leaving `previousRange` pointing to the token before it, but already
                // having moved the formatting scanner (where we just got `tokenInfo`) to the next token.
                // If this happens, our supposed pair [previousRange, tokenInfo] actually straddles the
                // token that intersects the end of the range we're supposed to format, so the pair will
                // produce bogus edits if we try to `processPair`. Recall that the point of this logic is
                // to perform a trailing edit at the end of the selection range: but there can be no valid
                // edit in the middle of a token where the range ended, so if we have a non-contiguous
                // pair here, we're already done and we can ignore it.
                let mut parent =
                    astnav::find_preceding_token(self.source_file, token_info.loc.end());
                if parent.is_some() {
                    parent = parent.parent();
                }
                if parent.is_nil() {
                    parent = self.previous_parent;
                }
                let line = get_ecma_line_of_position(self.source_file, token_info.loc.pos());
                self.process_pair(
                    token_info,
                    line,
                    parent,
                    self.previous_range,
                    self.previous_range_start_line,
                    self.previous_parent,
                    parent,
                    None,
                );
            }
        }

        // PORT: Go returns the edits slice; the worker is dropped after this.
        std::mem::take(&mut self.edits)
    }

    // Go: format/span.go:334 processChildNode
    pub fn process_child_node(
        &mut self,
        node: Node,
        indenter: Option<&Rc<DynamicIndenter>>,
        node_start_line: i32,
        undecorated_node_start_line: i32,
        child: Node,
        mut inherited_indentation: i32,
        parent: Node,
        parent_dynamic_indentation: Option<&Rc<DynamicIndenter>>,
        parent_start_line: i32,
        undecorated_parent_start_line: i32,
        is_list_item: bool,
        is_first_list_item: bool,
    ) -> i32 {
        debug_assert!(!node_is_synthesized(child));

        if node_is_missing(child) || child.flags().intersects(NodeFlags::REPARSED) {
            return inherited_indentation;
        }
        let child_start_pos = get_token_pos_of_node(child, self.source_file, false);
        let child_start_line = get_ecma_line_of_position(self.source_file, child_start_pos);

        let mut undecorated_child_start_line = child_start_line;
        if has_decorators(child) {
            undecorated_child_start_line = get_ecma_line_of_position(
                self.source_file,
                get_non_decorator_token_pos_of_node(child, self.source_file),
            );
        }

        let is_error_member_list_element = child.flags().intersects(NodeFlags::THIS_NODE_HAS_ERROR)
            && is_member_list_element(parent, child);
        // if child is a list item - try to get its indentation, only if parent is within the original range.
        let mut child_indentation_amount = -1;

        if !is_error_member_list_element
            && is_list_item
            && parent.loc().contained_by(self.original_range)
        {
            child_indentation_amount = self.try_compute_indentation_for_list_item(
                child_start_pos,
                child.end(),
                parent_start_line,
                self.original_range,
                inherited_indentation,
            );
            if child_indentation_amount != -1 {
                inherited_indentation = child_indentation_amount;
            }
        }

        // child node is outside the target range - do not dive inside
        if !self.original_range.overlaps(child.loc()) {
            if child.end() < self.original_range.pos() {
                let child_loc = child.loc();
                self.formatting_scanner().skip_to_end_of(&child_loc);
            }
            return inherited_indentation;
        }

        if child.loc().len() == 0 {
            return inherited_indentation;
        }

        while self.formatting_scanner().is_on_token()
            && self.formatting_scanner().get_token_full_start() < self.original_range.end()
        {
            // proceed any parent tokens that are located prior to child.getStart()
            let token_info = self.formatting_scanner().read_token_info(node);
            if token_info.token.loc.end() > self.original_range.end() {
                return inherited_indentation;
            }
            if token_info.token.loc.end() > child_start_pos {
                if token_info.token.loc.pos() > child_start_pos {
                    let child_loc = child.loc();
                    self.formatting_scanner().skip_to_start_of(&child_loc);
                }
                // stop when formatting scanner advances past the beginning of the child
                break;
            }

            self.consume_token_and_advance_scanner(
                token_info,
                node,
                parent_dynamic_indentation,
                node,
                false,
            );
        }

        if !self.formatting_scanner().is_on_token()
            || self.formatting_scanner().get_token_full_start() >= self.original_range.end()
        {
            return inherited_indentation;
        }

        if is_token_kind(child.kind()) {
            // if child node is a token, it does not impact indentation, proceed it using parent indentation scope rules
            let token_info = self.formatting_scanner().read_token_info(child);
            // JSX text shouldn't affect indenting
            if child.kind() != SyntaxKind::JsxText {
                debug_assert!(
                    token_info.token.loc.end() == child.loc().end(),
                    "Token end is child end"
                );
                self.consume_token_and_advance_scanner(
                    token_info,
                    node,
                    parent_dynamic_indentation,
                    child,
                    false,
                );
                return inherited_indentation;
            }
        }

        let mut effective_parent_start_line = undecorated_parent_start_line;
        if child.kind() == SyntaxKind::Decorator {
            effective_parent_start_line = child_start_line;
        }
        let child_indentation;
        let mut delta = 0;
        if is_error_member_list_element {
            child_indentation = self.get_current_indentation_at_position(child_start_pos);
        } else {
            (child_indentation, delta) = self.compute_indentation(
                child,
                child_start_line,
                child_indentation_amount,
                node,
                parent_dynamic_indentation,
                effective_parent_start_line,
            );
        }

        let child_context_node = self.child_context_node;
        self.process_node(
            child,
            child_context_node,
            child_start_line,
            undecorated_child_start_line,
            child_indentation,
            delta,
        );

        self.child_context_node = node;

        if is_first_list_item
            && parent.kind() == SyntaxKind::ArrayLiteralExpression
            && inherited_indentation == -1
        {
            inherited_indentation = child_indentation;
        }

        inherited_indentation
    }

    // Go: format/span.go:439 processChildNodes
    pub fn process_child_nodes(
        &mut self,
        node: Node,
        indenter: Option<&Rc<DynamicIndenter>>,
        node_start_line: i32,
        undecorated_node_start_line: i32,
        nodes: NodeList,
        parent: Node,
        parent_start_line: i32,
        parent_dynamic_indentation: Option<&Rc<DynamicIndenter>>,
    ) {
        debug_assert!(nodes.is_some());
        debug_assert!(!position_is_synthesized(nodes.pos()));
        debug_assert!(!position_is_synthesized(nodes.end()));

        let list_start_token = get_open_token_for_list(parent, nodes);

        let mut list_dynamic_indentation: Option<Rc<DynamicIndenter>> =
            parent_dynamic_indentation.cloned();
        let mut start_line = parent_start_line;

        // node range is outside the target range - do not dive inside
        if !self.original_range.overlaps(nodes.loc()) {
            if nodes.end() < self.original_range.pos()
                && (nodes.nodes().is_empty()
                    || !nodes.nodes().get(0).flags().intersects(NodeFlags::REPARSED))
            {
                let nodes_loc = nodes.loc();
                self.formatting_scanner().skip_to_end_of(&nodes_loc);
            }
            return;
        }

        if list_start_token != SyntaxKind::Unknown {
            // introduce a new indentation scope for lists (including list start and end tokens)
            while self.formatting_scanner().is_on_token()
                && self.formatting_scanner().get_token_full_start() < self.original_range.end()
            {
                let token_info = self.formatting_scanner().read_token_info(parent);
                if token_info.token.loc.end() > nodes.pos() {
                    // stop when formatting scanner moves past the beginning of node list
                    break;
                } else if token_info.token.kind == list_start_token {
                    // consume list start token
                    start_line =
                        get_ecma_line_of_position(self.source_file, token_info.token.loc.pos());

                    let token_pos = token_info.token.loc.pos();
                    self.consume_token_and_advance_scanner(
                        token_info,
                        parent,
                        parent_dynamic_indentation,
                        parent,
                        false,
                    );

                    let indentation_on_list_start_token;
                    if self.indentation_on_last_indented_line != -1 {
                        // scanner just processed list start token so consider last indentation as list indentation
                        // function foo(): { // last indentation was 0, list item will be indented based on this value
                        //   foo: number;
                        // }: {};
                        indentation_on_list_start_token = self.indentation_on_last_indented_line;
                    } else {
                        indentation_on_list_start_token =
                            self.get_current_indentation_at_position(token_pos);
                    }

                    let indent_size = self.options().editor_settings.indent_size;
                    list_dynamic_indentation = Some(self.get_dynamic_indentation(
                        parent,
                        parent_start_line,
                        indentation_on_list_start_token,
                        indent_size,
                    ));
                } else {
                    // consume any tokens that precede the list as child elements of 'node' using its indentation scope
                    self.consume_token_and_advance_scanner(
                        token_info,
                        parent,
                        parent_dynamic_indentation,
                        parent,
                        false,
                    );
                }
            }
        }

        let mut inherited_indentation = -1;
        let children = nodes.nodes();
        for i in 0..children.len() {
            let child = children.get(i);
            inherited_indentation = self.process_child_node(
                node,
                indenter,
                node_start_line,
                undecorated_node_start_line,
                child,
                inherited_indentation,
                node,
                list_dynamic_indentation.as_ref(),
                start_line,
                start_line,
                true,
                i == 0,
            );
        }

        let list_end_token = get_close_token_for_open_token(list_start_token);
        if list_end_token != SyntaxKind::Unknown
            && self.formatting_scanner().is_on_token()
            && self.formatting_scanner().get_token_full_start() < self.original_range.end()
        {
            let mut token_info = self.formatting_scanner().read_token_info(parent);
            if token_info.token.kind == SyntaxKind::CommaToken {
                // consume the comma
                self.consume_token_and_advance_scanner(
                    token_info,
                    parent,
                    list_dynamic_indentation.as_ref(),
                    parent,
                    false,
                );
                if self.formatting_scanner().is_on_token() {
                    token_info = self.formatting_scanner().read_token_info(parent);
                } else {
                    return;
                }
            }

            // consume the list end token only if it is still belong to the parent
            // there might be the case when current token matches end token but does not considered as one
            // function (x: function) <--
            // without this check close paren will be interpreted as list end token for function expression which is wrong
            if token_info.token.kind == list_end_token
                && token_info.token.loc.contained_by(parent.loc())
            {
                // consume list end token
                self.consume_token_and_advance_scanner(
                    token_info,
                    parent,
                    list_dynamic_indentation.as_ref(),
                    parent,
                    true, /*isListEndToken*/
                );
            }
        }
    }

    // Go: format/span.go:528 executeProcessNodeVisitor
    pub fn execute_process_node_visitor(
        &mut self,
        node: Node,
        indenter: Option<&Rc<DynamicIndenter>>,
        node_start_line: i32,
        undecorated_node_start_line: i32,
    ) {
        let old_node = self.visiting_node;
        let old_indenter = self.visiting_indenter.take();
        let old_start = self.visiting_node_start_line;
        let old_undecorated_start = self.visiting_undecorated_node_start_line;
        self.visiting_node = node;
        self.visiting_indenter = indenter.cloned();
        self.visiting_node_start_line = node_start_line;
        self.visiting_undecorated_node_start_line = undecorated_node_start_line;
        {
            let mut visitor = new_format_span_visitor(self);
            node.visit_each_child(&mut visitor);
        }
        self.visiting_node = old_node;
        self.visiting_indenter = old_indenter;
        self.visiting_node_start_line = old_start;
        self.visiting_undecorated_node_start_line = old_undecorated_start;
    }

    // Go: format/span.go:544 getCurrentIndentationAtPosition
    pub fn get_current_indentation_at_position(&self, pos: i32) -> i32 {
        let start_line_position = get_line_start_position_for_position(pos, self.source_file);
        find_first_non_whitespace_column(start_line_position, pos, self.source_file, self.options())
    }

    // Go: format/span.go:549 computeIndentation
    pub fn compute_indentation(
        &mut self,
        node: Node,
        start_line: i32,
        inherited_indentation: i32,
        parent: Node,
        parent_dynamic_indentation: Option<&Rc<DynamicIndenter>>,
        effective_parent_start_line: i32,
    ) -> (i32, i32) {
        // PORT: Go dereferences a nil `parentDynamicIndentation` only where
        // it is used; so does this.
        let pdi = || parent_dynamic_indentation.expect("nil dynamicIndenter");
        let mut delta = 0;
        if should_indent_child_node(self.options(), node, Node::NIL, Node::NIL, &[]) {
            delta = self.options().editor_settings.indent_size;
        }

        if effective_parent_start_line == start_line {
            // if node is located on the same line with the parent
            // - inherit indentation from the parent
            // - push children if either parent of node itself has non-zero delta
            let mut indentation = self.indentation_on_last_indented_line;
            if start_line != self.last_indented_line {
                indentation = pdi().get_indentation();
            }
            delta = self
                .options()
                .editor_settings
                .indent_size
                .min(pdi().get_delta(node) + delta);
            return (indentation, delta);
        } else if inherited_indentation == -1 {
            if node.kind() == SyntaxKind::OpenParenToken && start_line == self.last_indented_line {
                // the is used for chaining methods formatting
                // - we need to get the indentation on last line and the delta of parent
                return (
                    self.indentation_on_last_indented_line,
                    pdi().get_delta(node),
                );
            } else if child_starts_on_the_same_line_with_else_in_if_statement(
                parent,
                node,
                start_line,
                self.source_file,
            ) || child_is_unindented_branch_of_conditional_expression(
                parent,
                node,
                start_line,
                self.source_file,
            ) || argument_starts_on_same_line_as_previous_argument(
                parent,
                node,
                start_line,
                self.source_file,
            ) {
                return (pdi().get_indentation(), delta);
            } else {
                let i = pdi().get_indentation();
                if i == -1 {
                    return (pdi().get_indentation(), delta);
                }
                return (i + pdi().get_delta(node), delta);
            }
        }

        (inherited_indentation, delta)
    }

    // Go: format/span.go:593 tryComputeIndentationForListItem
    /** Tries to compute the indentation for a list element.
     * If list element is not in range then
     * function will pick its actual indentation
     * so it can be pushed downstream as inherited indentation.
     * If list element is in the range - its indentation will be equal
     * to inherited indentation from its predecessors.
     */
    pub fn try_compute_indentation_for_list_item(
        &mut self,
        start_pos: i32,
        end_pos: i32,
        parent_start_line: i32,
        r: TextRange,
        inherited_indentation: i32,
    ) -> i32 {
        let r2 = TextRange::new(start_pos, end_pos);
        if r.overlaps(r2) || r2.contained_by(r) {
            /* Not to miss zero-range nodes e.g. JsxText */
            if inherited_indentation != -1 {
                return inherited_indentation;
            }
        } else {
            let start_line = get_ecma_line_of_position(self.source_file, start_pos);
            let column = self.get_current_indentation_at_position(start_pos);
            if start_line != parent_start_line || start_pos == column {
                // Use the base indent size if it is greater than
                // the indentation of the inherited predecessor.
                let base_indent_size = self.options().editor_settings.base_indent_size;
                if base_indent_size > column {
                    return base_indent_size;
                }
                return column;
            }
        }
        -1
    }

    // Go: format/span.go:615 processNode
    pub fn process_node(
        &mut self,
        node: Node,
        context_node: Node,
        node_start_line: i32,
        undecorated_node_start_line: i32,
        indentation: i32,
        delta: i32,
    ) {
        if !self
            .original_range
            .overlaps(with_token_start(node, self.source_file))
        {
            return;
        }

        let node_dynamic_indentation =
            self.get_dynamic_indentation(node, node_start_line, indentation, delta);

        // a useful observations when tracking context node
        //        /
        //      [a]
        //   /   |   \
        //  [b] [c] [d]
        // node 'a' is a context node for nodes 'b', 'c', 'd'
        // except for the leftmost leaf token in [b] - in this case context node ('e') is located somewhere above 'a'
        // this rule can be applied recursively to child nodes of 'a'.
        //
        // context node is set to parent node value after processing every child node
        // context node is set to parent of the token after processing every token

        self.child_context_node = context_node;

        // if there are any tokens that logically belong to node and interleave child nodes
        // such tokens will be consumed in processChildNode for the child that follows them
        self.execute_process_node_visitor(
            node,
            Some(&node_dynamic_indentation),
            node_start_line,
            undecorated_node_start_line,
        );

        // proceed any tokens in the node that are located after child nodes
        while self.formatting_scanner().is_on_token()
            && self.formatting_scanner().get_token_full_start() < self.original_range.end()
        {
            let token_info = self.formatting_scanner().read_token_info(node);
            if token_info.token.loc.end() > node.end().min(self.original_range.end()) {
                break;
            }
            self.consume_token_and_advance_scanner(
                token_info,
                node,
                Some(&node_dynamic_indentation),
                node,
                false,
            );
        }
    }

    // Go: format/span.go:650 processPair
    pub fn process_pair(
        &mut self,
        current_item: TextRangeWithKind,
        current_start_line: i32,
        current_parent: Node,
        previous_item: TextRangeWithKind,
        previous_start_line: i32,
        previous_parent: Node,
        context_node: Node,
        dynamic_indentation: Option<&Rc<DynamicIndenter>>,
    ) -> LineAction {
        self.formatting_context
            .as_mut()
            .expect("nil formattingContext")
            .update_context(
                previous_item,
                previous_parent,
                current_item,
                current_parent,
                context_node,
            );

        self.current_rules.clear();
        get_rules(
            self.formatting_context
                .as_mut()
                .expect("nil formattingContext"),
            &mut self.current_rules,
        );

        let mut trim_trailing_whitespaces = !self
            .options()
            .editor_settings
            .trim_trailing_whitespace
            .is_false();
        let mut line_action = LineAction::NONE;

        if !self.current_rules.is_empty() {
            // Apply rules in reverse order so that higher priority rules (which are first in the array)
            // win in a conflict with lower priority rules.
            for i in (0..self.current_rules.len()).rev() {
                let rule = self.current_rules[i];
                line_action = self.apply_rule_edits(
                    rule,
                    previous_item,
                    previous_start_line,
                    current_item,
                    current_start_line,
                );
                if let Some(dynamic_indentation) = dynamic_indentation {
                    match line_action {
                        LineAction::LINE_REMOVED => {
                            // Handle the case where the next line is moved to be the end of this line.
                            // In this case we don't indent the next line in the next pass.
                            if get_token_pos_of_node(current_parent, self.source_file, false)
                                == current_item.loc.pos()
                            {
                                dynamic_indentation.recompute_indentation(
                                    false, /*lineAddedByFormatting*/
                                    context_node,
                                );
                            }
                        }
                        LineAction::LINE_ADDED => {
                            // Handle the case where token2 is moved to the new line.
                            // In this case we indent token2 in the next pass but we set
                            // sameLineIndent flag to notify the indenter that the indentation is within the line.
                            if get_token_pos_of_node(current_parent, self.source_file, false)
                                == current_item.loc.pos()
                            {
                                dynamic_indentation.recompute_indentation(
                                    true, /*lineAddedByFormatting*/
                                    context_node,
                                );
                            }
                        }
                        _ => {
                            debug_assert!(line_action == LineAction::NONE);
                        }
                    }
                }

                // We need to trim trailing whitespace between the tokens if they were on different lines, and no rule was applied to put them on the same line
                trim_trailing_whitespaces = trim_trailing_whitespaces
                    && (rule.action().0 & RuleAction::DELETE_SPACE.0 == 0)
                    && rule.flags() != RuleFlags::CAN_DELETE_NEW_LINES;
            }
        } else {
            trim_trailing_whitespaces =
                trim_trailing_whitespaces && current_item.kind != SyntaxKind::EndOfFile;
        }

        if current_start_line != previous_start_line && trim_trailing_whitespaces {
            // We need to trim trailing whitespace between the tokens if they were on different lines, and no rule was applied to put them on the same line
            self.trim_trailing_whitespaces_for_lines(
                previous_start_line,
                current_start_line,
                previous_item,
            );
        }

        line_action
    }

    // Go: format/span.go:700 applyRuleEdits
    pub fn apply_rule_edits(
        &mut self,
        rule: &RuleImpl,
        previous_range: TextRangeWithKind,
        previous_start_line: i32,
        current_range: TextRangeWithKind,
        current_start_line: i32,
    ) -> LineAction {
        let on_later_line = current_start_line != previous_start_line;
        match rule.action() {
            RuleAction::STOP_PROCESSING_SPACE_ACTIONS => {
                // no action required
                return LineAction::NONE;
            }
            RuleAction::DELETE_SPACE => {
                if previous_range.loc.end() != current_range.loc.pos() {
                    // delete characters starting from t1.end up to t2.pos exclusive
                    self.record_delete(
                        previous_range.loc.end(),
                        current_range.loc.pos() - previous_range.loc.end(),
                    );
                    if on_later_line {
                        return LineAction::LINE_REMOVED;
                    }
                    return LineAction::NONE;
                }
            }
            RuleAction::DELETE_TOKEN => {
                self.record_delete(previous_range.loc.pos(), previous_range.loc.len());
            }
            RuleAction::INSERT_NEW_LINE => {
                // exit early if we on different lines and rule cannot change number of newlines
                // if line1 and line2 are on subsequent lines then no edits are required - ok to exit
                // if line1 and line2 are separated with more than one newline - ok to exit since we cannot delete extra new lines
                if rule.flags() != RuleFlags::CAN_DELETE_NEW_LINES
                    && previous_start_line != current_start_line
                {
                    return LineAction::NONE;
                }

                // edit should not be applied if we have one line feed between elements
                let line_delta = current_start_line - previous_start_line;
                if line_delta != 1 {
                    let new_line = get_new_line_or_default_from_context(&self.ctx);
                    self.record_replace(
                        previous_range.loc.end(),
                        current_range.loc.pos() - previous_range.loc.end(),
                        &new_line,
                    );
                    if on_later_line {
                        return LineAction::NONE;
                    }
                    return LineAction::LINE_ADDED;
                }
            }
            RuleAction::INSERT_SPACE => {
                // exit early if we on different lines and rule cannot change number of newlines
                if rule.flags() != RuleFlags::CAN_DELETE_NEW_LINES
                    && previous_start_line != current_start_line
                {
                    return LineAction::NONE;
                }

                let pos_delta = current_range.loc.pos() - previous_range.loc.end();
                if pos_delta != 1
                    || !source_file_text(self.source_file).as_bytes()
                        [previous_range.loc.end() as usize..]
                        .starts_with(b" ")
                {
                    self.record_replace(previous_range.loc.end(), pos_delta, " ");
                    if on_later_line {
                        return LineAction::LINE_REMOVED;
                    }
                    return LineAction::NONE;
                }
            }
            RuleAction::INSERT_TRAILING_SEMICOLON => {
                self.record_insert(previous_range.loc.end(), ";");
            }
            _ => {}
        }
        LineAction::NONE
    }
}

// Go: format/span.go:754 LineAction
go_enum!(LineAction, i32 {
    NONE = 0; // LineActionNone
    LINE_ADDED = 1; // LineActionLineAdded
    LINE_REMOVED = 2; // LineActionLineRemoved
});

impl<'t> FormatSpanWorker<'t> {
    // Go: format/span.go:762 processRange
    pub fn process_range(
        &mut self,
        r: TextRangeWithKind,
        range_start_line: i32,
        range_start_character: i32,
        parent: Node,
        context_node: Node,
        dynamic_indentation: Option<&Rc<DynamicIndenter>>,
    ) -> LineAction {
        let range_has_error = (self.range_contains_error)(r.loc);
        let mut line_action = LineAction::NONE;
        if !range_has_error {
            if self.previous_range == new_text_range_with_kind(0, 0, SyntaxKind::Unknown) {
                // trim whitespaces starting from the beginning of the span up to the current line
                let original_start_line =
                    get_ecma_line_of_position(self.source_file, self.original_range.pos());
                self.trim_trailing_whitespaces_for_lines(
                    original_start_line,
                    range_start_line,
                    new_text_range_with_kind(0, 0, SyntaxKind::Unknown),
                );
            } else {
                line_action = self.process_pair(
                    r,
                    range_start_line,
                    parent,
                    self.previous_range,
                    self.previous_range_start_line,
                    self.previous_parent,
                    context_node,
                    dynamic_indentation,
                );
            }
        }

        self.previous_range = r;
        self.previous_range_trivia_end = r.loc.end();
        self.previous_parent = parent;
        self.previous_range_start_line = range_start_line;

        line_action
    }

    // Go: format/span.go:783 processTrivia
    pub fn process_trivia(
        &mut self,
        trivia: &[TextRangeWithKind],
        parent: Node,
        context_node: Node,
        dynamic_indentation: Option<&Rc<DynamicIndenter>>,
    ) {
        for &trivia_item in trivia {
            if is_comment(trivia_item.kind) && trivia_item.loc.contained_by(self.original_range) {
                let (trivia_item_start_line, trivia_item_start_character) =
                    get_ecma_line_and_byte_offset_of_position(
                        self.source_file,
                        trivia_item.loc.pos(),
                    );
                self.process_range(
                    trivia_item,
                    trivia_item_start_line,
                    trivia_item_start_character,
                    parent,
                    context_node,
                    dynamic_indentation,
                );
            }
        }
    }

    // Go: format/span.go:796 trimTrailingWhitespacesForRemainingRange
    /**
     * Trimming will be done for lines after the previous range.
     * Exclude comments as they had been previously processed.
     */
    pub fn trim_trailing_whitespaces_for_remaining_range(&mut self, trivias: &[TextRangeWithKind]) {
        let mut start_pos = self.original_range.pos();
        if self.previous_range != new_text_range_with_kind(0, 0, SyntaxKind::Unknown) {
            start_pos = self.previous_range.loc.end();
        }

        for &trivia in trivias {
            if is_comment(trivia.kind) {
                if start_pos < trivia.loc.pos() {
                    let previous_range = self.previous_range;
                    self.trim_trailing_witespaces_for_positions(
                        start_pos,
                        trivia.loc.pos() - 1,
                        previous_range,
                    );
                }

                start_pos = trivia.loc.end() + 1;
            }
        }

        if start_pos < self.original_range.end() {
            let previous_range = self.previous_range;
            self.trim_trailing_witespaces_for_positions(
                start_pos,
                self.original_range.end(),
                previous_range,
            );
        }
    }

    // Go: format/span.go:817 trimTrailingWitespacesForPositions
    pub fn trim_trailing_witespaces_for_positions(
        &mut self,
        start_pos: i32,
        end_pos: i32,
        previous_range: TextRangeWithKind,
    ) {
        let start_line = get_ecma_line_of_position(self.source_file, start_pos);
        let end_line = get_ecma_line_of_position(self.source_file, end_pos);

        self.trim_trailing_whitespaces_for_lines(start_line, end_line + 1, previous_range);
    }

    // Go: format/span.go:824 trimTrailingWhitespacesForLines
    pub fn trim_trailing_whitespaces_for_lines(
        &mut self,
        line1: i32,
        line2: i32,
        r: TextRangeWithKind,
    ) {
        let line_starts = &*get_ecma_line_starts(self.source_file);
        for line in line1..line2 {
            let line_start_position = line_starts[line as usize];
            let line_end_position = get_ecma_end_line_position(self.source_file, line);

            // do not trim whitespaces in comments or template expression
            if r != new_text_range_with_kind(0, 0, SyntaxKind::Unknown)
                && (is_comment(r.kind)
                    || is_string_or_regular_expression_or_template_literal(r.kind))
                && r.loc.pos() <= line_end_position
                && r.loc.end() > line_end_position
            {
                continue;
            }

            let whitespace_start =
                self.get_trailing_whitespace_start_position(line_start_position, line_end_position);
            if whitespace_start != -1 {
                if whitespace_start != line_start_position {
                    let (r, _) = utf8_decode_rune_in_string(
                        &source_file_text(self.source_file),
                        (whitespace_start - 1) as usize,
                    );
                    debug_assert!(!is_white_space_single_line(rune_to_char(r)));
                }
                self.record_delete(whitespace_start, line_end_position + 1 - whitespace_start);
            }
        }
    }

    // Go: format/span.go:850 getTrailingWhitespaceStartPosition
    /**
     * @param start The position of the first character in range
     * @param end The position of the last character in range
     */
    pub fn get_trailing_whitespace_start_position(&self, start: i32, end: i32) -> i32 {
        let mut pos = end;
        let text = source_file_text(self.source_file);
        while pos >= start {
            let (ch, size) = utf8_decode_rune_in_string(&text, pos as usize);
            if size == 0 {
                pos -= 1; // multibyte character, rewind more
                continue;
            }
            if !is_white_space_single_line(rune_to_char(ch)) {
                break;
            }
            pos -= 1;
        }
        if pos != end {
            return pos + 1;
        }
        -1
    }
}

// Go: format/span.go:870 isStringOrRegularExpressionOrTemplateLiteral
pub fn is_string_or_regular_expression_or_template_literal(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::StringLiteral
        || kind == SyntaxKind::RegularExpressionLiteral
        || is_template_literal_kind(kind)
}

// Go: format/span.go:874 isComment
pub fn is_comment(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::SingleLineCommentTrivia || kind == SyntaxKind::MultiLineCommentTrivia
}

impl<'t> FormatSpanWorker<'t> {
    // Go: format/span.go:878 insertIndentation
    pub fn insert_indentation(&mut self, pos: i32, indentation: i32, line_added: bool) {
        let indentation_string = get_indentation_string(indentation, self.options());
        if line_added {
            // new line is added before the token by the formatting rules
            // insert indentation string at the very beginning of the token
            self.record_replace(pos, 0, &indentation_string);
        } else {
            let (token_start_line, token_start_character) =
                get_ecma_line_and_byte_offset_of_position(self.source_file, pos);
            let start_line_position =
                get_ecma_line_starts(self.source_file)[token_start_line as usize];
            if indentation != self.character_to_column(start_line_position, token_start_character)
                || self.indentation_is_different(&indentation_string, start_line_position)
            {
                self.record_replace(
                    start_line_position,
                    token_start_character,
                    &indentation_string,
                );
            }
        }
    }

    // Go: format/span.go:893 characterToColumn
    pub fn character_to_column(&self, start_line_position: i32, character_in_line: i32) -> i32 {
        let mut column = 0;
        let text_text = source_file_text(self.source_file);
        let text = text_text.as_bytes();
        for i in 0..character_in_line {
            if text[(start_line_position + i) as usize] == b'\t' {
                if self.options().editor_settings.tab_size > 0 {
                    column += self.options().editor_settings.tab_size
                        - (column % self.options().editor_settings.tab_size);
                }
            } else {
                column += 1;
            }
        }
        column
    }

    // Go: format/span.go:907 indentationIsDifferent
    pub fn indentation_is_different(
        &self,
        indentation_string: &str,
        start_line_position: i32,
    ) -> bool {
        let text_text = source_file_text(self.source_file);
        let text = text_text.as_bytes();
        let end = start_line_position as usize + indentation_string.len();
        if end > text.len() {
            return true;
        }
        indentation_string.as_bytes() != &text[start_line_position as usize..end]
    }

    // Go: format/span.go:916 indentTriviaItems
    // PORT: Go `indentSingleLine` closes over the worker. Here the callback
    // gets the worker as its first argument.
    pub fn indent_trivia_items(
        &mut self,
        trivia: &[TextRangeWithKind],
        comment_indentation: i32,
        mut indent_next_token_or_trivia: bool,
        indent_single_line: &mut dyn FnMut(&mut FormatSpanWorker<'t>, TextRangeWithKind),
    ) -> bool {
        for &trivia_item in trivia {
            let trivia_in_range = trivia_item.loc.contained_by(self.original_range);
            match trivia_item.kind {
                SyntaxKind::MultiLineCommentTrivia => {
                    if trivia_in_range {
                        self.indent_multiline_comment(
                            trivia_item.loc,
                            comment_indentation,
                            !indent_next_token_or_trivia,
                            true,
                        );
                    }
                    indent_next_token_or_trivia = false;
                }
                SyntaxKind::SingleLineCommentTrivia => {
                    if indent_next_token_or_trivia && trivia_in_range {
                        indent_single_line(self, trivia_item);
                    }
                    indent_next_token_or_trivia = false;
                }
                SyntaxKind::NewLineTrivia => {
                    indent_next_token_or_trivia = true;
                }
                _ => {}
            }
        }
        indent_next_token_or_trivia
    }

    // Go: format/span.go:937 indentMultilineComment
    pub fn indent_multiline_comment(
        &mut self,
        comment_range: TextRange,
        indentation: i32,
        first_line_is_indented: bool,
        indent_final_line: bool,
    ) {
        // split comment in lines
        let mut start_line = get_ecma_line_of_position(self.source_file, comment_range.pos());
        let end_line = get_ecma_line_of_position(self.source_file, comment_range.end());

        if start_line == end_line {
            if !first_line_is_indented {
                // treat as single line comment
                self.insert_indentation(comment_range.pos(), indentation, false);
            }
            return;
        }

        let text = source_file_text(self.source_file);
        let mut parts: Vec<TextRange> = Vec::with_capacity(
            text.as_bytes()[comment_range.pos() as usize..comment_range.end() as usize]
                .iter()
                .filter(|&&b| b == b'\n')
                .count(),
        );
        let mut start_pos = comment_range.pos();
        for line in start_line..end_line {
            let end_of_line = get_ecma_end_line_position(self.source_file, line);
            parts.push(TextRange::new(start_pos, end_of_line));
            start_pos = get_ecma_line_starts(self.source_file)[(line + 1) as usize];
        }

        if indent_final_line {
            parts.push(TextRange::new(start_pos, comment_range.end()));
        }

        if parts.is_empty() {
            return;
        }

        let start_line_pos = get_ecma_line_starts(self.source_file)[start_line as usize];

        let (non_whitespace_in_first_part_character, non_whitespace_in_first_part_column) =
            find_first_non_whitespace_character_and_column(
                start_line_pos,
                parts[0].pos(),
                self.source_file,
                self.options(),
            );

        let mut start_index = 0;

        if first_line_is_indented {
            start_index = 1;
            start_line += 1;
        }

        // shift all parts on the delta size
        let delta = indentation - non_whitespace_in_first_part_column;
        for i in start_index..parts.len() {
            let start_line_pos = get_ecma_line_starts(self.source_file)[start_line as usize];
            let mut non_whitespace_character = non_whitespace_in_first_part_character;
            let mut non_whitespace_column = non_whitespace_in_first_part_column;
            if i != 0 {
                (non_whitespace_character, non_whitespace_column) =
                    find_first_non_whitespace_character_and_column(
                        parts[i].pos(),
                        parts[i].end(),
                        self.source_file,
                        self.options(),
                    );
            }
            let new_indentation = non_whitespace_column + delta;
            if new_indentation > 0 {
                let indentation_string = get_indentation_string(new_indentation, self.options());
                self.record_replace(
                    start_line_pos,
                    non_whitespace_character,
                    &indentation_string,
                );
            } else {
                self.record_delete(start_line_pos, non_whitespace_character);
            }

            start_line += 1;
        }
    }
}

/// Go `strings.Repeat(s, count)`.
// PORT: Go panics on a negative count; `str::repeat` takes a usize.
fn strings_repeat(s: &str, count: i32) -> String {
    if count < 0 {
        panic!("strings: negative Repeat count");
    }
    s.repeat(count as usize)
}

// Go: format/span.go:998 getIndentationString
pub fn get_indentation_string(indentation: i32, options: &lsutil::FormatCodeSettings) -> String {
    // go's `strings.Repeat` already has static, global caching for repeated tabs and spaces, so there's no need to cache here like in strada
    if !options.editor_settings.convert_tabs_to_spaces.is_true() {
        if options.editor_settings.tab_size == 0 {
            return String::new();
        }
        let tabs =
            (f64::from(indentation) / f64::from(options.editor_settings.tab_size)).floor() as i32;
        let spaces = indentation - (tabs * options.editor_settings.tab_size);
        let mut res = strings_repeat("\t", tabs);
        if spaces > 0 {
            res = res + &strings_repeat(" ", spaces);
        }

        res
    } else {
        strings_repeat(" ", indentation)
    }
}

// Go: format/span.go:1017 createTextChangeFromStartLength
pub fn create_text_change_from_start_length(start: i32, length: i32, new_text: &str) -> TextChange {
    TextChange {
        new_text: new_text.to_string(),
        text_range: TextRange::new(start, start + length),
    }
}

impl<'t> FormatSpanWorker<'t> {
    // Go: format/span.go:1024 recordDelete
    pub fn record_delete(&mut self, start: i32, length: i32) {
        if length != 0 {
            self.edits
                .push(create_text_change_from_start_length(start, length, ""));
        }
    }

    // Go: format/span.go:1030 recordReplace
    pub fn record_replace(&mut self, start: i32, length: i32, new_text: &str) {
        if length != 0 || !new_text.is_empty() {
            self.edits.push(create_text_change_from_start_length(
                start, length, new_text,
            ));
        }
    }

    // Go: format/span.go:1036 recordInsert
    pub fn record_insert(&mut self, start: i32, text: &str) {
        if !text.is_empty() {
            self.edits
                .push(create_text_change_from_start_length(start, 0, text));
        }
    }

    // Go: format/span.go:1042 consumeTokenAndAdvanceScanner
    pub fn consume_token_and_advance_scanner(
        &mut self,
        current_token_info: TokenInfo,
        parent: Node,
        dynamic_indenation: Option<&Rc<DynamicIndenter>>,
        container: Node,
        is_list_end_token: bool,
    ) {
        // assert(currentTokenInfo.token.Loc.ContainedBy(parent.Loc)) // !!!
        let last_trivia_was_new_line = self
            .formatting_scanner()
            .last_trailing_trivia_was_new_line();
        let mut indent_token = false;

        if !current_token_info.leading_trivia.is_empty() {
            let child_context_node = self.child_context_node;
            self.process_trivia(
                &current_token_info.leading_trivia,
                parent,
                child_context_node,
                dynamic_indenation,
            );
        }

        let mut line_action = LineAction::NONE;
        let is_token_in_range = current_token_info
            .token
            .loc
            .contained_by(self.original_range);

        let (token_start_line, token_start_char) = get_ecma_line_and_byte_offset_of_position(
            self.source_file,
            current_token_info.token.loc.pos(),
        );

        if is_token_in_range {
            let range_has_error = (self.range_contains_error)(current_token_info.token.loc);
            // save previousRange since processRange will overwrite this value with current one
            let save_previous_range = self.previous_range;
            let child_context_node = self.child_context_node;
            line_action = self.process_range(
                current_token_info.token,
                token_start_line,
                token_start_char,
                parent,
                child_context_node,
                dynamic_indenation,
            );
            // do not indent comments\token if token range overlaps with some error
            if !range_has_error {
                if line_action == LineAction::NONE {
                    // indent token only if end line of previous range does not match start line of the token
                    if save_previous_range != new_text_range_with_kind(0, 0, SyntaxKind::Unknown) {
                        let prev_end_line = get_ecma_line_of_position(
                            self.source_file,
                            save_previous_range.loc.end(),
                        );
                        indent_token =
                            last_trivia_was_new_line && token_start_line != prev_end_line;
                    } else {
                        // When there's no previous range (first token), TS sets prevEndLine to undefined.
                        // tokenStart.line !== undefined is always true in JS, so indentToken = lastTriviaWasNewLine.
                        indent_token = last_trivia_was_new_line;
                    }
                } else {
                    indent_token = line_action == LineAction::LINE_ADDED;
                }
            }
        }

        if !current_token_info.trailing_trivia.is_empty() {
            self.previous_range_trivia_end = current_token_info
                .trailing_trivia
                .last()
                .expect("non-empty")
                .loc
                .end();
            // If any trailing comment trivia extends past the original range, it won't be
            // processed by processTrivia (which skips comments not contained by originalRange).
            // Cap previousRangeTriviaEnd before such comments so the trailing edit contiguity
            // check in execute() won't pair across unprocessed comment content.
            for trivia in &current_token_info.trailing_trivia {
                if is_comment(trivia.kind) && !trivia.loc.contained_by(self.original_range) {
                    self.previous_range_trivia_end = trivia.loc.pos();
                    break;
                }
            }
            let child_context_node = self.child_context_node;
            self.process_trivia(
                &current_token_info.trailing_trivia,
                parent,
                child_context_node,
                dynamic_indenation,
            );
        }

        if indent_token {
            // PORT: Go dereferences a nil `dynamicIndenation` here; so does this.
            let mut token_indentation = -1;
            if is_token_in_range && !(self.range_contains_error)(current_token_info.token.loc) {
                token_indentation = dynamic_indenation
                    .expect("nil dynamicIndenter")
                    .get_indentation_for_token(
                        token_start_line,
                        current_token_info.token.kind,
                        container,
                        is_list_end_token,
                    );
            }
            let mut indent_next_token_or_trivia = true;
            if !current_token_info.leading_trivia.is_empty() {
                let comment_indentation = dynamic_indenation
                    .expect("nil dynamicIndenter")
                    .get_indentation_for_comment(
                        current_token_info.token.kind,
                        token_indentation,
                        container,
                    );
                indent_next_token_or_trivia = self.indent_trivia_items(
                    &current_token_info.leading_trivia,
                    comment_indentation,
                    indent_next_token_or_trivia,
                    &mut |w: &mut FormatSpanWorker<'t>, item: TextRangeWithKind| {
                        w.insert_indentation(item.loc.pos(), comment_indentation, false);
                    },
                );
            }

            // indent token only if is it is in target range and does not overlap with any error ranges
            if token_indentation != -1 && indent_next_token_or_trivia {
                self.insert_indentation(
                    current_token_info.token.loc.pos(),
                    token_indentation,
                    line_action == LineAction::LINE_ADDED,
                );

                self.last_indented_line = token_start_line;
                self.indentation_on_last_indented_line = token_indentation;
            }
        }

        self.formatting_scanner().advance();

        self.child_context_node = parent;
    }
}

// Go: format/span.go:1121 dynamicIndenter
// PORT: Go shares `*dynamicIndenter` between the worker and its callers and
// mutates `indentation` and `delta` through it (recomputeIndentation). Here
// it is shared as `Rc<DynamicIndenter>` with those two fields in `Cell`s. It
// holds no reference to the worker, so there is no cycle.
pub struct DynamicIndenter {
    pub node: Node,
    pub node_start_line: i32,
    pub indentation: Cell<i32>,
    pub delta: Cell<i32>,

    pub options: lsutil::FormatCodeSettings,
    pub source_file: Node,
}

impl DynamicIndenter {
    // Go: format/span.go:1131 getIndentationForComment
    pub fn get_indentation_for_comment(
        &self,
        kind: SyntaxKind,
        token_indentation: i32,
        container: Node,
    ) -> i32 {
        match kind {
            // preceding comment to the token that closes the indentation scope inherits the indentation from the scope
            // ..  {
            //     // comment
            // }
            SyntaxKind::CloseBraceToken
            | SyntaxKind::CloseBracketToken
            | SyntaxKind::CloseParenToken => {
                return self.indentation.get() + self.get_delta(container);
            }
            _ => {}
        }
        if token_indentation != -1 {
            return token_indentation;
        }
        self.indentation.get()
    }

    // Go: format/span.go:1159 getIndentationForToken
    // if list end token is LessThanToken '>' then its delta should be explicitly suppressed
    // so that LessThanToken as a binary operator can still be indented.
    // foo.then
    //
    //	<
    //	    number,
    //	    string,
    //	>();
    //
    // vs
    // var a = xValue
    //
    //	> yValue;
    pub fn get_indentation_for_token(
        &self,
        line: i32,
        kind: SyntaxKind,
        container: Node,
        suppress_delta: bool,
    ) -> i32 {
        if !suppress_delta && self.should_add_delta(line, kind, container) {
            return self.indentation.get() + self.get_delta(container);
        }
        self.indentation.get()
    }

    // Go: format/span.go:1166 getIndentation
    pub fn get_indentation(&self) -> i32 {
        self.indentation.get()
    }

    // Go: format/span.go:1170 getDelta
    pub fn get_delta(&self, child: Node) -> i32 {
        // Delta value should be zero when the node explicitly prevents indentation of the child node
        if node_will_indent_child(&self.options, self.node, child, self.source_file, true) {
            return self.delta.get();
        }
        0
    }

    // Go: format/span.go:1178 recomputeIndentation
    pub fn recompute_indentation(&self, line_added: bool, parent: Node) {
        if should_indent_child_node(&self.options, parent, self.node, self.source_file, &[]) {
            if line_added {
                self.indentation
                    .set(self.indentation.get() + self.options.editor_settings.indent_size); // !!! no nil check???
            } else {
                self.indentation
                    .set(self.indentation.get() - self.options.editor_settings.indent_size); // !!! no nil check???
            }
            if should_indent_child_node(&self.options, self.node, Node::NIL, Node::NIL, &[]) {
                self.delta.set(self.options.editor_settings.indent_size);
            } else {
                self.delta.set(0);
            }
        }
    }

    // Go: format/span.go:1193 shouldAddDelta
    pub fn should_add_delta(&self, line: i32, kind: SyntaxKind, container: Node) -> bool {
        match kind {
            // open and close brace, 'else' and 'while' (in do statement) tokens has indentation of the parent
            SyntaxKind::OpenBraceToken
            | SyntaxKind::CloseBraceToken
            | SyntaxKind::CloseParenToken
            | SyntaxKind::ElseKeyword
            | SyntaxKind::WhileKeyword
            | SyntaxKind::AtToken => {
                return false;
            }
            SyntaxKind::SlashToken | SyntaxKind::GreaterThanToken => match container.kind() {
                SyntaxKind::JsxOpeningElement
                | SyntaxKind::JsxClosingElement
                | SyntaxKind::JsxSelfClosingElement => {
                    return false;
                }
                _ => {}
            },
            SyntaxKind::OpenBracketToken | SyntaxKind::CloseBracketToken => {
                if container.kind() != SyntaxKind::MappedType {
                    return false;
                }
            }
            _ => {}
        }
        // if token line equals to the line of containing node (this is a first token in the node) - use node indentation
        self.node_start_line != line
            // if this token is the first token following the list of decorators, we do not need to indent
            && !(has_decorators(self.node) && kind == get_first_non_decorator_token_of_node(self.node))
    }
}

// Go: format/span.go:1214 getFirstNonDecoratorTokenOfNode
pub fn get_first_non_decorator_token_of_node(node: Node) -> SyntaxKind {
    if can_have_modifiers(node) {
        let modifier_nodes = node.modifier_nodes();
        let first_decorator = modifier_nodes.iter().position(is_decorator);
        // PORT: Go slices from core.FindIndex, which is -1 when there is no
        // decorator; that slice expression panics.
        let Some(first_decorator) = first_decorator else {
            panic!("runtime error: slice bounds out of range [-1:]");
        };
        let modifier = modifier_nodes
            .iter()
            .skip(first_decorator)
            .find(|&m| is_modifier(m))
            .unwrap_or(Node::NIL);
        if modifier.is_some() {
            return modifier.kind();
        }
    }

    match node.kind() {
        SyntaxKind::ClassDeclaration => return SyntaxKind::ClassKeyword,
        SyntaxKind::InterfaceDeclaration => return SyntaxKind::InterfaceKeyword,
        SyntaxKind::FunctionDeclaration => return SyntaxKind::FunctionKeyword,
        SyntaxKind::EnumDeclaration => return SyntaxKind::EnumDeclaration,
        SyntaxKind::GetAccessor => return SyntaxKind::GetKeyword,
        SyntaxKind::SetAccessor => return SyntaxKind::SetKeyword,
        SyntaxKind::MethodDeclaration | SyntaxKind::PropertyDeclaration | SyntaxKind::Parameter => {
            // PORT: Go `case KindMethodDeclaration` falls through to the
            // PropertyDeclaration/Parameter case after the asterisk check.
            if node.kind() == SyntaxKind::MethodDeclaration && node.asterisk_token().is_some() {
                return SyntaxKind::AsteriskToken;
            }
            let name = get_name_of_declaration(node);
            if name.is_some() {
                return name.kind();
            }
        }
        _ => {}
    }

    SyntaxKind::Unknown
}

impl<'t> FormatSpanWorker<'t> {
    // Go: format/span.go:1251 getDynamicIndentation
    pub fn get_dynamic_indentation(
        &self,
        node: Node,
        node_start_line: i32,
        indentation: i32,
        delta: i32,
    ) -> Rc<DynamicIndenter> {
        Rc::new(DynamicIndenter {
            node,
            node_start_line,
            indentation: Cell::new(indentation),
            delta: Cell::new(delta),
            options: self.options().clone(),
            source_file: self.source_file,
        })
    }
}
