// Go: internal/typeparser/effect_type.go

use crate::effect::typeparser::*;
use crate::prelude::*;
use std::sync::LazyLock;

pub static EFFECT_MODULE_DESCRIPTOR: LazyLock<PackageSourceFileDescriptor> = LazyLock::new(|| {
    new_package_source_file_descriptor("effect", Some(is_effect_type_source_file))
});

pub static EFFECT_PACKAGE_EXPORT_DESCRIPTOR: LazyLock<PackageSourceFileDescriptor> =
    LazyLock::new(|| new_package_source_file_descriptor("effect", None));

/// EffectTypeId is the property key for Effect's variance struct.
/// Effect v4 (effect-smol) uses this pattern to encode type parameters.
pub const EFFECT_TYPE_ID: &str = "~effect/Effect";

/// Effect represents parsed Effect<A, E, R> type parameters.
#[derive(Clone, Debug)]
pub struct Effect {
    /// Success type
    pub a: TypeId,
    /// Error type
    pub e: TypeId,
    /// Requirements type
    pub r: TypeId,
}

impl TypeParser<'_> {
    /// EffectType parses an Effect type and extracts A, E, R parameters.
    /// Returns nil if the type is not an Effect.
    /// The detection strategy is chosen based on the detected Effect version:
    /// v4 uses direct symbol lookup, v3/unknown uses property iteration.
    pub fn effect_type(&mut self, t: TypeId) -> Option<Rc<Effect>> {
        if t.is_nil() {
            return None;
        }
        cached!(self, effect_type, t, 'compute: {
            let version = self.detect_effect_version();
            if version == EffectMajorVersion::V4 {
                let variance_struct_type = self.get_type_of_property_by_name(t, EFFECT_TYPE_ID);
                if variance_struct_type.is_nil() {
                    break 'compute None;
                }

                // Parse the variance struct to extract A, E, R
                break 'compute self.parse_variance_struct(variance_struct_type);
            }

            // v3 / unknown: iterate properties looking for a variance struct
            let props = self.checker.get_properties_of_type_exported(t);

            // Filter to required, non-optional properties with a value declaration
            let mut candidates: Vec<SymbolId> = Vec::new();
            for prop in props {
                if prop.is_nil() {
                    continue;
                }
                let symbol = self.checker.sym(prop);
                if !symbol.flags.intersects(SymbolFlags::PROPERTY) {
                    continue;
                }
                if symbol.flags.intersects(SymbolFlags::OPTIONAL) {
                    continue;
                }
                if symbol.value_declaration.is_nil() {
                    continue;
                }
                candidates.push(prop);
            }

            if candidates.is_empty() {
                break 'compute None;
            }

            // Sort so properties containing "EffectTypeId" come first (optimization heuristic)
            // PORT: Go sort.SliceStable with less(i, j) = iHas && !jHas.
            let mut keyed: Vec<(SymbolId, bool)> = candidates
                .iter()
                .map(|&prop| {
                    (
                        prop,
                        self.checker
                            .sym(prop)
                            .name
                            .as_str()
                            .contains("EffectTypeId"),
                    )
                })
                .collect();
            keyed.sort_by(|(_, i_has), (_, j_has)| j_has.cmp(i_has));
            let candidates: Vec<SymbolId> = keyed.into_iter().map(|(prop, _)| prop).collect();

            // Try each candidate as a variance struct
            for prop in candidates {
                let prop_type = self.checker.get_type_of_symbol_at_location(prop, Node::NIL);
                if let Some(result) = self.parse_variance_struct(prop_type) {
                    break 'compute Some(result);
                }
            }

            None
        })
    }

    /// parseVarianceStruct extracts A, E, R from a variance struct type.
    pub fn parse_variance_struct(&mut self, t: TypeId) -> Option<Rc<Effect>> {
        let a = self.extract_covariant_type(t, "_A");
        if a.is_nil() {
            return None;
        }

        let e = self.extract_covariant_type(t, "_E");
        if e.is_nil() {
            return None;
        }

        let r = self.extract_covariant_type(t, "_R");
        if r.is_nil() {
            return None;
        }

        Some(Rc::new(Effect { a, e, r }))
    }

    /// IsEffectType returns true if the type has the Effect variance struct.
    pub fn is_effect_type(&mut self, t: TypeId) -> bool {
        self.effect_type(t).is_some()
    }

    /// StrictEffectType returns the parsed Effect type only if the type's symbol name
    /// is "Effect". This filters out types like Stream, Layer, HttpApp.Default that
    /// carry the variance struct but are not Effect itself.
    pub fn strict_effect_type(&mut self, t: TypeId) -> Option<Rc<Effect>> {
        if t.is_nil() {
            return None;
        }
        cached!(self, strict_effect_type, t, 'compute: {
            let sym = self.checker.ty(t).symbol;
            if sym.is_nil() || self.checker.sym(sym).name != *EFFECT_NAME {
                break 'compute None;
            }
            self.effect_type(t)
        })
    }

    /// StrictIsEffectType returns true if the type has the Effect variance struct
    /// AND the type's symbol name is "Effect". This filters out types like Stream,
    /// Layer, HttpApp.Default that carry the variance struct but are not Effect itself.
    pub fn strict_is_effect_type(&mut self, t: TypeId) -> bool {
        self.strict_effect_type(t).is_some()
    }

    /// EffectSubtype detects types that have the Effect variance struct AND a "_tag" or "get"
    /// marker property (e.g., Exit, Option, Either, Pool). Returns nil if not an Effect subtype.
    pub fn effect_subtype(&mut self, t: TypeId) -> Option<Rc<Effect>> {
        if t.is_nil() {
            return None;
        }
        cached!(self, effect_subtype, t, 'compute: {
            // Check for "_tag" or "get" property first (quick rejection)
            let tag_symbol = self.checker.get_property_of_type_exported(t, "_tag");
            let get_symbol = self.checker.get_property_of_type_exported(t, "get");
            if tag_symbol.is_nil() && get_symbol.is_nil() {
                break 'compute None;
            }
            // Must also be an Effect type
            self.effect_type(t)
        })
    }

    /// IsEffectSubtype returns true if the type is an Effect subtype (has variance struct + "_tag" or "get").
    pub fn is_effect_subtype(&mut self, t: TypeId) -> bool {
        self.effect_subtype(t).is_some()
    }

    /// FiberType detects types that have the Effect variance struct AND both "await" and "poll"
    /// properties. Returns nil if the type is not a Fiber.
    pub fn fiber_type(&mut self, t: TypeId) -> Option<Rc<Effect>> {
        if t.is_nil() {
            return None;
        }
        cached!(self, fiber_type, t, 'compute: {
            // Check for both "await" and "poll" properties (quick rejection)
            let await_symbol = self.checker.get_property_of_type_exported(t, "await");
            let poll_symbol = self.checker.get_property_of_type_exported(t, "poll");
            if await_symbol.is_nil() || poll_symbol.is_nil() {
                break 'compute None;
            }
            // Must also be an Effect type
            self.effect_type(t)
        })
    }

    /// IsFiberType returns true if the type is a Fiber type (has variance struct + "await" and "poll").
    pub fn is_fiber_type(&mut self, t: TypeId) -> bool {
        self.fiber_type(t).is_some()
    }

    /// HasEffectTypeId returns true if the type has the Effect type identifier.
    /// For v4, this is a quick check for the "~effect/Effect" property.
    /// For v3/unknown, this defers to IsEffectType since there is no single property shortcut.
    pub fn has_effect_type_id(&mut self, t: TypeId) -> bool {
        if t.is_nil() {
            return false;
        }
        cached!(self, has_effect_type_id, t, 'compute: {
            let version = self.detect_effect_version();
            if version == EffectMajorVersion::V4 {
                break 'compute self
                    .get_type_of_property_by_name(t, EFFECT_TYPE_ID)
                    .is_some();
            }
            // For v3/unknown, the quick check is not available; defer to full detection.
            self.is_effect_type(t)
        })
    }
}

// Go: isEffectTypeSourceFile
pub fn is_effect_type_source_file(tp: &mut TypeParser<'_>, sf: Node) -> bool {
    if sf.is_nil() {
        return false;
    }

    let module_sym = tp.checker.get_symbol_of_declaration(sf);
    if module_sym.is_nil() {
        return false;
    }

    let effect_sym = tp
        .checker
        .try_get_member_in_module_exports_and_properties("Effect", module_sym);
    if effect_sym.is_nil() {
        return false;
    }

    let effect_type = tp.checker.get_declared_type_of_symbol_exported(effect_sym);
    if effect_type.is_nil() {
        return false;
    }

    tp.effect_type(effect_type).is_some()
}

impl TypeParser<'_> {
    /// IsExpressionEffectModule reports whether node resolves to the Effect module namespace
    /// (e.g., the `Effect` in `import { Effect } from "effect"`).
    pub fn is_expression_effect_module(&mut self, node: Node) -> bool {
        self.is_node_reference_to_module(node, &EFFECT_MODULE_DESCRIPTOR)
    }

    /// IsNodeReferenceToEffectModuleApi reports whether node resolves to a member exported by the "effect" package.
    pub fn is_node_reference_to_effect_module_api(
        &mut self,
        node: Node,
        member: &ExportMember,
    ) -> bool {
        self.is_node_reference_to_module_export(node, &EFFECT_MODULE_DESCRIPTOR, member)
    }

    /// IsNodeReferenceToEffectPackageExport reports whether node resolves to a member
    /// exported by any module in the "effect" npm package. Unlike IsNodeReferenceToEffectModuleApi,
    /// this does not require the source file to export the Effect type — it only checks
    /// that the declaration lives inside the "effect" package and matches the named export.
    /// This is needed for functions like `pipe` which are exported from `effect/Function`.
    pub fn is_node_reference_to_effect_package_export(
        &mut self,
        node: Node,
        member: &ExportMember,
    ) -> bool {
        self.is_node_reference_to_module_export(node, &EFFECT_PACKAGE_EXPORT_DESCRIPTOR, member)
    }
}
