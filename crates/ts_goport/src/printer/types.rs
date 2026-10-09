//! Go package `printer`: the small type files.
//!
//! Ports `emitflags.go`, `generatedidentifierflags.go`, `emittextwriter.go`,
//! `emitresolver.go`, `emithost.go` and `sourcefilemetadataprovider.go`.

use crate::flags_macros::{go_enum, go_flags};
use crate::prelude::*;

// ──────────────────────────────────────────────────────────────────────
// emitflags.go
// ──────────────────────────────────────────────────────────────────────

// Go: printer/emitflags.go:3 EmitFlags
go_flags!(EmitFlags, u32 {
    NONE = 0; // EFNone
    SINGLE_LINE = 1 << 0; // The contents of this node should be emitted on a single line.
    MULTI_LINE = 1 << 1; // The contents of this node should be emitted on multiple lines.
    NO_LEADING_SOURCE_MAP = 1 << 2; // Do not emit a leading source map location for this node.
    NO_TRAILING_SOURCE_MAP = 1 << 3; // Do not emit a trailing source map location for this node.
    NO_NESTED_SOURCE_MAPS = 1 << 4; // Do not emit source map locations for children of this node.
    NO_TOKEN_LEADING_SOURCE_MAPS = 1 << 5; // Do not emit leading source map location for token nodes.
    NO_TOKEN_TRAILING_SOURCE_MAPS = 1 << 6; // Do not emit trailing source map location for token nodes.
    NO_LEADING_COMMENTS = 1 << 7; // Do not emit leading comments for this node.
    NO_TRAILING_COMMENTS = 1 << 8; // Do not emit trailing comments for this node.
    NO_NESTED_COMMENTS = 1 << 9; // Do not emit nested comments for children of this node.
    HELPER_NAME = 1 << 10; // The Identifier refers to an *unscoped* emit helper (one that is emitted at the top of the file)
    EXPORT_NAME = 1 << 11; // Ensure an export prefix is added for an identifier that points to an exported declaration with a local name (see SymbolFlags.ExportHasLocal).
    LOCAL_NAME = 1 << 12; // Ensure an export prefix is not added for an identifier that points to an exported declaration.
    INDENTED = 1 << 13; // Adds an explicit extra indentation level for class and function bodies when printing (used to match old emitter).
    NO_INDENTATION = 1 << 14; // Do not indent the node.
    REUSE_TEMP_VARIABLE_SCOPE = 1 << 15; // Reuse the existing temp variable scope during emit.
    CUSTOM_PROLOGUE = 1 << 16; // Treat the statement as if it were a prologue directive (NOTE: Prologue directives are *not* transformed).
    NO_ASCII_ESCAPING = 1 << 17; // When synthesizing nodes that lack an original node or textSourceNode, we want to write the text on the node with ASCII escaping substitutions.
    EXTERNAL_HELPERS = 1 << 18; // This source file has external helpers
    START_ON_NEW_LINE = 1 << 19; // Start this node on a new line
    INDIRECT_CALL = 1 << 20; // Emit CallExpression as an indirect call: `(0, f)()`
    ASYNC_FUNCTION_BODY = 1 << 21; // The node was originally an async function body.
    NO_LEXICAL_ARGUMENTS = 1 << 22; // Do not capture `arguments` for this arrow function.
    TRANSFORM_PRIVATE_STATIC_ELEMENTS = 1 << 23; // Indicates static private elements in a file or class should be transformed regardless of --target (used by esDecorators transform).
    NO_LEXICAL_THIS = 1 << 24; // Do not capture `this` for this node's subtree.
    NO_SOURCE_MAP = (1 << 2) | (1 << 3); // EFNoLeadingSourceMap | EFNoTrailingSourceMap
    NO_TOKEN_SOURCE_MAPS = (1 << 5) | (1 << 6); // EFNoTokenLeadingSourceMaps | EFNoTokenTrailingSourceMaps
    NO_COMMENTS = (1 << 7) | (1 << 8); // EFNoLeadingComments | EFNoTrailingComments
});

// ──────────────────────────────────────────────────────────────────────
// generatedidentifierflags.go
// ──────────────────────────────────────────────────────────────────────

// Go: printer/generatedidentifierflags.go:3 GeneratedIdentifierFlags
go_flags!(GeneratedIdentifierFlags, i32 {
    // Kind
    NONE = 0; // Not automatically generated.
    AUTO = 1; // Automatically generated identifier.
    LOOP = 2; // Automatically generated identifier with a preference for '_i'.
    UNIQUE = 3; // Unique name based on the 'text' property.
    NODE = 4; // Unique name based on the node in the 'Node' property.
    KIND_MASK = 7; // Mask to extract the kind of identifier from its flags.
    // Flags
    RESERVED_IN_NESTED_SCOPES = 1 << 3; // Reserve the generated name in nested scopes
    OPTIMISTIC = 1 << 4; // First instance won't use '_#' if there's no conflict
    FILE_LEVEL = 1 << 5; // Use only the file identifiers list and not generated names to search for conflicts
    ALLOW_NAME_SUBSTITUTION = 1 << 6; // Used by `module.ts` to indicate generated nodes which can have substitutions performed upon them
});

impl GeneratedIdentifierFlags {
    // Go: printer/generatedidentifierflags.go:23 Kind
    #[must_use]
    pub fn kind(self) -> GeneratedIdentifierFlags {
        self & GeneratedIdentifierFlags::KIND_MASK
    }

    // Go: printer/generatedidentifierflags.go:27 IsAuto
    #[must_use]
    pub fn is_auto(self) -> bool {
        self.kind() == GeneratedIdentifierFlags::AUTO
    }

    // Go: printer/generatedidentifierflags.go:31 IsLoop
    #[must_use]
    pub fn is_loop(self) -> bool {
        self.kind() == GeneratedIdentifierFlags::LOOP
    }

    // Go: printer/generatedidentifierflags.go:35 IsUnique
    #[must_use]
    pub fn is_unique(self) -> bool {
        self.kind() == GeneratedIdentifierFlags::UNIQUE
    }

    // Go: printer/generatedidentifierflags.go:39 IsNode
    #[must_use]
    pub fn is_node(self) -> bool {
        self.kind() == GeneratedIdentifierFlags::NODE
    }

    // Go: printer/generatedidentifierflags.go:43 IsReservedInNestedScopes
    #[must_use]
    pub fn is_reserved_in_nested_scopes(self) -> bool {
        self.intersects(GeneratedIdentifierFlags::RESERVED_IN_NESTED_SCOPES)
    }

    // Go: printer/generatedidentifierflags.go:47 IsOptimistic
    #[must_use]
    pub fn is_optimistic(self) -> bool {
        self.intersects(GeneratedIdentifierFlags::OPTIMISTIC)
    }

    // Go: printer/generatedidentifierflags.go:51 IsFileLevel
    #[must_use]
    pub fn is_file_level(self) -> bool {
        self.intersects(GeneratedIdentifierFlags::FILE_LEVEL)
    }

    // Go: printer/generatedidentifierflags.go:55 HasAllowNameSubstitution
    #[must_use]
    pub fn has_allow_name_substitution(self) -> bool {
        self.intersects(GeneratedIdentifierFlags::ALLOW_NAME_SUBSTITUTION)
    }
}

// ──────────────────────────────────────────────────────────────────────
// emittextwriter.go
// ──────────────────────────────────────────────────────────────────────

/// Go `printer.EmitTextWriter`: externally opaque interface for printing text.
// PORT: Go `core.UTF16Offset` is `i32` (the crate's `utf16_len` type).
// Go `String()` is `string()`.
// Go: printer/emittextwriter.go:9 EmitTextWriter
pub trait EmitTextWriter {
    fn write(&mut self, s: &str);
    fn write_trailing_semicolon(&mut self, text: &str);
    fn write_comment(&mut self, text: &str);
    fn write_keyword(&mut self, text: &str);
    fn write_operator(&mut self, text: &str);
    fn write_punctuation(&mut self, text: &str);
    fn write_space(&mut self, text: &str);
    fn write_string_literal(&mut self, text: &str);
    fn write_parameter(&mut self, text: &str);
    fn write_property(&mut self, text: &str);
    fn write_symbol(&mut self, text: &str, symbol: SymbolId);
    fn write_line(&mut self);
    fn write_line_force(&mut self, force: bool);
    fn increase_indent(&mut self);
    fn decrease_indent(&mut self);
    fn clear(&mut self);
    fn string(&self) -> String;
    fn raw_write(&mut self, s: &str);
    fn write_literal(&mut self, s: &str);
    fn get_text_pos(&self) -> i32;
    fn get_line(&self) -> i32;
    fn get_column(&self) -> i32;
    fn get_indent(&self) -> i32;
    fn is_at_start_of_line(&self) -> bool;
    fn has_trailing_comment(&self) -> bool;
    fn has_trailing_whitespace(&self) -> bool;

    /// PORT: not in the Go interface. Go `textWriter.Grow`, here on the trait
    /// so the emitter can size the output buffer before it prints. Other
    /// writers ignore it.
    fn grow(&mut self, _n: i32) {}

    /// PORT: not in Go. `string()` then `clear()`. A writer can override it
    /// to move its text out instead of copying it.
    fn take_string(&mut self) -> String {
        let text = self.string();
        self.clear();
        text
    }
}

// ──────────────────────────────────────────────────────────────────────
// emitresolver.go
// ──────────────────────────────────────────────────────────────────────

// Go: printer/emitresolver.go:10 SymbolAccessibility
go_enum!(SymbolAccessibility, i32 {
    ACCESSIBLE = 0; // SymbolAccessibilityAccessible
    NOT_ACCESSIBLE = 1; // SymbolAccessibilityNotAccessible
    CANNOT_BE_NAMED = 2; // SymbolAccessibilityCannotBeNamed
    NOT_RESOLVED = 3; // SymbolAccessibilityNotResolved
});

/// Go `printer.SymbolAccessibilityResult`.
// Go: printer/emitresolver.go:19 SymbolAccessibilityResult
#[derive(Clone, Debug, Default)]
pub struct SymbolAccessibilityResult {
    pub accessibility: SymbolAccessibility,
    pub aliases_to_make_visible: Vec<Node>, // aliases that need to have this symbol visible
    pub error_symbol_name: String,          // Optional - symbol name that results in error
    pub error_node: Node,                   // Optional - node that results in error
    pub error_module_name: String, // Optional - If the symbol is not visible from module, module's name
}

// Go: printer/emitresolver.go:32 TypeReferenceSerializationKind
// Indicates how to serialize the name for a TypeReferenceNode when emitting decorator metadata
go_enum!(TypeReferenceSerializationKind, i32 {
    // The TypeReferenceNode could not be resolved.
    // The type name should be emitted using a safe fallback.
    UNKNOWN = 0;
    // The TypeReferenceNode resolves to a type with a constructor
    // function that can be reached at runtime (e.g. a `class`
    // declaration or a `var` declaration for the static side
    // of a type, such as the global `Promise` type in lib.d.ts).
    TYPE_WITH_CONSTRUCT_SIGNATURE_AND_VALUE = 1;
    // The TypeReferenceNode resolves to a Void-like, Nullable, or Never type.
    VOID_NULLABLE_OR_NEVER_TYPE = 2;
    // The TypeReferenceNode resolves to a Number-like type.
    NUMBER_LIKE_TYPE = 3;
    // The TypeReferenceNode resolves to a BigInt-like type.
    BIG_INT_LIKE_TYPE = 4;
    // The TypeReferenceNode resolves to a String-like type.
    STRING_LIKE_TYPE = 5;
    // The TypeReferenceNode resolves to a Boolean-like type.
    BOOLEAN_TYPE = 6;
    // The TypeReferenceNode resolves to an Array-like type.
    ARRAY_LIKE_TYPE = 7;
    // The TypeReferenceNode resolves to the ESSymbol type.
    ES_SYMBOL_TYPE = 8;
    // The TypeReferenceNode resolved to the global Promise constructor symbol.
    PROMISE = 9;
    // The TypeReferenceNode resolves to a Function type or a type with call signatures.
    TYPE_WITH_CALL_SIGNATURE = 10;
    // The TypeReferenceNode resolves to any other type.
    OBJECT_TYPE = 11;
});

/// Go `nodebuilder.SymbolTracker` as the emit resolver receives it (an
/// interface value that may be nil).
// PORT: Go passes the interface by value; here it is a shared handle.
pub type EmitSymbolTracker = Option<Rc<dyn SymbolTracker>>;

/// Go `printer.EmitResolver`.
// PORT: Go embeds `binder.ReferenceResolver`; its six methods are the
// first methods of this trait.
// PORT: Go methods on the resolver lock a mutex and call the checker. Here
// they take `&self`; an implementation holds its checker in a `RefCell`.
// PORT: Go `GetConstantValue` returns `any` (string, float64 or nil); that is
// `Option<LiteralValue>`. Go `*ast.SourceFile` results are `Node`
// (`Node::NIL` for nil).
// Go: printer/emitresolver.go:76 EmitResolver
pub trait EmitResolver {
    // Go binder.ReferenceResolver (embedded)
    fn get_referenced_export_container(&self, node: Node, prefix_locals: bool) -> Node;
    fn get_referenced_import_declaration(&self, node: Node) -> Node;
    fn get_referenced_value_declarations(&self, node: Node) -> Vec<Node>;
    fn get_referenced_value_declaration(&self, node: Node) -> Node;
    fn get_element_access_expression_name(&self, expression: Node) -> String;
    fn get_referenced_member_value_declaration(&self, node: Node) -> Node;
    /// PORT: not in Go. Go reads `symbol.ValueDeclaration` directly; here
    /// symbols live in the checker arena, so callers outside the checker
    /// read it through the resolver.
    fn symbol_value_declaration(&self, symbol: SymbolId) -> Node;
    /// PORT: not in Go. Go `make(ast.SymbolTable)` plus `table[name] = symbol`; tables live in the checker arena.
    fn make_symbol_table(&self, entries: &[(&str, SymbolId)]) -> SymbolTable;

    fn is_referenced_alias_declaration(&self, node: Node) -> bool;
    fn is_value_alias_declaration(&self, node: Node) -> bool;
    fn is_top_level_value_import_equals_with_entity_name(&self, node: Node) -> bool;
    fn mark_linked_references_recursively(&self, file: Node);
    fn get_external_module_file_from_declaration(&self, node: Node) -> Node;
    fn get_effective_declaration_flags(&self, node: Node, flags: ModifierFlags) -> ModifierFlags;

    // decorator metadata
    fn get_type_reference_serialization_kind(
        &self,
        name: Node,
        serial_scope: Node,
    ) -> TypeReferenceSerializationKind;

    // const enum inlining
    fn get_constant_value(&self, node: Node) -> Option<LiteralValue>;

    // JSX Emit
    fn get_jsx_factory_entity(&self, location: Node) -> Node;
    fn get_jsx_fragment_factory_entity(&self, location: Node) -> Node;
    // for overriding the reference resolver behavior for generated identifiers
    fn set_referenced_import_declaration(&self, node: Node, ref_: Node);

    // declaration emit checker functionality projections
    fn precalculate_declaration_emit_visibility(&self, file: Node);
    fn is_symbol_accessible(
        &self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        should_compute_alias_to_mark_visible: bool,
    ) -> SymbolAccessibilityResult;
    // previously SymbolVisibilityResult in strada - ErrorModuleName never set
    fn is_entity_name_visible(
        &self,
        entity_name: Node,
        enclosing_declaration: Node,
    ) -> SymbolAccessibilityResult;
    fn is_expando_function_declaration(&self, node: Node) -> bool;
    fn is_expando_function_declaration_unsafe(&self, node: Node) -> bool;
    fn is_literal_const_declaration(&self, node: Node) -> bool;
    fn requires_adding_implicit_undefined(
        &self,
        node: Node,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool;
    fn is_declaration_visible(&self, node: Node) -> bool;
    fn is_name_resolvable(&self, location: Node, name: &str) -> bool;
    fn is_import_required_by_augmentation(&self, decl: Node) -> bool;
    fn is_definitely_reference_to_global_symbol_object(&self, node: Node) -> bool;
    fn is_implementation_of_overload(&self, node: Node) -> bool;
    fn get_enum_member_value(&self, node: Node) -> EvaluatorResult;
    fn is_late_bound(&self, node: Node) -> bool;
    fn is_optional_parameter(&self, node: Node) -> bool;
    fn is_this_property_assignment_declaration_redundant(&self, node: Node) -> bool;

    // isolatedDeclarations-specific declaration emit
    fn get_properties_of_container_function(&self, node: Node) -> Vec<SymbolId>;
    fn requires_adding_implicit_undefined_unsafe(
        &self,
        node: Node,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool;
    fn get_referenced_value_declaration_unsafe(&self, node: Node) -> Node;

    // Node construction for declaration emit
    fn create_type_of_declaration(
        &self,
        emit_context: &EmitContext,
        declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node;
    fn create_return_type_of_signature_declaration(
        &self,
        emit_context: &EmitContext,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node;
    fn create_type_parameters_of_signature_declaration(
        &self,
        emit_context: &EmitContext,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Vec<Node>;
    fn create_literal_const_value(
        &self,
        emit_context: &EmitContext,
        node: Node,
        tracker: EmitSymbolTracker,
    ) -> Node;
    fn create_type_of_expression(
        &self,
        emit_context: &EmitContext,
        expression: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node;
    fn create_late_bound_index_signatures(
        &self,
        emit_context: &EmitContext,
        container: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Vec<Node>;
    fn try_js_type_node_to_type_node(
        &self,
        emit_context: &EmitContext,
        type_node: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node;
}

// ──────────────────────────────────────────────────────────────────────
// emithost.go
// ──────────────────────────────────────────────────────────────────────

/// Go `printer.EmitHost`.
// NOTE: EmitHost operations must be thread-safe
// PORT: Go `WriteFile` returns `error`; here `Result<(), String>`.
// PORT: Go `GetProjectReferenceFromSource(path tspath.Path)
// *tsoptions.SourceOutputAndProjectReference` is left out: tsoptions project
// references are not ported in this crate.
// PORT: ts#64159 types the paths, replaces `UseCaseSensitiveFileNames` with
// `CaseSensitivity()` and removes `GetCurrentDirectory`. The port keeps the
// bool and the method until the emit host lanes change this trait; no emit
// code reads the current directory.
// Go: printer/emithost.go:11 EmitHost
pub trait EmitHost {
    fn options(&self) -> &CompilerOptions;
    fn source_files(&self) -> Vec<Node>;
    fn use_case_sensitive_file_names(&self) -> bool;
    fn get_current_directory(&self) -> String;
    fn common_source_directory(&self) -> String;
    fn is_emit_blocked(&self, file: &str) -> bool;
    fn write_file(&self, file_name: &str, text: &str) -> Result<(), String>;
    fn get_emit_module_format_of_file(&self, file: Node) -> ModuleKind;
    fn get_emit_resolver(&self) -> Rc<dyn EmitResolver>;
    fn is_source_file_from_external_library(&self, file: Node) -> bool;
}

// ──────────────────────────────────────────────────────────────────────
// sourcefilemetadataprovider.go
// ──────────────────────────────────────────────────────────────────────

/// Go `printer.SourceFileMetaDataProvider`.
// PORT: `tspath.Path` is `&str`; a nil `*ast.SourceFileMetaData` is `None`.
// Go: printer/sourcefilemetadataprovider.go:8 SourceFileMetaDataProvider
pub trait SourceFileMetaDataProvider {
    fn get_source_file_meta_data(&self, path: &str) -> Option<SourceFileMetaData>;
}
