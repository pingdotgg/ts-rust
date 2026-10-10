# Porting the Effect diagnostics

`crates/ts_goport/src/effect` is a line-by-line port of Effect-TS/tsgo at
the `@effect/tsgo@0.46.1` release commit
`f1a7cad0292d9d315e7f87694f127f605711d55e`. The Go source is checked out at
`~/.cache/repo-explorer/Effect-TS-tsgo` (read-only). Go package
`internal/X` becomes `crate::effect::X`, and each Go file becomes one Rust
file with the same base name (`effect_type.go` -> `effect_type.rs`).

The main port contract is `crates/ts_goport/PORTING.md`. Read its "Names"
and "Types" sections and follow them. This file adds the Effect rules.

## Ownership and builds

- Write only the files your task names. Do not edit `mod.rs` files, the
  checker, or another agent's files.
- Do not run Cargo. Root builds and sends compile errors back.
- Start every file with `use crate::prelude::*;` and
  `use crate::effect::typeparser::*;` (rules also add
  `use crate::effect::rule::*;`, `use crate::effect::etscore::*;` and
  `use crate::effect::diag;`). Unused imports are allowed.
- Run `rustfmt --edition 2024 <file>` on each file you wrote before you
  finish.
- Port every Go function and type in your files, in Go order, with the Go
  logic intact. Keep Go comments that explain behavior. Do not simplify,
  merge or improve. Skip `_test.go` files, and skip code that only
  editor features use (quick fixes, refactors, completions, hover) unless a
  diagnostic path calls it.
- If something cannot be ported, call `unported!("goName")` there. Never
  return a guessed value. List every `unported!` in your final answer.

## Where the port differs from the reference

- Patch 002 runs the rules inside the type-check block of
  `checkSourceFile`, before the unused-identifier check. The port runs them
  after that check, once per file (`SourceFileLinks.effect_checked`,
  `checker/checker_p03.rs`). A rule's type queries can mark a name
  referenced, for example the parameter of an `x is T` type predicate, and
  the reference then loses TS6133 for it. With the plugin the unused check
  always runs before the rules. Without `noUnusedLocals` and
  `noUnusedParameters` it adds only suggestions (Go `reportUnused`), so
  `tsc` output does not change, and a later suggestion request gets the
  TS6133 that plain tsgo gives.
- A standalone API process runs no rules unless `TSGO_EFFECT_API=1`
  (`rulerunner::enabled_options`).

## Go shim -> Rust

Effect code calls typescript-go through `github.com/microsoft/TypeScript/tsc/shim/...`.
Each shim function forwards to the internal Go function of the same name.
The Go internals are at `~/.explore/repos/microsoft__TypeScript@673a5f17d713/tsc/internal`
(the same Go the Rust crate ports; Effect pins a close revision).

- `checker.Checker_fooBar(c, a, b)` (an unexported checker method) ->
  `c.foo_bar(a, b)`.
- `c.FooBar(a)` (an exported checker method, `checker/exports.go` or
  `services.go`) -> find the Rust port with
  `grep -rn "pub fn foo_bar" crates/ts_goport/src/checker`. Exported and
  unexported Go methods with the same snake name: the exported one may be
  `foo_bar_exported`, or only the unexported one may exist with an extra
  argument. Read the Go exported wrapper and call what it calls. If the
  Rust crate lacks an exported wrapper, port the Go wrapper body as a
  private free function in your own file: `fn foo_bar(c: &mut Checker, ..)`.
- `ast.X`, `scanner.X`, `tspath.X`, `core.X` -> the snake-case free
  function (`grep -rn "pub fn x\b" crates/ts_goport/src`).
- `*checker.Type` -> `TypeId` (Go nil is `TypeId::NIL`). Type fields and
  methods go through the checker: `t.Flags()` -> `c.ty(t).flags`,
  `t.Symbol()` -> `c.ty(t).symbol`, `t.Types()` -> `c.ty(t).types()`.
- `*ast.Node`, `*ast.SourceFile` and every node alias -> `Node`.
  `node.ForEachChild(visit)` -> `node.for_each_child(|n| ..)` (returns
  `bool`, true stops). `sf.Text()` -> `source_file_text(sf)`,
  `sf.FileName()` -> `source_file_file_name(sf)`.
- `*ast.Symbol` -> `SymbolId`; `symbol.Name` -> `c.sym(s).name` (check the
  field type), `symbol.Flags` -> `c.sym(s).flags`,
  `symbol.Declarations` -> `c.sym(s).declarations`.
- `*ast.Diagnostic` -> `Diagnostic`. `*diagnostics.Message` -> `&'static Message`.
  Effect messages are `diag::Name` from `crate::effect::diag` (the Go
  `tsdiag.Name`, generated from `effectDiagnosticMessages.json`).
- Go `core.TextRange` -> `TextRange`.

## Type parser shape

`effect/typeparser/mod.rs` defines `TypeParser<'c> { program, checker }` and
`EffectLinks` (Go `EffectLinks`, one per checker). Read it first.

- Go `func (tp *TypeParser) FooBar(..) R` -> a method in an
  `impl TypeParser<'_>` block in your file: `pub fn foo_bar(&mut self, ..) -> R`.
  The checker is `self.checker`, the program `self.program`.
- Go free functions that take `tp` (and maybe `c`) -> `pub fn foo(tp: &mut TypeParser<'_>, ..)`.
  Drop the separate `c *checker.Checker` parameter: use `tp.checker`.
  Go free functions that take only `c` -> `pub fn foo(c: &mut Checker, ..)`.
- Go `Cached(&tp.links.Foo, key, func() V { .. })` ->
  `cached!(self, foo, key, { .. })` (the macro is in `mod.rs`; the closure
  body becomes a block that can use `self`). The large node caches
  (`TypeAtLocation`, `ReferenceSymbol`, `ParseEffectFnOpportunity`, the
  effect context stores) use node link pages instead of hash maps, and
  their call sites spell out the cache; the `EffectLinks` doc says why.
- Go struct results: a Go `*T` result -> `Option<Rc<T>>`. A Go `[]*T` ->
  `Vec<Rc<T>>`; a Go `[]T` -> `Vec<T>`. Every Go type and field is `pub`.
  Go field names become snake case. Keep Go type names (an unexported Go
  type `fooBar` becomes `pub struct FooBar`). The cache value types in
  `EffectLinks` are fixed: define the types it names in the file that
  defines them in Go, with exactly those names.
- Go enums of `int` consts -> a Rust `enum` or `go_flags!` type with the Go
  names, as the main contract describes.

## Rules shape

`effect/rule.rs` defines `Rule` and `RuleContext` (Go `rule.Context`; the
Rust name avoids the Go `context.Context` port, `Context`). Read it first.
Each Go rule file `internal/rules/foo_bar.go` defines `var FooBar = rule.Rule{..}`.
Port it as `pub static FOO_BAR: Rule = Rule { .., run: run_foo_bar };` plus
`fn run_foo_bar(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic>` (the Go
`Run` closure body). `Codes` becomes a `&[i32]` list of the literal message
codes (see `crates/ts_goport/src/diagnostics/effect_catalog.rs`).
`SupportedEffect` becomes `&["v3", "v4"]`. `DefaultSeverity:
etscore.SeverityError` becomes `Severity::Error`.

- Go `ctx.Checker` -> `ctx.tp.checker` (a `&mut Checker`), Go
  `ctx.TypeParser` -> `ctx.tp`, `ctx.SourceFile` -> `ctx.source_file`,
  `ctx.Options` -> `ctx.options`, `ctx.Program` -> `ctx.program`.
- `ctx.NewDiagnostic(sf, loc, msg, related, args...)` ->
  `ctx.new_diagnostic(sf, loc, diag::Msg, related_vec, vec![args..])`.
- Go analysis helpers that rules export for quick fixes
  (`AnalyzeFloatingEffect` and similar) stay `pub fn` with the type parser
  as the first parameter: `pub fn analyze_floating_effect(tp: &mut TypeParser<'_>, sf: Node)`.
  Drop Go's separate `c *checker.Checker` parameter; use `tp.checker`.
- Skip code that only quick fixes use (`internal/fixables`), but keep any
  helper the rule's own `Run` reaches.
