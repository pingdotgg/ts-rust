// Go: internal/typeparser/extends_effect_model_class.go

use crate::effect::typeparser::*;
use crate::prelude::*;

/// EffectModelClassResult holds the parsed result of a class extending Model.Class
/// from effect/schema.
#[derive(Clone, Debug)]
pub struct EffectModelClassResult {
    /// The class name identifier
    pub class_name: Node,
    /// The Self type argument node (first type arg of the inner call)
    pub self_type_node: Node,
}

impl TypeParser<'_> {
    // Go: typeparser/extends_effect_model_class.go ExtendsEffectModelClass
    /// ExtendsEffectModelClass checks if a class declaration extends Model.Class<Self>(...)({...})
    /// from the effect/schema module.
    /// It detects the double-call pattern:
    ///
    /// ```text
    /// class X extends Model.Class<X>("name")({}) {}
    /// ```
    ///
    /// where the ExpressionWithTypeArguments.expression is a CallExpression (outer call)
    /// whose own .expression is also a CallExpression (inner call) with type arguments,
    /// and the inner call resolves to the effect Model.Class.
    pub fn extends_effect_model_class(
        &mut self,
        class_node: Node,
    ) -> Option<Rc<EffectModelClassResult>> {
        if class_node.is_nil() {
            return None;
        }

        cached!(self, extends_effect_model_class, class_node, 'compute: {
            if class_node.name().is_nil() {
                break 'compute None;
            }

            let heritage_elements = get_extends_heritage_clause_elements(class_node);
            if heritage_elements.is_empty() {
                break 'compute None;
            }

            for element in heritage_elements {
                if element.is_nil() {
                    continue;
                }

                let ewta = element;
                if ewta.is_nil() || ewta.expression().is_nil() {
                    continue;
                }

                let outer_call_node = ewta.expression();
                if !is_call_expression(outer_call_node) {
                    continue;
                }
                let outer_call = outer_call_node;
                if outer_call.is_nil() {
                    continue;
                }

                let inner_call_node = outer_call.expression();
                if inner_call_node.is_nil() || !is_call_expression(inner_call_node) {
                    continue;
                }
                let inner_call = inner_call_node;
                if inner_call.is_nil() {
                    continue;
                }

                let inner_type_arguments = inner_call.type_argument_list();
                if inner_type_arguments.is_nil() || inner_type_arguments.nodes().is_empty() {
                    continue;
                }

                if inner_call.expression().is_nil() {
                    continue;
                }
                if !self
                    .is_node_reference_to_effect_model_module_api(inner_call.expression(), "Class")
                {
                    continue;
                }

                break 'compute Some(Rc::new(EffectModelClassResult {
                    class_name: class_node.name(),
                    self_type_node: inner_type_arguments.nodes().get(0),
                }));
            }

            None
        })
    }
}
