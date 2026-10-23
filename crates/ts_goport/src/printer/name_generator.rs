//! Port of Go `printer/namegenerator.go`.

use crate::prelude::*;

use super::emit_context::{AutoGenerateId, EmitContext};
use super::types::GeneratedIdentifierFlags;
use super::utilities::{
    ensure_leading_hash, format_generated_name, make_identifier_from_module_name,
    remove_leading_hash,
};

/// Go `tempFlags`. Flags enum to track count of temp variables and a few dedicated names
pub(crate) type TempFlags = i32;

pub(crate) const TEMP_FLAGS_AUTO: TempFlags = 0x0000_0000; // No preferred name
pub(crate) const TEMP_FLAGS_COUNT_MASK: TempFlags = 0x0FFF_FFFF; // Temp variable counter
pub(crate) const TEMP_FLAGS_I: TempFlags = 0x1000_0000; // Use/preference flag for '_i'

/// Go `func(string, bool) bool` (Printer.isFileLevelUniqueNameInCurrentFile).
pub type IsFileLevelUniqueNameFn = Rc<dyn Fn(&str, bool) -> bool>;
/// Go `func(*ast.Node) string` (Printer.getTextOfNode).
// PORT: the Go closure captures the Printer, and Printer.getTextOfNode can
// call back into this same NameGenerator (GenerateName). Rust cannot give the
// closure a second mutable path to the generator, so the generator passes
// itself to the callback.
pub type GetTextOfNodeFn = Rc<dyn Fn(&mut NameGenerator, Node) -> String>;

/// Go `NameGenerator`.
// PORT: Go func fields are `Option<Rc<dyn Fn>>`; `None` is Go `nil`. Nil
// maps are empty maps (Go reads of a nil map see no entries).
#[derive(Default)]
pub struct NameGenerator {
    pub context: Option<Rc<EmitContext>>,
    pub is_file_level_unique_name_in_current_file: Option<IsFileLevelUniqueNameFn>, // callback for Printer.isFileLevelUniqueNameInCurrentFile
    pub get_text_of_node: Option<GetTextOfNodeFn>, // callback for Printer.getTextOfNode
    node_id_to_generated_name: FxHashMap<u64, String>, // Map of generated names for specific nodes
    node_id_to_generated_private_name: FxHashMap<u64, String>, // Map of generated private names for specific nodes
    auto_generated_id_to_generated_name: FxHashMap<AutoGenerateId, String>, // Map of generated names for temp and loop variables
    name_generation_scope: Option<Box<NameGenerationScope>>,
    private_name_generation_scope: Option<Box<NameGenerationScope>>,
    generated_names: FxHashSet<String>, // NOTE: Used to match Strada, but should be moved to nameGenerationScope after port is complete.
}

/// Go `nameGenerationScope`.
#[derive(Default)]
struct NameGenerationScope {
    next: Option<Box<NameGenerationScope>>, // The next nameGenerationScope in the stack
    temp_flags: TempFlags,                  // TempFlags for the current name generation scope.
    formatted_name_temp_flags: FxHashMap<String, TempFlags>, // TempFlags for the current name generation scope.
    reserved_names: FxHashSet<String>, // Names reserved in nested name generation scopes.
                                       // generatedNames         collections.Set[string] // NOTE: generated names should be scoped after Strada port is complete.
}

impl NameGenerator {
    // Go: printer/namegenerator.go:41 PushScope
    pub fn push_scope(&mut self, reuse_temp_variable_scope: bool) {
        self.private_name_generation_scope = Some(Box::new(NameGenerationScope {
            next: self.private_name_generation_scope.take(),
            ..Default::default()
        }));
        if !reuse_temp_variable_scope {
            self.name_generation_scope = Some(Box::new(NameGenerationScope {
                next: self.name_generation_scope.take(),
                ..Default::default()
            }));
        }
    }

    // Go: printer/namegenerator.go:48 PopScope
    pub fn pop_scope(&mut self, reuse_temp_variable_scope: bool) {
        if let Some(scope) = self.private_name_generation_scope.take() {
            self.private_name_generation_scope = scope.next;
        }
        if !reuse_temp_variable_scope {
            if let Some(scope) = self.name_generation_scope.take() {
                self.name_generation_scope = scope.next;
            }
        }
    }

    // Go: printer/namegenerator.go:57 getScope
    fn get_scope(&mut self, private_name: bool) -> &mut Option<Box<NameGenerationScope>> {
        if private_name {
            &mut self.private_name_generation_scope
        } else {
            &mut self.name_generation_scope
        }
    }

    /// Read-only `getScope`.
    fn get_scope_ref(&self, private_name: bool) -> &Option<Box<NameGenerationScope>> {
        if private_name {
            &self.private_name_generation_scope
        } else {
            &self.name_generation_scope
        }
    }

    // Go: printer/namegenerator.go:61 getTempFlags
    fn get_temp_flags(&self, private_name: bool) -> TempFlags {
        if let Some(scope) = self.get_scope_ref(private_name) {
            return scope.temp_flags;
        }
        TEMP_FLAGS_AUTO
    }

    // Go: printer/namegenerator.go:69 setTempFlags
    fn set_temp_flags(&mut self, private_name: bool, flags: TempFlags) {
        let scope = self
            .get_scope(private_name)
            .get_or_insert_with(Box::default);
        scope.temp_flags = flags;
    }

    // Go: printer/namegenerator.go:78 getTempFlagsForFormattedName
    // Gets the TempFlags to use in the current nameGenerationScope for the given key
    fn get_temp_flags_for_formatted_name(
        &self,
        private_name: bool,
        formatted_name_key: &str,
    ) -> TempFlags {
        if let Some(scope) = self.get_scope_ref(private_name) {
            if let Some(flags) = scope.formatted_name_temp_flags.get(formatted_name_key) {
                return *flags;
            }
        }
        TEMP_FLAGS_AUTO
    }

    // Go: printer/namegenerator.go:89 setTempFlagsForFormattedName
    // Sets the TempFlags to use in the current nameGenerationScope for the given key
    fn set_temp_flags_for_formatted_name(
        &mut self,
        private_name: bool,
        formatted_name_key: &str,
        flags: TempFlags,
    ) {
        let scope = self
            .get_scope(private_name)
            .get_or_insert_with(Box::default);
        scope
            .formatted_name_temp_flags
            .insert(formatted_name_key.to_string(), flags);
    }

    // Go: printer/namegenerator.go:100 reserveName
    fn reserve_name(&mut self, name: &str, private_name: bool, scoped: bool, temp: bool) {
        let scope = self
            .get_scope(private_name)
            .get_or_insert_with(Box::default);
        if private_name || scoped {
            scope.reserved_names.insert(name.to_string());
        } else if !temp {
            self.generated_names.insert(name.to_string()); // NOTE: Matches Strada, but is incorrect.
            // (*scope).generatedNames.Add(name) // TODO: generated names should be scoped after Strada port is complete.
        }
    }

    /// Go `g.GetTextOfNode(node)`. Panics when the callback is nil, as Go does.
    fn text_of_node(&mut self, node: Node) -> String {
        let get_text_of_node = self
            .get_text_of_node
            .clone()
            .expect("nil GetTextOfNode callback");
        get_text_of_node(self, node)
    }

    // Go: printer/namegenerator.go:114 GenerateName
    // Generate the text for a generated identifier or private identifier
    pub fn generate_name(&mut self, name: Node) -> String {
        if let Some(context) = self.context.clone() {
            let auto_generate = context.auto_generate.borrow().get(&name).cloned();
            if let Some(auto_generate) = auto_generate {
                if auto_generate.flags.is_node() {
                    // Node names generate unique names based on their original node
                    // and are cached based on that node's id.
                    return self.generate_name_for_node_cached(
                        context.get_node_for_generated_name(name),
                        is_private_identifier(name),
                        auto_generate.flags,
                        &auto_generate.prefix,
                        &auto_generate.suffix,
                    );
                } else {
                    // Auto, Loop, and Unique names are cached based on their unique autoGenerateId.
                    if let Some(auto_generated_name) = self
                        .auto_generated_id_to_generated_name
                        .get(&auto_generate.id)
                    {
                        return auto_generated_name.clone();
                    }
                    let auto_generated_name = self.make_name(name);
                    self.auto_generated_id_to_generated_name
                        .insert(auto_generate.id, auto_generated_name.clone());
                    return auto_generated_name;
                }
            }
        }
        self.text_of_node(name)
    }

    // Go: printer/namegenerator.go:138 generateNameForNodeCached
    fn generate_name_for_node_cached(
        &mut self,
        node: Node,
        private_name: bool,
        flags: GeneratedIdentifierFlags,
        prefix: &str,
        suffix: &str,
    ) -> String {
        let node_id = get_node_id(node);
        let cache = if private_name {
            &self.node_id_to_generated_private_name
        } else {
            &self.node_id_to_generated_name
        };

        if let Some(name) = cache.get(&node_id) {
            return name.clone();
        }

        let name = self.generate_name_for_node(node, private_name, flags, prefix, suffix);
        let cache = if private_name {
            &mut self.node_id_to_generated_private_name
        } else {
            &mut self.node_id_to_generated_name
        };
        cache.insert(node_id, name.clone());
        name
    }

    // Go: printer/namegenerator.go:154 generateNameForNode
    fn generate_name_for_node(
        &mut self,
        node: Node,
        private_name: bool,
        flags: GeneratedIdentifierFlags,
        prefix: &str,
        suffix: &str,
    ) -> String {
        match node.kind() {
            SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier => {
                let text = self.text_of_node(node);
                self.make_unique_name(
                    &text,
                    None, /*checkFn*/
                    flags.is_optimistic(),
                    flags.is_reserved_in_nested_scopes(),
                    private_name,
                    prefix,
                    suffix,
                )
            }
            SyntaxKind::ModuleDeclaration | SyntaxKind::EnumDeclaration => {
                if private_name || !prefix.is_empty() || !suffix.is_empty() {
                    panic!(
                        "Generated name for a module or enum cannot be private and may have neither a prefix nor suffix"
                    );
                }
                self.generate_name_for_module_or_enum(node)
            }
            SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::ExportDeclaration => {
                if private_name || !prefix.is_empty() || !suffix.is_empty() {
                    panic!(
                        "Generated name for an import or export cannot be private and may have neither a prefix nor suffix"
                    );
                }
                self.generate_name_for_import_or_export_declaration(node)
            }
            SyntaxKind::FunctionDeclaration | SyntaxKind::ClassDeclaration => {
                if private_name || !prefix.is_empty() || !suffix.is_empty() {
                    panic!(
                        "Generated name for a class or function declaration cannot be private and may have neither a prefix nor suffix"
                    );
                }
                let name = node.name();
                // PORT: Go reads `g.Context == nil && g.Context.HasAutoGenerateInfo(name)`.
                // With a nil context that dereferences nil and panics; with a
                // context the condition is false.
                let context_nil_and_has_info = match &self.context {
                    Some(_) => false,
                    None => panic!("nil EmitContext dereference in HasAutoGenerateInfo"),
                };
                if name.is_some() && !context_nil_and_has_info {
                    return self.generate_name_for_node(
                        name, false, /*privateName*/
                        flags, "", /*prefix*/
                        "", /*suffix*/
                    );
                }
                self.generate_name_for_export_default()
            }
            SyntaxKind::ExportAssignment => {
                if private_name || !prefix.is_empty() || !suffix.is_empty() {
                    panic!(
                        "Generated name for an export assignment cannot be private and may have neither a prefix nor suffix"
                    );
                }
                self.generate_name_for_export_default()
            }
            SyntaxKind::ClassExpression => {
                if private_name || !prefix.is_empty() || !suffix.is_empty() {
                    panic!(
                        "Generated name for a class expression cannot be private and may have neither a prefix nor suffix"
                    );
                }
                self.generate_name_for_class_expression()
            }
            SyntaxKind::MethodDeclaration | SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => {
                self.generate_name_for_method_or_accessor(node, private_name, prefix, suffix)
            }
            SyntaxKind::ComputedPropertyName => {
                self.make_temp_variable_name(
                    TEMP_FLAGS_AUTO,
                    true, /*reservedInNestedScopes*/
                    private_name,
                    prefix,
                    suffix,
                )
            }
            _ => self.make_temp_variable_name(
                TEMP_FLAGS_AUTO,
                false, /*reservedInNestedScopes*/
                private_name,
                prefix,
                suffix,
            ),
        }
    }

    // Go: printer/namegenerator.go:196 generateNameForModuleOrEnum
    fn generate_name_for_module_or_enum(
        &mut self,
        node: Node, /* ModuleDeclaration | EnumDeclaration */
    ) -> String {
        let name = self.text_of_node(node.name());
        // Use module/enum name itself if it is unique, otherwise make a unique variation
        if is_unique_local_name(&name, node) {
            name
        } else {
            self.make_unique_name(
                &name, None,  /*checkFn*/
                false, /*optimistic*/
                false, /*scoped*/
                false, /*privateName*/
                "",    /*prefix*/
                "",    /*suffix*/
            )
        }
    }

    // Go: printer/namegenerator.go:206 generateNameForImportOrExportDeclaration
    fn generate_name_for_import_or_export_declaration(
        &mut self,
        node: Node, /* ImportDeclaration | ExportDeclaration */
    ) -> String {
        let expr = get_external_module_name(node);
        let mut base_name = "module".to_string();
        if is_string_literal(expr) {
            base_name = make_identifier_from_module_name(expr.text());
        }
        self.make_unique_name(
            &base_name, None,  /*checkFn*/
            false, /*optimistic*/
            false, /*scoped*/
            false, /*privateName*/
            "",    /*prefix*/
            "",    /*suffix*/
        )
    }

    // Go: printer/namegenerator.go:215 generateNameForExportDefault
    fn generate_name_for_export_default(&mut self) -> String {
        self.make_unique_name(
            "default", None,  /*checkFn*/
            false, /*optimistic*/
            false, /*scoped*/
            false, /*privateName*/
            "",    /*prefix*/
            "",    /*suffix*/
        )
    }

    // Go: printer/namegenerator.go:219 generateNameForClassExpression
    fn generate_name_for_class_expression(&mut self) -> String {
        self.make_unique_name(
            "class", None,  /*checkFn*/
            false, /*optimistic*/
            false, /*scoped*/
            false, /*privateName*/
            "",    /*prefix*/
            "",    /*suffix*/
        )
    }

    // Go: printer/namegenerator.go:223 generateNameForMethodOrAccessor
    fn generate_name_for_method_or_accessor(
        &mut self,
        node: Node, /* MethodDeclaration | AccessorDeclaration */
        private_name: bool,
        prefix: &str,
        suffix: &str,
    ) -> String {
        if is_identifier(node.name()) {
            return self.generate_name_for_node_cached(
                node.name(),
                private_name,
                GeneratedIdentifierFlags::NONE,
                prefix,
                suffix,
            );
        }
        self.make_temp_variable_name(
            TEMP_FLAGS_AUTO,
            false, /*reservedInNestedScopes*/
            private_name,
            prefix,
            suffix,
        )
    }

    // Go: printer/namegenerator.go:230 makeName
    fn make_name(&mut self, name: Node) -> String {
        if let Some(context) = self.context.clone() {
            let auto_generate = context.auto_generate.borrow().get(&name).cloned();
            if let Some(auto_generate) = auto_generate {
                let kind = auto_generate.flags.kind();
                if kind == GeneratedIdentifierFlags::AUTO {
                    return self.make_temp_variable_name(
                        TEMP_FLAGS_AUTO,
                        auto_generate.flags.is_reserved_in_nested_scopes(),
                        is_private_identifier(name),
                        &auto_generate.prefix,
                        &auto_generate.suffix,
                    );
                } else if kind == GeneratedIdentifierFlags::LOOP {
                    go_assert!(is_identifier(name));
                    return self.make_temp_variable_name(
                        TEMP_FLAGS_I,
                        auto_generate.flags.is_reserved_in_nested_scopes(),
                        false, /*privateName*/
                        &auto_generate.prefix,
                        &auto_generate.suffix,
                    );
                } else if kind == GeneratedIdentifierFlags::UNIQUE {
                    let check_fn = if auto_generate.flags.is_file_level() {
                        self.is_file_level_unique_name_in_current_file.clone()
                    } else {
                        None
                    };
                    return self.make_unique_name(
                        name.text(),
                        check_fn.as_deref(),
                        auto_generate.flags.is_optimistic(),
                        auto_generate.flags.is_reserved_in_nested_scopes(),
                        is_private_identifier(name),
                        &auto_generate.prefix,
                        &auto_generate.suffix,
                    );
                }
            }
        }
        self.text_of_node(name)
    }

    // Go: printer/namegenerator.go:258 makeTempVariableName
    // Return the next available name in the pattern _a ... _z, _0, _1, ...
    // TempFlags._i may be used to express a preference for that dedicated name.
    // Note that names generated by makeTempVariableName and makeUniqueName will never conflict.
    fn make_temp_variable_name(
        &mut self,
        flags: TempFlags,
        reserved_in_nested_scopes: bool,
        private_name: bool,
        prefix: &str,
        suffix: &str,
    ) -> String {
        let mut temp_flags: TempFlags;
        let mut key = String::new();
        let simple = prefix.is_empty() && suffix.is_empty();
        if simple {
            temp_flags = self.get_temp_flags(private_name);
        } else {
            // Generate a key to use to acquire a TempFlags counter based on the fixed portions of the generated name.
            key = format_generated_name(private_name, prefix, "" /*base*/, suffix);
            if private_name {
                key = ensure_leading_hash(&key);
            }
            temp_flags = self.get_temp_flags_for_formatted_name(private_name, &key);
        }

        if flags != 0 && temp_flags & flags == 0 {
            let full_name = format_generated_name(private_name, prefix, "_i", suffix);
            if self.is_unique_name(&full_name, private_name) {
                temp_flags |= flags;
                self.reserve_name(
                    &full_name,
                    private_name,
                    reserved_in_nested_scopes,
                    true, /*temp*/
                );
                if simple {
                    self.set_temp_flags(private_name, temp_flags);
                } else {
                    self.set_temp_flags_for_formatted_name(private_name, &key, temp_flags);
                }
                return full_name;
            }
        }

        loop {
            let count = temp_flags & TEMP_FLAGS_COUNT_MASK;
            temp_flags += 1;
            // Skip over 'i' and 'n'
            if count != 8 && count != 13 {
                let name = if count < 26 {
                    format!("_{}", char::from(b'a' + count as u8))
                } else {
                    format!("_{}", count - 26)
                };
                let full_name = format_generated_name(private_name, prefix, &name, suffix);
                if self.is_unique_name(&full_name, private_name) {
                    self.reserve_name(
                        &full_name,
                        private_name,
                        reserved_in_nested_scopes,
                        true, /*temp*/
                    );
                    if simple {
                        self.set_temp_flags(private_name, temp_flags);
                    } else {
                        self.set_temp_flags_for_formatted_name(private_name, &key, temp_flags);
                    }
                    return full_name;
                }
            }
        }
    }

    // Go: printer/namegenerator.go:317 makeUniqueName
    // Generate a name that is unique within the current file and doesn't conflict with any names
    // in global scope. The name is formed by adding an '_n' suffix to the specified base name,
    // where n is a positive integer. Note that names generated by makeTempVariableName and
    // makeUniqueName are guaranteed to never conflict.
    // If `optimistic` is set, the first instance will use 'baseName' verbatim instead of 'baseName_1'
    #[allow(clippy::too_many_arguments)]
    fn make_unique_name(
        &mut self,
        base_name: &str,
        check_fn: Option<&dyn Fn(&str, bool) -> bool>,
        optimistic: bool,
        scoped: bool,
        private_name: bool,
        prefix: &str,
        suffix: &str,
    ) -> String {
        let mut base_name = remove_leading_hash(base_name).to_string();
        if optimistic {
            let full_name = format_generated_name(private_name, prefix, &base_name, suffix);
            if self.check_unique_name(&full_name, private_name, check_fn) {
                self.reserve_name(&full_name, private_name, scoped, false /*temp*/);
                return full_name;
            }
        }

        // Find the first unique 'name_n', where n is a positive integer
        if !base_name.is_empty() && base_name.as_bytes()[base_name.len() - 1] != b'_' {
            base_name.push('_');
        }

        let mut i = 1;
        loop {
            let full_name =
                format_generated_name(private_name, prefix, &format!("{base_name}{i}"), suffix);
            if self.check_unique_name(&full_name, private_name, check_fn) {
                self.reserve_name(&full_name, private_name, scoped, false /*temp*/);
                return full_name;
            }
            i += 1;
        }
    }

    // Go: printer/namegenerator.go:343 MakeFileLevelOptimisticUniqueName
    pub fn make_file_level_optimistic_unique_name(&mut self, name: &str) -> String {
        let check_fn = self.is_file_level_unique_name_in_current_file.clone();
        self.make_unique_name(
            name,
            check_fn.as_deref(),
            true,  /*optimistic*/
            false, /*scoped*/
            false, /*privateName*/
            "",    /*prefix*/
            "",    /*suffix*/
        )
    }

    // Go: printer/namegenerator.go:347 checkUniqueName
    fn check_unique_name(
        &self,
        name: &str,
        private_name: bool,
        check_fn: Option<&dyn Fn(&str, bool) -> bool>,
    ) -> bool {
        if let Some(check_fn) = check_fn {
            check_fn(name, private_name)
        } else {
            self.is_unique_name(name, private_name)
        }
    }

    // Go: printer/namegenerator.go:378 isUniqueName
    fn is_unique_name(&self, name: &str, private_name: bool) -> bool {
        (self
            .is_file_level_unique_name_in_current_file
            .as_ref()
            .is_none_or(|f| f(name, private_name)))
            && !self.is_reserved_name(name, private_name)
    }

    // Go: printer/namegenerator.go:383 isReservedName
    fn is_reserved_name(&self, name: &str, private_name: bool) -> bool {
        let mut scope = self.get_scope_ref(private_name);

        // NOTE: The following matches Strada, but is incorrect.
        if self.generated_names.contains(name) {
            return true;
        }

        // TODO: generated names should be scoped after Strada port is complete.
        ////if *scope != nil {
        ////	if (*scope).generatedNames.Has(name) {
        ////		return true
        ////	}
        ////}

        while let Some(s) = scope {
            if s.reserved_names.contains(name) {
                return true;
            }
            scope = &s.next;
        }
        false
    }
}

// Go: printer/namegenerator.go:355 nextContainer
fn next_container(node: Node) -> Node {
    if is_locals_container(node) {
        return node.next_container();
    }
    Node::NIL
}

// Go: printer/namegenerator.go:363 isUniqueLocalName
// PORT: Go reads `local.Flags` from the symbol. Locals are binder symbols, so
// this reads them from the program's bound symbol arena.
fn is_unique_local_name(name: &str, container: Node) -> bool {
    let symbols = crate::program::bound_symbols();
    let mut node = container;
    while node.is_some() && is_node_descendant_of(node, container) && is_locals_container(node) {
        let locals = node.locals();
        if locals.is_some() {
            // We conservatively include alias symbols to cover cases where they're emitted as locals
            let local = symbols.get(locals, name);
            if local.is_some()
                && symbols
                    .sym_flags(local)
                    .intersects(SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE | SymbolFlags::ALIAS)
            {
                return false;
            }
        }
        node = next_container(node);
    }
    true
}
