//! Go: `internal/execute/tsctests/showconfig_test.go` (`tsc/showConfig`
//! baselines). These are not watch inputs, so they run on this branch.
//!
//! PORT: Go `stringtestutil.Dedent` turns each leading tab into 4 spaces
//! before it removes the common indentation. The literals below use 4 spaces
//! for each Go tab, so `dedent` gives the same text.

use crate::support::runner::{FileMap, TscInput, WatchFilter, run_tsc_inputs};
use crate::support::stringtestutil::dedent;
use crate::support::vfstest::MapFile;

/// Go `FileMap{...}` literal.
fn file_map<const N: usize>(entries: [(&str, MapFile); N]) -> FileMap {
    entries
        .into_iter()
        .map(|(path, file)| (path.to_string(), file))
        .collect()
}

/// Go `[]string{...}` command line literal.
fn args<const N: usize>(args: [&str; N]) -> Vec<String> {
    args.map(String::from).into()
}

// Go: showconfig_test.go:9 TestShowConfig
#[test]
fn show_config() {
    let test_cases = vec![
        TscInput {
            sub_scenario: "Default initialized TSConfig".into(),
            command_line_args: args(["--showConfig"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with files options".into(),
            command_line_args: args(["--showConfig", "file0.ts", "file1.ts", "file2.ts"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with boolean value compiler options".into(),
            command_line_args: args(["--showConfig", "--noUnusedLocals"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with enum value compiler options".into(),
            command_line_args: args(["--showConfig", "--target", "es5", "--jsx", "react"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with list compiler options".into(),
            command_line_args: args(["--showConfig", "--types", "jquery,mocha"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with list compiler options with enum value".into(),
            command_line_args: args(["--showConfig", "--lib", "es5,es2015.core"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with incorrect compiler option".into(),
            command_line_args: args(["--showConfig", "--someNonExistOption"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with incorrect compiler option value".into(),
            command_line_args: args(["--showConfig", "--lib", "nonExistLib,es5,es2015.promise"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with advanced options".into(),
            command_line_args: args([
                "--showConfig",
                "--declaration",
                "--declarationDir",
                "lib",
                "--skipLibCheck",
                "--noErrorTruncation",
            ]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with compileOnSave and more".into(),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/index.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    dedent(
                        r#"
                {
                    "compilerOptions": {
                        "esModuleInterop": true,
                        "target": "es5",
                        "module": "commonjs",
                        "strict": true
                    },
                    "compileOnSave": true,
                    "exclude": [
                        "dist"
                    ],
                    "files": [],
                    "include": [
                        "src/*"
                    ],
                    "references": [
                        { "path": "./test" }
                    ]
                }"#,
                    )
                    .into(),
                ),
            ]),
            command_line_args: args(["-p", "tsconfig.json", "--showConfig"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with paths and more".into(),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/index.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    dedent(
                        r#"
                {
                    "compilerOptions": {
                        "allowJs": true,
                        "outDir": "./lib",
                        "esModuleInterop": true,
                        "module": "commonjs",
                        "moduleResolution": "node",
                        "target": "ES2017",
                        "sourceMap": true,
                        "baseUrl": ".",
                        "paths": {
                            "@root/*": ["./*"],
                            "@configs/*": ["src/configs/*"],
                            "@common/*": ["src/common/*"],
                            "*": [
                                "node_modules/*",
                                "src/types/*"
                            ]
                        },
                        "experimentalDecorators": true,
                        "emitDecoratorMetadata": true,
                        "resolveJsonModule": true
                    },
                    "include": [
                        "./src/**/*"
                    ]
                }"#,
                    )
                    .into(),
                ),
            ]),
            command_line_args: args(["-p", "tsconfig.json", "--showConfig"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with include filtering files".into(),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/main.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/src/util.ts",
                    "export const b = 2;".into(),
                ),
                (
                    "/home/src/workspaces/project/extra.ts",
                    "export const c = 3;".into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    dedent(
                        r#"
                {
                    "compilerOptions": {
                        "strict": true
                    },
                    "include": [
                        "src/**/*"
                    ]
                }"#,
                    )
                    .into(),
                ),
            ]),
            command_line_args: args(["-p", "tsconfig.json", "--showConfig"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with references".into(),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/index.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    dedent(
                        r#"
                {
                    "compilerOptions": {
                        "composite": true,
                        "strict": true
                    },
                    "references": [
                        { "path": "./packages/a" },
                        { "path": "./packages/b" }
                    ]
                }"#,
                    )
                    .into(),
                ),
            ]),
            command_line_args: args(["-p", "tsconfig.json", "--showConfig"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with exclude".into(),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/index.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/test/test1.ts",
                    r#"import { a } from "../src";"#.into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    dedent(
                        r#"
                {
                    "compilerOptions": {
                        "strict": true
                    },
                    "exclude": [
                        "test"
                    ]
                }"#,
                    )
                    .into(),
                ),
            ]),
            command_line_args: args(["-p", "tsconfig.json", "--showConfig"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with files and include".into(),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/main.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/extra.ts",
                    "export const c = 3;".into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    dedent(
                        r#"
                {
                    "compilerOptions": {
                        "strict": true
                    },
                    "files": [
                        "extra.ts"
                    ],
                    "include": [
                        "src/**/*"
                    ]
                }"#,
                    )
                    .into(),
                ),
            ]),
            command_line_args: args(["-p", "tsconfig.json", "--showConfig"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with transitively implied options".into(),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/index.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    dedent(
                        r#"
                {
                    "compilerOptions": {
                        "module": "nodenext"
                    }
                }"#,
                    )
                    .into(),
                ),
            ]),
            command_line_args: args(["-p", "tsconfig.json", "--showConfig"]),
            ..Default::default()
        },
        TscInput {
            sub_scenario: "Show TSConfig with exclude and outDir".into(),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/index.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/src/bin/tool.ts",
                    "export const b = 2;".into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    dedent(
                        r#"
                {
                    "compilerOptions": {
                        "strict": true,
                        "outDir": "./build"
                    },
                    "exclude": [
                        "build"
                    ]
                }"#,
                    )
                    .into(),
                ),
            ]),
            command_line_args: args(["-p", "tsconfig.json", "--showConfig"]),
            ..Default::default()
        },
    ];

    // ts#64457 (showconfig_test.go:217): the file listing flags on the command
    // line and in the config.
    let mut test_cases = test_cases;
    for value in ["true", "false"] {
        test_cases.push(TscInput {
            sub_scenario: format!("Show TSConfig with command line file listing flags {value}"),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/index.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    r#"{"files": ["src/index.ts"]}"#.into(),
                ),
            ]),
            command_line_args: args([
                "--showConfig",
                "--listFiles",
                value,
                "--listEmittedFiles",
                value,
                "--listFilesOnly",
                value,
                "--explainFiles",
                value,
            ]),
            ..Default::default()
        });
        test_cases.push(TscInput {
            sub_scenario: format!("Show TSConfig with configured file listing flags {value}"),
            files: file_map([
                (
                    "/home/src/workspaces/project/src/index.ts",
                    "export const a = 1;".into(),
                ),
                (
                    "/home/src/workspaces/project/tsconfig.json",
                    dedent(&format!(
                        r#"
                {{
                    "compilerOptions": {{
                        "listFiles": {value},
                        "listEmittedFiles": {value},
                        "explainFiles": {value}
                    }},
                    "files": ["src/index.ts"]
                }}"#
                    ))
                    .into(),
                ),
            ]),
            command_line_args: args(["--showConfig"]),
            ..Default::default()
        });
    }

    run_tsc_inputs("showConfig", test_cases, WatchFilter::NonWatch);
}
