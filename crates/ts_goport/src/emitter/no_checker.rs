//! PORT: not in Go. The emit resolver of the JS part of a file that runs on
//! the program's emit pool (`program_emit`), where there is no checker.
//! `emitter::js_emit_needs_checker` sends a JS part there only when its
//! transforms make no emit resolver call. Every method panics, so a call
//! that the rule misses ends the run (exit 70). It can never give an output
//! that differs from the checker thread's. The one exception is
//! `emit_context` (ts#64649): the script transforms read their emit context
//! from the resolver (Go emitter.go:118), and that needs no checker.

use crate::prelude::*;

/// An emit resolver whose every method except `emit_context` panics (see
/// the file comment).
pub struct NoCheckerEmitResolver {
    /// The emit context that the resolver was made for.
    pub emit_context: Rc<EmitContext>,
}

/// The panic of every `NoCheckerEmitResolver` method.
fn no_checker(method: &str) -> ! {
    panic!("JS emit off the checker thread called EmitResolver::{method}");
}

impl crate::printer::EmitResolver for NoCheckerEmitResolver {
    fn emit_context(&self) -> &Rc<EmitContext> {
        &self.emit_context
    }

    fn get_referenced_export_container(&self, _node: Node, _prefix_locals: bool) -> Node {
        no_checker("get_referenced_export_container")
    }

    fn get_referenced_import_declaration(&self, _node: Node) -> Node {
        no_checker("get_referenced_import_declaration")
    }

    fn get_referenced_value_declarations(&self, _node: Node) -> Vec<Node> {
        no_checker("get_referenced_value_declarations")
    }

    fn get_referenced_value_declaration(&self, _node: Node) -> Node {
        no_checker("get_referenced_value_declaration")
    }

    fn get_element_access_expression_name(&self, _expression: Node) -> String {
        no_checker("get_element_access_expression_name")
    }

    fn get_referenced_member_value_declaration(&self, _node: Node) -> Node {
        no_checker("get_referenced_member_value_declaration")
    }

    fn symbol_value_declaration(&self, _symbol: SymbolId) -> Node {
        no_checker("symbol_value_declaration")
    }

    fn make_symbol_table(&self, _entries: &[(&str, SymbolId)]) -> SymbolTable {
        no_checker("make_symbol_table")
    }

    fn is_referenced_alias_declaration(&self, _node: Node) -> bool {
        no_checker("is_referenced_alias_declaration")
    }

    fn is_value_alias_declaration(&self, _node: Node) -> bool {
        no_checker("is_value_alias_declaration")
    }

    fn is_top_level_value_import_equals_with_entity_name(&self, _node: Node) -> bool {
        no_checker("is_top_level_value_import_equals_with_entity_name")
    }

    fn mark_linked_references_recursively(&self, _file: Node) {
        no_checker("mark_linked_references_recursively")
    }

    fn get_external_module_file_from_declaration(&self, _node: Node) -> Node {
        no_checker("get_external_module_file_from_declaration")
    }

    fn get_effective_declaration_flags(&self, _node: Node, _flags: ModifierFlags) -> ModifierFlags {
        no_checker("get_effective_declaration_flags")
    }

    fn get_type_reference_serialization_kind(
        &self,
        _name: Node,
        _serial_scope: Node,
    ) -> TypeReferenceSerializationKind {
        no_checker("get_type_reference_serialization_kind")
    }

    fn get_constant_value(&self, _node: Node) -> Option<LiteralValue> {
        no_checker("get_constant_value")
    }

    fn get_jsx_factory_entity(&self, _location: Node) -> Node {
        no_checker("get_jsx_factory_entity")
    }

    fn get_jsx_fragment_factory_entity(&self, _location: Node) -> Node {
        no_checker("get_jsx_fragment_factory_entity")
    }

    fn set_referenced_import_declaration(&self, _node: Node, _ref: Node) {
        no_checker("set_referenced_import_declaration")
    }

    fn precalculate_declaration_emit_visibility(&self, _file: Node) {
        no_checker("precalculate_declaration_emit_visibility")
    }

    fn is_symbol_accessible(
        &self,
        _symbol: SymbolId,
        _enclosing_declaration: Node,
        _meaning: SymbolFlags,
        _should_compute_alias_to_mark_visible: bool,
    ) -> SymbolAccessibilityResult {
        no_checker("is_symbol_accessible")
    }

    fn is_entity_name_visible(
        &self,
        _entity_name: Node,
        _enclosing_declaration: Node,
    ) -> SymbolAccessibilityResult {
        no_checker("is_entity_name_visible")
    }

    fn is_expando_function_declaration(&self, _node: Node) -> bool {
        no_checker("is_expando_function_declaration")
    }

    fn is_expando_function_declaration_unsafe(&self, _node: Node) -> bool {
        no_checker("is_expando_function_declaration_unsafe")
    }

    fn is_literal_const_declaration(&self, _node: Node) -> bool {
        no_checker("is_literal_const_declaration")
    }

    fn requires_adding_implicit_undefined(
        &self,
        _node: Node,
        _symbol: SymbolId,
        _enclosing_declaration: Node,
    ) -> bool {
        no_checker("requires_adding_implicit_undefined")
    }

    fn is_declaration_visible(&self, _node: Node) -> bool {
        no_checker("is_declaration_visible")
    }

    fn is_name_resolvable(&self, _location: Node, _name: &str) -> bool {
        no_checker("is_name_resolvable")
    }

    fn is_import_required_by_augmentation(&self, _decl: Node) -> bool {
        no_checker("is_import_required_by_augmentation")
    }

    fn is_definitely_reference_to_global_symbol_object(&self, _node: Node) -> bool {
        no_checker("is_definitely_reference_to_global_symbol_object")
    }

    fn is_implementation_of_overload(&self, _node: Node) -> bool {
        no_checker("is_implementation_of_overload")
    }

    fn get_enum_member_value(&self, _node: Node) -> EvaluatorResult {
        no_checker("get_enum_member_value")
    }

    fn is_late_bound(&self, _node: Node) -> bool {
        no_checker("is_late_bound")
    }

    fn is_optional_parameter(&self, _node: Node) -> bool {
        no_checker("is_optional_parameter")
    }

    fn is_this_property_assignment_declaration_redundant(&self, _node: Node) -> bool {
        no_checker("is_this_property_assignment_declaration_redundant")
    }

    fn get_properties_of_container_function(&self, _node: Node) -> Vec<SymbolId> {
        no_checker("get_properties_of_container_function")
    }

    fn requires_adding_implicit_undefined_unsafe(
        &self,
        _node: Node,
        _symbol: SymbolId,
        _enclosing_declaration: Node,
    ) -> bool {
        no_checker("requires_adding_implicit_undefined_unsafe")
    }

    fn get_referenced_value_declaration_unsafe(&self, _node: Node) -> Node {
        no_checker("get_referenced_value_declaration_unsafe")
    }

    fn create_type_of_declaration(
        &self,
        _declaration: Node,
        _enclosing_declaration: Node,
        _flags: NodeBuilderFlags,
        _internal_flags: InternalNodeBuilderFlags,
        _tracker: EmitSymbolTracker,
    ) -> Node {
        no_checker("create_type_of_declaration")
    }

    fn create_return_type_of_signature_declaration(
        &self,
        _signature_declaration: Node,
        _enclosing_declaration: Node,
        _flags: NodeBuilderFlags,
        _internal_flags: InternalNodeBuilderFlags,
        _tracker: EmitSymbolTracker,
    ) -> Node {
        no_checker("create_return_type_of_signature_declaration")
    }

    fn create_type_parameters_of_signature_declaration(
        &self,
        _signature_declaration: Node,
        _enclosing_declaration: Node,
        _flags: NodeBuilderFlags,
        _internal_flags: InternalNodeBuilderFlags,
        _tracker: EmitSymbolTracker,
    ) -> Vec<Node> {
        no_checker("create_type_parameters_of_signature_declaration")
    }

    fn create_literal_const_value(&self, _node: Node, _tracker: EmitSymbolTracker) -> Node {
        no_checker("create_literal_const_value")
    }

    fn create_type_of_expression(
        &self,
        _expression: Node,
        _enclosing_declaration: Node,
        _flags: NodeBuilderFlags,
        _internal_flags: InternalNodeBuilderFlags,
        _tracker: EmitSymbolTracker,
    ) -> Node {
        no_checker("create_type_of_expression")
    }

    fn create_late_bound_index_signatures(
        &self,
        _container: Node,
        _enclosing_declaration: Node,
        _flags: NodeBuilderFlags,
        _internal_flags: InternalNodeBuilderFlags,
        _tracker: EmitSymbolTracker,
    ) -> Vec<Node> {
        no_checker("create_late_bound_index_signatures")
    }

    fn try_js_type_node_to_type_node(
        &self,
        _type_node: Node,
        _enclosing_declaration: Node,
        _flags: NodeBuilderFlags,
        _internal_flags: InternalNodeBuilderFlags,
        _tracker: EmitSymbolTracker,
    ) -> Node {
        no_checker("try_js_type_node_to_type_node")
    }
}
