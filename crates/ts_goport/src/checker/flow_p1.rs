//! Port of `checker/flow.go` lines 1-952: flow state, the flow graph walk
//! (`getTypeAtFlowNode`) and the narrowing functions up to
//! `getNarrowedTypeWorker`.

use crate::prelude::*;
use std::borrow::Cow;
use std::sync::LazyLock;

// Go: checker/flow.go:19 FlowType
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowType {
    pub t: TypeId,
    pub incomplete: bool,
}

impl FlowType {
    // Go: checker/flow.go:24 isNil
    #[must_use]
    pub fn is_nil(&self) -> bool {
        self.t.is_nil()
    }
}

impl Checker {
    // Go: checker/flow.go:28 newFlowType
    pub fn new_flow_type(&mut self, t: TypeId, incomplete: bool) -> FlowType {
        let mut t = t;
        if incomplete && self.ty(t).flags.intersects(TypeFlags::NEVER) {
            t = self.silent_never_type;
        }
        FlowType { t, incomplete }
    }
}

// Go: checker/flow.go:35 SharedFlow
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedFlow {
    pub flow: FlowNodeId,
    pub flow_type: FlowType,
}

/// Go `*ast.FlowReduceLabelData` on the flow state: the target label and
/// the temporary antecedent list of a reduce label flow node.
// PERF: the parts come from the flow node's `antecedents`
// (`[target, antecedents...]`, see `FlowNode::as_flow_reduce_label_data`).
// The list is a copy: a flow node of a freeable file version (lsshells M3b)
// is not `'static`, so the flow state cannot borrow it. Reduce labels come
// only from `finally` blocks; `get_branch_label_antecedents` still borrows
// the list of an ordinary branch label (perf14).
#[derive(Clone, Debug)]
pub struct ReduceLabel {
    pub target: FlowNodeId,
    pub antecedents: Box<[FlowNodeId]>,
}

impl ReduceLabel {
    /// Go `flow.Node.AsFlowReduceLabelData()` of a reduce label flow node.
    #[must_use]
    pub fn of(flow_data: &FlowNode) -> Self {
        assert!(
            flow_data.flags.intersects(FlowFlags::REDUCE_LABEL),
            "not a reduce label flow node"
        );
        Self {
            target: flow_data.antecedents[0],
            antecedents: flow_data.antecedents[1..].into(),
        }
    }
}

// Go: checker/flow.go:40 FlowState
// PORT: Go `*FlowState` is `&mut FlowState`. Go `getFlowState` (flow.go:52)
// and `putFlowState` (:61) keep a free list so that a walk does not
// allocate its state on the heap; here each walk keeps its state on the
// stack (`flow_walk`, `is_reachable_flow_node`, `is_post_super_flow_node`),
// which starts as Go's reset state, so there is no free list and no `next`.
// Go `[]*ast.FlowReduceLabelData` is a list of `ReduceLabel`.
// PERF: flowcost1. The `Rc<RefCell<FlowState>>` before it paid a borrow
// flag test at each field read of each flow step and the free list moves.
#[derive(Clone, Debug)]
pub struct FlowState {
    pub reference: Node,
    /// PERF: flowcost1. Not in Go. `reference.kind()`, read once per walk
    /// (`flow_walk`) for the kind tests of the reference in each step
    /// (`is_matching_flow_reference`). `Unknown` for a nil reference.
    pub reference_kind: SyntaxKind,
    pub declared_type: TypeId,
    pub initial_type: TypeId,
    pub flow_container: Node,
    pub ref_key: CacheHashKey,
    pub depth: i32,
    pub shared_flow_start: i32,
    pub reduce_labels: Vec<ReduceLabel>,
    /// PERF: flowcost1. Not in Go. The flow nodes of the static file of the
    /// last step (`flow_data`, `static_file_flow_nodes`) and that file's
    /// index + 1 (0: none yet). A
    /// walk stays in one file, so most steps read their flow node with one
    /// compare and one index, as Go reads `*flow`.
    pub flow_nodes: &'static [FlowNode],
    pub flow_nodes_file: usize,
}

/// All flow nodes of the file of `flow` when that file is in a static
/// publish, indexed by `local_index`; `None` for any other flow node
/// (synthetic, a freeable file version), as the static path of
/// `FlowNodeId::get_flow_in`.
#[inline]
fn static_file_flow_nodes(flow: FlowNodeId) -> Option<&'static [FlowNode]> {
    assert!(flow.is_some(), "nil flow node dereference");
    let g = crate::ast::static_go_file(flow.file_index())?;
    Some(
        g.flow_nodes
            .get()
            .expect("flow nodes are not built for this file"),
    )
}

impl Default for FlowState {
    /// Go `FlowState{}`.
    fn default() -> Self {
        Self {
            reference: Node::NIL,
            reference_kind: SyntaxKind::Unknown,
            declared_type: TypeId::NIL,
            initial_type: TypeId::NIL,
            flow_container: Node::NIL,
            ref_key: CacheHashKey::default(),
            depth: 0,
            shared_flow_start: 0,
            reduce_labels: Vec::new(),
            flow_nodes: &[],
            flow_nodes_file: 0,
        }
    }
}

impl FlowState {
    /// Go `*flow` for a step of this walk. A flow node of a static file is
    /// read from the cached slice of its file; any other flow node (synthetic,
    /// a freeable file version) through `guard`, as `FlowNodeId::get_flow_in`.
    #[inline]
    pub fn flow_data<'g>(
        &mut self,
        flow: FlowNodeId,
        guard: &'g mut Option<FileRef<FlowNode>>,
    ) -> &'g FlowNode {
        let nodes = self.flow_nodes;
        // `checked_add`: the synthetic flow file is 0xffff_ffff, and on a
        // 32-bit target `+ 1` wraps to 0, the "no file" marker.
        if flow.file_index().checked_add(1) == Some(self.flow_nodes_file) {
            return &nodes[flow.local_index()];
        }
        self.flow_data_slow(flow, guard)
    }

    #[inline(never)]
    fn flow_data_slow<'g>(
        &mut self,
        flow: FlowNodeId,
        guard: &'g mut Option<FileRef<FlowNode>>,
    ) -> &'g FlowNode {
        match static_file_flow_nodes(flow) {
            Some(nodes) => {
                self.flow_nodes = nodes;
                self.flow_nodes_file = flow.file_index().wrapping_add(1);
                &nodes[flow.local_index()]
            }
            None => flow.get_flow_in(guard),
        }
    }
}

// Go: checker/flow.go:69 getFlowNodeOfNode
pub fn get_flow_node_of_node(node: Node) -> FlowNodeId {
    // PORT: Go `node.FlowNodeData()` is nil for kinds without flow data; the
    // binder data then has a nil flow node, which is the same result.
    node.flow_node()
}

impl Checker {
    // Go: checker/flow.go:77 getFlowTypeOfReference
    pub fn get_flow_type_of_reference(&mut self, reference: Node, declared_type: TypeId) -> TypeId {
        self.get_flow_type_of_reference_ex(
            reference,
            declared_type,
            declared_type,
            Node::NIL,
            FlowNodeId::NIL,
        )
    }

    // Go: checker/flow.go:81 getFlowTypeOfReferenceEx
    pub fn get_flow_type_of_reference_ex(
        &mut self,
        reference: Node,
        declared_type: TypeId,
        initial_type: TypeId,
        flow_container: Node,
        flow_node: FlowNodeId,
    ) -> TypeId {
        if self.flow_analysis_disabled {
            return self.error_type;
        }
        let explicit_flow_node = flow_node.is_some();
        let mut flow_node = flow_node;
        if flow_node.is_nil() {
            flow_node = get_flow_node_of_node(reference);
            if flow_node.is_nil() {
                return declared_type;
            }
        }
        self.flow_invocation_count += 1;
        // flowskip1: a walk that can only return the declared type and
        // makes nothing is skipped (flow_skip.rs). P1: no explicit flow node.
        // P3: declared == initial, not auto (go-model.md 2.4).
        let evolved_type = if self.flow_skip.mode != FlowSkipMode::Off
            && !explicit_flow_node
            && (initial_type.is_nil() || initial_type == declared_type)
            && declared_type != self.auto_type
            && declared_type != self.auto_array_type
            && let Some(nest) =
                self.flow_skip_test(reference, declared_type, flow_container, flow_node)
        {
            if self.flow_skip.mode == FlowSkipMode::Verify {
                self.flow_skip_verify(reference, declared_type, flow_container, flow_node, nest)
            } else {
                declared_type
            }
        } else {
            self.flow_walk(
                reference,
                declared_type,
                initial_type,
                flow_container,
                flow_node,
            )
        };
        // When the reference is 'x' in an 'x.length', 'x.push(value)', 'x.unshift(value)' or x[n] = value' operation,
        // we give type 'any[]' to 'x' instead of using the type determined by control flow analysis such that operations
        // on empty arrays are possible without implicit any errors and new element types can be inferred without
        // type mismatch errors.
        let result_type = if self
            .ty(evolved_type)
            .object_flags
            .intersects(ObjectFlags::EVOLVING_ARRAY)
            && self.is_evolving_array_operation_target(reference)
        {
            self.auto_array_type
        } else {
            self.finalize_evolving_array_type(evolved_type)
        };
        // PERF: chkA. The parent of `reference` and its kind are read once.
        if result_type == self.unreachable_never_type
            || node_parent_and_kind(reference).1 == SyntaxKind::NonNullExpression
                && !self.ty(result_type).flags.intersects(TypeFlags::NEVER)
                && {
                    let with_facts =
                        self.get_type_with_facts(result_type, TypeFacts::NE_UNDEFINED_OR_NULL);
                    self.ty(with_facts).flags.intersects(TypeFlags::NEVER)
                }
        {
            return declared_type;
        }
        result_type
    }

    /// The walk of `get_flow_type_of_reference_ex` (flow.go:87-99 without
    /// the count): a flow state, `getTypeAtFlowNode`, and the release of
    /// the state and of the shared flow entries of the walk.
    pub(crate) fn flow_walk(
        &mut self,
        reference: Node,
        declared_type: TypeId,
        initial_type: TypeId,
        flow_container: Node,
        flow_node: FlowNodeId,
    ) -> TypeId {
        let mut f = FlowState {
            reference,
            reference_kind: reference.kind(),
            declared_type,
            initial_type: if initial_type.is_some() {
                initial_type
            } else {
                declared_type
            },
            flow_container,
            shared_flow_start: self.shared_flows.len() as i32,
            ..FlowState::default()
        };
        let evolved_type = self.get_type_at_flow_node(&mut f, flow_node).t;
        self.shared_flows.truncate(f.shared_flow_start as usize);
        evolved_type
    }

    // Go: checker/flow.go:117 getTypeAtFlowNode
    pub fn get_type_at_flow_node(&mut self, f: &mut FlowState, flow: FlowNodeId) -> FlowType {
        let mut flow = flow;
        if f.depth == 2000 {
            // We have made 2000 recursive invocations. To avoid overflowing the call stack we report an error
            // and disable further control flow analysis in the containing function or module body.
            if let Some(tr) = self.tracer {
                let depth = f.depth;
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "getTypeAtFlowNode_DepthLimit",
                    vec![("depth", depth.into())],
                );
            }
            self.flow_analysis_disabled = true;
            let reference = f.reference;
            self.report_flow_control_error(reference);
            return FlowType {
                t: self.error_type,
                incomplete: false,
            };
        }
        f.depth += 1;
        let mut shared_flow = FlowNodeId::NIL;
        // PERF: lsshells M3b. No guard for a static file (`get_flow_in`).
        // lsshells M3 repair: one guard for the whole walk, so the steps in
        // one freeable file version share one pin (`get_flow_in`).
        // flowcost1: a static file's flow nodes come from the walk's cache
        // (`FlowState::flow_data`).
        let mut flow_data_guard = None;
        // flowskip1 verify mode: record each loop turn (read once per call;
        // a nested verify restores the recording before it returns).
        let recording = self.flow_skip.recording.is_some();
        loop {
            if recording {
                let depth = f.depth;
                self.flow_skip_record(flow, depth);
            }
            let flow_data = f.flow_data(flow, &mut flow_data_guard);
            let flags = flow_data.flags;
            if flags.intersects(FlowFlags::SHARED) {
                // We cache results of flow type resolution for shared nodes that were previously visited in
                // the same getFlowTypeOfReference invocation. A node is considered shared when it is the
                // antecedent of more than one node.
                let start = f.shared_flow_start as usize;
                for i in start..self.shared_flows.len() {
                    if self.shared_flows[i].flow == flow {
                        f.depth -= 1;
                        return self.shared_flows[i].flow_type;
                    }
                }
                shared_flow = flow;
            }
            let t: FlowType;
            if flags.intersects(FlowFlags::ASSIGNMENT) {
                t = self.get_type_at_flow_assignment(f, flow, flow_data);
                if t.is_nil() {
                    flow = flow_data.antecedent;
                    continue;
                }
            } else if flags.intersects(FlowFlags::CALL) {
                t = self.get_type_at_flow_call(f, flow_data);
                if t.is_nil() {
                    flow = flow_data.antecedent;
                    continue;
                }
            } else if flags.intersects(FlowFlags::CONDITION) {
                t = self.get_type_at_flow_condition(f, flow_data);
            } else if flags.intersects(FlowFlags::SWITCH_CLAUSE) {
                t = self.get_type_at_switch_clause(f, flow_data);
            } else if flags.intersects(FlowFlags::BRANCH_LABEL) {
                let antecedents = get_branch_label_antecedents(flow, flow_data, &f.reduce_labels);
                // PORT: Go `antecedents.Next == nil` (a one-element FlowList).
                if antecedents.len() <= 1 {
                    flow = antecedents[0];
                    continue;
                }
                t = self.get_type_at_flow_branch_label(f, &antecedents);
            } else if flags.intersects(FlowFlags::LOOP_LABEL) {
                if flow_data.antecedents.len() <= 1 {
                    flow = flow_data.antecedents[0];
                    continue;
                }
                t = self.get_type_at_flow_loop_label(f, flow, flow_data);
            } else if flags.intersects(FlowFlags::ARRAY_MUTATION) {
                t = self.get_type_at_flow_array_mutation(f, flow_data);
                if t.is_nil() {
                    flow = flow_data.antecedent;
                    continue;
                }
            } else if flags.intersects(FlowFlags::REDUCE_LABEL) {
                f.reduce_labels.push(ReduceLabel::of(flow_data));
                t = self.get_type_at_flow_node(f, flow_data.antecedent);
                f.reduce_labels.pop();
            } else if flags.intersects(FlowFlags::START) {
                // Check if we should continue with the control flow of the containing function.
                let container = flow_data.node;
                let (reference_kind, flow_container, initial_type) =
                    (f.reference_kind, f.flow_container, f.initial_type);
                // PERF: Go `IsPropertyAccessExpression`,
                // `IsElementAccessExpression` and the `KindThisKeyword` test
                // on the reference kind of the walk (`FlowState::reference_kind`).
                if container.is_some() && container != flow_container && {
                    reference_kind != SyntaxKind::PropertyAccessExpression
                        && reference_kind != SyntaxKind::ElementAccessExpression
                        && !(reference_kind == SyntaxKind::ThisKeyword
                            && !is_arrow_function(container))
                } {
                    flow = container.flow_node();
                    continue;
                }
                // At the top of the flow we have the initial type.
                t = FlowType {
                    t: initial_type,
                    incomplete: false,
                };
            } else {
                // Unreachable code errors are reported in the binding phase. Here we
                // simply return the non-auto declared type to reduce follow-on errors.
                let declared_type = f.declared_type;
                t = FlowType {
                    t: self.convert_auto_to_any(declared_type),
                    incomplete: false,
                };
            }
            if shared_flow.is_some() {
                // Record visited node and the associated type in the cache.
                self.shared_flows.push(SharedFlow {
                    flow: shared_flow,
                    flow_type: t,
                });
            }
            f.depth -= 1;
            return t;
        }
    }
}

// Go: checker/flow.go:208 getBranchLabelAntecedents
// PORT: Go returns the `*ast.FlowList`; here it is the antecedent slice of a
// flow node (see `core::FlowNode::antecedents` and `ReduceLabel`): borrowed
// from `flow_data`, or a copy of a reduce label's list (the caller changes
// the flow state while it walks the list).
// `flow_data` is `flow.get_flow()`, which the caller has read.
pub fn get_branch_label_antecedents<'a>(
    flow: FlowNodeId,
    flow_data: &'a FlowNode,
    reduce_labels: &[ReduceLabel],
) -> Cow<'a, [FlowNodeId]> {
    let mut i = reduce_labels.len();
    while i != 0 {
        i -= 1;
        let data = &reduce_labels[i];
        if data.target == flow {
            return Cow::Owned(data.antecedents.to_vec());
        }
    }
    Cow::Borrowed(&flow_data.antecedents)
}

impl Checker {
    // Go: checker/flow.go:220 getTypeAtFlowAssignment
    // PERF: `flow_data` is `flow.get_flow()`, which the caller has read
    // (perf14), so a freeable file version is not pinned again (lsshells M3
    // repair).
    pub fn get_type_at_flow_assignment(
        &mut self,
        f: &mut FlowState,
        flow: FlowNodeId,
        flow_data: &FlowNode,
    ) -> FlowType {
        let node = flow_data.node;
        let (reference, reference_kind, declared_type) =
            (f.reference, f.reference_kind, f.declared_type);
        // PERF: chkA. The kind of `node` is read once, for the match and the
        // `for ... in` test below. noderef1 G2: through a view (`NodeRef`),
        // which the `for ... in` test reads the grandparent with.
        let node_ref = NodeRef::new(node);
        let node_kind = node_ref.kind();
        // Assignments only narrow the computed type if the declared type is a union type. Thus, we
        // only need to evaluate the assigned type if the declared type is a union type.
        // PERF: this8000. See synthetic_access_memo_says_no.
        if !self.matching_reference_memo_says_no(reference, node, node_kind)
            && !self.synthetic_access_memo_says_no(reference, reference_kind, node, node_kind)
            && self.is_matching_reference_kinds(reference, reference_kind, node, node_kind)
        {
            if !self.is_reachable_flow_node(flow) {
                return FlowType {
                    t: self.unreachable_never_type,
                    incomplete: false,
                };
            }
            if get_assignment_target_kind(node) == AssignmentKind::COMPOUND {
                let flow_type = self.get_type_at_flow_node(f, flow_data.antecedent);
                let base = self.get_base_type_of_literal_type(flow_type.t);
                return self.new_flow_type(base, flow_type.incomplete);
            }
            if declared_type == self.auto_type || declared_type == self.auto_array_type {
                if self.is_empty_array_assignment(node) {
                    let never_type = self.never_type;
                    return FlowType {
                        t: self.get_evolving_array_type(never_type),
                        incomplete: false,
                    };
                }
                let initial_or_assigned = self.get_initial_or_assigned_type(f, node);
                let assigned_type = self.get_widened_literal_type(initial_or_assigned);
                if self.is_type_assignable_to(assigned_type, declared_type) {
                    return FlowType {
                        t: assigned_type,
                        incomplete: false,
                    };
                }
                return FlowType {
                    t: self.any_array_type,
                    incomplete: false,
                };
            }
            let mut t = declared_type;
            if is_in_compound_like_assignment(node) {
                t = self.get_base_type_of_literal_type(t);
            }
            if self.ty(t).flags.intersects(TypeFlags::UNION) {
                let assigned = self.get_initial_or_assigned_type(f, node);
                return FlowType {
                    t: self.get_assignment_reduced_type(t, assigned),
                    incomplete: false,
                };
            }
            return FlowType {
                t,
                incomplete: false,
            };
        }
        // We didn't have a direct match. However, if the reference is a dotted name, this
        // may be an assignment to a left hand part of the reference. For example, for a
        // reference 'x.y.z', we may be at an assignment to 'x.y' or 'x'. In that case,
        // return the declared type.
        // PERF: chkA. `contains_matching_reference` is false at once for a
        // reference that is not an access expression (its kind is read once
        // per walk).
        if matches!(
            reference_kind,
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression
        ) && self.contains_matching_reference(reference, node)
        {
            if !self.is_reachable_flow_node(flow) {
                return FlowType {
                    t: self.unreachable_never_type,
                    incomplete: false,
                };
            }
            // A matching dotted name might also be an expando property on a function *expression*,
            // in which case we continue control flow analysis back to the function's declaration
            if is_variable_declaration(node) && (is_in_js_file(node) || is_var_const_like(node)) {
                let init = node.initializer();
                if init.is_some() && is_function_expression_or_arrow_function(init) {
                    return self.get_type_at_flow_node(f, flow_data.antecedent);
                }
            }
            return FlowType {
                t: declared_type,
                incomplete: false,
            };
        }
        // for (const _ in ref) acts as a nonnull on ref
        if node_kind == SyntaxKind::VariableDeclaration && {
            let grand_parent = node_ref.parent().parent();
            grand_parent.kind() == SyntaxKind::ForInStatement
                && (self.is_matching_flow_reference(f, grand_parent.expression())
                    || self.optional_chain_contains_reference(grand_parent.expression(), reference))
        } {
            let antecedent_type = self.get_type_at_flow_node(f, flow_data.antecedent).t;
            let finalized = self.finalize_evolving_array_type(antecedent_type);
            return FlowType {
                t: self.get_non_nullable_type_if_needed(finalized),
                incomplete: false,
            };
        }
        // Assignment doesn't affect reference
        FlowType::default()
    }

    // Go: checker/flow.go:276 getInitialOrAssignedType
    // PORT: takes `flow.Node` (`node`), the only part of the flow node it reads.
    pub fn get_initial_or_assigned_type(&mut self, f: &mut FlowState, node: Node) -> TypeId {
        let reference = f.reference;
        if is_variable_declaration(node) || is_binding_element(node) {
            let initial_type = self.get_initial_type(node);
            return self.get_narrowable_type_for_reference(
                initial_type,
                reference,
                CheckMode::NORMAL,
            );
        }
        let assigned_type = self.get_assigned_type(node);
        self.get_narrowable_type_for_reference(assigned_type, reference, CheckMode::NORMAL)
    }

    // Go: checker/flow.go:283 isEmptyArrayAssignment
    pub fn is_empty_array_assignment(&self, node: Node) -> bool {
        is_variable_declaration(node)
            && node.initializer().is_some()
            && is_empty_array_literal(node.initializer())
            || !is_binding_element(node)
                && is_binary_expression(node.parent())
                && is_empty_array_literal(node.parent().right())
    }

    // Go: checker/flow.go:288 getTypeAtFlowCall
    // PERF: takes the flow node that the caller has read (`flow.get_flow()`).
    pub fn get_type_at_flow_call(&mut self, f: &mut FlowState, flow_data: &FlowNode) -> FlowType {
        let signature = self.get_effects_signature(flow_data.node);
        if signature.is_some() {
            let predicate = self.get_type_predicate_of_signature(signature);
            if predicate.is_some()
                && (self.pred(predicate).kind == TypePredicateKind::ASSERTS_THIS
                    || self.pred(predicate).kind == TypePredicateKind::ASSERTS_IDENTIFIER)
            {
                let flow_type = self.get_type_at_flow_node(f, flow_data.antecedent);
                let t = self.finalize_evolving_array_type(flow_type.t);
                let (pred_kind, pred_t, pred_parameter_index) = {
                    let p = self.pred(predicate);
                    (p.kind, p.t, p.parameter_index)
                };
                let arguments = flow_data.node.arguments();
                let narrowed_type = if pred_t.is_some() {
                    self.narrow_type_by_type_predicate(
                        f,
                        t,
                        predicate,
                        flow_data.node,
                        true, /*assumeTrue*/
                    )
                } else if pred_kind == TypePredicateKind::ASSERTS_IDENTIFIER
                    && pred_parameter_index >= 0
                    && (pred_parameter_index as usize) < arguments.len()
                {
                    self.narrow_type_by_assertion(
                        f,
                        t,
                        arguments.get(pred_parameter_index as usize),
                    )
                } else {
                    t
                };
                if narrowed_type == t {
                    return flow_type;
                }
                return self.new_flow_type(narrowed_type, flow_type.incomplete);
            }
            let return_type = self.get_return_type_of_signature(signature);
            if self.ty(return_type).flags.intersects(TypeFlags::NEVER) {
                return FlowType {
                    t: self.unreachable_never_type,
                    incomplete: false,
                };
            }
        }
        FlowType::default()
    }

    // Go: checker/flow.go:316 narrowTypeByTypePredicate
    pub fn narrow_type_by_type_predicate(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        predicate: TypePredicateId,
        call_expression: Node,
        assume_true: bool,
    ) -> TypeId {
        let mut t = t;
        let predicate_t = self.pred(predicate).t;
        // Don't narrow from 'any' if the predicate type is exactly 'Object' or 'Function'
        if predicate_t.is_some()
            && !(self.is_type_any(t)
                && (predicate_t == self.global_object_type
                    || predicate_t == self.global_function_type))
        {
            let predicate_argument = self.get_type_predicate_argument(predicate, call_expression);
            if predicate_argument.is_some() {
                let reference = f.reference;
                if self.is_matching_flow_reference(f, predicate_argument) {
                    return self.get_narrowed_type(
                        t,
                        predicate_t,
                        assume_true,
                        false, /*checkDerived*/
                    );
                }
                if self.strict_null_checks
                    && self.optional_chain_contains_reference(predicate_argument, reference)
                    && (assume_true && !self.has_type_facts(predicate_t, TypeFacts::EQ_UNDEFINED)
                        || !assume_true
                            && self.every_type(predicate_t, &mut |c, t| c.is_nullable_type(t)))
                {
                    t = self.get_adjusted_type_with_facts(t, TypeFacts::NE_UNDEFINED_OR_NULL);
                }
                let access = self.get_discriminant_property_access(f, predicate_argument, t);
                if access.is_some() {
                    return self.narrow_type_by_discriminant(t, access, &mut |c, t| {
                        c.get_narrowed_type(
                            t,
                            predicate_t,
                            assume_true,
                            false, /*checkDerived*/
                        )
                    });
                }
            }
        }
        t
    }

    // Go: checker/flow.go:338 narrowTypeByAssertion
    pub fn narrow_type_by_assertion(&mut self, f: &mut FlowState, t: TypeId, expr: Node) -> TypeId {
        let node = skip_parentheses(expr);
        if node.kind() == SyntaxKind::FalseKeyword {
            return self.unreachable_never_type;
        }
        if node.kind() == SyntaxKind::BinaryExpression {
            if node.operator_token().kind() == SyntaxKind::AmpersandAmpersandToken {
                let left = self.narrow_type_by_assertion(f, t, node.left());
                return self.narrow_type_by_assertion(f, left, node.right());
            }
            if node.operator_token().kind() == SyntaxKind::BarBarToken {
                let left = self.narrow_type_by_assertion(f, t, node.left());
                let right = self.narrow_type_by_assertion(f, t, node.right());
                return self.get_union_type(&[left, right]);
            }
        }
        self.narrow_type(f, t, node, true /*assumeTrue*/)
    }

    // Go: checker/flow.go:354 getTypeAtFlowCondition
    // PERF: takes the flow node that the caller has read (`flow.get_flow()`).
    pub fn get_type_at_flow_condition(
        &mut self,
        f: &mut FlowState,
        flow_data: &FlowNode,
    ) -> FlowType {
        let flow_type = self.get_type_at_flow_node(f, flow_data.antecedent);
        if self.ty(flow_type.t).flags.intersects(TypeFlags::NEVER) {
            return flow_type;
        }
        // If we have an antecedent type (meaning we're reachable in some way), we first
        // attempt to narrow the antecedent type. If that produces the never type, and if
        // the antecedent type is incomplete (i.e. a transient type in a loop), then we
        // take the type guard as an indication that control *could* reach here once we
        // have the complete type. We proceed by switching to the silent never type which
        // doesn't report errors when operators are applied to it. Note that this is the
        // *only* place a silent never type is ever generated.
        let assume_true = flow_data.flags.intersects(FlowFlags::TRUE_CONDITION);
        let non_evolving_type = self.finalize_evolving_array_type(flow_type.t);
        let narrowed_type = self.narrow_type(f, non_evolving_type, flow_data.node, assume_true);
        if narrowed_type == non_evolving_type {
            return flow_type;
        }
        self.new_flow_type(narrowed_type, flow_type.incomplete)
    }

    // Go: checker/flow.go:377 narrowType
    // Narrow the given type based on the given expression having the assumed boolean value. The returned type
    // will be a subtype or the same type as the argument.
    pub fn narrow_type(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: Node,
        assume_true: bool,
    ) -> TypeId {
        // for `a?.b`, we emulate a synthetic `a !== null && a !== undefined` condition for `a`
        // PERF: chkA. The parent, its kind and its operator are read once
        // (`node_parent_and_kind`), and the kind of `expr` once.
        // flowcost1: Go `ast.IsExpressionOfOptionalChainRoot(expr)` on the
        // same read of the parent: `IsOptionalChainRoot` is an optional
        // chain flag, a property or element access or call kind (not a
        // non-null expression) and a question dot token.
        let (parent, parent_kind) = node_parent_and_kind(expr);
        if match parent_kind {
            SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::CallExpression => {
                !parent.parser_flags(NodeFlags::OPTIONAL_CHAIN).is_empty()
                    && parent.question_dot_token().is_some()
                    && parent.expression() == expr
            }
            SyntaxKind::BinaryExpression => {
                matches!(
                    parent.operator_token().kind(),
                    SyntaxKind::QuestionQuestionToken | SyntaxKind::QuestionQuestionEqualsToken
                ) && parent.left() == expr
            }
            _ => false,
        } {
            debug_assert!(
                is_expression_of_optional_chain_root(expr)
                    || parent_kind == SyntaxKind::BinaryExpression
            );
            return self.narrow_type_by_optionality(f, t, expr, assume_true);
        }
        debug_assert!(!is_expression_of_optional_chain_root(expr));
        let expr_kind = expr.kind();
        match expr_kind {
            SyntaxKind::Identifier
            | SyntaxKind::ThisKeyword
            | SyntaxKind::SuperKeyword
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression => {
                if expr_kind == SyntaxKind::Identifier {
                    // When narrowing a reference to a const variable, non-assigned parameter, or readonly property, we inline
                    // up to five levels of aliased conditional expressions that are themselves declared as const variables.
                    let reference = f.reference;
                    if !self.is_matching_flow_reference(f, expr) && self.inline_level < 5 {
                        let symbol = self.get_resolved_symbol(expr);
                        if self.is_constant_variable(symbol) {
                            let declaration = self.sym(symbol).value_declaration;
                            if declaration.is_some()
                                && is_variable_declaration(declaration)
                                && declaration.type_().is_nil()
                                && declaration.initializer().is_some()
                                && self.is_constant_reference(reference)
                            {
                                self.inline_level += 1;
                                let result =
                                    self.narrow_type(f, t, declaration.initializer(), assume_true);
                                self.inline_level -= 1;
                                return result;
                            }
                        }
                    }
                    // fallthrough
                }
                return self.narrow_type_by_truthiness(f, t, expr, assume_true);
            }
            SyntaxKind::CallExpression => {
                return self.narrow_type_by_call_expression(f, t, expr, assume_true);
            }
            SyntaxKind::ParenthesizedExpression
            | SyntaxKind::NonNullExpression
            | SyntaxKind::SatisfiesExpression => {
                return self.narrow_type(f, t, expr.expression(), assume_true);
            }
            SyntaxKind::BinaryExpression => {
                return self.narrow_type_by_binary_expression(f, t, expr, assume_true);
            }
            SyntaxKind::PrefixUnaryExpression => {
                if expr.operator() == SyntaxKind::ExclamationToken {
                    return self.narrow_type(f, t, expr.operand(), !assume_true);
                }
            }
            _ => {}
        }
        t
    }

    // Go: checker/flow.go:415 narrowTypeByOptionality
    pub fn narrow_type_by_optionality(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: Node,
        assume_present: bool,
    ) -> TypeId {
        let reference = f.reference;
        if self.is_matching_flow_reference(f, expr) {
            return self.get_adjusted_type_with_facts(
                t,
                if assume_present {
                    TypeFacts::NE_UNDEFINED_OR_NULL
                } else {
                    TypeFacts::EQ_UNDEFINED_OR_NULL
                },
            );
        }
        let access = self.get_discriminant_property_access(f, expr, t);
        if access.is_some() {
            return self.narrow_type_by_discriminant(t, access, &mut |c, t| {
                c.get_type_with_facts(
                    t,
                    if assume_present {
                        TypeFacts::NE_UNDEFINED_OR_NULL
                    } else {
                        TypeFacts::EQ_UNDEFINED_OR_NULL
                    },
                )
            });
        }
        t
    }

    // Go: checker/flow.go:428 narrowTypeByTruthiness
    pub fn narrow_type_by_truthiness(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: Node,
        assume_true: bool,
    ) -> TypeId {
        let mut t = t;
        let reference = f.reference;
        if self.is_matching_flow_reference(f, expr) {
            return self.get_adjusted_type_with_facts(
                t,
                if assume_true {
                    TypeFacts::TRUTHY
                } else {
                    TypeFacts::FALSY
                },
            );
        }
        if self.strict_null_checks
            && assume_true
            && self.optional_chain_contains_reference(expr, reference)
        {
            t = self.get_adjusted_type_with_facts(t, TypeFacts::NE_UNDEFINED_OR_NULL);
        }
        let access = self.get_discriminant_property_access(f, expr, t);
        if access.is_some() {
            return self.narrow_type_by_discriminant(t, access, &mut |c, t| {
                c.get_type_with_facts(
                    t,
                    if assume_true {
                        TypeFacts::TRUTHY
                    } else {
                        TypeFacts::FALSY
                    },
                )
            });
        }
        t
    }

    // Go: checker/flow.go:444 narrowTypeByCallExpression
    pub fn narrow_type_by_call_expression(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        call_expression: Node,
        assume_true: bool,
    ) -> TypeId {
        let reference = f.reference;
        if self.has_matching_argument(call_expression, reference) {
            let mut predicate = TypePredicateId::NIL;
            if assume_true || !is_call_chain(call_expression) {
                let signature = self.get_effects_signature(call_expression);
                if signature.is_some() {
                    predicate = self.get_type_predicate_of_signature(signature);
                }
            }
            if predicate.is_some()
                && (self.pred(predicate).kind == TypePredicateKind::THIS
                    || self.pred(predicate).kind == TypePredicateKind::IDENTIFIER)
            {
                return self.narrow_type_by_type_predicate(
                    f,
                    t,
                    predicate,
                    call_expression,
                    assume_true,
                );
            }
        }
        if self.contains_missing_type(t)
            && is_access_expression(reference)
            && is_property_access_expression(call_expression.expression())
        {
            let call_access = call_expression.expression();
            let candidate = self.get_reference_candidate(call_access.expression());
            if self.is_matching_reference(reference.expression(), candidate)
                && is_identifier(call_access.name())
                && call_access.name().text() == "hasOwnProperty"
                && call_expression.arguments().len() == 1
            {
                let argument = call_expression.arguments().get(0);
                let (accessed_name, ok) = self.get_accessed_property_name(reference);
                if ok && is_string_literal_like(argument) && accessed_name == argument.text() {
                    return self.get_type_with_facts(
                        t,
                        if assume_true {
                            TypeFacts::NE_UNDEFINED
                        } else {
                            TypeFacts::EQ_UNDEFINED
                        },
                    );
                }
            }
        }
        t
    }

    // Go: checker/flow.go:469 narrowTypeByBinaryExpression
    // PORT: Go takes `*ast.BinaryExpression`; this takes the node.
    pub fn narrow_type_by_binary_expression(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: Node,
        assume_true: bool,
    ) -> TypeId {
        let mut t = t;
        let reference = f.reference;
        // PERF: flowcost1. The operator kind is read once (Go reads the
        // `OperatorToken.Kind` field for the switch and again for `operator`).
        let operator = expr.operator_token().kind();
        match operator {
            SyntaxKind::EqualsToken
            | SyntaxKind::BarBarEqualsToken
            | SyntaxKind::AmpersandAmpersandEqualsToken
            | SyntaxKind::QuestionQuestionEqualsToken => {
                let narrowed = self.narrow_type(f, t, expr.right(), assume_true);
                return self.narrow_type_by_truthiness(f, narrowed, expr.left(), assume_true);
            }
            SyntaxKind::EqualsEqualsToken
            | SyntaxKind::ExclamationEqualsToken
            | SyntaxKind::EqualsEqualsEqualsToken
            | SyntaxKind::ExclamationEqualsEqualsToken => {
                let left = self.get_reference_candidate(expr.left());
                let right = self.get_reference_candidate(expr.right());
                // PERF: flowcost1. The kinds of `left` and `right` are read
                // once, for the `typeof` tests and the reference matches.
                let (left_kind, right_kind) = (left.kind(), right.kind());
                if left_kind == SyntaxKind::TypeOfExpression && is_string_literal_like(right) {
                    return self.narrow_type_by_typeof(f, t, left, operator, right, assume_true);
                }
                if right_kind == SyntaxKind::TypeOfExpression && is_string_literal_like(left) {
                    return self.narrow_type_by_typeof(f, t, right, operator, left, assume_true);
                }
                if self.is_matching_reference_kinds(reference, f.reference_kind, left, left_kind) {
                    return self.narrow_type_by_equality(t, operator, right, assume_true);
                }
                if self.is_matching_reference_kinds(reference, f.reference_kind, right, right_kind)
                {
                    return self.narrow_type_by_equality(t, operator, left, assume_true);
                }
                if self.strict_null_checks {
                    if self.optional_chain_contains_reference(left, reference) {
                        t = self.narrow_type_by_optional_chain_containment(
                            f,
                            t,
                            operator,
                            right,
                            assume_true,
                        );
                    } else if self.optional_chain_contains_reference(right, reference) {
                        t = self.narrow_type_by_optional_chain_containment(
                            f,
                            t,
                            operator,
                            left,
                            assume_true,
                        );
                    }
                }
                let left_access = self.get_discriminant_property_access(f, left, t);
                if left_access.is_some() {
                    return self.narrow_type_by_discriminant_property(
                        t,
                        left_access,
                        operator,
                        right,
                        assume_true,
                    );
                }
                let right_access = self.get_discriminant_property_access(f, right, t);
                if right_access.is_some() {
                    return self.narrow_type_by_discriminant_property(
                        t,
                        right_access,
                        operator,
                        left,
                        assume_true,
                    );
                }
                if self.is_matching_constructor_reference(f, left) {
                    return self.narrow_type_by_constructor(t, operator, right, assume_true);
                }
                if self.is_matching_constructor_reference(f, right) {
                    return self.narrow_type_by_constructor(t, operator, left, assume_true);
                }
                if is_boolean_literal(right) && !is_access_expression(left) {
                    return self.narrow_type_by_boolean_comparison(
                        f,
                        t,
                        left,
                        right,
                        operator,
                        assume_true,
                    );
                }
                if is_boolean_literal(left) && !is_access_expression(right) {
                    return self.narrow_type_by_boolean_comparison(
                        f,
                        t,
                        right,
                        left,
                        operator,
                        assume_true,
                    );
                }
            }
            SyntaxKind::InstanceOfKeyword => {
                return self.narrow_type_by_instanceof(f, t, expr, assume_true);
            }
            SyntaxKind::InKeyword => {
                if is_private_identifier(expr.left()) {
                    return self.narrow_type_by_private_identifier_in_in_expression(
                        f,
                        t,
                        expr,
                        assume_true,
                    );
                }
                let target = self.get_reference_candidate(expr.right());
                if self.contains_missing_type(t)
                    && is_access_expression(reference)
                    && self.is_matching_reference(reference.expression(), target)
                {
                    let left_type = self.get_type_of_expression(expr.left());
                    if self.is_type_usable_as_property_name(left_type) {
                        let (accessed_name, ok) = self.get_accessed_property_name(reference);
                        if ok && accessed_name == self.get_property_name_from_type(left_type) {
                            return self.get_type_with_facts(
                                t,
                                if assume_true {
                                    TypeFacts::NE_UNDEFINED
                                } else {
                                    TypeFacts::EQ_UNDEFINED
                                },
                            );
                        }
                    }
                }
                if self.is_matching_flow_reference(f, target) {
                    let left_type = self.get_type_of_expression(expr.left());
                    if self.is_type_usable_as_property_name(left_type) {
                        return self.narrow_type_by_in_keyword(f, t, left_type, assume_true);
                    }
                }
            }
            SyntaxKind::CommaToken => {
                return self.narrow_type(f, t, expr.right(), assume_true);
            }
            SyntaxKind::AmpersandAmpersandToken => {
                // Ordinarily we won't see && and || expressions in control flow analysis because the Binder breaks those
                // expressions down to individual conditional control flows. However, we may encounter them when analyzing
                // aliased conditional expressions.
                if assume_true {
                    let left = self.narrow_type(f, t, expr.left(), true /*assumeTrue*/);
                    return self.narrow_type(f, left, expr.right(), true /*assumeTrue*/);
                }
                let left = self.narrow_type(f, t, expr.left(), false /*assumeTrue*/);
                let right = self.narrow_type(f, t, expr.right(), false /*assumeTrue*/);
                return self.get_union_type(&[left, right]);
            }
            SyntaxKind::BarBarToken => {
                if assume_true {
                    let left = self.narrow_type(f, t, expr.left(), true /*assumeTrue*/);
                    let right = self.narrow_type(f, t, expr.right(), true /*assumeTrue*/);
                    return self.get_union_type(&[left, right]);
                }
                let left = self.narrow_type(f, t, expr.left(), false /*assumeTrue*/);
                return self.narrow_type(f, left, expr.right(), false /*assumeTrue*/);
            }
            _ => {}
        }
        t
    }
}

impl Checker {
    // Go: checker/flow.go:556 narrowTypeByEquality
    pub fn narrow_type_by_equality(
        &mut self,
        t: TypeId,
        operator: SyntaxKind,
        value: Node,
        assume_true: bool,
    ) -> TypeId {
        let mut assume_true = assume_true;
        if self.ty(t).flags.intersects(TypeFlags::ANY) {
            return t;
        }
        if operator == SyntaxKind::ExclamationEqualsToken
            || operator == SyntaxKind::ExclamationEqualsEqualsToken
        {
            assume_true = !assume_true;
        }
        let value_type = self.get_type_of_expression(value);
        let double_equals = operator == SyntaxKind::EqualsEqualsToken
            || operator == SyntaxKind::ExclamationEqualsToken;
        let value_flags = self.ty(value_type).flags;
        if value_flags.intersects(TypeFlags::NULLABLE) {
            if !self.strict_null_checks {
                return t;
            }
            let facts = if double_equals {
                if assume_true {
                    TypeFacts::EQ_UNDEFINED_OR_NULL
                } else {
                    TypeFacts::NE_UNDEFINED_OR_NULL
                }
            } else if value_flags.intersects(TypeFlags::NULL) {
                if assume_true {
                    TypeFacts::EQ_NULL
                } else {
                    TypeFacts::NE_NULL
                }
            } else if assume_true {
                TypeFacts::EQ_UNDEFINED
            } else {
                TypeFacts::NE_UNDEFINED
            };
            return self.get_adjusted_type_with_facts(t, facts);
        }
        if assume_true {
            if !double_equals
                && (self.ty(t).flags.intersects(TypeFlags::UNKNOWN)
                    || self.some_type(t, &mut |c, t| c.is_empty_anonymous_object_type(t)))
            {
                if value_flags.intersects(TypeFlags::PRIMITIVE | TypeFlags::NON_PRIMITIVE)
                    || self.is_empty_anonymous_object_type(value_type)
                {
                    return value_type;
                }
                if value_flags.intersects(TypeFlags::OBJECT) {
                    return self.non_primitive_type;
                }
            }
            if !double_equals
                && value_flags.intersects(TypeFlags::PRIMITIVE)
                && self.is_uniform_union_type(t)
            {
                let regular_type = self.get_regular_type_of_literal_type(value_type);
                if self.union_contains_type(t, regular_type, false /*matchSymbol*/) {
                    return regular_type;
                }
            }
            let filtered_type = self.filter_type(t, &mut |c, t| {
                c.are_types_comparable(t, value_type)
                    || double_equals && c.is_coercible_under_double_equals(t, value_type)
            });
            return self.replace_primitives_with_literals(filtered_type, value_type);
        }
        if self.is_unit_type(value_type) {
            if self.is_uniform_union_type(t) {
                let regular_type = self.get_regular_type_of_literal_type(value_type);
                let filtered_type = self.remove_type(t, regular_type);
                if filtered_type != t {
                    return filtered_type;
                }
            }
            return self.filter_type(t, &mut |c, t| {
                !(c.is_unit_like_type(t) && c.are_types_comparable(t, value_type))
            });
        }
        t
    }

    // Go: checker/flow.go:614 narrowTypeByTypeof
    // PORT: Go takes `*ast.TypeOfExpression`; this takes the node.
    pub fn narrow_type_by_typeof(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        type_of_expr: Node,
        operator: SyntaxKind,
        literal: Node,
        assume_true: bool,
    ) -> TypeId {
        let mut t = t;
        let mut assume_true = assume_true;
        // We have '==', '!=', '===', or !==' operator with 'typeof xxx' and string literal operands
        if operator == SyntaxKind::ExclamationEqualsToken
            || operator == SyntaxKind::ExclamationEqualsEqualsToken
        {
            assume_true = !assume_true;
        }
        let reference = f.reference;
        let target = self.get_reference_candidate(type_of_expr.expression());
        if !self.is_matching_flow_reference(f, target) {
            if self.strict_null_checks
                && self.optional_chain_contains_reference(target, reference)
                && assume_true == (literal.text() != "undefined")
            {
                t = self.get_adjusted_type_with_facts(t, TypeFacts::NE_UNDEFINED_OR_NULL);
            }
            let property_access = self.get_discriminant_property_access(f, target, t);
            if property_access.is_some() {
                return self.narrow_type_by_discriminant(t, property_access, &mut |c, t| {
                    c.narrow_type_by_literal_expression(t, literal, assume_true)
                });
            }
            return t;
        }
        self.narrow_type_by_literal_expression(t, literal, assume_true)
    }
}

// Go: checker/flow.go:635 typeofNEFacts
// PORT: Go package map var; read with `TYPEOF_NE_FACTS.get(name)`.
pub static TYPEOF_NE_FACTS: LazyLock<FxHashMap<&'static str, TypeFacts>> = LazyLock::new(|| {
    let mut m = FxHashMap::default();
    m.insert("string", TypeFacts::TYPEOF_NE_STRING);
    m.insert("number", TypeFacts::TYPEOF_NE_NUMBER);
    m.insert("bigint", TypeFacts::TYPEOF_NE_BIG_INT);
    m.insert("boolean", TypeFacts::TYPEOF_NE_BOOLEAN);
    m.insert("symbol", TypeFacts::TYPEOF_NE_SYMBOL);
    m.insert("undefined", TypeFacts::NE_UNDEFINED);
    m.insert("object", TypeFacts::TYPEOF_NE_OBJECT);
    m.insert("function", TypeFacts::TYPEOF_NE_FUNCTION);
    m
});

impl Checker {
    // Go: checker/flow.go:646 narrowTypeByLiteralExpression
    pub fn narrow_type_by_literal_expression(
        &mut self,
        t: TypeId,
        literal: Node,
        assume_true: bool,
    ) -> TypeId {
        if assume_true {
            return self.narrow_type_by_type_name(t, literal.text());
        }
        let facts = match TYPEOF_NE_FACTS.get(literal.text()) {
            Some(facts) => *facts,
            None => TypeFacts::TYPEOF_NE_HOST_OBJECT,
        };
        self.get_adjusted_type_with_facts(t, facts)
    }

    // Go: checker/flow.go:657 narrowTypeByTypeName
    pub fn narrow_type_by_type_name(&mut self, t: TypeId, type_name: &str) -> TypeId {
        match type_name {
            "string" => {
                let implied = self.string_type;
                return self.narrow_type_by_type_facts(t, implied, TypeFacts::TYPEOF_EQ_STRING);
            }
            "number" => {
                let implied = self.number_type;
                return self.narrow_type_by_type_facts(t, implied, TypeFacts::TYPEOF_EQ_NUMBER);
            }
            "bigint" => {
                let implied = self.bigint_type;
                return self.narrow_type_by_type_facts(t, implied, TypeFacts::TYPEOF_EQ_BIG_INT);
            }
            "boolean" => {
                let implied = self.boolean_type;
                return self.narrow_type_by_type_facts(t, implied, TypeFacts::TYPEOF_EQ_BOOLEAN);
            }
            "symbol" => {
                let implied = self.es_symbol_type;
                return self.narrow_type_by_type_facts(t, implied, TypeFacts::TYPEOF_EQ_SYMBOL);
            }
            "object" => {
                if self.ty(t).flags.intersects(TypeFlags::ANY) {
                    return t;
                }
                let non_primitive_type = self.non_primitive_type;
                let null_type = self.null_type;
                let object_part = self.narrow_type_by_type_facts(
                    t,
                    non_primitive_type,
                    TypeFacts::TYPEOF_EQ_OBJECT,
                );
                let null_part = self.narrow_type_by_type_facts(t, null_type, TypeFacts::EQ_NULL);
                return self.get_union_type(&[object_part, null_part]);
            }
            "function" => {
                if self.ty(t).flags.intersects(TypeFlags::ANY) {
                    return t;
                }
                let implied = self.global_function_type;
                return self.narrow_type_by_type_facts(t, implied, TypeFacts::TYPEOF_EQ_FUNCTION);
            }
            "undefined" => {
                let implied = self.undefined_type;
                return self.narrow_type_by_type_facts(t, implied, TypeFacts::EQ_UNDEFINED);
            }
            _ => {}
        }
        let implied = self.non_primitive_type;
        self.narrow_type_by_type_facts(t, implied, TypeFacts::TYPEOF_EQ_HOST_OBJECT)
    }

    // Go: checker/flow.go:685 narrowTypeByTypeFacts
    pub fn narrow_type_by_type_facts(
        &mut self,
        t: TypeId,
        implied_type: TypeId,
        facts: TypeFacts,
    ) -> TypeId {
        let strict_subtype_relation = self.strict_subtype_relation.clone();
        self.map_type(t, &mut |c, t| {
            if c.is_type_related_to(t, implied_type, &strict_subtype_relation) {
                if c.has_type_facts(t, facts) {
                    return t;
                }
                return c.never_type;
            } else if c.is_type_subtype_of(implied_type, t) {
                return implied_type;
            } else if c.has_type_facts(t, facts) {
                return c.get_intersection_type(&[t, implied_type]);
            }
            c.never_type
        })
    }

    // Go: checker/flow.go:702 narrowTypeByDiscriminantProperty
    pub fn narrow_type_by_discriminant_property(
        &mut self,
        t: TypeId,
        access: Node,
        operator: SyntaxKind,
        value: Node,
        assume_true: bool,
    ) -> TypeId {
        if (operator == SyntaxKind::EqualsEqualsEqualsToken
            || operator == SyntaxKind::ExclamationEqualsEqualsToken)
            && self.ty(t).flags.intersects(TypeFlags::UNION)
        {
            let key_property_name = self.get_key_property_name(t);
            if !key_property_name.is_empty() {
                let (accessed_name, ok) = self.get_accessed_property_name(access);
                if ok && key_property_name == accessed_name {
                    let value_type = self.get_type_of_expression(value);
                    let candidate = self.get_constituent_type_for_key_type(t, value_type);
                    if candidate.is_some() {
                        if assume_true && operator == SyntaxKind::EqualsEqualsEqualsToken
                            || !assume_true && operator == SyntaxKind::ExclamationEqualsEqualsToken
                        {
                            return candidate;
                        }
                        let prop_type =
                            self.get_type_of_property_of_type(candidate, &key_property_name);
                        if prop_type.is_some() && self.is_unit_type(prop_type) {
                            return self.remove_type(t, candidate);
                        }
                        return t;
                    }
                }
            }
        }
        self.narrow_type_by_discriminant(t, access, &mut |c, t| {
            c.narrow_type_by_equality(t, operator, value, assume_true)
        })
    }

    // Go: checker/flow.go:725 narrowTypeByDiscriminant
    pub fn narrow_type_by_discriminant(
        &mut self,
        t: TypeId,
        access: Node,
        narrow_type: &mut dyn FnMut(&mut Checker, TypeId) -> TypeId,
    ) -> TypeId {
        let (prop_name, ok) = self.get_accessed_property_name(access);
        if !ok {
            return t;
        }
        let optional_chain = is_optional_chain(access);
        let remove_nullable = self.strict_null_checks
            && (optional_chain || is_non_null_access(access))
            && self.maybe_type_of_kind(t, TypeFlags::NULLABLE);
        let mut non_null_type = t;
        if remove_nullable {
            non_null_type = self.get_type_with_facts(t, TypeFacts::NE_UNDEFINED_OR_NULL);
        }
        let mut prop_type = self.get_type_of_property_of_type(non_null_type, &prop_name);
        if prop_type.is_nil() {
            return t;
        }
        if remove_nullable && optional_chain {
            prop_type = self.get_optional_type(prop_type, false);
        }
        let narrowed_prop_type = narrow_type(self, prop_type);
        self.filter_type(t, &mut |c, t| {
            let mut discriminant_type =
                c.get_type_of_property_or_index_signature_of_type(t, &prop_name);
            if discriminant_type.is_nil() {
                discriminant_type = c.unknown_type;
            }
            !c.ty(discriminant_type).flags.intersects(TypeFlags::NEVER)
                && !c.ty(narrowed_prop_type).flags.intersects(TypeFlags::NEVER)
                && c.are_types_comparable(narrowed_prop_type, discriminant_type)
        })
    }

    // Go: checker/flow.go:750 isMatchingConstructorReference
    pub fn is_matching_constructor_reference(&mut self, f: &mut FlowState, expr: Node) -> bool {
        let mut name = Node::NIL;
        if is_property_access_expression(expr) {
            name = expr.name();
        } else if is_element_access_expression(expr)
            && is_string_literal_like(expr.argument_expression())
        {
            name = expr.argument_expression();
        }
        let reference = f.reference;
        name.is_some()
            && name.text() == "constructor"
            && self.is_matching_flow_reference(f, expr.expression())
    }

    // Go: checker/flow.go:760 narrowTypeByConstructor
    pub fn narrow_type_by_constructor(
        &mut self,
        t: TypeId,
        operator: SyntaxKind,
        identifier: Node,
        assume_true: bool,
    ) -> TypeId {
        // Do not narrow when checking inequality.
        if assume_true
            && operator != SyntaxKind::EqualsEqualsToken
            && operator != SyntaxKind::EqualsEqualsEqualsToken
            || !assume_true
                && operator != SyntaxKind::ExclamationEqualsToken
                && operator != SyntaxKind::ExclamationEqualsEqualsToken
        {
            return t;
        }
        // Get the type of the constructor identifier expression, if it is not a function then do not narrow.
        let identifier_type = self.get_type_of_expression(identifier);
        if !self.is_function_type(identifier_type) && !self.is_constructor_type(identifier_type) {
            return t;
        }
        // Get the prototype property of the type identifier so we can find out its type.
        let prototype_property = self.get_property_of_type(identifier_type, "prototype");
        if prototype_property.is_nil() {
            return t;
        }
        // Get the type of the prototype, if it is undefined, or the global `Object` or `Function` types then do not narrow.
        let prototype_type = self.get_type_of_symbol(prototype_property);
        let mut candidate = TypeId::NIL;
        if !self.is_type_any(prototype_type) {
            candidate = prototype_type;
        }
        if candidate.is_nil()
            || candidate == self.global_object_type
            || candidate == self.global_function_type
        {
            return t;
        }
        // If the type that is being narrowed is `any` then just return the `candidate` type since every type is a subtype of `any`.
        if self.is_type_any(t) {
            return candidate;
        }
        // Filter out types that are not considered to be "constructed by" the `candidate` type.
        self.filter_type(t, &mut |c, t| c.is_constructed_by(t, candidate))
    }

    // Go: checker/flow.go:794 isConstructedBy
    pub fn is_constructed_by(&mut self, source: TypeId, target: TypeId) -> bool {
        // If either the source or target type are a class type then we need to check that they are the same exact type.
        // This is because you may have a class `A` that defines some set of properties, and another class `B`
        // that defines the same set of properties as class `A`, in that case they are structurally the same
        // type, but when you do something like `instanceOfA.constructor === B` it will return false.
        let (source_flags, source_object_flags, source_symbol) = {
            let s = self.ty(source);
            (s.flags, s.object_flags, s.symbol)
        };
        let (target_flags, target_object_flags, target_symbol) = {
            let t = self.ty(target);
            (t.flags, t.object_flags, t.symbol)
        };
        if source_flags.intersects(TypeFlags::OBJECT)
            && source_object_flags.intersects(ObjectFlags::CLASS)
            || target_flags.intersects(TypeFlags::OBJECT)
                && target_object_flags.intersects(ObjectFlags::CLASS)
        {
            return source_symbol == target_symbol;
        }
        // For all other types just check that the `source` type is a subtype of the `target` type.
        self.is_type_subtype_of(source, target)
    }

    // Go: checker/flow.go:806 narrowTypeByBooleanComparison
    pub fn narrow_type_by_boolean_comparison(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: Node,
        bool_value: Node,
        operator: SyntaxKind,
        assume_true: bool,
    ) -> TypeId {
        let assume_true = (assume_true != (bool_value.kind() == SyntaxKind::TrueKeyword))
            != (operator != SyntaxKind::ExclamationEqualsEqualsToken
                && operator != SyntaxKind::ExclamationEqualsToken);
        self.narrow_type(f, t, expr, assume_true)
    }

    // Go: checker/flow.go:811 narrowTypeByInstanceof
    // PORT: Go takes `*ast.BinaryExpression`; this takes the node.
    pub fn narrow_type_by_instanceof(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: Node,
        assume_true: bool,
    ) -> TypeId {
        let reference = f.reference;
        let left = self.get_reference_candidate(expr.left());
        if !self.is_matching_flow_reference(f, left) {
            if assume_true
                && self.strict_null_checks
                && self.optional_chain_contains_reference(left, reference)
            {
                return self.get_adjusted_type_with_facts(t, TypeFacts::NE_UNDEFINED_OR_NULL);
            }
            return t;
        }
        let right = expr.right();
        let right_type = self.get_type_of_expression(right);
        let global_object_type = self.global_object_type;
        if !self.is_type_derived_from(right_type, global_object_type) {
            return t;
        }
        // if the right-hand side has an object type with a custom `[Symbol.hasInstance]` method, and that method
        // has a type predicate, use the type predicate to perform narrowing. This allows normal `object` types to
        // participate in `instanceof`, as per Step 2 of https://tc39.es/ecma262/#sec-instanceofoperator.
        let mut predicate = TypePredicateId::NIL;
        let signature = self.get_effects_signature(expr);
        if signature.is_some() {
            predicate = self.get_type_predicate_of_signature(signature);
        }
        if predicate.is_some()
            && self.pred(predicate).kind == TypePredicateKind::IDENTIFIER
            && self.pred(predicate).parameter_index == 0
        {
            let predicate_t = self.pred(predicate).t;
            return self.get_narrowed_type(t, predicate_t, assume_true, true /*checkDerived*/);
        }
        let global_function_type = self.global_function_type;
        if !self.is_type_derived_from(right_type, global_function_type) {
            return t;
        }
        let instance_type = self.map_type(right_type, &mut |c, t| c.get_instance_type(t));
        // Don't narrow from `any` if the target type is exactly `Object` or `Function`, and narrow
        // in the false branch only if the target is a non-empty object type.
        if self.is_type_any(t)
            && (instance_type == self.global_object_type
                || instance_type == self.global_function_type)
            || !assume_true
                && !(self.ty(instance_type).flags.intersects(TypeFlags::OBJECT)
                    && !self.is_empty_anonymous_object_type(instance_type))
        {
            return t;
        }
        self.get_narrowed_type(t, instance_type, assume_true, true /*checkDerived*/)
    }

    // Go: checker/flow.go:846 getNarrowedType
    pub fn get_narrowed_type(
        &mut self,
        t: TypeId,
        candidate: TypeId,
        assume_true: bool,
        check_derived: bool,
    ) -> TypeId {
        if !self.ty(t).flags.intersects(TypeFlags::UNION) {
            return self.get_narrowed_type_worker(t, candidate, assume_true, check_derived);
        }
        let key = NarrowedTypeKey {
            t,
            candidate,
            assume_true,
            check_derived,
        };
        if let Some(narrowed_type) = self.narrowed_types.get(&key) {
            return *narrowed_type;
        }
        let narrowed_type = self.get_narrowed_type_worker(t, candidate, assume_true, check_derived);
        self.narrowed_types.insert(key, narrowed_type);
        narrowed_type
    }

    // Go: checker/flow.go:859 getNarrowedTypeWorker
    pub fn get_narrowed_type_worker(
        &mut self,
        t: TypeId,
        candidate: TypeId,
        assume_true: bool,
        check_derived: bool,
    ) -> TypeId {
        let mut t = t;
        if !assume_true {
            if t == candidate {
                return self.never_type;
            }
            if check_derived {
                return self.filter_type(t, &mut |c, t| !c.is_type_derived_from(t, candidate));
            }
            if self.ty(t).flags.intersects(TypeFlags::UNKNOWN) {
                t = self.unknown_union_type;
            }
            let true_type = self.get_narrowed_type(
                t, candidate, true,  /*assumeTrue*/
                false, /*checkDerived*/
            );
            let filtered = self.filter_type(t, &mut |c, t| !c.is_type_subset_of(t, true_type));
            return self.recombine_unknown_type(filtered);
        }
        if self.ty(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
            return candidate;
        }
        if t == candidate {
            return candidate;
        }
        // We first attempt to filter the current type, narrowing constituents as appropriate and removing
        // constituents that are unrelated to the candidate.
        let mut key_property_name = String::new();
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            key_property_name = self.get_key_property_name(t);
        }
        let outer_t = t;
        let narrowed_type = self.map_type(candidate, &mut |c, n| {
            // If a discriminant property is available, use that to reduce the type.
            let mut matching = outer_t;
            if !key_property_name.is_empty() {
                let discriminant = c.get_type_of_property_of_type(n, &key_property_name);
                if discriminant.is_some() {
                    let constituent = c.get_constituent_type_for_key_type(outer_t, discriminant);
                    if constituent.is_some() {
                        matching = constituent;
                    }
                }
            }
            // For each constituent t in the current type, if t and c are directly related, pick the most
            // specific of the two. When t and c are related in both directions, we prefer c for type predicates
            // because that is the asserted type, but t for `instanceof` because generics aren't reflected in
            // prototype object types.
            let directly_related = c.map_type(matching, &mut |c, t| {
                if check_derived {
                    if c.is_type_derived_from(t, n) {
                        return t;
                    } else if c.is_type_derived_from(n, t) {
                        return n;
                    }
                    return c.never_type;
                }
                if c.is_type_strict_subtype_of(t, n) {
                    return t;
                } else if c.is_type_strict_subtype_of(n, t) {
                    return n;
                } else if c.is_type_subtype_of(t, n) {
                    return t;
                } else if c.is_type_subtype_of(n, t) {
                    return n;
                }
                c.never_type
            });
            if !c.ty(directly_related).flags.intersects(TypeFlags::NEVER) {
                return directly_related;
            }
            // If no constituents are directly related, create intersections for any generic constituents that
            // are related by constraint.
            // PORT: Go picks `isRelated` as a func value (isTypeDerivedFrom or
            // isTypeSubtypeOf); the choice is made inline on `check_derived`.
            c.map_type(outer_t, &mut |c, t| {
                if c.maybe_type_of_kind(t, TypeFlags::INSTANTIABLE) {
                    let constraint = c.get_base_constraint_of_type(t);
                    if constraint.is_nil()
                        || if check_derived {
                            c.is_type_derived_from(n, constraint)
                        } else {
                            c.is_type_subtype_of(n, constraint)
                        }
                    {
                        return c.get_intersection_type(&[t, n]);
                    }
                }
                c.never_type
            })
        });
        // If filtering produced a non-empty type, return that. Otherwise, pick the most specific of the two
        // based on assignability, or as a last resort produce an intersection.
        if !self.ty(narrowed_type).flags.intersects(TypeFlags::NEVER) {
            return narrowed_type;
        } else if self.is_type_subtype_of(candidate, t) {
            return candidate;
        } else if self.is_type_assignable_to(t, candidate) {
            return t;
        } else if self.is_type_assignable_to(candidate, t) {
            return candidate;
        }
        self.get_intersection_type(&[t, candidate])
    }
}
