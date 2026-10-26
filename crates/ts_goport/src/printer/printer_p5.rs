//! Port of printer/printer.go lines 4692 to 6280: lists, general entry
//! points (Emit, Write), comments, source map stubs, name generation,
//! scoped operations, token emit flags and list formats.
//!
//! PORT: this part assumes these `Printer` field types from printer_p1.rs:
//! - `writer` and `own_writer`: `Option<Rc<RefCell<dyn EmitTextWriter>>>`
//!   (Go interface values are shared; the caller of `Write` keeps its writer).
//! - `print_handlers: PrintHandlers` (Go embedding), with handler fields
//!   `Option<Rc<dyn Fn(..)>>`.
//! - `current_source_file`, `external_helpers_module_name`: `Node`.
//! - `source_map_source`, `most_recent_source_map_source`: `SourceMapSource`.
//! - `unique_helper_names`: `Option<FxHashMap<String, Node>>` (Go nil map).
//! - `detached_comments_info`: `Vec<DetachedCommentsInfo>` (Go core.Stack).
//! - `source_map_generator`: `Option<Rc<RefCell<SourceMapGenerator>>>`.
//! - `source_map_source_index`, `most_recent_source_map_source_index`: `i32`.
//! - `source_map_line_char_cache`: an `Option<..>` (Go pointer).
//! - Go `*commentState` / `*sourceMapState` from the arenas are
//!   `Option<CommentState>` / `Option<SourceMapState>` values. The arenas
//!   only avoid Go heap allocation, so they are not used here.

use crate::flags_macros::{go_enum, go_flags};
use crate::gostd::debug::kind_string;
use crate::prelude::*;
use crate::printer::semicolon_writer::get_trailing_semicolon_deferring_writer;
use crate::printer::*;

thread_local! {
    /// Go `p.emitContext.Factory.AsNodeFactory()` for the comment range
    /// scanner. The scanner only uses it to build `CommentRange` values.
    static COMMENT_RANGE_FACTORY: NodeFactory = NodeFactory::default();
}

// Go: scanner/scanner.go:2799 GetLeadingCommentRanges
// PORT: the free functions below have no printer, so they use a plain
// `ast` factory; the scanner does not allocate nodes with it.
fn get_leading_comment_ranges(text: &str, pos: i32) -> Vec<CommentRange> {
    COMMENT_RANGE_FACTORY
        .with(|f| crate::frontend::scanner::get_leading_comment_ranges(f, text, pos))
}

// Go: scanner/scanner.go:2803 GetTrailingCommentRanges
fn get_trailing_comment_ranges(text: &str, pos: i32) -> Vec<CommentRange> {
    COMMENT_RANGE_FACTORY
        .with(|f| crate::frontend::scanner::get_trailing_comment_ranges(f, text, pos))
}

#[inline]
fn for_each_leading_comment_range(text: &str, pos: i32, yield_: impl FnMut(CommentRange) -> bool) {
    COMMENT_RANGE_FACTORY
        .with(|f| crate::frontend::scanner::for_each_comment_range(f, text, pos, false, yield_))
}

#[inline]
fn for_each_trailing_comment_range(text: &str, pos: i32, yield_: impl FnMut(CommentRange) -> bool) {
    COMMENT_RANGE_FACTORY
        .with(|f| crate::frontend::scanner::for_each_comment_range(f, text, pos, true, yield_))
}

impl Printer {
    // PORT: shorthand for Go `p.writer` method calls. Panics on a nil writer,
    // like Go.
    fn writer_p5(&self) -> std::cell::RefMut<'_, dyn EmitTextWriter> {
        self.writer.as_ref().expect("nil writer").borrow_mut()
    }

    //
    // Lists
    //

    // Go: printer/printer.go:4726 emitList
    pub(crate) fn emit_list(
        &mut self,
        emit: fn(&mut Printer, Node),
        parent_node: Node,
        children: NodeList,
        mut format: ListFormat,
    ) {
        if self.should_emit_on_multiple_lines(parent_node) {
            format |= ListFormat::PREFER_NEW_LINE | ListFormat::INDENTED;
        }

        self.emit_list_range(
            emit,
            parent_node,
            children,
            format,
            -1, /*start*/
            -1, /*count*/
        );
    }

    // Go: printer/printer.go:4734 emitListRange
    pub(crate) fn emit_list_range(
        &mut self,
        emit: fn(&mut Printer, Node),
        parent_node: Node,
        children: NodeList,
        format: ListFormat,
        mut start: i32,
        mut count: i32,
    ) {
        let is_nil = children.is_nil();

        let mut length: i32 = 0;
        if !is_nil {
            length = children.nodes().len() as i32;
        }

        if start < 0 {
            start = 0;
        }

        if count < 0 {
            count = length - start;
        }

        if is_nil && format.intersects(ListFormat::OPTIONAL_IF_NIL) {
            return;
        }

        let is_empty = is_nil || start >= length || count <= 0;
        if is_empty && format.intersects(ListFormat::OPTIONAL_IF_EMPTY) {
            if let Some(f) = &self.print_handlers.on_before_emit_node_list {
                f(children);
            }
            if let Some(f) = &self.print_handlers.on_after_emit_node_list {
                f(children);
            }
            return;
        }

        if format.intersects(ListFormat::BRACKETS_MASK) {
            self.write_punctuation(get_opening_bracket(format));
            if is_empty && !is_nil {
                self.emit_trailing_comments(children.pos(), CommentSeparator::BEFORE); // Emit comments within empty lists
            }
        }

        if let Some(f) = &self.print_handlers.on_before_emit_node_list {
            f(children);
        }

        if is_empty {
            // Write a line terminator if the parent node was multi-line
            if format.intersects(ListFormat::MULTI_LINE)
                && !(self.options.preserve_source_newlines
                    && (parent_node.is_nil()
                        || self.current_source_file.is_some()
                            && range_is_on_single_line(
                                parent_node.loc(),
                                self.current_source_file,
                            )))
            {
                self.write_line();
            } else if format.intersects(ListFormat::SPACE_BETWEEN_BRACES)
                && !format.intersects(ListFormat::NO_SPACE_IF_EMPTY)
            {
                self.write_space();
            }
        } else {
            let end = (start + count).min(length);

            // PERF: printfast1. Go slices `children.Nodes` and reads
            // `children.Loc`. A factory list gives its nodes and its range
            // in one arena read, not one read per node.
            let (nodes, children_loc) = match children.synthetic_list() {
                Some(list) => crate::ast::synthetic::synthetic_list_nodes_and_loc(list),
                None => (children.nodes().to_vec(), children.loc()),
            };
            // Go `children.HasTrailingComma()` (ast.go:139) from the nodes
            // and range read above.
            let list_has_trailing_comma = nodes
                .last()
                .is_some_and(|last| last.end() < children_loc.end());
            let has_trailing_comma =
                self.has_trailing_comma(parent_node, children, list_has_trailing_comma);
            self.emit_list_items(
                emit,
                parent_node,
                &nodes[start as usize..end as usize],
                format,
                has_trailing_comma,
                children_loc,
            );
        }

        if let Some(f) = &self.print_handlers.on_after_emit_node_list {
            f(children);
        }

        if format.intersects(ListFormat::BRACKETS_MASK) {
            if is_empty && !is_nil {
                self.emit_leading_comments(children.end(), false /*elided*/); // Emit comments within empty lists
            }
            self.write_punctuation(get_closing_bracket(format));
        }
    }

    // Go: printer/printer.go:4801 hasTrailingComma
    // PERF: printfast1. The caller passes Go `children.HasTrailingComma()`
    // (`list_has_trailing_comma`): `emit_list_range` has the nodes and range
    // of the list, so it needs no second read of a factory list.
    pub(crate) fn has_trailing_comma(
        &mut self,
        parent_node: Node,
        children: NodeList,
        list_has_trailing_comma: bool,
    ) -> bool {
        // NodeList.HasTrailingComma() is unreliable on transformed nodes as some nodes may have been removed. In the event
        // we believe we may need to emit a trailing comma, we must first look to the respective node list on the original
        // node first.
        if !list_has_trailing_comma {
            return false;
        }

        let original_parent = self.emit_context.most_original(parent_node);
        if original_parent == parent_node {
            // if this node is the original node, we can trust the result
            return true;
        }

        if original_parent.kind() != parent_node.kind() {
            // if the original node is some other kind of node, we cannot correlate the list
            return false;
        }

        // find the respective node list on the original parent
        let mut original_list = children;
        match original_parent.kind() {
            SyntaxKind::ObjectLiteralExpression => {
                original_list = original_parent.property_list();
            }
            SyntaxKind::ArrayLiteralExpression => {
                original_list = original_parent.element_list();
            }
            SyntaxKind::CallExpression | SyntaxKind::NewExpression => {
                if children == parent_node.type_argument_list() {
                    original_list = original_parent.type_argument_list();
                } else if children == parent_node.argument_list() {
                    original_list = original_parent.argument_list();
                }
            }
            SyntaxKind::Constructor
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::FunctionType
            | SyntaxKind::ConstructorType
            | SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature => {
                if children == parent_node.type_parameter_list() {
                    original_list = original_parent.type_parameter_list();
                } else if children == parent_node.parameter_list() {
                    original_list = original_parent.parameter_list();
                }
            }
            SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration => {
                if children == parent_node.type_parameter_list() {
                    original_list = original_parent.type_parameter_list();
                }
            }
            SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern => {
                if children == parent_node.element_list() {
                    original_list = original_parent.element_list();
                }
            }
            SyntaxKind::NamedImports | SyntaxKind::NamedExports => {
                original_list = original_parent.element_list();
            }
            SyntaxKind::ImportAttributes => {
                // PORT: Go `AsImportAttributes().Attributes`. The accessor name
                // matches the one printer_p2 and printer_p4 use.
                original_list = original_parent.attribute_list();
            }
            _ => {}
        }

        // if we have the original list, we can use it's result.
        if original_list.is_some() {
            return original_list.has_trailing_comma();
        }

        false
    }

    // Go: printer/printer.go:4875 writeDelimiter
    pub(crate) fn write_delimiter(&mut self, format: ListFormat) {
        let delimiter = format & ListFormat::DELIMITERS_MASK;
        if delimiter == ListFormat::NONE {
            // no delimiter for this format
        } else if delimiter == ListFormat::COMMA_DELIMITED {
            self.write_punctuation(",");
        } else if delimiter == ListFormat::BAR_DELIMITED {
            self.write_space();
            self.write_punctuation("|");
        } else if delimiter == ListFormat::ASTERISK_DELIMITED {
            self.write_space();
            self.write_punctuation("*");
            self.write_space();
        } else if delimiter == ListFormat::AMPERSAND_DELIMITED {
            self.write_space();
            self.write_punctuation("&");
        }
    }

    // Emits a list without brackets or raising events.
    //
    // NOTE: You probably don't want to call this directly and should be using `emitList` instead.
    // Go: printer/printer.go:4897 emitListItems
    pub(crate) fn emit_list_items(
        &mut self,
        emit: fn(&mut Printer, Node),
        parent_node: Node,
        children: &[Node],
        format: ListFormat,
        has_trailing_comma: bool,
        children_text_range: TextRange,
    ) {
        // Write the opening line terminator or leading whitespace.
        let may_emit_intervening_comments = !format.intersects(ListFormat::NO_INTERVENING_COMMENTS);
        let mut should_emit_intervening_comments = may_emit_intervening_comments;

        let mut leading_line_terminator_count = 0;
        if !children.is_empty() {
            leading_line_terminator_count =
                self.get_leading_line_terminator_count(parent_node, children[0], format);
        }
        if leading_line_terminator_count > 0 {
            for _ in 0..leading_line_terminator_count {
                self.write_line();
            }
            should_emit_intervening_comments = false;
        } else if format.intersects(ListFormat::SPACE_BETWEEN_BRACES) {
            self.write_space();
        }

        // Increase the indent, if requested.
        if format.intersects(ListFormat::INDENTED) {
            self.increase_indent();
        }

        let parent_end = greatest_end(-1, &[&parent_node]);

        // Emit each child.
        let mut previous_sibling = Node::NIL;
        // PERF: printfast1. `previous_sibling.End()`, read with its `Pos`
        // before its emit. Go reads its `Loc` again; a factory node reads
        // it in the thread's arena. The printer moves no list node: a debug
        // build asserts that the emit kept the end.
        let mut previous_end = 0;
        let mut should_decrease_indent_after_emit = false;
        for &child in children {
            // Write the delimiter if this is not the first node.
            if format.intersects(ListFormat::ASTERISK_DELIMITED) {
                // always write JSDoc in the format "\n *"
                self.write_line();
                self.write_delimiter(format);
            } else if previous_sibling.is_some() {
                // i.e
                //      function commentedParameters(
                //          /* Parameter a */
                //          a
                //          /* End of parameter a */ -> this comment isn't considered to be trailing comment of parameter "a" due to newline
                //          ,
                if format.intersects(ListFormat::DELIMITERS_MASK) && previous_end != parent_end {
                    if !self.comments_disabled
                        && self.should_emit_trailing_comments(previous_sibling)
                    {
                        self.emit_leading_comments(previous_end, false /*elided*/);
                    }
                }

                self.write_delimiter(format);

                // Write either a line terminator or whitespace to separate the elements.
                let separating_line_terminator_count =
                    self.get_separating_line_terminator_count(previous_sibling, child, format);
                if separating_line_terminator_count > 0 {
                    // If a synthesized node in a single-line list starts on a new
                    // line, we should increase the indent.
                    if format & (ListFormat::LINES_MASK | ListFormat::INDENTED)
                        == ListFormat::SINGLE_LINE
                    {
                        self.increase_indent();
                        should_decrease_indent_after_emit = true;
                    }

                    if should_emit_intervening_comments
                        && format.intersects(ListFormat::DELIMITERS_MASK)
                        && !position_is_synthesized(child.pos())
                        && self.should_emit_leading_comments(child)
                    {
                        let comment_range = self.emit_context.comment_range(child);
                        self.emit_trailing_comments_of_position(
                            comment_range.pos(),
                            format.intersects(ListFormat::SPACE_BETWEEN_SIBLINGS),
                            true, /*forceNoNewline*/
                        );
                    }

                    for _ in 0..separating_line_terminator_count {
                        self.write_line();
                    }

                    should_emit_intervening_comments = false;
                } else if format.intersects(ListFormat::SPACE_BETWEEN_SIBLINGS) {
                    self.write_space();
                }
            }

            // Emit this child.
            if should_emit_intervening_comments && self.should_emit_leading_comments(child) {
                let comment_range = self.emit_context.comment_range(child);
                self.emit_trailing_comments_of_position(
                    comment_range.pos(),
                    false, /*prefixSpace*/
                    false, /*forceNoNewline*/
                );
            } else {
                should_emit_intervening_comments = may_emit_intervening_comments;
            }

            let child_loc = child.loc();
            self.next_list_element_pos = child_loc.pos();
            emit(self, child);
            debug_assert_eq!(
                child.end(),
                child_loc.end(),
                "the emit of a list child moved its end"
            );

            if should_decrease_indent_after_emit {
                self.decrease_indent();
                should_decrease_indent_after_emit = false;
            }

            previous_sibling = child;
            previous_end = child_loc.end();
        }

        // Write a trailing comma, if requested.
        let skip_trailing_comments =
            self.comments_disabled || !self.should_emit_trailing_comments(previous_sibling);
        let emit_trailing_comma = has_trailing_comma
            && format.intersects(ListFormat::ALLOW_TRAILING_COMMA)
            && format.intersects(ListFormat::COMMA_DELIMITED);
        if emit_trailing_comma {
            if previous_sibling.is_some() && !skip_trailing_comments {
                self.emit_token(
                    SyntaxKind::CommaToken,
                    previous_end,
                    WriteKind::PUNCTUATION,
                    previous_sibling,
                );
            } else {
                self.write_punctuation(",");
            }
        }

        // Emit any trailing comment of the last element in the list
        // i.e
        //       var array = [...
        //          2
        //          /* end of element 2 */
        //       ];
        if previous_sibling.is_some()
            && parent_end != previous_end
            && format.intersects(ListFormat::DELIMITERS_MASK)
            && !skip_trailing_comments
        {
            let comments_pos = if emit_trailing_comma && children_text_range.end() > 0 {
                children_text_range.end()
            } else {
                previous_end
            };
            self.emit_leading_comments(comments_pos, false /*elided*/);
        }

        // Decrease the indent, if requested.
        if format.intersects(ListFormat::INDENTED) {
            self.decrease_indent();
        }

        // Write the closing line terminator or closing whitespace.
        let last_child = children.last().copied().unwrap_or(Node::NIL);
        let closing_line_terminator_count = self.get_closing_line_terminator_count(
            parent_node,
            last_child,
            format,
            children_text_range,
        );
        if closing_line_terminator_count > 0 {
            for _ in 0..closing_line_terminator_count {
                self.write_line();
            }
        } else if format.intersects(ListFormat::SPACE_AFTER_LIST | ListFormat::SPACE_BETWEEN_BRACES)
        {
            self.write_space();
        }
    }

    //
    // General
    //

    // Go: printer/printer.go:5044 Emit
    pub fn emit(&mut self, node: Node, source_file: Node) -> String {
        // ensure a reusable writer
        if self.own_writer.is_none() {
            let writer: Rc<RefCell<dyn EmitTextWriter>> = Rc::new(RefCell::new(new_text_writer(
                self.options.new_line.get_new_line_character(),
                0,
            )));
            self.own_writer = Some(writer);
        }

        let own_writer = self.own_writer.clone().expect("own writer");
        self.write_exported(
            node,
            source_file,
            own_writer.clone(),
            None, /*sourceMapGenerator*/
        );
        let text = own_writer.borrow().string();

        own_writer.borrow_mut().clear();
        text
    }

    // Go: printer/printer.go:5057 EmitSourceFile
    // PORT: `_exported` suffix, because the unexported Go `emitSourceFile`
    // has the same snake name.
    pub fn emit_source_file_exported(&mut self, source_file: Node) -> String {
        self.emit(source_file, source_file)
    }

    // Go: printer/printer.go:5061 setSourceFile
    pub(crate) fn set_source_file(&mut self, source_file: Node) {
        self.current_source_file = source_file;
        // PERF: clear the one-entry caches of the current file (printer_p1).
        self.current_original_cache.take();
        self.current_line_map_cache.take();
        self.current_text_cache.take();
        self.unique_helper_names = None;
        self.external_helpers_module_name = Node::NIL;
        if source_file.is_some() {
            if self
                .emit_context
                .emit_flags(self.emit_context.most_original(source_file))
                .intersects(EmitFlags::EXTERNAL_HELPERS)
            {
                self.unique_helper_names = Some(FxHashMap::default());
            }
            self.external_helpers_module_name = self
                .emit_context
                .get_external_helpers_module_name(source_file);
            self.set_source_map_source(SourceMapSource::Node(source_file));
        }

        // !!!
    }

    // Go: printer/printer.go:5076 Write
    // PORT: `_exported` suffix, because the unexported Go `write` has the
    // same snake name.
    pub fn write_exported(
        &mut self,
        node: Node,
        source_file: Node,
        writer: Rc<RefCell<dyn EmitTextWriter>>,
        source_map_generator: Option<Rc<RefCell<SourceMapGenerator>>>,
    ) {
        let saved_current_source_file = self.current_source_file;
        let saved_writer = self.writer.clone();
        let saved_unique_helper_names = self.unique_helper_names.take();
        let saved_source_maps_disabled = self.source_maps_disabled;
        let saved_source_map_generator = self.source_map_generator.take();
        // PORT: `replace` stands for Go's save and the `= nil` below.
        let saved_source_map_source =
            std::mem::replace(&mut self.source_map_source, SourceMapSource::NIL);
        let saved_source_map_source_index = self.source_map_source_index;
        // PORT: `take` stands for Go's save and the `= nil` below.
        let saved_source_map_line_char_cache = self.source_map_line_char_cache.take();

        self.source_maps_disabled = source_map_generator.is_none();
        self.source_map_generator = source_map_generator;
        self.source_map_source_index = -1;

        self.set_source_file(source_file);
        let mut writer = writer;
        if self.options.omit_trailing_semicolon {
            writer = get_trailing_semicolon_deferring_writer(writer);
        }
        self.writer = Some(writer);
        self.writer_p5().clear();
        // PORT: Go grows the writer buffer to the source text length when the
        // writer supports it. That only reserves capacity, so it is skipped.

        match node.kind() {
            // Pseudo-literals
            SyntaxKind::TemplateHead => self.emit_template_head(node),
            SyntaxKind::TemplateMiddle => self.emit_template_middle(node),
            SyntaxKind::TemplateTail => self.emit_template_tail(node),

            // Identifiers
            SyntaxKind::Identifier => self.emit_identifier_name(node),

            // PrivateIdentifiers
            SyntaxKind::PrivateIdentifier => self.emit_private_identifier(node),

            // Parse tree nodes
            // Names
            SyntaxKind::QualifiedName => self.emit_qualified_name(node),
            SyntaxKind::ComputedPropertyName => self.emit_computed_property_name(node),

            // Signature elements
            SyntaxKind::TypeParameter => self.emit_type_parameter(node),
            SyntaxKind::Parameter => self.emit_parameter(node),
            SyntaxKind::Decorator => self.emit_decorator(node),

            // Type members
            SyntaxKind::PropertySignature => self.emit_property_signature(node),
            SyntaxKind::PropertyDeclaration => self.emit_property_declaration(node),
            SyntaxKind::MethodSignature => self.emit_method_signature(node),
            SyntaxKind::MethodDeclaration => self.emit_method_declaration(node),
            SyntaxKind::ClassStaticBlockDeclaration => {
                self.emit_class_static_block_declaration(node)
            }
            SyntaxKind::Constructor => self.emit_constructor(node),
            SyntaxKind::GetAccessor => self.emit_get_accessor_declaration(node),
            SyntaxKind::SetAccessor => self.emit_set_accessor_declaration(node),
            SyntaxKind::CallSignature => self.emit_call_signature(node),
            SyntaxKind::ConstructSignature => self.emit_construct_signature(node),
            SyntaxKind::IndexSignature => self.emit_index_signature(node),

            // Binding patterns
            SyntaxKind::ObjectBindingPattern => self.emit_object_binding_pattern(node),
            SyntaxKind::ArrayBindingPattern => self.emit_array_binding_pattern(node),
            SyntaxKind::BindingElement => self.emit_binding_element(node),

            // Misc
            SyntaxKind::TemplateSpan => self.emit_template_span(node),
            SyntaxKind::SemicolonClassElement => self.emit_semicolon_class_element(node),

            // Declarations (non-statement)
            SyntaxKind::VariableDeclaration => self.emit_variable_declaration(node),
            SyntaxKind::VariableDeclarationList => self.emit_variable_declaration_list(node),
            SyntaxKind::ModuleBlock => self.emit_module_block(node),
            SyntaxKind::CaseBlock => self.emit_case_block(node),
            SyntaxKind::ImportClause => self.emit_import_clause(node),
            SyntaxKind::NamespaceImport => self.emit_namespace_import(node),
            SyntaxKind::NamespaceExport => self.emit_namespace_export(node),
            SyntaxKind::NamedImports => self.emit_named_imports(node),
            SyntaxKind::ImportSpecifier => self.emit_import_specifier(node),
            SyntaxKind::NamedExports => self.emit_named_exports(node),
            SyntaxKind::ExportSpecifier => self.emit_export_specifier(node),
            SyntaxKind::ImportAttributes => self.emit_import_attributes(node),
            SyntaxKind::ImportAttribute => self.emit_import_attribute(node),

            // Module references
            SyntaxKind::ExternalModuleReference => self.emit_external_module_reference(node),

            // JSX (non-expression)
            SyntaxKind::JsxText => self.emit_jsx_text(node),
            SyntaxKind::JsxOpeningElement => self.emit_jsx_opening_element(node),
            SyntaxKind::JsxOpeningFragment => self.emit_jsx_opening_fragment(node),
            SyntaxKind::JsxClosingElement => self.emit_jsx_closing_element(node),
            SyntaxKind::JsxClosingFragment => self.emit_jsx_closing_fragment(node),
            SyntaxKind::JsxAttribute => self.emit_jsx_attribute(node),
            SyntaxKind::JsxAttributes => self.emit_jsx_attributes(node),
            SyntaxKind::JsxSpreadAttribute => self.emit_jsx_spread_attribute(node),
            SyntaxKind::JsxExpression => self.emit_jsx_expression(node),
            SyntaxKind::JsxNamespacedName => self.emit_jsx_namespaced_name(node),

            // Clauses
            SyntaxKind::CaseClause => self.emit_case_clause(node),
            SyntaxKind::DefaultClause => self.emit_default_clause(node),
            SyntaxKind::HeritageClause => self.emit_heritage_clause(node),
            SyntaxKind::CatchClause => self.emit_catch_clause(node),

            // Property assignments
            SyntaxKind::PropertyAssignment => self.emit_property_assignment(node),
            SyntaxKind::ShorthandPropertyAssignment => {
                self.emit_shorthand_property_assignment(node)
            }
            SyntaxKind::SpreadAssignment => self.emit_spread_assignment(node),

            // Enum
            SyntaxKind::EnumMember => self.emit_enum_member(node),

            // Top-level nodes
            SyntaxKind::SourceFile => self.emit_source_file(node),

            // Transformation nodes
            SyntaxKind::NotEmittedTypeElement => self.emit_not_emitted_type_element(node),

            _ => {
                if is_type_node(node) {
                    self.emit_type_node_outside_extends(node);
                } else if is_statement(node) {
                    self.emit_statement(node);
                } else if is_expression(node) {
                    self.emit_expression(node, OperatorPrecedence::LOWEST);
                } else if is_keyword_kind(node.kind()) {
                    self.emit_keyword_node(node);
                } else if is_punctuation_kind(node.kind()) {
                    self.emit_punctuation_node(node);
                } else if is_js_doc_kind(node.kind()) {
                    self.emit_js_doc_node(node);
                } else {
                    panic!("unhandled Node: {}", kind_string(node.kind()));
                }
            }
        }

        self.current_source_file = saved_current_source_file;
        self.writer = saved_writer;
        self.unique_helper_names = saved_unique_helper_names;
        self.source_maps_disabled = saved_source_maps_disabled;
        self.source_map_generator = saved_source_map_generator;
        self.source_map_source = saved_source_map_source;
        self.source_map_source_index = saved_source_map_source_index;
        self.source_map_line_char_cache = saved_source_map_line_char_cache;
    }

    //
    // Comments
    //

    // Go: printer/printer.go:5291 emitCommentsBeforeNode
    pub(crate) fn emit_comments_before_node(&mut self, node: Node) -> Option<CommentState> {
        if !self.should_emit_comments(node) {
            return None;
        }

        let emit_flags = self.emit_context.emit_flags(node);
        let comment_range = self.emit_context.comment_range(node);
        let container_pos = self.container_pos;
        let container_end = self.container_end;
        let declaration_list_container_end = self.declaration_list_container_end;

        // Emit leading comments
        self.emit_leading_comments_of_node(node, emit_flags, comment_range);
        self.emit_leading_synthetic_comments_of_node(node, emit_flags);
        if emit_flags.intersects(EmitFlags::NO_NESTED_COMMENTS) {
            self.comments_disabled = true;
        }

        Some(CommentState {
            emit_flags,
            comment_range,
            container_pos,
            container_end,
            declaration_list_container_end,
        })
    }

    // Go: printer/printer.go:5314 emitCommentsAfterNode
    pub(crate) fn emit_comments_after_node(&mut self, node: Node, state: Option<CommentState>) {
        let Some(state) = state else {
            return;
        };

        let emit_flags = state.emit_flags;
        let comment_range = state.comment_range;
        let container_pos = state.container_pos;
        let container_end = state.container_end;
        let declaration_list_container_end = state.declaration_list_container_end;

        // Emit trailing comments
        if emit_flags.intersects(EmitFlags::NO_NESTED_COMMENTS) {
            self.comments_disabled = false;
        }

        self.emit_trailing_synthetic_comments_of_node(node, emit_flags);
        self.emit_trailing_comments_of_node(
            node,
            emit_flags,
            comment_range,
            container_pos,
            container_end,
            declaration_list_container_end,
        );

        // Preserve comments from erased type annotation
        let type_node = self.emit_context.get_type_node(node);
        if type_node.is_some() {
            self.emit_trailing_comments_of_node(
                node,
                emit_flags,
                type_node.loc(),
                container_pos,
                container_end,
                declaration_list_container_end,
            );
        }
    }

    // Go: printer/printer.go:5339 emitCommentsBeforeToken
    pub(crate) fn emit_comments_before_token(
        &mut self,
        token: SyntaxKind,
        mut pos: i32,
        context_node: Node,
        flags: TokenEmitFlags,
    ) -> (Option<CommentState>, i32) {
        if flags.intersects(TokenEmitFlags::NO_COMMENTS) || self.comments_disabled {
            // Still skip trivia so that the returned pos correctly identifies the token position.
            // This is needed for trailing source map positions (writeTokenText advances pos by token length).
            if self.current_source_file.is_some() && !position_is_synthesized(pos) {
                pos = skip_trivia(&self.current_source_file_text(), pos);
            }
            return (None, pos);
        }

        let start_pos = pos;
        if self.current_source_file.is_some() {
            pos = skip_trivia(&self.current_source_file_text(), start_pos);
        }

        let node = self.emit_context.parse_node(context_node);
        let is_similar_node = node.is_some() && node.kind() == context_node.kind();
        if !is_similar_node {
            return (None, pos);
        }

        if context_node.pos() != start_pos {
            let indent_leading = flags.intersects(TokenEmitFlags::INDENT_LEADING_COMMENTS);
            let needs_indent = indent_leading
                && self.current_source_file.is_some()
                && !positions_are_on_same_line(start_pos, pos, self.current_source_file);
            self.increase_indent_if(needs_indent);
            self.emit_leading_comments(start_pos, false /*elided*/);
            self.decrease_indent_if(needs_indent);
        }

        (Some(CommentState::default()), pos)
    }

    // Go: printer/printer.go:5371 emitCommentsAfterToken
    pub(crate) fn emit_comments_after_token(
        &mut self,
        token: SyntaxKind,
        pos: i32,
        context_node: Node,
        state: Option<CommentState>,
    ) {
        if state.is_none() {
            return;
        }

        if context_node.end() != pos {
            let is_jsx_expr_context = context_node.kind() == SyntaxKind::JsxExpression;
            self.emit_trailing_comments(
                pos,
                if is_jsx_expr_context {
                    CommentSeparator::NONE
                } else {
                    CommentSeparator::BEFORE
                },
            );
        }
    }

    // Go: printer/printer.go:5382 emitDetachedCommentsBeforeStatementList
    pub(crate) fn emit_detached_comments_before_statement_list(
        &mut self,
        node: Node,
        detached_range: TextRange,
    ) -> Option<CommentState> {
        if !self.should_emit_detached_comments(node) {
            return None;
        }

        let emit_flags = self.emit_context.emit_flags(node);
        let container_pos = self.container_pos;
        let container_end = self.container_end;
        let declaration_list_container_end = self.declaration_list_container_end;
        let skip_leading_comments = position_is_synthesized(detached_range.pos())
            || emit_flags.intersects(EmitFlags::NO_LEADING_COMMENTS);

        if !skip_leading_comments {
            self.emit_detached_comments_and_update_comments_info(detached_range);
        }

        if emit_flags.intersects(EmitFlags::NO_NESTED_COMMENTS) {
            self.comments_disabled = true;
        }

        Some(CommentState {
            emit_flags,
            comment_range: detached_range,
            container_pos,
            container_end,
            declaration_list_container_end,
        })
    }

    // Go: printer/printer.go:5404 emitDetachedCommentsAfterStatementList
    pub(crate) fn emit_detached_comments_after_statement_list(
        &mut self,
        node: Node,
        detached_range: TextRange,
        state: Option<CommentState>,
    ) {
        let Some(state) = state else {
            return;
        };

        let emit_flags = state.emit_flags;
        let skip_trailing_comments = self.comments_disabled
            || position_is_synthesized(detached_range.end())
            || emit_flags.intersects(EmitFlags::NO_TRAILING_COMMENTS);

        if !skip_trailing_comments {
            let has_written_comment =
                self.emit_leading_comments(detached_range.end(), false /*elided*/);
            if has_written_comment && !self.writer_p5().is_at_start_of_line() {
                self.write_line();
            }
        }
    }

    // Go: printer/printer.go:5420 emitLeadingCommentsOfNode
    pub(crate) fn emit_leading_comments_of_node(
        &mut self,
        node: Node,
        emit_flags: EmitFlags,
        comment_range: TextRange,
    ) {
        let pos = comment_range.pos();
        let end = comment_range.end();

        // Save current container state on the stack.
        if (!position_is_synthesized(pos) || !position_is_synthesized(end)) && pos != end {
            // We have to explicitly check that the node is JsxText because if the compilerOptions.jsx is "preserve" we will not do any transformation.
            // It is expensive to walk entire tree just to set one kind of node to have no comments.
            let skip_leading_comments = position_is_synthesized(pos)
                || emit_flags.intersects(EmitFlags::NO_LEADING_COMMENTS)
                || node.kind() == SyntaxKind::JsxText;
            let skip_trailing_comments = position_is_synthesized(end)
                || emit_flags.intersects(EmitFlags::NO_TRAILING_COMMENTS)
                || node.kind() == SyntaxKind::JsxText;

            // Emit leading comments if the position is not synthesized and the node
            // has not opted out from emitting leading comments.
            if !skip_leading_comments {
                self.emit_leading_comments(
                    pos,
                    node.kind() == SyntaxKind::NotEmittedStatement, /*elided*/
                );
            }

            if !skip_leading_comments
                || (pos >= 0 && emit_flags.intersects(EmitFlags::NO_LEADING_COMMENTS))
            {
                // Advance the container position if comments get emitted or if they've been disabled explicitly using NoLeadingComments.
                self.container_pos = pos;
            }

            if !skip_trailing_comments
                || (end >= 0 && emit_flags.intersects(EmitFlags::NO_TRAILING_COMMENTS))
            {
                // Advance the container end if comments get emitted or if they've been disabled explicitly using NoTrailingComments.
                self.container_end = end;

                // To avoid invalid comment emit in a down-level binding pattern, we
                // keep track of the last declaration list container's end
                if node.kind() == SyntaxKind::VariableDeclarationList {
                    self.declaration_list_container_end = end;
                }
            }
        }
    }

    // Go: printer/printer.go:5455 emitTrailingCommentsOfNode
    pub(crate) fn emit_trailing_comments_of_node(
        &mut self,
        node: Node,
        emit_flags: EmitFlags,
        comment_range: TextRange,
        container_pos: i32,
        container_end: i32,
        declaration_list_container_end: i32,
    ) {
        let pos = comment_range.pos();
        let end = comment_range.end();
        let skip_trailing_comments = end < 0
            || emit_flags.intersects(EmitFlags::NO_TRAILING_COMMENTS)
            || node.kind() == SyntaxKind::JsxText;
        if (!position_is_synthesized(pos) || !position_is_synthesized(end)) && pos != end {
            // Restore previous container state.
            self.container_pos = container_pos;
            self.container_end = container_end;
            self.declaration_list_container_end = declaration_list_container_end;

            // Emit trailing comments if the position is not synthesized and the node
            // has not opted out from emitting leading comments and is an emitted node.
            if !skip_trailing_comments && node.kind() != SyntaxKind::NotEmittedStatement {
                self.emit_trailing_comments(end, CommentSeparator::BEFORE);
            }
        }
    }

    // Go: printer/printer.go:5473 emitLeadingSyntheticCommentsOfNode
    pub(crate) fn emit_leading_synthetic_comments_of_node(
        &mut self,
        node: Node,
        emit_flags: EmitFlags,
    ) {
        if emit_flags.intersects(EmitFlags::NO_LEADING_COMMENTS) {
            return;
        }
        let synth = self.emit_context.get_synthetic_leading_comments(node);
        for c in &synth {
            self.emit_leading_synthesized_comment(c);
        }
    }

    // Go: printer/printer.go:5483 emitLeadingSynthesizedComment
    pub(crate) fn emit_leading_synthesized_comment(&mut self, comment: &SynthesizedComment) {
        if comment.has_leading_new_line || comment.kind == SyntaxKind::SingleLineCommentTrivia {
            self.writer_p5().write_line();
        }
        self.write_synthesized_comment(comment);
        if comment.has_trailing_new_line || comment.kind == SyntaxKind::SingleLineCommentTrivia {
            self.writer_p5().write_line();
        } else {
            self.writer_p5().write_space(" ");
        }
    }

    // Go: printer/printer.go:5495 emitTrailingSyntheticCommentsOfNode
    pub(crate) fn emit_trailing_synthetic_comments_of_node(
        &mut self,
        node: Node,
        emit_flags: EmitFlags,
    ) {
        if emit_flags.intersects(EmitFlags::NO_TRAILING_COMMENTS) {
            return;
        }
        let synth = self.emit_context.get_synthetic_trailing_comments(node);
        for c in &synth {
            self.emit_trailing_synthesized_comment(c);
        }
    }

    // Go: printer/printer.go:5505 emitTrailingSynthesizedComment
    pub(crate) fn emit_trailing_synthesized_comment(&mut self, comment: &SynthesizedComment) {
        if !self.writer_p5().is_at_start_of_line() {
            self.writer_p5().write_space(" ");
        }
        self.write_synthesized_comment(comment);
        if comment.has_trailing_new_line {
            self.writer_p5().write_line();
        }
    }

    // Go: printer/printer.go:5522 writeSynthesizedComment
    pub(crate) fn write_synthesized_comment(&mut self, comment: &SynthesizedComment) {
        let text = format_synthesized_comment(comment);
        let mut line_map: Vec<i32> = Vec::new();
        if comment.kind == SyntaxKind::MultiLineCommentTrivia {
            line_map = compute_ecma_line_starts(&text);
        }
        self.write_comment_range_worker(
            &text,
            &line_map,
            comment.kind,
            TextRange::new(0, text.len() as i32),
        );
    }

    // Go: printer/printer.go:5531 emitLeadingComments
    pub(crate) fn emit_leading_comments(&mut self, mut pos: i32, elided: bool) -> bool {
        // Emit the leading comments only if the container's pos doesn't match because the container should take care of emitting these comments
        if self.comments_disabled
            || self.current_source_file.is_nil()
            || position_is_synthesized(pos)
            || pos == self.container_pos
        {
            return false;
        }

        let mut triple_slash = Tristate::Unknown;
        if !elided {
            if pos == 0
                && self.current_source_file.is_some()
                && source_file_is_declaration_file(self.current_source_file)
            {
                triple_slash = Tristate::False;
            }
        } else if pos == 0 {
            // If the node will not be emitted in JS, remove all the comments(normal, pinned and ///) associated with the node,
            // unless it is a triple slash comment at the top of the file.
            // For Example:
            //      /// <reference-path ...>
            //      declare var x;
            //      /// <reference-path ...>
            //      interface F {}
            //  The first /// will NOT be removed while the second one will be removed even though both node will not be emitted
            triple_slash = Tristate::True;
        } else {
            return false;
        }

        // skip detached comments
        if !self.detached_comments_info.is_empty() {
            if let Some(info) = self.detached_comments_info.last() {
                if info.node_pos == pos {
                    pos = self
                        .detached_comments_info
                        .pop()
                        .expect("detached comments info")
                        .detached_comment_end_pos;
                }
            }
        }

        let mut comments: Vec<CommentRange> = Vec::new();
        {
            let text = self.current_source_file_text();
            for_each_leading_comment_range(&text, pos, |comment| {
                if self.should_write_comment(comment)
                    && self.should_emit_comment_if_triple_slash(comment, triple_slash)
                {
                    comments.push(comment);
                }
                true
            });
        }

        if !comments.is_empty()
            && self.should_emit_new_line_before_leading_comment_of_position(pos, comments[0].pos())
        {
            self.write_line();
        }

        // Leading comments are emitted as /*leading comment1*/space/*leading comment*/space
        self.emit_comments(&comments, CommentSeparator::AFTER)
    }

    // Go: printer/printer.go:5578 shouldEmitCommentIfTripleSlash
    pub(crate) fn should_emit_comment_if_triple_slash(
        &self,
        comment: CommentRange,
        triple_slash: Tristate,
    ) -> bool {
        match triple_slash {
            Tristate::True => self.is_triple_slash_comment(comment),
            Tristate::False => !self.is_triple_slash_comment(comment),
            _ => true,
        }
    }

    // Go: printer/printer.go:5589 shouldEmitNewLineBeforeLeadingCommentOfPosition
    pub(crate) fn should_emit_new_line_before_leading_comment_of_position(
        &self,
        pos: i32,
        comment_pos: i32,
    ) -> bool {
        // If the leading comments start on different line than the start of node, write new line
        if self.current_source_file.is_nil() || pos == comment_pos {
            return false;
        }
        let line_map = self.current_line_map();
        compute_line_of_position(&line_map, pos) != compute_line_of_position(&line_map, comment_pos)
    }

    // Go: printer/printer.go:5596 emitLeadingCommentsOfPosition
    pub(crate) fn emit_leading_comments_of_position(&mut self, pos: i32) {
        if self.comments_disabled || pos == -1 {
            return;
        }

        self.emit_leading_comments(pos, false /*elided*/);
    }

    // Go: printer/printer.go:5604 emitTrailingComments
    pub(crate) fn emit_trailing_comments(&mut self, pos: i32, comment_separator: CommentSeparator) {
        if self.comments_disabled {
            return;
        }
        // Emit the trailing comments only if the container's end doesn't match because the container should take care of emitting these comments
        if self.comments_disabled
            || self.current_source_file.is_nil()
            || self.container_end != -1
                && (pos == self.container_end || pos == self.declaration_list_container_end)
        {
            return;
        }

        let mut comments: Vec<CommentRange> = Vec::new();
        {
            let text = self.current_source_file_text();
            for_each_trailing_comment_range(&text, pos, |comment| {
                if self.should_write_comment(comment) {
                    comments.push(comment);
                }
                true
            });
        }

        // trailing comments are normally emitted as space/*trailing comment1*/space/*trailing comment2*/
        self.emit_comments(&comments, comment_separator);
    }

    // Go: printer/printer.go:5624 emitTrailingCommentsOfPosition
    pub(crate) fn emit_trailing_comments_of_position(
        &mut self,
        pos: i32,
        prefix_space: bool,
        force_no_newline: bool,
    ) {
        if self.comments_disabled || self.current_source_file.is_nil() {
            return;
        }
        if self.container_end != -1
            && (pos == self.container_end || pos == self.declaration_list_container_end)
        {
            return;
        }

        let mut comments = Vec::new();
        {
            let text = self.current_source_file_text();
            for_each_trailing_comment_range(&text, pos, |c| {
                comments.push(c);
                true
            });
        }
        if comments.is_empty() {
            return;
        }

        for comment in comments {
            if prefix_space {
                if !self.should_write_comment(comment) {
                    continue;
                }
                if !self.writer_p5().is_at_start_of_line() {
                    self.write_space();
                }
                self.emit_comment(comment);
                if comment.has_trailing_new_line {
                    self.write_line();
                }
                continue;
            }

            self.emit_comment(comment);
            if force_no_newline {
                if comment.kind == SyntaxKind::SingleLineCommentTrivia {
                    self.write_line();
                }
            } else if comment.has_trailing_new_line {
                self.write_line();
            } else {
                self.write_space();
            }
        }
    }

    // Go: printer/printer.go:5669 emitDetachedCommentsAndUpdateCommentsInfo
    pub(crate) fn emit_detached_comments_and_update_comments_info(
        &mut self,
        text_range: TextRange,
    ) {
        if self.current_source_file.is_nil() {
            return;
        }
        if let Some(current_detached_comment_info) = self.emit_detached_comments(text_range) {
            self.detached_comments_info
                .push(current_detached_comment_info);
        }
    }

    // Go: printer/printer.go:5678 emitDetachedComments
    // PORT: Go returns `(result, hasResult)`; this returns `Some(result)`
    // when `hasResult` is true.
    pub(crate) fn emit_detached_comments(
        &mut self,
        text_range: TextRange,
    ) -> Option<DetachedCommentsInfo> {
        if self.current_source_file.is_nil() {
            return None;
        }

        let text = self.current_source_file_text();
        let line_map = &*self.current_line_map();

        let mut leading_comments: Vec<CommentRange> = Vec::new();
        if self.comments_disabled {
            // removeComments is true, only reserve pinned comment at the top of file
            // For example:
            //      /*! Pinned Comment */
            //
            //      var x = 10;
            if text_range.pos() == 0 {
                for comment in get_leading_comment_ranges(&text, text_range.pos()) {
                    if is_pinned_comment(&text, comment) {
                        leading_comments.push(comment);
                    }
                }
            }
        } else {
            // removeComments is false, just get detached as normal and bypass the process to filter comment
            leading_comments = get_leading_comment_ranges(&text, text_range.pos());
        }

        let mut result = None;
        if !leading_comments.is_empty() {
            let mut detached_comments: Vec<CommentRange> = Vec::new();
            let mut last_comment: Option<CommentRange> = None;
            for (i, &comment) in leading_comments.iter().enumerate() {
                if i > 0 {
                    let last_comment_line = compute_line_of_position(
                        line_map,
                        last_comment.expect("last comment").end(),
                    );
                    let comment_line = compute_line_of_position(line_map, comment.pos());

                    if comment_line >= last_comment_line + 2 {
                        // There was a blank line between the last comment and this comment.  This
                        // comment is not part of the copyright comments.  Return what we have so
                        // far.
                        break;
                    }
                }

                detached_comments.push(comment);
                last_comment = Some(comment);
            }

            if !detached_comments.is_empty() {
                // All comments look like they could have been part of the copyright header.  Make
                // sure there is at least one blank line between it and the node.  If not, it's not
                // a copyright header.
                let last_detached_end = detached_comments.last().expect("detached comment").end();
                let last_comment_line = compute_line_of_position(line_map, last_detached_end);
                let node_line =
                    compute_line_of_position(line_map, skip_trivia(&text, text_range.pos()));
                if node_line >= last_comment_line + 2 {
                    // Valid detachedComments

                    // Filter to only comments that should be written (e.g., JSDoc-style in declaration emit)
                    let mut comments_to_emit: Vec<CommentRange> = Vec::new();
                    for &comment in &detached_comments {
                        if self.should_write_comment(comment) {
                            comments_to_emit.push(comment);
                        }
                    }

                    if !comments_to_emit.is_empty() {
                        if self.should_emit_new_line_before_leading_comment_of_position(
                            text_range.pos(),
                            comments_to_emit[0].pos(),
                        ) {
                            self.write_line();
                        }

                        self.emit_comments(&comments_to_emit, CommentSeparator::AFTER);
                    }
                    result = Some(DetachedCommentsInfo {
                        node_pos: text_range.pos(),
                        detached_comment_end_pos: last_detached_end,
                    });
                }
            }
        }
        result
    }

    // Go: printer/printer.go:5765 emitComments
    pub(crate) fn emit_comments(
        &mut self,
        comments: &[CommentRange],
        comment_separator: CommentSeparator,
    ) -> bool {
        let mut intervening_separator = false;
        if comments.is_empty() {
            return false;
        }

        if comment_separator == CommentSeparator::BEFORE {
            self.write_space();
        }

        for &comment in comments {
            if intervening_separator {
                self.write_space();
                intervening_separator = false;
            }

            self.emit_comment(comment);

            if comment.kind == SyntaxKind::SingleLineCommentTrivia
                || comment.has_trailing_new_line && comment_separator != CommentSeparator::NONE
            {
                self.write_line();
            } else {
                intervening_separator = comment_separator != CommentSeparator::NONE;
            }
        }

        if intervening_separator && comment_separator == CommentSeparator::AFTER {
            self.write_space();
        }

        true
    }

    // Go: printer/printer.go:5797 emitComment
    pub(crate) fn emit_comment(&mut self, comment: CommentRange) {
        self.emit_pos(comment.pos());
        self.write_comment_range(comment);
        self.emit_pos(comment.end());
    }

    // Go: printer/printer.go:5803 isTripleSlashComment
    pub(crate) fn is_triple_slash_comment(&self, comment: CommentRange) -> bool {
        self.current_source_file.is_some()
            && is_recognized_triple_slash_comment(&self.current_source_file_text(), comment)
    }

    //
    // Source Maps
    //

    // Go: printer/printer.go:5812 setSourceMapSource
    pub(crate) fn set_source_map_source(&mut self, source: SourceMapSource) {
        if self.source_maps_disabled {
            return;
        }

        // PORT: a transformed source file is a synthetic node. Go copies the
        // text and line map of the original file into it. `get_ecma_line_starts`
        // caches line maps by file index, and all synthetic nodes share one
        // file index, so the line map comes from the original file here.
        let line_source = match &source {
            SourceMapSource::Node(node) if is_synthetic_node(*node) => {
                SourceMapSource::Node(self.emit_context.most_original(*node))
            }
            _ => source.clone(),
        };
        self.source_map_source = source.clone();
        self.source_map_line_char_cache = Some(new_line_character_cache(&line_source));
        if self.most_recent_source_map_source == source {
            self.source_map_source_index = self.most_recent_source_map_source_index;
            return;
        }

        let file_name = source.file_name();
        self.source_map_source_is_json =
            crate::frontend::tspath::file_extension_is(file_name, ".json");
        if self.source_map_source_is_json {
            return;
        }

        let generator = self
            .source_map_generator
            .clone()
            .expect("source map generator");
        self.source_map_source_index = generator.borrow_mut().add_source(file_name);
        if self.options.inline_sources {
            if let Err(err) = generator
                .borrow_mut()
                .set_source_content(self.source_map_source_index, &source.text())
            {
                panic!("{err}");
            }
        }

        self.most_recent_source_map_source = source;
        self.most_recent_source_map_source_index = self.source_map_source_index;
    }

    // Go: printer/printer.go:5840 emitPos
    pub(crate) fn emit_pos(&mut self, mut pos: i32) {
        if self.source_maps_disabled
            || self.source_map_source.is_nil()
            || self.source_map_generator.is_none()
            || self.source_map_source_is_json
            || position_is_synthesized(pos)
        {
            return;
        }

        let mut source_index = self.source_map_source_index;
        // PORT: Go copies the `*lineCharacterCache` pointer. The printer's own
        // cache is used in place; a mapped source gets its own cache here.
        let mut mapped_line_char_cache = None;
        if let Some(map_source_position) = self.print_handlers.map_source_position.clone() {
            let source = match &self.source_map_source {
                SourceMapSource::Node(node) => *node,
                // PORT: only the branch below sets a source that is not a
                // node, and it restores the node before it returns.
                SourceMapSource::Other(_) => {
                    panic!("emitPos: the source map source is not a source file")
                }
            };
            let Some((mapped_source, mapped_pos)) = map_source_position(source, pos) else {
                let (line, column) = {
                    let writer = self.writer_p5();
                    (writer.get_line(), writer.get_column())
                };
                let generator = self
                    .source_map_generator
                    .as_ref()
                    .expect("source map generator");
                if let Err(err) = generator.borrow_mut().add_generated_mapping(line, column) {
                    panic!("{err}");
                }
                return;
            };
            pos = mapped_pos;
            // PORT: Go `mappedSource != source`. `None` is the same source.
            if let Some(mapped_source) = mapped_source {
                let saved_source = self.source_map_source.clone();
                let saved_source_index = self.source_map_source_index;
                let saved_source_is_json = self.source_map_source_is_json;
                let saved_line_char_cache = self.source_map_line_char_cache.take();
                self.set_source_map_source(SourceMapSource::Other(mapped_source));
                source_index = self.source_map_source_index;
                mapped_line_char_cache = self.source_map_line_char_cache.take();
                self.source_map_source = saved_source;
                self.source_map_source_index = saved_source_index;
                self.source_map_source_is_json = saved_source_is_json;
                self.source_map_line_char_cache = saved_line_char_cache;
            }
        }

        let (source_line, source_character) = mapped_line_char_cache
            .as_mut()
            .or(self.source_map_line_char_cache.as_mut())
            .expect("source map line character cache")
            .get_line_and_character(pos);
        let (line, column) = {
            let writer = self.writer_p5();
            (writer.get_line(), writer.get_column())
        };
        // PERF: borrow the generator; no `Rc` clone for each position.
        let generator = self
            .source_map_generator
            .as_ref()
            .expect("source map generator");
        if let Err(err) = generator.borrow_mut().add_source_mapping(
            line,
            column,
            source_index,
            source_line,
            source_character,
        ) {
            panic!("{err}");
        }
    }

    // Go: printer/printer.go:5904 emitSourcePos
    pub(crate) fn emit_source_pos(&mut self, source: SourceMapSource, pos: i32) {
        if source != self.source_map_source {
            let saved_source_map_source = self.source_map_source.clone();
            let saved_source_map_source_index = self.source_map_source_index;
            // PORT: `take` saves the Go pointer; it is restored below and
            // nothing reads it in between while source maps are disabled.
            let saved_source_map_line_char_cache = self.source_map_line_char_cache.take();
            self.set_source_map_source(source);
            self.emit_pos(pos);
            self.source_map_source = saved_source_map_source;
            self.source_map_source_index = saved_source_map_source_index;
            self.source_map_line_char_cache = saved_source_map_line_char_cache;
        } else {
            self.emit_pos(pos);
        }
    }

    // Go: printer/printer.go:5933 emitSourceMapsBeforeNode
    pub(crate) fn emit_source_maps_before_node(&mut self, node: Node) -> Option<SourceMapState> {
        if !self.should_emit_source_maps(node) {
            return None;
        }

        let emit_flags = self.emit_context.emit_flags(node);
        let loc = self.emit_context.source_map_range(node);

        if !is_not_emitted_statement(node)
            && !emit_flags.intersects(EmitFlags::NO_LEADING_SOURCE_MAP)
            && self.current_source_file.is_some()
            && !position_is_synthesized(loc.pos())
        {
            // PERF: nested nodes often start at the same pos (see `SkipTriviaMemo`).
            let pos = self
                .skip_trivia_memo
                .skip_trivia(self.current_source_file, loc.pos());
            self.emit_source_pos(self.source_map_source.clone(), pos);
        }

        if emit_flags.intersects(EmitFlags::NO_NESTED_SOURCE_MAPS) {
            self.source_maps_disabled = true;
        }

        Some(SourceMapState {
            emit_flags,
            source_map_range: loc,
            has_token_source_map_range: false,
        })
    }

    // Go: printer/printer.go:5957 emitSourceMapsAfterNode
    pub(crate) fn emit_source_maps_after_node(
        &mut self,
        node: Node,
        previous_state: Option<SourceMapState>,
    ) {
        let Some(previous_state) = previous_state else {
            return;
        };

        let emit_flags = previous_state.emit_flags;
        let loc = previous_state.source_map_range;

        if emit_flags.intersects(EmitFlags::NO_NESTED_SOURCE_MAPS) {
            self.source_maps_disabled = false;
        }

        if !is_not_emitted_statement(node)
            && !emit_flags.intersects(EmitFlags::NO_TRAILING_SOURCE_MAP)
            && !position_is_synthesized(loc.end())
        {
            self.emit_source_pos(self.source_map_source.clone(), loc.end());
        }
    }

    // Go: printer/printer.go:5976 emitSourceMapsBeforeToken
    pub(crate) fn emit_source_maps_before_token(
        &mut self,
        token: SyntaxKind,
        mut pos: i32,
        context_node: Node,
        flags: TokenEmitFlags,
    ) -> Option<SourceMapState> {
        if !self.should_emit_token_source_maps(token, pos, context_node, flags) {
            return None;
        }

        let emit_flags = self.emit_context.emit_flags(context_node);
        let (loc, has_loc) = self
            .emit_context
            .token_source_map_range(context_node, token);
        if has_loc {
            pos = loc.pos();
        }
        if pos >= 0 && self.current_source_file.is_some() {
            pos = skip_trivia(&self.current_source_file_text(), pos);
        }
        if !emit_flags.intersects(EmitFlags::NO_TOKEN_LEADING_SOURCE_MAPS) && pos >= 0 {
            self.emit_source_pos(self.source_map_source.clone(), pos);
        }

        Some(SourceMapState {
            emit_flags,
            source_map_range: loc,
            has_token_source_map_range: has_loc,
        })
    }

    // Go: printer/printer.go:5998 emitSourceMapsAfterToken
    pub(crate) fn emit_source_maps_after_token(
        &mut self,
        token: SyntaxKind,
        mut pos: i32,
        context_node: Node,
        previous_state: Option<SourceMapState>,
    ) {
        let Some(previous_state) = previous_state else {
            return;
        };

        let emit_flags = previous_state.emit_flags;
        let loc = previous_state.source_map_range;
        let has_loc = previous_state.has_token_source_map_range;
        if !emit_flags.intersects(EmitFlags::NO_TOKEN_TRAILING_SOURCE_MAPS) {
            if has_loc {
                pos = loc.end();
            }
            if pos >= 0 {
                self.emit_source_pos(self.source_map_source.clone(), pos);
            }
        }
    }

    //
    // Name Generation
    //

    // Go: printer/printer.go:6020 shouldReuseTempVariableScope
    pub(crate) fn should_reuse_temp_variable_scope(&self, node: Node) -> bool {
        node.is_some()
            && self
                .emit_context
                .emit_flags(node)
                .intersects(EmitFlags::REUSE_TEMP_VARIABLE_SCOPE)
    }

    // Go: printer/printer.go:6024 pushNameGenerationScope
    pub(crate) fn push_name_generation_scope(&mut self, node: Node) {
        let reuse = self.should_reuse_temp_variable_scope(node);
        self.name_generator.push_scope(reuse);
    }

    // Go: printer/printer.go:6028 popNameGenerationScope
    pub(crate) fn pop_name_generation_scope(&mut self, node: Node) {
        let reuse = self.should_reuse_temp_variable_scope(node);
        self.name_generator.pop_scope(reuse);
    }

    // Go: printer/printer.go:6032 generateAllNames
    pub(crate) fn generate_all_names(&mut self, nodes: NodeList) {
        if nodes.is_nil() {
            return;
        }
        for node in nodes.nodes() {
            self.generate_names(node);
        }
    }

    // Go: printer/printer.go:6041 generateNames
    pub(crate) fn generate_names(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }

        match node.kind() {
            SyntaxKind::Block | SyntaxKind::CaseClause | SyntaxKind::DefaultClause => {
                self.generate_all_names(node.statement_list());
            }
            SyntaxKind::LabeledStatement
            | SyntaxKind::WithStatement
            | SyntaxKind::DoStatement
            | SyntaxKind::WhileStatement => {
                self.generate_names(node.statement());
            }
            SyntaxKind::IfStatement => {
                self.generate_names(node.then_statement());
                self.generate_names(node.else_statement());
            }
            SyntaxKind::ForStatement | SyntaxKind::ForOfStatement | SyntaxKind::ForInStatement => {
                self.generate_names(node.initializer());
                self.generate_names(node.statement());
            }
            SyntaxKind::SwitchStatement => {
                self.generate_names(node.case_block());
            }
            SyntaxKind::CaseBlock => {
                self.generate_all_names(node.clauses());
            }
            SyntaxKind::TryStatement => {
                self.generate_names(node.try_block());
                self.generate_names(node.catch_clause());
                self.generate_names(node.finally_block());
            }
            SyntaxKind::CatchClause => {
                self.generate_names(node.variable_declaration());
                self.generate_names(node.block());
            }
            SyntaxKind::VariableStatement => {
                self.generate_names(node.declaration_list());
            }
            SyntaxKind::VariableDeclarationList => {
                self.generate_all_names(node.declarations());
            }
            SyntaxKind::VariableDeclaration
            | SyntaxKind::Parameter
            | SyntaxKind::BindingElement
            | SyntaxKind::ClassDeclaration => {
                self.generate_name_if_needed(node.name());
            }
            SyntaxKind::FunctionDeclaration => {
                self.generate_name_if_needed(node.name());
                if self.should_reuse_temp_variable_scope(node) {
                    self.generate_all_names(node.parameter_list());
                    self.generate_names(node.body());
                }
            }
            SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern => {
                self.generate_all_names(node.element_list());
            }
            SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
                self.generate_names(node.import_clause());
            }
            SyntaxKind::ImportClause => {
                self.generate_name_if_needed(node.name());
                self.generate_names(node.named_bindings());
            }
            SyntaxKind::NamespaceImport | SyntaxKind::NamespaceExport => {
                self.generate_name_if_needed(node.name());
            }
            SyntaxKind::NamedImports => {
                self.generate_all_names(node.element_list());
            }
            SyntaxKind::ImportSpecifier => {
                let property_name = node.property_name();
                if property_name.is_some() {
                    self.generate_name_if_needed(property_name);
                } else {
                    self.generate_name_if_needed(node.name());
                }
            }
            _ => {}
        }
    }

    // Go: printer/printer.go:6101 generateAllMemberNames
    pub(crate) fn generate_all_member_names(&mut self, nodes: NodeList) {
        if nodes.is_nil() {
            return;
        }
        // PERF: printfast1. With no auto-generate record, every
        // `generateName` of this walk only reads the text of a name and
        // drops it (see `generate_name`), so the walk changes nothing. The
        // walk adds no record. Type text (`typeToString`) has none.
        if !self.emit_context.has_any_auto_generate_info() {
            return;
        }
        for node in nodes.nodes() {
            self.generate_member_names(node);
        }
    }

    // Go: printer/printer.go:6110 generateMemberNames
    pub(crate) fn generate_member_names(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }
        match node.kind() {
            SyntaxKind::PropertyAssignment
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor => {
                self.generate_name_if_needed(node.name());
            }
            _ => {}
        }
    }

    // Go: printer/printer.go:6127 generateNameIfNeeded
    pub(crate) fn generate_name_if_needed(&mut self, name: Node) {
        if name.is_some() {
            if is_member_name(name) {
                self.generate_name(name);
            } else if is_binding_pattern(name) {
                self.generate_names(name);
            }
        }
    }

    // Generate the text for a generated identifier or private identifier
    // Go: printer/printer.go:6138 generateName
    pub(crate) fn generate_name(&mut self, name: Node) {
        // PERF: printfast1. Go drops the result. For a name with no
        // auto-generate record, Go `GenerateName` (namegenerator.go:114)
        // only returns `GetTextOfNode(name)`, which changes nothing, so the
        // port does not build that string.
        if !self.emit_context.has_auto_generate_info(name) {
            return;
        }
        let _ = self.sync_name_generator().generate_name(name);
    }

    // Returns a value indicating whether a name is unique globally or within the current file.
    // Go: printer/printer.go:6143 isFileLevelUniqueNameInCurrentFile
    pub(crate) fn is_file_level_unique_name_in_current_file(
        &self,
        name: &str,
        _private_name: bool,
    ) -> bool {
        if self.current_source_file.is_some() {
            self.emit_context.is_file_level_unique_name(
                self.current_source_file,
                name,
                self.print_handlers.has_global_name.as_deref(),
            )
        } else {
            true
        }
    }

    //
    // Scoped operations
    //

    // Go: printer/printer.go:6155 enterNode
    pub(crate) fn enter_node(&mut self, node: Node) -> PrinterState {
        let mut state = PrinterState::default();

        if let Some(f) = &self.print_handlers.on_before_emit_node {
            f(node);
        }

        state.comment_state = self.emit_comments_before_node(node);
        state.source_map_state = self.emit_source_maps_before_node(node);
        state
    }

    // Go: printer/printer.go:6167 exitNode
    pub(crate) fn exit_node(&mut self, node: Node, previous_state: PrinterState) {
        self.emit_source_maps_after_node(node, previous_state.source_map_state);
        self.emit_comments_after_node(node, previous_state.comment_state);

        if let Some(f) = &self.print_handlers.on_after_emit_node {
            f(node);
        }
    }

    // Go: printer/printer.go:6176 enterTokenNode
    pub(crate) fn enter_token_node(&mut self, node: Node, flags: TokenEmitFlags) -> PrinterState {
        let mut state = PrinterState::default();

        if let Some(f) = &self.print_handlers.on_before_emit_token {
            f(node);
        }

        if !flags.intersects(TokenEmitFlags::NO_COMMENTS) {
            state.comment_state = self.emit_comments_before_node(node);
        }
        if !flags.intersects(TokenEmitFlags::NO_SOURCE_MAPS) {
            state.source_map_state = self.emit_source_maps_before_node(node);
        }
        state
    }

    // Go: printer/printer.go:6192 exitTokenNode
    pub(crate) fn exit_token_node(&mut self, node: Node, previous_state: PrinterState) {
        self.emit_source_maps_after_node(node, previous_state.source_map_state);
        self.emit_comments_after_node(node, previous_state.comment_state);

        if let Some(f) = &self.print_handlers.on_after_emit_token {
            f(node);
        }
    }

    // Go: printer/printer.go:6211 enterToken
    pub(crate) fn enter_token(
        &mut self,
        token: SyntaxKind,
        pos: i32,
        context_node: Node,
        flags: TokenEmitFlags,
    ) -> (PrinterState, i32) {
        let mut state = PrinterState::default();
        let (comment_state, pos) = self.emit_comments_before_token(token, pos, context_node, flags);
        state.comment_state = comment_state;
        state.source_map_state =
            self.emit_source_maps_before_token(token, pos, context_node, flags);
        (state, pos)
    }

    // Go: printer/printer.go:6218 exitToken
    pub(crate) fn exit_token(
        &mut self,
        token: SyntaxKind,
        pos: i32,
        context_node: Node,
        previous_state: PrinterState,
    ) {
        self.emit_source_maps_after_token(
            token,
            pos,
            context_node,
            previous_state.source_map_state,
        );
        self.emit_comments_after_token(token, pos, context_node, previous_state.comment_state);
    }
}

// Go: printer/printer.go:5515 formatSynthesizedComment
pub(crate) fn format_synthesized_comment(comment: &SynthesizedComment) -> String {
    if comment.kind == SyntaxKind::MultiLineCommentTrivia {
        return format!("/*{}*/", comment.text);
    }
    format!("//{}", comment.text)
}

// Go: printer/printer.go:5757 commentSeparator
go_enum!(CommentSeparator, u32 {
    NONE = 0; // commentSeparatorNone
    BEFORE = 1; // commentSeparatorBefore
    AFTER = 2; // commentSeparatorAfter
});

// Go: printer/printer.go:6201 tokenEmitFlags
go_flags!(TokenEmitFlags, u32 {
    NO_COMMENTS = 1 << 0; // tefNoComments
    INDENT_LEADING_COMMENTS = 1 << 1; // tefIndentLeadingComments
    NO_SOURCE_MAPS = 1 << 2; // tefNoSourceMaps

    NONE = 0; // tefNone
});

// Go: printer/printer.go:6223 ListFormat
go_flags!(ListFormat, i32 {
    NONE = 0; // LFNone

    // Line separators
    SINGLE_LINE = 0; // Prints the list on a single line (default).
    MULTI_LINE = 1 << 0; // Prints the list on multiple lines.
    PRESERVE_LINES = 1 << 1; // Prints the list using line preservation if possible.
    LINES_MASK = ListFormat::SINGLE_LINE.0 | ListFormat::MULTI_LINE.0 | ListFormat::PRESERVE_LINES.0;

    // Delimiters
    NOT_DELIMITED = 0; // There is no delimiter between list items (default).
    BAR_DELIMITED = 1 << 2; // Each list item is space-and-bar (" |") delimited.
    AMPERSAND_DELIMITED = 1 << 3; // Each list item is space-and-ampersand (" &") delimited.
    COMMA_DELIMITED = 1 << 4; // Each list item is comma (",") delimited.
    ASTERISK_DELIMITED = 1 << 5; // Each list item is asterisk ("\n *") delimited, used with JSDoc.
    DELIMITERS_MASK = ListFormat::BAR_DELIMITED.0 | ListFormat::AMPERSAND_DELIMITED.0 | ListFormat::COMMA_DELIMITED.0 | ListFormat::ASTERISK_DELIMITED.0;

    ALLOW_TRAILING_COMMA = 1 << 6; // Write a trailing comma (",") if present.

    // Whitespace
    INDENTED = 1 << 7; // The list should be indented.
    SPACE_BETWEEN_BRACES = 1 << 8; // Inserts a space after the opening brace and before the closing brace.
    SPACE_BETWEEN_SIBLINGS = 1 << 9; // Inserts a space between each sibling node.

    // Brackets/Braces
    BRACES = 1 << 10; // The list is surrounded by "{" and "}".
    PARENTHESIS = 1 << 11; // The list is surrounded by "(" and ")".
    ANGLE_BRACKETS = 1 << 12; // The list is surrounded by "<" and ">".
    SQUARE_BRACKETS = 1 << 13; // The list is surrounded by "[" and "]".
    BRACKETS_MASK = ListFormat::BRACES.0 | ListFormat::PARENTHESIS.0 | ListFormat::ANGLE_BRACKETS.0 | ListFormat::SQUARE_BRACKETS.0;

    OPTIONAL_IF_NIL = 1 << 14; // Do not emit brackets if the list is nil.
    OPTIONAL_IF_EMPTY = 1 << 15; // Do not emit brackets if the list is empty.
    OPTIONAL = ListFormat::OPTIONAL_IF_NIL.0 | ListFormat::OPTIONAL_IF_EMPTY.0;

    // Other
    PREFER_NEW_LINE = 1 << 16; // Prefer adding a LineTerminator between synthesized nodes.
    NO_TRAILING_NEW_LINE = 1 << 17; // Do not emit a trailing NewLine for a MultiLine list.
    NO_INTERVENING_COMMENTS = 1 << 18; // Do not emit comments between each node
    NO_SPACE_IF_EMPTY = 1 << 19; // If the literal is empty, do not add spaces between braces.
    SINGLE_ELEMENT = 1 << 20;
    SPACE_AFTER_LIST = 1 << 21; // Add space after list

    // Precomputed Formats
    MODIFIERS = ListFormat::SINGLE_LINE.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::NO_INTERVENING_COMMENTS.0 | ListFormat::SPACE_AFTER_LIST.0;
    HERITAGE_CLAUSES = ListFormat::SINGLE_LINE.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0;
    SINGLE_LINE_TYPE_LITERAL_MEMBERS = ListFormat::SINGLE_LINE.0 | ListFormat::SPACE_BETWEEN_BRACES.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0;
    MULTI_LINE_TYPE_LITERAL_MEMBERS = ListFormat::MULTI_LINE.0 | ListFormat::INDENTED.0 | ListFormat::OPTIONAL_IF_EMPTY.0;

    SINGLE_LINE_TUPLE_TYPE_ELEMENTS = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0;
    MULTI_LINE_TUPLE_TYPE_ELEMENTS = ListFormat::COMMA_DELIMITED.0 | ListFormat::INDENTED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::MULTI_LINE.0;
    UNION_TYPE_CONSTITUENTS = ListFormat::BAR_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0;
    INTERSECTION_TYPE_CONSTITUENTS = ListFormat::AMPERSAND_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0;
    OBJECT_BINDING_PATTERN_ELEMENTS = ListFormat::SINGLE_LINE.0 | ListFormat::ALLOW_TRAILING_COMMA.0 | ListFormat::SPACE_BETWEEN_BRACES.0 | ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::NO_SPACE_IF_EMPTY.0;
    ARRAY_BINDING_PATTERN_ELEMENTS = ListFormat::SINGLE_LINE.0 | ListFormat::ALLOW_TRAILING_COMMA.0 | ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::NO_SPACE_IF_EMPTY.0;
    OBJECT_LITERAL_EXPRESSION_PROPERTIES = ListFormat::PRESERVE_LINES.0 | ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SPACE_BETWEEN_BRACES.0 | ListFormat::INDENTED.0 | ListFormat::BRACES.0 | ListFormat::NO_SPACE_IF_EMPTY.0;
    IMPORT_ATTRIBUTES = ListFormat::PRESERVE_LINES.0 | ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SPACE_BETWEEN_BRACES.0 | ListFormat::INDENTED.0 | ListFormat::BRACES.0 | ListFormat::NO_SPACE_IF_EMPTY.0;
    ARRAY_LITERAL_EXPRESSION_ELEMENTS = ListFormat::PRESERVE_LINES.0 | ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::ALLOW_TRAILING_COMMA.0 | ListFormat::INDENTED.0 | ListFormat::SQUARE_BRACKETS.0;
    COMMA_LIST_ELEMENTS = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0;
    CALL_EXPRESSION_ARGUMENTS = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0 | ListFormat::PARENTHESIS.0;
    NEW_EXPRESSION_ARGUMENTS = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0 | ListFormat::PARENTHESIS.0 | ListFormat::OPTIONAL_IF_NIL.0;
    TEMPLATE_EXPRESSION_SPANS = ListFormat::SINGLE_LINE.0 | ListFormat::NO_INTERVENING_COMMENTS.0;
    SINGLE_LINE_BLOCK_STATEMENTS = ListFormat::SPACE_BETWEEN_BRACES.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0;
    MULTI_LINE_BLOCK_STATEMENTS = ListFormat::INDENTED.0 | ListFormat::MULTI_LINE.0;
    VARIABLE_DECLARATION_LIST = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0;
    SINGLE_LINE_FUNCTION_BODY_STATEMENTS = ListFormat::SINGLE_LINE.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SPACE_BETWEEN_BRACES.0;
    MULTI_LINE_FUNCTION_BODY_STATEMENTS = ListFormat::MULTI_LINE.0;
    CLASS_HERITAGE_CLAUSES = ListFormat::SINGLE_LINE.0;
    CLASS_MEMBERS = ListFormat::INDENTED.0 | ListFormat::MULTI_LINE.0;
    INTERFACE_MEMBERS = ListFormat::INDENTED.0 | ListFormat::MULTI_LINE.0;
    ENUM_MEMBERS = ListFormat::COMMA_DELIMITED.0 | ListFormat::INDENTED.0 | ListFormat::MULTI_LINE.0;
    CASE_BLOCK_CLAUSES = ListFormat::INDENTED.0 | ListFormat::MULTI_LINE.0;
    NAMED_IMPORTS_OR_EXPORTS_ELEMENTS = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::ALLOW_TRAILING_COMMA.0 | ListFormat::SINGLE_LINE.0 | ListFormat::SPACE_BETWEEN_BRACES.0 | ListFormat::NO_SPACE_IF_EMPTY.0;
    JSX_ELEMENT_OR_FRAGMENT_CHILDREN = ListFormat::SINGLE_LINE.0 | ListFormat::NO_INTERVENING_COMMENTS.0;
    JSX_ELEMENT_ATTRIBUTES = ListFormat::SINGLE_LINE.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::NO_INTERVENING_COMMENTS.0;
    CASE_OR_DEFAULT_CLAUSE_STATEMENTS = ListFormat::INDENTED.0 | ListFormat::MULTI_LINE.0 | ListFormat::NO_TRAILING_NEW_LINE.0 | ListFormat::OPTIONAL_IF_EMPTY.0;
    HERITAGE_CLAUSE_TYPES = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0;
    SOURCE_FILE_STATEMENTS = ListFormat::MULTI_LINE.0 | ListFormat::NO_TRAILING_NEW_LINE.0;
    DECORATORS = ListFormat::MULTI_LINE.0 | ListFormat::OPTIONAL.0 | ListFormat::SPACE_AFTER_LIST.0;
    TYPE_ARGUMENTS = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0 | ListFormat::ANGLE_BRACKETS.0 | ListFormat::OPTIONAL.0;
    TYPE_PARAMETERS = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0 | ListFormat::ANGLE_BRACKETS.0 | ListFormat::OPTIONAL.0;
    PARAMETERS = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0 | ListFormat::PARENTHESIS.0;
    SINGLE_ARROW_PARAMETER = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0;
    INDEX_SIGNATURE_PARAMETERS = ListFormat::COMMA_DELIMITED.0 | ListFormat::SPACE_BETWEEN_SIBLINGS.0 | ListFormat::SINGLE_LINE.0 | ListFormat::INDENTED.0 | ListFormat::SQUARE_BRACKETS.0;
    JS_DOC_COMMENT = ListFormat::MULTI_LINE.0 | ListFormat::ASTERISK_DELIMITED.0;
    IMPORT_CLAUSE_ENTRIES = ListFormat::IMPORT_ATTRIBUTES.0; // Deprecated: Use LFImportAttributes
});

// Go: printer/printer.go:6313 getOpeningBracket
pub(crate) fn get_opening_bracket(format: ListFormat) -> &'static str {
    let brackets = format & ListFormat::BRACKETS_MASK;
    if brackets == ListFormat::BRACES {
        "{"
    } else if brackets == ListFormat::PARENTHESIS {
        "("
    } else if brackets == ListFormat::ANGLE_BRACKETS {
        "<"
    } else if brackets == ListFormat::SQUARE_BRACKETS {
        "["
    } else {
        panic!("Unexpected bracket: {:?}", brackets)
    }
}

// Go: printer/printer.go:6328 getClosingBracket
pub(crate) fn get_closing_bracket(format: ListFormat) -> &'static str {
    let brackets = format & ListFormat::BRACKETS_MASK;
    if brackets == ListFormat::BRACES {
        "}"
    } else if brackets == ListFormat::PARENTHESIS {
        ")"
    } else if brackets == ListFormat::ANGLE_BRACKETS {
        ">"
    } else if brackets == ListFormat::SQUARE_BRACKETS {
        "]"
    } else {
        panic!("Unexpected bracket: {:?}", brackets)
    }
}
