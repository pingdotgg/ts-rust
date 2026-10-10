// Go: internal/typeparser/effect_model_type.go

use crate::effect::typeparser::*;
use crate::prelude::*;
use std::sync::LazyLock;

static EFFECT_MODEL_PACKAGE_SOURCE_FILE_DESCRIPTOR: LazyLock<PackageSourceFileDescriptor> =
    LazyLock::new(|| {
        new_package_source_file_descriptor("effect", Some(is_effect_model_type_source_file))
    });

// Go: typeparser/effect_model_type.go isEffectModelTypeSourceFile
/// isEffectModelTypeSourceFile checks if a source file is the effect/schema Model module
/// by verifying it exports "Class", "Generated", and "FieldOption".
/// These symbols are chosen to disambiguate Model from Schema (which also exports "Class"),
/// matching the TypeScript reference implementation.
fn is_effect_model_type_source_file(tp: &mut TypeParser<'_>, sf: Node) -> bool {
    if sf.is_nil() {
        return false;
    }

    let module_sym = tp.checker.get_symbol_of_declaration(sf);
    if module_sym.is_nil() {
        return false;
    }

    if tp
        .checker
        .try_get_member_in_module_exports_and_properties("Class", module_sym)
        .is_nil()
    {
        return false;
    }
    // Generated was split into GeneratedByDb / GeneratedByApp in newer v4 betas.
    if tp
        .checker
        .try_get_member_in_module_exports_and_properties("Generated", module_sym)
        .is_nil()
        && tp
            .checker
            .try_get_member_in_module_exports_and_properties("GeneratedByDb", module_sym)
            .is_nil()
        && tp
            .checker
            .try_get_member_in_module_exports_and_properties("GeneratedByApp", module_sym)
            .is_nil()
    {
        return false;
    }
    // FieldOption is unique to v4 Model
    if tp
        .checker
        .try_get_member_in_module_exports_and_properties("FieldOption", module_sym)
        .is_nil()
    {
        return false;
    }

    true
}

impl TypeParser<'_> {
    // Go: typeparser/effect_model_type.go IsNodeReferenceToEffectModelModuleApi
    /// IsNodeReferenceToEffectModelModuleApi reports whether node resolves to a member
    /// exported by the "effect" package from a module that exports the Model API
    /// (effect/schema).
    pub fn is_node_reference_to_effect_model_module_api(
        &mut self,
        node: Node,
        member_name: &str,
    ) -> bool {
        self.is_node_reference_to_module_export(
            node,
            &EFFECT_MODEL_PACKAGE_SOURCE_FILE_DESCRIPTOR,
            member_name,
        )
    }
}
