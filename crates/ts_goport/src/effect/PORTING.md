# Porting the Effect diagnostics

`crates/ts_goport/src/effect` is a line-by-line port of Effect-TS/tsgo at
the `@effect/tsgo@0.51.1` release commit
`47cb1ed7704aff44cacaa0f4d2ef24de990cba0e` (tag `@effect/tsgo@0.51.1`). The
Go source is the clone `~/.explore/repos/Effect-TS__tsgo` (read-only, full
history; its HEAD is not the tag, so read files with
`git show 47cb1ed7:<path>` or from a checkout at the tag). Go package
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
- Patch 031 (`stability-tag-fast-path`) adds a scanner token flag and a
  parser node flag (`NodeFlagsPossiblyContainsStabilityTag`, 1<<29) set when
  a JSDoc comment in a node's leading trivia has `@stability`. The port does
  not change the parser: the API encoder writes raw node flags, so the flag
  would change plain API output for every file with the tag, and a parser
  change makes the lib blobs stale. `possibly_contains_stability_tag(node)`
  (`typeparser/api_stability_p1.rs`) gets the same answer from the comment
  text: `HasJSDoc` is set, and a `/**` comment of the trailing and leading
  comment ranges at `node.pos()` (what the scanner scanned before the first
  token) passes Go `scanJSDocCommentForTags` for `stability`. The result is
  used only together with the JSDoc tag parse of the same node.
- Patch 032 (`vfsmatch-root-include`) changes TypeScript core
  (`getIncludeBasePath` for a file include directly in a root dir). It is
  not ported: plain output must equal tsgo at pin N, which keeps
  `RemoveTrailingDirectorySeparator`. With the plugin, an `include` such as
  `/index.ts` can match other files than in effect-tsgo 0.51.1.
- The Effect submodule moved to TypeScript `a1ef42b9` (#775, #779, #783,
  #795). The port follows its own TypeScript pin N, so only Effect behavior
  is ported, not their TypeScript move.
- `tsc -v`, `--help` and the language server `serverInfo` print the plain
  version (one server serves projects with and without the plugin). Only
  build info records `<version>+effect-tsgo.0.51.1`
  (`etscore::EFFECT_VERSION`).

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
  body becomes a block that can use `self`).
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

## API stability rules (0.51.1)

`experimentalApiUsage` (TS377135), `unstableApiUsage` (TS377136) and
`apiStabilityLeak` (TS377137, related TS377138) come from #772, #781, #787,
#790, #793, #796 and #798. Go `internal/typeparser/api_stability.go` is 5,023
lines, so the port splits it in Go order: `api_stability_p1.rs` (Go 1-1153:
levels, declared lookups, session, result conversion, keys, analysis state),
`api_stability_p2.rs` (Go 1154-2682: settlement, shared surfaces, symbol and
type surfaces) and `api_stability_p3.rs` (Go 2683-5023: declaration
surfaces, signatures and substitution, provenance, child boundaries, filters).
`api_stability_safety.rs` and `api_stability_inheritance.rs` are the Go files
of the same names. `checker_integration.rs` is the Effect-owned Go shim
`shim/checker/integration.go` (cached-field peeks such as
`GetResolvedMembersOfTypeIfMaterialized`); call it as
`checker_integration::foo(c, ..)`.

- Go `apiStabilityAnalysis` keeps `tp`. Here a session lives across rule
  calls that also use the type parser, so analysis methods take
  `tp: &mut TypeParser<'_>`. The safety scan holds `a: &mut
  ApiStabilityAnalysis` (Go `s.a`).
- Go `*apiStabilityCarrier` is an interned index (`ApiStabilityCarrierId`, 0
  is nil). Go `apiStabilitySubstitution` holds `Option<Rc<frame>>`.
- Go map iteration order is random. Session and finding maps are
  `FxIndexMap` (insertion order); Go sorts exports, member table keys and
  dependencies, and the port sorts them the same way with Go byte order
  (`scanner_util::compare_go_strings`).
- `materialized_value_symbol_links` (the only `value_symbol_links` read) calls
  `get_symbol_id` first, as Go `symbolArenaLinkStore.TryGet` gives the
  symbol its id. Node ids from `nodeLinkStore.TryGet` are not given (the
  port never gives them on link reads).
- A surface clones its findings map where Go copies a struct that shares
  the map. Go never writes a shared map, so results are the same.
- The allow lists (`allowedUnstableApis`, `allowedExperimentalApis`, root
  and `overrides`, strict string arrays) are `Option<Vec<String>>`: Go
  writes a root `[]` to build info, and its `cloneEffectOptions` (through
  `extends`) drops it (`configraw.rs` maps `Some([])` to `None` there).
- Go `omittedArgumentDefaults` fills one `args` slice that its binding frame
  shares; the port makes one frame per default from the filled prefix. No
  frame outlives its scan, so the views are the same.
- Dead Go branches are left out with a note: the `IndexSignature` case of
  `typeNodeMentionsReplacedParameter` and of `collectDeclarationProvenance`
  (an index signature has function-like data, so an earlier branch takes it).
- The rule walker (`rules/api_stability_leak.rs`) holds only its own state;
  its methods take `ctx`.
- Group and preset `maintainers` (`rules/metadata.go`) feed only the
  `effect-tsgo setup` CLI, the Oxlint presets and `metadata.json`. The
  tsconfig plugin entry has no preset key, so the port keeps only
  `group: "maintainers"` on the rule.

Checks: `scripts/effect/reference-cases.mjs` (542 cases at the tag; its
default tsconfig turns the 3 rules off, as Go `effecttest.DefaultTsConfig`),
`scripts/effect/go-unit-cases.mjs` (the inline sources of the Go unit tests,
the 3 rules on, `--pretty false` and `--pretty true`), and the fixtures
`unstable-api-usage`, `allowed-unstable-apis`, `experimental-api-usage` and
`api-stability-leak`.
