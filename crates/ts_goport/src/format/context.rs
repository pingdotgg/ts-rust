use crate::format::prelude::*;

// Go: format/context.go:11 FormattingContext
pub struct FormattingContext {
    pub current_token_span: TextRangeWithKind,
    pub next_token_span: TextRangeWithKind,
    pub context_node: Node,
    pub current_token_parent: Node,
    pub next_token_parent: Node,

    pub context_node_all_on_same_line: Tristate,
    pub next_node_all_on_same_line: Tristate,
    pub tokens_are_on_same_line: Tristate,
    pub context_node_block_is_on_one_line: Tristate,
    pub next_node_block_is_on_one_line: Tristate,

    pub source_file: Node,
    pub formatting_request_kind: FormatRequestKind,
    pub options: lsutil::FormatCodeSettings,
}

// Go: format/context.go:29 NewFormattingContext
// PORT: Go returns a pointer to a new context; the caller owns the value. The
// span fields hold Go zero values (Loc 0..0, KindUnknown) until UpdateContext.
pub fn new_formatting_context(
    file: Node,
    kind: FormatRequestKind,
    options: lsutil::FormatCodeSettings,
) -> FormattingContext {
    FormattingContext {
        current_token_span: new_text_range_with_kind(0, 0, SyntaxKind::Unknown),
        next_token_span: new_text_range_with_kind(0, 0, SyntaxKind::Unknown),
        context_node: Node::NIL,
        current_token_parent: Node::NIL,
        next_token_parent: Node::NIL,
        context_node_all_on_same_line: Tristate::Unknown,
        next_node_all_on_same_line: Tristate::Unknown,
        tokens_are_on_same_line: Tristate::Unknown,
        context_node_block_is_on_one_line: Tristate::Unknown,
        next_node_block_is_on_one_line: Tristate::Unknown,
        source_file: file,
        formatting_request_kind: kind,
        options,
    }
}

impl FormattingContext {
    // Go: format/context.go:38 UpdateContext
    pub fn update_context(
        &mut self,
        cur: TextRangeWithKind,
        cur_parent: Node,
        next: TextRangeWithKind,
        next_parent: Node,
        common_parent: Node,
    ) {
        if cur_parent.is_nil() {
            panic!("nil current range node parent in update context");
        }
        if next_parent.is_nil() {
            panic!("nil next range node parent in update context");
        }
        if common_parent.is_nil() {
            panic!("nil common parent node in update context");
        }
        self.current_token_span = cur;
        self.current_token_parent = cur_parent;
        self.next_token_span = next;
        self.next_token_parent = next_parent;
        self.context_node = common_parent;

        // drop cached results
        self.context_node_all_on_same_line = Tristate::Unknown;
        self.next_node_all_on_same_line = Tristate::Unknown;
        self.tokens_are_on_same_line = Tristate::Unknown;
        self.context_node_block_is_on_one_line = Tristate::Unknown;
        self.next_node_block_is_on_one_line = Tristate::Unknown;
    }

    // Go: format/context.go:62 rangeIsOnOneLine
    pub fn range_is_on_one_line(&self, node: TextRange) -> Tristate {
        if range_is_on_one_line(node, self.source_file) {
            return Tristate::True;
        }
        Tristate::False
    }

    // Go: format/context.go:69 nodeIsOnOneLine
    pub fn node_is_on_one_line(&self, node: Node) -> Tristate {
        self.range_is_on_one_line(with_token_start(node, self.source_file))
    }
}

// Go: format/context.go:73 withTokenStart
pub fn with_token_start(loc: Node, file: Node) -> TextRange {
    let start_pos = get_token_pos_of_node(loc, file, false);
    TextRange::new(start_pos, loc.end())
}

impl FormattingContext {
    // Go: format/context.go:78 blockIsOnOneLine
    pub fn block_is_on_one_line(&self, node: Node) -> Tristate {
        let open_brace =
            astnav::find_child_of_kind(node, SyntaxKind::OpenBraceToken, self.source_file);
        let close_brace =
            astnav::find_child_of_kind(node, SyntaxKind::CloseBraceToken, self.source_file);
        if open_brace.is_some() && close_brace.is_some() {
            let close_brace_start = get_token_pos_of_node(close_brace, self.source_file, false);
            return self.range_is_on_one_line(TextRange::new(open_brace.end(), close_brace_start));
        }
        Tristate::False
    }

    // Go: format/context.go:88 ContextNodeAllOnSameLine
    pub fn context_node_all_on_same_line(&mut self) -> bool {
        if self.context_node_all_on_same_line == Tristate::Unknown {
            self.context_node_all_on_same_line = self.node_is_on_one_line(self.context_node);
        }
        self.context_node_all_on_same_line == Tristate::True
    }

    // Go: format/context.go:95 NextNodeAllOnSameLine
    pub fn next_node_all_on_same_line(&mut self) -> bool {
        if self.next_node_all_on_same_line == Tristate::Unknown {
            self.next_node_all_on_same_line = self.node_is_on_one_line(self.next_token_parent);
        }
        self.next_node_all_on_same_line == Tristate::True
    }

    // Go: format/context.go:102 TokensAreOnSameLine
    pub fn tokens_are_on_same_line(&mut self) -> bool {
        if self.tokens_are_on_same_line == Tristate::Unknown {
            // ts#64597: from the current token's start to the next token's
            // start (N used the next token's end).
            self.tokens_are_on_same_line = self.range_is_on_one_line(TextRange::new(
                self.current_token_span.loc.pos(),
                self.next_token_span.loc.pos(),
            ));
        }
        self.tokens_are_on_same_line == Tristate::True
    }

    // Go: format/context.go:109 ContextNodeBlockIsOnOneLine
    pub fn context_node_block_is_on_one_line(&mut self) -> bool {
        if self.context_node_block_is_on_one_line == Tristate::Unknown {
            self.context_node_block_is_on_one_line = self.block_is_on_one_line(self.context_node);
        }
        self.context_node_block_is_on_one_line == Tristate::True
    }

    // Go: format/context.go:116 NextNodeBlockIsOnOneLine
    pub fn next_node_block_is_on_one_line(&mut self) -> bool {
        if self.next_node_block_is_on_one_line == Tristate::Unknown {
            self.next_node_block_is_on_one_line = self.block_is_on_one_line(self.next_token_parent);
        }
        self.next_node_block_is_on_one_line == Tristate::True
    }
}
