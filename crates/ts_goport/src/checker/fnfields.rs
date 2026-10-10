//! Method wrappers for Go function-valued Checker fields, so ported code can
//! write `self.resolve_name(..)` like Go `c.resolveName(..)`. Generated.

use crate::diagnostics::Message;
use crate::prelude::*;

impl Checker {
    pub fn compare_symbols(&mut self, a: SymbolId, b: SymbolId) -> i32 {
        let f = self.compare_symbols.clone();
        f(self, a, b)
    }

    pub fn compare_symbol_chains(&mut self, a: &[SymbolId], b: &[SymbolId]) -> i32 {
        let f = self.compare_symbol_chains.clone();
        f(self, a, b)
    }

    // PORT: takes the plain Go message. A caller that defers the "cannot
    // find name" message calls the `resolve_name` field with a `NameNotFound`.
    pub fn resolve_name(
        &mut self,
        location: Node,
        name: &str,
        meaning: SymbolFlags,
        name_not_found_message: Option<&'static Message>,
        is_use: bool,
        exclude_globals: bool,
    ) -> SymbolId {
        let f = self.resolve_name.clone();
        f(
            self,
            location,
            name,
            meaning,
            name_not_found_message.map(NameNotFound::Message),
            is_use,
            exclude_globals,
        )
    }

    pub fn resolve_name_for_symbol_suggestion(
        &mut self,
        location: Node,
        name: &str,
        meaning: SymbolFlags,
        name_not_found_message: Option<&'static Message>,
        is_use: bool,
        exclude_globals: bool,
    ) -> SymbolId {
        let f = self.resolve_name_for_symbol_suggestion.clone();
        f(
            self,
            location,
            name,
            meaning,
            name_not_found_message.map(NameNotFound::Message),
            is_use,
            exclude_globals,
        )
    }

    pub fn get_global_es_symbol_type(&mut self) -> TypeId {
        let f = self.get_global_es_symbol_type.clone();
        f(self)
    }

    pub fn get_global_big_int_type(&mut self) -> TypeId {
        let f = self.get_global_big_int_type.clone();
        f(self)
    }

    pub fn get_global_import_meta_type(&mut self) -> TypeId {
        let f = self.get_global_import_meta_type.clone();
        f(self)
    }

    pub fn get_global_import_attributes_type(&mut self) -> TypeId {
        let f = self.get_global_import_attributes_type.clone();
        f(self)
    }

    pub fn get_global_import_attributes_type_checked(&mut self) -> TypeId {
        let f = self.get_global_import_attributes_type_checked.clone();
        f(self)
    }

    pub fn get_global_non_nullable_type_alias_or_nil(&mut self) -> SymbolId {
        let f = self.get_global_non_nullable_type_alias_or_nil.clone();
        f(self)
    }

    pub fn get_global_extract_symbol(&mut self) -> SymbolId {
        let f = self.get_global_extract_symbol.clone();
        f(self)
    }

    pub fn get_global_disposable_type(&mut self) -> TypeId {
        let f = self.get_global_disposable_type.clone();
        f(self)
    }

    pub fn get_global_async_disposable_type(&mut self) -> TypeId {
        let f = self.get_global_async_disposable_type.clone();
        f(self)
    }

    pub fn get_global_awaited_symbol(&mut self) -> SymbolId {
        let f = self.get_global_awaited_symbol.clone();
        f(self)
    }

    pub fn get_global_awaited_symbol_or_nil(&mut self) -> SymbolId {
        let f = self.get_global_awaited_symbol_or_nil.clone();
        f(self)
    }

    pub fn get_global_na_n_symbol_or_nil(&mut self) -> SymbolId {
        let f = self.get_global_na_n_symbol_or_nil.clone();
        f(self)
    }

    pub fn get_global_record_symbol(&mut self) -> SymbolId {
        let f = self.get_global_record_symbol.clone();
        f(self)
    }

    pub fn get_global_template_strings_array_type(&mut self) -> TypeId {
        let f = self.get_global_template_strings_array_type.clone();
        f(self)
    }

    pub fn get_global_es_symbol_constructor_symbol_or_nil(&mut self) -> SymbolId {
        let f = self.get_global_es_symbol_constructor_symbol_or_nil.clone();
        f(self)
    }

    pub fn get_global_es_symbol_constructor_type_symbol_or_nil(&mut self) -> SymbolId {
        let f = self
            .get_global_es_symbol_constructor_type_symbol_or_nil
            .clone();
        f(self)
    }

    pub fn get_global_import_call_options_type(&mut self) -> TypeId {
        let f = self.get_global_import_call_options_type.clone();
        f(self)
    }

    pub fn get_global_import_call_options_type_checked(&mut self) -> TypeId {
        let f = self.get_global_import_call_options_type_checked.clone();
        f(self)
    }

    pub fn get_global_promise_type(&mut self) -> TypeId {
        let f = self.get_global_promise_type.clone();
        f(self)
    }

    pub fn get_global_promise_type_checked(&mut self) -> TypeId {
        let f = self.get_global_promise_type_checked.clone();
        f(self)
    }

    pub fn get_global_promise_like_type(&mut self) -> TypeId {
        let f = self.get_global_promise_like_type.clone();
        f(self)
    }

    pub fn get_global_abstract_module_source_type(&mut self) -> TypeId {
        let f = self.get_global_abstract_module_source_type.clone();
        f(self)
    }

    pub fn get_global_promise_constructor_symbol(&mut self) -> SymbolId {
        let f = self.get_global_promise_constructor_symbol.clone();
        f(self)
    }

    pub fn get_global_promise_constructor_symbol_or_nil(&mut self) -> SymbolId {
        let f = self.get_global_promise_constructor_symbol_or_nil.clone();
        f(self)
    }

    pub fn get_global_omit_symbol(&mut self) -> SymbolId {
        let f = self.get_global_omit_symbol.clone();
        f(self)
    }

    pub fn get_global_no_infer_symbol_or_nil(&mut self) -> SymbolId {
        let f = self.get_global_no_infer_symbol_or_nil.clone();
        f(self)
    }

    pub fn get_global_iterator_type(&mut self) -> TypeId {
        let f = self.get_global_iterator_type.clone();
        f(self)
    }

    pub fn get_global_iterable_type(&mut self) -> TypeId {
        let f = self.get_global_iterable_type.clone();
        f(self)
    }

    pub fn get_global_iterable_type_checked(&mut self) -> TypeId {
        let f = self.get_global_iterable_type_checked.clone();
        f(self)
    }

    pub fn get_global_iterable_iterator_type(&mut self) -> TypeId {
        let f = self.get_global_iterable_iterator_type.clone();
        f(self)
    }

    pub fn get_global_iterable_iterator_type_checked(&mut self) -> TypeId {
        let f = self.get_global_iterable_iterator_type_checked.clone();
        f(self)
    }

    pub fn get_global_iterator_object_type(&mut self) -> TypeId {
        let f = self.get_global_iterator_object_type.clone();
        f(self)
    }

    pub fn get_global_generator_type(&mut self) -> TypeId {
        let f = self.get_global_generator_type.clone();
        f(self)
    }

    pub fn get_global_async_iterator_type(&mut self) -> TypeId {
        let f = self.get_global_async_iterator_type.clone();
        f(self)
    }

    pub fn get_global_async_iterable_type(&mut self) -> TypeId {
        let f = self.get_global_async_iterable_type.clone();
        f(self)
    }

    pub fn get_global_async_iterable_type_checked(&mut self) -> TypeId {
        let f = self.get_global_async_iterable_type_checked.clone();
        f(self)
    }

    pub fn get_global_async_iterable_iterator_type(&mut self) -> TypeId {
        let f = self.get_global_async_iterable_iterator_type.clone();
        f(self)
    }

    pub fn get_global_async_iterable_iterator_type_checked(&mut self) -> TypeId {
        let f = self.get_global_async_iterable_iterator_type_checked.clone();
        f(self)
    }

    pub fn get_global_async_iterator_object_type(&mut self) -> TypeId {
        let f = self.get_global_async_iterator_object_type.clone();
        f(self)
    }

    pub fn get_global_async_generator_type(&mut self) -> TypeId {
        let f = self.get_global_async_generator_type.clone();
        f(self)
    }

    pub fn get_global_iterator_yield_result_type(&mut self) -> TypeId {
        let f = self.get_global_iterator_yield_result_type.clone();
        f(self)
    }

    pub fn get_global_iterator_return_result_type(&mut self) -> TypeId {
        let f = self.get_global_iterator_return_result_type.clone();
        f(self)
    }

    pub fn get_global_typed_property_descriptor_type(&mut self) -> TypeId {
        let f = self.get_global_typed_property_descriptor_type.clone();
        f(self)
    }

    pub fn get_global_class_decorator_context_type(&mut self) -> TypeId {
        let f = self.get_global_class_decorator_context_type.clone();
        f(self)
    }

    pub fn get_global_class_method_decorator_context_type(&mut self) -> TypeId {
        let f = self.get_global_class_method_decorator_context_type.clone();
        f(self)
    }

    pub fn get_global_class_getter_decorator_context_type(&mut self) -> TypeId {
        let f = self.get_global_class_getter_decorator_context_type.clone();
        f(self)
    }

    pub fn get_global_class_setter_decorator_context_type(&mut self) -> TypeId {
        let f = self.get_global_class_setter_decorator_context_type.clone();
        f(self)
    }

    pub fn get_global_class_accessor_decorator_contxt_type(&mut self) -> TypeId {
        let f = self.get_global_class_accessor_decorator_contxt_type.clone();
        f(self)
    }

    pub fn get_global_class_accessor_decorator_context_type(&mut self) -> TypeId {
        let f = self
            .get_global_class_accessor_decorator_context_type
            .clone();
        f(self)
    }

    pub fn get_global_class_accessor_decorator_target_type(&mut self) -> TypeId {
        let f = self.get_global_class_accessor_decorator_target_type.clone();
        f(self)
    }

    pub fn get_global_class_accessor_decorator_result_type(&mut self) -> TypeId {
        let f = self.get_global_class_accessor_decorator_result_type.clone();
        f(self)
    }

    pub fn get_global_class_field_decorator_context_type(&mut self) -> TypeId {
        let f = self.get_global_class_field_decorator_context_type.clone();
        f(self)
    }

    pub fn is_primitive_or_object_or_empty_type(&mut self, t: TypeId) -> bool {
        let f = self.is_primitive_or_object_or_empty_type.clone();
        f(self, t)
    }

    pub fn contains_missing_type(&mut self, t: TypeId) -> bool {
        let f = self.contains_missing_type.clone();
        f(self, t)
    }

    /// Go `c.couldContainTypeVariables(t)`. Go stores a func field that only
    /// calls the worker, so the port has no field and calls it directly. The
    /// first two worker tests run inline here, because they answer most calls.
    #[inline]
    pub fn could_contain_type_variables(&mut self, t: TypeId) -> bool {
        let ty = self.ty(t);
        if !ty.flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE) {
            return false;
        }
        if ty
            .object_flags
            .intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED)
        {
            return ty
                .object_flags
                .intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES);
        }
        self.could_contain_type_variables_worker(t)
    }

    pub fn is_string_index_signature_only_type(&mut self, t: TypeId) -> bool {
        let f = self.is_string_index_signature_only_type.clone();
        f(self, t)
    }

    pub fn mark_node_assignments(&mut self, node: Node) -> bool {
        let f = self.mark_node_assignments.clone();
        f(self, node)
    }
}
