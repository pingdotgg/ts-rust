//! Ports of internal/transformers/tstransforms/typeeraser_test.go and
//! importelision_test.go.

use super::Subtests;
use super::childprog::{in_child, install_map_fs, new_program, source_file};
use super::emittestutil::check_emit;
use super::parsetestutil::{check_diagnostics, parse_type_script_published};
use crate::support::vfstest::MapFs;
use ts_goport::checker::emit_resolver_p1::new_emit_resolver_of_shared_checker;
use ts_goport::gostd::context;
use ts_goport::prelude::*;
use ts_goport::program::ls_program;
use ts_goport::transformers::transformer::{
    EmitResolverReferenceResolver, TransformOptions, TransformReferenceResolver,
};
use ts_goport::transformers::tstransforms::{
    new_import_elision_transformer, new_type_eraser_transformer,
};

/// Go nil `EmitResolver` and `Resolver` in `transformers.TransformOptions`.
/// Every call panics, as a call on a Go nil interface does.
struct NilResolver;

fn nil_emit_resolver() -> ! {
    panic!("invalid memory address or nil pointer dereference: nil EmitResolver")
}

impl TransformReferenceResolver for NilResolver {
    fn get_referenced_export_container(&self, _node: Node, _prefix_locals: bool) -> Node {
        nil_emit_resolver()
    }
    fn get_referenced_import_declaration(&self, _node: Node) -> Node {
        nil_emit_resolver()
    }
    fn get_referenced_value_declaration(&self, _node: Node) -> Node {
        nil_emit_resolver()
    }
    fn get_referenced_value_declarations(&self, _node: Node) -> Vec<Node> {
        nil_emit_resolver()
    }
    fn get_element_access_expression_name(&self, _expression: Node) -> String {
        nil_emit_resolver()
    }
    fn get_referenced_member_value_declaration(&self, _node: Node) -> Node {
        nil_emit_resolver()
    }
}

#[allow(unused_variables)]
impl ts_goport::printer::EmitResolver for NilResolver {
    fn emit_context(&self) -> &Rc<EmitContext> {
        nil_emit_resolver()
    }
    fn get_referenced_export_container(&self, node: Node, prefix_locals: bool) -> Node {
        nil_emit_resolver()
    }
    fn get_referenced_import_declaration(&self, node: Node) -> Node {
        nil_emit_resolver()
    }
    fn get_referenced_value_declarations(&self, node: Node) -> Vec<Node> {
        nil_emit_resolver()
    }
    fn get_referenced_value_declaration(&self, node: Node) -> Node {
        nil_emit_resolver()
    }
    fn get_element_access_expression_name(&self, expression: Node) -> String {
        nil_emit_resolver()
    }
    fn get_referenced_member_value_declaration(&self, node: Node) -> Node {
        nil_emit_resolver()
    }
    fn symbol_value_declaration(&self, symbol: SymbolId) -> Node {
        nil_emit_resolver()
    }
    fn make_symbol_table(&self, entries: &[(&str, SymbolId)]) -> SymbolTable {
        nil_emit_resolver()
    }
    fn is_referenced_alias_declaration(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn is_value_alias_declaration(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn is_top_level_value_import_equals_with_entity_name(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn mark_linked_references_recursively(&self, file: Node) {
        nil_emit_resolver()
    }
    fn get_external_module_file_from_declaration(&self, node: Node) -> Node {
        nil_emit_resolver()
    }
    fn get_effective_declaration_flags(&self, node: Node, flags: ModifierFlags) -> ModifierFlags {
        nil_emit_resolver()
    }
    fn get_type_reference_serialization_kind(
        &self,
        name: Node,
        serial_scope: Node,
    ) -> TypeReferenceSerializationKind {
        nil_emit_resolver()
    }
    fn get_constant_value(&self, node: Node) -> Option<LiteralValue> {
        nil_emit_resolver()
    }
    fn get_jsx_factory_entity(&self, location: Node) -> Node {
        nil_emit_resolver()
    }
    fn get_jsx_fragment_factory_entity(&self, location: Node) -> Node {
        nil_emit_resolver()
    }
    fn set_referenced_import_declaration(&self, node: Node, ref_: Node) {
        nil_emit_resolver()
    }
    fn precalculate_declaration_emit_visibility(&self, file: Node) {
        nil_emit_resolver()
    }
    fn is_symbol_accessible(
        &self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        should_compute_alias_to_mark_visible: bool,
    ) -> SymbolAccessibilityResult {
        nil_emit_resolver()
    }
    fn is_entity_name_visible(
        &self,
        entity_name: Node,
        enclosing_declaration: Node,
    ) -> SymbolAccessibilityResult {
        nil_emit_resolver()
    }
    fn is_expando_function_declaration(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn is_expando_function_declaration_unsafe(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn is_literal_const_declaration(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn requires_adding_implicit_undefined(
        &self,
        node: Node,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool {
        nil_emit_resolver()
    }
    fn is_declaration_visible(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn is_name_resolvable(&self, location: Node, name: &str) -> bool {
        nil_emit_resolver()
    }
    fn is_import_required_by_augmentation(&self, decl: Node) -> bool {
        nil_emit_resolver()
    }
    fn is_definitely_reference_to_global_symbol_object(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn is_implementation_of_overload(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn get_enum_member_value(&self, node: Node) -> EvaluatorResult {
        nil_emit_resolver()
    }
    fn is_late_bound(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn is_optional_parameter(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn is_this_property_assignment_declaration_redundant(&self, node: Node) -> bool {
        nil_emit_resolver()
    }
    fn get_properties_of_container_function(&self, node: Node) -> Vec<SymbolId> {
        nil_emit_resolver()
    }
    fn requires_adding_implicit_undefined_unsafe(
        &self,
        node: Node,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool {
        nil_emit_resolver()
    }
    fn get_referenced_value_declaration_unsafe(&self, node: Node) -> Node {
        nil_emit_resolver()
    }
    fn create_type_of_declaration(
        &self,
        declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        nil_emit_resolver()
    }
    fn create_return_type_of_signature_declaration(
        &self,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        nil_emit_resolver()
    }
    fn create_type_parameters_of_signature_declaration(
        &self,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Vec<Node> {
        nil_emit_resolver()
    }
    fn create_literal_const_value(&self, node: Node, tracker: EmitSymbolTracker) -> Node {
        nil_emit_resolver()
    }
    fn create_type_of_expression(
        &self,
        expression: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        nil_emit_resolver()
    }
    fn create_late_bound_index_signatures(
        &self,
        container: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Vec<Node> {
        nil_emit_resolver()
    }
    fn try_js_type_node_to_type_node(
        &self,
        type_node: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        nil_emit_resolver()
    }
}

/// Go `&transformers.TransformOptions{CompilerOptions: compilerOptions,
/// Context: emitContext}`: no emit resolver and no resolver.
pub(crate) fn transform_options(
    compiler_options: &'static CompilerOptions,
    emit_context: &Rc<EmitContext>,
) -> TransformOptions {
    TransformOptions {
        context: Rc::clone(emit_context),
        compiler_options,
        resolver: Rc::new(NilResolver),
        emit_resolver: Rc::new(NilResolver),
        get_emit_module_format_of_file: Rc::new(|_file: Node| -> ModuleKind {
            panic!("invalid memory address or nil pointer dereference: GetEmitModuleFormatOfFile")
        }),
    }
}

/// Go TestTypeEraser rows: (title, input, output, jsx, vms).
#[rustfmt::skip]
const TYPE_ERASER: &[(&str, &str, &str, bool, bool)] = &[
    ("Modifiers", "class C { public x; private y }", "class C {\n    x;\n    y;\n}", false, false),
    ("InterfaceDeclaration", "interface I { }", "", false, false),
    ("TypeAliasDeclaration", "type T = U;", "", false, false),
    ("NamespaceExportDeclaration", "export as namespace N;", "", false, false),
    ("UninstantiatedNamespace1", "namespace N {}", "", false, false),
    ("UninstantiatedNamespace2", "namespace N { export interface I {} }", "", false, false),
    ("UninstantiatedNamespace3", "namespace N { export type T = U; }", "", false, false),
    ("ExpressionWithTypeArguments", "F<T>", "F;", false, false),
    ("PropertyDeclaration1", "class C { declare x; }", "class C {\n}", false, false),
    ("PropertyDeclaration2", "class C { public x: number; }", "class C {\n    x;\n}", false, false),
    ("PropertyDeclaration3", "class C { public static x: number; }", "class C {\n    static x;\n}", false, false),
    ("ConstructorDeclaration1", "class C { constructor(); }", "class C {\n}", false, false),
    ("ConstructorDeclaration2", "class C { public constructor() {} }", "class C {\n    constructor() { }\n}", false, false),
    ("MethodDeclaration1", "class C { m(); }", "class C {\n}", false, false),
    ("MethodDeclaration2", "class C { public m<T>(): U {} }", "class C {\n    m() { }\n}", false, false),
    ("MethodDeclaration3", "class C { public static m<T>(): U {} }", "class C {\n    static m() { }\n}", false, false),
    ("GetAccessorDeclaration1", "class C { get m(); }", "class C {\n    get m() { }\n}", false, false),
    ("GetAccessorDeclaration2", "class C { public get m<T>(): U {} }", "class C {\n    get m() { }\n}", false, false),
    ("GetAccessorDeclaration3", "class C { public static get m<T>(): U {} }", "class C {\n    static get m() { }\n}", false, false),
    ("SetAccessorDeclaration1", "class C { set m(v); }", "class C {\n    set m(v) { }\n}", false, false),
    ("SetAccessorDeclaration2", "class C { public set m<T>(v): U {} }", "class C {\n    set m(v) { }\n}", false, false),
    ("SetAccessorDeclaration3", "class C { public static set m<T>(v): U {} }", "class C {\n    static set m(v) { }\n}", false, false),
    ("IndexSignature", "class C { [key: string]: number; }", "class C {\n}", false, false),
    ("VariableDeclaration1", "declare var a;", "", false, false),
    ("VariableDeclaration2", "var a: number", "var a;", false, false),
    ("HeritageClause", "class C implements I {}", "class C {\n}", false, false),
    ("ClassDeclaration1", "declare class C {}", "", false, false),
    ("ClassDeclaration2", "class C<T> {}", "class C {\n}", false, false),
    ("ClassExpression", "(class C<T> {})", "(class C {\n});", false, false),
    ("FunctionDeclaration1", "declare function f() {}", "", false, false),
    ("FunctionDeclaration2", "function f();", "", false, false),
    ("FunctionDeclaration3", "function f<T>(): U {}", "function f() { }", false, false),
    ("FunctionExpression", "(function f<T>(): U {})", "(function f() { });", false, false),
    ("ArrowFunction", "(<T>(): U => {})", "(() => { });", false, false),
    ("ParameterDeclaration", "function f(this: x, a: number, b?: boolean) {}", "function f(a, b) { }", false, false),
    ("CallExpression", "f<T>()", "f();", false, false),
    ("NewExpression1", "new f<T>()", "new f();", false, false),
    ("NewExpression2", "new f<T>", "new f;", false, false),
    ("TaggedTemplateExpression", "f<T>``", "f ``;", false, false),
    ("NonNullExpression", "x!", "x;", false, false),
    ("TypeAssertionExpression#1", "<T>x", "x;", false, false),
    ("TypeAssertionExpression#2", "(<T>x).c", "x.c;", false, false),
    ("AsExpression#1", "x as T", "x;", false, false),
    ("AsExpression#2", "(x as T).c", "x.c;", false, false),
    ("SatisfiesExpression#1", "x satisfies T", "x;", false, false),
    ("SatisfiesExpression#2", "(x satisfies T).c", "x.c;", false, false),
    ("JsxSelfClosingElement", "<x<T> />", "<x />;", true, false),
    ("JsxOpeningElement", "<x<T>></x>", "<x></x>;", true, false),
    ("ImportEqualsDeclaration#1", "import x = require(\"m\");", "import x = require(\"m\");", false, false),
    ("ImportEqualsDeclaration#2", "import type x = require(\"m\");", "", false, false),
    ("ImportEqualsDeclaration#3", "import x = y;", "import x = y;", false, false),
    ("ImportEqualsDeclaration#4", "import type x = y;", "", false, false),
    ("ImportDeclaration#1", "import \"m\";", "import \"m\";", false, false),
    ("ImportDeclaration#2", "import * as x from \"m\"; x;", "import * as x from \"m\";\nx;", false, false),
    ("ImportDeclaration#3", "import x from \"m\"; x;", "import x from \"m\";\nx;", false, false),
    ("ImportDeclaration#4", "import { x } from \"m\"; x;", "import { x } from \"m\";\nx;", false, false),
    ("ImportDeclaration#5", "import type * as x from \"m\";", "", false, false),
    ("ImportDeclaration#6", "import type x from \"m\";", "", false, false),
    ("ImportDeclaration#7", "import type { x } from \"m\";", "", false, false),
    ("ImportDeclaration#8", "import { type x } from \"m\";", "", false, false),
    ("ImportDeclaration#9", "import { type x } from \"m\";", "import {} from \"m\";", false, true),
    ("ExportDeclaration#1", "export * from \"m\";", "export * from \"m\";", false, false),
    ("ExportDeclaration#2", "export * as x from \"m\";", "export * as x from \"m\";", false, false),
    ("ExportDeclaration#3", "export { x } from \"m\";", "export { x } from \"m\";", false, false),
    ("ExportDeclaration#4", "export type * from \"m\";", "", false, false),
    ("ExportDeclaration#5", "export type * as x from \"m\";", "", false, false),
    ("ExportDeclaration#6", "export type { x } from \"m\";", "", false, false),
    ("ExportDeclaration#7", "export { type x } from \"m\";", "", false, false),
    ("ExportDeclaration#7", "export { type x } from \"m\";", "export {} from \"m\";", false, true),
];

// Go: transformers/tstransforms/typeeraser_test.go:14 TestTypeEraser
// PORT: runs in a child process, because the transform and the printer
// need the parsed file published (see `parse_type_script_published`).
#[test]
fn test_type_eraser() {
    in_child(module_path!(), "test_type_eraser", type_eraser);
}

fn type_eraser() {
    let mut t = Subtests::new("TestTypeEraser");
    for &(title, input, output, jsx, vms) in TYPE_ERASER {
        t.run(title, || {
            let file = parse_type_script_published(input, jsx);
            check_diagnostics(file)?;
            let mut compiler_options = CompilerOptions::default();
            if vms {
                compiler_options.verbatim_module_syntax = Tristate::True;
            }
            let compiler_options: &'static CompilerOptions = Box::leak(Box::new(compiler_options));
            let file = new_type_eraser_transformer(&transform_options(
                compiler_options,
                &new_emit_context(),
            ))
            .expect("type eraser")
            .transform_source_file(file);
            check_emit(None, file, output)
        });
    }
    t.finish();
}

/// Go TestImportElision rows: (title, input, output, other, jsx).
#[rustfmt::skip]
const IMPORT_ELISION: &[(&str, &str, &str, &str, bool)] = &[
    ("ImportEquals#1", "import x = require(\"other\"); x;", "import x = require(\"other\");\nx;", "", false),
    ("ImportEquals#2", "import x = require(\"other\");", "", "", false),
    ("ImportDeclaration#1", r#"import "m";"#, r#"import "m";"#, "", false),
    ("ImportDeclaration#2", "import * as x from \"other\"; x;", "import * as x from \"other\";\nx;", "", false),
    ("ImportDeclaration#3", "import x from \"other\"; x;", "import x from \"other\";\nx;", "", false),
    ("ImportDeclaration#4", "import { x } from \"other\"; x;", "import { x } from \"other\";\nx;", "", false),
    ("ImportDeclaration#5", "import * as x from \"other\";", "", "", false),
    ("ImportDeclaration#6", "import x from \"other\";", "", "", false),
    ("ImportDeclaration#7", "import { x } from \"other\";", "", "", false),
    ("ExportDeclaration#1", "export * from \"other\";", "export * from \"other\";", "export let x;", false),
    ("ExportDeclaration#2", "export * as x from \"other\";", "export * as x from \"other\";", "export let x;", false),
    ("ExportDeclaration#3", "export * from \"other\";", "export * from \"other\";", "export let x;", false),
    ("ExportDeclaration#4", "export * as x from \"other\";", "export * as x from \"other\";", "export let x;", false),
    ("ExportDeclaration#5", "export { x } from \"other\";", "export { x } from \"other\";", "export let x;", false),
    ("ExportDeclaration#6", "export { x } from \"other\";", "", "export type x = any;", false),
    ("ExportDeclaration#7", "export { x }; let x;", "export { x };\nlet x;", "", false),
    ("ExportDeclaration#8", "export { x }; type x = any;", "", "", false),
    ("ExportDeclaration#9", "import { x } from \"other\"; export { x };", "", "export type x = any;", false),
    ("ExportAssignment#1", "let x; export default x;", "let x;\nexport default x;", "", false),
    ("ExportAssignment#2", "type x = any; export default x;", "", "", false),
];

// Go: transformers/tstransforms/importelision_test.go:185 TestImportElision
// PORT: Go makes a checker for a fake `checker.Program` that holds the
// parsed files and resolves "other" to the other file. The port has no such
// seam, so each row is a real program on a map file system, in one child
// process (see `childprog`): the input is `/main.ts`, the other file is
// `/other.ts`, and `paths` maps "other" to it. `noLib` keeps the lib files
// out, as the fake program has none. The transforms get the empty Go
// compiler options, as in Go.
#[test]
fn test_import_elision() {
    in_child(module_path!(), "test_import_elision", || {
        let map_fs = MapFs::from_map([("/tsconfig.json", "{}")], true);
        install_map_fs(&map_fs, "/");
        let fs = map_fs.fs();
        let mut t = Subtests::new("TestImportElision");
        for &(title, input, output, other, jsx) in IMPORT_ELISION {
            t.run(title, || {
                let main = if jsx { "/main.tsx" } else { "/main.ts" };
                fs.write_file(main, input).expect("write main");
                let _ = fs.remove("/other.ts");
                if !other.is_empty() {
                    fs.write_file("/other.ts", other).expect("write other");
                }
                let mut paths = IndexMap::new();
                paths.insert("other".to_string(), Some(vec!["./other.ts".to_string()]));
                let p = new_program(
                    map_fs.fs(),
                    "/",
                    &[main],
                    CompilerOptions {
                        no_lib: Tristate::True,
                        paths: Some(paths),
                        paths_base_path: "/".to_string(),
                        ..Default::default()
                    },
                );
                let _current = ls_program::enter(&p);
                let file = source_file(&p, main).root;
                check_diagnostics(file)?;
                if !other.is_empty() {
                    check_diagnostics(source_file(&p, "/other.ts").root)?;
                }

                let compiler_options: &'static CompilerOptions = Box::leak(Box::default());

                let (c, release) = ls_program::get_type_checker(&p, &context::background());
                // ts#64649 (importelision_test.go:266, :267): the resolver of a new
                // emit context, and the transforms use that context.
                let emit_context = new_emit_context();
                let emit_resolver: Rc<dyn ts_goport::printer::EmitResolver> =
                    new_emit_resolver_of_shared_checker(&c, Rc::clone(&emit_context));

                let opts = TransformOptions {
                    context: emit_context,
                    compiler_options,
                    resolver: Rc::new(EmitResolverReferenceResolver(Rc::clone(&emit_resolver))),
                    emit_resolver,
                    get_emit_module_format_of_file: Rc::new(|_file: Node| -> ModuleKind {
                        panic!("invalid memory address or nil pointer dereference: GetEmitModuleFormatOfFile")
                    }),
                };
                let file = new_type_eraser_transformer(&opts)
                    .expect("type eraser")
                    .transform_source_file(file);
                let file = new_import_elision_transformer(&opts)
                    .expect("import elision")
                    .transform_source_file(file);
                let result = check_emit(None, file, output);
                release.call();
                result
            });
        }
        t.finish();
    });
}
