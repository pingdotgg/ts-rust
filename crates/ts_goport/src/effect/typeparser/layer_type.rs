// Go: internal/typeparser/layer_type.go

use crate::effect::typeparser::*;
use crate::prelude::*;
use std::cmp::Ordering;
use std::sync::LazyLock;

static EFFECT_LAYER_MODULE_DESCRIPTOR: LazyLock<PackageSourceFileDescriptor> =
    LazyLock::new(|| new_package_source_file_descriptor("effect", Some(is_layer_type_source_file)));

/// LayerTypeId is the property key for Layer's variance struct.
pub const LAYER_TYPE_ID: &str = "~effect/Layer";

/// Layer represents parsed Layer<ROut, E, RIn> type parameters.
#[derive(Clone, Debug)]
pub struct Layer {
    /// Provided services (contravariant)
    pub r_out: TypeId,
    /// Error type (covariant)
    pub e: TypeId,
    /// Required services (covariant)
    pub r_in: TypeId,
}

impl TypeParser<'_> {
    // Go: typeparser/layer_type.go parseLayerVarianceStruct
    /// parseLayerVarianceStruct extracts ROut, E, RIn from a Layer variance struct type.
    pub fn parse_layer_variance_struct(&mut self, t: TypeId) -> Option<Rc<Layer>> {
        let r_out = self.extract_contravariant_type(t, "_ROut");
        if r_out.is_nil() {
            return None;
        }

        let e = self.extract_covariant_type(t, "_E");
        if e.is_nil() {
            return None;
        }

        let r_in = self.extract_covariant_type(t, "_RIn");
        if r_in.is_nil() {
            return None;
        }

        Some(Rc::new(Layer { r_out, e, r_in }))
    }

    // Go: typeparser/layer_type.go LayerType
    /// LayerType parses a Layer type and extracts ROut, E, RIn parameters.
    /// Returns nil if the type is not a Layer.
    /// The detection strategy is chosen based on the detected Effect version:
    /// v4 uses direct symbol lookup, v3/unknown uses property iteration.
    pub fn layer_type(&mut self, t: TypeId) -> Option<Rc<Layer>> {
        if t.is_nil() {
            return None;
        }
        cached!(self, layer_type, t, 'compute: {
            let version = self.detect_effect_version();
            if version == EffectMajorVersion::V4 {
                let variance_struct_type = self.get_type_of_property_by_name(t, LAYER_TYPE_ID);
                if variance_struct_type.is_nil() {
                    break 'compute None;
                }

                break 'compute self.parse_layer_variance_struct(variance_struct_type);
            }

            // v3 / unknown: iterate properties looking for a layer variance struct
            let props = self.checker.get_properties_of_type_exported(t);

            // Filter to required, non-optional properties with a value declaration
            let mut candidates: Vec<SymbolId> = Vec::new();
            for prop in props {
                if prop.is_nil() {
                    continue;
                }
                let s = self.checker.sym(prop);
                if !s.flags.intersects(SymbolFlags::PROPERTY) {
                    continue;
                }
                if s.flags.intersects(SymbolFlags::OPTIONAL) {
                    continue;
                }
                if s.value_declaration.is_nil() {
                    continue;
                }
                candidates.push(prop);
            }

            if candidates.is_empty() {
                break 'compute None;
            }

            // Sort so properties containing "LayerTypeId" come first (optimization heuristic)
            // Go sort.SliceStable with less(i, j) = iHas && !jHas.
            let c = &*self.checker;
            candidates.sort_by(|&i, &j| {
                let i_has = c.sym(i).name.as_str().contains("LayerTypeId");
                let j_has = c.sym(j).name.as_str().contains("LayerTypeId");
                if i_has && !j_has {
                    Ordering::Less
                } else if j_has && !i_has {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            });

            // Try each candidate as a layer variance struct
            for prop in candidates {
                let prop_type = self.checker.get_type_of_symbol_at_location(prop, Node::NIL);
                if let Some(result) = self.parse_layer_variance_struct(prop_type) {
                    break 'compute Some(result);
                }
            }

            None
        })
    }

    // Go: typeparser/layer_type.go IsLayerType
    /// IsLayerType returns true if the type has the Layer variance struct.
    pub fn is_layer_type(&mut self, t: TypeId) -> bool {
        self.layer_type(t).is_some()
    }
}

// Go: typeparser/layer_type.go isLayerTypeSourceFile
fn is_layer_type_source_file(tp: &mut TypeParser<'_>, sf: Node) -> bool {
    if sf.is_nil() {
        return false;
    }

    let module_sym = tp.checker.get_symbol_of_declaration(sf);
    if module_sym.is_nil() {
        return false;
    }

    let layer_sym = tp
        .checker
        .try_get_member_in_module_exports_and_properties("Layer", module_sym);
    if layer_sym.is_nil() {
        return false;
    }

    let layer_type = tp.checker.get_declared_type_of_symbol_exported(layer_sym);
    if layer_type.is_nil() {
        return false;
    }

    tp.layer_type(layer_type).is_some()
}

impl TypeParser<'_> {
    // Go: typeparser/layer_type.go IsNodeReferenceToEffectLayerModuleApi
    /// IsNodeReferenceToEffectLayerModuleApi reports whether node resolves to a member
    /// exported by the "effect" package from a module that exports the Layer type.
    pub fn is_node_reference_to_effect_layer_module_api(
        &mut self,
        node: Node,
        member: &ExportMember,
    ) -> bool {
        self.is_node_reference_to_module_export(node, &EFFECT_LAYER_MODULE_DESCRIPTOR, member)
    }
}
