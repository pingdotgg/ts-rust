// Go: internal/typeparser/context_tag.go

use crate::effect::typeparser::*;
use crate::prelude::*;
use std::cmp::Ordering;

impl TypeParser<'_> {
    // Go: typeparser/context_tag.go ContextTag
    /// ContextTag parses a v3 Context.Tag type and extracts Identifier, Shape parameters.
    /// Returns nil if the type is not a v3 Context.Tag.
    pub fn context_tag(&mut self, t: TypeId) -> Option<Rc<Service>> {
        if t.is_nil() {
            return None;
        }
        cached!(self, context_tag, t, 'compute: {
            if self.detect_effect_version() != EffectMajorVersion::V3 {
                break 'compute None;
            }
            if !self.is_pipeable_type(t) {
                break 'compute None;
            }

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

            // Go sort.SliceStable with less(i, j) = iHas && !jHas.
            let c = &*self.checker;
            candidates.sort_by(|&i, &j| {
                let i_has = c.sym(i).name.as_str().contains("TypeId");
                let j_has = c.sym(j).name.as_str().contains("TypeId");
                if i_has && !j_has {
                    Ordering::Less
                } else if j_has && !i_has {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            });

            for prop in candidates {
                let prop_type = self.checker.get_type_of_symbol_at_location(prop, Node::NIL);
                if let Some(result) = self.parse_service_variance_struct(prop_type) {
                    break 'compute Some(result);
                }
            }

            None
        })
    }

    // Go: typeparser/context_tag.go IsContextTag
    /// IsContextTag returns true if the type has the Context.Tag variance struct.
    pub fn is_context_tag(&mut self, t: TypeId) -> bool {
        self.context_tag(t).is_some()
    }
}
