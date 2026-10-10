//! Port of the typescript-go `internal/execute/tsctests/tsc_test.go` lines 2380-4614:
//! `TestTscModuleResolution`, `TestTscNoCheck`, `TestTscNoEmit`, `TestTscNoEmitOnError`,
//! `TestTscProjectReferences`, `TestTypeAcquisition` and `TestGenerateTrace`.
//!
//! Each Go test function builds the same inputs in the same order and runs them through
//! `run_tsc_inputs`. Inputs with a watch flag run in a separate `_watch` test. Go backtick
//! literals are Rust raw strings with the same bytes (tabs kept).

use crate::support::runner::{
    FileMap, TscEdit, TscInput, WatchFilter, edit, no_change, no_change_only_edit, run_tsc_inputs,
};
use crate::support::stringtestutil::{dedent, go_sprintf};
use crate::support::test_sys::get_file_map_with_build;
use crate::support::vfstest::{MapFile, symlink};

/// Go `FileMap{path: value, ...}`. Each value goes through `MapFile::from`.
// PORT: a local macro with the same job as `support::runner::file_map!`,
// so this file does not depend on how that macro is exported.
macro_rules! files {
    ($($path:expr => $value:expr),* $(,)?) => {{
        let mut map = FileMap::new();
        $(map.insert(String::from($path), MapFile::from($value));)*
        map
    }};
}

/// Go `[]string{...}`.
fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(std::string::ToString::to_string).collect()
}

/// Go `slices.Concat(args, []string{...})`.
fn concat_args(args: &[String], extra: &[&str]) -> Vec<String> {
    args.iter()
        .cloned()
        .chain(extra.iter().map(std::string::ToString::to_string))
        .collect()
}

// Go: tsc_test.go:2380 TestTscModuleResolution
fn tsc_module_resolution_inputs() -> Vec<TscInput> {
    fn get_build_module_resolution_in_project_ref_test_case(preserve_symlinks: bool) -> TscInput {
        TscInput {
            sub_scenario:
                r"resolves specifier in output declaration file from referenced project correctly"
                    .to_string()
                    + if preserve_symlinks {
                        " with preserveSymlinks"
                    } else {
                        ""
                    },
            files: files! {
                r"/user/username/projects/myproject/packages/pkg1/index.ts" => dedent(r"
					import type { TheNum } from 'pkg2'
					export const theNum: TheNum = 42;"),
                r"/user/username/projects/myproject/packages/pkg1/tsconfig.json" => dedent(&go_sprintf(r#"
					{
						"compilerOptions": { 
							"outDir": "build",
							"preserveSymlinks": %t
						},
						"references": [{ "path": "../pkg2" }]
					}
				"#, &[&preserve_symlinks])),
                r"/user/username/projects/myproject/packages/pkg2/const.ts" => dedent(r"
					export type TheNum = 42;
				"),
                r"/user/username/projects/myproject/packages/pkg2/index.ts" => dedent(r"
					export type { TheNum } from 'const';
				"),
                r"/user/username/projects/myproject/packages/pkg2/tsconfig.json" => dedent(&go_sprintf(r#"
					{
						"compilerOptions": {
							"composite": true,
							"outDir": "build",
							"paths": {
								"const": ["./const"]
							},
							"preserveSymlinks": %t,
						},
					}
				"#, &[&preserve_symlinks])),
                r"/user/username/projects/myproject/packages/pkg2/package.json" => dedent(r#"
					{
						"name": "pkg2",
						"version": "1.0.0",
						"main": "build/index.js"
					}
				"#),
                r"/user/username/projects/myproject/node_modules/pkg2" => symlink(r"/user/username/projects/myproject/packages/pkg2"),
            },
            cwd: "/user/username/projects/myproject".to_string(),
            command_line_args: argv(&["-b", "packages/pkg1", "--verbose", "--traceResolution"]),
            ..Default::default()
        }
    }
    fn get_tsc_module_resolution_sharing_file_map() -> FileMap {
        files! {
            "/home/src/workspaces/project/packages/a/index.js" => r"export const a = 'a';",
            "/home/src/workspaces/project/packages/a/test/index.js" => r"import 'a';",
            "/home/src/workspaces/project/packages/a/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"checkJs": true,
						"composite": true,
						"declaration": true,
						"emitDeclarationOnly": true,
						"module": "nodenext",
						"outDir": "types",
					},
				}"#),
            "/home/src/workspaces/project/packages/a/package.json" => dedent(r#"
				{
					"name": "a",
					"version": "0.0.0",
					"type": "module",
					"exports": {
						".": {
							"types": "./types/index.d.ts",
							"default": "./index.js"
						}
					}
				}"#),
            "/home/src/workspaces/project/packages/b/index.js" => r"export { a } from 'a';",
            "/home/src/workspaces/project/packages/b/tsconfig.json" => dedent(r#"
				{
				"references": [{ "path": "../a" }],
					"compilerOptions": {
						"checkJs": true,
						"module": "nodenext",
						"noEmit": true,
						"noImplicitAny": true,
					},
				}"#),
            "/home/src/workspaces/project/packages/b/package.json" => dedent(r#"
				{
					"name": "b",
					"version": "0.0.0",
					"type": "module"
				}"#),
            "/home/src/workspaces/project/node_modules/a" => symlink("/home/src/workspaces/project/packages/a"),
        }
    }
    fn get_tsc_module_resolution_alternate_result_at_types_package_json(
        package_name: &str,
        add_types_condition: bool,
    ) -> String {
        let types_string = if add_types_condition {
            r#""types": "./index.d.ts","#
        } else {
            ""
        };
        dedent(&go_sprintf(
            r#"
			{
				"name": "@types/%s",
				"version": "1.0.0",
				"types": "index.d.ts",
				"exports": {
					".": {
						%s
						"require": "./index.d.ts"
					}
				}
			}"#,
            &[&package_name, &types_string],
        ))
    }
    fn get_tsc_module_resolution_alternate_result_package_json(
        package_name: &str,
        add_types: bool,
        add_types_condition: bool,
    ) -> String {
        let types = if add_types {
            r#""types": "index.d.ts","#
        } else {
            ""
        };
        let types_string = if add_types_condition {
            r#""types": "./index.d.ts","#
        } else {
            ""
        };
        dedent(&go_sprintf(
            r#"
		{
			"name": "%s",
			"version": "1.0.0",
			"main": "index.js",
			%s
			"exports": {
				".": {
					%s
					"import": "./index.mjs",
					"require": "./index.js"
				}
			}
		}"#,
            &[&package_name, &types, &types_string],
        ))
    }
    fn get_tsc_module_resolution_alternate_result_dts(package_name: &str) -> String {
        go_sprintf(r"export declare const %s: number;", &[&package_name])
    }
    fn get_tsc_module_resolution_alternate_result_js(package_name: &str) -> String {
        go_sprintf(r"module.exports = { %s: 1 };", &[&package_name])
    }
    fn get_tsc_module_resolution_alternate_result_mjs(package_name: &str) -> String {
        go_sprintf(r"export const %s = 1;", &[&package_name])
    }
    vec![
        get_build_module_resolution_in_project_ref_test_case(false),
        get_build_module_resolution_in_project_ref_test_case(true),
        TscInput {
            sub_scenario: r"type reference resolution uses correct options for different resolution options referenced project".to_string(),
            files: files! {
                "/home/src/workspaces/project/packages/pkg1_index.ts" => r#"export const theNum: TheNum = "type1";"#,
                "/home/src/workspaces/project/packages/pkg1.tsconfig.json" => dedent(r#"
                    {
                        "compilerOptions": {
                            "composite": true,
                            "typeRoots": ["./typeroot1"]
                        },
                        "files": ["./pkg1_index.ts"],
                    }
                "#),
                "/home/src/workspaces/project/packages/typeroot1/sometype/index.d.ts" => r#"declare type TheNum = "type1";"#,
                "/home/src/workspaces/project/packages/pkg2_index.ts" => r#"export const theNum: TheNum2 = "type2";"#,
                "/home/src/workspaces/project/packages/pkg2.tsconfig.json" => dedent(r#"
                    {
                        "compilerOptions": {
                            "composite": true,
                            "typeRoots": ["./typeroot2"]
                        },
                        "files": ["./pkg2_index.ts"],
                    }
                "#),
                "/home/src/workspaces/project/packages/typeroot2/sometype/index.d.ts" => r#"declare type TheNum2 = "type2";"#,
            },
            command_line_args: argv(&[
                "-b",
                "packages/pkg1.tsconfig.json",
                "packages/pkg2.tsconfig.json",
                "--verbose",
                "--traceResolution",
            ]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "impliedNodeFormat differs between projects for shared file".to_string(),
            files: files! {
                "/home/src/workspaces/project/a/src/index.ts" => "",
                "/home/src/workspaces/project/a/tsconfig.json" => dedent(r#"
				{
                    "compilerOptions": {
						"strict": true
					}
				}
                "#),
                "/home/src/workspaces/project/b/src/index.ts" => dedent(r#"
                    import pg from "pg";
                    pg.foo();
                "#),
                "/home/src/workspaces/project/b/tsconfig.json" => dedent(r#"
				{
                    "compilerOptions": { 
						"strict": true,
						"module": "node16"
					},
                }"#),
                "/home/src/workspaces/project/b/package.json" => dedent(r#"
				{
                    "name": "b",
                    "type": "module"
                }"#),
                "/home/src/workspaces/project/node_modules/@types/pg/index.d.ts" => "export function foo(): void;",
                "/home/src/workspaces/project/node_modules/@types/pg/package.json" => dedent(r#"
				{
                    "name": "@types/pg",
                    "types": "index.d.ts"
                }"#),
            },
            command_line_args: argv(&[
                "-b",
                "a",
                "b",
                "--verbose",
                "--traceResolution",
                "--explainFiles",
            ]),
            edits: no_change_only_edit(),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "shared resolution should not report error".to_string(),
            files: get_tsc_module_resolution_sharing_file_map(),
            command_line_args: argv(&[
                "-b",
                "packages/b",
                "--verbose",
                "--traceResolution",
                "--explainFiles",
            ]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when resolution is not shared".to_string(),
            files: get_tsc_module_resolution_sharing_file_map(),
            command_line_args: argv(&[
                "-b",
                "packages/a",
                "--verbose",
                "--traceResolution",
                "--explainFiles",
            ]),
            edits: vec![TscEdit {
                caption: "build b".to_string(),
                command_line_args: Some(argv(&[
                    "-b",
                    "packages/b",
                    "--verbose",
                    "--traceResolution",
                    "--explainFiles",
                ])),
                ..Default::default()
            }],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "pnpm style layout".to_string(),
            files: files! {
                // button@0.0.1
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+button@0.0.1/node_modules/@component-type-checker/button/src/index.ts" => dedent(r"
                    export interface Button {
                        a: number;
                        b: number;
                    }
                    export function createButton(): Button {
                        return {
                            a: 0,
                            b: 1,
                        };
                    }
                "),
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+button@0.0.1/node_modules/@component-type-checker/button/package.json" => dedent(r#"
					{
						"name": "@component-type-checker/button",
						"version": "0.0.1",
						"main": "./src/index.ts"
					}"#),

                // button@0.0.2
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+button@0.0.2/node_modules/@component-type-checker/button/src/index.ts" => dedent(r"
                    export interface Button {
                        a: number;
                        c: number;
                    }
                    export function createButton(): Button {
                        return {
                            a: 0,
                            c: 2,
                        };
                    }
                "),
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+button@0.0.2/node_modules/@component-type-checker/button/package.json" => dedent(r#"
                    {
                        "name": "@component-type-checker/button",
                        "version": "0.0.2",
                        "main": "./src/index.ts"
                    }"#),

                // @component-type-checker+components@0.0.1_@component-type-checker+button@0.0.1
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+components@0.0.1_@component-type-checker+button@0.0.1/node_modules/@component-type-checker/button" => symlink(
                    "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+button@0.0.1/node_modules/@component-type-checker/button",
                ),
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+components@0.0.1_@component-type-checker+button@0.0.1/node_modules/@component-type-checker/components/src/index.ts" => dedent(r#"
                    export { createButton, Button } from "@component-type-checker/button";
                "#),
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+components@0.0.1_@component-type-checker+button@0.0.1/node_modules/@component-type-checker/components/package.json" => dedent(r#"
					{
						"name": "@component-type-checker/components",
						"version": "0.0.1",
						"main": "./src/index.ts",
						"peerDependencies": {
							"@component-type-checker/button": "*"
						},
						"devDependencies": {
							"@component-type-checker/button": "0.0.2"
						}
					}"#),

                // @component-type-checker+components@0.0.1_@component-type-checker+button@0.0.2
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+components@0.0.1_@component-type-checker+button@0.0.2/node_modules/@component-type-checker/button" => symlink(
                    "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+button@0.0.2/node_modules/@component-type-checker/button",
                ),
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+components@0.0.1_@component-type-checker+button@0.0.2/node_modules/@component-type-checker/components/src/index.ts" => dedent(r#"
                    export { createButton, Button } from "@component-type-checker/button";
                "#),
                "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+components@0.0.1_@component-type-checker+button@0.0.2/node_modules/@component-type-checker/components/package.json" => dedent(r#"
					{
						"name": "@component-type-checker/components",
						"version": "0.0.1",
						"main": "./src/index.ts",
						"peerDependencies": {
							"@component-type-checker/button": "*"
						},
						"devDependencies": {
							"@component-type-checker/button": "0.0.2"
						}
					}"#),

                // sdk => @component-type-checker+components@0.0.1_@component-type-checker+button@0.0.1
                "/home/src/projects/component-type-checker/packages/sdk/src/index.ts" => dedent(r#"
                    export { Button, createButton } from "@component-type-checker/components";
                    export const VERSION = "0.0.2";
                "#),
                "/home/src/projects/component-type-checker/packages/sdk/package.json" => dedent(r#"
                    {
                        "name": "@component-type-checker/sdk1",
                        "version": "0.0.2",
                        "main": "./src/index.ts",
                        "dependencies": {
                            "@component-type-checker/components": "0.0.1",
                            "@component-type-checker/button": "0.0.1"
                        }
                    }"#),
                "/home/src/projects/component-type-checker/packages/sdk/node_modules/@component-type-checker/button" => symlink(
                    "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+button@0.0.1/node_modules/@component-type-checker/button",
                ),
                "/home/src/projects/component-type-checker/packages/sdk/node_modules/@component-type-checker/components" => symlink(
                    "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+components@0.0.1_@component-type-checker+button@0.0.1/node_modules/@component-type-checker/components",
                ),

                // app => @component-type-checker+components@0.0.1_@component-type-checker+button@0.0.2
                "/home/src/projects/component-type-checker/packages/app/src/app.tsx" => dedent(r#"
                    import { VERSION } from "@component-type-checker/sdk";
                    import { Button } from "@component-type-checker/components";
                    import { createButton } from "@component-type-checker/button";
                    const button: Button = createButton();
                "#),
                "/home/src/projects/component-type-checker/packages/app/package.json" => dedent(r#"
					{
						"name": "app",
						"version": "1.0.0",
						"dependencies": {
							"@component-type-checker/button": "0.0.2",
							"@component-type-checker/components": "0.0.1",
							"@component-type-checker/sdk": "0.0.2"
						}
					}"#),
                "/home/src/projects/component-type-checker/packages/app/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"target": "es5",
							"module": "esnext",
							"lib": ["ES5"],
							"outDir": "dist",
						},
						"include": ["src"],
					}"#),
                "/home/src/projects/component-type-checker/packages/app/node_modules/@component-type-checker/button" => symlink(
                    "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+button@0.0.2/node_modules/@component-type-checker/button",
                ),
                "/home/src/projects/component-type-checker/packages/app/node_modules/@component-type-checker/components" => symlink(
                    "/home/src/projects/component-type-checker/node_modules/.pnpm/@component-type-checker+components@0.0.1_@component-type-checker+button@0.0.2/node_modules/@component-type-checker/components",
                ),
                "/home/src/projects/component-type-checker/packages/app/node_modules/@component-type-checker/sdk" => symlink(
                    "/home/src/projects/component-type-checker/packages/sdk",
                ),
            },
            cwd: "/home/src/projects/component-type-checker/packages/app".to_string(),
            command_line_args: argv(&["--traceResolution", "--explainFiles"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "package json scope".to_string(),
            files: files! {
                "/home/src/workspaces/project/src/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"target": "ES2016",
							"composite": true,
							"module": "Node16",
							"traceResolution": true,
						},
						"files": [
							"main.ts",
							"fileA.ts",
							"fileB.mts",
						],
					}"#),
                "/home/src/workspaces/project/src/main.ts" => "export const x = 10;",
                "/home/src/workspaces/project/src/fileA.ts" => dedent(r#"
                    import { foo } from "./fileB.mjs";
                    foo();
                "#),
                "/home/src/workspaces/project/src/fileB.mts" => "export function foo() {}",
                "/home/src/workspaces/project/package.json" => dedent(r#"
                    {
                        "name": "app",
                        "version": "1.0.0"
                    }
                "#),
            },
            command_line_args: argv(&["-p", "src", "--explainFiles", "--extendedDiagnostics"]),
            edits: vec![TscEdit {
                caption: "Delete package.json".to_string(),
                edit: edit(|sys| sys.remove_no_error("/home/src/workspaces/project/package.json")),
                ..Default::default()
            }],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "alternateResult".to_string(),
            files: files! {
                "/home/src/projects/project/node_modules/@types/bar/package.json" => get_tsc_module_resolution_alternate_result_at_types_package_json("bar" /*addTypesCondition*/, false),
                "/home/src/projects/project/node_modules/@types/bar/index.d.ts" => get_tsc_module_resolution_alternate_result_dts("bar"),
                "/home/src/projects/project/node_modules/bar/package.json" => get_tsc_module_resolution_alternate_result_package_json("bar" /*addTypes*/, false /*addTypesCondition*/, false),
                "/home/src/projects/project/node_modules/bar/index.js" => get_tsc_module_resolution_alternate_result_js("bar"),
                "/home/src/projects/project/node_modules/bar/index.mjs" => get_tsc_module_resolution_alternate_result_mjs("bar"),
                "/home/src/projects/project/node_modules/foo/package.json" => get_tsc_module_resolution_alternate_result_package_json("foo" /*addTypes*/, true /*addTypesCondition*/, false),
                "/home/src/projects/project/node_modules/foo/index.js" => get_tsc_module_resolution_alternate_result_js("foo"),
                "/home/src/projects/project/node_modules/foo/index.mjs" => get_tsc_module_resolution_alternate_result_mjs("foo"),
                "/home/src/projects/project/node_modules/foo/index.d.ts" => get_tsc_module_resolution_alternate_result_dts("foo"),
                "/home/src/projects/project/node_modules/@types/bar2/package.json" => get_tsc_module_resolution_alternate_result_at_types_package_json("bar2" /*addTypesCondition*/, true),
                "/home/src/projects/project/node_modules/@types/bar2/index.d.ts" => get_tsc_module_resolution_alternate_result_dts("bar2"),
                "/home/src/projects/project/node_modules/bar2/package.json" => get_tsc_module_resolution_alternate_result_package_json("bar2" /*addTypes*/, false /*addTypesCondition*/, false),
                "/home/src/projects/project/node_modules/bar2/index.js" => get_tsc_module_resolution_alternate_result_js("bar2"),
                "/home/src/projects/project/node_modules/bar2/index.mjs" => get_tsc_module_resolution_alternate_result_mjs("bar2"),
                "/home/src/projects/project/node_modules/foo2/package.json" => get_tsc_module_resolution_alternate_result_package_json("foo2" /*addTypes*/, true /*addTypesCondition*/, true),
                "/home/src/projects/project/node_modules/foo2/index.js" => get_tsc_module_resolution_alternate_result_js("foo2"),
                "/home/src/projects/project/node_modules/foo2/index.mjs" => get_tsc_module_resolution_alternate_result_mjs("foo2"),
                "/home/src/projects/project/node_modules/foo2/index.d.ts" => get_tsc_module_resolution_alternate_result_dts("foo2"),
                "/home/src/projects/project/index.mts" => dedent(r#"
					import { foo } from "foo";
					import { bar } from "bar";
					import { foo2 } from "foo2";
					import { bar2 } from "bar2";
				"#),
                "/home/src/projects/project/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"module": "node16",
							"moduleResolution": "node16",
							"traceResolution": true,
							"incremental": true,
							"strict": true,
							"types": [],
						},
						"files": ["index.mts"],
					}"#),
            },
            cwd: "/home/src/projects/project".to_string(),
            edits: vec![
                TscEdit {
                    caption: "delete the alternateResult in @types".to_string(),
                    edit: edit(|sys| {
                        sys.remove_no_error(
                            "/home/src/projects/project/node_modules/@types/bar/index.d.ts",
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "delete the node10Result in package/types".to_string(),
                    edit: edit(|sys| {
                        sys.remove_no_error("/home/src/projects/project/node_modules/foo/index.d.ts");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "add the alternateResult in @types".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            "/home/src/projects/project/node_modules/@types/bar/index.d.ts",
                            &get_tsc_module_resolution_alternate_result_dts("bar"),
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "add the alternateResult in package/types".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            "/home/src/projects/project/node_modules/foo/index.d.ts",
                            &get_tsc_module_resolution_alternate_result_dts("foo"),
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "update package.json from @types so error is fixed".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            "/home/src/projects/project/node_modules/@types/bar/package.json",
                            &get_tsc_module_resolution_alternate_result_at_types_package_json(
                                "bar", /*addTypesCondition*/ true,
                            ),
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "update package.json so error is fixed".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            "/home/src/projects/project/node_modules/foo/package.json",
                            &get_tsc_module_resolution_alternate_result_package_json(
                                "foo", /*addTypes*/ true, /*addTypesCondition*/ true,
                            ),
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "update package.json from @types so error is introduced".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            "/home/src/projects/project/node_modules/@types/bar2/package.json",
                            &get_tsc_module_resolution_alternate_result_at_types_package_json(
                                "bar2", /*addTypesCondition*/ false,
                            ),
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "update package.json so error is introduced".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            "/home/src/projects/project/node_modules/foo2/package.json",
                            &get_tsc_module_resolution_alternate_result_package_json(
                                "foo2", /*addTypes*/ true, /*addTypesCondition*/ false,
                            ),
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "delete the alternateResult in @types".to_string(),
                    edit: edit(|sys| {
                        sys.remove_no_error(
                            "/home/src/projects/project/node_modules/@types/bar2/index.d.ts",
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "delete the node10Result in package/types".to_string(),
                    edit: edit(|sys| {
                        sys.remove_no_error(
                            "/home/src/projects/project/node_modules/foo2/index.d.ts",
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "add the alternateResult in @types".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            "/home/src/projects/project/node_modules/@types/bar2/index.d.ts",
                            &get_tsc_module_resolution_alternate_result_dts("bar2"),
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "add the ndoe10Result in package/types".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            "/home/src/projects/project/node_modules/foo2/index.d.ts",
                            &get_tsc_module_resolution_alternate_result_dts("foo2"),
                        );
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "handles the cache correctly when two projects use different module resolution settings".to_string(),
            files: files! {
                r"/user/username/projects/myproject/project1/index.ts" => r#"import { foo } from "file";"#,
                r"/user/username/projects/myproject/project1/node_modules/file/index.d.ts" => "export const foo = 10;",
                r"/user/username/projects/myproject/project1/tsconfig.json" => dedent(r#"
				   {
                       "compilerOptions": {
						   "composite": true,
						   "types": ["foo", "bar"]
					   },
                       "files": ["index.ts"],
                   }"#),
                r"/user/username/projects/myproject/project2/index.ts" => r#"import { foo } from "file";"#,
                r"/user/username/projects/myproject/project2/node_modules/file/index.d.ts" => "export const foo = 10;",
                r"/user/username/projects/myproject/project2/tsconfig.json" => dedent(r#"
				   {
                       "compilerOptions": {
						   "composite": true,
						   "types": ["foo"],
						   "module": "nodenext",
						   "moduleResolution": "nodenext"
					   },
                       "files": ["index.ts"],
                   }"#),
                r"/user/username/projects/myproject/node_modules/@types/foo/index.d.ts" => "export const foo = 10;",
                r"/user/username/projects/myproject/node_modules/@types/bar/index.d.ts" => "export const bar = 10;",
                r"/user/username/projects/myproject/tsconfig.json" => dedent(r#"
				   {
						"files": [],
						"references": [
							{ "path": "./project1" },
							{ "path": "./project2" },
						],
                   }"#),
            },
            cwd: "/user/username/projects/myproject".to_string(),
            command_line_args: argv(&["--b", "-w", "-v"]),
            edits: vec![TscEdit {
                caption: "Append text".to_string(),
                edit: edit(|sys| sys.append_file(r"/user/username/projects/myproject/project1/index.ts", "const bar = 10;")),
                ..Default::default()
            }],
            ..Default::default()
        },
        // !!! sheetal package.json watches not yet implemented
        TscInput {
            sub_scenario: r"resolves specifier in output declaration file from referenced project correctly with cts and mts extensions".to_string(),
            files: files! {
                r"/user/username/projects/myproject/packages/pkg1/package.json" => dedent(r#"
					{
						"name": "pkg1",
						"version": "1.0.0",
						"main": "build/index.js",
						"type": "module"
					}"#),
                r"/user/username/projects/myproject/packages/pkg1/index.ts" => dedent(r"
					import type { TheNum } from 'pkg2'
					export const theNum: TheNum = 42;"),
                r"/user/username/projects/myproject/packages/pkg1/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"outDir": "build",
							"module": "node16",
						},
						"references": [{ "path": "../pkg2" }],
					}"#),
                r"/user/username/projects/myproject/packages/pkg2/const.cts" => r"export type TheNum = 42;",
                r"/user/username/projects/myproject/packages/pkg2/index.ts" => r"export type { TheNum } from './const.cjs';",
                r"/user/username/projects/myproject/packages/pkg2/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"composite": true,
							"outDir": "build",
							"module": "node16",
						},
					}"#),
                r"/user/username/projects/myproject/packages/pkg2/package.json" => dedent(r#"
					{
						"name": "pkg2",
						"version": "1.0.0",
						"main": "build/index.js",
						"type": "module"
					}"#),
                r"/user/username/projects/myproject/node_modules/pkg2" => symlink(r"/user/username/projects/myproject/packages/pkg2"),
            },
            cwd: "/user/username/projects/myproject".to_string(),
            command_line_args: argv(&[
                "-b",
                "packages/pkg1",
                "-w",
                "--verbose",
                "--traceResolution",
            ]),
            edits: vec![
                TscEdit {
                    caption: "reports import errors after change to package file".to_string(),
                    edit: edit(|sys| {
                        sys.replace_file_text(r"/user/username/projects/myproject/packages/pkg1/package.json", r#""module""#, r#""commonjs""#);
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "removes those errors when a package file is changed back".to_string(),
                    edit: edit(|sys| {
                        sys.replace_file_text(r"/user/username/projects/myproject/packages/pkg1/package.json", r#""commonjs""#, r#""module""#);
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "reports import errors after change to package file".to_string(),
                    edit: edit(|sys| {
                        sys.replace_file_text(r"/user/username/projects/myproject/packages/pkg1/package.json", r#""module""#, r#""commonjs""#);
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "removes those errors when a package file is changed to cjs extensions"
                        .to_string(),
                    edit: edit(|sys| {
                        sys.replace_file_text(r"/user/username/projects/myproject/packages/pkg2/package.json", r#""build/index.js""#, r#""build/index.cjs""#);
                        sys.rename_file_no_error(r"/user/username/projects/myproject/packages/pkg2/index.ts", r"/user/username/projects/myproject/packages/pkg2/index.cts");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: r"build mode watches for changes to package-json main fields".to_string(),
            files: files! {
                r"/user/username/projects/myproject/packages/pkg1/package.json" => dedent(r#"
					{
                        "name": "pkg1",
                        "version": "1.0.0",
                        "main": "build/index.js"
                    }"#),
                r"/user/username/projects/myproject/packages/pkg1/index.ts" => dedent(r"
                    import type { TheNum } from 'pkg2'
                    export const theNum: TheNum = 42;"),
                r"/user/username/projects/myproject/packages/pkg1/tsconfig.json" => dedent(r#"
					{
                        "compilerOptions": {
                            "outDir": "build",
                        },
                        "references": [{ "path": "../pkg2" }],
                    }"#),
                r"/user/username/projects/myproject/packages/pkg2/tsconfig.json" => dedent(r#"
					{
                        "compilerOptions": {
                            "composite": true,
                            "outDir": "build",
                        },
                    }"#),
                r"/user/username/projects/myproject/packages/pkg2/const.ts" => r"export type TheNum = 42;",
                r"/user/username/projects/myproject/packages/pkg2/index.ts" => r"export type { TheNum } from './const.js';",
                r"/user/username/projects/myproject/packages/pkg2/other.ts" => r"export type TheStr = string;",
                r"/user/username/projects/myproject/packages/pkg2/package.json" => dedent(r#"
					{
						"name": "pkg2",
                        "version": "1.0.0",
                        "main": "build/index.js"
                    }"#),
                r"/user/username/projects/myproject/node_modules/pkg2" => symlink(r"/user/username/projects/myproject/packages/pkg2"),
            },
            cwd: "/user/username/projects/myproject".to_string(),
            command_line_args: argv(&[
                "-b",
                "packages/pkg1",
                "--verbose",
                "-w",
                "--traceResolution",
            ]),
            edits: vec![
                TscEdit {
                    caption: "reports import errors after change to package file".to_string(),
                    edit: edit(|sys| {
                        sys.replace_file_text(r"/user/username/projects/myproject/packages/pkg2/package.json", r"index.js", r"other.js");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "removes those errors when a package file is changed back".to_string(),
                    edit: edit(|sys| {
                        sys.replace_file_text(r"/user/username/projects/myproject/packages/pkg2/package.json", r"other.js", r"index.js");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: r"build mode watches missing package-json lookups".to_string(),
            files: files! {
                r"/user/username/projects/myproject/packages/pkg1/index.ts" => dedent(r"
					import type { TheNum } from 'pkg2'
					export const theNum: TheNum = 42;"),
                r"/user/username/projects/myproject/packages/pkg1/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"outDir": "build",
						},
					}"#),
            },
            cwd: "/user/username/projects/myproject".to_string(),
            command_line_args: argv(&[
                "-b",
                "packages/pkg1",
                "-w",
                "--verbose",
                "--traceResolution",
            ]),
            edits: vec![
                TscEdit {
                    caption: "resolves import after package is installed".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(r"/user/username/projects/myproject/node_modules/pkg2/package.json", &dedent(r#"
							{
								"name": "pkg2",
								"version": "1.0.0",
								"types": "index.d.ts"
							}"#));
                        sys.write_file_no_error(r"/user/username/projects/myproject/node_modules/pkg2/index.d.ts", r"export type TheNum = 42;");
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "reports import errors after package is removed".to_string(),
                    edit: edit(|sys| {
                        sys.remove_no_error(r"/user/username/projects/myproject/node_modules/pkg2/package.json");
                        sys.remove_no_error(r"/user/username/projects/myproject/node_modules/pkg2/index.d.ts");
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        TscInput {
            sub_scenario: r"build mode watches package-json lookups from existing buildinfo".to_string(),
            files: get_file_map_with_build(
                files! {
                    r"/user/username/projects/myproject/packages/pkg1/index.ts" => dedent(r"
					import type { TheNum } from 'pkg2'
					export const theNum: TheNum = 42;"),
                    r"/user/username/projects/myproject/packages/pkg1/tsconfig.json" => dedent(r#"
					{
						"compilerOptions": {
							"outDir": "zzbuild",
						},
					}"#),
                    r"/user/username/projects/myproject/node_modules/pkg2/package.json" => dedent(r#"
					{
						"name": "pkg2",
						"version": "1.0.0",
						"types": "index.d.ts"
					}"#),
                    r"/user/username/projects/myproject/node_modules/pkg2/index.d.ts" => r"export type TheNum = 42;",
                },
                &argv(&[
                    "-b",
                    "/user/username/projects/myproject/packages/pkg1",
                    "--verbose",
                    "--traceResolution",
                ]),
            ),
            cwd: "/user/username/projects/myproject".to_string(),
            command_line_args: argv(&[
                "-b",
                "packages/pkg1",
                "-w",
                "--verbose",
                "--traceResolution",
            ]),
            edits: vec![TscEdit {
                caption: "reports import errors after package is removed".to_string(),
                edit: edit(|sys| {
                    sys.remove_no_error(r"/user/username/projects/myproject/node_modules/pkg2/package.json");
                    sys.remove_no_error(r"/user/username/projects/myproject/node_modules/pkg2/index.d.ts");
                }),
                ..Default::default()
            }],
            ..Default::default()
        },
        TscInput {
            sub_scenario: "resolution from d.ts of referenced project".to_string(),
            files: files! {
                "/home/src/workspaces/project/common.d.ts" => "export type OnValue = (value: number) => void",
                "/home/src/workspaces/project/producer/index.ts" => dedent(r#"
                    export { ValueProducerDeclaration } from "./in-js"
                    import { OnValue } from "@common"
                    export interface ValueProducerFromTs {
                        onValue: OnValue;
                    }
                "#),
                "/home/src/workspaces/project/producer/in-js.d.ts" => dedent(r#"
                    import { OnValue } from "@common"
                    export interface ValueProducerDeclaration {
                        onValue: OnValue;
                    }
                "#),
                "/home/src/workspaces/project/producer/tsconfig.json" => dedent(r#"
				{
                    "compilerOptions": {
                        "strict": true,
                        "composite": true,
                        "module": "nodenext",
                        "moduleResolution": "nodenext",
                        "paths": {
                            "@common": ["../common.d.ts"],
                        },
                    },
                }"#),
                "/home/src/workspaces/project/consumer/index.ts" => dedent(r#"
                    import { ValueProducerDeclaration, ValueProducerFromTs } from "@producer"
                    declare let v: ValueProducerDeclaration;
					// n is implicitly any because onValue is actually any (despite what the tooltip says)
					v.onValue = (n) => {
                    }
                    // n is implicitly number as expected
                    declare let v2: ValueProducerFromTs;
                    v2.onValue = (n) => {
                    }"#),
                "/home/src/workspaces/project/consumer/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"strict": true,
						"module": "nodenext",
						"moduleResolution": "nodenext",
						"paths": {
							"@producer": ["../producer/index"],
						},
					},
					"references": [
						{ "path": "../producer" },
                    ],
                }"#),
            },
            command_line_args: argv(&["--b", "consumer", "--traceResolution", "-v"]),
            ..Default::default()
        },
    ]
}

#[test]
fn tsc_module_resolution() {
    run_tsc_inputs(
        "moduleResolution",
        tsc_module_resolution_inputs(),
        WatchFilter::NonWatch,
    );
}

#[test]
fn tsc_module_resolution_watch() {
    run_tsc_inputs(
        "moduleResolution",
        tsc_module_resolution_inputs(),
        WatchFilter::WatchOnly,
    );
}

// Go: tsc_test.go:3150 TestTscNoCheck
fn tsc_no_check_inputs() -> Vec<TscInput> {
    struct NoCheckScenario {
        sub_scenario: &'static str,
        a_text: &'static str,
    }
    fn get_tsc_no_check_test_case(
        scenario: &NoCheckScenario,
        incremental: bool,
        command_line_args: Vec<String>,
    ) -> TscInput {
        let no_change_with_check = TscEdit {
            caption: "No Change run with checking".to_string(),
            command_line_args: Some(command_line_args.clone()),
            ..Default::default()
        };
        let fix_error_no_check = TscEdit {
            caption: "Fix `a` error with noCheck".to_string(),
            edit: edit(|sys| {
                sys.write_file_no_error(
                    "/home/src/workspaces/project/a.ts",
                    r#"export const a = "hello";"#,
                );
            }),
            ..Default::default()
        };
        let a_text = scenario.a_text;
        let add_error_no_check = TscEdit {
            caption: "Introduce error with noCheck".to_string(),
            edit: edit(move |sys| {
                sys.write_file_no_error("/home/src/workspaces/project/a.ts", a_text);
            }),
            ..Default::default()
        };
        TscInput {
            sub_scenario: scenario.sub_scenario.to_string()
                + if incremental { " with incremental" } else { "" },
            files: files! {
                "/home/src/workspaces/project/a.ts" => scenario.a_text,
                "/home/src/workspaces/project/b.ts" => r"export const b = 10;",
                "/home/src/workspaces/project/tsconfig.json" => dedent(&go_sprintf(r#"
				{
					"compilerOptions": {
						"declaration": true,
						"incremental": %t
					}
				}"#, &[&incremental])),
            },
            command_line_args: concat_args(&command_line_args, &["--noCheck"]),
            edits: vec![
                no_change(),
                fix_error_no_check.clone(),   // Fix error with noCheck
                no_change(),                  // Should be no op
                no_change_with_check.clone(), // Check errors - should not report any errors - update buildInfo
                no_change_with_check.clone(), // Should be no op
                no_change(),                  // Should be no op
                add_error_no_check.clone(),
                no_change(),                  // Should be no op
                no_change_with_check.clone(), // Should check errors and update buildInfo
                fix_error_no_check.clone(),   // Fix error with noCheck
                no_change_with_check.clone(), // Should check errors and update buildInfo
                TscEdit {
                    caption: "Add file with error".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            "/home/src/workspaces/project/c.ts",
                            r#"export const c: number = "hello";"#,
                        );
                    }),
                    command_line_args: Some(command_line_args.clone()),
                    ..Default::default()
                },
                add_error_no_check,
                fix_error_no_check,
                no_change_with_check.clone(),
                no_change(),          // Should be no op
                no_change_with_check, // Should be no op
            ],
            ..Default::default()
        }
    }

    let cases = [
        NoCheckScenario {
            sub_scenario: "syntax errors",
            a_text: r#"export const a = "hello"#,
        },
        NoCheckScenario {
            sub_scenario: "semantic errors",
            a_text: r#"export const a: number = "hello";"#,
        },
        NoCheckScenario {
            sub_scenario: "dts errors",
            a_text: r"export const a = class { private p = 10; };",
        },
    ];
    cases
        .iter()
        .flat_map(|c| {
            [
                get_tsc_no_check_test_case(c, false, argv(&[])),
                get_tsc_no_check_test_case(c, true, argv(&[])),
                get_tsc_no_check_test_case(c, false, argv(&["-b", "-v"])),
                get_tsc_no_check_test_case(c, true, argv(&["-b", "-v"])),
            ]
        })
        .collect()
}

#[test]
fn tsc_no_check() {
    run_tsc_inputs("noCheck", tsc_no_check_inputs(), WatchFilter::NonWatch);
}

// Go: tsc_test.go:3233 TestTscNoEmit
fn tsc_no_emit_inputs() -> Vec<TscInput> {
    struct TscNoEmitScenario {
        sub_scenario: &'static str,
        a_text: &'static str,
        dts_enabled: bool,
    }
    const NO_EMIT_SCENARIOS: [TscNoEmitScenario; 4] = [
        TscNoEmitScenario {
            sub_scenario: "syntax errors",
            a_text: r#"const a = "hello"#,
            dts_enabled: false,
        },
        TscNoEmitScenario {
            sub_scenario: "semantic errors",
            a_text: r#"const a: number = "hello""#,
            dts_enabled: false,
        },
        TscNoEmitScenario {
            sub_scenario: "dts errors",
            a_text: r"const a = class { private p = 10; };",
            dts_enabled: true,
        },
        TscNoEmitScenario {
            sub_scenario: "dts errors without dts enabled",
            a_text: r"const a = class { private p = 10; };",
            dts_enabled: false,
        },
    ];
    type NoEmitEdits = fn(&TscNoEmitScenario, &[String], bool) -> Vec<TscEdit>;
    fn get_tsc_no_emit_and_errors_file_map(
        scenario: &TscNoEmitScenario,
        incremental: bool,
        as_modules: bool,
        modify: Option<fn(&mut FileMap)>,
    ) -> FileMap {
        let mut files = files! {
            "/home/src/projects/project/a.ts" => (if as_modules { r"export " } else { "" }).to_string() + scenario.a_text,
            "/home/src/projects/project/tsconfig.json" => dedent(&go_sprintf(r#"
				{
					"compilerOptions": {
						"incremental": %t,
						"declaration": %t
					}
				}
		"#, &[&incremental, &scenario.dts_enabled])),
        };
        if as_modules {
            files.insert(
                "/home/src/projects/project/b.ts".to_string(),
                MapFile::from(r"export const b = 10;"),
            );
        }
        if let Some(modify) = modify {
            modify(&mut files);
        }
        files
    }
    fn get_tsc_no_emit_and_errors_test_cases_worker(
        command_line_args: Vec<String>,
        add_no_emit_on_command_line: bool,
        modify: Option<fn(&mut FileMap)>,
        edits: NoEmitEdits,
    ) -> Vec<TscInput> {
        let mut testing_cases = Vec::with_capacity(NO_EMIT_SCENARIOS.len() * 3);
        let mut command_line_args_for_input = command_line_args.clone();
        if add_no_emit_on_command_line {
            command_line_args_for_input = concat_args(&command_line_args, &["--noEmit"]);
        }
        for scenario in &NO_EMIT_SCENARIOS {
            testing_cases.extend([
                TscInput {
                    sub_scenario: scenario.sub_scenario.to_string(),
                    command_line_args: command_line_args_for_input.clone(),
                    files: get_tsc_no_emit_and_errors_file_map(scenario, false, false, modify),
                    cwd: "/home/src/projects/project".to_string(),
                    edits: edits(scenario, &command_line_args, false),
                    ..Default::default()
                },
                TscInput {
                    sub_scenario: scenario.sub_scenario.to_string() + " with incremental",
                    command_line_args: command_line_args_for_input.clone(),
                    files: get_tsc_no_emit_and_errors_file_map(scenario, true, false, modify),
                    cwd: "/home/src/projects/project".to_string(),
                    edits: edits(scenario, &command_line_args, false),
                    ..Default::default()
                },
                TscInput {
                    sub_scenario: scenario.sub_scenario.to_string()
                        + " with incremental as modules",
                    command_line_args: command_line_args_for_input.clone(),
                    files: get_tsc_no_emit_and_errors_file_map(scenario, true, true, modify),
                    cwd: "/home/src/projects/project".to_string(),
                    edits: edits(scenario, &command_line_args, true),
                    ..Default::default()
                },
            ]);
        }
        testing_cases
    }
    fn get_tsc_no_emit_and_errors_test_cases(command_line_args: Vec<String>) -> Vec<TscInput> {
        get_tsc_no_emit_and_errors_test_cases_worker(
            command_line_args,
            true,
            None,
            |scenario: &TscNoEmitScenario, command_line_args: &[String], as_modules: bool| {
                let fixed_a_ts_content =
                    (if as_modules { "export " } else { "" }).to_string() + r#"const a = "hello";"#;
                let a_text = scenario.a_text;
                vec![
                    no_change(),
                    TscEdit {
                        caption: "Fix error".to_string(),
                        edit: edit(move |sys| {
                            sys.write_file_no_error(
                                "/home/src/projects/project/a.ts",
                                &fixed_a_ts_content,
                            );
                        }),
                        ..Default::default()
                    },
                    no_change(),
                    TscEdit {
                        caption: "Emit after fixing error".to_string(),
                        command_line_args: Some(command_line_args.to_vec()),
                        ..Default::default()
                    },
                    no_change(),
                    TscEdit {
                        caption: "Introduce error".to_string(),
                        edit: edit(move |sys| {
                            sys.write_file_no_error("/home/src/projects/project/a.ts", a_text);
                        }),
                        ..Default::default()
                    },
                    TscEdit {
                        caption: "Emit when error".to_string(),
                        command_line_args: Some(command_line_args.to_vec()),
                        ..Default::default()
                    },
                    no_change(),
                ]
            },
        )
    }
    fn get_tsc_no_emit_and_errors_watch_test_cases(
        command_line_args: Vec<String>,
    ) -> Vec<TscInput> {
        get_tsc_no_emit_and_errors_test_cases_worker(
            command_line_args,
            false,
            Some(
                (|files: &mut FileMap| {
                    // Go: strings.Replace(files[...].(string), "}", ..., 1)
                    let tsconfig = String::from_utf8(
                        files["/home/src/projects/project/tsconfig.json"]
                            .data
                            .clone(),
                    )
                    .unwrap();
                    files.insert(
                        "/home/src/projects/project/tsconfig.json".to_string(),
                        MapFile::from(tsconfig.replacen('}', r#", "noEmit": true }"#, 1)),
                    );
                }) as fn(&mut FileMap),
            ),
            |scenario: &TscNoEmitScenario, _command_line_args: &[String], as_modules: bool| {
                let fixed_a_ts_content =
                    (if as_modules { "export " } else { "" }).to_string() + r#"const a = "hello";"#;
                let a_text = scenario.a_text;
                vec![
                    TscEdit {
                        caption: "Fix error".to_string(),
                        edit: edit(move |sys| {
                            sys.write_file_no_error(
                                "/home/src/projects/project/a.ts",
                                &fixed_a_ts_content,
                            );
                        }),
                        ..Default::default()
                    },
                    TscEdit {
                        caption: "Emit after fixing error".to_string(),
                        edit: edit(|sys| {
                            sys.replace_file_text(
                                "/home/src/projects/project/tsconfig.json",
                                r#""noEmit": true"#,
                                r#""noEmit": false"#,
                            );
                        }),
                        ..Default::default()
                    },
                    TscEdit {
                        caption: "no Emit run after fixing error".to_string(),
                        edit: edit(|sys| {
                            sys.replace_file_text(
                                "/home/src/projects/project/tsconfig.json",
                                r#""noEmit": false"#,
                                r#""noEmit": true"#,
                            );
                        }),
                        ..Default::default()
                    },
                    TscEdit {
                        caption: "Introduce error".to_string(),
                        edit: edit(move |sys| {
                            sys.write_file_no_error("/home/src/projects/project/a.ts", a_text);
                        }),
                        ..Default::default()
                    },
                    TscEdit {
                        caption: "Emit when error".to_string(),
                        edit: edit(|sys| {
                            sys.replace_file_text(
                                "/home/src/projects/project/tsconfig.json",
                                r#""noEmit": true"#,
                                r#""noEmit": false"#,
                            );
                        }),
                        ..Default::default()
                    },
                    TscEdit {
                        caption: "no Emit run when error".to_string(),
                        edit: edit(|sys| {
                            sys.replace_file_text(
                                "/home/src/projects/project/tsconfig.json",
                                r#""noEmit": false"#,
                                r#""noEmit": true"#,
                            );
                        }),
                        ..Default::default()
                    },
                ]
            },
        )
    }
    fn get_tsc_no_emit_changes_file_map(options_str: &str) -> FileMap {
        files! {
            "/home/src/workspaces/project/src/class.ts" => dedent(r"
				export class classC {
					prop = 1;
				}"),
            "/home/src/workspaces/project/src/indirectClass.ts" => dedent(r"
				import { classC } from './class';
				export class indirectClass {
					classC = new classC();
				}"),
            "/home/src/workspaces/project/src/directUse.ts" => dedent(r"
				import { indirectClass } from './indirectClass';
				new indirectClass().classC.prop;"),
            "/home/src/workspaces/project/src/indirectUse.ts" => dedent(r"
				import { indirectClass } from './indirectClass';
				new indirectClass().classC.prop;"),
            "/home/src/workspaces/project/src/noChangeFile.ts" => dedent(r"
				export function writeLog(s: string) {
				}"),
            "/home/src/workspaces/project/src/noChangeFileWithEmitSpecificError.ts" => dedent(r"
				function someFunc(arguments: boolean, ...rest: any[]) {
				}"),
            "/home/src/workspaces/project/tsconfig.json" => dedent(&go_sprintf(r#"
				{
					"compilerOptions":  { %s }
				}"#, &[&options_str])),
        }
    }

    struct TscNoEmitChangesScenario {
        sub_scenario: &'static str,
        options_string: &'static str,
    }
    const NO_EMIT_CHANGES_SCENARIOS: [TscNoEmitChangesScenario; 3] = [
        TscNoEmitChangesScenario {
            // !!! sheetal missing initial reporting of Duplicate_identifier_arguments_Compiler_uses_arguments_to_initialize_rest_parameters is absent
            sub_scenario: "composite",
            options_string: r#""composite": true"#,
        },
        TscNoEmitChangesScenario {
            sub_scenario: "incremental declaration",
            options_string: r#""incremental": true, "declaration": true"#,
        },
        TscNoEmitChangesScenario {
            sub_scenario: "incremental",
            options_string: r#""incremental": true"#,
        },
    ];
    fn get_tsc_no_emit_changes_test_cases(command_line_args: Vec<String>) -> Vec<TscInput> {
        let no_change_with_no_emit = TscEdit {
            caption: "No Change run with noEmit".to_string(),
            command_line_args: Some(concat_args(&command_line_args, &["--noEmit"])),
            ..Default::default()
        };
        let no_change_with_emit = TscEdit {
            caption: "No Change run with emit".to_string(),
            command_line_args: Some(command_line_args.clone()),
            ..Default::default()
        };
        let introduce_error = edit(|sys| {
            sys.replace_file_text("/home/src/workspaces/project/src/class.ts", "prop", "prop1");
        });
        let fix_error = edit(|sys| {
            sys.replace_file_text("/home/src/workspaces/project/src/class.ts", "prop1", "prop");
        });
        let mut test_cases = Vec::with_capacity(NO_EMIT_CHANGES_SCENARIOS.len());
        for scenario in &NO_EMIT_CHANGES_SCENARIOS {
            test_cases.extend([
                TscInput {
                    sub_scenario: "changes ".to_string() + scenario.sub_scenario,
                    command_line_args: command_line_args.clone(),
                    files: get_tsc_no_emit_changes_file_map(scenario.options_string),
                    edits: vec![
                        no_change_with_no_emit.clone(),
                        no_change_with_no_emit.clone(),
                        TscEdit {
                            caption: "Introduce error but still noEmit".to_string(),
                            command_line_args: no_change_with_no_emit.command_line_args.clone(),
                            edit: introduce_error.clone(),
                            ..Default::default()
                        },
                        TscEdit {
                            caption: "Fix error and emit".to_string(),
                            edit: fix_error.clone(),
                            ..Default::default()
                        },
                        no_change_with_emit.clone(),
                        no_change_with_no_emit.clone(),
                        no_change_with_no_emit.clone(),
                        no_change_with_emit.clone(),
                        TscEdit {
                            caption: "Introduce error and emit".to_string(),
                            edit: introduce_error.clone(),
                            ..Default::default()
                        },
                        no_change_with_emit.clone(),
                        no_change_with_no_emit.clone(),
                        no_change_with_no_emit.clone(),
                        no_change_with_emit.clone(),
                        TscEdit {
                            caption: "Fix error and no emit".to_string(),
                            command_line_args: no_change_with_no_emit.command_line_args.clone(),
                            edit: fix_error.clone(),
                            ..Default::default()
                        },
                        no_change_with_emit.clone(),
                        no_change_with_no_emit.clone(),
                        no_change_with_no_emit.clone(),
                        no_change_with_emit.clone(),
                    ],
                    ..Default::default()
                },
                TscInput {
                    sub_scenario: "changes with initial noEmit ".to_string()
                        + scenario.sub_scenario,
                    command_line_args: no_change_with_no_emit
                        .command_line_args
                        .clone()
                        .unwrap_or_default(),
                    files: get_tsc_no_emit_changes_file_map(scenario.options_string),
                    edits: vec![
                        no_change_with_emit.clone(),
                        TscEdit {
                            caption: "Introduce error with emit".to_string(),
                            command_line_args: Some(command_line_args.clone()),
                            edit: introduce_error.clone(),
                            ..Default::default()
                        },
                        TscEdit {
                            caption: "Fix error and no emit".to_string(),
                            edit: fix_error.clone(),
                            ..Default::default()
                        },
                        no_change_with_emit.clone(),
                    ],
                    ..Default::default()
                },
            ]);
        }
        test_cases
    }
    fn get_tsc_no_emit_dts_changes_file_map(incremental: bool, as_modules: bool) -> FileMap {
        let mut files = files! {
            "/home/src/projects/project/a.ts" => if as_modules { r"export const a = class { private p = 10; };" } else { r"const a = class { private p = 10; };" },
            "/home/src/projects/project/tsconfig.json" => dedent(&go_sprintf(r#"
				{
					"compilerOptions": {
						"incremental": %t,
					}
				}
		"#, &[&incremental])),
        };
        if as_modules {
            files.insert(
                "/home/src/projects/project/b.ts".to_string(),
                MapFile::from(r"export const b = 10;"),
            );
        }
        files
    }
    fn get_tsc_no_emit_dts_changes_edits(command_line_args: &[String]) -> Vec<TscEdit> {
        vec![
            no_change(),
            TscEdit {
                caption: "With declaration enabled noEmit - Should report errors".to_string(),
                command_line_args: Some(concat_args(
                    command_line_args,
                    &["--noEmit", "--declaration"],
                )),
                ..Default::default()
            },
            TscEdit {
                caption: "With declaration and declarationMap noEmit - Should report errors"
                    .to_string(),
                command_line_args: Some(concat_args(
                    command_line_args,
                    &["--noEmit", "--declaration", "--declarationMap"],
                )),
                ..Default::default()
            },
            no_change(),
            TscEdit {
                caption: "Dts Emit with error".to_string(),
                command_line_args: Some(concat_args(command_line_args, &["--declaration"])),
                ..Default::default()
            },
            TscEdit {
                caption: "Fix the error".to_string(),
                edit: edit(|sys| {
                    sys.replace_file_text("/home/src/projects/project/a.ts", "private", "public");
                }),
                ..Default::default()
            },
            TscEdit {
                caption: "With declaration enabled noEmit".to_string(),
                command_line_args: Some(concat_args(
                    command_line_args,
                    &["--noEmit", "--declaration"],
                )),
                ..Default::default()
            },
            TscEdit {
                caption: "With declaration and declarationMap noEmit".to_string(),
                command_line_args: Some(concat_args(
                    command_line_args,
                    &["--noEmit", "--declaration", "--declarationMap"],
                )),
                ..Default::default()
            },
        ]
    }
    fn get_tsc_no_emit_dts_changes_test_cases() -> Vec<TscInput> {
        vec![
            TscInput {
                sub_scenario: "dts errors with declaration enable changes".to_string(),
                command_line_args: argv(&["-b", "-v", "--noEmit"]),
                files: get_tsc_no_emit_dts_changes_file_map(false, false),
                cwd: "/home/src/projects/project".to_string(),
                edits: get_tsc_no_emit_dts_changes_edits(&argv(&["-b", "-v"])),
                ..Default::default()
            },
            TscInput {
                sub_scenario: "dts errors with declaration enable changes with incremental"
                    .to_string(),
                command_line_args: argv(&["-b", "-v", "--noEmit"]),
                files: get_tsc_no_emit_dts_changes_file_map(true, false),
                cwd: "/home/src/projects/project".to_string(),
                edits: get_tsc_no_emit_dts_changes_edits(&argv(&["-b", "-v"])),
                ..Default::default()
            },
            TscInput {
                sub_scenario:
                    "dts errors with declaration enable changes with incremental as modules"
                        .to_string(),
                command_line_args: argv(&["-b", "-v", "--noEmit"]),
                files: get_tsc_no_emit_dts_changes_file_map(true, true),
                cwd: "/home/src/projects/project".to_string(),
                edits: get_tsc_no_emit_dts_changes_edits(&argv(&["-b", "-v"])),
                ..Default::default()
            },
        ]
    }
    fn get_tsc_no_emit_dts_changes_multi_file_errors_test_cases(
        command_line_args: Vec<String>,
    ) -> Vec<TscInput> {
        let a_content = r"export const a = class { private p = 10; };";
        vec![TscInput {
            sub_scenario: "dts errors with declaration enable changes with multiple files"
                .to_string(),
            command_line_args: concat_args(&command_line_args, &["--noEmit"]),
            files: files! {
                "/home/src/projects/project/a.ts" => a_content,
                "/home/src/projects/project/b.ts" => r"export const b = 10;",
                "/home/src/projects/project/c.ts" => a_content.replacen('a', "c", 1),
                "/home/src/projects/project/d.ts" => a_content.replacen('a', "d", 1),
                "/home/src/projects/project/tsconfig.json" => dedent(r#"
						{
							"compilerOptions": {
								"incremental": true,
							}
						}
				"#),
            },
            cwd: "/home/src/projects/project".to_string(),
            edits: [
                get_tsc_no_emit_dts_changes_edits(&command_line_args),
                vec![TscEdit {
                    caption: "Fix the another ".to_string(),
                    edit: edit(|sys| {
                        sys.replace_file_text(
                            "/home/src/projects/project/c.ts",
                            "private",
                            "public",
                        );
                    }),
                    command_line_args: Some(concat_args(
                        &command_line_args,
                        &["--noEmit", "--declaration", "--declarationMap"],
                    )),
                    ..Default::default()
                }],
            ]
            .concat(),
            ..Default::default()
        }]
    }
    fn get_tsc_no_emit_loop_test_case(suffix: &str, command_line_args: Vec<String>) -> TscInput {
        TscInput {
            sub_scenario: "does not go in loop when watching when no files are emitted".to_string()
                + suffix,
            files: files! {
                "/user/username/projects/myproject/a.js" => "",
                "/user/username/projects/myproject/b.ts" => "",
                "/user/username/projects/myproject/tsconfig.json" => dedent(r#"
					{
                        "compilerOptions": {
                            "allowJs": true,
                            "noEmit": true,
                        },
                    }"#),
            },
            cwd: "/user/username/projects/myproject".to_string(),
            command_line_args,
            edits: vec![
                TscEdit {
                    caption: "No change".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            r"/user/username/projects/myproject/a.js",
                            &sys.read_file_no_error(r"/user/username/projects/myproject/a.js"),
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "change".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            r"/user/username/projects/myproject/a.js",
                            "const x = 10;",
                        );
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }
    [
        vec![
            TscInput {
                sub_scenario: "when project has strict true".to_string(),
                files: files! {
                    "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
						{
							"compilerOptions": {
								"incremental": true,
								"strict": true
							}
						}"#),
                    "/home/src/workspaces/project/class1.ts" => r"export class class1 {}",
                },
                command_line_args: argv(&["--noEmit"]),
                edits: no_change_only_edit(),
                ..Default::default()
            },
            get_tsc_no_emit_loop_test_case("", argv(&["-b", "-w", "-verbose"])),
            get_tsc_no_emit_loop_test_case(
                " with incremental",
                argv(&["-b", "-w", "-verbose", "--incremental"]),
            ),
        ],
        get_tsc_no_emit_and_errors_test_cases(argv(&[])),
        get_tsc_no_emit_and_errors_test_cases(argv(&["-b", "-v"])),
        get_tsc_no_emit_changes_test_cases(argv(&[])),
        get_tsc_no_emit_changes_test_cases(argv(&["-b", "-v"])),
        get_tsc_no_emit_dts_changes_test_cases(),
        get_tsc_no_emit_dts_changes_multi_file_errors_test_cases(argv(&[])),
        get_tsc_no_emit_dts_changes_multi_file_errors_test_cases(argv(&["-b", "-v"])),
        get_tsc_no_emit_and_errors_watch_test_cases(argv(&["-b", "-verbose", "-w"])),
    ]
    .concat()
}

#[test]
fn tsc_no_emit() {
    run_tsc_inputs("noEmit", tsc_no_emit_inputs(), WatchFilter::NonWatch);
}

#[test]
fn tsc_no_emit_watch() {
    run_tsc_inputs("noEmit", tsc_no_emit_inputs(), WatchFilter::WatchOnly);
}

// Go: tsc_test.go:3703 TestTscNoEmitOnError
fn tsc_no_emit_on_error_inputs() -> Vec<TscInput> {
    struct TscNoEmitOnErrorScenario {
        sub_scenario: &'static str,
        main_error_content: String,
        fixed_error_content: String,
    }
    fn get_tsc_no_emit_on_error_file_map(
        scenario: &TscNoEmitOnErrorScenario,
        declaration: bool,
        incremental: bool,
    ) -> FileMap {
        files! {
            "/user/username/projects/noEmitOnError/tsconfig.json" => dedent(&go_sprintf(r#"
			{
				"compilerOptions": {
					"outDir": "./dev-build",
					"declaration": %t,
					"incremental": %t,
					"noEmitOnError": true,
				},
			}"#, &[&declaration, &incremental])),
            "/user/username/projects/noEmitOnError/shared/types/db.ts" => dedent(r"
				export interface A {
					name: string;
				}
			"),
            "/user/username/projects/noEmitOnError/src/main.ts" => scenario.main_error_content.clone(),
            "/user/username/projects/noEmitOnError/src/other.ts" => dedent(r#"
				console.log("hi");
				export { }
			"#),
        }
    }
    fn get_tsc_no_emit_on_error_test_cases(
        scenarios: &[TscNoEmitOnErrorScenario],
        command_line_args: Vec<String>,
    ) -> Vec<TscInput> {
        let mut test_cases = Vec::with_capacity(scenarios.len() * 4);
        for scenario in scenarios {
            let fixed_error_content = scenario.fixed_error_content.clone();
            let edits = vec![
                no_change(),
                TscEdit {
                    caption: "Fix error".to_string(),
                    edit: edit(move |sys| {
                        sys.write_file_no_error(
                            "/user/username/projects/noEmitOnError/src/main.ts",
                            &fixed_error_content,
                        );
                    }),
                    ..Default::default()
                },
                no_change(),
            ];
            test_cases.extend([
                TscInput {
                    sub_scenario: scenario.sub_scenario.to_string(),
                    files: get_tsc_no_emit_on_error_file_map(scenario, false, false),
                    cwd: "/user/username/projects/noEmitOnError".to_string(),
                    command_line_args: command_line_args.clone(),
                    edits: edits.clone(),
                    ..Default::default()
                },
                TscInput {
                    sub_scenario: scenario.sub_scenario.to_string() + " with declaration",
                    files: get_tsc_no_emit_on_error_file_map(scenario, true, false),
                    cwd: "/user/username/projects/noEmitOnError".to_string(),
                    command_line_args: command_line_args.clone(),
                    edits: edits.clone(),
                    ..Default::default()
                },
                TscInput {
                    sub_scenario: scenario.sub_scenario.to_string() + " with incremental",
                    files: get_tsc_no_emit_on_error_file_map(scenario, false, true),
                    cwd: "/user/username/projects/noEmitOnError".to_string(),
                    command_line_args: command_line_args.clone(),
                    edits: edits.clone(),
                    ..Default::default()
                },
                TscInput {
                    sub_scenario: scenario.sub_scenario.to_string()
                        + " with declaration with incremental",
                    files: get_tsc_no_emit_on_error_file_map(scenario, true, true),
                    cwd: "/user/username/projects/noEmitOnError".to_string(),
                    command_line_args: command_line_args.clone(),
                    edits: edits.clone(),
                    ..Default::default()
                },
            ]);
        }
        test_cases
    }
    fn get_tsc_watch_no_emit_on_error_test_cases(
        scenarios: &[TscNoEmitOnErrorScenario],
        command_line_args: Vec<String>,
    ) -> Vec<TscInput> {
        let mut edits: Vec<TscEdit> = Vec::new();
        for scenario in scenarios {
            // PORT: Go checks `edits != nil`, which is false only before the first append.
            if !edits.is_empty() {
                let main_error_content = scenario.main_error_content.clone();
                edits.push(TscEdit {
                    caption: scenario.sub_scenario.to_string(),
                    edit: edit(move |sys| {
                        sys.write_file_no_error(
                            r"/user/username/projects/noEmitOnError/src/main.ts",
                            &main_error_content,
                        );
                    }),
                    ..Default::default()
                });
            }
            let fixed_error_content = scenario.fixed_error_content.clone();
            edits.extend([
                TscEdit {
                    caption: "No Change".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            r"/user/username/projects/noEmitOnError/src/main.ts",
                            &sys.read_file_no_error(
                                r"/user/username/projects/noEmitOnError/src/main.ts",
                            ),
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "Fix ".to_string() + scenario.sub_scenario,
                    edit: edit(move |sys| {
                        sys.write_file_no_error(
                            "/user/username/projects/noEmitOnError/src/main.ts",
                            &fixed_error_content,
                        );
                    }),
                    ..Default::default()
                },
                TscEdit {
                    caption: "No Change".to_string(),
                    edit: edit(|sys| {
                        sys.write_file_no_error(
                            r"/user/username/projects/noEmitOnError/src/main.ts",
                            &sys.read_file_no_error(
                                r"/user/username/projects/noEmitOnError/src/main.ts",
                            ),
                        );
                    }),
                    ..Default::default()
                },
            ]);
        }
        vec![
            TscInput {
                sub_scenario: "noEmitOnError".to_string(),
                files: get_tsc_no_emit_on_error_file_map(&scenarios[0], false, false),
                cwd: "/user/username/projects/noEmitOnError".to_string(),
                command_line_args: command_line_args.clone(),
                edits: edits.clone(),
                ..Default::default()
            },
            TscInput {
                sub_scenario: "noEmitOnError with declaration".to_string(),
                files: get_tsc_no_emit_on_error_file_map(&scenarios[0], true, false),
                cwd: "/user/username/projects/noEmitOnError".to_string(),
                command_line_args: command_line_args.clone(),
                edits: edits.clone(),
                ..Default::default()
            },
            TscInput {
                sub_scenario: "noEmitOnError with incremental".to_string(),
                files: get_tsc_no_emit_on_error_file_map(&scenarios[0], false, true),
                cwd: "/user/username/projects/noEmitOnError".to_string(),
                command_line_args: command_line_args.clone(),
                edits: edits.clone(),
                ..Default::default()
            },
            TscInput {
                sub_scenario: "noEmitOnError with declaration with incremental".to_string(),
                files: get_tsc_no_emit_on_error_file_map(&scenarios[0], true, true),
                cwd: "/user/username/projects/noEmitOnError".to_string(),
                command_line_args,
                edits,
                ..Default::default()
            },
        ]
    }
    let scenarios = vec![
        TscNoEmitOnErrorScenario {
            sub_scenario: "syntax errors",
            main_error_content: dedent(
                r#"
                import { A } from "../shared/types/db";
                const a = {
                    lastName: 'sdsd'
                ;
            "#,
            ),
            fixed_error_content: dedent(
                r#"
                import { A } from "../shared/types/db";
                const a = {
                    lastName: 'sdsd'
                };"#,
            ),
        },
        TscNoEmitOnErrorScenario {
            sub_scenario: "semantic errors",
            main_error_content: dedent(
                r#"
                import { A } from "../shared/types/db";
                const a: string = 10;"#,
            ),
            fixed_error_content: dedent(
                r#"
                import { A } from "../shared/types/db";
                const a: string = "hello";"#,
            ),
        },
        TscNoEmitOnErrorScenario {
            sub_scenario: "dts errors",
            main_error_content: dedent(
                r#"
                import { A } from "../shared/types/db";
                export const a = class { private p = 10; };
            "#,
            ),
            fixed_error_content: dedent(
                r#"
                import { A } from "../shared/types/db";
                export const a = class { p = 10; };
            "#,
            ),
        },
    ];
    [
        get_tsc_no_emit_on_error_test_cases(&scenarios, argv(&[])),
        get_tsc_no_emit_on_error_test_cases(&scenarios, argv(&["-b", "-v"])),
        vec![
            TscInput {
                sub_scenario: r"when declarationMap changes".to_string(),
                files: files! {
                    "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
						{
							"compilerOptions": {
								"noEmitOnError": true,
								"declaration": true,
								"composite": true,
							},
						}"#),
                    "/home/src/workspaces/project/a.ts" => "const x = 10;",
                    "/home/src/workspaces/project/b.ts" => "const y = 10;",
                },
                edits: vec![
                    TscEdit {
                        caption: "error and enable declarationMap".to_string(),
                        edit: edit(|sys| {
                            sys.replace_file_text(
                                "/home/src/workspaces/project/a.ts",
                                "x",
                                "x: 20",
                            );
                        }),
                        command_line_args: Some(argv(&["--declarationMap"])),
                        ..Default::default()
                    },
                    TscEdit {
                        caption: "fix error declarationMap".to_string(),
                        edit: edit(|sys| {
                            sys.replace_file_text(
                                "/home/src/workspaces/project/a.ts",
                                "x: 20",
                                "x",
                            );
                        }),
                        command_line_args: Some(argv(&["--declarationMap"])),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            TscInput {
                sub_scenario: "file deleted before fixing error with noEmitOnError".to_string(),
                files: files! {
                    "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
						{
							"compilerOptions": {
								"outDir": "outDir",
								"noEmitOnError": true,
							},
						}"#),
                    "/home/src/workspaces/project/file1.ts" => r#"export const x: 30 = "hello";"#,
                    "/home/src/workspaces/project/file2.ts" => r"export class D { }",
                },
                command_line_args: argv(&["-i"]),
                edits: vec![TscEdit {
                    caption: "delete file without error".to_string(),
                    edit: edit(|sys| sys.remove_no_error("/home/src/workspaces/project/file2.ts")),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        get_tsc_watch_no_emit_on_error_test_cases(&scenarios, argv(&["-b", "-w", "-v"])),
    ]
    .concat()
}

#[test]
fn tsc_no_emit_on_error() {
    run_tsc_inputs(
        "noEmitOnError",
        tsc_no_emit_on_error_inputs(),
        WatchFilter::NonWatch,
    );
}

#[test]
fn tsc_no_emit_on_error_watch() {
    run_tsc_inputs(
        "noEmitOnError",
        tsc_no_emit_on_error_inputs(),
        WatchFilter::WatchOnly,
    );
}

// Go: tsc_test.go:3947 TestTscProjectReferences
fn tsc_project_references_inputs() -> Vec<TscInput> {
    vec![
        TscInput {
            sub_scenario: "when project references composite project with noEmit".to_string(),
            files: files! {
                "/home/src/workspaces/solution/utils/index.ts" => "export const x = 10;",
                "/home/src/workspaces/solution/utils/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"noEmit": true
					}
				}"#),
                "/home/src/workspaces/solution/project/index.ts" => r#"import { x } from "../utils";"#,
                "/home/src/workspaces/solution/project/tsconfig.json" => dedent(r#"
				{
					"references": [
						{ "path": "../utils" },
					],
				}"#),
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "project"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when project references composite".to_string(),
            files: files! {
                "/home/src/workspaces/solution/utils/index.ts" => "export const x = 10;",
                "/home/src/workspaces/solution/utils/index.d.ts" => "export declare const x = 10;",
                "/home/src/workspaces/solution/utils/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true
					}
				}"#),
                "/home/src/workspaces/solution/project/index.ts" => r#"import { x } from "../utils";"#,
                "/home/src/workspaces/solution/project/tsconfig.json" => dedent(r#"
				{
					"references": [
						{ "path": "../utils" },
					],
				}"#),
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "project"]),
            ..Default::default()
        },
        // ts#64544 (tsc_test.go:4401)
        TscInput {
            sub_scenario: "incremental nested triple-slash reference to composite project source"
                .to_string(),
            files: files! {
                "/home/src/workspaces/solution/utils/index.ts" => "interface ReferencedType {}",
                "/home/src/workspaces/solution/utils/index.d.ts" => "interface ReferencedType {}",
                "/home/src/workspaces/solution/utils/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true
					}
				}"#),
                "/home/src/workspaces/solution/project/src/index.ts" => "/// <reference path=\"../../utils/index.ts\" />\nlet value: ReferencedType;",
                "/home/src/workspaces/solution/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"disableSourceOfProjectReferenceRedirect": true,
						"incremental": true
					},
					"files": ["src/index.ts"],
					"references": [
						{ "path": "../utils" }
					]
				}"#),
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "project"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when project reference is not built".to_string(),
            files: files! {
                "/home/src/workspaces/solution/utils/index.ts" => "export const x = 10;",
                "/home/src/workspaces/solution/utils/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true
					}
				}"#),
                "/home/src/workspaces/solution/project/index.ts" => r#"import { x } from "../utils";"#,
                "/home/src/workspaces/solution/project/tsconfig.json" => dedent(r#"
				{
					"references": [
						{ "path": "../utils" },
					],
				}"#),
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "project"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when project contains invalid project reference".to_string(),
            files: files! {
                "/home/src/workspaces/solution/project/index.ts" => r"export const x = 10;",
                "/home/src/workspaces/solution/project/tsconfig.json" => dedent(r#"
				{
					"references": [
						{ "path": "../utils" },
					],
				}"#),
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "project"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "when project references have invalid fields".to_string(),
            files: files! {
                "/home/src/workspaces/solution/project/index.ts" => r"export const x = 10;",
                "/home/src/workspaces/solution/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"noEmit": true
					},
					"files": ["index.ts"],
					"references": [
						{ "path": true },
						{ "circular": true },
						{ "path": "../utils", "circular": "yes" },
						{ "path": "" },
						{ "path": "../valid", "circular": true }
					]
				}"#),
                "/home/src/workspaces/solution/utils/index.ts" => "export const y = 10;",
                "/home/src/workspaces/solution/utils/index.d.ts" => "export declare const y = 10;",
                "/home/src/workspaces/solution/utils/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true
					}
				}"#),
                "/home/src/workspaces/solution/valid/index.ts" => "export const z = 10;",
                "/home/src/workspaces/solution/valid/index.d.ts" => "export declare const z = 10;",
                "/home/src/workspaces/solution/valid/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true
					}
				}"#),
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "project"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "default import interop uses referenced project settings".to_string(),
            files: files! {
                "/home/src/workspaces/project/node_modules/ambiguous-package/package.json" => dedent(r#"
				{
					"name": "ambiguous-package"
				}"#),
                "/home/src/workspaces/project/node_modules/ambiguous-package/index.d.ts" => "export declare const ambiguous: number;",
                "/home/src/workspaces/project/node_modules/esm-package/package.json" => dedent(r#"
				{
					"name": "esm-package",
					"type": "module"
				}"#),
                "/home/src/workspaces/project/node_modules/esm-package/index.d.ts" => "export declare const esm: number;",
                "/home/src/workspaces/project/lib/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"declaration": true,
						"rootDir": "src",
						"outDir": "dist",
						"module": "esnext",
						"moduleResolution": "bundler",
					},
					"include": ["src"],
				}"#),
                "/home/src/workspaces/project/lib/src/a.ts" => "export const a = 0;",
                "/home/src/workspaces/project/lib/dist/a.d.ts" => "export declare const a = 0;",
                "/home/src/workspaces/project/app/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"module": "esnext",
						"moduleResolution": "bundler",
						"rootDir": "src",
						"outDir": "dist",
					},
					"include": ["src"],
					"references": [
						{ "path": "../lib" },
					],
				}"#),
                "/home/src/workspaces/project/app/src/local.ts" => "export const local = 0;",
                "/home/src/workspaces/project/app/src/index.ts" => dedent(r#"
					import local from "./local"; // Error
					import esm from "esm-package"; // Error
					import referencedSource from "../../lib/src/a"; // Error
					import referencedDeclaration from "../../lib/dist/a"; // Error
					import ambiguous from "ambiguous-package"; // Ok"#),
            },
            command_line_args: argv(&["--p", "app", "--pretty", "false"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "referenced project with esnext module disallows synthetic default imports"
                .to_string(),
            files: files! {
                "/home/src/workspaces/project/lib/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"declaration": true,
						"module": "esnext",
						"moduleResolution": "bundler",
						"rootDir": "src",
						"outDir": "dist"
					},
					"include": ["src"]
				}"#),
                "/home/src/workspaces/project/lib/src/utils.ts" => "export const test = () => 'test';",
                "/home/src/workspaces/project/lib/dist/utils.d.ts" => "export declare const test: () => string;",
                "/home/src/workspaces/project/app/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"module": "esnext",
						"moduleResolution": "bundler"
					},
					"references": [
						{ "path": "../lib" }
					]
				}"#),
                "/home/src/workspaces/project/app/index.ts" => dedent(r"
					import TestSrc from '../lib/src/utils'; // Error
					import TestDecl from '../lib/dist/utils'; // Error
					console.log(TestSrc.test());
					console.log(TestDecl.test());"),
            },
            command_line_args: argv(&["--p", "app", "--pretty", "false"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario:
                "referencing ambient const enum from referenced project with preserveConstEnums"
                    .to_string(),
            files: files! {
                "/home/src/workspaces/solution/utils/index.ts" => "export const enum E { A = 1 }",
                "/home/src/workspaces/solution/utils/index.d.ts" => "export declare const enum E { A = 1 }",
                "/home/src/workspaces/solution/utils/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"declaration": true,
						"preserveConstEnums": true,
					},
				}"#),
                "/home/src/workspaces/solution/project/index.ts" => r#"import { E } from "../utils"; E.A;"#,
                "/home/src/workspaces/solution/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"isolatedModules": true,
					},
					"references": [
						{ "path": "../utils" },
					],
				}"#),
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "project"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "importing const enum from referenced project with preserveConstEnums and verbatimModuleSyntax".to_string(),
            files: files! {
                "/home/src/workspaces/solution/preserve/index.ts" => "export const enum E { A = 1 }",
                "/home/src/workspaces/solution/preserve/index.d.ts" => "export declare const enum E { A = 1 }",
                "/home/src/workspaces/solution/preserve/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"declaration": true,
						"preserveConstEnums": true,
					},
				}"#),
                "/home/src/workspaces/solution/no-preserve/index.ts" => "export const enum E { A = 1 }",
                "/home/src/workspaces/solution/no-preserve/index.d.ts" => "export declare const enum F { A = 1 }",
                "/home/src/workspaces/solution/no-preserve/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"declaration": true,
						"preserveConstEnums": false,
					},
				}"#),
                "/home/src/workspaces/solution/project/index.ts" => dedent(r#"
					import { E } from "../preserve";
					import { F } from "../no-preserve";
					E.A;
					F.A;"#),
                "/home/src/workspaces/solution/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"module": "preserve",
						"verbatimModuleSyntax": true,
					},
					"references": [
						{ "path": "../preserve" },
						{ "path": "../no-preserve" },
					],
				}"#),
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "project", "--pretty", "false"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "rewriteRelativeImportExtensionsProjectReferences1".to_string(),
            files: files! {
                "/home/src/workspaces/packages/common/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"rootDir": "src",
						"outDir": "dist", 
						"module": "nodenext"
					}
				}"#),
                "/home/src/workspaces/packages/common/package.json" => dedent(r#"
				{
						"name": "common",
						"version": "1.0.0",
						"type": "module",
						"exports": {
							".": {
								"source": "./src/index.ts",
								"default": "./dist/index.js"
							}
						}
				}"#),
                "/home/src/workspaces/packages/common/src/index.ts" => "export {};",
                "/home/src/workspaces/packages/common/dist/index.d.ts" => "export {};",
                "/home/src/workspaces/packages/main/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"module": "nodenext",
						"rewriteRelativeImportExtensions": true,
						"rootDir": "src",
						"outDir": "dist"
					},
					"references": [
						{ "path": "../common" }
					]
				}"#),
                "/home/src/workspaces/packages/main/package.json" => dedent(r#"
				{
					"type": "module"
				}"#),
                "/home/src/workspaces/packages/main/src/index.ts" => r#"import {} from "../../common/src/index.ts";"#,
            },
            cwd: "/home/src/workspaces".to_string(),
            command_line_args: argv(&["-p", "packages/main", "--pretty", "false"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "rewriteRelativeImportExtensionsProjectReferences2".to_string(),
            files: files! {
                "/home/src/workspaces/solution/src/tsconfig-base.json" => dedent(r#"
				{
					"compilerOptions": {
						"module": "nodenext",
						"composite": true,
						"rootDir": ".",
						"outDir": "../dist",
						"rewriteRelativeImportExtensions": true
					}
				}"#),
                "/home/src/workspaces/solution/src/compiler/tsconfig.json" => dedent(r#"
				{
					"extends": "../tsconfig-base.json",
					"compilerOptions": {}
				}"#),
                "/home/src/workspaces/solution/src/compiler/parser.ts" => "export {};",
                "/home/src/workspaces/solution/dist/compiler/parser.d.ts" => "export {};",
                "/home/src/workspaces/solution/src/services/tsconfig.json" => dedent(r#"
				{
					"extends": "../tsconfig-base.json",
					"compilerOptions": {},
					"references": [
						{ "path": "../compiler" }
					]
				}"#),
                "/home/src/workspaces/solution/src/services/services.ts" => r#"import {} from "../compiler/parser.ts";"#,
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "src/services", "--pretty", "false"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "rewriteRelativeImportExtensionsProjectReferences3".to_string(),
            files: files! {
                "/home/src/workspaces/solution/src/tsconfig-base.json" => dedent(r#"
				{
					"compilerOptions": { 
						"module": "nodenext",
						"composite": true,
						"rewriteRelativeImportExtensions": true
					}
				}"#),
                "/home/src/workspaces/solution/src/compiler/tsconfig.json" => dedent(r#"
				{
					"extends": "../tsconfig-base.json",
					"compilerOptions": {
						"rootDir": ".",
						"outDir": "../../dist/compiler"
					}
				}"#),
                "/home/src/workspaces/solution/src/compiler/parser.ts" => "export {};",
                "/home/src/workspaces/solution/dist/compiler/parser.d.ts" => "export {};",
                "/home/src/workspaces/solution/src/services/tsconfig.json" => dedent(r#"
				{
					"extends": "../tsconfig-base.json",
					"compilerOptions": {
						"rootDir": ".", 
						"outDir": "../../dist/services"
					},
					"references": [
						{ "path": "../compiler" }
					]
				}"#),
                "/home/src/workspaces/solution/src/services/services.ts" => r#"import {} from "../compiler/parser.ts";"#,
            },
            cwd: "/home/src/workspaces/solution".to_string(),
            command_line_args: argv(&["--p", "src/services", "--pretty", "false"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "default setup was created correctly".to_string(),
            files: files! {
                "/home/src/workspaces/project/primary/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					}
				}"#),
                "/home/src/workspaces/project/primary/a.ts" => "export { };",
                "/home/src/workspaces/project/secondary/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"references": [{
						"path": "../primary"
					}]
				}"#),
                "/home/src/workspaces/project/secondary/b.ts" => r#"import * as mod_1 from "../primary/a";"#,
            },
            command_line_args: argv(&["--p", "primary/tsconfig.json"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "errors when declaration = false".to_string(),
            files: files! {
                "/home/src/workspaces/project/primary/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
						"declaration": false
					}
				}"#),
                "/home/src/workspaces/project/primary/a.ts" => "export { };",
            },
            command_line_args: argv(&["--p", "primary/tsconfig.json"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "errors when the referenced project doesnt have composite".to_string(),
            files: files! {
                "/home/src/workspaces/project/primary/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": false,
						"outDir": "bin",
					}
				}"#),
                "/home/src/workspaces/project/primary/a.ts" => "export { };",
                "/home/src/workspaces/project/reference/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"files": [ "b.ts" ],
					"references": [ { "path": "../primary" } ]
				}"#),
                "/home/src/workspaces/project/reference/b.ts" => r#"import * as mod_1 from "../primary/a";"#,
            },
            command_line_args: argv(&["--p", "reference/tsconfig.json"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario:
                "does not error when the referenced project doesnt have composite if its a container project"
                    .to_string(),
            files: files! {
                "/home/src/workspaces/project/primary/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": false,
						"outDir": "bin",
					}
				}"#),
                "/home/src/workspaces/project/primary/a.ts" => "export { };",
                "/home/src/workspaces/project/reference/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"files": [ ],
					"references": [{
						"path": "../primary"
					}]
				}"#),
                "/home/src/workspaces/project/reference/b.ts" => r#"import * as mod_1 from "../primary/a";"#,
            },
            command_line_args: argv(&["--p", "reference/tsconfig.json"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "errors when the file list is not exhaustive".to_string(),
            files: files! {
                "/home/src/workspaces/project/primary/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"files": [ "a.ts" ]
				}"#),
                "/home/src/workspaces/project/primary/a.ts" => "import * as b from './b'",
                "/home/src/workspaces/project/primary/b.ts" => "export {}",
            },
            command_line_args: argv(&["--p", "primary/tsconfig.json"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "errors when the referenced project doesnt exist".to_string(),
            files: files! {
                "/home/src/workspaces/project/primary/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"references": [{
						"path": "../foo"
					}]
				}"#),
                "/home/src/workspaces/project/primary/a.ts" => "export { };",
            },
            command_line_args: argv(&["--p", "primary/tsconfig.json"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "redirects to the output dts file".to_string(),
            files: files! {
                "/home/src/workspaces/project/alpha/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					}
				}"#),
                "/home/src/workspaces/project/alpha/a.ts" => "export const m: number = 3;",
                "/home/src/workspaces/project/alpha/bin/a.d.ts" => "export { };",
                "/home/src/workspaces/project/beta/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"references": [ { "path": "../alpha" } ]
				}"#),
                "/home/src/workspaces/project/beta/b.ts" => "import { m } from '../alpha/a'",
            },
            command_line_args: argv(&["--p", "beta/tsconfig.json", "--explainFiles"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "issues a nice error when the input file is missing".to_string(),
            files: files! {
                "/home/src/workspaces/project/alpha/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"references": []
				}"#),
                "/home/src/workspaces/project/alpha/a.ts" => "export const m: number = 3;",
                "/home/src/workspaces/project/beta/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"references": [ { "path": "../alpha" } ]
				}"#),
                "/home/src/workspaces/project/beta/b.ts" => "import { m } from '../alpha/a'",
            },
            command_line_args: argv(&["--p", "beta/tsconfig.json"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario:
                "issues a nice error when the input file is missing when module reference is not relative"
                    .to_string(),
            files: files! {
                "/home/src/workspaces/project/alpha/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					}
				}"#),
                "/home/src/workspaces/project/alpha/a.ts" => "export const m: number = 3;",
                "/home/src/workspaces/project/beta/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
						"paths": {
                            "@alpha/*": ["../alpha/*"],
                        },
					},
					"references": [ { "path": "../alpha" } ]
				}"#),
                "/home/src/workspaces/project/beta/b.ts" => "import { m } from '@alpha/a'",
            },
            command_line_args: argv(&["--p", "beta/tsconfig.json"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "doesnt infer the rootDir from source paths".to_string(),
            files: files! {
                "/home/src/workspaces/project/alpha/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"references": []
				}"#),
                "/home/src/workspaces/project/alpha/src/a.ts" => "export const m: number = 3;",
            },
            command_line_args: argv(&["--p", "alpha/tsconfig.json"]),
            ..Default::default()
        },
        // !!! sheetal rootDir error not reported
        TscInput {
            sub_scenario: "errors when a file is outside the rootdir".to_string(),
            files: files! {
                "/home/src/workspaces/project/alpha/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"composite": true,
						"outDir": "bin",
					},
					"references": []
				}"#),
                "/home/src/workspaces/project/alpha/src/a.ts" => "import * as b from '../../beta/b'",
                "/home/src/workspaces/project/beta/b.ts" => "export { }",
            },
            command_line_args: argv(&["--p", "alpha/tsconfig.json"]),
            ..Default::default()
        },
    ]
}

#[test]
fn tsc_project_references() {
    run_tsc_inputs(
        "projectReferences",
        tsc_project_references_inputs(),
        WatchFilter::NonWatch,
    );
}

// Go: tsc_test.go:4538 TestTypeAcquisition
#[test]
fn type_acquisition() {
    run_tsc_inputs(
        "typeAcquisition",
        vec![TscInput {
            sub_scenario: "parse tsconfig with typeAcquisition".to_string(),
            files: files! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
			{
				"compilerOptions": {
					"composite": true,
					"noEmit": true,
				},
				"typeAcquisition": {
					"enable": true,
					"include": ["0.d.ts", "1.d.ts"],
					"exclude": ["0.js", "1.js"],
					"disableFilenameBasedTypeAcquisition": true,
				},
			}"#),
            },
            command_line_args: argv(&[]),
            ..Default::default()
        }],
        WatchFilter::NonWatch,
    );
}

// Go: tsc_test.go:4561 TestGenerateTrace
fn generate_trace_inputs() -> Vec<TscInput> {
    vec![
        TscInput {
            sub_scenario: "generateTrace generates types file".to_string(),
            files: files! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"strict": true,
						"noEmit": true
					}
				}"#),
                "/home/src/workspaces/project/a.ts" => dedent(r#"
				interface Person {
					name: string;
					age: number;
				}
				const p: Person = { name: "Alice", age: 30 };
				"#),
            },
            command_line_args: argv(&[
                "--generateTrace",
                "/home/src/workspaces/project/trace",
                "--singleThreaded",
            ]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "generateTrace with multiple files and complex types".to_string(),
            files: files! {
                "/home/src/workspaces/project/tsconfig.json" => dedent(r#"
				{
					"compilerOptions": {
						"strict": true,
						"noEmit": true
					}
				}"#),
                "/home/src/workspaces/project/types.ts" => dedent(r"
				export interface Container<T> {
					value: T;
					map<U>(fn: (x: T) => U): Container<U>;
				}
				export type Nullable<T> = T | null | undefined;
				"),
                "/home/src/workspaces/project/main.ts" => dedent(r#"
				import { Container, Nullable } from "./types";
				const c: Container<number> = { value: 42, map: (fn) => ({ value: fn(42), map: c.map }) };
				const n: Nullable<string> = "hello";
				"#),
            },
            command_line_args: argv(&[
                "--generateTrace",
                "/home/src/workspaces/project/trace",
                "--singleThreaded",
            ]),
            ..Default::default()
        },
    ]
}

#[test]
fn generate_trace() {
    run_tsc_inputs(
        "generateTrace",
        generate_trace_inputs(),
        WatchFilter::NonWatch,
    );
}

/// Port-only tests of two `tsc -b --verbose` statuses that ts#64159 changed
/// in build/buildtask.go (the bump D build skeptic's probes).
///
/// PORT: no Go counterpart. The expected text is what the Go N'
/// (fed0bf24149f) oracle prints for the same steps. They run the real
/// `tsgo` on a temp dir, because the first status needs the bundled default
/// library directory.
#[cfg(unix)]
mod build_status_64159 {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    use ts_goport::execute::incremental::build_info::build_info_version;

    /// A new empty dir under the system temp dir; removed on drop.
    struct TmpDir(PathBuf);

    impl TmpDir {
        fn new(name: &str) -> TmpDir {
            let dir = std::env::temp_dir().join(format!("goport-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)
                .unwrap_or_else(|e| panic!("mkdir {}: {e}", dir.display()));
            TmpDir(dir)
        }

        fn write(&self, name: &str, text: &str) {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().expect("a parent dir"))
                .unwrap_or_else(|e| panic!("mkdir for {name}: {e}"));
            std::fs::write(path, text).unwrap_or_else(|e| panic!("write {name}: {e}"));
        }

        /// Runs `tsgo` with `args`: the exit code and stdout without the
        /// `-b` time stamps and empty lines.
        fn tsgo(&self, args: &[&str]) -> (Option<i32>, String) {
            let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
                .args(args)
                .current_dir(&self.0)
                .stdin(Stdio::null())
                .output()
                .expect("run tsgo");
            let stdout = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| match line.split_once(" - ") {
                    Some((time, rest)) if time.ends_with("AM") || time.ends_with("PM") => rest,
                    _ => line,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (output.status.code(), stdout)
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // buildtask.go:892-905 getLatestChangedDtsMTime: an upstream with
    // `noEmitOnError` and an error has an empty `latestChangedDtsFile`.
    // `incremental.ResolveBuildInfoFileName` gives the default library
    // directory for it, whose bundled `Stat` has the zero mtime, so the
    // downstream is not "up to date with .d.ts files" (buildtask.go:565).
    // Before ts#64159 the name resolved against the build info directory.
    #[test]
    fn empty_latest_changed_dts_file_has_the_zero_mtime() {
        let dir = TmpDir::new("empty-latest-changed-dts");
        dir.write(
            "a/tsconfig.json",
            r#"{"compilerOptions":{"composite":true,"noEmitOnError":true},"files":["x.ts"]}"#,
        );
        dir.write("a/x.ts", "export const x: number = \"s\";\n");
        dir.write(
            "sub/tsconfig.json",
            r#"{"compilerOptions":{"composite":true},"files":["y.ts"],"references":[{"path":"../a"}]}"#,
        );
        dir.write("sub/y.ts", "export const y = 1;\n");
        assert_eq!(dir.tsgo(&["-b", "sub"]).0, Some(1));
        assert_eq!(
            dir.tsgo(&["-b", "sub", "--verbose"]),
            (
                Some(1),
                [
                    "Projects in this build: ",
                    "    * a/tsconfig.json",
                    "    * sub/tsconfig.json",
                    "Project 'a/tsconfig.json' is out of date because buildinfo file 'a/tsconfig.tsbuildinfo' indicates that program needs to report errors.",
                    "Building project 'a/tsconfig.json'...",
                    "a/x.ts(1,14): error TS2322: Type 'string' is not assignable to type 'number'.",
                    "Project 'sub/tsconfig.json' is out of date because output 'sub/tsconfig.tsbuildinfo' is older than input 'a'",
                    "Building project 'sub/tsconfig.json'...",
                    "Updating unchanged output timestamps of project 'sub/tsconfig.json'...",
                ]
                .join("\n")
            )
        );
    }

    // buildtask.go:733-739 TsVersionOutputOfDate: since ts#64159 the build
    // info version is printed as it is, not as a file name relative to the
    // current directory.
    #[test]
    fn ts_version_output_of_date_prints_the_version_as_is() {
        let dir = TmpDir::new("ts-version-as-is");
        dir.write("tsconfig.json", r#"{"compilerOptions":{"composite":true}}"#);
        dir.write("a.ts", "export const a = 1;\n");
        assert_eq!(dir.tsgo(&["-b"]).0, Some(0));
        let build_info = dir.0.join("tsconfig.tsbuildinfo");
        let text = std::fs::read_to_string(&build_info).expect("build info");
        let start = text.find(r#""version":""#).expect("a version") + r#""version":""#.len();
        let end = start + text[start..].find('"').expect("the version end");
        std::fs::write(
            &build_info,
            format!("{}/weird/ver{}", &text[..start], &text[end..]),
        )
        .expect("write build info");
        let current = build_info_version(false);
        assert_eq!(
            dir.tsgo(&["-b", "--verbose"]),
            (
                Some(0),
                format!(
                    "Projects in this build: \n    * tsconfig.json\nProject 'tsconfig.json' is out of date because output for it was generated with version '/weird/ver' that differs with current version '{current}'\nBuilding project 'tsconfig.json'..."
                )
            )
        );
    }
}
