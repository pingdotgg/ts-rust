//! Port of internal/testutil/parsetestutil/parsetestutil.go.

use super::leak;
use ts_goport::ast::{NodeVisitorHooks, new_node_visitor, set_node_loc, source_file_diagnostics};
use ts_goport::frontend::core_ext::get_script_kind_from_file_name;
use ts_goport::frontend::parser::{ParsedSourceFile, SourceFileParseOptions, parse_source_file};
use ts_goport::frontend::tspath::Path;
use ts_goport::prelude::*;
use ts_goport::program::{note_parsed_source_file, publish_parsed_files};

// Go: testutil/parsetestutil/parsetestutil.go:15 ParseTypeScript
/// Simplifies parsing an input string into a SourceFile for testing purposes.
// PORT: the file is not published (see `parse_type_script_published`).
// Its parse diagnostics, tokens and syntax can be read.
pub(crate) fn parse_type_script(text: &str, jsx: bool) -> Node {
    parse_type_script_file(text, jsx).root
}

/// `parse_type_script` with the whole parse result (Go reads fields such as
/// `ExternalModuleIndicator` from the returned `*ast.SourceFile`).
pub(crate) fn parse_type_script_file(text: &str, jsx: bool) -> ParsedSourceFile {
    let file_name = if jsx { "/main.tsx" } else { "/main.ts" };
    parse_source_file(
        &SourceFileParseOptions {
            file_name: file_name.to_string(),
            path: Path(file_name.to_string()),
            ..Default::default()
        },
        leak(text),
        get_script_kind_from_file_name(file_name),
    )
}

/// Go `ParseTypeScript` for a file that a test prints, clones or
/// transforms.
// PORT: the printer and the node clone read the Go file data of a parsed
// file, which exists once its node store is published (a Go file needs no
// such step). Publishing is process-wide, and a store parsed on another
// thread at the same time could get the same file id, so only a test in a
// child process of its own (`childprog::in_child`) may call this.
pub(crate) fn parse_type_script_published(text: &str, jsx: bool) -> Node {
    let parsed = Rc::new(parse_type_script_file(text, jsx));
    note_parsed_source_file(&parsed);
    publish_parsed_files("/");
    parsed.root
}

/// Go `diagnosticwriter.WriteFormatDiagnostics(&b, ..., &FormattingOptions{NewLine: "\n"})`
/// of the parse diagnostics of `file`, or None when there are none.
// PORT: the text is only a failure message; the port's plain formatter is
// used.
fn format_parse_diagnostics(file: Node) -> Option<String> {
    let diagnostics = source_file_diagnostics(file);
    if diagnostics.is_empty() {
        return None;
    }
    let mut b = String::new();
    ts_goport::program::write_format_diagnostics(&mut b, &diagnostics);
    Some(b)
}

// Go: testutil/parsetestutil/parsetestutil.go:25 CheckDiagnostics
/// Asserts that the given file has no parse diagnostics.
pub(crate) fn check_diagnostics(file: Node) -> Result<(), String> {
    match format_parse_diagnostics(file) {
        Some(text) => Err(text),
        None => Ok(()),
    }
}

// Go: testutil/parsetestutil/parsetestutil.go:37 CheckDiagnosticsMessage
/// Asserts that the given file has no parse diagnostics and asserts the given message.
pub(crate) fn check_diagnostics_message(file: Node, message: &str) -> Result<(), String> {
    match format_parse_diagnostics(file) {
        Some(text) => Err(format!("{message}{text}")),
        None => Ok(()),
    }
}

// Go: testutil/parsetestutil/parsetestutil.go:48 newSyntheticRecursiveVisitor
// Go: testutil/parsetestutil/parsetestutil.go:86 MarkSyntheticRecursive
/// Sets the Loc of the given node and every Node in its subtree to an undefined TextRange (-1,-1).
// PORT: a Rust node list or modifier list gets its `Loc` when it is made
// (see `ast::factory::new_node_list_with_loc`), so the list hooks cannot
// write it. The Go tests call this only on factory trees, whose lists
// already have an undefined `Loc`.
pub(crate) fn mark_synthetic_recursive(node: Node) {
    let hooks = NodeVisitorHooks::<()> {
        visit_node: Some(Rc::new(|node: Node, v: &mut _| {
            if node.is_some() {
                set_node_loc(node, TextRange::undefined());
            }
            v.visit_node(node)
        })),
        visit_token: Some(Rc::new(|node: Node, v: &mut _| {
            if node.is_some() {
                set_node_loc(node, TextRange::undefined());
            }
            v.visit_node(node)
        })),
        ..Default::default()
    };
    let mut v = new_node_visitor(
        |node: Node, v: &mut _| v.visit_each_child(node),
        None,
        hooks,
        (),
    );
    v.visit_node(node);
}
