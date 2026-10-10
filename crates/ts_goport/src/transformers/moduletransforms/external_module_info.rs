//! Port of Go `transformers/moduletransforms/externalmoduleinfo.go`.

use super::utilities::is_effective_external_module_file;
use crate::prelude::*;
use crate::transformers::transformer::TransformReferenceResolver;
use crate::transformers::utilities::is_local_name;

/// Go `collections.MultiMap[K, V]`.
// PORT: Go never iterates these maps, so a hash map keeps the Go behavior.
#[derive(Debug)]
pub(crate) struct MultiMap<K, V> {
    m: FxHashMap<K, Vec<V>>,
}

impl<K, V> Default for MultiMap<K, V> {
    fn default() -> Self {
        Self {
            m: FxHashMap::default(),
        }
    }
}

impl<K: std::hash::Hash + Eq, V: Clone> MultiMap<K, V> {
    /// Go `MultiMap.Add`.
    pub(crate) fn add(&mut self, key: K, value: V) {
        self.m.entry(key).or_default().push(value);
    }

    /// Go `MultiMap.Get`. A missing key gives an empty (Go nil) slice.
    pub(crate) fn get<Q>(&self, key: &Q) -> Vec<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.m.get(key).cloned().unwrap_or_default()
    }

    /// Go `MultiMap.Has`.
    pub(crate) fn has<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.m.contains_key(key)
    }

    /// Go `MultiMap.Len`.
    pub(crate) fn len(&self) -> usize {
        self.m.len()
    }
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:15 externalModuleInfo
#[derive(Debug, Default)]
pub(crate) struct ExternalModuleInfo {
    /// ImportDeclaration | ImportEqualsDeclaration | ExportDeclaration. imports and reexports of other external modules
    pub external_imports: Vec<Node>,
    /// Maps local names to their associated export specifiers (excludes reexports)
    pub export_specifiers: MultiMap<String, Node>,
    /// Maps local declarations to their associated export aliases
    pub exported_bindings: MultiMap<Node, Node>,
    /// all exported names in the module, both local and re-exported, excluding the names of locally exported function declarations
    pub exported_names: Vec<Node>,
    /// all of the top-level exported function declarations
    pub exported_functions: IndexSet<Node>,
    /// an export=/module.exports= declaration if one was present
    pub export_equals: Node,
    /// whether this module contains export*
    pub has_export_stars_to_export_values: bool,
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:25 externalModuleInfoCollector
struct ExternalModuleInfoCollector<'a> {
    source_file: Node,
    #[allow(dead_code)]
    compiler_options: &'a CompilerOptions,
    emit_context: &'a EmitContext,
    resolver: &'a dyn TransformReferenceResolver,
    unique_exports: FxHashSet<String>,
    has_export_default: bool,
    output: ExternalModuleInfo,
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:35 collectExternalModuleInfo
pub(crate) fn collect_external_module_info(
    source_file: Node,
    compiler_options: &CompilerOptions,
    emit_context: &EmitContext,
    resolver: &dyn TransformReferenceResolver,
) -> ExternalModuleInfo {
    let c = ExternalModuleInfoCollector {
        source_file,
        compiler_options,
        emit_context,
        resolver,
        unique_exports: FxHashSet::default(),
        has_export_default: false,
        output: ExternalModuleInfo::default(),
    };
    c.collect()
}

impl ExternalModuleInfoCollector<'_> {
    // Go: transformers/moduletransforms/externalmoduleinfo.go:46 externalModuleInfoCollector.collect
    fn collect(mut self) -> ExternalModuleInfo {
        let mut has_import_star = false;
        let mut has_import_default = false;
        for node in self.source_file.statements().iter() {
            // Look through NotEmittedStatement to find elided export= declarations
            // (e.g., `declare export = x` is elided by the type eraser but must still be collected)
            if is_not_emitted_statement(node) {
                let original = self.emit_context.most_original(node);
                if original.is_some() && is_export_assignment(original) {
                    let n = original;
                    if n.is_export_equals() && self.output.export_equals.is_nil() {
                        self.output.export_equals = n;
                    }
                }
                continue;
            }
            match node.kind() {
                SyntaxKind::ImportDeclaration => {
                    // import "mod"
                    // import x from "mod"
                    // import * as x from "mod"
                    // import { x, y } from "mod"
                    let n = node;
                    self.add_external_import(node);
                    if !has_import_star && get_import_needs_import_star_helper(n) {
                        has_import_star = true;
                    }
                    if !has_import_default && get_import_needs_import_default_helper(n) {
                        has_import_default = true;
                    }
                }

                SyntaxKind::ImportEqualsDeclaration => {
                    let n = node;
                    if is_external_module_reference(n.module_reference()) {
                        // import x = require("mod")
                        self.add_external_import(node);
                    }
                }

                SyntaxKind::ExportDeclaration => {
                    let n = node;
                    if n.module_specifier().is_some() {
                        // export * from "mod"
                        // export * as ns from "mod"
                        // export { x, y } from "mod"
                        self.add_external_import(node);
                        if n.export_clause().is_nil() {
                            // export * from "mod"
                            self.output.has_export_stars_to_export_values = true;
                        } else if is_named_exports(n.export_clause()) {
                            // export { x, y } from "mod"
                            self.add_exported_names_for_export_declaration(n);
                            if !has_import_default {
                                has_import_default = contains_default_reference(n.export_clause());
                            }
                        } else {
                            // export * as ns from "mod"
                            let name = n.export_clause().name();
                            let name_text = name.text();
                            if self.add_unique_export(name_text) {
                                self.add_exported_binding(node, name);
                                self.add_exported_name(name);
                            }
                            // we use the same helpers for `export * as ns` as we do for `import * as ns`
                            has_import_star = true;
                        }
                    } else {
                        // export { x, y }
                        self.add_exported_names_for_export_declaration(node);
                    }
                }

                SyntaxKind::ExportAssignment => {
                    let n = node;
                    if n.is_export_equals() && self.output.export_equals.is_nil() {
                        // export = x
                        self.output.export_equals = n;
                    }
                }

                SyntaxKind::VariableStatement => {
                    let n = node;
                    if has_syntactic_modifier(node, ModifierFlags::EXPORT) {
                        for decl in n.declaration_list().declarations().nodes().iter() {
                            self.collect_exported_variable_info(decl);
                        }
                    }
                }

                SyntaxKind::FunctionDeclaration => {
                    let n = node;
                    if has_syntactic_modifier(node, ModifierFlags::EXPORT) {
                        self.add_exported_function_declaration(
                            n,
                            Node::NIL, /*name*/
                            has_syntactic_modifier(node, ModifierFlags::DEFAULT),
                        );
                    }
                }

                SyntaxKind::ClassDeclaration => {
                    let n = node;
                    if has_syntactic_modifier(node, ModifierFlags::EXPORT) {
                        if has_syntactic_modifier(node, ModifierFlags::DEFAULT) {
                            // export default class { }
                            if !self.has_export_default {
                                let mut name = n.name();
                                if name.is_nil() {
                                    name = self
                                        .emit_context
                                        .factory()
                                        .new_generated_name_for_node(node);
                                }
                                self.add_exported_binding(node, name);
                                self.has_export_default = true;
                            }
                        } else {
                            // export class x { }
                            let name = n.name();
                            if name.is_some() && self.add_unique_export(name.text()) {
                                self.add_exported_binding(node, name);
                                self.add_exported_name(name);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // PORT: Go keeps the two flags only as locals; they do not reach the output.
        let _ = (has_import_star, has_import_default);
        self.output
    }

    // Go: transformers/moduletransforms/externalmoduleinfo.go:167 externalModuleInfoCollector.addUniqueExport
    fn add_unique_export(&mut self, name: &str) -> bool {
        if !self.unique_exports.contains(name) {
            self.unique_exports.insert(name.to_string());
            return true;
        }
        false
    }

    // Go: transformers/moduletransforms/externalmoduleinfo.go:175 externalModuleInfoCollector.addExportedBinding
    fn add_exported_binding(&mut self, decl: Node, name: Node) {
        self.output
            .exported_bindings
            .add(self.emit_context.most_original(decl), name);
    }

    // Go: transformers/moduletransforms/externalmoduleinfo.go:179 externalModuleInfoCollector.addExternalImport
    fn add_external_import(
        &mut self,
        node: Node, /*ImportDeclaration | ImportEqualsDeclaration | ExportDeclaration*/
    ) {
        self.output.external_imports.push(node);
    }

    // Go: transformers/moduletransforms/externalmoduleinfo.go:183 externalModuleInfoCollector.addExportedName
    fn add_exported_name(&mut self, name: Node) {
        self.output.exported_names.push(name);
    }

    // Go: transformers/moduletransforms/externalmoduleinfo.go:187 externalModuleInfoCollector.addExportedNamesForExportDeclaration
    fn add_exported_names_for_export_declaration(&mut self, node: Node) {
        for specifier in node.export_clause().elements().iter() {
            let specifier_name_text = specifier.name().text();
            if self.add_unique_export(specifier_name_text) {
                let name = specifier.property_name_or_name();
                if name.kind() != SyntaxKind::StringLiteral {
                    if node.module_specifier().is_nil() {
                        self.output
                            .export_specifiers
                            .add(name.text().to_string(), specifier);
                    }

                    let mut decl = self
                        .resolver
                        .get_referenced_import_declaration(self.emit_context.most_original(name));
                    if decl.is_nil() {
                        decl = self.resolver.get_referenced_value_declaration(
                            self.emit_context.most_original(name),
                        );
                    }
                    if decl.is_some() {
                        if decl.kind() == SyntaxKind::FunctionDeclaration {
                            self.unique_exports.remove(specifier_name_text);
                            self.add_exported_function_declaration(
                                decl,
                                specifier.name(),
                                module_export_name_is_default(specifier.name()),
                            );
                            continue;
                        }
                        self.add_exported_binding(decl, specifier.name());
                    }
                }

                self.add_exported_name(specifier.name());
            }
        }
    }

    // Go: transformers/moduletransforms/externalmoduleinfo.go:216 externalModuleInfoCollector.addExportedFunctionDeclaration
    fn add_exported_function_declaration(&mut self, node: Node, mut name: Node, is_default: bool) {
        self.output
            .exported_functions
            .insert(self.emit_context.most_original(node));
        if is_default {
            // export default function() { }
            // function x() { } + export { x as default };
            if !self.has_export_default {
                if name.is_nil() {
                    name = self
                        .emit_context
                        .factory()
                        .new_generated_name_for_node(node);
                }
                self.add_exported_binding(node, name);
                self.has_export_default = true;
            }
        } else {
            // export function x() { }
            // function x() { } + export { x }
            if name.is_nil() {
                name = node.name();
            }
            let name_text = name.text();
            if self.add_unique_export(name_text) {
                self.add_exported_binding(node, name);
            }
        }
    }

    // Go: transformers/moduletransforms/externalmoduleinfo.go:241 externalModuleInfoCollector.collectExportedVariableInfo
    fn collect_exported_variable_info(
        &mut self,
        decl: Node, /*VariableDeclaration | BindingElement*/
    ) {
        if is_binding_pattern(decl.name()) {
            for element in decl.name().elements().iter() {
                let e = element;
                if e.name().is_some() {
                    self.collect_exported_variable_info(element);
                }
            }
        } else if !self.emit_context.has_auto_generate_info(decl.name()) {
            let text = decl.name().text();
            if self.add_unique_export(text) {
                self.add_exported_name(decl.name());
                if is_local_name(self.emit_context, decl.name()) {
                    self.add_exported_binding(decl, decl.name());
                }
            }
        }
    }
}

const EXTERNAL_HELPERS_MODULE_NAME_TEXT: &str = "tslib";

// Go: transformers/moduletransforms/externalmoduleinfo.go:262 createExternalHelpersImportDeclarationIfNeeded
pub(crate) fn create_external_helpers_import_declaration_if_needed(
    emit_context: &EmitContext,
    source_file: Node,
    compiler_options: &CompilerOptions,
    file_module_kind: ModuleKind,
    has_export_stars_to_export_values: bool,
    has_import_star: bool,
    has_import_default: bool,
) -> Node /*ImportDeclaration | ImportEqualsDeclaration*/ {
    if compiler_options.import_helpers.is_true()
        && is_effective_external_module_file(source_file, compiler_options)
    {
        let f = emit_context.factory();
        let module_kind = compiler_options.get_emit_module_kind();
        let helpers = get_imported_helpers(emit_context, source_file);
        if file_module_kind == ModuleKind::COMMON_JS
            || (file_module_kind == ModuleKind::NONE && module_kind == ModuleKind::COMMON_JS)
        {
            // When we emit to a non-ES module, generate a synthetic `import tslib = require("tslib")` to be further transformed.
            let external_helpers_module_name = get_or_create_external_helpers_module_name_if_needed(
                emit_context,
                source_file,
                compiler_options,
                &helpers,
                has_export_stars_to_export_values,
                has_import_star || has_import_default,
                file_module_kind,
            );
            if external_helpers_module_name.is_some() {
                let external_helpers_import_declaration = f.new_import_equals_declaration(
                    ModifierList::NIL, /*modifiers*/
                    false,             /*isTypeOnly*/
                    external_helpers_module_name,
                    f.new_external_module_reference(
                        f.new_string_literal(EXTERNAL_HELPERS_MODULE_NAME_TEXT, TokenFlags::NONE),
                    ),
                );
                emit_context.add_emit_flags(
                    external_helpers_import_declaration,
                    EmitFlags::CUSTOM_PROLOGUE,
                );
                return external_helpers_import_declaration;
            }
        } else {
            // When we emit as an ES module, generate an `import` declaration that uses named imports for helpers.
            // If we cannot determine the implied module kind under `module: preserve` we assume ESM.
            let mut helper_names: Vec<&'static str> = Vec::new();
            for helper in &helpers {
                let import_name = helper.import_name;
                if !import_name.is_empty() && !helper_names.contains(&import_name) {
                    helper_names.push(import_name);
                }
            }
            if !helper_names.is_empty() {
                // Go: transformers/moduletransforms/externalmoduleinfo.go:290
                // slices.SortFunc(helperNames, stringutil.CompareStringsCaseSensitive)
                // PORT: Go `stringutil.CompareStringsCaseSensitive` is an ordinal compare.
                crate::gostd::slices::sort_func(&mut helper_names, |a, b| a.cmp(b) as i32);
                // Alias the imports if the names are used somewhere in the file.
                // NOTE: We don't need to care about global import collisions as this is a module.

                let import_specifiers: Vec<Node> = helper_names
                    .iter()
                    .map(|name| {
                        if emit_context.is_file_level_unique_name(
                            source_file,
                            name,
                            None, /*hasGlobalName*/
                        ) {
                            f.new_import_specifier(
                                false,     /*isTypeOnly*/
                                Node::NIL, /*propertyName*/
                                f.new_identifier(*name),
                            )
                        } else {
                            f.new_import_specifier(
                                false, /*isTypeOnly*/
                                f.new_identifier(*name),
                                f.new_unscoped_helper_name(name),
                            )
                        }
                    })
                    .collect();
                let named_bindings = f.new_named_imports(f.new_node_list(&import_specifiers));
                let parse_node = emit_context.most_original(source_file);
                emit_context.add_emit_flags(parse_node, EmitFlags::EXTERNAL_HELPERS);

                let external_helpers_import_declaration = f.new_import_declaration(
                    ModifierList::NIL, /*modifiers*/
                    f.new_import_clause(
                        SyntaxKind::Unknown, /*phaseModifier*/
                        Node::NIL,           /*name*/
                        named_bindings,
                    ),
                    f.new_string_literal(EXTERNAL_HELPERS_MODULE_NAME_TEXT, TokenFlags::NONE),
                    Node::NIL, /*attributes*/
                );

                emit_context.add_emit_flags(
                    external_helpers_import_declaration,
                    EmitFlags::CUSTOM_PROLOGUE,
                );
                return external_helpers_import_declaration;
            }
        }
    }
    Node::NIL
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:320 getImportedHelpers
pub(crate) fn get_imported_helpers(
    emit_context: &EmitContext,
    source_file: Node,
) -> Vec<EmitHelperRef> {
    let mut helpers = Vec::new();
    for helper in emit_context.get_emit_helpers(source_file) {
        if !helper.scoped {
            helpers.push(helper);
        }
    }
    helpers
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:330 getOrCreateExternalHelpersModuleNameIfNeeded
pub(crate) fn get_or_create_external_helpers_module_name_if_needed(
    emit_context: &EmitContext,
    node: Node,
    _compiler_options: &CompilerOptions,
    helpers: &[EmitHelperRef],
    has_export_stars_to_export_values: bool,
    has_import_star_or_import_default: bool,
    file_module_kind: ModuleKind,
) -> Node {
    let mut external_helpers_module_name = emit_context.get_external_helpers_module_name(node);
    if external_helpers_module_name.is_some() {
        return external_helpers_module_name;
    }

    let create = !helpers.is_empty()
        || ((has_export_stars_to_export_values || has_import_star_or_import_default)
            && file_module_kind < ModuleKind::SYSTEM);

    if create {
        external_helpers_module_name = emit_context
            .factory()
            .new_unique_name(EXTERNAL_HELPERS_MODULE_NAME_TEXT);
        emit_context.set_external_helpers_module_name(node, external_helpers_module_name);
    }

    external_helpers_module_name
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:348 isNamedDefaultReference
pub(crate) fn is_named_default_reference(e: Node, /*ImportSpecifier | ExportSpecifier*/) -> bool {
    module_export_name_is_default(e.property_name_or_name())
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:352 containsDefaultReference
pub(crate) fn contains_default_reference(
    node: Node, /*NamedImportBindings | NamedExportBindings*/
) -> bool {
    node.is_some()
        && (is_named_imports(node) || is_named_exports(node))
        && node.elements().iter().any(is_named_default_reference)
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:356 getExportNeedsImportStarHelper
pub(crate) fn get_export_needs_import_star_helper(node: Node) -> bool {
    get_namespace_declaration_node(node).is_some()
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:360 getImportNeedsImportStarHelper
pub(crate) fn get_import_needs_import_star_helper(node: Node) -> bool {
    if get_namespace_declaration_node(node).is_some() {
        return true;
    }
    if node.import_clause().is_nil() {
        return false;
    }
    let bindings = node.import_clause().named_bindings();
    if bindings.is_nil() {
        return false;
    }
    if !is_named_imports(bindings) {
        return false;
    }
    let named_imports = bindings;
    let elements = named_imports.elements();
    let mut default_ref_count = 0;
    for binding in elements.iter() {
        if is_named_default_reference(binding) {
            default_ref_count += 1;
        }
    }
    // Import star is required if there's default named refs mixed with non-default refs, or if theres non-default refs and it has a default import
    (default_ref_count > 0 && default_ref_count != elements.len())
        || ((elements.len() - default_ref_count) != 0 && is_default_import(node))
}

// Go: transformers/moduletransforms/externalmoduleinfo.go:385 getImportNeedsImportDefaultHelper
pub(crate) fn get_import_needs_import_default_helper(node: Node) -> bool {
    // Import default is needed if there's a default import or a default ref and no other refs (meaning an import star helper wasn't requested)
    !get_import_needs_import_star_helper(node)
        && (is_default_import(node)
            || (node.import_clause().is_some()
                && contains_default_reference(node.import_clause().named_bindings())))
}
