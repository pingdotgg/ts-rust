//! Port of Go `ls/change/tracker.go`.

use crate::ls::change::prelude::*;

use crate::flags_macros::go_enum;
use crate::spanmap::{self, Feature, SpanMap};

// Go: ls/change/tracker.go:22 NodeOptions
// PORT: Go `indentation *int` and `delta *int` are `Option<i32>`. The
// embedded `LeadingTriviaOption` and `TrailingTriviaOption` are the fields
// `leading_trivia_option` and `trailing_trivia_option`. Go unexported fields
// are `pub(crate)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeOptions {
    /// Text to be inserted before the new node
    pub prefix: String,

    /// Text to be inserted after the new node
    pub suffix: String,

    /// Text of inserted node will be formatted with this indentation, otherwise indentation will be inferred from the old node
    pub(crate) indentation: Option<i32>,

    /// Text of inserted node will be formatted with this delta, otherwise delta will be inferred from the new node kind
    pub(crate) delta: Option<i32>,

    pub leading_trivia_option: LeadingTriviaOption,
    pub trailing_trivia_option: TrailingTriviaOption,
    pub(crate) joiner: String,
}

// Go: ls/change/tracker.go:40 LeadingTriviaOption
go_enum!(LeadingTriviaOption, i32 {
    NONE = 0; // LeadingTriviaOptionNone
    EXCLUDE = 1; // LeadingTriviaOptionExclude
    INCLUDE_ALL = 2; // LeadingTriviaOptionIncludeAll
    JS_DOC = 3; // LeadingTriviaOptionJSDoc
    START_LINE = 4; // LeadingTriviaOptionStartLine
});

// Go: ls/change/tracker.go:50 TrailingTriviaOption
go_enum!(TrailingTriviaOption, i32 {
    NONE = 0; // TrailingTriviaOptionNone
    EXCLUDE = 1; // TrailingTriviaOptionExclude
    EXCLUDE_WHITESPACE = 2; // TrailingTriviaOptionExcludeWhitespace
    INCLUDE = 3; // TrailingTriviaOptionInclude
});

// Go: ls/change/tracker.go:59 trackerEditKind
go_enum!(TrackerEditKind, i32 {
    TEXT = 1; // trackerEditKindText
    REMOVE = 2; // trackerEditKindRemove
    REPLACE_WITH_SINGLE_NODE = 3; // trackerEditKindReplaceWithSingleNode
    REPLACE_WITH_MULTIPLE_NODES = 4; // trackerEditKindReplaceWithMultipleNodes
});

// Go: ls/change/tracker.go:68 trackerEdit
// PORT: Go embeds `core.TextRange` and `*ast.Node`; they are the fields
// `text_range` and `node`. Go stores `*trackerEdit` in the change map; here
// the map owns the edits.
#[derive(Clone, Debug, Default)]
pub struct TrackerEdit {
    pub kind: TrackerEditKind,
    pub text_range: TextRange,

    pub new_text: String, // kind == text

    pub node: Node,       // single
    pub nodes: Vec<Node>, // multiple
    pub options: NodeOptions,
}

// Go: ls/change/tracker.go:79 nodesInsertedAtStartState
#[derive(Clone, Copy, Debug)]
pub struct NodesInsertedAtStartState {
    pub node: Node,
    pub source_file: Node,
}

// Go: ls/change/tracker.go:84 Tracker
// PORT: Go embeds `*printer.EmitContext` and `*ast.NodeFactory`. The context
// is the field `emit_context`; the factory (Go
// `&emitContext.Factory.NodeFactory`) is the method `node_factory()`. Go
// promoted calls such as `t.NewToken(..)` are `t.node_factory().new_token(..)`.
// Go unexported fields are `pub(crate)`.
pub struct Tracker {
    // initialized with
    pub(crate) format_settings: lsutil::FormatCodeSettings,
    pub(crate) new_line: String,
    pub(crate) converters: Rc<lsconv::Converters>,
    pub(crate) ctx: Context,
    pub emit_context: Rc<EmitContext>,

    // PORT: Go `*collections.MultiMap[*ast.SourceFile, *trackerEdit]` (a Go
    // map). Go map order is random; the IndexMap keeps insertion order. The
    // result is a map by file name and each file's edits are sorted, so only
    // edits with equal ranges can come out in a different order than Go.
    pub(crate) changes: IndexMap<Node, Vec<TrackerEdit>>,
    pub(crate) deleted_nodes: Vec<DeletedNode>,
    // PORT: Go `map[*ast.Node]*nodesInsertedAtStartState`. Go map order is
    // random; the IndexMap keeps insertion order. Go never stores nil, so the
    // values are not `Option`.
    pub(crate) nodes_with_insertions_at_start: IndexMap<Node, NodesInsertedAtStartState>,

    /// unmappableFiles collects the files for which an edit could not be represented within a single
    /// verbatim span of the original text. GetChanges drops their edits so a partial, corrupting change is
    /// never emitted for a content-mapped file.
    // PORT: Go `collections.Set[string]` is `FxHashSet<String>`. GetChanges
    // sorts the names, so the set order does not show.
    pub(crate) unmappable_files: FxHashSet<String>,

    // created during call to getChanges
    // PORT: Go never assigns this field; `getNonformattedText` uses a local
    // writer.
    pub(crate) writer: Option<ChangeTrackerWriter>,
    // printer
}

// Go: ls/change/tracker.go:108 deletedNode
#[derive(Clone, Copy, Debug)]
pub struct DeletedNode {
    pub source_file: Node,
    pub node: Node,
}

// Go: ls/change/tracker.go:113 NewTracker
pub fn new_tracker(
    ctx: &Context,
    compiler_options: &CompilerOptions,
    format_options: lsutil::FormatCodeSettings,
    converters: Rc<lsconv::Converters>,
) -> Tracker {
    let emit_context = new_emit_context();
    let new_line = compiler_options
        .new_line
        .get_new_line_character()
        .to_string();
    let ctx = format::with_format_code_settings(ctx, &format_options, &new_line); // !!! formatSettings in context?
    Tracker {
        emit_context,
        changes: IndexMap::new(),
        ctx,
        converters,
        format_settings: format_options,
        new_line,
        nodes_with_insertions_at_start: IndexMap::new(),
        deleted_nodes: Vec::new(),
        unmappable_files: FxHashSet::default(),
        writer: None,
    }
}

// PORT: methods that add changes take `&mut self` (Go mutates through the
// `*Tracker` pointer). Go `core.TextPos` positions are `i32`.
impl Tracker {
    /// Go embedded `*ast.NodeFactory` (`&emitContext.Factory.NodeFactory`).
    #[must_use]
    pub fn node_factory(&self) -> &NodeFactory {
        &self.emit_context.factory().ast
    }

    // Go: ls/change/tracker.go:135 GetChanges
    /// GetChanges returns the accumulated text edits grouped by file name. Any file whose edits could not be
    /// faithfully mapped back onto content-mapped original text is omitted from the returned map, and its name
    /// is included in the returned slice. Dropping the whole file (rather than the individual edit) keeps a
    /// logical change atomic, and returning the result inline means a caller cannot forget to check it or
    /// accidentally emit a partial, corrupting change.
    /// Note: after calling this, the Tracker object must be discarded!
    // PORT: Go `map[string][]*lsproto.TextEdit`; the IndexMap keeps the order
    // in which files were first changed (Go map order is random), and
    // `shift_remove` keeps that order for the other files. Go returns a nil
    // slice when no file is unmappable; that is an empty `Vec`.
    pub fn get_changes(&mut self) -> (IndexMap<String, Vec<lsproto::TextEdit>>, Vec<String>) {
        self.finish_delete_declarations();
        self.finish_nodes_with_insertions_at_start();
        let mut changes = self.get_text_changes_from_changes();
        // !!! changes for new files
        if self.unmappable_files.is_empty() {
            return (changes, Vec::new());
        }
        let mut unmappable = Vec::with_capacity(self.unmappable_files.len());
        for file_name in &self.unmappable_files {
            changes.shift_remove(file_name);
            unmappable.push(file_name.clone());
        }
        unmappable.sort();
        (changes, unmappable)
    }

    // Go: ls/change/tracker.go:155 fromLSPEditRange
    /// fromLSPEditRange converts an LSP range to a source file range. For a content-mapped file, an original
    /// range may have several projections; this selects the one belonging to sourceFile. A range with no exact
    /// projection cannot be written back and marks the file unmappable.
    pub(crate) fn from_lsp_edit_range(
        &mut self,
        source_file: Node,
        lsproto_range: lsproto::Range,
    ) -> TextRange {
        let spans = lsconv::from_lsp_range_for_source_file(
            &self.converters,
            source_file,
            lsproto_range,
            Feature::ALL,
        );
        for span in &spans {
            if span.fidelity.is_exact() && span.script == source_file {
                return span.span;
            }
        }
        self.unmappable_files
            .insert(lsconv::Script::original_file_name(&source_file).to_string());
        if !spans.is_empty() {
            return spans[0].span;
        }
        TextRange::new(0, 0)
    }

    // Go: ls/change/tracker.go:173 toLSPEditRange
    /// toLSPEditRange converts a source file range to an LSP range. For a content-mapped file, the range is
    /// mapped back to the original document. If it does not fall entirely within a single verbatim span, the
    /// edit cannot be represented safely: the file is recorded so GetChanges drops its edits, and a best-effort
    /// range is returned so the accumulated edits stay well-formed.
    pub(crate) fn to_lsp_edit_range(
        &mut self,
        source_file: Node,
        text_range: TextRange,
    ) -> lsproto::Range {
        let (r, fidelity) = self.converters.to_lsp_range(&source_file, text_range);
        if !fidelity.is_exact() {
            // The range does not map into a single verbatim span, so the edit cannot be represented safely in
            // the original text. Record the file so GetChanges drops its edits, keeping the best-effort range so
            // the accumulated edits stay well-formed.
            self.unmappable_files
                .insert(lsconv::Script::original_file_name(&source_file).to_string());
        }
        r
    }

    // Go: ls/change/tracker.go:184 ReplaceNode
    // PORT: Go `options *NodeOptions` is `Option<&NodeOptions>`.
    pub fn replace_node(
        &mut self,
        source_file: Node,
        old_node: Node,
        new_node: Node,
        options: Option<&NodeOptions>,
    ) {
        let options = match options {
            Some(options) => options.clone(),
            None => {
                // defaults to `useNonAdjustedPositions`
                NodeOptions {
                    leading_trivia_option: LeadingTriviaOption::EXCLUDE,
                    trailing_trivia_option: TrailingTriviaOption::EXCLUDE,
                    ..NodeOptions::default()
                }
            }
        };
        let range = self.get_adjusted_range(
            source_file,
            old_node,
            old_node,
            options.leading_trivia_option,
            options.trailing_trivia_option,
        );
        self.replace_range(source_file, range, new_node, options);
    }

    // Go: ls/change/tracker.go:195 ReplaceNodeWithNodes
    pub fn replace_node_with_nodes(
        &mut self,
        source_file: Node,
        old_node: Node,
        new_nodes: &[Node],
        options: Option<&NodeOptions>,
    ) {
        let options = match options {
            Some(options) => options.clone(),
            None => NodeOptions {
                leading_trivia_option: LeadingTriviaOption::EXCLUDE,
                trailing_trivia_option: TrailingTriviaOption::EXCLUDE,
                ..NodeOptions::default()
            },
        };
        let range = self.get_adjusted_range(
            source_file,
            old_node,
            old_node,
            options.leading_trivia_option,
            options.trailing_trivia_option,
        );
        self.replace_range_with_nodes(source_file, range, new_nodes, options);
    }

    // Go: ls/change/tracker.go:206 ReplaceRange
    /// ReplaceRange replaces textRange in sourceFile with newNode.
    pub fn replace_range(
        &mut self,
        source_file: Node,
        text_range: TextRange,
        new_node: Node,
        options: NodeOptions,
    ) {
        self.changes
            .entry(source_file)
            .or_default()
            .push(TrackerEdit {
                kind: TrackerEditKind::REPLACE_WITH_SINGLE_NODE,
                text_range,
                options,
                node: new_node,
                ..TrackerEdit::default()
            });
    }

    // Go: ls/change/tracker.go:212 ReplaceRangeWithText
    /// ReplaceRangeWithText replaces an LSP range with text. For a content-mapped file, the LSP range is in the
    /// original document; text may be placed at any exact projection because it does not need formatting context.
    pub fn replace_range_with_text(
        &mut self,
        source_file: Node,
        lsproto_range: lsproto::Range,
        text: &str,
    ) {
        let text_range = self.from_lsp_edit_range(source_file, lsproto_range);
        self.replace_text_range_with_text(source_file, text_range, text);
    }

    // Go: ls/change/tracker.go:218 ReplaceTextRangeWithText
    /// ReplaceTextRangeWithText replaces textRange in sourceFile with text. For a content-mapped file, GetChanges
    /// maps the range back to the original document and drops the file if the edit cannot be represented there.
    pub fn replace_text_range_with_text(
        &mut self,
        source_file: Node,
        text_range: TextRange,
        text: &str,
    ) {
        self.changes
            .entry(source_file)
            .or_default()
            .push(TrackerEdit {
                kind: TrackerEditKind::TEXT,
                text_range,
                new_text: text.to_string(),
                ..TrackerEdit::default()
            });
    }

    // Go: ls/change/tracker.go:223 ReplaceRangeWithNodes
    /// ReplaceRangeWithNodes replaces textRange in sourceFile with newNodes.
    pub fn replace_range_with_nodes(
        &mut self,
        source_file: Node,
        text_range: TextRange,
        new_nodes: &[Node],
        options: NodeOptions,
    ) {
        if new_nodes.len() == 1 {
            self.replace_range(source_file, text_range, new_nodes[0], options);
            return;
        }
        self.changes
            .entry(source_file)
            .or_default()
            .push(TrackerEdit {
                kind: TrackerEditKind::REPLACE_WITH_MULTIPLE_NODES,
                text_range,
                nodes: new_nodes.to_vec(),
                options,
                ..TrackerEdit::default()
            });
    }

    // Go: ls/change/tracker.go:232 insertTextAt
    /// insertTextAt inserts text at an offset in sourceFile.
    pub(crate) fn insert_text_at(&mut self, source_file: Node, pos: i32, text: &str) {
        self.replace_text_range_with_text(source_file, TextRange::new(pos, pos), text);
    }

    // Go: ls/change/tracker.go:237 InsertText
    /// InsertText inserts text at an LSP position.
    pub fn insert_text(&mut self, source_file: Node, pos: lsproto::Position, text: &str) {
        self.replace_range_with_text(
            source_file,
            lsproto::Range {
                start: pos,
                end: pos,
            },
            text,
        );
    }

    // Go: ls/change/tracker.go:241 InsertNodeAt
    pub fn insert_node_at(
        &mut self,
        source_file: Node,
        pos: i32,
        new_node: Node,
        options: NodeOptions,
    ) {
        self.replace_range(source_file, TextRange::new(pos, pos), new_node, options);
    }

    // Go: ls/change/tracker.go:245 InsertNodesAt
    pub fn insert_nodes_at(
        &mut self,
        source_file: Node,
        pos: i32,
        new_nodes: &[Node],
        options: NodeOptions,
    ) {
        self.replace_range_with_nodes(source_file, TextRange::new(pos, pos), new_nodes, options);
    }

    // Go: ls/change/tracker.go:249 InsertNodeAfter
    pub fn insert_node_after(&mut self, source_file: Node, after: Node, new_node: Node) {
        let end_position = self.end_pos_for_insert_node_after(source_file, after, new_node);
        let options = self.get_insert_node_after_options(source_file, after);
        self.insert_node_at(source_file, end_position, new_node, options);
    }

    // Go: ls/change/tracker.go:254 InsertNodesAfter
    pub fn insert_nodes_after(&mut self, source_file: Node, after: Node, new_nodes: &[Node]) {
        let end_position = self.end_pos_for_insert_node_after(source_file, after, new_nodes[0]);
        let options = self.get_insert_node_after_options(source_file, after);
        self.insert_nodes_at(source_file, end_position, new_nodes, options);
    }

    // Go: ls/change/tracker.go:259 InsertNodeBefore
    pub fn insert_node_before(
        &mut self,
        source_file: Node,
        before: Node,
        new_node: Node,
        blank_line_between: bool,
        leading_trivia_option: LeadingTriviaOption,
    ) {
        let pos =
            self.get_adjusted_start_position(source_file, before, leading_trivia_option, false);
        let options = self.get_options_for_insert_node_before(before, new_node, blank_line_between);
        self.insert_node_at(source_file, pos, new_node, options);
    }

    // Go: ls/change/tracker.go:266 TryInsertTypeAnnotation
    /// TryInsertTypeAnnotation inserts a type annotation after the appropriate position on a node
    /// (after the close paren for function-like, after the name/exclamation/question for variable-like).
    /// Returns true if successful.
    pub fn try_insert_type_annotation(
        &mut self,
        source_file: Node,
        node: Node,
        type_node: Node,
    ) -> bool {
        let mut end_node = Node::NIL;
        if is_function_like(node) {
            end_node = astnav::find_child_of_kind(node, SyntaxKind::CloseParenToken, source_file);
            if end_node.is_nil() {
                if !is_arrow_function(node) {
                    return false;
                }
                // If no `)`, is an arrow function `x => x`, so use the end of the first parameter
                let params = node.parameters();
                if params.len() == 0 {
                    return false;
                }
                end_node = params.get(0);
            }
        } else {
            match node.kind() {
                SyntaxKind::VariableDeclaration => {
                    end_node = node.exclamation_token();
                }
                SyntaxKind::PropertySignature => {
                    end_node = node.postfix_token();
                }
                SyntaxKind::PropertyDeclaration => {
                    end_node = node.postfix_token();
                }
                SyntaxKind::Parameter => {
                    end_node = node.question_token();
                }
                _ => {}
            }
            if end_node.is_nil() {
                end_node = node.name();
            }
        }
        if end_node.is_nil() {
            return false;
        }
        self.insert_node_at(
            source_file,
            end_node.end(),
            type_node,
            NodeOptions {
                prefix: ": ".to_string(),
                ..NodeOptions::default()
            },
        );
        true
    }

    // Go: ls/change/tracker.go:305 ParenthesizeArrowParameters
    /// ParenthesizeArrowParameters wraps the parameters of a paren-less arrow function in `(` and `)`.
    /// This is a no-op if the arrow function already has parens.
    pub fn parenthesize_arrow_parameters(&mut self, source_file: Node, arrow_func: Node) {
        if astnav::find_child_of_kind(arrow_func, SyntaxKind::CloseParenToken, source_file)
            .is_some()
        {
            return;
        }
        let params = arrow_func.parameters();
        if params.len() == 0 {
            return;
        }
        let first_param = params.get(0);
        let last_param = params.get(params.len() - 1);
        let start_pos = astnav::get_start_of_node(first_param, source_file, false);
        self.insert_text_at(source_file, start_pos, "(");
        self.insert_text_at(source_file, last_param.end(), ")");
    }

    // Go: ls/change/tracker.go:321 InsertModifierBefore
    /// InsertModifierBefore inserts a modifier token (like 'type') before a node with a trailing space.
    pub fn insert_modifier_before(
        &mut self,
        source_file: Node,
        modifier: SyntaxKind,
        before: Node,
    ) {
        let pos = astnav::get_start_of_node(before, source_file, false);
        let token = self.node_factory().new_token(modifier);
        set_node_loc(token, TextRange::new(pos, pos));
        set_node_parent(token, before.parent());
        self.insert_node_at(
            source_file,
            pos,
            token,
            NodeOptions {
                suffix: " ".to_string(),
                ..NodeOptions::default()
            },
        );
    }

    // Go: ls/change/tracker.go:331 Delete
    /// Delete queues a node for deletion with smart handling of list items, imports, etc.
    /// The actual deletion happens in finishDeleteDeclarations during GetChanges.
    pub fn delete(&mut self, source_file: Node, node: Node) {
        self.deleted_nodes.push(DeletedNode { source_file, node });
    }

    // Go: ls/change/tracker.go:336 DeleteRange
    /// DeleteRange deletes a text range from the source file.
    pub fn delete_range(&mut self, source_file: Node, text_range: TextRange) {
        self.replace_text_range_with_text(source_file, text_range, "");
    }

    // Go: ls/change/tracker.go:342 DeleteNode
    /// DeleteNode deletes a node immediately with specified trivia options.
    /// Stop! Consider using Delete instead, which has logic for deleting nodes from delimited lists.
    pub fn delete_node(
        &mut self,
        source_file: Node,
        node: Node,
        leading_trivia: LeadingTriviaOption,
        trailing_trivia: TrailingTriviaOption,
    ) {
        let rng = self.get_adjusted_range(source_file, node, node, leading_trivia, trailing_trivia);
        self.replace_text_range_with_text(source_file, rng, "");
    }

    // Go: ls/change/tracker.go:347 DeleteNodeRange
    /// DeleteNodeRange deletes a range of nodes with specified trivia options.
    pub fn delete_node_range(
        &mut self,
        source_file: Node,
        start_node: Node,
        end_node: Node,
        leading_trivia: LeadingTriviaOption,
        trailing_trivia: TrailingTriviaOption,
    ) {
        let start_position =
            self.get_adjusted_start_position(source_file, start_node, leading_trivia, false);
        let end_position = self.get_adjusted_end_position(source_file, end_node, trailing_trivia);
        self.replace_text_range_with_text(
            source_file,
            TextRange::new(start_position, end_position),
            "",
        );
    }

    // Go: ls/change/tracker.go:354 finishDeleteDeclarations
    /// finishDeleteDeclarations processes all queued deletions with smart handling for lists and trailing commas.
    fn finish_delete_declarations(&mut self) {
        // PORT: Go `map[*ast.Node]bool` that only ever holds `true`. Go map
        // order is random; the IndexSet keeps insertion order.
        let mut deleted_nodes_in_lists: IndexSet<Node> = IndexSet::new();

        // PORT: a copy, because `delete_declaration` takes the tracker
        // mutably. It never changes `deleted_nodes`.
        let deleted_nodes = self.deleted_nodes.clone();
        for deleted in &deleted_nodes {
            // Skip if this node is contained within another deleted node
            let mut is_contained = false;
            for other in &deleted_nodes {
                if other.source_file == deleted.source_file
                    && other.node != deleted.node
                    && range_contains_range_exclusive(other.node, deleted.node)
                {
                    is_contained = true;
                    break;
                }
            }
            if is_contained {
                continue;
            }

            delete_declaration(
                self,
                &mut deleted_nodes_in_lists,
                deleted.source_file,
                deleted.node,
            );
        }

        // Handle trailing commas for last elements in lists
        for &node in &deleted_nodes_in_lists {
            let source_file = get_source_file_of_node(node);
            let list = format::get_containing_list(node, source_file);
            if list.is_nil() || node != list.nodes().get(list.nodes().len() - 1) {
                continue;
            }
            let list_nodes = list.nodes();

            let mut last_non_deleted_index: i32 = -1;
            let mut i = list_nodes.len() as i32 - 2;
            while i >= 0 {
                if !deleted_nodes_in_lists.contains(&list_nodes.get(i as usize)) {
                    last_non_deleted_index = i;
                    break;
                }
                i -= 1;
            }

            if last_non_deleted_index != -1 {
                let start = list_nodes.get(last_non_deleted_index as usize).end();
                let end = self.start_position_to_delete_node_in_list(
                    source_file,
                    list_nodes.get((last_non_deleted_index + 1) as usize),
                );
                self.replace_text_range_with_text(source_file, TextRange::new(start, end), "");
            }
        }
    }

    // Go: ls/change/tracker.go:398 endPosForInsertNodeAfter
    fn end_pos_for_insert_node_after(
        &mut self,
        source_file: Node,
        after: Node,
        new_node: Node,
    ) -> i32 {
        if need_semicolon_between(after, new_node)
            && (char::from(source_file_text(source_file).as_bytes()[(after.end() - 1) as usize])
                != ';')
        {
            // check if previous statement ends with semicolon
            // if not - insert semicolon to preserve the code from changing the meaning due to ASI
            let end_pos = after.end();
            let semicolon = self.node_factory().new_token(SyntaxKind::SemicolonToken);
            set_node_loc(semicolon, TextRange::new(after.end(), after.end()));
            set_node_parent(semicolon, after.parent());
            self.replace_range(
                source_file,
                TextRange::new(end_pos, end_pos),
                semicolon,
                NodeOptions::default(),
            );
        }
        self.get_adjusted_end_position(source_file, after, TrailingTriviaOption::NONE)
    }

    // Go: ls/change/tracker.go:421 InsertNodeInListAfter
    /**
     * This function should be used to insert nodes in lists when nodes don't carry separators as the part of the node range,
     * i.e. arguments in arguments lists, parameters in parameter lists etc.
     * Note that separators are part of the node in statements and class elements.
     */
    // PORT: Go `containingList *ast.NodeList`; pass `NodeList::NIL` for nil.
    pub fn insert_node_in_list_after(
        &mut self,
        source_file: Node,
        after: Node,
        new_node: Node,
        containing_list: NodeList,
    ) {
        let mut containing_list = containing_list;
        if containing_list.is_nil() {
            containing_list = format::get_containing_list(after, source_file);
        }
        if containing_list.is_nil() {
            // Debug.fail("node is not a list element")
            return;
        }
        let containing_nodes = containing_list.nodes();
        let index: i32 = containing_nodes
            .iter()
            .position(|n| n == after)
            .map_or(-1, |i| i as i32);
        if index < 0 {
            return;
        }
        let end = after.end();
        if index != containing_nodes.len() as i32 - 1 {
            // any element except the last one
            // use next sibling as an anchor
            let next_token = astnav::get_token_at_position(source_file, after.end());
            if next_token.is_some() && is_separator(after, next_token) {
                // for list
                // a, b, c
                // create change for adding 'e' after 'a' as
                // - find start of next element after a (it is b)
                // - use next element start as start and end position in final change
                // - build text of change by formatting the text of node + whitespace trivia of b

                // in multiline case it will work as
                //   a,
                //   b,
                //   c,
                // result - '*' denotes leading trivia that will be inserted after new text (displayed as '#')
                //   a,
                //   insertedtext<separator>#
                // ###b,
                //   c,
                let next_node = containing_nodes.get((index + 1) as usize);
                let text = source_file_text(source_file);
                let start_pos = skip_trivia_ex(
                    &text,
                    next_node.pos(),
                    Some(&SkipTriviaOptions {
                        stop_after_line_break: false,
                        stop_at_comments: true,
                        ..SkipTriviaOptions::default()
                    }),
                );

                // write separator and leading trivia of the next element as suffix
                // PORT: Go slices the text by byte; token end and trivia end
                // are character boundaries.
                let suffix = token_to_string(next_token.kind()).to_string()
                    + &text[next_token.end() as usize..start_pos as usize];
                self.insert_nodes_at(
                    source_file,
                    start_pos,
                    &[new_node],
                    NodeOptions {
                        suffix,
                        ..NodeOptions::default()
                    },
                );
            }
            return;
        }

        let after_start = astnav::get_start_of_node(after, source_file, false);
        let after_start_line_position =
            format::get_line_start_position_for_position(after_start, source_file);

        // insert element after the last element in the list that has more than one item
        // pick the element preceding the after element to:
        // - pick the separator
        // - determine if list is a multiline
        let mut multiline_list = false;

        // if list has only one element then we'll format is as multiline if node has comment in trailing trivia, or as singleline otherwise
        // i.e. var x = 1 // this is x
        //     | new element will be inserted at this position
        let mut separator = SyntaxKind::CommaToken; // SyntaxKind.CommaToken | SyntaxKind.SemicolonToken
        if containing_nodes.len() != 1 {
            // otherwise, if list has more than one element, pick separator from the list
            let token_before_insert_position =
                astnav::find_preceding_token(source_file, after.pos());
            // PORT: Go `core.IfElse` evaluates both arguments, so the token
            // kind is read before the test (as Go, a nil token fails here).
            let token_before_insert_position_kind = token_before_insert_position.kind();
            separator = if is_separator(after, token_before_insert_position) {
                token_before_insert_position_kind
            } else {
                SyntaxKind::CommaToken
            };
            // determine if list is multiline by checking lines of after element and element that precedes it.
            let after_minus_one_start_line_position = format::get_line_start_position_for_position(
                astnav::get_start_of_node(
                    containing_nodes.get((index - 1) as usize),
                    source_file,
                    false,
                ),
                source_file,
            );
            multiline_list = after_minus_one_start_line_position != after_start_line_position;
        }
        if has_comments_before_line_break(&source_file_text(source_file), after.end())
            || !positions_are_on_same_line(
                containing_list.pos(),
                containing_list.end(),
                source_file,
            )
        {
            // in this case we'll always treat containing list as multiline
            multiline_list = true;
        }
        if multiline_list {
            // insert separator immediately following the 'after' node to preserve comments in trailing trivia
            let separator_token = self.node_factory().new_token(separator);
            let separator_string = token_to_string(separator);
            set_node_loc(
                separator_token,
                TextRange::new(end, end + separator_string.len() as i32),
            );
            set_node_parent(separator_token, after.parent());
            let end_pos = end;
            self.replace_range(
                source_file,
                TextRange::new(end_pos, end_pos),
                separator_token,
                NodeOptions::default(),
            );
            // use the same indentation as 'after' item
            let indentation = format::find_first_non_whitespace_column(
                after_start_line_position,
                after_start,
                source_file,
                &self.format_settings,
            );
            // insert element before the line break on the line that contains 'after' element
            let text = source_file_text(source_file);
            let mut insert_pos = skip_trivia_ex(
                &text,
                end,
                Some(&SkipTriviaOptions {
                    stop_after_line_break: true,
                    stop_at_comments: false,
                    ..SkipTriviaOptions::default()
                }),
            );
            // find position before "\n" or "\r\n"
            while insert_pos != end
                && is_line_break(char::from(text.as_bytes()[(insert_pos - 1) as usize]))
            {
                insert_pos -= 1;
            }
            let insert_ls_pos = insert_pos;
            let prefix = self.new_line.clone();
            self.replace_range(
                source_file,
                TextRange::new(insert_ls_pos, insert_ls_pos),
                new_node,
                NodeOptions {
                    indentation: Some(indentation),
                    prefix,
                    ..NodeOptions::default()
                },
            );
        } else {
            let separator_string = token_to_string(separator);
            let end_pos = end;
            self.replace_range(
                source_file,
                TextRange::new(end_pos, end_pos),
                new_node,
                NodeOptions {
                    prefix: separator_string.to_string() + " ",
                    ..NodeOptions::default()
                },
            );
        }
    }

    // Go: ls/change/tracker.go:523 InsertImportSpecifierAtIndex
    /// InsertImportSpecifierAtIndex inserts a new import specifier at the specified index in a NamedImports list
    pub fn insert_import_specifier_at_index(
        &mut self,
        source_file: Node,
        new_specifier: Node,
        named_imports: Node,
        index: i32,
    ) {
        let elements = named_imports.elements();

        let mut prev_specifier = Node::NIL;
        if index > 0 && ((index - 1) as usize) < elements.len() {
            prev_specifier = elements.get((index - 1) as usize);
        }
        if prev_specifier.is_some() {
            self.insert_node_in_list_after(
                source_file,
                prev_specifier,
                new_specifier,
                NodeList::NIL,
            );
        } else {
            let blank_line_between = !positions_are_on_same_line(
                astnav::get_start_of_node(elements.get(0), source_file, false),
                astnav::get_start_of_node(named_imports.parent().parent(), source_file, false),
                source_file,
            );
            self.insert_node_before(
                source_file,
                elements.get(0),
                new_specifier,
                blank_line_between,
                LeadingTriviaOption::NONE,
            );
        }
    }

    // Go: ls/change/tracker.go:544 InsertAtTopOfFile
    pub fn insert_at_top_of_file(
        &mut self,
        source_file: Node,
        insert: &[Node],
        blank_line_between: bool,
    ) {
        if insert.is_empty() {
            return;
        }

        let mut pos = self.get_insertion_position_at_source_file_top(source_file);
        let mut original_pos = pos;
        // A content mapper may synthesize a header. Advance to the first writable segment so the insertion
        // maps exactly to the original file, and use its original position when deciding leading trivia.
        // PORT: Go `sourceFile.SpanMap()`; the `lsconv::Script` method of the
        // file root `Node`, the span map `to_lsp_edit_range` checks.
        if let Some(span_map) = lsconv::Script::span_map(&source_file) {
            for segment in SpanMap::segments(Some(span_map)) {
                if segment.kind != spanmap::Kind::VERBATIM || segment.virtual_end <= pos {
                    continue;
                }
                if segment.virtual_start > pos {
                    pos = segment.virtual_start;
                }
                original_pos = segment.original_start + pos - segment.virtual_start;
                break;
            }
        }
        let mut options = NodeOptions::default();
        if original_pos != 0 {
            options.prefix = self.new_line.clone();
        }
        let text = source_file_text(source_file);
        if text.is_empty() || !is_line_break(char::from(text.as_bytes()[pos as usize])) {
            options.suffix = self.new_line.clone();
        }
        if blank_line_between {
            options.suffix += &self.new_line;
        }

        if insert.len() == 1 {
            self.insert_node_at(source_file, pos, insert[0], options);
        } else {
            self.insert_nodes_at(source_file, pos, insert, options);
        }
    }

    // Go: ls/change/tracker.go:583 InsertMemberAtStart
    pub fn insert_member_at_start(&mut self, source_file: Node, node: Node, new_element: Node) {
        self.insert_node_at_start_worker(source_file, node, new_element);
    }

    // Go: ls/change/tracker.go:587 insertNodeAtStartWorker
    fn insert_node_at_start_worker(&mut self, source_file: Node, node: Node, new_element: Node) {
        let mut indentation = self.try_compute_indentation_from_existing_members(source_file, node);
        if indentation < 0 {
            indentation = self.try_compute_indentation_for_new_member(source_file, node);
        }

        let members = get_members_or_properties(node);
        if members.is_nil() {
            return;
        }

        let options = self.get_insert_node_at_start_insert_options(source_file, node, indentation);
        self.insert_node_at(source_file, members.pos(), new_element, options);
    }

    // Go: ls/change/tracker.go:601 tryComputeIndentationForNewMember
    fn try_compute_indentation_for_new_member(&self, source_file: Node, node: Node) -> i32 {
        let node_start = astnav::get_start_of_node(node, source_file, false);
        let line_start = format::get_line_start_position_for_position(node_start, source_file);

        let mut tab_size = self.format_settings.editor_settings.tab_size;
        if tab_size <= 0 {
            tab_size = 4;
        }

        let mut indent_size = self.format_settings.editor_settings.indent_size;
        if indent_size <= 0 {
            indent_size = 4;
        }
        i32::max(
            find_indentation_column(
                &source_file_text(source_file),
                line_start,
                node_start,
                tab_size,
            ),
            0,
        ) + indent_size
    }

    // Go: ls/change/tracker.go:617 tryComputeIndentationFromExistingMembers
    fn try_compute_indentation_from_existing_members(&self, source_file: Node, node: Node) -> i32 {
        let members = get_members_or_properties(node);
        if members.is_nil() {
            return -1;
        }

        let mut indentation: i32 = -1;
        let text = source_file_text(source_file);
        let mut tab_size = self.format_settings.editor_settings.tab_size;
        let mut last = node;

        if tab_size <= 0 {
            tab_size = 4;
        }

        for member in members.nodes() {
            if member.is_nil() {
                continue;
            }
            if range_start_positions_are_on_same_line(last.loc(), member.loc(), source_file) {
                return -1;
            }

            let member_start = astnav::get_start_of_node(member, source_file, false);
            let line_start =
                format::get_line_start_position_for_position(member_start, source_file);
            let column = find_indentation_column(&text, line_start, member_start, tab_size);
            if column < 0 {
                return -1;
            }

            if indentation >= 0 {
                if indentation != column {
                    return -1;
                }
                last = member;
                continue;
            }

            indentation = column;
            last = member;
        }

        indentation
    }

    // Go: ls/change/tracker.go:662 getInsertNodeAfterOptions
    fn get_insert_node_after_options(&self, source_file: Node, node: Node) -> NodeOptions {
        let new_line_char = &self.new_line;
        let mut options: NodeOptions;
        match node.kind() {
            SyntaxKind::Parameter => {
                // default opts
                options = NodeOptions::default();
            }
            SyntaxKind::ClassDeclaration | SyntaxKind::ModuleDeclaration => {
                options = NodeOptions {
                    prefix: new_line_char.clone(),
                    suffix: new_line_char.clone(),
                    ..NodeOptions::default()
                };
            }

            SyntaxKind::VariableDeclaration
            | SyntaxKind::StringLiteral
            | SyntaxKind::Identifier => {
                options = NodeOptions {
                    prefix: ", ".to_string(),
                    ..NodeOptions::default()
                };
            }

            SyntaxKind::PropertyAssignment => {
                options = NodeOptions {
                    suffix: ",".to_string() + new_line_char,
                    ..NodeOptions::default()
                };
            }

            SyntaxKind::ExportKeyword => {
                options = NodeOptions {
                    prefix: " ".to_string(),
                    ..NodeOptions::default()
                };
            }

            _ => {
                if !(is_statement(node) || is_class_or_type_element(node)) {
                    // Else we haven't handled this kind of node yet -- add it
                    crate::core::go_panic(format!(
                        "unimplemented node type {} in changeTracker.getInsertNodeAfterOptions",
                        crate::gostd::debug::kind_string(node.kind())
                    ));
                }
                options = NodeOptions {
                    suffix: new_line_char.clone(),
                    ..NodeOptions::default()
                };
            }
        }
        if node.end() == source_file.end() && is_statement(node) {
            options.prefix = self.new_line.clone() + &options.prefix;
        }

        options
    }

    // Go: ls/change/tracker.go:695 getOptionsForInsertNodeBefore
    fn get_options_for_insert_node_before(
        &self,
        before: Node,
        inserted: Node,
        blank_line_between: bool,
    ) -> NodeOptions {
        if is_statement(before) || is_class_or_type_element(before) {
            if blank_line_between {
                return NodeOptions {
                    suffix: self.new_line.clone() + &self.new_line,
                    ..NodeOptions::default()
                };
            }
            return NodeOptions {
                suffix: self.new_line.clone(),
                ..NodeOptions::default()
            };
        } else if before.kind() == SyntaxKind::VariableDeclaration {
            // insert `x = 1, ` into `const x = 1, y = 2;
            return NodeOptions {
                suffix: ", ".to_string(),
                ..NodeOptions::default()
            };
        } else if before.kind() == SyntaxKind::Parameter {
            if inserted.kind() == SyntaxKind::Parameter {
                return NodeOptions {
                    suffix: ", ".to_string(),
                    ..NodeOptions::default()
                };
            }
            return NodeOptions::default();
        } else if (before.kind() == SyntaxKind::StringLiteral
            && before.parent().is_some()
            && before.parent().kind() == SyntaxKind::ImportDeclaration)
            || before.kind() == SyntaxKind::NamedImports
        {
            return NodeOptions {
                suffix: ", ".to_string(),
                ..NodeOptions::default()
            };
        } else if before.kind() == SyntaxKind::ImportSpecifier {
            let mut suffix = ",".to_string();
            if blank_line_between {
                suffix += &self.new_line;
            } else {
                suffix += " ";
            }
            return NodeOptions {
                suffix,
                ..NodeOptions::default()
            };
        }
        // We haven't handled this kind of node yet -- add it
        crate::core::go_panic(format!(
            "unimplemented node type {} in changeTracker.getOptionsForInsertNodeBefore",
            crate::gostd::debug::kind_string(before.kind())
        ));
    }

    // Go: ls/change/tracker.go:724 getInsertNodeAtStartInsertOptions
    fn get_insert_node_at_start_insert_options(
        &mut self,
        source_file: Node,
        node: Node,
        indentation: i32,
    ) -> NodeOptions {
        let has_previous_insertion = self.nodes_with_insertions_at_start.contains_key(&node);
        if !has_previous_insertion {
            self.nodes_with_insertions_at_start
                .insert(node, NodesInsertedAtStartState { node, source_file });
        }

        let members = get_members_or_properties(node);
        let is_object_literal = is_object_literal_expression(node);
        let is_json = is_json_source_file(source_file);

        let has_members = members.is_some() && members.nodes().len() > 0;

        let insert_trailing_comma = is_object_literal && (has_members || !is_json);
        let insert_leading_comma =
            is_object_literal && is_json && !has_members && has_previous_insertion;

        let mut suffix = String::new();
        if insert_trailing_comma {
            suffix = ",".to_string();
        } else if is_interface_declaration(node) && !has_members {
            suffix = ";".to_string();
        }

        let mut prefix = self.new_line.clone();
        if insert_leading_comma {
            prefix = ",".to_string() + &prefix;
        }

        NodeOptions {
            indentation: Some(indentation),
            prefix,
            suffix,
            ..NodeOptions::default()
        }
    }

    // Go: ls/change/tracker.go:759 finishNodesWithInsertionsAtStart
    fn finish_nodes_with_insertions_at_start(&mut self) {
        // PORT: Go map order is random; this walks the states in insertion
        // order. A copy, because the loop adds changes. Go also skips nil
        // states, which it never stores.
        let states: Vec<NodesInsertedAtStartState> = self
            .nodes_with_insertions_at_start
            .values()
            .copied()
            .collect();
        for state in states {
            let open_brace = astnav::find_child_of_kind(
                state.node,
                SyntaxKind::OpenBraceToken,
                state.source_file,
            );
            if open_brace.is_nil() {
                continue;
            }

            let close_brace = astnav::find_child_of_kind(
                state.node,
                SyntaxKind::CloseBraceToken,
                state.source_file,
            );
            if close_brace.is_nil() {
                continue;
            }

            let members = get_members_or_properties(state.node);
            let is_empty = members.is_nil() || members.nodes().len() == 0;
            let is_single_line =
                positions_are_on_same_line(open_brace.end(), close_brace.end(), state.source_file);

            if is_empty && is_single_line && open_brace.end() != close_brace.end() - 1 {
                self.delete_range(
                    state.source_file,
                    TextRange::new(open_brace.end(), close_brace.end() - 1),
                );
            }

            if is_single_line {
                let new_line = self.new_line.clone();
                self.insert_text_at(state.source_file, close_brace.end() - 1, &new_line);
            }
        }
    }
}

// Go: ls/change/tracker.go:789 getMembersOrProperties
fn get_members_or_properties(node: Node) -> NodeList {
    if is_object_literal_expression(node) {
        return node.property_list();
    }
    node.member_list()
}

// Go: ls/change/tracker.go:796 rangeContainsRangeExclusive
fn range_contains_range_exclusive(outer: Node, inner: Node) -> bool {
    outer.pos() < inner.pos() && inner.end() < outer.end()
}

// Go: ls/change/tracker.go:800 isSeparator
pub fn is_separator(node: Node, candidate: Node) -> bool {
    candidate.is_some()
        && node.parent().is_some()
        && (candidate.kind() == SyntaxKind::CommaToken
            || (candidate.kind() == SyntaxKind::SemicolonToken
                && node.parent().kind() == SyntaxKind::ObjectLiteralExpression))
}

// Go: ls/change/tracker.go:804 findIndentationColumn
fn find_indentation_column(text: &str, line_start: i32, member_start: i32, tab_size: i32) -> i32 {
    let mut column: i32 = 0;

    let bytes = text.as_bytes();
    let mut i = line_start;
    while i < member_start && (i as usize) < bytes.len() {
        // PORT: Go `rune(text[i])` converts one byte, as `char::from(u8)`.
        let ch = char::from(bytes[i as usize]);

        if is_line_break(ch) {
            return -1;
        }
        if is_white_space_single_line(ch) {
            column = advance_indentation_column(column, ch, tab_size);
            i += 1;
            continue;
        }
        return column;
    }

    column
}

// Go: ls/change/tracker.go:823 advanceIndentationColumn
fn advance_indentation_column(column: i32, ch: char, tab_size: i32) -> i32 {
    if ch == '\t' {
        return column + tab_size - (column % tab_size);
    }
    column + 1
}
