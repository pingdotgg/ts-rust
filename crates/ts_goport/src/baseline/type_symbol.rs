//! Port of `internal/testutil/tsbaseline/type_symbol_baseline.go`: the
//! harness `.types` and `.symbols` writers.
//!
//! PORT: the `testing` wrappers (`DoTypeAndSymbolBaseline`, `checkBaselines`,
//! `isTypeBaselineNodeReuseLine` and the diff fixup) are left out. Callers
//! use `generate_baseline` directly, like the Go dumper `cmd/typesymdump`.

use std::any::Any;
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::baseline::util::{
    is_default_library_file, remove_line_delimiters, remove_test_path_prefixes,
};
use crate::frontend::tspath::path::{get_base_file_name, get_normalized_absolute_path};
use crate::prelude::*;

/// Go `baseline.NoContent`.
pub const NO_CONTENT: &str = "<no content>";

/// Go `harnessutil.TestFile`.
#[derive(Clone, Debug)]
pub struct TestFile {
    pub unit_name: String,
    pub content: String,
}

/// Go `codeLinesRegexp.Split(s, -1)` with `[\r  ]|\r?\n`. RE2 is
/// leftmost-first, so at `\r\n` the first branch takes `\r` alone and the
/// `\n` splits again: every single `\r`, `\n`, U+2028 and U+2029 separates.
pub fn split_code_lines(text: &str) -> Vec<&str> {
    text.split(['\r', '\n', '\u{2028}', '\u{2029}']).collect()
}

/// Go `bracketLineRegex.MatchString` with `^\s*[{|}]\s*$`. RE2 `\s` is
/// `[\t\n\f\r ]`.
pub fn is_bracket_line(line: &str) -> bool {
    let rest = line.trim_matches(['\t', '\n', '\x0C', '\r', ' ']);
    matches!(rest, "{" | "|" | "}")
}

/// The Go condition that skips the blank line before the next group:
/// the next line is a bracket line or is empty after `strings.TrimSpace`.
/// Rust `trim` uses Unicode White_Space, the same set as Go `unicode.IsSpace`.
fn next_line_needs_no_blank(code_lines: &[&str], next: usize) -> bool {
    next < code_lines.len()
        && (is_bracket_line(code_lines[next]) || code_lines[next].trim().is_empty())
}

// Go: type_symbol_baseline.go:144 generateBaseline
pub fn generate_baseline(
    all_files: &[TestFile],
    full_walker: &mut TypeWriterWalker,
    header: &str,
    is_symbol_baseline: bool,
) -> String {
    let mut result = String::new();
    // !!! Perf baseline
    let perf_lines: Vec<String> = Vec::new();
    let baselines = iterate_baseline(all_files, full_walker, is_symbol_baseline);
    for value in &baselines {
        result.push_str(value);
    }
    // PORT: the commented-out Go perf stats block is not ported; perfLines
    // stays empty.
    if !result.is_empty() {
        return format!(
            "//// [{header}] ////\r\n\r\n{}{result}",
            perf_lines.join("\n")
        );
    }
    NO_CONTENT.to_string()
}

// Go: type_symbol_baseline.go:196 iterateBaseline
pub fn iterate_baseline(
    all_files: &[TestFile],
    full_walker: &mut TypeWriterWalker,
    is_symbol_baseline: bool,
) -> Vec<String> {
    let mut baselines = Vec::new();

    for file in all_files {
        let unit_name = &file.unit_name;
        let mut type_lines = String::new();
        type_lines.push_str("=== ");
        type_lines.push_str(unit_name);
        type_lines.push_str(" ===\r\n");
        let code_lines = split_code_lines(&file.content);
        let results = if is_symbol_baseline {
            full_walker.get_symbols(unit_name)
        } else {
            full_walker.get_types(unit_name)
        };
        let mut last_index_written: i64 = -1;
        for result in &results {
            if is_symbol_baseline && result.symbol.is_empty() {
                return baselines;
            }
            let line = result.line as usize;
            if last_index_written == -1 {
                type_lines.push_str(&code_lines[..=line].join("\r\n"));
                type_lines.push_str("\r\n");
            } else if last_index_written != line as i64 {
                let next = (last_index_written + 1) as usize;
                if !next_line_needs_no_blank(&code_lines, next) {
                    type_lines.push_str("\r\n");
                }
                type_lines.push_str(&code_lines[next..=line].join("\r\n"));
                type_lines.push_str("\r\n");
            }
            last_index_written = line as i64;
            let type_or_symbol_string = if is_symbol_baseline {
                &result.symbol
            } else {
                &result.typ
            };
            let line_text = remove_line_delimiters(&result.source_text);
            type_lines.push('>');
            let _ = write!(type_lines, "{line_text} : {type_or_symbol_string}");
            type_lines.push_str("\r\n");
            if !result.underline.is_empty() {
                type_lines.push('>');
                for _ in 0..line_text.len() {
                    type_lines.push(' ');
                }
                type_lines.push_str(" : ");
                type_lines.push_str(&result.underline);
                type_lines.push_str("\r\n");
            }
        }

        let next = (last_index_written + 1) as usize;
        if next < code_lines.len() {
            if !next_line_needs_no_blank(&code_lines, next) {
                type_lines.push_str("\r\n");
            }
            type_lines.push_str(&code_lines[next..].join("\r\n"));
        }
        type_lines.push_str("\r\n");

        baselines.push(remove_test_path_prefixes(
            &type_lines,
            false, /*retainTrailingDirectorySeparator*/
        ));
    }

    baselines
}

// Go: type_symbol_baseline.go:263 typeWriterWalker
// PORT: `program` is the thread-local loaded program, so it is not a field.
// `declarationTextCache` is only read in this pin (never written), so it is
// left out; its lookup always misses.
pub struct TypeWriterWalker {
    pub had_error_baseline: bool,
    pub current_source_file: Node,
    /// Rust-only: when true, a panic in one node's checker work becomes a
    /// `<<goport panic: MESSAGE>>` result instead of unwinding. The Go
    /// harness has no such guard (a Go panic fails the test).
    pub catch_panics: bool,
    /// Rust-only: the number of nodes that panicked under `catch_panics`.
    pub panic_count: usize,
}

// Go: type_symbol_baseline.go:270 newTypeWriterWalker
pub fn new_type_writer_walker(had_error_baseline: bool) -> TypeWriterWalker {
    TypeWriterWalker {
        had_error_baseline,
        current_source_file: Node::NIL,
        catch_panics: false,
        panic_count: 0,
    }
}

// Go: type_symbol_baseline.go:284 typeWriterResult
#[derive(Clone, Debug, Default)]
pub struct TypeWriterResult {
    pub line: i32,
    pub source_text: String,
    pub symbol: String,
    pub typ: String,
    pub underline: String, // !!!
}

const UNPORTED_PREFIX: &str = "unported Go code";

fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        String::new()
    }
}

/// Go `tspath.ToRootedFilePath(filename, walker.program.Program().BaseDirectory())`
/// (ts#64159, type_symbol_baseline.go:293): the name of `getTypes` and
/// `getSymbols` is rooted against the program's base directory.
// PORT: Go `ToRootedFilePath` is `get_normalized_absolute_path` here; the
// names that callers give are absolute or relative, never empty. A process
// without a frontend program looks the name up as given.
fn program_file_name(filename: &str) -> String {
    match crate::program::go_frontend_program() {
        Some(program) => get_normalized_absolute_path(filename, &program.base_directory()),
        None => filename.to_string(),
    }
}

impl TypeWriterWalker {
    // Go: type_symbol_baseline.go:278 getTypeCheckerForCurrentFile
    // PORT: Go returns the checker and a release func. The Rust pool lends
    // the same checker (`GetTypeCheckerForFile`) to a closure on the
    // checker's own thread, so the closure and its result must be `Send`.
    fn with_type_checker_for_current_file<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Checker) -> R + Send + 'static,
    ) -> R {
        with_type_checker_for_file(self.current_source_file, f)
    }

    // Go: type_symbol_baseline.go:292 getTypes
    pub fn get_types(&mut self, filename: &str) -> Vec<TypeWriterResult> {
        let source_file = get_source_file(&program_file_name(filename));
        self.current_source_file = source_file;
        self.visit_node(source_file, false /*isSymbolWalk*/)
    }

    // Go: type_symbol_baseline.go:298 getSymbols
    pub fn get_symbols(&mut self, filename: &str) -> Vec<TypeWriterResult> {
        let source_file = get_source_file(&program_file_name(filename));
        self.current_source_file = source_file;
        self.visit_node(source_file, true /*isSymbolWalk*/)
    }

    // Go: type_symbol_baseline.go:304 visitNode
    pub fn visit_node(&mut self, node: Node, is_symbol_walk: bool) -> Vec<TypeWriterResult> {
        let nodes = for_each_ast_node(node);
        let mut results = Vec::new();
        for n in nodes {
            // tsgo#4797: a heritage clause type reference name (`A.B` in
            // `interface I extends A.B`) is a QualifiedName, not an
            // expression. The symbol walk writes each such name. The type
            // walk writes only the inner ones (parent is a QualifiedName).
            if is_expression_node(n)
                || n.kind() == SyntaxKind::Identifier
                || is_declaration_name(n)
                || (is_qualified_name(n)
                    && is_name_of_heritage_clause_type_reference(n)
                    && (is_symbol_walk || is_qualified_name(n.parent())))
            {
                let result = if self.catch_panics {
                    self.write_type_or_symbol_guarded(n, is_symbol_walk)
                } else {
                    self.write_type_or_symbol(n, is_symbol_walk)
                };
                if let Some(result) = result {
                    results.push(result);
                }
            }
        }
        results
    }

    /// Rust-only: `write_type_or_symbol` under `catch_unwind`. The checker is
    /// kept after a panic, because a new checker would change the type ids
    /// of every later node. Panics that are not unported hits are recorded
    /// as `panic` in the unported report. A Go panic (`go_panic`) goes on,
    /// as Go has no recover here.
    fn write_type_or_symbol_guarded(
        &mut self,
        node: Node,
        is_symbol_walk: bool,
    ) -> Option<TypeWriterResult> {
        match catch_unwind(AssertUnwindSafe(|| {
            self.write_type_or_symbol(node, is_symbol_walk)
        })) {
            Ok(result) => result,
            Err(payload) => {
                let payload = resume_go_panic(payload);
                let message = payload_message(payload.as_ref());
                if !message.starts_with(UNPORTED_PREFIX) {
                    record_unported("panic");
                }
                self.panic_count += 1;
                let file = self.current_source_file;
                let (line, source_text) = catch_unwind(AssertUnwindSafe(|| {
                    let actual_pos = skip_trivia(&source_file_text(file), node.pos());
                    (
                        get_ecma_line_of_position(file, actual_pos),
                        get_source_text_of_node_from_source_file(file, node, false),
                    )
                }))
                .ok()?;
                let text = format!("<<goport panic: {}>>", remove_line_delimiters(&message));
                Some(TypeWriterResult {
                    line,
                    source_text,
                    symbol: text.clone(),
                    typ: text,
                    underline: String::new(),
                })
            }
        }
    }

    // Go: type_symbol_baseline.go:346 writeTypeOrSymbol
    pub fn write_type_or_symbol(
        &mut self,
        node: Node,
        is_symbol_walk: bool,
    ) -> Option<TypeWriterResult> {
        let current_source_file = self.current_source_file;
        let actual_pos = skip_trivia(&source_file_text(current_source_file), node.pos());
        let line = get_ecma_line_of_position(current_source_file, actual_pos);
        let source_text = get_source_text_of_node_from_source_file(
            current_source_file,
            node,
            false, /*includeTrivia*/
        );
        let had_error_baseline = self.had_error_baseline;

        self.with_type_checker_for_current_file(move |file_checker| {
            let (ctx, put_ctx) = get_emit_context();
            let result = write_type_or_symbol_with_checker(
                file_checker,
                &ctx,
                current_source_file,
                had_error_baseline,
                node,
                is_symbol_walk,
                line,
                source_text,
            );
            // Go: `defer putCtx()`.
            put_ctx();
            result
        })
    }
}

/// The body of Go `writeTypeOrSymbol` after the checker and emit context
/// are acquired.
#[allow(clippy::too_many_arguments)]
fn write_type_or_symbol_with_checker(
    file_checker: &mut Checker,
    ctx: &Rc<EmitContext>,
    current_source_file: Node,
    had_error_baseline: bool,
    node: Node,
    is_symbol_walk: bool,
    line: i32,
    source_text: String,
) -> Option<TypeWriterResult> {
    if !is_symbol_walk {
        // Don't try to get the type of something that's already a type.
        // Exception for `T` in `type T = something` because that may evaluate to some interesting type.
        if is_part_of_type_node(node)
            || (node.kind() == SyntaxKind::AsExpression
                || node.kind() == SyntaxKind::SatisfiesExpression)
                && node.type_().flags().intersects(NodeFlags::REPARSED)
            || is_identifier(node)
                && !get_meaning_from_declaration(node.parent()).intersects(SemanticMeaning::VALUE)
                && !(is_type_or_js_type_alias_declaration(node.parent())
                    && node == node.parent().name())
        {
            return None;
        }

        if is_omitted_expression(node) {
            return None;
        }

        let mut t = TypeId::NIL;
        // Workaround to ensure we output 'C' instead of 'typeof C' for base class expressions
        if is_expression_with_type_arguments_in_class_extends_clause(node.parent()) {
            t = file_checker.get_type_at_location(node.parent());
        }
        if t.is_nil() || file_checker.is_type_any(t) {
            t = file_checker.get_type_at_location(node);
        }
        let type_string;
        if !had_error_baseline
            && file_checker.is_type_any(t)
            && !is_binding_element(node.parent())
            && !is_property_access_or_qualified_name(node.parent())
            && !is_label_name(node)
            && !is_global_scope_augmentation(node.parent())
            && !is_meta_property(node.parent())
            && !is_import_statement_name(node)
            && !is_export_statement_name(node)
            && !is_intrinsic_jsx_tag(node, current_source_file)
        {
            type_string = file_checker
                .ty(t)
                .as_intrinsic_type()
                .intrinsic_name()
                .to_string();
        } else {
            ctx.reset();
            let builder = Rc::new(RefCell::new(new_node_builder(file_checker, Rc::clone(ctx))));
            let type_format_flags = TypeFormatFlags::NO_TRUNCATION
                | TypeFormatFlags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                | TypeFormatFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS;
            let mut type_node = file_checker.node_builder_type_to_type_node(
                &builder,
                t,
                node.parent(),
                to_node_builder_flags(type_format_flags) | NodeBuilderFlags::IGNORE_ERRORS,
                InternalNodeBuilderFlags::ALLOW_UNRESOLVED_NAMES,
                None,
            );
            if is_identifier(node)
                && is_type_alias_declaration(node.parent())
                && node.parent().name() == node
                && is_identifier(type_node)
                && type_node.text() == node.text()
            {
                // for a complex type alias `type T = ...`, showing "T : T" isn't very helpful for type tests. When the type produced is the same as
                // the name of the type alias, recreate the type string without reusing the alias name
                type_node = file_checker.node_builder_type_to_type_node(
                    &builder,
                    t,
                    node.parent(),
                    to_node_builder_flags(type_format_flags | TypeFormatFlags::IN_TYPE_ALIAS)
                        | NodeBuilderFlags::IGNORE_ERRORS,
                    InternalNodeBuilderFlags::ALLOW_UNRESOLVED_NAMES,
                    None,
                );
            }

            // !!! TODO: port underline printer, memoize
            let writer = Rc::new(RefCell::new(new_text_writer("", 0)));
            let mut printer = new_printer(
                PrinterOptions {
                    remove_comments: true,
                    ..Default::default()
                },
                PrintHandlers::default(),
                Some(Rc::clone(ctx)),
            );
            printer.write_exported(type_node, current_source_file, writer.clone(), None);
            type_string = writer.borrow().string();
        }
        return Some(TypeWriterResult {
            line,
            source_text,
            typ: type_string,
            // underline: underline, // !!! TODO: underline
            ..Default::default()
        });
    }

    let symbol = file_checker.get_symbol_at_location_exported(node);
    if symbol.is_nil() {
        return None;
    }

    let mut symbol_string = String::with_capacity(256);
    symbol_string.push_str("Symbol(");
    symbol_string.push_str(&escape_all_internal_symbol_names(
        &file_checker.symbol_to_string_ex(
            symbol,
            node.parent(),
            SymbolFlags::NONE,
            SymbolFormatFlags::ALLOW_ANY_NODE_KIND,
        ),
    ));
    let declarations = file_checker.sym(symbol).declarations.clone();
    let mut count = 0;
    for declaration in &declarations {
        if count >= 5 {
            let _ = write!(
                symbol_string,
                " ... and {} more",
                declarations.len() - count
            );
            break;
        }
        count += 1;
        symbol_string.push_str(", ");
        // PORT: Go reads `declarationTextCache` here, but never writes it, so
        // the lookup always misses and is left out.

        let decl_source_file = get_source_file_of_node(*declaration);
        let (decl_line, decl_char) =
            get_ecma_line_and_utf16_character_of_position(decl_source_file, declaration.pos());
        let file_name = get_base_file_name(source_file_file_name(decl_source_file));
        symbol_string.push_str("Decl(");
        symbol_string.push_str(&file_name);
        symbol_string.push_str(", ");
        if is_default_library_file(&file_name) {
            symbol_string.push_str("--, --)");
        } else {
            let _ = write!(symbol_string, "{decl_line}, {decl_char})");
        }
    }
    symbol_string.push(')');
    Some(TypeWriterResult {
        line,
        source_text,
        symbol: symbol_string,
        ..Default::default()
    })
}

// Go: type_symbol_baseline.go:319 forEachASTNode
pub fn for_each_ast_node(node: Node) -> Vec<Node> {
    let mut result = Vec::new();
    let mut work = vec![node];

    let mut res_children: Vec<Node> = Vec::new();

    while let Some(elem) = work.pop() {
        let reparsed = elem.flags().intersects(NodeFlags::REPARSED);
        if !reparsed
            || elem.kind() == SyntaxKind::AsExpression
            || elem.kind() == SyntaxKind::SatisfiesExpression
            || ((elem.parent().kind() == SyntaxKind::SatisfiesExpression
                || elem.parent().kind() == SyntaxKind::AsExpression)
                && elem == elem.parent().expression())
        {
            if !reparsed
                || elem.parent().kind() == SyntaxKind::AsExpression
                || elem.parent().kind() == SyntaxKind::SatisfiesExpression
            {
                result.push(elem);
            }
            elem.for_each_child(|child| {
                res_children.push(child);
                false
            });
            res_children.reverse();
            work.append(&mut res_children);
        }
    }
    result
}

// Go: type_symbol_baseline.go:459 isImportStatementName
pub fn is_import_statement_name(node: Node) -> bool {
    let parent = node.parent();
    if is_import_specifier(parent) && (node == parent.name() || node == parent.property_name()) {
        return true;
    }
    if is_import_clause(parent) && node == parent.name() {
        return true;
    }
    if is_import_equals_declaration(parent) && node == parent.name() {
        return true;
    }
    false
}

// Go: type_symbol_baseline.go:472 isExportStatementName
pub fn is_export_statement_name(node: Node) -> bool {
    let parent = node.parent();
    if is_export_assignment(parent) && node == parent.expression() {
        return true;
    }
    if is_export_specifier(parent) && (node == parent.name() || node == parent.property_name()) {
        return true;
    }
    false
}

// Go: type_symbol_baseline.go:482 isIntrinsicJsxTag
pub fn is_intrinsic_jsx_tag(node: Node, source_file: Node) -> bool {
    let parent = node.parent();
    if !(is_jsx_opening_element(parent)
        || is_jsx_closing_element(parent)
        || is_jsx_self_closing_element(parent))
    {
        return false;
    }
    if parent.tag_name() != node {
        return false;
    }
    let text =
        get_source_text_of_node_from_source_file(source_file, node, false /*includeTrivia*/);
    is_intrinsic_jsx_name(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_matchers() {
        // CRLF gives an extra empty line (RE2 leftmost-first takes `\r` alone).
        assert_eq!(
            split_code_lines("a\r\nb\nc\u{2028}d"),
            vec!["a", "", "b", "c", "d"]
        );
        assert_eq!(split_code_lines(""), vec![""]);
        // A lone `\r` stays in the line text.
        assert_eq!(remove_line_delimiters("x\r\ny\rz\n"), "xy\rz");
        assert!(is_bracket_line(" \t{ "));
        assert!(is_bracket_line("|"));
        assert!(!is_bracket_line("{}"));
        assert!(!is_bracket_line("\u{A0}}"));
        assert!(!is_bracket_line(""));
    }
}
