//! Ports of internal/compiler/program_test.go (TestProgram,
//! TestIncludeProcessorDiagnosticsWithMissingFileCasing),
//! internal/compiler/contentmapper_test.go (tsgo#4712),
//! internal/checker/checker_test.go (TestGetSymbolAtLocation) and
//! internal/checker/tracer_test.go (TestTracerPushPreservesEndArgMutations).
//! The Go benchmarks are not ported.
//!
//! Each test builds a program or starts the process tracing session, so it
//! runs in a child process of its own (see `childprog`).

use super::Subtests;
use super::childprog::{
    in_child, install_map_fs, new_program, new_program_with_config, source_file,
};
use crate::support::vfstest::{MapFile, MapFs};
use std::sync::Arc;
use ts_goport::ast::{MappedDiagnosticDirective, MappedDiagnosticDirectivePolicy, TextRange};
use ts_goport::contentmapper;
use ts_goport::diagnostics_loc;
use ts_goport::frontend::bundled;
use ts_goport::frontend::compiler::{
    CompilerHost, NewProgram, ProgramOptions, content_mapper_project_error_diagnostic,
    new_compiler_host,
};
use ts_goport::frontend::json::json_unmarshal;
use ts_goport::frontend::json_ext::LspAny;
use ts_goport::frontend::tsoptions::{ParseConfigHost, get_parsed_command_line_of_config_file};
use ts_goport::frontend::tsoptions::{ParsedCommandLine, ParsedOptions};
use ts_goport::frontend::vfs::{FileMode, Fs, os_override_installed};
use ts_goport::gostd::context;
use ts_goport::gostd::{GoError, errors};
use ts_goport::locale;
use ts_goport::prelude::*;
use ts_goport::program::ls_program;
use ts_goport::spanmap;
use ts_goport::tracing::{Arg, Phase, new_tracer, start_tracing};

// Go: compiler/program_test.go:33 esnextLibs
#[rustfmt::skip]
const ESNEXT_LIBS: &[&str] = &[
    "lib.es5.d.ts",
    "lib.es2015.d.ts",
    "lib.es2016.d.ts",
    "lib.es2017.d.ts",
    "lib.es2018.d.ts",
    "lib.es2019.d.ts",
    "lib.es2020.d.ts",
    "lib.es2021.d.ts",
    "lib.es2022.d.ts",
    "lib.es2023.d.ts",
    "lib.es2024.d.ts",
    "lib.es2025.d.ts",
    "lib.es2026.d.ts",
    "lib.esnext.d.ts",
    "lib.dom.d.ts",
    "lib.dom.iterable.d.ts",
    "lib.dom.asynciterable.d.ts",
    "lib.webworker.importscripts.d.ts",
    "lib.scripthost.d.ts",
    "lib.es2015.core.d.ts",
    "lib.es2015.collection.d.ts",
    "lib.es2015.generator.d.ts",
    "lib.es2015.iterable.d.ts",
    "lib.es2015.promise.d.ts",
    "lib.es2015.proxy.d.ts",
    "lib.es2015.reflect.d.ts",
    "lib.es2015.symbol.d.ts",
    "lib.es2015.symbol.wellknown.d.ts",
    "lib.es2016.array.include.d.ts",
    "lib.es2016.intl.d.ts",
    "lib.es2017.arraybuffer.d.ts",
    "lib.es2017.date.d.ts",
    "lib.es2017.object.d.ts",
    "lib.es2017.sharedmemory.d.ts",
    "lib.es2017.string.d.ts",
    "lib.es2017.intl.d.ts",
    "lib.es2017.typedarrays.d.ts",
    "lib.es2018.asyncgenerator.d.ts",
    "lib.es2018.asynciterable.d.ts",
    "lib.es2018.intl.d.ts",
    "lib.es2018.promise.d.ts",
    "lib.es2018.regexp.d.ts",
    "lib.es2019.array.d.ts",
    "lib.es2019.object.d.ts",
    "lib.es2019.string.d.ts",
    "lib.es2019.symbol.d.ts",
    "lib.es2019.intl.d.ts",
    "lib.es2020.bigint.d.ts",
    "lib.es2020.date.d.ts",
    "lib.es2020.promise.d.ts",
    "lib.es2020.sharedmemory.d.ts",
    "lib.es2020.string.d.ts",
    "lib.es2020.symbol.wellknown.d.ts",
    "lib.es2020.intl.d.ts",
    "lib.es2020.number.d.ts",
    "lib.es2021.promise.d.ts",
    "lib.es2021.string.d.ts",
    "lib.es2021.weakref.d.ts",
    "lib.es2021.intl.d.ts",
    "lib.es2022.array.d.ts",
    "lib.es2022.error.d.ts",
    "lib.es2022.intl.d.ts",
    "lib.es2022.object.d.ts",
    "lib.es2022.string.d.ts",
    "lib.es2022.regexp.d.ts",
    "lib.es2023.array.d.ts",
    "lib.es2023.collection.d.ts",
    "lib.es2023.intl.d.ts",
    "lib.es2024.arraybuffer.d.ts",
    "lib.es2024.collection.d.ts",
    "lib.es2024.object.d.ts",
    "lib.es2024.promise.d.ts",
    "lib.es2024.regexp.d.ts",
    "lib.es2024.sharedmemory.d.ts",
    "lib.es2024.string.d.ts",
    "lib.es2025.collection.d.ts",
    "lib.es2025.float16.d.ts",
    "lib.es2025.intl.d.ts",
    "lib.es2025.iterator.d.ts",
    "lib.es2025.promise.d.ts",
    "lib.es2025.regexp.d.ts",
    "lib.es2026.array.d.ts",
    "lib.es2026.collection.d.ts",
    "lib.es2026.error.d.ts",
    "lib.es2026.iterator.d.ts",
    "lib.es2026.json.d.ts",
    "lib.es2026.math.d.ts",
    "lib.es2026.typedarrays.d.ts",
    "lib.esnext.promise.d.ts",
    "lib.esnext.date.d.ts",
    "lib.esnext.decorators.d.ts",
    "lib.esnext.disposable.d.ts",
    "lib.esnext.intl.d.ts",
    "lib.esnext.modulesource.d.ts",
    "lib.esnext.sharedmemory.d.ts",
    "lib.esnext.temporal.d.ts",
    "lib.decorators.d.ts",
    "lib.decorators.legacy.d.ts",
    "lib.esnext.full.d.ts",
];

/// The source files of each Go test case after the libs, in order.
#[rustfmt::skip]
const ORDERED_FILES: &[&str] = &[
    "c:/dev/src2/a/b/c/1.ts",
    "c:/dev/src2/a/b/2.ts",
    "c:/dev/src2/a/b/3.ts",
    "c:/dev/src2/a/4.ts",
    "c:/dev/src2/a/5.ts",
    "c:/dev/src2/a/b/c/d/e/f/6.ts",
    "c:/dev/src2/a/b/c/d/e/7.ts",
    "c:/dev/src2/a/b/c/d/e/8.ts",
    "c:/dev/src2/a/b/c/d/9.ts",
    "c:/dev/src2/a/10.ts",
    "c:/dev/src/index.ts",
];

/// Go `programTest`: (testName, files, target). Every Go case expects
/// `esnextLibs` then `ORDERED_FILES`.
type ProgramTest = (
    &'static str,
    &'static [(&'static str, &'static str)],
    ScriptTarget,
);

// Go: compiler/program_test.go:133 programTestCases
#[rustfmt::skip]
const PROGRAM_TEST_CASES: &[ProgramTest] = &[
    (
        "BasicFileOrdering",
        &[
            ("c:/dev/src/index.ts", "/// <reference path='c:/dev/src2/a/5.ts' />\n/// <reference path='c:/dev/src2/a/10.ts' />"),
            ("c:/dev/src2/a/5.ts", "/// <reference path='4.ts' />"),
            ("c:/dev/src2/a/4.ts", "/// <reference path='b/3.ts' />"),
            ("c:/dev/src2/a/b/3.ts", "/// <reference path='2.ts' />"),
            ("c:/dev/src2/a/b/2.ts", "/// <reference path='c/1.ts' />"),
            ("c:/dev/src2/a/b/c/1.ts", "console.log('hello');"),
            ("c:/dev/src2/a/10.ts", "/// <reference path='b/c/d/9.ts' />"),
            ("c:/dev/src2/a/b/c/d/9.ts", "/// <reference path='e/8.ts' />"),
            ("c:/dev/src2/a/b/c/d/e/8.ts", "/// <reference path='7.ts' />"),
            ("c:/dev/src2/a/b/c/d/e/7.ts", "/// <reference path='f/6.ts' />"),
            ("c:/dev/src2/a/b/c/d/e/f/6.ts", "console.log('world!');"),
        ],
        ScriptTarget::ES_NEXT,
    ),
    (
        "FileOrderingImports",
        &[
            ("c:/dev/src/index.ts", "import * as five from '../src2/a/5.ts';\nimport * as ten from '../src2/a/10.ts';"),
            ("c:/dev/src2/a/5.ts", "import * as four from './4.ts';"),
            ("c:/dev/src2/a/4.ts", "import * as three from './b/3.ts';"),
            ("c:/dev/src2/a/b/3.ts", "import * as two from './2.ts';"),
            ("c:/dev/src2/a/b/2.ts", "import * as one from './c/1.ts';"),
            ("c:/dev/src2/a/b/c/1.ts", "console.log('hello');"),
            ("c:/dev/src2/a/10.ts", "import * as nine from './b/c/d/9.ts';"),
            ("c:/dev/src2/a/b/c/d/9.ts", "import * as eight from './e/8.ts';"),
            ("c:/dev/src2/a/b/c/d/e/8.ts", "import * as seven from './7.ts';"),
            ("c:/dev/src2/a/b/c/d/e/7.ts", "import * as six from './f/6.ts';"),
            ("c:/dev/src2/a/b/c/d/e/f/6.ts", "console.log('world!');"),
        ],
        ScriptTarget::ES_NEXT,
    ),
    (
        "FileOrderingCycles",
        &[
            ("c:/dev/src/index.ts", "import * as five from '../src2/a/5.ts';\nimport * as ten from '../src2/a/10.ts';"),
            ("c:/dev/src2/a/5.ts", "import * as four from './4.ts';"),
            ("c:/dev/src2/a/4.ts", "import * as three from './b/3.ts';"),
            ("c:/dev/src2/a/b/3.ts", "import * as two from './2.ts';\nimport * as cycle from 'c:/dev/src/index.ts'; "),
            ("c:/dev/src2/a/b/2.ts", "import * as one from './c/1.ts';"),
            ("c:/dev/src2/a/b/c/1.ts", "console.log('hello');"),
            ("c:/dev/src2/a/10.ts", "import * as nine from './b/c/d/9.ts';"),
            ("c:/dev/src2/a/b/c/d/9.ts", "import * as eight from './e/8.ts';\nimport * as cycle from 'c:/dev/src/index.ts';"),
            ("c:/dev/src2/a/b/c/d/e/8.ts", "import * as seven from './7.ts';"),
            ("c:/dev/src2/a/b/c/d/e/7.ts", "import * as six from './f/6.ts';"),
            ("c:/dev/src2/a/b/c/d/e/f/6.ts", "console.log('world!');"),
        ],
        ScriptTarget::ES_NEXT,
    ),
];

// Go: compiler/program_test.go:229 TestProgram
// PORT: the Go subtests run on one map file system each. Here they run in
// one child process with one map file system (the OS override is set once
// per process); each subtest writes its files over the previous ones, and
// every case writes the same eleven file names.
#[test]
fn test_program() {
    in_child(module_path!(), "test_program", || {
        let map_fs = MapFs::from_map(
            Vec::<(String, MapFile)>::new(),
            false, /*useCaseSensitiveFileNames*/
        );
        install_map_fs(&map_fs, "c:/dev/src");
        let lib_prefix = format!("{}/", bundled::lib_path());
        let mut t = Subtests::new("TestProgram");
        for &(test_name, files, target) in PROGRAM_TEST_CASES {
            t.run(test_name, || {
                let fs = map_fs.fs();
                for &(file_name, contents) in files {
                    let _ = fs.write_file(file_name, contents);
                }

                let program = new_program(
                    map_fs.fs(),
                    "c:/dev/src",
                    &["c:/dev/src/index.ts"],
                    CompilerOptions {
                        target,
                        ..Default::default()
                    },
                );

                let actual_files: Vec<String> = program
                    .get_source_files()
                    .iter()
                    .map(|file| {
                        let name = file.parse_options.file_name.as_str();
                        name.strip_prefix(&lib_prefix).unwrap_or(name).to_string()
                    })
                    .collect();

                let expected_files: Vec<String> = ESNEXT_LIBS
                    .iter()
                    .chain(ORDERED_FILES)
                    .map(|s| s.to_string())
                    .collect();
                if expected_files != actual_files {
                    return Err(format!(
                        "assert.DeepEqual(expectedFiles, actualFiles) failed\n  expected: {expected_files:?}\n  actual:   {actual_files:?}"
                    ));
                }
                Ok(())
            });
        }
        t.finish();
    });
}

// Go: compiler/program_test.go:271 TestIncludeProcessorDiagnosticsWithMissingFileCasing
#[test]
fn test_include_processor_diagnostics_with_missing_file_casing() {
    in_child(
        module_path!(),
        "test_include_processor_diagnostics_with_missing_file_casing",
        || {
            // Use case-sensitive file names so that /src/MyFile.ts and /src/myFile.ts
            // have different canonical paths but the same lower-case path, triggering
            // file casing diagnostics in the include processor.
            let map_fs = MapFs::from_map(
                Vec::<(String, MapFile)>::new(),
                true, /*useCaseSensitiveFileNames*/
            );
            install_map_fs(&map_fs, "/");

            // Only create the lowercase version; /src/MyFile.ts does not exist.
            let _ = map_fs
                .fs()
                .write_file("/src/myFile.ts", "export const y = 2;");

            // List both casings as root files. The first one (/src/MyFile.ts) will fail
            // to load because it does not exist on the case-sensitive filesystem.
            let program = new_program(
                map_fs.fs(),
                "/",
                &["/src/MyFile.ts", "/src/myFile.ts"],
                CompilerOptions {
                    skip_default_lib_check: Tristate::True,
                    ..Default::default()
                },
            );

            // GetProgramDiagnostics triggers getDiagnostics which processes all
            // include processor diagnostics including the casing diagnostic whose
            // file path points to the missing /src/MyFile.ts. Before the fix this
            // panicked with a nil pointer dereference.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ls_program::get_program_diagnostics(&program)
            }));
            if let Err(payload) = result {
                panic!(
                    "assertion failed: error is not nil: panic: {}",
                    crate::astnav_api::panic_message(payload.as_ref())
                );
            }
        },
    );
}

// Go: checker/checker_test.go:20 TestGetSymbolAtLocation
#[test]
fn test_get_symbol_at_location() {
    in_child(module_path!(), "test_get_symbol_at_location", || {
        let content = "interface Foo {
  bar: string;
}
declare const foo: Foo;
foo.bar;";
        let map_fs = MapFs::from_map(
            [
                ("/foo.ts", content),
                (
                    "/tsconfig.json",
                    r#"
				{
					"compilerOptions": {},
					"files": ["foo.ts"]
				}
			"#,
                ),
            ],
            false, /*useCaseSensitiveFileNames*/
        );
        install_map_fs(&map_fs, "/");
        let fs = bundled::wrap_fs(map_fs.fs());

        let cd = "/";
        let host = new_compiler_host(cd, fs, &bundled::lib_path(), None, None, None);

        // PORT: Go passes the compiler host, which is also a
        // `tsoptions.ParseConfigHost`; `HostAsParseConfigHost` forwards the
        // two methods.
        let (parsed, errors) = get_parsed_command_line_of_config_file(
            "/tsconfig.json",
            Some(&CompilerOptions::default()),
            None,
            &HostAsParseConfigHost(host),
            None,
        );
        assert_eq!(errors.len(), 0, "Expected no errors in parsed command line");

        let p = new_program_with_config(map_fs.fs(), cd, Rc::new(parsed.expect("parsed config")));
        let _current = ls_program::enter(&p);
        ls_program::bind_source_files(&p);
        let (c, done) = ls_program::get_type_checker(&p, &context::background());
        let file = source_file(&p, "/foo.ts").root;
        let statements = file.statements();
        let interface_id = statements.get(0).name();
        let var_id = statements
            .get(1)
            .declaration_list()
            .declarations()
            .nodes()
            .get(0)
            .name();
        let prop_access = statements.get(2).expression();
        let nodes = [interface_id, var_id, prop_access];
        for node in nodes {
            let symbol = c.borrow_mut().get_symbol_at_location_exported(node);
            assert!(symbol.is_some(), "Expected symbol to be non-nil");
        }
        done.call();
    });
}

// PORT: no Go counterpart (followups11 item 1). Go `EagerJSDoc` (ast.go:1581)
// reads the file's JSDoc cache, which also holds the comments that a lazy
// parse (`resolveJSDoc`, ast.go:2745) added. The parser defers a TS comment
// whose `{@link}` its prefilter misses (`{@link<form feed>Foo}`). The call
// `f()` before `f` reads that comment lazily (the `@deprecated` check), so
// `checkSourceElementWorker` (checker.go:2296) sees the link and marks `Foo`
// used. Go N reports nothing; before 6472f970b the port reported TS6133
// "'Foo' is declared but its value is never read".
#[test]
fn test_lazy_js_doc_link_marks_its_target_used() {
    in_child(
        module_path!(),
        "test_lazy_js_doc_link_marks_its_target_used",
        || {
            let map_fs = MapFs::from_map(
                [
                    (
                        "/a.ts",
                        "import { Foo } from \"./foo\";\nf();\n/** @deprecated {@link\u{C}Foo} */\nexport function f() {}\n",
                    ),
                    ("/foo.ts", "export class Foo {}\n"),
                    (
                        "/tsconfig.json",
                        r#"{"compilerOptions":{"strict":true,"noUnusedLocals":true,"noEmit":true,"module":"esnext","target":"es2022"},"files":["a.ts","foo.ts"]}"#,
                    ),
                ],
                false, /*useCaseSensitiveFileNames*/
            );
            install_map_fs(&map_fs, "/");
            let fs = bundled::wrap_fs(map_fs.fs());
            let host = new_compiler_host("/", fs, &bundled::lib_path(), None, None, None);
            let (parsed, errors) = get_parsed_command_line_of_config_file(
                "/tsconfig.json",
                Some(&CompilerOptions::default()),
                None,
                &HostAsParseConfigHost(host),
                None,
            );
            assert_eq!(errors.len(), 0, "Expected no errors in parsed command line");
            let p =
                new_program_with_config(map_fs.fs(), "/", Rc::new(parsed.expect("parsed config")));
            let _current = ls_program::enter(&p);
            let file = source_file(&p, "/a.ts").root;
            let codes: Vec<i32> =
                ls_program::get_semantic_diagnostics(&p, &context::background(), file)
                    .iter()
                    .map(Diagnostic::code)
                    .collect();
            assert_eq!(codes, Vec::<i32>::new());
        },
    );
}

// PORT: no Go test (followups22, R169 reviewer on basetype1). The test
// above runs in a multi-program process (the test harness registers its
// programs as versions), where a list of more than 3 base types is an owned
// list that `SharedList::detach` does not read in place. This one installs
// its program as `tsc` does (`program::load`, one program in the process),
// so such a list is in the checker arena, and the `has_base_type` walk reads
// it in place. The answers must be Go `hasBaseType`'s (checker.go:19888).
#[test]
fn test_has_base_type_reads_arena_base_types_in_place() {
    in_child(
        module_path!(),
        "test_has_base_type_reads_arena_base_types_in_place",
        || {
            let map_fs = MapFs::from_map(
                [
                    (
                        "/a.ts",
                        "class A {}\ninterface I {}\nclass B extends A implements I {}\nclass C extends B {}\ninterface J {}\ninterface K extends A, B, C, I {}\ninterface L extends K, A, B, C {}\n",
                    ),
                    (
                        "/tsconfig.json",
                        r#"{"compilerOptions":{"strict":true,"noEmit":true},"files":["a.ts"]}"#,
                    ),
                ],
                false, /*useCaseSensitiveFileNames*/
            );
            install_map_fs(&map_fs, "/");
            ts_goport::program::load("/tsconfig.json");
            assert!(!ts_goport::core::is_multi_program());
            ts_goport::program::bind_all();
            let file = ts_goport::program::get_source_file("/a.ts");
            ts_goport::program::with_type_checker_for_file(file, |c| {
                let statements = ts_goport::program::get_source_file("/a.ts").statements();
                let mut declared = |i: usize| {
                    let symbol = c.get_symbol_at_location_exported(statements.get(i).name());
                    c.get_declared_type_of_symbol(symbol)
                };
                let [a, i, b, cc, j, k, l] = [0, 1, 2, 3, 4, 5, 6].map(&mut declared);
                // Resolve the base types first, so each step of the walk
                // reads them in place.
                for t in [a, i, b, cc, j, k, l] {
                    c.get_base_types_shared(t);
                }
                for (t, bases) in [(k, vec![a, b, cc, i]), (l, vec![k, a, b, cc])] {
                    let mut buf = Default::default();
                    let list = c.ty(t).resolved_base_types().expect("resolved");
                    assert_eq!(
                        list.detach(&mut buf),
                        Some(&bases[..]),
                        "base types of {t:?} in the checker arena"
                    );
                }
                // L -> K -> I and L -> K -> C -> B -> A, through two arena
                // lists. J is no base of either.
                assert!(c.has_base_type(l, i));
                assert!(c.has_base_type(l, a));
                assert!(c.has_base_type(l, k));
                assert!(c.has_base_type(k, cc));
                assert!(!c.has_base_type(l, j));
                assert!(!c.has_base_type(k, l));
                assert!(!c.has_base_type(a, k));
            });
        },
    );
}

// PORT: no Go test (basetype1). `has_base_type` (Go `hasBaseType`,
// checker.go:19888) reads resolved base types in place with
// `Type::resolved_base_types` and calls `get_base_types_shared` (Go
// `getBaseTypes`, checker.go:19504) only for a type whose base types are
// not resolved yet. The in-place read must be `None` before resolution, so
// the walk still runs `getBaseTypes` and its side effects in Go's order,
// and must equal `get_base_types_shared` after it.
#[test]
fn test_has_base_type_reads_resolved_base_types_in_place() {
    in_child(
        module_path!(),
        "test_has_base_type_reads_resolved_base_types_in_place",
        || {
            let map_fs = MapFs::from_map(
                [
                    (
                        "/a.ts",
                        "class A {}\ninterface I {}\nclass B extends A implements I {}\ninterface J extends B, I {}\nclass C extends B {}\nclass G<T> { x!: T }\nclass H extends G<string> {}\ndeclare const t: [string, number];\ninterface K extends A, B, C, I {}\n",
                    ),
                    (
                        "/tsconfig.json",
                        r#"{"compilerOptions":{"strict":true,"noEmit":true},"files":["a.ts"]}"#,
                    ),
                ],
                false, /*useCaseSensitiveFileNames*/
            );
            install_map_fs(&map_fs, "/");
            let fs = bundled::wrap_fs(map_fs.fs());
            let host = new_compiler_host("/", fs, &bundled::lib_path(), None, None, None);
            let (parsed, errors) = get_parsed_command_line_of_config_file(
                "/tsconfig.json",
                Some(&CompilerOptions::default()),
                None,
                &HostAsParseConfigHost(host),
                None,
            );
            assert_eq!(errors.len(), 0, "Expected no errors in parsed command line");
            let p =
                new_program_with_config(map_fs.fs(), "/", Rc::new(parsed.expect("parsed config")));
            let _current = ls_program::enter(&p);
            ls_program::bind_source_files(&p);
            let (checker, done) = ls_program::get_type_checker(&p, &context::background());
            let statements = source_file(&p, "/a.ts").root.statements();
            {
                let mut c = checker.borrow_mut();
                let c = &mut *c;
                let mut declared = |i: usize| {
                    let symbol = c.get_symbol_at_location_exported(statements.get(i).name());
                    c.get_declared_type_of_symbol(symbol)
                };
                let [a, i, b, j, cc, g, h, k] = [0, 1, 2, 3, 4, 5, 6, 8].map(&mut declared);
                let t_name = statements
                    .get(7)
                    .declaration_list()
                    .declarations()
                    .nodes()
                    .get(0)
                    .name();
                let t_symbol = c.get_symbol_at_location_exported(t_name);
                let t_type = c.get_type_of_symbol(t_symbol);
                let tuple = c.ty(t_type).target();
                let resolved =
                    |c: &Checker, t: TypeId| c.ty(t).resolved_base_types().map(|l| l.to_vec());
                for t in [a, i, b, j, cc, g, h, k, tuple] {
                    assert_eq!(
                        resolved(c, t),
                        None,
                        "base types of {t:?} resolved too early"
                    );
                }
                // A type that is not a class, interface or tuple: no
                // in-place read, and `getBaseTypes` gives nil.
                assert_eq!(resolved(c, c.string_type), None);
                assert!(c.get_base_types_shared(c.string_type).is_empty());

                // C -> B -> A. The walk resolves C's base types; the
                // circularity check in Go `resolveBaseTypesOfClass`
                // (`hasBaseType(B, C)`, checker.go:19597) resolves B's and A's,
                // so the walk then reads B's in place.
                assert!(c.has_base_type(cc, a));
                assert_eq!(resolved(c, cc), Some(vec![b]));
                assert_eq!(resolved(c, b), Some(vec![a]));
                assert!(!c.has_base_type(a, cc));
                assert_eq!(resolved(c, a), Some(Vec::new()));
                // The second walk takes the in-place read for C and B.
                assert!(!c.has_base_type(cc, i));
                assert_eq!(resolved(c, i), None);
                assert!(c.has_base_type(j, i));
                assert_eq!(resolved(c, j), Some(vec![b, i]));
                // H's base is the reference G<string>; its target is G.
                assert!(c.has_base_type(h, g));
                assert!(!c.has_base_type(g, h));
                let h_bases = resolved(c, h).expect("base types of H");
                assert_eq!(h_bases.len(), 1);
                assert_ne!(h_bases[0], g);
                assert_eq!(c.get_target_type(h_bases[0]), g);
                assert!(c.has_base_type(t_type, c.global_array_type));
                // More base types than an inline list holds.
                assert!(c.has_base_type(k, i));
                assert_eq!(resolved(c, k), Some(vec![a, b, cc, i]));

                for t in [a, b, j, cc, g, h, k, tuple] {
                    let shared = c.get_base_types_shared(t).to_vec();
                    assert_eq!(resolved(c, t), Some(shared.clone()), "base types of {t:?}");
                    let mut buf = Default::default();
                    let list = c.ty(t).resolved_base_types().expect("resolved");
                    // `None` only for an owned list (a multi-program process).
                    if let Some(items) = list.detach(&mut buf) {
                        assert_eq!(items, &shared[..], "detached base types of {t:?}");
                    }
                }
            }
            done.call();
        },
    );
}

/// A compiler host used as a `tsoptions.ParseConfigHost`.
struct HostAsParseConfigHost(Rc<dyn CompilerHost>);

impl ParseConfigHost for HostAsParseConfigHost {
    fn fs(&self) -> Rc<dyn Fs> {
        self.0.fs()
    }
    fn get_current_directory(&self) -> String {
        self.0.get_current_directory()
    }
}

/// Go `testTraceEvent`: (ph, name, args).
type TestTraceEvent = (String, String, IndexMap<String, LspAny>);

// Go: checker/tracer_test.go:60 findTestTraceEvent
fn find_test_trace_event(events: &[TestTraceEvent], phase: &str, name: &str) -> TestTraceEvent {
    for event in events {
        if event.0 == phase && event.1 == name {
            return event.clone();
        }
    }
    panic!("failed to find {phase} event {name:?}");
}

// Go: checker/tracer_test.go:14 TestTracerPushPreservesEndArgMutations
// PORT: Go passes the args map to `Push` and mutates it before `pop()`;
// the end event shows the change and the caller's map never gets
// "checkerId". The Rust `Push` takes the args by value, so the caller has no
// map to check, and the change goes through `Pop::args_mut` (what
// `getVariancesWorker` uses). The trace session is the process global and
// writes through the OS override, so the test runs in a child process.
#[test]
fn test_tracer_push_preserves_end_arg_mutations() {
    in_child(
        module_path!(),
        "test_tracer_push_preserves_end_arg_mutations",
        || {
            let map_fs = MapFs::from_map(
                [(
                    "/trace",
                    MapFile {
                        mode: FileMode::DIR,
                        ..MapFile::default()
                    },
                )],
                true,
            );
            install_map_fs(&map_fs, "/");

            let tr = start_tracing("/trace", "", true /*deterministic*/).expect("StartTracing");

            let args = vec![("id", Arg::Int(1))];
            let tracer = new_tracer(tr, 7);
            let mut pop = tracer.push(Phase::CheckTypes, "getVariancesWorker", args, true);

            pop.args_mut()
                .expect("recorded event")
                .push(("variances", Arg::Strs(vec!["out".to_string()])));
            drop(pop);

            tr.stop_tracing().expect("StopTracing");

            let (trace_text, ok) = map_fs.fs().read_file("/trace/trace.json");
            assert!(ok, "trace.json exists");

            let mut raw_events = LspAny::Null;
            json_unmarshal(trace_text.as_bytes(), &mut raw_events, &[]).expect("json.Unmarshal");
            let LspAny::Array(raw_events) = raw_events else {
                panic!("trace.json is not an array");
            };
            let events: Vec<TestTraceEvent> = raw_events
                .into_iter()
                .map(|event| {
                    let LspAny::Object(fields) = event else {
                        panic!("trace event is not an object");
                    };
                    let text = |key: &str| match fields.get(key) {
                        Some(LspAny::String(s)) => s.clone(),
                        _ => String::new(),
                    };
                    let args = match fields.get("args") {
                        Some(LspAny::Object(args)) => args.clone(),
                        _ => IndexMap::new(),
                    };
                    (text("ph"), text("name"), args)
                })
                .collect();

            let begin_event = find_test_trace_event(&events, "B", "getVariancesWorker");
            assert_eq!(begin_event.2.get("checkerId"), Some(&LspAny::Number(7.0)));
            assert_eq!(begin_event.2.get("variances"), None);

            let end_event = find_test_trace_event(&events, "E", "getVariancesWorker");
            assert_eq!(end_event.2.get("checkerId"), Some(&LspAny::Number(7.0)));
            let Some(LspAny::Array(variances)) = end_event.2.get("variances") else {
                panic!("end event has no variances array: {:?}", end_event.2);
            };
            assert_eq!(variances, &vec![LspAny::String("out".to_string())]);
        },
    );
}

// ---------------------------------------------------------------------------
// compiler/contentmapper_test.go (tsgo#4712)
// ---------------------------------------------------------------------------

type FakeTransform = Rc<dyn Fn(&str, &str) -> std::result::Result<contentmapper::Result, GoError>>;

/// Go `func(fileName string, content string) (contentmapper.Result, error)`.
fn fake_transform(
    transform: impl Fn(&str, &str) -> std::result::Result<contentmapper::Result, GoError> + 'static,
) -> FakeTransform {
    Rc::new(transform)
}

// Go: compiler/contentmapper_test.go:22 fakeContentMapperHost
struct FakeContentMapperHost {
    transform: FakeTransform,
}

impl contentmapper::Project for FakeContentMapperHost {
    fn refresh(&self) -> std::result::Result<(), GoError> {
        Ok(())
    }
    fn identities(&self) -> std::result::Result<Vec<String>, GoError> {
        Ok(Vec::new())
    }
    fn identity(
        &self,
        _mapper: &Rc<contentmapper::Mapper>,
    ) -> std::result::Result<String, GoError> {
        Ok("test".to_string())
    }
    fn watched_files(&self) -> std::result::Result<Vec<String>, GoError> {
        Ok(Vec::new())
    }
    fn diagnostics(&self) -> Vec<contentmapper::OptionDiagnostic> {
        Vec::new()
    }
    fn transform(
        &self,
        _mapper: &Rc<contentmapper::Mapper>,
        request: contentmapper::Request,
    ) -> std::result::Result<contentmapper::Result, GoError> {
        (self.transform)(&request.file_name, &request.content)
    }
    fn close(&self) -> std::result::Result<(), GoError> {
        Ok(())
    }
}

// Go: compiler/contentmapper_test.go:39 newContentMapperProgram
fn new_content_mapper_program(
    transform: FakeTransform,
    files: &[(&str, &str)],
    root_files: &[&str],
) -> Rc<NewProgram> {
    new_content_mapper_program_with_options(
        transform,
        files,
        root_files,
        CompilerOptions {
            skip_lib_check: Tristate::True,
            module: ModuleKind::ES_NEXT,
            module_resolution: ModuleResolutionKind::BUNDLER,
            ..Default::default()
        },
    )
}

// Go: compiler/contentmapper_test.go:47 newContentMapperProgramWithOptions
// PORT: Go writes the files into an empty case-insensitive map file system;
// `MapFs::from_map` makes the same files and parent directories. The first
// map file system of the test process is also its OS file system (see
// `childprog`); a process installs one override only. The load is single
// threaded, so it reads the files through the host, not the OS file system.
fn new_content_mapper_program_with_options(
    transform: FakeTransform,
    files: &[(&str, &str)],
    root_files: &[&str],
    options: CompilerOptions,
) -> Rc<NewProgram> {
    let map_fs = MapFs::from_map(
        files
            .iter()
            .map(|(name, content)| ((*name).to_string(), MapFile::from(*content))),
        false, /*useCaseSensitiveFileNames*/
    );
    if !os_override_installed() {
        install_map_fs(&map_fs, "/src");
    }
    let fs = bundled::wrap_fs(map_fs.fs());

    let config = ParsedCommandLine {
        parsed_config: ParsedOptions {
            file_names: root_files.iter().map(|name| (*name).to_string()).collect(),
            compiler_options: Rc::new(options),
            content_mappers: vec![Rc::new(contentmapper::Mapper {
                definition: contentmapper::Definition {
                    package: "vue".to_string(),
                    extensions: vec![".vue".to_string()],
                    ..Default::default()
                },
                manifest: contentmapper::Manifest {
                    name: "vue-mapper".to_string(),
                    version: "1.0.0".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            })],
            ..Default::default()
        },
        ..Default::default()
    };
    let project: Rc<dyn contentmapper::Project> = Rc::new(FakeContentMapperHost { transform });
    ls_program::new_program(
        ProgramOptions {
            host: new_compiler_host("/src", fs, &bundled::lib_path(), None, None, Some(project)),
            config: Rc::new(config),
            use_source_of_project_reference: false,
            // Load files on the calling goroutine for deterministic diagnostics ordering.
            single_threaded: Tristate::True,
            typings_location: String::new(),
            project_name: String::new(),
            create_module_resolver: None,
            skip_module_resolution: false,
        },
        None,
    )
}

/// A transform result with `text`, `virtual_extension` and `mappings`.
fn transform_result(
    text: &str,
    virtual_extension: &str,
    mappings: Arc<spanmap::SpanMap>,
) -> contentmapper::Result {
    contentmapper::Result {
        text: text.to_string(),
        virtual_extension: virtual_extension.to_string(),
        mappings: Some(mappings),
        ..Default::default()
    }
}

// Go: compiler/contentmapper_test.go:73 TestContentMapperVirtualExtensionSetsImpliedNodeFormat
#[test]
fn test_content_mapper_virtual_extension_sets_implied_node_format() {
    in_child(
        module_path!(),
        "test_content_mapper_virtual_extension_sets_implied_node_format",
        || {
            let program = new_content_mapper_program_with_options(
                fake_transform(|_file_name, _content| {
                    Ok(transform_result(
                        "export {};",
                        ".mts",
                        Arc::new(spanmap::new(&[])),
                    ))
                }),
                &[("/src/Component.vue", "<template />")],
                &["/src/Component.vue"],
                CompilerOptions {
                    skip_lib_check: Tristate::True,
                    module: ModuleKind::NODE_NEXT,
                    module_resolution: ModuleResolutionKind::NODE_NEXT,
                    ..Default::default()
                },
            );

            let file = source_file(&program, "/src/Component.vue");
            assert_eq!(
                program
                    .get_source_file_meta_data(file.path())
                    .implied_node_format,
                RESOLUTION_MODE_ESM
            );
        },
    );
}

// Go: compiler/contentmapper_test.go:94 TestContentMapperDirectivesPreserveIncrementalGlobals
#[test]
fn test_content_mapper_directives_preserve_incremental_globals() {
    in_child(
        module_path!(),
        "test_content_mapper_directives_preserve_incremental_globals",
        || {
            let mut t = Subtests::new("TestContentMapperDirectivesPreserveIncrementalGlobals");
            for policy in [
                MappedDiagnosticDirectivePolicy::IGNORE,
                MappedDiagnosticDirectivePolicy::EXPECT,
            ] {
                let expect = policy == MappedDiagnosticDirectivePolicy::EXPECT;
                t.run(if expect { "expect" } else { "ignore" }, || {
                    const TEXT: &str =
                        "export function values() { function* generator() { yield 1; } }";
                    let program = new_content_mapper_program_with_options(
                        fake_transform(move |_file_name, content| {
                            // The virtual source is unchanged; the directive covers the entire file, including offset zero.
                            let len = content.len() as i32;
                            Ok(contentmapper::Result {
                                text: content.to_string(),
                                virtual_extension: ".ts".to_string(),
                                mappings: Some(Arc::new(spanmap::new(&[spanmap::Segment {
                                    original_end: len,
                                    virtual_end: len,
                                    kind: spanmap::Kind::VERBATIM,
                                    features: spanmap::Feature::ALL,
                                    ..Default::default()
                                }]))),
                                diagnostic_directives: vec![MappedDiagnosticDirective {
                                    virtual_range: TextRange::new(0, len),
                                    original_range: TextRange::new(0, len),
                                    policy,
                                    source: "vue".to_string(),
                                    unused_code: 2578,
                                    unused_message_text: "Unused mapped expect directive."
                                        .to_string(),
                                }],
                                ..Default::default()
                            })
                        }),
                        &[("/src/Component.vue", TEXT)],
                        &["/src/Component.vue"],
                        CompilerOptions {
                            lib: Some(vec!["lib.es5.d.ts".to_string()]),
                            skip_lib_check: Tristate::True,
                            module: ModuleKind::ES_NEXT,
                            module_resolution: ModuleResolutionKind::BUNDLER,
                            ..Default::default()
                        },
                    );
                    let ctx = context::background();
                    assert_eq!(ls_program::get_global_diagnostics(&program, &ctx).len(), 0);
                    let file = source_file(&program, "/src/Component.vue").root;
                    let diags = ls_program::get_semantic_diagnostics_for_incremental(
                        &program,
                        &ctx,
                        &[file],
                    )
                    .remove(&file)
                    .unwrap_or_default();
                    let (mut globals, mut unused) = (0, 0);
                    for diag in &diags {
                        if diag.file.is_nil() {
                            assert_eq!(diag.code, diag::Cannot_find_global_type_0.code() as i32);
                            assert_eq!(diag.message_args[0], "IterableIterator");
                            globals += 1;
                        } else {
                            assert_eq!(diag.file, file);
                            assert_eq!(diag.source, "vue");
                            assert_eq!(diag.code, 2578);
                            unused += 1;
                        }
                    }
                    assert_eq!(globals, 1);
                    assert_eq!(unused, if expect { 1 } else { 0 });
                    Ok(())
                });
            }
            t.finish();
        },
    );
}

// Go: compiler/contentmapper_test.go:153 TestCompositeProjectContentMapperSupplementalRoots
#[test]
fn test_composite_project_content_mapper_supplemental_roots() {
    in_child(
        module_path!(),
        "test_composite_project_content_mapper_supplemental_roots",
        || {
            let content_mapper_host = fake_transform(|_file_name, _content| {
                Ok(contentmapper::Result {
                    text: "export {};".to_string(),
                    virtual_extension: ".ts".to_string(),
                    mappings: Some(Arc::new(spanmap::new(&[]))),
                    supplemental: vec![contentmapper::MappedResult {
                        text: "export {};".to_string(),
                        virtual_extension: ".mts".to_string(),
                        mappings: Some(Arc::new(spanmap::new(&[]))),
                        ..Default::default()
                    }],
                    ..Default::default()
                })
            });
            let options = || CompilerOptions {
                composite: Tristate::True,
                skip_lib_check: Tristate::True,
                module: ModuleKind::ES_NEXT,
                module_resolution: ModuleResolutionKind::BUNDLER,
                ..Default::default()
            };
            let unlisted_file_code = diag::File_0_is_not_listed_within_the_file_list_of_project_1_Projects_must_list_all_files_or_use_an_include_pattern.code() as i32;
            let mut t = Subtests::new("TestCompositeProjectContentMapperSupplementalRoots");

            t.run("listed canonical root", || {
                let program = new_content_mapper_program_with_options(
                    content_mapper_host.clone(),
                    &[("/src/Component.vue", "<template />")],
                    &["/src/Component.vue"],
                    options(),
                );

                let program_diagnostics = collect_content_mapper_diagnostics(&program);
                let has_unlisted_file_diagnostic = program_diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code == unlisted_file_code);
                assert!(
                    !has_unlisted_file_diagnostic,
                    "supplemental output should be covered by its listed canonical root: {program_diagnostics:?}"
                );
                Ok(())
            });

            t.run("imported canonical file", || {
                let program = new_content_mapper_program_with_options(
                    content_mapper_host.clone(),
                    &[
                        ("/src/index.ts", r#"import "./Component.vue";"#),
                        ("/src/Component.vue", "<template />"),
                    ],
                    &["/src/index.ts"],
                    options(),
                );

                let unlisted_file_diagnostic_count = collect_content_mapper_diagnostics(&program)
                    .iter()
                    .filter(|diagnostic| diagnostic.code == unlisted_file_code)
                    .count();
                assert_eq!(unlisted_file_diagnostic_count, 2);
                Ok(())
            });

            t.finish();
        },
    );
}

// Go: compiler/contentmapper_test.go:216 collectContentMapperDiagnostics
fn collect_content_mapper_diagnostics(program: &NewProgram) -> Vec<Diagnostic> {
    let ctx = context::background();
    let mut diagnostics = ls_program::get_syntactic_diagnostics(program, &ctx, Node::NIL);
    diagnostics.extend(ls_program::get_semantic_diagnostics(
        program,
        &ctx,
        Node::NIL,
    ));
    diagnostics.extend(ls_program::get_program_diagnostics(program));
    diagnostics
}

// Go: compiler/contentmapper_test.go:225 TestContentMapperInvalidMappings
#[test]
fn test_content_mapper_invalid_mappings() {
    in_child(
        module_path!(),
        "test_content_mapper_invalid_mappings",
        || {
            const TRANSFORMED: &str = "export const x = 1;\n";
            const ORIGINAL: &str = "<template>x</template>\n";
            let mappings = Arc::new(spanmap::new(&[
                spanmap::Segment {
                    virtual_start: 0,
                    virtual_end: 10,
                    original_start: 0,
                    original_end: 0,
                    kind: spanmap::Kind::ATOM,
                    ..Default::default()
                },
                spanmap::Segment {
                    virtual_start: 5,
                    virtual_end: TRANSFORMED.len() as i32,
                    original_start: 0,
                    original_end: 0,
                    kind: spanmap::Kind::ATOM,
                    ..Default::default()
                },
            ]));
            let program = new_content_mapper_program(
                fake_transform(move |_file_name, _content| {
                    Ok(transform_result(TRANSFORMED, ".ts", mappings.clone()))
                }),
                &[
                    ("/src/app.ts", r#"import "./Component.vue";"#),
                    ("/src/Component.vue", ORIGINAL),
                ],
                &["/src/app.ts"],
            );
            let program_diagnostics = collect_content_mapper_diagnostics(&program);
            let code = diag::The_content_mapper_0_produced_overlapping_or_out_of_order_position_mappings_near_virtual_offset_1.code() as i32;
            let found = program_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == code);
            assert!(
                found,
                "expected an invalid mapping diagnostic, got: {program_diagnostics:?}"
            );
        },
    );
}

// Go: compiler/contentmapper_test.go:251 TestContentMapperSourceFileState
#[test]
fn test_content_mapper_source_file_state() {
    in_child(
        module_path!(),
        "test_content_mapper_source_file_state",
        || {
            let mut t = Subtests::new("TestContentMapperSourceFileState");

            t.run("successful synthesized empty file", || {
                let program = new_content_mapper_program(
                    fake_transform(|_file_name, _content| {
                        Ok(transform_result(
                            "export {};",
                            ".ts",
                            Arc::new(spanmap::new(&[])),
                        ))
                    }),
                    &[("/src/empty.vue", "")],
                    &["/src/empty.vue"],
                );
                let file = source_file(&program, "/src/empty.vue");
                assert_eq!(file.original_text(), "");
                assert_eq!(file.content_mapper(), "vue-mapper@1.0.0");
                assert!(!file.is_content_mapper_failure_stub());
                Ok(())
            });

            t.run("failed transform", || {
                let program = new_content_mapper_program(
                    fake_transform(|_file_name, _content| Err(errors::new("failed"))),
                    &[("/src/fail.vue", "original")],
                    &["/src/fail.vue"],
                );
                let file = source_file(&program, "/src/fail.vue");
                assert_eq!(file.original_text(), "original");
                assert_eq!(file.content_mapper(), "vue-mapper@1.0.0");
                assert!(file.is_content_mapper_failure_stub());
                Ok(())
            });

            t.run("project error is localized", || {
                let program = new_content_mapper_program(
                    fake_transform(|_file_name, _content| {
                        Err(contentmapper::new_transform_error(
                            contentmapper::TransformErrorKind::PROJECT,
                            Some(
                                contentmapper::ProjectError {
                                    kind: contentmapper::ProjectErrorKind::MALFORMED_RESPONSE,
                                }
                                .to_go_error(),
                            ),
                        )
                        .to_go_error())
                    }),
                    &[("/src/fail.vue", "original")],
                    &["/src/fail.vue"],
                );
                let program_diagnostics = collect_content_mapper_diagnostics(&program);
                let code =
                    diag::The_content_mapper_returned_a_project_response_that_could_not_be_decoded
                        .code() as i32;
                let found = program_diagnostics.iter().any(|diagnostic| {
                    diagnostic
                        .message_chain
                        .iter()
                        .any(|message| message.code == code)
                });
                assert!(
                    found,
                    "expected a localized project response diagnostic, got: {program_diagnostics:?}"
                );
                Ok(())
            });

            t.finish();
        },
    );
}

// Go: compiler/contentmapper_test.go:302 TestContentMapperProjectErrorDiagnostics
#[test]
fn test_content_mapper_project_error_diagnostics() {
    let tests = [
        (
            contentmapper::ProjectErrorKind::MISSING_CONFIG_IDENTITY,
            diag::The_content_mapper_did_not_return_configIdentity_which_is_required_when_the_content_mapper_has_dynamicConfig_Colon_true_in_its_package_json,
            r#"The content mapper did not return 'configIdentity', which is required when the content mapper has '"dynamicConfig": true' in its package.json."#,
        ),
        (
            contentmapper::ProjectErrorKind::UNEXPECTED_CONFIG_IDENTITY,
            diag::The_content_mapper_returned_configIdentity_which_is_only_allowed_when_it_declares_dynamicConfig_Colon_true_in_its_package_json,
            r#"The content mapper returned 'configIdentity', which is only allowed when it declares '"dynamicConfig": true' in its package.json."#,
        ),
        (
            contentmapper::ProjectErrorKind::UNEXPECTED_WATCHED_FILES,
            diag::The_content_mapper_returned_watchedFiles_which_is_only_allowed_when_it_declares_dynamicConfig_Colon_true_in_its_package_json,
            r#"The content mapper returned 'watchedFiles', which is only allowed when it declares '"dynamicConfig": true' in its package.json."#,
        ),
    ];
    let mut t = Subtests::new("TestContentMapperProjectErrorDiagnostics");
    for (kind, want, text) in tests {
        t.run(text, || {
            let message = content_mapper_project_error_diagnostic(
                &contentmapper::ProjectError { kind }.to_go_error(),
            );
            assert_eq!(message.code(), want.code());
            assert_eq!(
                diagnostics_loc::localize(&locale::DEFAULT, Some(message), message.key(), &[]),
                text
            );
            Ok(())
        });
    }
    t.finish();
}
