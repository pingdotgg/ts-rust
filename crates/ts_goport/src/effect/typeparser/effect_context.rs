// Go: internal/typeparser/effect_context.go

use crate::effect::typeparser::*;
use crate::prelude::*;

// Go: `1 << iota` starts at iota 1 here, so CanYieldEffect is 2.
crate::flags_macros::go_flags!(EffectContextFlags, u8 {
    NONE = 0; // EffectContextFlagNone
    CAN_YIELD_EFFECT = 1 << 1; // EffectContextFlagCanYieldEffect
    IN_EFFECT_CONSTRUCTOR_THUNK = 1 << 2; // EffectContextFlagInEffectConstructorThunk
    PENDING_NEXT_FUNCTION_IS_EFFECT_THUNK = 1 << 3; // EffectContextFlagPendingNextFunctionIsEffectThunk
    PENDING_NEXT_OBJECT_TRY_PROPERTY_IS_EFFECT_THUNK = 1 << 4; // EffectContextFlagPendingNextObjectTryPropertyIsEffectThunk
    IN_EFFECT = (1 << 1) | (1 << 2); // EffectContextFlagInEffect
});

impl TypeParser<'_> {
    pub fn get_effect_context_flags(&mut self, node: Node) -> EffectContextFlags {
        let Some(links) = self.ensure_effect_context_analyzed(node) else {
            return EffectContextFlags::NONE;
        };

        let (closest, ok) = get_closest_node_with_links(&links.effect_context_flags, node);
        if ok {
            return links
                .effect_context_flags
                .try_get(closest)
                .copied()
                .unwrap_or_default()
                & EffectContextFlags::IN_EFFECT;
        }
        EffectContextFlags::NONE
    }

    pub fn get_effect_yield_generator_function(&mut self, node: Node) -> Node {
        let Some(links) = self.ensure_effect_context_analyzed(node) else {
            return Node::NIL;
        };

        let (closest, ok) =
            get_closest_node_with_links(&links.effect_yield_generator_function, node);
        if ok {
            return links
                .effect_yield_generator_function
                .try_get(closest)
                .map_or(Node::NIL, |n| n.get());
        }
        Node::NIL
    }
}

/// Go `getClosestNodeWithLinks[T]` over a `core.LinkStore[*ast.Node, T]`.
pub fn get_closest_node_with_links<T: Default>(
    store: &LinkStore<Node, T>,
    node: Node,
) -> (Node, bool) {
    if node.is_nil() {
        return (Node::NIL, false);
    }

    let mut current = node;
    while current.is_some() {
        if store.has(current) {
            return (current, true);
        }
        current = current.parent();
    }

    (Node::NIL, false)
}

impl TypeParser<'_> {
    pub fn ensure_effect_context_analyzed(&mut self, node: Node) -> Option<&mut EffectLinks> {
        if node.is_nil() {
            return None;
        }

        if self.links().effect_context_flags.has(node) {
            return Some(self.links());
        }

        let sf = get_source_file_of_node(node);
        if sf.is_nil() {
            return None;
        }

        cached!(self, effect_context_analyzed, sf, {
            self.analyze_effect_context_for_source_file(sf);
            true
        });
        Some(self.links())
    }

    pub fn analyze_effect_context_for_source_file(&mut self, sf: Node) {
        if sf.is_nil() {
            return;
        }

        let pending_flags_mask = EffectContextFlags::PENDING_NEXT_FUNCTION_IS_EFFECT_THUNK
            | EffectContextFlags::PENDING_NEXT_OBJECT_TRY_PROPERTY_IS_EFFECT_THUNK;
        let mut state = EffectContextWalk {
            pending_enable_flags: FxHashMap::default(),
            pending_disable_flags: FxHashMap::default(),
            pending_flags_mask,
            function_scope_reset_flags: EffectContextFlags::CAN_YIELD_EFFECT
                | EffectContextFlags::IN_EFFECT_CONSTRUCTOR_THUNK
                | pending_flags_mask,
        };
        effect_context_walk(self, &mut state, sf);
    }
}

/// The locals of Go `analyzeEffectContextForSourceFile` that its closures
/// share. PORT: Go's `walk` closure is `effect_context_walk`, and the
/// closures that only touch the pending stores are methods here.
pub struct EffectContextWalk {
    pub pending_enable_flags: FxHashMap<Node, EffectContextFlags>,
    pub pending_disable_flags: FxHashMap<Node, EffectContextFlags>,
    pub pending_flags_mask: EffectContextFlags,
    pub function_scope_reset_flags: EffectContextFlags,
}

impl EffectContextWalk {
    // Go: setPendingEnableFlags
    pub fn set_pending_enable_flags(&mut self, node: Node, flags: EffectContextFlags) {
        if node.is_nil() || flags == EffectContextFlags::NONE {
            return;
        }
        *self.pending_enable_flags.entry(node).or_default() |= flags;
    }

    // Go: setPendingDisableFlags
    pub fn set_pending_disable_flags(&mut self, node: Node, flags: EffectContextFlags) {
        if node.is_nil() || flags == EffectContextFlags::NONE {
            return;
        }
        *self.pending_disable_flags.entry(node).or_default() |= flags;
    }

    // Go: resetChildFunctionScopeFlags
    pub fn reset_child_function_scope_flags(&mut self, node: Node) -> bool {
        let flags = self.function_scope_reset_flags;
        self.set_pending_disable_flags(node, flags);
        false
    }

    // Go: resetPendingFlags
    pub fn reset_pending_flags(&mut self, child: Node) -> bool {
        let flags = self.pending_flags_mask;
        self.set_pending_disable_flags(child, flags);
        false
    }
}

// Go: transparentPendingExpression
pub fn transparent_pending_expression(node: Node) -> Node {
    if node.is_nil() {
        return Node::NIL;
    }
    match node.kind() {
        SyntaxKind::ParenthesizedExpression
        | SyntaxKind::SatisfiesExpression
        | SyntaxKind::AsExpression
        | SyntaxKind::NonNullExpression
        | SyntaxKind::TypeAssertionExpression => node.expression(),
        _ => Node::NIL,
    }
}

// Go: the `walk` closure of analyzeEffectContextForSourceFile
pub fn effect_context_walk(tp: &mut TypeParser<'_>, w: &mut EffectContextWalk, node: Node) -> bool {
    if node.is_nil() {
        return false;
    }

    {
        let links = tp.links();
        if node.parent().is_some() {
            // inherit from parent, if any
            let parent_flags = *links.effect_context_flags.get(node.parent());
            *links.effect_context_flags.get(node) = parent_flags;
            if !links.effect_yield_generator_function.has(node) {
                let parent_generator = *links.effect_yield_generator_function.get(node.parent());
                *links.effect_yield_generator_function.get(node) = parent_generator;
            }
        } else {
            // default, no flags.
            *links.effect_context_flags.get(node) = EffectContextFlags::NONE;
        }

        // disable pending disable flags
        if let Some(&disable) = w.pending_disable_flags.get(&node) {
            let flags = links.effect_context_flags.get(node);
            *flags = flags.without(disable);
        }

        // merge pending state for this node
        if let Some(&enable) = w.pending_enable_flags.get(&node) {
            *links.effect_context_flags.get(node) |= enable;
        }
    }

    if tp
        .links()
        .effect_context_flags
        .get(node)
        .intersects(EffectContextFlags::PENDING_NEXT_FUNCTION_IS_EFFECT_THUNK)
        && (node.kind() == SyntaxKind::ArrowFunction
            || node.kind() == SyntaxKind::FunctionExpression)
    {
        let body = node.body();
        if body.is_some() {
            w.set_pending_enable_flags(body, EffectContextFlags::IN_EFFECT_CONSTRUCTOR_THUNK);
            let mask = w.pending_flags_mask;
            w.set_pending_disable_flags(body, mask);
        }
    } else if tp
        .links()
        .effect_context_flags
        .get(node)
        .intersects(EffectContextFlags::PENDING_NEXT_OBJECT_TRY_PROPERTY_IS_EFFECT_THUNK)
        && node.kind() == SyntaxKind::ObjectLiteralExpression
    {
        node.for_each_child(|child| w.reset_pending_flags(child));

        let obj = node;
        for prop in obj.properties().iter() {
            if prop.is_nil() || prop.kind() != SyntaxKind::PropertyAssignment {
                continue;
            }
            let assignment = prop;
            if assignment.name().is_nil() || assignment.initializer().is_nil() {
                continue;
            }
            if assignment.name().text() != "try" {
                continue;
            }
            w.set_pending_enable_flags(
                assignment.initializer(),
                EffectContextFlags::PENDING_NEXT_FUNCTION_IS_EFFECT_THUNK,
            );
        }
    } else if transparent_pending_expression(node).is_some() {
        let expr = transparent_pending_expression(node);
        let flags = *tp.links().effect_context_flags.get(node) & w.pending_flags_mask;
        w.set_pending_enable_flags(expr, flags);
    } else if tp
        .links()
        .effect_context_flags
        .get(node)
        .intersects(w.pending_flags_mask)
    {
        node.for_each_child(|child| w.reset_pending_flags(child));
    }

    // logic for this node
    if let Some(effect_gen) = tp.effect_gen_call(node) {
        let body_node = effect_gen.body;
        w.set_pending_enable_flags(body_node, EffectContextFlags::CAN_YIELD_EFFECT);
        *tp.links().effect_yield_generator_function.get(body_node) =
            CachedNode::new(effect_gen.generator_function);
    } else if let Some(effect_fn) = tp.effect_fn_call(node)
        && effect_fn.is_generator()
    {
        let body = effect_fn.body();
        let gen_fn = effect_fn.generator_function();
        if body.is_some() && gen_fn.is_some() {
            let body_node = body;
            w.set_pending_enable_flags(body_node, EffectContextFlags::CAN_YIELD_EFFECT);
            *tp.links().effect_yield_generator_function.get(body_node) = CachedNode::new(gen_fn);
        }
    }

    if node.kind() == SyntaxKind::CallExpression {
        let call = node;
        let arguments = call.arguments();
        if !arguments.is_empty() {
            let effect_thunk_arg = arguments.get(0);
            let callee = call.expression();
            if tp.is_node_reference_to_effect_module_api(callee, "sync")
                || tp.is_node_reference_to_effect_module_api(callee, "promise")
                || tp.is_node_reference_to_effect_module_api(callee, "callback")
                || tp.is_node_reference_to_effect_module_api(callee, "suspend")
            {
                w.set_pending_enable_flags(
                    effect_thunk_arg,
                    EffectContextFlags::PENDING_NEXT_FUNCTION_IS_EFFECT_THUNK,
                );
            } else if tp.is_node_reference_to_effect_module_api(callee, "try")
                || tp.is_node_reference_to_effect_module_api(callee, "tryPromise")
            {
                w.set_pending_enable_flags(
                    effect_thunk_arg,
                    EffectContextFlags::PENDING_NEXT_FUNCTION_IS_EFFECT_THUNK
                        | EffectContextFlags::PENDING_NEXT_OBJECT_TRY_PROPERTY_IS_EFFECT_THUNK,
                );
            }
        }
    }

    // Function-like nodes create a new scope, so they should not directly inherit
    // yieldability from an outer Effect scope. Matching Effect helpers re-enable the
    // flag on the specific body node below.
    if is_function_like(node) {
        node.for_each_child(|child| w.reset_child_function_scope_flags(child));
    }

    // reset stores correlated to a flag set here.
    if !tp
        .links()
        .effect_context_flags
        .get(node)
        .intersects(EffectContextFlags::CAN_YIELD_EFFECT)
    {
        *tp.links().effect_yield_generator_function.get(node) = CachedNode::default();
    }

    node.for_each_child(|child| effect_context_walk(tp, w, child));
    false
}
