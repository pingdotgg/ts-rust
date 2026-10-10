//! Port of Effect-TS/tsgo `internal/rules/node_builtin_import.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

#[derive(Clone, Copy, Debug)]
struct ModuleAlternative {
    alternative: &'static str,
    package: &'static str,
    module: &'static str,
}

// PORT: Go maps `map[string]moduleAlternative`; the port keeps the entries in
// slices and looks them up by key (`lookup_module_alternative`).
static MODULE_ALTERNATIVES_V3: &[(&str, ModuleAlternative)] = &[
    (
        "fs",
        ModuleAlternative {
            alternative: "FileSystem",
            package: "@effect/platform",
            module: "fs",
        },
    ),
    (
        "node:fs",
        ModuleAlternative {
            alternative: "FileSystem",
            package: "@effect/platform",
            module: "fs",
        },
    ),
    (
        "fs/promises",
        ModuleAlternative {
            alternative: "FileSystem",
            package: "@effect/platform",
            module: "fs",
        },
    ),
    (
        "node:fs/promises",
        ModuleAlternative {
            alternative: "FileSystem",
            package: "@effect/platform",
            module: "fs",
        },
    ),
    (
        "path",
        ModuleAlternative {
            alternative: "Path",
            package: "@effect/platform",
            module: "path",
        },
    ),
    (
        "node:path",
        ModuleAlternative {
            alternative: "Path",
            package: "@effect/platform",
            module: "path",
        },
    ),
    (
        "path/posix",
        ModuleAlternative {
            alternative: "Path",
            package: "@effect/platform",
            module: "path",
        },
    ),
    (
        "node:path/posix",
        ModuleAlternative {
            alternative: "Path",
            package: "@effect/platform",
            module: "path",
        },
    ),
    (
        "path/win32",
        ModuleAlternative {
            alternative: "Path",
            package: "@effect/platform",
            module: "path",
        },
    ),
    (
        "node:path/win32",
        ModuleAlternative {
            alternative: "Path",
            package: "@effect/platform",
            module: "path",
        },
    ),
    (
        "child_process",
        ModuleAlternative {
            alternative: "CommandExecutor",
            package: "@effect/platform",
            module: "child_process",
        },
    ),
    (
        "node:child_process",
        ModuleAlternative {
            alternative: "CommandExecutor",
            package: "@effect/platform",
            module: "child_process",
        },
    ),
    (
        "http",
        ModuleAlternative {
            alternative: "HttpClient",
            package: "@effect/platform",
            module: "http",
        },
    ),
    (
        "node:http",
        ModuleAlternative {
            alternative: "HttpClient",
            package: "@effect/platform",
            module: "http",
        },
    ),
    (
        "https",
        ModuleAlternative {
            alternative: "HttpClient",
            package: "@effect/platform",
            module: "https",
        },
    ),
    (
        "node:https",
        ModuleAlternative {
            alternative: "HttpClient",
            package: "@effect/platform",
            module: "https",
        },
    ),
    (
        "console",
        ModuleAlternative {
            alternative: "Console",
            package: "effect",
            module: "console",
        },
    ),
    (
        "node:console",
        ModuleAlternative {
            alternative: "Console",
            package: "effect",
            module: "console",
        },
    ),
    (
        "timers",
        ModuleAlternative {
            alternative: "Effect",
            package: "effect",
            module: "timers",
        },
    ),
    (
        "node:timers",
        ModuleAlternative {
            alternative: "Effect",
            package: "effect",
            module: "timers",
        },
    ),
    (
        "timers/promises",
        ModuleAlternative {
            alternative: "Effect",
            package: "effect",
            module: "timers",
        },
    ),
    (
        "node:timers/promises",
        ModuleAlternative {
            alternative: "Effect",
            package: "effect",
            module: "timers",
        },
    ),
    (
        "stream",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "node:stream",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "stream/promises",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "node:stream/promises",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "stream/web",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "node:stream/web",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
];

static MODULE_ALTERNATIVES_V4: &[(&str, ModuleAlternative)] = &[
    (
        "fs",
        ModuleAlternative {
            alternative: "FileSystem",
            package: "effect",
            module: "fs",
        },
    ),
    (
        "node:fs",
        ModuleAlternative {
            alternative: "FileSystem",
            package: "effect",
            module: "fs",
        },
    ),
    (
        "fs/promises",
        ModuleAlternative {
            alternative: "FileSystem",
            package: "effect",
            module: "fs",
        },
    ),
    (
        "node:fs/promises",
        ModuleAlternative {
            alternative: "FileSystem",
            package: "effect",
            module: "fs",
        },
    ),
    (
        "path",
        ModuleAlternative {
            alternative: "Path",
            package: "effect",
            module: "path",
        },
    ),
    (
        "node:path",
        ModuleAlternative {
            alternative: "Path",
            package: "effect",
            module: "path",
        },
    ),
    (
        "path/posix",
        ModuleAlternative {
            alternative: "Path",
            package: "effect",
            module: "path",
        },
    ),
    (
        "node:path/posix",
        ModuleAlternative {
            alternative: "Path",
            package: "effect",
            module: "path",
        },
    ),
    (
        "path/win32",
        ModuleAlternative {
            alternative: "Path",
            package: "effect",
            module: "path",
        },
    ),
    (
        "node:path/win32",
        ModuleAlternative {
            alternative: "Path",
            package: "effect",
            module: "path",
        },
    ),
    (
        "child_process",
        ModuleAlternative {
            alternative: "ChildProcess",
            package: "effect/process",
            module: "child_process",
        },
    ),
    (
        "node:child_process",
        ModuleAlternative {
            alternative: "ChildProcess",
            package: "effect/process",
            module: "child_process",
        },
    ),
    (
        "http",
        ModuleAlternative {
            alternative: "HttpClient",
            package: "effect/http",
            module: "http",
        },
    ),
    (
        "node:http",
        ModuleAlternative {
            alternative: "HttpClient",
            package: "effect/http",
            module: "http",
        },
    ),
    (
        "https",
        ModuleAlternative {
            alternative: "HttpClient",
            package: "effect/http",
            module: "https",
        },
    ),
    (
        "node:https",
        ModuleAlternative {
            alternative: "HttpClient",
            package: "effect/http",
            module: "https",
        },
    ),
    (
        "console",
        ModuleAlternative {
            alternative: "Console",
            package: "effect",
            module: "console",
        },
    ),
    (
        "node:console",
        ModuleAlternative {
            alternative: "Console",
            package: "effect",
            module: "console",
        },
    ),
    (
        "timers",
        ModuleAlternative {
            alternative: "Effect",
            package: "effect",
            module: "timers",
        },
    ),
    (
        "node:timers",
        ModuleAlternative {
            alternative: "Effect",
            package: "effect",
            module: "timers",
        },
    ),
    (
        "timers/promises",
        ModuleAlternative {
            alternative: "Effect",
            package: "effect",
            module: "timers",
        },
    ),
    (
        "node:timers/promises",
        ModuleAlternative {
            alternative: "Effect",
            package: "effect",
            module: "timers",
        },
    ),
    (
        "stream",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "node:stream",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "stream/promises",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "node:stream/promises",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "stream/web",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "node:stream/web",
        ModuleAlternative {
            alternative: "Stream",
            package: "effect",
            module: "stream",
        },
    ),
    (
        "crypto",
        ModuleAlternative {
            alternative: "Crypto",
            package: "effect",
            module: "crypto",
        },
    ),
    (
        "node:crypto",
        ModuleAlternative {
            alternative: "Crypto",
            package: "effect",
            module: "crypto",
        },
    ),
];
fn lookup_module_alternative(
    alternatives: &[(&str, ModuleAlternative)],
    specifier: &str,
) -> Option<ModuleAlternative> {
    alternatives
        .iter()
        .find(|(key, _)| *key == specifier)
        .map(|(_, alt)| *alt)
}

pub static NODE_BUILTIN_IMPORT: Rule = Rule {
    name: "nodeBuiltinImport",
    group: "effectNative",
    description: "Warns when importing Node.js built-in modules that have Effect-native counterparts",
    default_severity: Severity::Off,
    supported_effect: &["v3", "v4"],
    codes: &[377057],
    run: run_node_builtin_import,
};

fn run_node_builtin_import(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let matches = analyze_node_builtin_import(ctx.tp, ctx.source_file);
    let mut diags = Vec::with_capacity(matches.len());
    for m in &matches {
        diags.push(ctx.new_diagnostic(
            m.source_file,
            m.location,
            diag::This_module_reference_uses_the_2_module_the_corresponding_Effect_API_is_0_from_1_effect_nodeBuiltinImport,
            Vec::new(),
            vec![m.alternative.clone(), m.package.clone(), m.module.clone()],
        ));
    }
    diags
}

#[derive(Clone, Debug)]
pub struct NodeBuiltinImportMatch {
    pub source_file: Node,
    pub location: TextRange,
    pub alternative: String,
    pub package: String,
    pub module: String,
}

pub fn analyze_node_builtin_import(
    tp: &mut TypeParser<'_>,
    sf: Node,
) -> Vec<NodeBuiltinImportMatch> {
    let mut alternatives = MODULE_ALTERNATIVES_V4;
    if tp.supported_effect_version() == EffectMajorVersion::V3 {
        alternatives = MODULE_ALTERNATIVES_V3;
    }

    let mut matches = Vec::new();

    for stmt in sf.statements().iter() {
        match stmt.kind() {
            SyntaxKind::ImportDeclaration => {
                let import_decl = stmt;
                if import_decl.module_specifier().is_nil()
                    || import_decl.module_specifier().kind() != SyntaxKind::StringLiteral
                {
                    continue;
                }
                let specifier = import_decl.module_specifier().text();
                if let Some(alt) = lookup_module_alternative(alternatives, specifier) {
                    matches.push(NodeBuiltinImportMatch {
                        source_file: sf,
                        location: get_error_range_for_node(sf, import_decl.module_specifier()),
                        alternative: alt.alternative.to_string(),
                        package: alt.package.to_string(),
                        module: alt.module.to_string(),
                    });
                }
            }

            SyntaxKind::VariableStatement => {
                let var_stmt = stmt;
                if var_stmt.declaration_list().is_nil() {
                    continue;
                }
                let decl_list = var_stmt.declaration_list();
                if decl_list.declarations().is_nil() {
                    continue;
                }
                for decl_node in decl_list.declarations().nodes().iter() {
                    let decl = decl_node;
                    if decl.initializer().is_nil()
                        || decl.initializer().kind() != SyntaxKind::CallExpression
                    {
                        continue;
                    }
                    let call_expr = decl.initializer();
                    if call_expr.expression().is_nil()
                        || call_expr.expression().kind() != SyntaxKind::Identifier
                    {
                        continue;
                    }
                    if get_text_of_node(call_expr.expression()) != "require" {
                        continue;
                    }
                    if call_expr.arguments().len() != 1 {
                        continue;
                    }
                    let arg = call_expr.arguments().get(0);
                    if arg.kind() != SyntaxKind::StringLiteral {
                        continue;
                    }
                    let specifier = arg.text();
                    if let Some(alt) = lookup_module_alternative(alternatives, specifier) {
                        matches.push(NodeBuiltinImportMatch {
                            source_file: sf,
                            location: get_error_range_for_node(sf, arg),
                            alternative: alt.alternative.to_string(),
                            package: alt.package.to_string(),
                            module: alt.module.to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    matches
}
