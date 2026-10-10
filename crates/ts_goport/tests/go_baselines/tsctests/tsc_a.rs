//! Port of typescript-go `internal/execute/tsctests/tsc_test.go` lines 1-2379
//! (`TestTscCommandline` through `TestTscListFilesOnly`), pinned at dc37b5249.
//!
//! Each Go `TestX` becomes `x_inputs()`, with the same inputs in the same order, and a
//! `#[test] fn x()` that runs the non-watch inputs. When a test has watch inputs,
//! `x_watch()` runs them. String literals are byte-for-byte copies of the Go
//! literals (Go `Dedent` literals are raw strings with their original tabs).

use std::collections::BTreeMap;

use crate::support::runner::*;
use crate::support::stringtestutil::{dedent, go_sprintf};
use crate::support::test_sys::{TSC_LIB_PATH, TestSys, get_test_lib_path_for};
use crate::support::vfstest::symlink;
// PORT: `file_map!` and `args!` resolve through this glob when the runner
// `#[macro_export]`s them, and through the runner glob when it re-exports them.
#[allow(unused_imports)]
use crate::*;

// Go: sys.go:37 tscDefaultLibContent
// PORT: the shared support contract does not export this value, so this file keeps
// a byte-identical copy.
fn tsc_default_lib_content() -> String {
    dedent(
        r#"
/// <reference no-default-lib="true"/>
interface Boolean {}
interface Function {}
interface CallableFunction {}
interface NewableFunction {}
interface IArguments {}
interface Number { toExponential: any; }
interface Object {}
interface RegExp {}
interface String { charAt: any; }
interface Array<T> { length: number; [n: number]: T; }
interface ReadonlyArray<T> {}
interface SymbolConstructor {
    (desc?: string | number): symbol;
    for(name: string): symbol;
    readonly toStringTag: symbol;
}
declare var Symbol: SymbolConstructor;
interface Symbol {
    readonly [Symbol.toStringTag]: string;
}
declare const console: { log(msg: any): void; };
"#,
    )
}

// Go: tsc_test.go:15 TestTscCommandline
fn tsc_commandline_inputs() -> Vec<TscInput> {
    // Go: tsc_test.go:17 colorTest (ts#63941)
    let color_test = |sub_scenario: &str, env: &[(&str, &str)], output_is_tty: bool| TscInput {
        sub_scenario: sub_scenario.into(),
        files: file_map! {
            "/home/src/workspaces/project/index.ts" => "const x: string = 1;",
        },
        command_line_args: args!["index.ts", "--noEmit"],
        env: env
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        output_is_tty: Some(output_is_tty),
        ..Default::default()
    };
    vec![
        // ts#64452
        TscInput {
            sub_scenario: "global diagnostics produced during ordinary semantic checking".into(),
            files: file_map! {
                "/home/src/workspaces/project/index.ts" => "export function* values() { yield 1; }",
            },
            command_line_args: args!["index.ts", "--noEmit"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "show help with ExitStatus.DiagnosticsPresent_OutputsSkipped".into(),
            env: BTreeMap::from([
                ("TS_TEST_TERMINAL_WIDTH".to_string(), "120".to_string()),
            ]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "show help with ExitStatus.DiagnosticsPresent_OutputsSkipped when host cannot provide terminal width".into(),
            ..Default::default()
        },
        color_test("does not add color when NO_COLOR is set", &[("NO_COLOR", "true")], true),
        color_test("adds color when NO_COLOR is empty", &[("NO_COLOR", "")], true),
        color_test("adds color when FORCE_COLOR is empty and output is not a TTY", &[("FORCE_COLOR", "")], false),
        color_test("does not add color when FORCE_COLOR is zero", &[("FORCE_COLOR", "0")], true),
        color_test("adds color when FORCE_COLOR is one and output is not a TTY", &[("FORCE_COLOR", "1")], false),
        color_test("adds color when FORCE_COLOR is two and output is not a TTY", &[("FORCE_COLOR", "2")], false),
        color_test("adds color when FORCE_COLOR is three and output is not a TTY", &[("FORCE_COLOR", "3")], false),
        color_test("does not add color when FORCE_COLOR is four", &[("FORCE_COLOR", "4")], true),
        color_test("adds color when FORCE_COLOR is true and output is not a TTY", &[("FORCE_COLOR", "true")], false),
        color_test("does not add color when FORCE_COLOR is false", &[("FORCE_COLOR", "false")], true),
        color_test("does not add color when FORCE_COLOR is invalid", &[("FORCE_COLOR", "invalid")], true),
        color_test("FORCE_COLOR overrides NO_COLOR", &[("NO_COLOR", "true"), ("FORCE_COLOR", "true")], false),
        color_test("does not add color when TERM is dumb", &[("TERM", "dumb")], true),
        color_test("FORCE_COLOR overrides dumb TERM", &[("TERM", "dumb"), ("FORCE_COLOR", "true")], false),
        TscInput {
            sub_scenario: "when build not first argument".into(),
            command_line_args: args!["--verbose", "--build"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "malformed tsconfig property without value".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => r#"{"" }"#,
                "/home/src/workspaces/project/index.ts" => "",
            },
            command_line_args: vec![],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with files options".into(),
            command_line_args: args!["--init", "file0.st", "file1.ts", "file2.ts"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with boolean value compiler options".into(),
            command_line_args: args!["--init", "--noUnusedLocals"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with enum value compiler options".into(),
            command_line_args: args!["--init", "--target", "es5", "--jsx", "react"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with list compiler options".into(),
            command_line_args: args!["--init", "--types", "jquery,mocha"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with list compiler options with enum value".into(),
            command_line_args: args!["--init", "--lib", "es5,es2015.core"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with incorrect compiler option".into(),
            command_line_args: args!["--init", "--someNonExistOption"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with incorrect compiler option value".into(),
            command_line_args: args!["--init", "--lib", "nonExistLib,es5,es2015.promise"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with advanced options".into(),
            command_line_args: args!["--init", "--declaration", "--declarationDir", "lib", "--skipLibCheck", "--noErrorTruncation"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with --help".into(),
            command_line_args: args!["--init", "--help"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with --watch".into(),
            command_line_args: args!["--init", "--watch"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Initialized TSConfig with tsconfig.json".into(),
            command_line_args: args!["--init"],
            files: file_map! {
                "/home/src/workspaces/project/first.ts" => "export const a = 1",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"strict": true,
						"noEmit": true
					}
				}"#),
            },
            ..Default::default()
        },
        TscInput {
            sub_scenario: "help".into(),
            command_line_args: args!["--help"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "help all".into(),
            command_line_args: args!["--help", "--all"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Parse --lib option with file name".into(),
            files: file_map! {
                "/home/src/workspaces/project/first.ts" => "export const Key = Symbol()",
            },
            command_line_args: args!["--lib", "es6 ", "first.ts"],
            ..Default::default()
        },
        // #4407
        TscInput {
            sub_scenario: "noEmit with type error".into(),
            files: file_map! {
                "/home/src/workspaces/project/index.ts" => "x = 5;",
            },
            command_line_args: args!["--noEmit", "index.ts"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "option diagnostics are suppressed when there are syntactic errors".into(),
            files: file_map! {
                "/home/src/workspaces/project/a.ts" => "const x: = 1;",
            },
            command_line_args: args!["--strictPropertyInitialization", "--strictNullChecks", "false", "a.ts"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "non-object config root".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => "[]",
            },
            command_line_args: vec![],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Project is empty string".into(),
            files: file_map! {
                "/home/src/workspaces/project/first.ts" => "export const a = 1",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"strict": true,
						"noEmit": true
					}
				}"#),
            },
            command_line_args: vec![],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Parse -p".into(),
            files: file_map! {
                "/home/src/workspaces/project/first.ts" => "export const a = 1",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"strict": true,
						"noEmit": true
					}
				}"#),
            },
            command_line_args: args!["-p", "."],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Parse -p with path to tsconfig file".into(),
            files: file_map! {
                "/home/src/workspaces/project/first.ts" => "export const a = 1",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"strict": true,
						"noEmit": true
					}
				}"#),
            },
            command_line_args: args!["-p", "/home/src/workspaces/project/tsconfig.json"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Parse -p with path to tsconfig folder".into(),
            files: file_map! {
                "/home/src/workspaces/project/first.ts" => "export const a = 1",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"strict": true,
						"noEmit": true
					}
				}"#),
            },
            command_line_args: args!["-p", "/home/src/workspaces/project"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Parse -p with empty tsconfig file".into(),
            files: file_map! {
                "/home/src/workspaces/project/first.ts" => "export const a = 1",
                "/home/src/workspaces/project/tsconfig.json" => "",
            },
            command_line_args: args!["-p", "."],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "compiler option at top level of tsconfig".into(),
            files: file_map! {
                "/home/src/workspaces/project/index.ts" => "",
                "/home/src/workspaces/project/tsconfig.json" => r#"{ "strict": true }"#,
            },
            command_line_args: args!["--pretty", "false"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Parse enum type options".into(),
            command_line_args: args!["--moduleResolution", "nodenext ", "first.ts", "--module", "nodenext", "--target", "esnext", "--moduleDetection", "auto", "--jsx", "react", "--newLine", "crlf"],
            ..Default::default()
        },
        TscInput {
            // ts#64457: the watch interval option is removed (TS5023).
            sub_scenario: "Reject removed watch interval option".into(),
            files: file_map! {
                "/home/src/workspaces/project/first.ts" => "export const a = 1",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"strict": true,
						"noEmit": true
					}
				}"#),
            },
            command_line_args: args!["-w", "--watchInterval", "1000"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Reject removed watch interval option without tsconfig.json".into(),
            command_line_args: args!["-w", "--watchInterval", "1000"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Config with references and empty file and refers to config with noEmit".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"{
					"files": [],
					"references": [
						{
							"path": "./packages/pkg1"
						},
					],
				}"#),
                "/home/src/workspaces/project/packages/pkg1/tsconfig.json" => dedent(r#"{
					"compilerOptions": {
						"composite": true,
						"noEmit": true
					},
					"files": [
						"./index.ts",
					],
				}"#),
                "/home/src/workspaces/project/packages/pkg1/index.ts" => "export const a = 1;",
            },
            command_line_args: args!["-p", "."],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "locale".into(),
            command_line_args: args!["--locale", "cs", "--version"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "bad locale".into(),
            command_line_args: args!["--locale", "whoops", "--version"],
            ..Default::default()
        },
    ]
}

#[test]
fn tsc_commandline() {
    run_tsc_inputs(
        "commandLine",
        tsc_commandline_inputs(),
        WatchFilter::NonWatch,
    );
}

#[test]
fn tsc_commandline_watch() {
    run_tsc_inputs(
        "commandLine",
        tsc_commandline_inputs(),
        WatchFilter::WatchOnly,
    );
}

// Go: tsc_test.go:251 TestTscMissingFiles
fn tsc_missing_files_inputs() -> Vec<TscInput> {
    vec![
        TscInput {
            sub_scenario: "file in tsconfig does not exist".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"{
					"files": ["./src/doesNotExist.ts"]
					}"#),
            },
            command_line_args: args!["-p", "./tsconfig.json"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "extensionless file in tsconfig does not exist".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"{
					"files": ["./src/doesNotExist"]
					}"#),
            },
            command_line_args: args!["-p", "./tsconfig.json"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "extensionless file in tsconfig exists".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"{
					"files": ["./src/script"]
					}"#),
                "/home/src/workspaces/project/src/script" => r#"const n: number = "s";"#,
            },
            command_line_args: args!["-p", "./tsconfig.json"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "extensionless file on command line exists".into(),
            files: file_map! {
                "/home/src/workspaces/project/script" => r#"const n: number = "s";"#,
            },
            command_line_args: args!["script"],
            ..Default::default()
        },
        TscInput {
            sub_scenario:
                "extensionless file in extended tsconfig in different folder does not exist".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/tsconfig.json" => dedent(r#"{
					"extends": "./../tsconfig.base.json",
					}"#),
                "/home/src/workspaces/project/src/oops.ts" => "export const abc = 10;",
                "/home/src/workspaces/project/tsconfig.base.json" => dedent(r#"{
					"files": ["./oops"],
					}"#),
            },
            command_line_args: args!["-p", "./src/tsconfig.json"],
            ..Default::default()
        },
    ]
}

#[test]
fn tsc_missing_files() {
    run_tsc_inputs(
        "commandLine",
        tsc_missing_files_inputs(),
        WatchFilter::NonWatch,
    );
}

// Go: tsc_test.go:300 TestTscComposite
fn tsc_composite_inputs() -> Vec<TscInput> {
    vec![
        TscInput {
            sub_scenario: "when setting composite false on command line".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.ts" => "export const x = 10;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"target": "es5",
						"module": "commonjs",
						"composite": true,
					},
					"include": [
						"src/**/*.ts",
					],
				}"#),
            },
            command_line_args: args!["--composite", "false"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when setting composite null on command line".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.ts" => "export const x = 10;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"target": "es5",
						"module": "commonjs",
						"composite": true,
					},
					"include": [
						"src/**/*.ts",
					],
				}"#),
            },
            command_line_args: args!["--composite", "null"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when setting composite false on command line but has tsbuild info in config".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.ts" => "export const x = 10;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"target": "es5",
						"module": "commonjs",
						"composite": true,
						"tsBuildInfoFile": "tsconfig.json.tsbuildinfo",
					},
					"include": [
						"src/**/*.ts",
					],
				}"#),
            },
            command_line_args: args!["--composite", "false"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when setting composite false and tsbuildinfo as null on command line but has tsbuild info in config".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.ts" => "export const x = 10;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"target": "es5",
						"module": "commonjs",
						"composite": true,
						"tsBuildInfoFile": "tsconfig.json.tsbuildinfo",
					},
					"include": [
						"src/**/*.ts",
					],
				}"#),
            },
            command_line_args: args!["--composite", "false", "--tsBuildInfoFile", "null"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "converting to modules".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.ts" => "const x = 10;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"module": "none",
						"composite": true,
					},
				}"#),
            },
            edits: vec![
                TscEdit {
                    caption: "convert to modules".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/tsconfig.json", "none", "es2015");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "synthetic jsx import of ESM module from CJS module no crash no jsx element".into(),
            files: file_map! {
                "/home/src/projects/project/src/main.ts" => "export default 42;",
                "/home/src/projects/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"module": "Node16",
						"jsx": "react-jsx",
						"jsxImportSource": "solid-js",
					},
				}"#),
                "/home/src/projects/project/node_modules/solid-js/package.json" => dedent(r#"
					{
						"name": "solid-js",
						"type": "module"
					}
				"#),
                "/home/src/projects/project/node_modules/solid-js/jsx-runtime.d.ts" => dedent(r"
					export namespace JSX {
						type IntrinsicElements = { div: {}; };
					}
				"),
            },
            cwd: "/home/src/projects/project".into(),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "synthetic jsx import of ESM module from CJS module error on jsx element".into(),
            files: file_map! {
                "/home/src/projects/project/src/main.tsx" => "export default <div/>;",
                "/home/src/projects/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"module": "Node16",
						"jsx": "react-jsx",
						"jsxImportSource": "solid-js",
					},
				}"#),
                "/home/src/projects/project/node_modules/solid-js/package.json" => dedent(r#"
					{
						"name": "solid-js",
						"type": "module"
					}
				"#),
                "/home/src/projects/project/node_modules/solid-js/jsx-runtime.d.ts" => dedent(r"
					export namespace JSX {
						type IntrinsicElements = { div: {}; };
					}
				"),
            },
            cwd: "/home/src/projects/project".into(),
            ..Default::default()
        },
    ]
}

#[test]
fn tsc_composite() {
    run_tsc_inputs("composite", tsc_composite_inputs(), WatchFilter::NonWatch);
}

// Go: tsc_test.go:459 TestTscDeclarationEmit
fn tsc_declaration_emit_inputs() -> Vec<TscInput> {
    // Go: tsc_test.go:461 getBuildDeclarationEmitDtsReferenceAsTrippleSlashMap
    fn get_build_declaration_emit_dts_reference_as_tripple_slash_map(use_no_ref: bool) -> FileMap {
        let mut files = file_map! {
            "/home/src/workspaces/solution/tsconfig.base.json" => dedent(r#"
				{
					"compilerOptions": {
						"rootDir": "./",
						"outDir": "lib",
					},
				}"#),
            "/home/src/workspaces/solution/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": { "composite": true },
					"references": [{ "path": "./src" }],
					"include": [],
				}"#),
            "/home/src/workspaces/solution/src/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": { "composite": true },
					"references": [{ "path": "./subProject" }, { "path": "./subProject2" }],
					"include": [],
				}"#),
            "/home/src/workspaces/solution/src/subProject/tsconfig.json" => dedent(r#"
				{
					"extends": "../../tsconfig.base.json",
					"compilerOptions": { "composite": true },
					"references": [{ "path": "../common" }],
					"include": ["./index.ts"],
				}"#),
            "/home/src/workspaces/solution/src/subProject/index.ts" => dedent(r"
				import { Nominal } from '../common/nominal';
				export type MyNominal = Nominal<string, 'MyNominal'>;"),
            "/home/src/workspaces/solution/src/subProject2/tsconfig.json" => dedent(r#"
				{
					"extends": "../../tsconfig.base.json",
					"compilerOptions": { "composite": true },
					"references": [{ "path": "../subProject" }],
					"include": ["./index.ts"],
				}"#),
            "/home/src/workspaces/solution/src/subProject2/index.ts" => dedent(r"
				import { MyNominal } from '../subProject/index';
				const variable = {
					key: 'value' as MyNominal,
				};
				export function getVar(): keyof typeof variable {
					return 'key';
				}"),
            "/home/src/workspaces/solution/src/common/tsconfig.json" => dedent(r#"
				{
					"extends": "../../tsconfig.base.json",
					"compilerOptions": { "composite": true },
					"include": ["./nominal.ts"],
				}"#),
            "/home/src/workspaces/solution/src/common/nominal.ts" => dedent(r#"
				/// <reference path="./types.d.ts" preserve="true" />
				export declare type Nominal<T, Name extends string> = MyNominal<T, Name>;"#),
            "/home/src/workspaces/solution/src/common/types.d.ts" => dedent(r"
				declare type MyNominal<T, Name extends string> = T & {
					specialKey: Name;
				};"),
        };
        if use_no_ref {
            files.insert(
                "/home/src/workspaces/solution/tsconfig.json".into(),
                dedent(
                    r#"
			{
				"extends": "./tsconfig.base.json",
				"compilerOptions": { "composite": true },
				"include": ["./src/**/*.ts"],
			}"#,
                )
                .into(),
            );
        }
        files
    }

    // Go: tsc_test.go:532 getTscDeclarationEmitDtsErrorsFileMap
    fn get_tsc_declaration_emit_dts_errors_file_map(composite: bool, incremental: bool) -> FileMap {
        file_map! {
            "/home/src/workspaces/project/tsconfig.json" => dedent(&go_sprintf(r#"
				{
					"compilerOptions": {
						"module": "NodeNext",
						"moduleResolution": "NodeNext",
						"composite": %t,
						"incremental": %t,
						"declaration": true,
						"skipLibCheck": true,
						"skipDefaultLibCheck": true,
					},
				}"#, &[&composite, &incremental])),
            "/home/src/workspaces/project/index.ts" => dedent(r"
				import ky from 'ky';
				export const api = ky.extend({});
			"),
            "/home/src/workspaces/project/package.json" => dedent(r#"
				{
					"type": "module"
				}"#),
            "/home/src/workspaces/project/node_modules/ky/distribution/index.d.ts" => dedent(r"
				type KyInstance = {
					extend(options: Record<string,unknown>): KyInstance;
				}
				declare const ky: KyInstance;
				export default ky;
			"),
            "/home/src/workspaces/project/node_modules/ky/package.json" => dedent(r#"
				{
					"name": "ky",
					"type": "module",
					"main": "./distribution/index.js"
				}
			"#),
        }
    }

    // Go: tsc_test.go:571 pluginOneConfig
    fn plugin_one_config() -> String {
        dedent(
            r#"
		{
			"compilerOptions": {
				"target": "es5",
				"declaration": true,
				"traceResolution": true,
			},
		}"#,
        )
    }

    // Go: tsc_test.go:582 pluginOneIndex
    fn plugin_one_index() -> String {
        r#"import pluginTwo from "plugin-two"; // include this to add reference to symlink"#.into()
    }

    // Go: tsc_test.go:586 pluginOneAction
    fn plugin_one_action() -> String {
        dedent(
            r#"
			import { actionCreatorFactory } from "typescript-fsa"; // Include version of shared lib
			const action = actionCreatorFactory("somekey");
			const featureOne = action<{ route: string }>("feature-one");
			export const actions = { featureOne };"#,
        )
    }

    // Go: tsc_test.go:594 pluginTwoDts
    fn plugin_two_dts() -> String {
        dedent(
            r#"
			declare const _default: {
				features: {
					featureOne: {
						actions: {
							featureOne: {
								(payload: {
									name: string;
									order: number;
								}, meta?: {
									[key: string]: any;
								}): import("typescript-fsa").Action<{
									name: string;
									order: number;
								}>;
							};
						};
						path: string;
					};
				};
			};
			export default _default;"#,
        )
    }

    // Go: tsc_test.go:619 fsaPackageJson
    fn fsa_package_json() -> String {
        dedent(
            r#"
			{
				"name": "typescript-fsa",
				"version": "3.0.0-beta-2"
			}"#,
        )
    }

    // Go: tsc_test.go:627 fsaIndex
    fn fsa_index() -> String {
        dedent(
            r"
			export interface Action<Payload> {
				type: string;
				payload: Payload;
			}
			export declare type ActionCreator<Payload> = {
				type: string;
				(payload: Payload): Action<Payload>;
			}
			export interface ActionCreatorFactory {
				<Payload = void>(type: string): ActionCreator<Payload>;
			}
			export declare function actionCreatorFactory(prefix?: string | null): ActionCreatorFactory;
			export default actionCreatorFactory;",
        )
    }

    vec![
        TscInput {
            sub_scenario: "when declaration file is referenced through triple slash".into(),
            files: get_build_declaration_emit_dts_reference_as_tripple_slash_map(false),
            cwd: "/home/src/workspaces/solution".into(),
            command_line_args: args!["--b", "--verbose"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when declaration file is referenced through triple slash but uses no references".into(),
            files: get_build_declaration_emit_dts_reference_as_tripple_slash_map(true),
            cwd: "/home/src/workspaces/solution".into(),
            command_line_args: args!["--b", "--verbose"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when ts file is referenced through triple slash from another project".into(),
            files: file_map! {
                "/home/src/workspaces/solution/include/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": { "composite": true, "declaration": true },
					}"#),
                "/home/src/workspaces/solution/include/include.ts" => dedent(r"
					export const include = 1;"),
                "/home/src/workspaces/solution/src/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": { "composite": true, "declaration": true },
						"references": [{ "path": "../include" }],
					}"#),
                "/home/src/workspaces/solution/src/main.ts" => dedent(r#"
					/// <reference path="../include/include.ts" preserve="true" />
					export const main = 23;"#),
            },
            cwd: "/home/src/workspaces/solution".into(),
            command_line_args: args!["--b", "src", "--verbose"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when declaration file used inferred type from referenced project".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"composite": true,
							"paths": { "@fluentui/*": ["./packages/*/src"] },
						},
					}"#),
                "/home/src/workspaces/project/packages/pkg1/src/index.ts" => dedent(r"
					export interface IThing {
						a: string;
					}
					export interface IThings {
						thing1: IThing;
					}
				"),
                "/home/src/workspaces/project/packages/pkg1/tsconfig.json" => dedent(r#"
					{
						"extends": "../../tsconfig",
						"compilerOptions": { "outDir": "lib" },
						"include": ["src"],
					}
				"#),
                "/home/src/workspaces/project/packages/pkg2/src/index.ts" => dedent(r"
					import { IThings } from '@fluentui/pkg1';
					export function fn4() {
						const a: IThings = { thing1: { a: 'b' } };
						return a.thing1;
					}
				"),
                "/home/src/workspaces/project/packages/pkg2/tsconfig.json" => dedent(r#"
					{
						"extends": "../../tsconfig",
						"compilerOptions": { "outDir": "lib" },
						"include": ["src"],
						"references": [{ "path": "../pkg1" }],
					}
				"#),
            },
            command_line_args: args!["--b", "packages/pkg2/tsconfig.json", "--verbose"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when inferred export should reuse imported type alias across a module boundary".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"strict": true,
							"declaration": true,
							"emitDeclarationOnly": true,
							"target": "es2022",
							"module": "esnext",
						},
						"files": ["./a.ts", "./factory.ts", "./state.ts"],
					}"#),
                format!("{}{}", TSC_LIB_PATH, "/lib.es2022.full.d.ts") => format!("{}{}{}", tsc_default_lib_content(), "\n", dedent(r"
					type Partial<T> = {
						[K in keyof T]?: T[K];
					};
				")),
                "/home/src/workspaces/project/a.ts" => dedent(r"
					interface ISettings {
						age: number;
					}

					export type Settings = Partial<ISettings>;
				"),
                "/home/src/workspaces/project/factory.ts" => dedent(r#"
					import type { Settings } from "./a";

					export const makeObj = () => ({
						fn: (s?: Settings): Settings | undefined => s,
					});
				"#),
                "/home/src/workspaces/project/state.ts" => dedent(r#"
					import { makeObj } from "./factory";

					export const obj = makeObj();
				"#),
            },
            command_line_args: args!["--p", "tsconfig.json"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "reports dts generation errors".into(),
            files: get_tsc_declaration_emit_dts_errors_file_map(false, false),
            command_line_args: args!["-b", "--explainFiles", "--listEmittedFiles", "--v"],
            edits: no_change_only_edit(),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "reports dts generation errors with incremental".into(),
            files: get_tsc_declaration_emit_dts_errors_file_map(false, true),
            command_line_args: args!["-b", "--explainFiles", "--listEmittedFiles", "--v"],
            edits: no_change_only_edit(),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "reports dts generation errors".into(),
            files: get_tsc_declaration_emit_dts_errors_file_map(false, false),
            command_line_args: args!["--explainFiles", "--listEmittedFiles"],
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "build -b".into(),
                    command_line_args: Some(args!["-b", "--explainFiles", "--listEmittedFiles", "--v"]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "reports dts generation errors with incremental".into(),
            files: get_tsc_declaration_emit_dts_errors_file_map(true, true),
            command_line_args: args!["--explainFiles", "--listEmittedFiles"],
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "build -b".into(),
                    command_line_args: Some(args!["-b", "--explainFiles", "--listEmittedFiles", "--v"]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when using Windows paths and uppercase letters".into(),
            files: file_map! {
                "D:/Work/pkg1/package.json" => dedent(r#"
				{
					"name": "ts-specifier-bug",
					"version": "1.0.0",
					"main": "index.js"
				}"#),
                "D:/Work/pkg1/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"declaration": true,
						"target": "es2017",
						"outDir": "./dist",
					},
					"include": ["src"],
				}"#),
                "D:/Work/pkg1/src/main.ts" => dedent(r"
					import { PartialType } from './utils';

					class Common {}
					
					export class Sub extends PartialType(Common) {
						id: string;
					}
				"),
                "D:/Work/pkg1/src/utils/index.ts" => dedent(r"
					import { MyType, MyReturnType } from './type-helpers';

					export function PartialType<T>(classRef: MyType<T>) {
						abstract class PartialClassType {
							constructor() {}
						}
					
						return PartialClassType as MyReturnType;
					}
				"),
                "D:/Work/pkg1/src/utils/type-helpers.ts" => dedent(r"
					export type MyReturnType = {	
						new (...args: any[]): any;
					};
				
					export interface MyType<T = any> extends Function {
						new (...args: any[]): T;
					}
				"),
            },
            cwd: "D:/Work/pkg1".into(),
            windows_style_root: "D:/".into(),
            ignore_case: true,
            command_line_args: args!["-p", "D:\\Work\\pkg1", "--explainFiles"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when same version is referenced through source and another symlinked package".into(),
            files: file_map! {
                "/user/username/projects/myproject/plugin-two/index.d.ts" => plugin_two_dts(),
                "/user/username/projects/myproject/plugin-two/node_modules/typescript-fsa/package.json" => fsa_package_json(),
                "/user/username/projects/myproject/plugin-two/node_modules/typescript-fsa/index.d.ts" => fsa_index(),
                "/user/username/projects/myproject/plugin-one/tsconfig.json" => plugin_one_config(),
                "/user/username/projects/myproject/plugin-one/index.ts" => plugin_one_index(),
                "/user/username/projects/myproject/plugin-one/action.ts" => plugin_one_action(),
                "/user/username/projects/myproject/plugin-one/node_modules/typescript-fsa/package.json" => fsa_package_json(),
                "/user/username/projects/myproject/plugin-one/node_modules/typescript-fsa/index.d.ts" => fsa_index(),
                "/user/username/projects/myproject/plugin-one/node_modules/plugin-two" => symlink("/user/username/projects/myproject/plugin-two"),
            },
            cwd: "/user/username/projects/myproject".into(),
            command_line_args: args!["-p", "plugin-one", "--explainFiles"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when same version is referenced through source and another symlinked package with indirect link".into(),
            files: file_map! {
                "/user/username/projects/myproject/plugin-two/package.json" => dedent(r#"
				{
					"name": "plugin-two",
					"version": "0.1.3",
					"main": "dist/commonjs/index.js"
				}"#),
                "/user/username/projects/myproject/plugin-two/dist/commonjs/index.d.ts" => plugin_two_dts(),
                "/user/username/projects/myproject/plugin-two/node_modules/typescript-fsa/package.json" => fsa_package_json(),
                "/user/username/projects/myproject/plugin-two/node_modules/typescript-fsa/index.d.ts" => fsa_index(),
                "/user/username/projects/myproject/plugin-one/tsconfig.json" => plugin_one_config(),
                "/user/username/projects/myproject/plugin-one/index.ts" => format!("{}{}{}", plugin_one_index(), "\n", plugin_one_action()),
                "/user/username/projects/myproject/plugin-one/node_modules/typescript-fsa/package.json" => fsa_package_json(),
                "/user/username/projects/myproject/plugin-one/node_modules/typescript-fsa/index.d.ts" => fsa_index(),
                "/temp/yarn/data/link/plugin-two" => symlink("/user/username/projects/myproject/plugin-two"),
                "/user/username/projects/myproject/plugin-one/node_modules/plugin-two" => symlink("/temp/yarn/data/link/plugin-two"),
            },
            cwd: "/user/username/projects/myproject".into(),
            command_line_args: args!["-p", "plugin-one", "--explainFiles"],
            ..Default::default()
        },
        TscInput {
            // !!! sheetal strada has error for d.ts generation in pkg3/src/keys.ts but corsa doesnt have that
            sub_scenario: "when pkg references sibling package through indirect symlink".into(),
            files: file_map! {
                "/user/username/projects/myproject/pkg1/dist/index.d.ts" => "export * from './types';",
                "/user/username/projects/myproject/pkg1/dist/types.d.ts" => dedent(r"
					export declare type A = {
						id: string;
					};
					export declare type B = {
						id: number;
					};
					export declare type IdType = A | B;
					export declare class MetadataAccessor<T, D extends IdType = IdType> {
						readonly key: string;
						private constructor();
						toString(): string;
						static create<T, D extends IdType = IdType>(key: string): MetadataAccessor<T, D>;
					}"),
                "/user/username/projects/myproject/pkg1/package.json" => dedent(r#"
					{
						"name": "@raymondfeng/pkg1",
						"version": "1.0.0",
						"main": "dist/index.js",
						"typings": "dist/index.d.ts"
					}"#),
                "/user/username/projects/myproject/pkg2/dist/index.d.ts" => "export * from './types';",
                "/user/username/projects/myproject/pkg2/dist/types.d.ts" => "export {MetadataAccessor} from '@raymondfeng/pkg1';",
                "/user/username/projects/myproject/pkg2/package.json" => dedent(r#"
					{
						"name": "@raymondfeng/pkg2",
						"version": "1.0.0",
						"main": "dist/index.js",
						"typings": "dist/index.d.ts"
					}"#),
                "/user/username/projects/myproject/pkg3/src/index.ts" => "export * from './keys';",
                "/user/username/projects/myproject/pkg3/src/keys.ts" => dedent(r#"
					import {MetadataAccessor} from "@raymondfeng/pkg2";
					export const ADMIN = MetadataAccessor.create<boolean>('1');"#),
                "/user/username/projects/myproject/pkg3/tsconfig.json" => dedent(r#"
                    {
                        "compilerOptions": {
                            "outDir": "dist",
                            "rootDir": "src",
                            "target": "es5",
                            "module": "commonjs",
                            "strict": true,
                            "esModuleInterop": true,
                            "declaration": true,
                        },
                    }"#),
                "/user/username/projects/myproject/pkg2/node_modules/@raymondfeng/pkg1" => symlink("/user/username/projects/myproject/pkg1"),
                "/user/username/projects/myproject/pkg3/node_modules/@raymondfeng/pkg2" => symlink("/user/username/projects/myproject/pkg2"),
            },
            cwd: "/user/username/projects/myproject".into(),
            command_line_args: args!["-p", "pkg3", "--explainFiles"],
            ..Default::default()
        },
    ]
}

#[test]
fn tsc_declaration_emit() {
    run_tsc_inputs(
        "declarationEmit",
        tsc_declaration_emit_inputs(),
        WatchFilter::NonWatch,
    );
}

// Go: tsc_test.go:952 TestTscExtends
fn tsc_extends_inputs() -> Vec<TscInput> {
    // Go: tsc_test.go:954 getBuildConfigFileExtendsFileMap
    fn get_build_config_file_extends_file_map() -> FileMap {
        file_map! {
            "/home/src/workspaces/solution/tsconfig.json" => dedent(r#"
				{
					"references": [
						{ "path": "./shared/tsconfig.json" },
						{ "path": "./webpack/tsconfig.json" },
					],
					"files": [],
				}"#),
            "/home/src/workspaces/solution/shared/tsconfig-base.json" => dedent(r#"
				{
					"include": ["./typings-base/"],
				}"#),
            "/home/src/workspaces/solution/shared/typings-base/globals.d.ts" => "type Unrestricted = any;",
            "/home/src/workspaces/solution/shared/tsconfig.json" => dedent(r#"
				{
					"extends": "./tsconfig-base.json",
					"compilerOptions": {
						"composite": true,
						"outDir": "../target-tsc-build/",
						"rootDir": "..",
					},
					"files": ["./index.ts"],
				}"#),
            "/home/src/workspaces/solution/shared/index.ts" => "export const a: Unrestricted = 1;",
            "/home/src/workspaces/solution/webpack/tsconfig.json" => dedent(r#"
				{
					"extends": "../shared/tsconfig-base.json",
					"compilerOptions": {
						"composite": true,
						"outDir": "../target-tsc-build/",
						"rootDir": "..",
					},
					"files": ["./index.ts"],
					"references": [{ "path": "../shared/tsconfig.json" }],
				}"#),
            "/home/src/workspaces/solution/webpack/index.ts" => "export const b: Unrestricted = 1;",
        }
    }

    // Go: tsc_test.go:994 getTscExtendsWithSymlinkTestCase
    fn get_tsc_extends_with_symlink_test_case(built_type: &str) -> TscInput {
        TscInput {
            sub_scenario: "resolves the symlink path".into(),
            files: file_map! {
                "/users/user/projects/myconfigs/node_modules/@something/tsconfig-node/tsconfig.json" => dedent(r#"
					{
						"extends": "@something/tsconfig-base/tsconfig.json",
						"compilerOptions": {
							"removeComments": true
						}
					}
				"#),
                "/users/user/projects/myconfigs/node_modules/@something/tsconfig-base/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": { "composite": true }
					}
				"#),
                "/users/user/projects/myproject/src/index.ts" => dedent(r"
					// some comment
					export const x = 10;
				"),
                "/users/user/projects/myproject/src/tsconfig.json" => dedent(r#"
					{
						"extends": "@something/tsconfig-node/tsconfig.json"
					}"#),
                "/users/user/projects/myproject/node_modules/@something/tsconfig-node" => symlink("/users/user/projects/myconfigs/node_modules/@something/tsconfig-node"),
            },
            cwd: "/users/user/projects/myproject".into(),
            command_line_args: vec![
                built_type.to_string(),
                "src".to_string(),
                "--extendedDiagnostics".to_string(),
            ],
            ..Default::default()
        }
    }

    // Go: tsc_test.go:1025 getTscExtendsConfigDirTestCase
    fn get_tsc_extends_config_dir_test_case(
        sub_scenario_sufix: &str,
        command_line_args: Vec<String>,
        edits: Vec<TscEdit>,
    ) -> TscInput {
        TscInput {
            sub_scenario: format!("{}{}", "configDir template", sub_scenario_sufix),
            files: file_map! {
                "/home/src/projects/configs/first/tsconfig.json" => dedent(r#"
				{
					"extends": "../second/tsconfig.json",
					"include": ["${configDir}/src"],
					"compilerOptions": {
						"typeRoots": ["root1", "${configDir}/root2", "root3"],
						"types": [],
					},
				}"#),
                "/home/src/projects/configs/second/tsconfig.json" => dedent(r#"
				{
					"files": ["${configDir}/main.ts"],
					"compilerOptions": {
						"declarationDir": "${configDir}/decls",
						"paths": {
							"@myscope/*": ["${configDir}/types/*"],
						},
					},
				}"#),
                "/home/src/projects/myproject/tsconfig.json" => dedent(r#"
				{
					"extends": "../configs/first/tsconfig.json",
					"compilerOptions": {
						"declaration": true,
						"outDir": "outDir",
						"traceResolution": true,
					},
				}"#),
                "/home/src/projects/myproject/main.ts" => dedent(r#"
					// some comment
					export const y = 10;
					import { x } from "@myscope/sometype";
				"#),
                "/home/src/projects/myproject/types/sometype.ts" => dedent(r"
					export const x = 10;
				"),
            },
            cwd: "/home/src/projects/myproject".into(),
            command_line_args,
            edits,
            ..Default::default()
        }
    }

    // Go: tsc_test.go:1074 getTscExtendsNonStringPathTestCase (tsgo#4384)
    fn get_tsc_extends_non_string_path_test_case(property_name: &str) -> TscInput {
        TscInput {
            sub_scenario: format!("extends config with non-string {property_name}"),
            files: file_map! {
                "/home/src/projects/project/tsconfig.json" => dedent(r#"
					{
						"extends": "./base.json",
					}"#),
                "/home/src/projects/project/base.json" => dedent(&(r#"
					{
						""#.to_string() + property_name + r#"": [1],
					}"#)),
                "/home/src/projects/project/main.ts" => "export const x = 1;",
            },
            cwd: "/home/src/projects/project".into(),
            command_line_args: args!["-p", "tsconfig.json", "--pretty", "false"],
            ..Default::default()
        }
    }

    // Go: tsc_test.go:1092 getTscExtendsBase (tsgo#4384)
    fn get_tsc_extends_base(base_contents: &str) -> FileMap {
        file_map! {
            "/home/src/projects/project/tsconfig.json" => dedent(r#"
				{
					"extends": "./base.json",
				}"#),
            "/home/src/projects/project/base.json" => dedent(base_contents),
            "/home/src/projects/project/main.ts" => "export const x = 1;",
        }
    }

    vec![
        TscInput {
            sub_scenario: "when building solution with projects extends config with include".into(),
            files: get_build_config_file_extends_file_map(),
            cwd: "/home/src/workspaces/solution".into(),
            command_line_args: args!["--b", "--v", "--listFiles"],
            ..Default::default()
        },
        get_tsc_extends_non_string_path_test_case("include"),
        get_tsc_extends_non_string_path_test_case("exclude"),
        get_tsc_extends_non_string_path_test_case("files"),
        TscInput {
            sub_scenario: "extends config with mixed valid and non-string include".into(),
            files: get_tsc_extends_base(
                r#"
				{
					"include": ["main.ts", 1],
				}"#,
            ),
            cwd: "/home/src/projects/project".into(),
            command_line_args: args!["-p", "tsconfig.json", "--pretty", "false"],
            ..Default::default()
        },
        TscInput {
            sub_scenario:
                "when building project uses reference and both extend config with include".into(),
            files: get_build_config_file_extends_file_map(),
            cwd: "/home/src/workspaces/solution".into(),
            command_line_args: args!["--b", "webpack/tsconfig.json", "--v", "--listFiles"],
            ..Default::default()
        },
        get_tsc_extends_with_symlink_test_case("-p"),
        get_tsc_extends_with_symlink_test_case("-b"),
        get_tsc_extends_config_dir_test_case("", args!["--explainFiles"], vec![]),
        get_tsc_extends_config_dir_test_case(" showConfig", args!["--showConfig"], vec![]),
        get_tsc_extends_config_dir_test_case(
            " with commandline",
            args!["--explainFiles", "--outDir", "${configDir}/outDir"],
            vec![],
        ),
        get_tsc_extends_config_dir_test_case("", args!["--b", "--explainFiles", "--v"], vec![]),
        get_tsc_extends_config_dir_test_case(
            "",
            args!["--b", "-w", "--explainFiles", "--v"],
            vec![TscEdit {
                caption: "edit extended config file".into(),
                edit: edit(|sys: &TestSys| {
                    sys.write_file_no_error(
                        "/home/src/projects/configs/first/tsconfig.json",
                        &dedent(
                            r#"
						{
							"extends": "../second/tsconfig.json",
							"include": ["${configDir}/src"],
							"compilerOptions": {
								"typeRoots": ["${configDir}/root2"],
								"types": [],
							},
						}"#,
                        ),
                    );
                }),
                ..Default::default()
            }],
        ),
    ]
}

#[test]
fn tsc_extends() {
    run_tsc_inputs("extends", tsc_extends_inputs(), WatchFilter::NonWatch);
}

#[test]
fn tsc_extends_watch() {
    run_tsc_inputs("extends", tsc_extends_inputs(), WatchFilter::WatchOnly);
}

// Go: tsc_test.go:1119 TestForceConsistentCasingInFileNames
fn force_consistent_casing_in_file_names_inputs() -> Vec<TscInput> {
    vec![
        TscInput {
            sub_scenario: "with relative and non relative file resolutions".into(),
            files: file_map! {
                "/user/username/projects/myproject/src/struct.d.ts" => dedent(r#"
                    import * as xs1 from "fp-ts/lib/Struct";
                    import * as xs2 from "fp-ts/lib/struct";
                    import * as xs3 from "./Struct";
                    import * as xs4 from "./struct";
                "#),
                "/user/username/projects/myproject/node_modules/fp-ts/lib/struct.d.ts" => "export function foo(): void",
            },
            cwd: "/user/username/projects/myproject".into(),
            command_line_args: args![
                "/user/username/projects/myproject/src/struct.d.ts",
                "--forceConsistentCasingInFileNames",
                "--explainFiles"
            ],
            ignore_case: true,
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when file is included from multiple places with different casing".into(),
            files: file_map! {
                "/home/src/projects/project/src/struct.d.ts" => dedent(r#"
					import * as xs1 from "fp-ts/lib/Struct";
					import * as xs2 from "fp-ts/lib/struct";
					import * as xs3 from "./Struct";
					import * as xs4 from "./struct";
				"#),
                "/home/src/projects/project/src/anotherFile.ts" => dedent(r#"
					import * as xs1 from "fp-ts/lib/Struct";
					import * as xs2 from "fp-ts/lib/struct";
					import * as xs3 from "./Struct";
					import * as xs4 from "./struct";
				"#),
                "/home/src/projects/project/src/oneMore.ts" => dedent(r#"
					import * as xs1 from "fp-ts/lib/Struct";
					import * as xs2 from "fp-ts/lib/struct";
					import * as xs3 from "./Struct";
					import * as xs4 from "./struct";
				"#),
                "/home/src/projects/project/tsconfig.json" => "{}",
                "/home/src/projects/project/node_modules/fp-ts/lib/struct.d.ts" => "export function foo(): void",
            },
            cwd: "/home/src/projects/project".into(),
            command_line_args: args!["--explainFiles"],
            ignore_case: true,
            ..Default::default()
        },
        TscInput {
            sub_scenario: "with type ref from file".into(),
            files: file_map! {
                "/user/username/projects/myproject/src/fileOne.d.ts" => "declare class c { }",
                "/user/username/projects/myproject/src/file2.d.ts" => dedent(r#"
                    /// <reference types="./fileOne.d.ts"/>
                    declare const y: c;
                "#),
                "/user/username/projects/myproject/tsconfig.json" => "{ }",
            },
            cwd: "/user/username/projects/myproject".into(),
            command_line_args: args![
                "-p",
                "/user/username/projects/myproject",
                "--explainFiles",
                "--traceResolution"
            ],
            ignore_case: true,
            ..Default::default()
        },
        TscInput {
            sub_scenario: "with triple slash ref from file".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/c.ts" => r#"/// <reference path="./D.ts"/>"#,
                "/home/src/workspaces/project/src/d.ts" => "declare class c { }",
                "/home/src/workspaces/project/tsconfig.json" => "{ }",
            },
            ignore_case: true,
            ..Default::default()
        },
        TscInput {
            sub_scenario: "two files exist on disk that differs only in casing".into(),
            files: file_map! {
                "/home/src/workspaces/project/c.ts" => r#"import {x} from "./D""#,
                "/home/src/workspaces/project/D.ts" => "export const x = 10;",
                "/home/src/workspaces/project/d.ts" => "export const y = 20;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
					{
						"files": ["c.ts", "d.ts"]
					}"#),
            },
            ..Default::default()
        },
    ]
}

#[test]
fn force_consistent_casing_in_file_names() {
    run_tsc_inputs(
        "forceConsistentCasingInFileNames",
        force_consistent_casing_in_file_names_inputs(),
        WatchFilter::NonWatch,
    );
}

// Go: tsc_test.go:1206 TestTscIgnoreConfig
fn tsc_ignore_config_inputs() -> Vec<TscInput> {
    // Go: tsc_test.go:1208 filesWithoutConfig
    fn files_without_config() -> FileMap {
        file_map! {
            "/home/src/workspaces/project/src/a.ts" => "export const a = 10;",
            "/home/src/workspaces/project/src/b.ts" => "export const b = 10;",
            "/home/src/workspaces/project/c.ts" => "export const c = 10;",
        }
    }

    // Go: tsc_test.go:1215 filesWithConfig
    fn files_with_config() -> FileMap {
        let mut files = files_without_config();
        files.insert(
            "/home/src/workspaces/project/tsconfig.json".into(),
            dedent(
                r#"
			{
                "include": ["src"],
			}"#,
            )
            .into(),
        );
        files
    }

    // Go: tsc_test.go:1223 getScenarios
    fn get_scenarios(sub_scenario: &str, command_line_args: Vec<String>) -> Vec<TscInput> {
        let command_line_args_ignore_config =
            [command_line_args.clone(), args!["--ignoreConfig"]].concat();
        vec![
            TscInput {
                sub_scenario: sub_scenario.into(),
                files: files_with_config(),
                command_line_args: command_line_args.clone(),
                ..Default::default()
            },
            TscInput {
                sub_scenario: format!("{}{}", sub_scenario, " with --ignoreConfig"),
                files: files_with_config(),
                command_line_args: command_line_args_ignore_config.clone(),
                ..Default::default()
            },
            TscInput {
                sub_scenario: format!("{}{}", sub_scenario, " when config file absent"),
                files: files_without_config(),
                command_line_args: command_line_args.clone(),
                ..Default::default()
            },
            TscInput {
                sub_scenario: format!(
                    "{}{}",
                    sub_scenario, " when config file absent with --ignoreConfig"
                ),
                files: files_without_config(),
                command_line_args: command_line_args_ignore_config.clone(),
                ..Default::default()
            },
        ]
    }

    [
        get_scenarios("without any options", vec![]),
        get_scenarios("specifying files", args!["src/a.ts"]),
        get_scenarios("specifying project", args!["-p", "."]),
        get_scenarios(
            "mixing project and files",
            args!["-p", ".", "src/a.ts", "c.ts"],
        ),
    ]
    .concat()
}

#[test]
fn tsc_ignore_config() {
    run_tsc_inputs(
        "ignoreConfig",
        tsc_ignore_config_inputs(),
        WatchFilter::NonWatch,
    );
}

// Go: tsc_test.go:1259 TestTscIncremental
fn tsc_incremental_inputs() -> Vec<TscInput> {
    // Go: tsc_test.go:1359 libWithReadonlyArray (ts#64452)
    let lib_with_readonly_array = tsc_default_lib_content().replacen(
        "interface ReadonlyArray<T> {}",
        "interface ReadonlyArray<T> { readonly length: number; readonly [n: number]: T; }",
        1,
    );
    // Go: tsc_test.go:1360 getRecursiveTypeTest (ts#64452)
    let get_recursive_type_test = |name: &str, source: &str| -> TscInput {
        TscInput {
            sub_scenario: format!("{name} after comment only edit"),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => r#"{"compilerOptions": {"strict": true, "noEmit": true, "incremental": true}}"#,
                "/home/src/workspaces/project/repro.ts" => dedent(source),
                format!("{TSC_LIB_PATH}/lib.es2026.full.d.ts") => lib_with_readonly_array.clone(),
            },
            edits: vec![
                TscEdit {
                    caption: "add a comment".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file(
                            "/home/src/workspaces/project/repro.ts",
                            "\n// comment-only edit\n",
                        );
                    }),
                    ..Default::default()
                },
                no_change(),
                TscEdit {
                    caption: "add another comment".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file(
                            "/home/src/workspaces/project/repro.ts",
                            "\n// another comment\n",
                        );
                    }),
                    ..Default::default()
                },
                no_change(),
            ],
            ..Default::default()
        }
    };
    // Go: tsc_test.go:1386 getConstEnumTest
    fn get_const_enum_test(
        bds_contents: &str,
        change_enum_file: &str,
        test_suffix: &str,
    ) -> TscInput {
        TscInput {
            sub_scenario: format!("{}{}", "const enums", test_suffix),
            files: file_map! {
                "/home/src/workspaces/project/a.ts" => dedent(r#"
					import {A} from "./c"
					let a = A.ONE
				"#),
                "/home/src/workspaces/project/b.d.ts" => dedent(bds_contents),
                "/home/src/workspaces/project/c.ts" => dedent(r#"
					import {A} from "./b"
					let b = A.ONE
					export {A}
				"#),
                "/home/src/workspaces/project/worker.d.ts" => dedent(r"
					export const enum AWorker {
						ONE = 1
					}
				"),
            },
            command_line_args: args!["-i", "a.ts", "--tsbuildinfofile", "a.tsbuildinfo"],
            edits: vec![
                TscEdit {
                    caption: "change enum value".into(),
                    edit: edit({
                        let change_enum_file = change_enum_file.to_string();
                        move |sys: &TestSys| {
                            sys.replace_file_text(&change_enum_file, "1", "2");
                        }
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "change enum value again".into(),
                    edit: edit({
                        let change_enum_file = change_enum_file.to_string();
                        move |sys: &TestSys| {
                            sys.replace_file_text(&change_enum_file, "2", "3");
                        }
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "something else changes in b.d.ts".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file(
                            "/home/src/workspaces/project/b.d.ts",
                            "export const randomThing = 10;",
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "something else changes in b.d.ts again".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file(
                            "/home/src/workspaces/project/b.d.ts",
                            "export const randomThing2 = 10;",
                        );
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    vec![
        TscInput {
            sub_scenario: "serializing error chain".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
                    "compilerOptions": {
                        "incremental": true,
                        "strict": true,
                        "jsx": "react",
                        "module": "esnext",
                    },
                }"#),
                "/home/src/workspaces/project/index.tsx" => dedent(r"
                    declare namespace JSX {
                        interface ElementChildrenAttribute { children: {}; }
                        interface IntrinsicElements { div: {} }
                    }

                    declare var React: any;

                    declare function Component(props: never): any;
                    declare function Component(props: { children?: number }): any;
                    (<Component>
                        <div />
                        <div />
                    </Component>)"),
            },
            edits: no_change_only_edit(),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "serializing composite project".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
                    "compilerOptions": {
                        "composite": true,
                        "strict": true,
                        "module": "esnext",
                    },
                }"#),
                "/home/src/workspaces/project/index.tsx" => "export const a = 1;",
                "/home/src/workspaces/project/other.ts" => "export const b = 2;",
            },
            ..Default::default()
        },
        TscInput {
            sub_scenario: "change to modifier of class expression field with declaration emit enabled".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{ 
					"compilerOptions": {
						"module": "esnext",
						"declaration": true
					}
				}"#),
                "/home/src/workspaces/project/main.ts" => dedent(r"
                        import MessageablePerson from './MessageablePerson.js';
                        function logMessage( person: MessageablePerson ) {
                            console.log( person.message );
                        }"),
                "/home/src/workspaces/project/MessageablePerson.ts" => dedent(r"
                        const Messageable = () => {
                            return class MessageableClass {
                                public message = 'hello';
                            }
                        };
                        const wrapper = () => Messageable();
                        type MessageablePerson = InstanceType<ReturnType<typeof wrapper>>;
                        export default MessageablePerson;"),
                format!("{}{}", TSC_LIB_PATH, "/lib.d.ts") => format!("{}{}{}", tsc_default_lib_content(), "\n", dedent(r"
					type ReturnType<T extends (...args: any) => any> = T extends (...args: any) => infer R ? R : any;
                    type InstanceType<T extends abstract new (...args: any) => any> = T extends abstract new (...args: any) => infer R ? R : any;")),
            },
            command_line_args: args!["--incremental"],
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "modify public to protected".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/MessageablePerson.ts", "public", "protected");
                    }),
                    ..Default::default()
                },
                no_change(),
                TscEdit {
                    caption: "modify protected to public".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/MessageablePerson.ts", "protected", "public");
                    }),
                    ..Default::default()
                },
                no_change(),
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "change to modifier of class expression field".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{ 
					"compilerOptions": { 
						"module": "esnext"
					}
				}"#),
                "/home/src/workspaces/project/main.ts" => dedent(r"
					import MessageablePerson from './MessageablePerson.js';
					function logMessage( person: MessageablePerson ) {
						console.log( person.message );
					}"),
                "/home/src/workspaces/project/MessageablePerson.ts" => dedent(r"
					const Messageable = () => {
						return class MessageableClass {
							public message = 'hello';
						}
					};
					const wrapper = () => Messageable();
					type MessageablePerson = InstanceType<ReturnType<typeof wrapper>>;
					export default MessageablePerson;"),
                format!("{}{}", TSC_LIB_PATH, "/lib.d.ts") => format!("{}{}{}", tsc_default_lib_content(), "\n", dedent(r"
					type ReturnType<T extends (...args: any) => any> = T extends (...args: any) => infer R ? R : any;
                    type InstanceType<T extends abstract new (...args: any) => any> = T extends abstract new (...args: any) => infer R ? R : any;")),
            },
            command_line_args: args!["--incremental"],
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "modify public to protected".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/MessageablePerson.ts", "public", "protected");
                    }),
                    ..Default::default()
                },
                no_change(),
                TscEdit {
                    caption: "modify protected to public".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/MessageablePerson.ts", "protected", "public");
                    }),
                    ..Default::default()
                },
                no_change(),
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when passing filename for buildinfo on commandline".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.ts" => "export const x = 10;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
                    "compilerOptions": {
                        "target": "es5",
                        "module": "commonjs"
                    },
                    "include": [
                        "src/**/*.ts"
                    ],
                }"#),
            },
            command_line_args: args!["--incremental", "--tsBuildInfoFile", ".tsbuildinfo", "--explainFiles"],
            edits: no_change_only_edit(),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when passing rootDir from commandline".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.ts" => "export const x = 10;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
                    "compilerOptions": {
                        "incremental": true,
                        "outDir": "dist"
                    }
                }"#),
            },
            command_line_args: args!["--rootDir", "src"],
            edits: no_change_only_edit(),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "with only dts files".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.d.ts" => "export const x = 10;",
                "/home/src/workspaces/project/src/another.d.ts" => "export const y = 10;",
                "/home/src/workspaces/project/tsconfig.json" => "{}",
            },
            command_line_args: args!["--incremental"],
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "modify d.ts file".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/src/main.d.ts", "export const xy = 100;");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when passing rootDir is in the tsconfig".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.ts" => "export const x = 10;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
                    "compilerOptions": {
                        "incremental": true,
                        "outDir": "dist",
						"rootDir": "./"
                    }
                }"#),
            },
            edits: no_change_only_edit(),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "tsbuildinfo has error".into(),
            files: file_map! {
                "/home/src/workspaces/project/main.ts" => "export const x = 10;",
                "/home/src/workspaces/project/tsconfig.json" => "{}",
                "/home/src/workspaces/project/tsconfig.tsbuildinfo" => "Some random string",
            },
            command_line_args: args!["-i"],
            edits: vec![
                TscEdit {
                    caption: "tsbuildinfo written has error".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.prepend_file("/home/src/workspaces/project/tsconfig.tsbuildinfo", "Some random string");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when global file is added, the signatures are updated".into(),
            files: file_map! {
                "/home/src/workspaces/project/src/main.ts" => dedent(r#"
                    /// <reference path="./filePresent.ts"/>
                    /// <reference path="./fileNotFound.ts"/>
                    function main() { }
                "#),
                "/home/src/workspaces/project/src/anotherFileWithSameReferenes.ts" => dedent(r#"
                    /// <reference path="./filePresent.ts"/>
                    /// <reference path="./fileNotFound.ts"/>
                    function anotherFileWithSameReferenes() { }
                "#),
                "/home/src/workspaces/project/src/filePresent.ts" => "function something() { return 10; }",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
                    "compilerOptions": { "composite": true },
                    "include": ["src/**/*.ts"],
                }"#),
            },
            command_line_args: vec![],
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "Modify main file".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/src/main.ts", "something();");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "Modify main file again".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/src/main.ts", "something();");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "Add new file and update main file".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.write_file_no_error("/home/src/workspaces/project/src/newFile.ts", "function foo() { return 20; }");
                        sys.prepend_file("/home/src/workspaces/project/src/main.ts", r#"/// <reference path="./newFile.ts"/>
"#);
                        sys.append_file("/home/src/workspaces/project/src/main.ts", "foo();");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "Write file that could not be resolved".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.write_file_no_error("/home/src/workspaces/project/src/fileNotFound.ts", "function something2() { return 20; }");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "Modify main file".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/src/main.ts", "something();");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "react-jsx-emit-mode with no backing types found doesnt crash".into(),
            files: file_map! {
                "/home/src/workspaces/project/node_modules/react/jsx-runtime.js" => "export {}", // js needs to be present so there's a resolution result
                "/home/src/workspaces/project/node_modules/@types/react/index.d.ts" => dedent(r"
					export {};
					declare global {
						namespace JSX {
							interface Element {}
							interface IntrinsicElements {
								div: {
									propA?: boolean;
								};
							}
						}
					}"), // doesn't contain a jsx-runtime definition
                "/home/src/workspaces/project/src/index.tsx" => "export const App = () => <div propA={true}></div>;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{ 
					"compilerOptions": { 
						"module": "commonjs",
						"jsx": "react-jsx", 
						"incremental": true, 
						"jsxImportSource": "react" 
					} 
				}"#),
            },
            ..Default::default()
        },
        TscInput {
            sub_scenario: "react-jsx-emit-mode with no backing types found doesnt crash under --strict".into(),
            files: file_map! {
                "/home/src/workspaces/project/node_modules/react/jsx-runtime.js" => "export {}", // js needs to be present so there's a resolution result
                "/home/src/workspaces/project/node_modules/@types/react/index.d.ts" => dedent(r"
					export {};
					declare global {
						namespace JSX {
							interface Element {}
							interface IntrinsicElements {
								div: {
									propA?: boolean;
								};
							}
						}
					}"), // doesn't contain a jsx-runtime definition
                "/home/src/workspaces/project/src/index.tsx" => "export const App = () => <div propA={true}></div>;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{ 
					"compilerOptions": { 
						"module": "commonjs",
						"jsx": "react-jsx", 
						"incremental": true, 
						"jsxImportSource": "react" 
					} 
				}"#),
            },
            command_line_args: args!["--strict"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "change to type that gets used as global through export in another file".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true
					}
				}"#),
                "/home/src/workspaces/project/class1.ts" => dedent(r"
					const a: MagicNumber = 1;
					console.log(a);"),
                "/home/src/workspaces/project/constants.ts" => "export default 1;",
                "/home/src/workspaces/project/types.d.ts" => "type MagicNumber = typeof import('./constants').default",
            },
            edits: vec![
                TscEdit {
                    caption: "Modify imports used in global file".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.write_file_no_error("/home/src/workspaces/project/constants.ts", "export default 2;");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "change to type that gets used as global through export in another file through indirect import".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true
					}
				}"#),
                "/home/src/workspaces/project/class1.ts" => dedent(r"
					const a: MagicNumber = 1;
					console.log(a);"),
                "/home/src/workspaces/project/constants.ts" => "export default 1;",
                "/home/src/workspaces/project/reexport.ts" => r#"export { default as ConstantNumber } from "./constants""#,
                "/home/src/workspaces/project/types.d.ts" => "type MagicNumber = typeof import('./reexport').ConstantNumber",
            },
            edits: vec![
                TscEdit {
                    caption: "Modify imports used in global file".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.write_file_no_error("/home/src/workspaces/project/constants.ts", "export default 2;");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when file is deleted".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "outDir"
					}
				}"#),
                "/home/src/workspaces/project/file1.ts" => "export class  C { }",
                "/home/src/workspaces/project/file2.ts" => "export class D { }",
            },
            edits: vec![
                TscEdit {
                    caption: "delete file with imports".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.remove_no_error("/home/src/workspaces/project/file2.ts");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "generates typerefs correctly".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "outDir",
						"checkJs": true
					},
					"include": ["src"],
				}"#),
                "/home/src/workspaces/project/src/box.ts" => dedent(r"
                    export interface Box<T> {
                        unbox(): T
                    }
                "),
                "/home/src/workspaces/project/src/bug.js" => dedent(r#"
                    import * as B from "./box.js"
                    import * as W from "./wrap.js"

                    /**
                     * @template {object} C
                     * @param {C} source
                     * @returns {W.Wrap<C>}
                     */
                    const wrap = source => {
                    throw source
                    }

                    /**
                     * @returns {B.Box<number>}
                     */
                    const box = (n = 0) => ({ unbox: () => n })

                    export const bug = wrap({ n: box(1) });
                "#),
                "/home/src/workspaces/project/src/wrap.ts" => dedent(r"
                    export type Wrap<C> = {
                        [K in keyof C]: { wrapped: C[K] }
                    }
                "),
            },
            edits: vec![
                TscEdit {
                    caption: "modify js file".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/src/bug.js", "export const something = 1;");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        get_const_enum_test(r"
			export const enum A {
				ONE = 1
			}
		", "/home/src/workspaces/project/b.d.ts", ""),
        get_const_enum_test(r"
			export const enum AWorker {
				ONE = 1
			}
			export { AWorker as A };
		", "/home/src/workspaces/project/b.d.ts", " aliased"),
        get_const_enum_test(r#"export { AWorker as A } from "./worker";"#, "/home/src/workspaces/project/worker.d.ts", " aliased in different file"),
        TscInput {
            sub_scenario: "option changes with composite".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
					}
				}"#),
                "/home/src/workspaces/project/a.ts" => "export const a = 10;const aLocal = 10;",
                "/home/src/workspaces/project/b.ts" => "export const b = 10;const bLocal = 10;",
                "/home/src/workspaces/project/c.ts" => r#"import { a } from "./a";export const c = a;"#,
                "/home/src/workspaces/project/d.ts" => r#"import { b } from "./b";export const d = b;"#,
            },
            edits: vec![
                TscEdit {
                    caption: "with sourceMap".into(),
                    command_line_args: Some(args!["--sourceMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "should re-emit only js so they dont contain sourcemap".into(),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with declaration should not emit anything".into(),
                    command_line_args: Some(args!["--declaration"]),
                    // discrepancyExplanation: () => [
                    // 	`Clean build tsbuildinfo will have compilerOptions with composite and ${option.replace(/-/g, "")}`,
                    // 	`Incremental build will detect that it doesnt need to rebuild so tsbuild info is from before which has option composite only`,
                    // ],
                    ..Default::default()
                },
                no_change(),
                TscEdit {
                    caption: "with declaration and declarationMap".into(),
                    command_line_args: Some(args!["--declaration", "--declarationMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "should re-emit only dts so they dont contain sourcemap".into(),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with emitDeclarationOnly should not emit anything".into(),
                    command_line_args: Some(args!["--emitDeclarationOnly"]),
                    // discrepancyExplanation: () => [
                    // 	`Clean build tsbuildinfo will have compilerOptions with composite and ${option.replace(/-/g, "")}`,
                    // 	`Incremental build will detect that it doesnt need to rebuild so tsbuild info is from before which has option composite only`,
                    // ],
                    ..Default::default()
                },
                no_change(),
                TscEdit {
                    caption: "local change".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/a.ts", "Local = 1", "Local = 10");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with declaration should not emit anything".into(),
                    command_line_args: Some(args!["--declaration"]),
                    // discrepancyExplanation: () => [
                    // 	`Clean build tsbuildinfo will have compilerOptions with composite and ${option.replace(/-/g, "")}`,
                    // 	`Incremental build will detect that it doesnt need to rebuild so tsbuild info is from before which has option composite only`,
                    // ],
                    ..Default::default()
                },
                TscEdit {
                    caption: "with inlineSourceMap".into(),
                    command_line_args: Some(args!["--inlineSourceMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with sourceMap".into(),
                    command_line_args: Some(args!["--sourceMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "declarationMap enabling".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/tsconfig.json", r#""composite": true,"#, r#""composite": true,        "declarationMap": true"#);
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with sourceMap should not emit d.ts".into(),
                    command_line_args: Some(args!["--sourceMap"]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "option changes with incremental".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"incremental": true,
					}
				}"#),
                "/home/src/workspaces/project/a.ts" => "export const a = 10;const aLocal = 10;",
                "/home/src/workspaces/project/b.ts" => "export const b = 10;const bLocal = 10;",
                "/home/src/workspaces/project/c.ts" => r#"import { a } from "./a";export const c = a;"#,
                "/home/src/workspaces/project/d.ts" => r#"import { b } from "./b";export const d = b;"#,
            },
            edits: vec![
                TscEdit {
                    caption: "with sourceMap".into(),
                    command_line_args: Some(args!["--sourceMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "should re-emit only js so they dont contain sourcemap".into(),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with declaration, emit Dts and should not emit js".into(),
                    command_line_args: Some(args!["--declaration"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with declaration and declarationMap".into(),
                    command_line_args: Some(args!["--declaration", "--declarationMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "no change".into(),
                    // discrepancyExplanation: () => [
                    // 	`Clean build tsbuildinfo will have compilerOptions {}`,
                    // 	`Incremental build will detect that it doesnt need to rebuild so tsbuild info is from before which has option declaration and declarationMap`,
                    // ],
                    ..Default::default()
                },
                TscEdit {
                    caption: "local change".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/a.ts", "Local = 1", "Local = 10");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with declaration and declarationMap".into(),
                    command_line_args: Some(args!["--declaration", "--declarationMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "no change".into(),
                    // discrepancyExplanation: () => [
                    // 	`Clean build tsbuildinfo will have compilerOptions {}`,
                    // 	`Incremental build will detect that it doesnt need to rebuild so tsbuild info is from before which has option declaration and declarationMap`,
                    // ],
                    ..Default::default()
                },
                TscEdit {
                    caption: "with inlineSourceMap".into(),
                    command_line_args: Some(args!["--inlineSourceMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with sourceMap".into(),
                    command_line_args: Some(args!["--sourceMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "emit js files".into(),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with declaration and declarationMap".into(),
                    command_line_args: Some(args!["--declaration", "--declarationMap"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "with declaration and declarationMap, should not re-emit".into(),
                    command_line_args: Some(args!["--declaration", "--declarationMap"]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when there is bind diagnostics thats ignored".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"skipLibCheck": true,
						"incremental": true,
					}
				}"#),
                "/home/src/workspaces/project/a.ts" => "export const a = 10;",
                "/home/src/workspaces/project/b.d.ts" => dedent(r"
					interface NoName {
						Profiler: new ({ sampleInterval: number, maxBufferSize: number }) => {
							stop: () => Promise<any>;
						};
					}
				"),
            },
            command_line_args: args![""],
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "no change and tsc -b".into(),
                    command_line_args: Some(args!["-b", "-v"]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Compile incremental with case insensitive file names".into(),
            command_line_args: args!["-p", "."],
            files: file_map! {
                "/home/project/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"incremental": true
						},
					}"#),
                "/home/project/src/index.ts" => dedent(r#"
					import type { Foo1 } from 'lib1';
					import type { Foo2 } from 'lib2';
					export const foo1: Foo1 = { foo: "a" };
					export const foo2: Foo2 = { foo: "b" };"#),
                "/home/node_modules/lib1/index.d.ts" => dedent(r"
					import type { Foo } from 'someLib';
					export type { Foo as Foo1 };"),
                "/home/node_modules/lib1/package.json" => dedent(r#"
					{
						"name": "lib1"
					}"#),
                "/home/node_modules/lib2/index.d.ts" => dedent(r"
					import type { Foo } from 'somelib';
					export type { Foo as Foo2 };
					export declare const foo2: Foo;"),
                "/home/node_modules/lib2/package.json" => dedent(r#"
					{
						"name": "lib2"
					}
					"#),
                "/home/node_modules/someLib/index.d.ts" => dedent(r"
					import type { Str } from 'otherLib';
					export type Foo = { foo: Str; };"),
                "/home/node_modules/someLib/package.json" => dedent(r#"
					{
						"name": "somelib"
					}"#),
                "/home/node_modules/otherLib/index.d.ts" => dedent(r"
					export type Str = string;"),
                "/home/node_modules/otherLib/package.json" => dedent(r#"
					{
						"name": "otherlib"
					}"#),
            },
            cwd: "/home/project".into(),
            ignore_case: true,
            ..Default::default()
        },
        TscInput {
            sub_scenario: "const enums with refCycle".into(),
            files: file_map! {
                "/home/src/workspaces/project/file.ts" => dedent(r#"
					import {A} from "./c"
					let a = A.ONE
				"#),
                "/home/src/workspaces/project/b.ts" => dedent(r#"
					import { AWorker } from "./aworker"
					import { A as ACycle } from "./c"
					export const enum A {
						ONE = 1
					}
				"#),
                "/home/src/workspaces/project/c.ts" => dedent(r#"
					import {A} from "./b"
					let b = A.ONE
					export {A}
				"#),
                "/home/src/workspaces/project/aworker.ts" => dedent(r"
					export const AWorker  = 10
				"),
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
					}
				}"#),
            },
            command_line_args: vec![],
            edits: vec![
                TscEdit {
                    caption: "change aworker".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/aworker.ts", "10", "20");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "change aworker and enum value".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/aworker.ts", "20", "30");
                        sys.replace_file_text("/home/src/workspaces/project/b.ts", "1", "2");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "internal symbolname in tsbuildInfo".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"target": "es2017",
						"strict": true,
						"esModuleInterop": true
					}
				}"#),
                "/home/src/workspaces/project/a.ts" => dedent(r"
					const createFileListFromFiles = (files: File[]): FileList => {
					const fileList: FileList = {
						length: files.length,
						item: (index: number): File | null => files[index] || null,
						[Symbol.iterator]: function* (): IterableIterator<File> {
						for (const file of files) yield file;
						},
						...files,
					} as unknown as FileList;

					return fileList;
					};
				"),
                get_test_lib_path_for("es2015.iterable") => dedent(r"
					interface SymbolConstructor {
						readonly iterator: unique symbol;
					}
					interface IteratorYieldResult<TYield> {
						done?: false;
						value: TYield;
					}
					interface IteratorReturnResult<TReturn> {
						done: true;
						value: TReturn;
					}
					type IteratorResult<T, TReturn = any> = IteratorYieldResult<T> | IteratorReturnResult<TReturn>;
					interface Iterator<T, TReturn = any, TNext = any> {
						// NOTE: 'next' is defined using a tuple to ensure we report the correct assignability errors in all places.
						next(...[value]: [] | [TNext]): IteratorResult<T, TReturn>;
						return?(value?: TReturn): IteratorResult<T, TReturn>;
						throw?(e?: any): IteratorResult<T, TReturn>;
					}
					interface Iterable<T, TReturn = any, TNext = any> {
						[Symbol.iterator](): Iterator<T, TReturn, TNext>;
					}
					interface IterableIterator<T, TReturn = any, TNext = any> extends Iterator<T, TReturn, TNext> {
						[Symbol.iterator](): IterableIterator<T, TReturn, TNext>;
					}
					interface IteratorObject<T, TReturn = unknown, TNext = unknown> extends Iterator<T, TReturn, TNext> {
						[Symbol.iterator](): IteratorObject<T, TReturn, TNext>;
					}
					type BuiltinIteratorReturn = intrinsic;
					interface ArrayIterator<T> extends IteratorObject<T, BuiltinIteratorReturn, unknown> {
						[Symbol.iterator](): ArrayIterator<T>;
					}
					interface Array<T> {
						[Symbol.iterator](): ArrayIterator<T>;
						entries(): ArrayIterator<[number, T]>;
						keys(): ArrayIterator<number>;
						values(): ArrayIterator<T>;
					}
				"),
                get_test_lib_path_for("es2017.full") => format!("{}{}", dedent(r#"
					/// <reference lib="es2015.iterable"/>
					interface File {
					}
					interface FileList {
						readonly length: number;
						item(index: number): File | null;
						[index: number]: File;
						[Symbol.iterator](): ArrayIterator<File>;
					}
				"#), tsc_default_lib_content()),
            },
            command_line_args: args![""],
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "no change with incremental".into(),
                    command_line_args: Some(args!["--incremental"]),
                    ..Default::default()
                },
                TscEdit {
                    caption: "no change with incremental that reads buildInfo".into(),
                    command_line_args: Some(args!["--incremental"]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "js file with import in jsdoc in composite project".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => r#"{"compilerOptions": {"allowJs": true, "composite": true}}"#,
                "/home/src/workspaces/project/index.js" => dedent(r#"
					test("", async function () {
					  ;(/** @type {typeof import("a")} */ ({}))
					})

					test("", async function () {
					  ;(/** @type {typeof import("a")} */ a)
					})

					test("", async function () {
					  (/** @type {typeof import("a")} */ ({}))
					  ;(/** @type {typeof import("a")} */ ({}))
					})

					test("", async function () {
					  (/** @type {typeof import("a")} */ a)
					  ;(/** @type {typeof import("a")} */ a)
					})

					test("", async function () {
					  (/** @type {typeof import("a")} */ ({}))
					  ;(/** @type {typeof import("a")} */ ({}))
					})
				"#),
            },
            command_line_args: args!["--noEmit"],
            ..Default::default()
        },
        get_recursive_type_test("recursive mapped type", r#"
			type Json = string | Json[];
			type Parsed<T> = T extends object ? { [K in keyof T]: Parsed<T[K]> } : T;
			declare function wrap<T>(value: T): Parsed<T>;
			export const value = wrap({ items: [] as Json[] });
		"#),
        get_recursive_type_test("recursive readonly mapped type", r#"
			type Json = string | readonly Json[];
			type Parsed<T> = T extends object ? { [K in keyof T]: Parsed<T[K]> } : T;
			declare function wrap<T>(value: T): Parsed<T>;
			export const value = wrap({ items: [] as readonly Json[] });
		"#),
        TscInput {
            sub_scenario: "global diagnostics produced during semantic checking".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => r#"{"compilerOptions": {"noEmit": true, "incremental": true}}"#,
                "/home/src/workspaces/project/repro.ts" => "export function* values() { yield 1; }",
            },
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "add a comment".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/repro.ts", "\n// comment-only edit\n");
                    }),
                    expected_diff: "Like Strada, signature generation produces the missing-global diagnostic before semantic checking, so it is excluded from the file's semantic diagnostics.".into(),
                    ..Default::default()
                },
                TscEdit {
                    caption: "no change".into(),
                    edit: no_change().edit,
                    expected_diff: "Like Strada, the cached semantic diagnostics do not include the missing-global diagnostic produced during signature generation.".into(),
                    ..Default::default()
                },
                TscEdit {
                    caption: "delete build info to restore the semantic diagnostic".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.remove_no_error("/home/src/workspaces/project/tsconfig.tsbuildinfo");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "global diagnostics from function bodies after incremental edits".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => r#"{"compilerOptions": {"noEmit": true, "incremental": true}}"#,
                "/home/src/workspaces/project/repro.ts" => dedent(r#"
					export function values() {
						// @ts-ignore
						function* generator() { yield 1; }
					}
				"#),
            },
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "add a comment".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/repro.ts", "\n// comment-only edit\n");
                    }),
                    ..Default::default()
                },
                no_change(),
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "global diagnostics produced during unchecked javascript checking".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => r#"{"compilerOptions": {"allowJs": true, "noEmit": true, "incremental": true}}"#,
                "/home/src/workspaces/project/repro.js" => "export function* values() { yield 1; }",
            },
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "enable javascript checking".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/tsconfig.json", r#""allowJs": true"#, r#""allowJs": true, "checkJs": true"#);
                    }),
                    ..Default::default()
                },
                no_change(),
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "reverse mapped declaration consumption".into(),
            files: file_map! {
                "/home/src/workspaces/project/producer/tsconfig.json" => r#"{
					"compilerOptions": { "strict": true, "composite": true, "outDir": "dist" }
				}"#,
                "/home/src/workspaces/project/producer/index.ts" => dedent(r#"
					declare function unwrap<T>(input: { [K in keyof T]: { value: T[K] } }): T;
					declare const indexedInput: { [key: string]: { value: string } };
					export const indexed = unwrap(indexedInput);
					type Validator<T> = ((input: unknown) => T | undefined) | {
						[K in keyof T]: Validator<T[K]>;
					};
					declare function decode<T>(input: { [K in keyof T]: Validator<T[K]> }): T;
					declare const stringValidator: (input: unknown) => string | undefined;
					export const deep = decode({ a: { b: { c: { d: stringValidator } } } });
					interface NamedInput { leaf: typeof stringValidator }
					declare const namedInput: { node: NamedInput };
					export const named = decode(namedInput);
				"#),
                "/home/src/workspaces/project/consumer/tsconfig.json" => r#"{
					"compilerOptions": { "strict": true, "noEmit": true },
					"references": [{ "path": "../producer" }]
				}"#,
                "/home/src/workspaces/project/consumer/index.ts" => dedent(r#"
					import { indexed, deep, named } from "../producer/dist/index.js";
					const a: string = indexed["name"];
					const b: string = deep.a.b.c.d;
					const c: string = named.node.leaf;
					type IsAny<T> = 0 extends (1 & T) ? true : false;
					const notAny: false = null as unknown as
						IsAny<typeof indexed[string] | typeof deep.a.b.c.d | typeof named.node.leaf>;
					const invalidIndex: number = indexed["name"];
					const invalidDeep: number = deep.a.b.c.d;
					const invalidNamed: number = named.node.leaf;
				"#),
            },
            command_line_args: args!["--build", "consumer"],
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "add a comment to the producer".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/producer/index.ts", "\n// comment-only edit\n");
                    }),
                    ..Default::default()
                },
                no_change(),
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "recursive tagged tuple after incremental edits".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => r#"{"compilerOptions": {"strict": true, "incremental": true, "noEmit": true, "module": "esnext", "moduleResolution": "bundler"}}"#,
                format!("{TSC_LIB_PATH}/lib.es2026.full.d.ts") => lib_with_readonly_array.clone(),
                "/home/src/workspaces/project/doc.ts" => dedent(r#"
					type Doc =
						| string
						| { [k: string]: Doc }
						| readonly ["array", Doc]
						| readonly ["array", Doc, { length: number }]
						| readonly ["array", Doc, { min?: number; max?: number }]
						| readonly ["union", Doc, ...Doc[]];
					export declare const doc: Doc;
				"#),
                "/home/src/workspaces/project/consumer.ts" => dedent(r#"
					import { doc } from "./doc";
					export const value = doc;
				"#),
            },
            edits: vec![
                no_change(),
                TscEdit {
                    caption: "add a comment to the recursive type".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/doc.ts", "\n// comment-only edit\n");
                    }),
                    ..Default::default()
                },
                no_change(),
                TscEdit {
                    caption: "add a union constituent".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.replace_file_text("/home/src/workspaces/project/doc.ts", "| string", "| number\n    | string");
                    }),
                    ..Default::default()
                },
                no_change(),
                TscEdit {
                    caption: "verify the consumer type was not weakened".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.append_file("/home/src/workspaces/project/consumer.ts", "\nexport const invalid: number = value;\n");
                    }),
                    ..Default::default()
                },
                no_change(),
                TscEdit {
                    caption: "delete build info and check the edited source afresh".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.remove_no_error("/home/src/workspaces/project/tsconfig.tsbuildinfo");
                    }),
                    ..Default::default()
                },
                no_change(),
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "json module diagnostics are cleared after fixing the json file".into(),
            files: file_map! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"strict": true,
							"noEmit": true,
							"incremental": true,
							"resolveJsonModule": true,
							"esModuleInterop": true
						}
					}"#),
                "/home/src/workspaces/project/data.json" => r#"{ "title": "hello" }"#,
                "/home/src/workspaces/project/check.ts" => dedent(r#"
					import type data from "./data.json";

					type Shape = { title: string };
					type Covers<T extends Shape> = T;

					export type Check = Covers<typeof data>;
				"#),
            },
            edits: vec![
                TscEdit {
                    caption: "remove required property".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.write_file_no_error("/home/src/workspaces/project/data.json", "{}");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "restore required property".into(),
                    edit: edit(|sys: &TestSys| {
                        sys.write_file_no_error("/home/src/workspaces/project/data.json", r#"{ "title": "fixed" }"#);
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    ]
}

#[test]
fn tsc_incremental() {
    run_tsc_inputs(
        "incremental",
        tsc_incremental_inputs(),
        WatchFilter::NonWatch,
    );
}

// Go: tsc_test.go:2189 TestTscLibraryResolution
fn tsc_library_resolution_inputs() -> Vec<TscInput> {
    // Go: tsc_test.go:2191 getTscLibraryResolutionFileMap
    fn get_tsc_library_resolution_file_map(lib_replacement: bool) -> FileMap {
        let mut files = file_map! {
            "/home/src/workspace/projects/project1/utils.d.ts" => "export const y = 10;",
            "/home/src/workspace/projects/project1/file.ts" => "export const file = 10;",
            "/home/src/workspace/projects/project1/core.d.ts" => "export const core = 10;",
            "/home/src/workspace/projects/project1/index.ts" => r#"export const x = "type1";"#,
            "/home/src/workspace/projects/project1/file2.ts" => dedent(r#"
				/// <reference lib="webworker"/>
				/// <reference lib="scripthost"/>
				/// <reference lib="es5"/>
			"#),
            "/home/src/workspace/projects/project1/tsconfig.json" => dedent(&go_sprintf(r#"
				{
					"compilerOptions": {
						"composite": true,
						"typeRoots": ["./typeroot1"],
						"lib": ["es5", "dom"],
						"traceResolution": true,
						"libReplacement": %t
					}
				}
			"#, &[&lib_replacement])),
            "/home/src/workspace/projects/project1/typeroot1/sometype/index.d.ts" => r#"export type TheNum = "type1";"#,
            "/home/src/workspace/projects/project2/utils.d.ts" => "export const y = 10;",
            "/home/src/workspace/projects/project2/index.ts" => "export const y = 10",
            "/home/src/workspace/projects/project2/tsconfig.json" => dedent(&go_sprintf(r#"
				{
					"compilerOptions": {
						"composite": true,
						"lib": ["es5", "dom"],
						"traceResolution": true,
						"libReplacement": %t
					}
				}
			"#, &[&lib_replacement])),
            "/home/src/workspace/projects/project3/utils.d.ts" => "export const y = 10;",
            "/home/src/workspace/projects/project3/index.ts" => "export const z = 10",
            "/home/src/workspace/projects/project3/tsconfig.json" => dedent(&go_sprintf(r#"
				{
					"compilerOptions": {
						"composite": true,
						"lib": ["es5", "dom"],
						"traceResolution": true,
						"libReplacement": %t
					}
				}
			"#, &[&lib_replacement])),
            "/home/src/workspace/projects/project4/utils.d.ts" => "export const y = 10;",
            "/home/src/workspace/projects/project4/index.ts" => "export const z = 10",
            "/home/src/workspace/projects/project4/tsconfig.json" => dedent(&go_sprintf(r#"
				{
					"compilerOptions": {
						"composite": true,
						"lib": ["esnext", "dom", "webworker"],
						"traceResolution": true,
						"libReplacement": %t
					}
				}
			"#, &[&lib_replacement])),
            get_test_lib_path_for("dom") => "interface DOMInterface { }",
            get_test_lib_path_for("webworker") => "interface WebWorkerInterface { }",
            get_test_lib_path_for("scripthost") => "interface ScriptHostInterface { }",
            "/home/src/workspace/projects/node_modules/@typescript/unlreated/index.d.ts" => "export const unrelated = 10;",
        };
        if lib_replacement {
            files.insert(
                "/home/src/workspace/projects/node_modules/@typescript/lib-es5/index.d.ts".into(),
                tsc_default_lib_content().into(),
            );
            files.insert(
                "/home/src/workspace/projects/node_modules/@typescript/lib-esnext/index.d.ts"
                    .into(),
                tsc_default_lib_content().into(),
            );
            files.insert(
                "/home/src/workspace/projects/node_modules/@typescript/lib-dom/index.d.ts".into(),
                "interface DOMInterface { }".into(),
            );
            files.insert(
                "/home/src/workspace/projects/node_modules/@typescript/lib-webworker/index.d.ts"
                    .into(),
                "interface WebWorkerInterface { }".into(),
            );
            files.insert(
                "/home/src/workspace/projects/node_modules/@typescript/lib-scripthost/index.d.ts"
                    .into(),
                "interface ScriptHostInterface { }".into(),
            );
        }
        files
    }

    // Go: tsc_test.go:2264 getTscLibResolutionTestCases
    fn get_tsc_lib_resolution_test_cases(command_line_args: Vec<String>) -> Vec<TscInput> {
        vec![
            TscInput {
                sub_scenario: "with config".into(),
                files: get_tsc_library_resolution_file_map(false),
                cwd: "/home/src/workspace/projects".into(),
                command_line_args: command_line_args.clone(),
                ..Default::default()
            },
            TscInput {
                sub_scenario: "with config with libReplacement".into(),
                files: get_tsc_library_resolution_file_map(true),
                cwd: "/home/src/workspace/projects".into(),
                command_line_args: command_line_args.clone(),
                ..Default::default()
            },
        ]
    }

    // Go: tsc_test.go:2280 getTscLibraryResolutionUnknown
    fn get_tsc_library_resolution_unknown() -> FileMap {
        file_map! {
            "/home/src/workspace/projects/project1/utils.d.ts" => "export const y = 10;",
            "/home/src/workspace/projects/project1/file.ts" => "export const file = 10;",
            "/home/src/workspace/projects/project1/core.d.ts" => "export const core = 10;",
            "/home/src/workspace/projects/project1/index.ts" => r#"export const x = "type1";"#,
            "/home/src/workspace/projects/project1/file2.ts" => dedent(r#"
				/// <reference lib="webworker2"/>
				/// <reference lib="unknownlib"/>
				/// <reference lib="scripthost"/>
			"#),
            "/home/src/workspace/projects/project1/tsconfig.json" => dedent(r#"
			{
				"compilerOptions": {
					"composite": true,
					"traceResolution": true,
					"libReplacement": true
				}
			}"#),
            get_test_lib_path_for("webworker") => "interface WebWorkerInterface { }",
            get_test_lib_path_for("scripthost") => "interface ScriptHostInterface { }",
        }
    }

    [
        get_tsc_lib_resolution_test_cases(args![
            "-b",
            "project1",
            "project2",
            "project3",
            "project4",
            "--verbose",
            "--explainFiles"
        ]),
        get_tsc_lib_resolution_test_cases(args!["-p", "project1", "--explainFiles"]),
        get_tsc_lib_resolution_test_cases(args![
            "-b",
            "-w",
            "project1",
            "project2",
            "project3",
            "project4",
            "--verbose",
            "--explainFiles"
        ]),
        vec![
            TscInput {
                sub_scenario: "unknown lib".into(),
                files: get_tsc_library_resolution_unknown(),
                cwd: "/home/src/workspace/projects".into(),
                command_line_args: args!["-p", "project1", "--explainFiles"],
                ..Default::default()
            },
            TscInput {
                sub_scenario: "when noLib toggles".into(),
                files: file_map! {
                    "/home/src/workspaces/project/a.d.ts" => r#"declare const a = "hello";"#,
                    "/home/src/workspaces/project/b.ts" => "const b = 10;",
                    "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
                    {
                        "compilerOptions": {
                            "declaration": true,
                            "incremental": true,
                            "lib": ["es6"],
                        },
                    }
                "#),
                },
                edits: vec![TscEdit {
                    caption: "with --noLib".into(),
                    command_line_args: Some(args!["--noLib"]),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
    ]
    .concat()
}

#[test]
fn tsc_library_resolution() {
    run_tsc_inputs(
        "libraryResolution",
        tsc_library_resolution_inputs(),
        WatchFilter::NonWatch,
    );
}

#[test]
fn tsc_library_resolution_watch() {
    run_tsc_inputs(
        "libraryResolution",
        tsc_library_resolution_inputs(),
        WatchFilter::WatchOnly,
    );
}

// Go: tsc_test.go:2344 TestTscListFilesOnly
fn tsc_list_files_only_inputs() -> Vec<TscInput> {
    vec![
        TscInput {
            sub_scenario: "loose file".into(),
            files: file_map! {
                "/home/src/workspaces/project/test.ts" => "export const x = 1;",
            },
            command_line_args: args!["test.ts", "--listFilesOnly"],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "combined with incremental".into(),
            files: file_map! {
                "/home/src/workspaces/project/test.ts" => "export const x = 1;",
                "/home/src/workspaces/project/tsconfig.json" => "{}",
            },
            command_line_args: args!["--incremental", "--listFilesOnly"],
            edits: vec![
                TscEdit {
                    caption: "incremental actual build".into(),
                    command_line_args: Some(args!["--incremental"]),
                    ..Default::default()
                },
                no_change(),
                TscEdit {
                    caption: "incremental should not build".into(),
                    command_line_args: Some(args!["--incremental"]),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    ]
}

#[test]
fn tsc_list_files_only() {
    run_tsc_inputs(
        "listFilesOnly",
        tsc_list_files_only_inputs(),
        WatchFilter::NonWatch,
    );
}
