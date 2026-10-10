//! Rust ports of the Go compiler, emit and language service unit tests
//! (sub-track S3): ast, scanner, parser, sourcemap, module,
//! modulespecifiers, outputpaths, compiler, checker, execute/build,
//! execute/incremental, execute/tsc, printer, transformers/tstransforms, ls,
//! ls/lsconv and ls/lsutil, with the Go test helpers testutil/parsetestutil
//! and testutil/emittestutil.
//!
//! Expected values are the Go test literals. A test that shows a port bug
//! has `#[ignore = "bug: S3-NNN ..."]`; the bug is in
//! `target/continuation-r97-goport/tests2/bugs/S3.md`. A Go test that needs
//! a Rust item that is not `pub` is listed there as blocked and is not
//! ported.
//!
//! PORT: Go `t.Run` subtests run in order through `Subtests` (Go
//! `t.Parallel()` is dropped). Each Go `Test` function is one `#[test]`.
//! Go benchmarks and fuzz targets are not ported.
//!
//! Tests that build a program (compiler, checker, execute/build, name
//! generation with a bound file, import elision) run in a child process of
//! their own (`support::child::run_test_in_child`): a program and the OS
//! override are process-wide in the port.

mod ast_tests;
mod buildinfo_contentmapper_tests;
mod checker_tests;
mod checkerpool_tests;
mod childprog;
mod clean_tests;
mod emittestutil;
mod execute_tests;
mod ls_tests;
mod ls_userprefs;
mod module_tests;
mod outputpaths_tests;
mod parsetestutil;
mod printer_emit;
mod printer_misc;
mod printer_namegenerator;
mod printer_parenthesize;
mod program_tests;
mod scanner_parser;
mod sourcemap_generator;
mod tstransforms;

pub(crate) use crate::astnav_api::Subtests;
use ts_goport::prelude::*;

/// Go `nil` for a node, node list or modifier list argument. The parameter
/// type picks the handle.
pub(crate) trait Nil {
    const NIL: Self;
}

impl Nil for Node {
    const NIL: Node = Node::NIL;
}

impl Nil for NodeList {
    const NIL: NodeList = NodeList::NIL;
}

impl Nil for ModifierList {
    const NIL: ModifierList = ModifierList::NIL;
}

/// Go `nil` in a factory call (see `Nil`).
pub(crate) fn nil<T: Nil>() -> T {
    T::NIL
}

/// Fails the test with the message of a Go `t.Error` or `assert.*`.
#[track_caller]
pub(crate) fn must(result: Result<(), String>) {
    if let Err(message) = result {
        panic!("{message}");
    }
}

/// Go `assert.Equal(t, a, b)` inside a subtest.
pub(crate) fn assert_equal<T: PartialEq + std::fmt::Debug>(
    actual: T,
    expected: T,
    what: &str,
) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "assertion failed: {what}\n  actual:   {actual:?}\n  expected: {expected:?}"
        ))
    }
}

/// A text leaked for the parser, which takes `&'static str` (Go strings
/// live as long as their references).
pub(crate) fn leak(text: &str) -> &'static str {
    Box::leak(text.to_string().into_boxed_str())
}
