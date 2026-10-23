//! Go `internal/binder/binder.go` lines 1817-2795: statement and expression
//! control flow binding, narrowing predicates, declaration helpers and binder
//! diagnostics.

use crate::astdata::NodeData;
use crate::prelude::*;

// PORT: Binder storage used by this file. Go stores binder output on the AST
// nodes and flow nodes directly. The Rust `Binder` (defined in binder_p1)
// writes the node records and holds the rest until `bind_source_file` fills
// the `GoFile` cells:
// - `self.flow_nodes: Vec<FlowNode>`, indexed by `FlowNodeId::local_index()`.
// - `self.node_bind: NodeBindRecords`, indexed by `node.node_id().index()`.
// - `self.file_bind: FileBindData` (bind diagnostics and the other file data).
// - `self.symbols: SymbolArena` (Go `symbolArena`).
// - `self.active_label_list: Option<Rc<RefCell<ActiveLabel>>>`.
// All direct access goes through the `p3_*` helpers below so a storage
// mismatch is fixed in one place.
impl Binder {
    fn p3_flow_flags(&self, flow: FlowNodeId) -> FlowFlags {
        self.flow_nodes[flow.local_index()].flags
    }

    fn p3_flow_antecedents(&self, flow: FlowNodeId) -> Vec<FlowNodeId> {
        self.flow_nodes[flow.local_index()].antecedents.clone()
    }

    fn p3_set_flow_antecedents(&mut self, flow: FlowNodeId, antecedents: Vec<FlowNodeId>) {
        self.flow_nodes[flow.local_index()].antecedents = antecedents;
    }

    /// Writes the `flow_node` of a node: its record, or its extras entry
    /// (`NodeBindRecords::set_flow_node`).
    fn p3_set_node_flow(&mut self, node: Node, flow: FlowNodeId) {
        debug_assert!(
            node.file_index() == self.file.file_index(),
            "binder data for a node in another file"
        );
        self.node_bind.set_flow_node(node.node_id().index(), flow);
    }
}

// PORT: Go `node.FlowNodeData() != nil`. True for node kinds whose Go data
// embeds `FlowNodeBase` (StatementBase, IterationStatementBase,
// AccessorDeclarationBase, Identifier, QualifiedName, BindingElement,
// MethodDeclaration, KeywordExpression, ArrowFunction, FunctionExpression,
// PropertyAccessExpression, ElementAccessExpression, MetaProperty).
fn p3_has_flow_node_data(node: Node) -> bool {
    matches!(
        node.kind(),
        // IterationStatementBase
        SyntaxKind::DoStatement
            | SyntaxKind::WhileStatement
            | SyntaxKind::ForStatement
            // StatementBase
            | SyntaxKind::ForInStatement
            | SyntaxKind::ForOfStatement
            | SyntaxKind::EmptyStatement
            | SyntaxKind::IfStatement
            | SyntaxKind::BreakStatement
            | SyntaxKind::ContinueStatement
            | SyntaxKind::ReturnStatement
            | SyntaxKind::WithStatement
            | SyntaxKind::SwitchStatement
            | SyntaxKind::ThrowStatement
            | SyntaxKind::TryStatement
            | SyntaxKind::DebuggerStatement
            | SyntaxKind::LabeledStatement
            | SyntaxKind::ExpressionStatement
            | SyntaxKind::Block
            | SyntaxKind::VariableStatement
            | SyntaxKind::MissingDeclaration
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::ModuleBlock
            | SyntaxKind::NotEmittedStatement
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::ExportAssignment
            | SyntaxKind::NamespaceExportDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ExportDeclaration
            // AccessorDeclarationBase
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            // Other FlowNodeBase embedders
            | SyntaxKind::Identifier
            | SyntaxKind::QualifiedName
            | SyntaxKind::BindingElement
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::ArrowFunction
            | SyntaxKind::FunctionExpression
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::MetaProperty
            // KeywordExpression
            | SyntaxKind::NullKeyword
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::ThisKeyword
            | SyntaxKind::SuperKeyword
            | SyntaxKind::ImportKeyword
    )
}

// Go: binder/binder.go:1818 isLogicalAssignmentExpression
pub fn is_logical_assignment_expression(node: Node) -> bool {
    is_logical_or_coalescing_assignment_expression(skip_parentheses(node))
}

impl Binder {
    // Go: binder/binder.go:1822 bindAssignmentTargetFlow
    pub fn bind_assignment_target_flow(&mut self, node: Node) {
        match node.kind() {
            SyntaxKind::ArrayLiteralExpression => {
                for e in node.elements().iter() {
                    if e.kind() == SyntaxKind::SpreadElement {
                        self.bind_assignment_target_flow(e.expression());
                    } else {
                        self.bind_destructuring_target_flow(e);
                    }
                }
            }
            SyntaxKind::ObjectLiteralExpression => {
                for p in node.properties().iter() {
                    match p.kind() {
                        SyntaxKind::PropertyAssignment => {
                            self.bind_destructuring_target_flow(p.initializer());
                        }
                        SyntaxKind::ShorthandPropertyAssignment => {
                            self.bind_assignment_target_flow(p.name());
                        }
                        SyntaxKind::SpreadAssignment => {
                            self.bind_assignment_target_flow(p.expression());
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                if is_narrowable_reference(node) {
                    self.current_flow =
                        self.create_flow_mutation(FlowFlags::ASSIGNMENT, self.current_flow, node);
                }
            }
        }
    }

    // Go: binder/binder.go:1850 bindDestructuringTargetFlow
    pub fn bind_destructuring_target_flow(&mut self, node: Node) {
        if is_binary_expression(node) && node.operator_token().kind() == SyntaxKind::EqualsToken {
            self.bind_assignment_target_flow(node.left());
        } else {
            self.bind_assignment_target_flow(node);
        }
    }

    // Go: binder/binder.go:1858 bindWhileStatement
    pub fn bind_while_statement(&mut self, node: Node) {
        let loop_label = self.create_loop_label();
        let pre_while_label = self.set_continue_target(node, loop_label);
        let pre_body_label = self.create_branch_label();
        let post_while_label = self.create_branch_label();
        self.add_antecedent(pre_while_label, self.current_flow);
        self.current_flow = pre_while_label;
        self.bind_condition(node.expression(), pre_body_label, post_while_label);
        self.current_flow = self.finish_flow_label(pre_body_label);
        self.bind_iterative_statement(node.statement(), post_while_label, pre_while_label);
        self.add_antecedent(pre_while_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_while_label);
    }

    // Go: binder/binder.go:1872 bindDoStatement
    pub fn bind_do_statement(&mut self, node: Node) {
        let pre_do_label = self.create_loop_label();
        let branch_label = self.create_branch_label();
        let pre_condition_label = self.set_continue_target(node, branch_label);
        let post_do_label = self.create_branch_label();
        self.add_antecedent(pre_do_label, self.current_flow);
        self.current_flow = pre_do_label;
        self.bind_iterative_statement(node.statement(), post_do_label, pre_condition_label);
        self.add_antecedent(pre_condition_label, self.current_flow);
        self.current_flow = self.finish_flow_label(pre_condition_label);
        self.bind_condition(node.expression(), pre_do_label, post_do_label);
        self.current_flow = self.finish_flow_label(post_do_label);
    }

    // Go: binder/binder.go:1886 bindForStatement
    pub fn bind_for_statement(&mut self, node: Node) {
        self.bind(node.initializer());
        if self.current_flow == self.unreachable_flow {
            // Unlike while/do, the for-loop initializer is bound inside this function before the loop's
            // flow graph is constructed. If it makes flow unreachable (e.g. a throwing IIFE), addAntecedent
            // will filter out the unreachable entry to preLoopLabel, leaving only the back-edge from the
            // incrementor. This creates a cycle with no exit that crashes isReachableFlowNodeWorker.
            // Bail out early and just bind the remaining children with unreachable flow.
            self.bind(node.condition());
            self.bind(node.statement());
            self.bind(node.incrementor());
            return;
        }
        let loop_label = self.create_loop_label();
        let pre_loop_label = self.set_continue_target(node, loop_label);
        let pre_body_label = self.create_branch_label();
        let pre_incrementor_label = self.create_branch_label();
        let post_loop_label = self.create_branch_label();
        self.add_antecedent(pre_loop_label, self.current_flow);
        self.current_flow = pre_loop_label;
        self.bind_condition(node.condition(), pre_body_label, post_loop_label);
        self.current_flow = self.finish_flow_label(pre_body_label);
        self.bind_iterative_statement(node.statement(), post_loop_label, pre_incrementor_label);
        self.add_antecedent(pre_incrementor_label, self.current_flow);
        self.current_flow = self.finish_flow_label(pre_incrementor_label);
        self.bind(node.incrementor());
        self.add_antecedent(pre_loop_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_loop_label);
    }

    // Go: binder/binder.go:1916 bindForInOrForOfStatement
    pub fn bind_for_in_or_for_of_statement(&mut self, node: Node) {
        self.bind(node.expression());
        if self.current_flow == self.unreachable_flow {
            // Like the for-loop initializer, the for-in/for-of expression is bound before the loop's
            // flow graph is constructed. If it makes flow unreachable (e.g. a throwing IIFE), addAntecedent
            // will filter out the unreachable entry to preLoopLabel, leaving only the back-edge from the
            // loop body. This creates a cycle with no exit that crashes isReachableFlowNodeWorker.
            // Bail out early and just bind the remaining children with unreachable flow.
            self.bind(node.initializer());
            self.bind(node.statement());
            return;
        }
        let loop_label = self.create_loop_label();
        let pre_loop_label = self.set_continue_target(node, loop_label);
        let post_loop_label = self.create_branch_label();
        self.add_antecedent(pre_loop_label, self.current_flow);
        self.current_flow = pre_loop_label;
        if node.kind() == SyntaxKind::ForOfStatement {
            self.bind(node.await_modifier());
        }
        self.add_antecedent(post_loop_label, self.current_flow);
        let initializer = node.initializer();
        self.bind(initializer);
        if initializer.kind() != SyntaxKind::VariableDeclarationList {
            self.bind_assignment_target_flow(initializer);
        }
        self.bind_iterative_statement(node.statement(), post_loop_label, pre_loop_label);
        self.add_antecedent(pre_loop_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_loop_label);
    }

    // Go: binder/binder.go:1946 bindIfStatement
    pub fn bind_if_statement(&mut self, node: Node) {
        let then_label = self.create_branch_label();
        let else_label = self.create_branch_label();
        let post_if_label = self.create_branch_label();
        // PERF: noderead2. Both branches from one node data read (Go
        // `node.AsIfStatement()` fields).
        let (then_statement, else_statement) = match frozen_data_ids(node) {
            Some((NodeData::IfStatement(d), ids)) => (
                ids.node(d.then_statement),
                d.else_statement.map_or(Node::NIL, |id| ids.node(id)),
            ),
            _ => (node.then_statement(), node.else_statement()),
        };
        self.bind_condition(node.expression(), then_label, else_label);
        self.current_flow = self.finish_flow_label(then_label);
        self.bind(then_statement);
        self.add_antecedent(post_if_label, self.current_flow);
        self.current_flow = self.finish_flow_label(else_label);
        self.bind(else_statement);
        self.add_antecedent(post_if_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_if_label);
    }

    // Go: binder/binder.go:1961 bindReturnStatement
    pub fn bind_return_statement(&mut self, node: Node) {
        self.bind(node.expression());
        if self.current_return_target.is_some() {
            self.add_antecedent(self.current_return_target, self.current_flow);
        }
        self.current_flow = self.unreachable_flow;
        self.has_explicit_return = true;
        self.has_flow_effects = true;
    }

    // Go: binder/binder.go:1971 bindThrowStatement
    pub fn bind_throw_statement(&mut self, node: Node) {
        self.bind(node.expression());
        self.current_flow = self.unreachable_flow;
        self.has_flow_effects = true;
    }

    // Go: binder/binder.go:1977 bindBreakStatement
    pub fn bind_break_statement(&mut self, node: Node) {
        self.bind_break_or_continue_statement(
            node.label(),
            self.current_break_target,
            ActiveLabel::break_target_exported,
        );
    }

    // Go: binder/binder.go:1981 bindContinueStatement
    pub fn bind_continue_statement(&mut self, node: Node) {
        self.bind_break_or_continue_statement(
            node.label(),
            self.current_continue_target,
            ActiveLabel::continue_target_exported,
        );
    }

    // Go: binder/binder.go:1985 bindBreakOrContinueStatement
    pub fn bind_break_or_continue_statement(
        &mut self,
        label: Node,
        current_target: FlowNodeId,
        get_target: fn(&ActiveLabel) -> FlowNodeId,
    ) {
        self.bind(label);
        if label.is_some() {
            let active_label = self.find_active_label(label.text());
            if let Some(active_label) = active_label {
                active_label.borrow_mut().referenced = true;
                let target = get_target(&active_label.borrow());
                self.bind_break_or_continue_flow(target);
            }
        } else {
            self.bind_break_or_continue_flow(current_target);
        }
    }

    // Go: binder/binder.go:1998 findActiveLabel
    pub fn find_active_label(&self, name: &str) -> Option<Rc<RefCell<ActiveLabel>>> {
        let mut label = self.active_label_list.clone();
        while let Some(l) = label {
            if l.borrow().name == name {
                return Some(l);
            }
            label = l.borrow().next.clone();
        }
        None
    }

    // Go: binder/binder.go:2007 bindBreakOrContinueFlow
    pub fn bind_break_or_continue_flow(&mut self, flow_label: FlowNodeId) {
        if flow_label.is_some() {
            self.add_antecedent(flow_label, self.current_flow);
            self.current_flow = self.unreachable_flow;
            self.has_flow_effects = true;
        }
    }

    // Go: binder/binder.go:2015 bindTryStatement
    pub fn bind_try_statement(&mut self, node: Node) {
        // We conservatively assume that *any* code in the try block can cause an exception, but we only need
        // to track code that causes mutations (because only mutations widen the possible control flow type of
        // a variable). The exceptionLabel is the target label for control flows that result from exceptions.
        // We add all mutation flow nodes as antecedents of this label such that we can analyze them as possible
        // antecedents of the start of catch or finally blocks. Furthermore, we add the current control flow to
        // represent exceptions that occur before any mutations.
        let finally_block = node.finally_block();
        let catch_clause = node.catch_clause();
        let save_return_target = self.current_return_target;
        let save_exception_target = self.current_exception_target;
        let normal_exit_label = self.create_branch_label();
        let return_label = self.create_branch_label();
        let mut exception_label = self.create_branch_label();
        if finally_block.is_some() {
            self.current_return_target = return_label;
        }
        self.add_antecedent(exception_label, self.current_flow);
        self.current_exception_target = exception_label;
        self.bind(node.try_block());
        self.add_antecedent(normal_exit_label, self.current_flow);
        if catch_clause.is_some() {
            // Start of catch clause is the target of exceptions from try block.
            self.current_flow = self.finish_flow_label(exception_label);
            // The currentExceptionTarget now represents control flows from exceptions in the catch clause.
            // Effectively, in a try-catch-finally, if an exception occurs in the try block, the catch block
            // acts like a second try block.
            exception_label = self.create_branch_label();
            self.add_antecedent(exception_label, self.current_flow);
            self.current_exception_target = exception_label;
            self.bind(catch_clause);
            self.add_antecedent(normal_exit_label, self.current_flow);
        }
        self.current_return_target = save_return_target;
        self.current_exception_target = save_exception_target;
        if finally_block.is_some() {
            // Possible ways control can reach the finally block:
            // 1) Normal completion of try block of a try-finally or try-catch-finally
            // 2) Normal completion of catch block (following exception in try block) of a try-catch-finally
            // 3) Return in try or catch block of a try-finally or try-catch-finally
            // 4) Exception in try block of a try-finally
            // 5) Exception in catch block of a try-catch-finally
            // When analyzing a control flow graph that starts inside a finally block we want to consider all
            // five possibilities above. However, when analyzing a control flow graph that starts outside (past)
            // the finally block, we only want to consider the first two (if we're past a finally block then it
            // must have completed normally). Likewise, when analyzing a control flow graph from return statements
            // in try or catch blocks in an IIFE, we only want to consider the third. To make this possible, we
            // inject a ReduceLabel node into the control flow graph. This node contains an alternate reduced
            // set of antecedents for the pre-finally label. As control flow analysis passes by a ReduceLabel
            // node, the pre-finally label is temporarily switched to the reduced antecedent set.
            let finally_label = self.create_branch_label();
            let normal_exit_antecedents = self.p3_flow_antecedents(normal_exit_label);
            let exception_antecedents = self.p3_flow_antecedents(exception_label);
            let return_antecedents = self.p3_flow_antecedents(return_label);
            let tail = self.combine_flow_lists(&exception_antecedents, &return_antecedents);
            let combined = self.combine_flow_lists(&normal_exit_antecedents, &tail);
            self.p3_set_flow_antecedents(finally_label, combined);
            self.current_flow = finally_label;
            self.bind(finally_block);
            if self
                .p3_flow_flags(self.current_flow)
                .intersects(FlowFlags::UNREACHABLE)
            {
                // If the end of the finally block is unreachable, the end of the entire try statement is unreachable.
                self.current_flow = self.unreachable_flow;
            } else {
                // If we have an IIFE return target and return statements in the try or catch blocks, add a control
                // flow that goes back through the finally block and back through only the return statements.
                if self.current_return_target.is_some() && !return_antecedents.is_empty() {
                    let reduce = self.create_reduce_label(
                        finally_label,
                        &return_antecedents,
                        self.current_flow,
                    );
                    self.add_antecedent(self.current_return_target, reduce);
                }
                // If we have an outer exception target (i.e. a containing try-finally or try-catch-finally), add a
                // control flow that goes back through the finally block and back through each possible exception source.
                if self.current_exception_target.is_some() && !exception_antecedents.is_empty() {
                    let reduce = self.create_reduce_label(
                        finally_label,
                        &exception_antecedents,
                        self.current_flow,
                    );
                    self.add_antecedent(self.current_exception_target, reduce);
                }
                // If the end of the finally block is reachable, but the end of the try and catch blocks are not,
                // convert the current flow to unreachable. For example, 'try { return 1; } finally { ... }' should
                // result in an unreachable current control flow.
                if !normal_exit_antecedents.is_empty() {
                    self.current_flow = self.create_reduce_label(
                        finally_label,
                        &normal_exit_antecedents,
                        self.current_flow,
                    );
                } else {
                    self.current_flow = self.unreachable_flow;
                }
            }
        } else {
            self.current_flow = self.finish_flow_label(normal_exit_label);
        }
    }

    // Go: binder/binder.go:2096 bindSwitchStatement
    pub fn bind_switch_statement(&mut self, node: Node) {
        let post_switch_label = self.create_branch_label();
        self.bind(node.expression());
        let save_break_target = self.current_break_target;
        let save_pre_switch_case_flow = self.pre_switch_case_flow;
        self.current_break_target = post_switch_label;
        self.pre_switch_case_flow = self.current_flow;
        let case_block = node.case_block();
        self.bind(case_block);
        self.add_antecedent(post_switch_label, self.current_flow);
        let has_default = case_block
            .clauses()
            .nodes()
            .iter()
            .any(|c| c.kind() == SyntaxKind::DefaultClause);
        if !has_default {
            let clause_flow = self.create_flow_switch_clause(self.pre_switch_case_flow, node, 0, 0);
            self.add_antecedent(post_switch_label, clause_flow);
        }
        self.current_break_target = save_break_target;
        self.pre_switch_case_flow = save_pre_switch_case_flow;
        self.current_flow = self.finish_flow_label(post_switch_label);
    }

    // Go: binder/binder.go:2117 bindCaseBlock
    pub fn bind_case_block(&mut self, node: Node) {
        let switch_statement = node.parent();
        let clauses = node.clauses().nodes();
        let is_narrowing_switch = switch_statement.expression().kind() == SyntaxKind::TrueKeyword
            || is_narrowing_expression(switch_statement.expression());
        let mut fallthrough_flow: FlowNodeId = self.unreachable_flow;
        let mut i = 0usize;
        while i < clauses.len() {
            let clause_start = i;
            while clauses.get(i).statements().is_empty() && i + 1 < clauses.len() {
                if fallthrough_flow == self.unreachable_flow {
                    self.current_flow = self.pre_switch_case_flow;
                }
                self.bind(clauses.get(i));
                i += 1;
            }
            let pre_case_label = self.create_branch_label();
            let mut pre_case_flow = self.pre_switch_case_flow;
            if is_narrowing_switch {
                pre_case_flow = self.create_flow_switch_clause(
                    self.pre_switch_case_flow,
                    switch_statement,
                    clause_start as i32,
                    (i + 1) as i32,
                );
            }
            self.add_antecedent(pre_case_label, pre_case_flow);
            self.add_antecedent(pre_case_label, fallthrough_flow);
            self.current_flow = self.finish_flow_label(pre_case_label);
            let clause = clauses.get(i);
            self.bind(clause);
            fallthrough_flow = self.current_flow;
            if !self
                .p3_flow_flags(self.current_flow)
                .intersects(FlowFlags::UNREACHABLE)
                && i != clauses.len() - 1
            {
                // PORT: Go `clause.AsCaseOrDefaultClause().FallthroughFlowNode`. `NodeBindData` has no
                // separate field, and case/default clauses have no Go `FlowNodeData`, so the fallthrough
                // flow node is stored in the clause's `flow_node` slot (read it with `clause.flow_node()`).
                let current_flow = self.current_flow;
                self.p3_set_node_flow(clause, current_flow);
            }
            i += 1;
        }
    }

    // Go: binder/binder.go:2148 bindCaseOrDefaultClause
    pub fn bind_case_or_default_clause(&mut self, node: Node) {
        let expression = node.expression();
        if expression.is_some() {
            let save_current_flow = self.current_flow;
            self.current_flow = self.pre_switch_case_flow;
            self.bind(expression);
            self.current_flow = save_current_flow;
        }
        self.bind_each(&node.statements().to_vec());
    }

    // Go: binder/binder.go:2159 bindExpressionStatement
    pub fn bind_expression_statement(&mut self, node: Node) {
        let expression = node.expression();
        self.bind(expression);
        self.maybe_bind_expression_flow_if_call(expression);
    }

    // Go: binder/binder.go:2165 maybeBindExpressionFlowIfCall
    pub fn maybe_bind_expression_flow_if_call(&mut self, node: Node) {
        // A top level or comma expression call expression with a dotted function name and at least one argument
        // is potentially an assertion and is therefore included in the control flow.
        if is_call_expression(node) {
            let expression = node.expression();
            if expression.kind() != SyntaxKind::SuperKeyword && is_dotted_name(expression) {
                self.current_flow = self.create_flow_call(self.current_flow, node);
            }
        }
    }

    // Go: binder/binder.go:2175 bindLabeledStatement
    pub fn bind_labeled_statement(&mut self, node: Node) {
        let label = node.label();
        let post_statement_label = self.create_branch_label();
        self.active_label_list = Some(Rc::new(RefCell::new(ActiveLabel {
            next: self.active_label_list.take(),
            name: label.text().to_string(),
            break_target: post_statement_label,
            continue_target: FlowNodeId::NIL,
            referenced: false,
        })));
        self.bind(label);
        self.bind(node.statement());
        let active = self.active_label_list.clone().expect("active label list");
        if !active.borrow().referenced {
            // Mark the label as unused; the checker will decide whether to report it
            self.add_node_flags(label, NodeFlags::UNREACHABLE);
        }
        self.active_label_list = active.borrow().next.clone();
        self.add_antecedent(post_statement_label, self.current_flow);
        self.current_flow = self.finish_flow_label(post_statement_label);
    }

    // Go: binder/binder.go:2196 bindPrefixUnaryExpressionFlow
    pub fn bind_prefix_unary_expression_flow(&mut self, node: Node) {
        let operator = node.operator();
        if operator == SyntaxKind::ExclamationToken {
            let save_true_target = self.current_true_target;
            self.current_true_target = self.current_false_target;
            self.current_false_target = save_true_target;
            self.bind_each_child(node);
            self.current_false_target = self.current_true_target;
            self.current_true_target = save_true_target;
        } else {
            self.bind_each_child(node);
            if operator == SyntaxKind::PlusPlusToken || operator == SyntaxKind::MinusMinusToken {
                self.bind_assignment_target_flow(node.operand());
            }
        }
    }

    // Go: binder/binder.go:2213 bindPostfixUnaryExpressionFlow
    pub fn bind_postfix_unary_expression_flow(&mut self, node: Node) {
        let operator = node.operator();
        self.bind_each_child(node);
        if operator == SyntaxKind::PlusPlusToken || operator == SyntaxKind::MinusMinusToken {
            self.bind_assignment_target_flow(node.operand());
        }
    }

    // Go: binder/binder.go:2221 bindDestructuringAssignmentFlow
    // PERF: binderview1. `expr` holds the fields of the node (Go
    // `node.AsBinaryExpression()`), read once.
    pub fn bind_destructuring_assignment_flow(&mut self, expr: &BinaryView) {
        if self.in_assignment_pattern {
            self.in_assignment_pattern = false;
            self.bind(expr.operator_token);
            self.bind(expr.right);
            self.in_assignment_pattern = true;
            self.bind(expr.left);
            self.bind(expr.type_);
        } else {
            self.in_assignment_pattern = true;
            self.bind(expr.left);
            self.bind(expr.type_);
            self.in_assignment_pattern = false;
            self.bind(expr.operator_token);
            self.bind(expr.right);
        }
        self.bind_assignment_target_flow(expr.left);
    }

    // Go: binder/binder.go:2241 bindBinaryExpressionFlow
    // PERF: binderview1. `expr` holds the fields of `node`, read once.
    // PORT: issue #99. Go binds an operand that is a BinaryExpression by
    // recursion (bind, bindChildren, bindBinaryExpressionFlow, and for a
    // logical operator bindLogicalLikeExpression and bindCondition), 3 to 5
    // frames for each level of a chain. A long chain overflows a small host
    // stack (wasm). When an operand is a BinaryExpression, the binary frames
    // (`BindBinaryFrame`) bind the chain instead: the same calls in Go's
    // order, with a heap frame for each BinaryExpression operand.
    pub fn bind_binary_expression_flow(&mut self, node: Node, expr: &BinaryView) {
        if expr.left.kind() == SyntaxKind::BinaryExpression
            || expr.right.kind() == SyntaxKind::BinaryExpression
        {
            let mut frames = BindFrames::new(self);
            frames.push_flow(node, *expr, None);
            frames.run();
            return;
        }
        let operator = expr.operator;
        if is_logical_or_coalescing_binary_operator(operator)
            || is_logical_or_coalescing_assignment_operator(operator)
        {
            if is_top_level_logical_expression(node) {
                let post_expression_label = self.create_branch_label();
                let save_current_flow = self.current_flow;
                let save_has_flow_effects = self.has_flow_effects;
                self.has_flow_effects = false;
                self.bind_logical_like_expression(
                    node,
                    expr,
                    post_expression_label,
                    post_expression_label,
                );
                if self.has_flow_effects {
                    self.current_flow = self.finish_flow_label(post_expression_label);
                } else {
                    self.current_flow = save_current_flow;
                }
                self.has_flow_effects = self.has_flow_effects || save_has_flow_effects;
            } else {
                self.bind_logical_like_expression(
                    node,
                    expr,
                    self.current_true_target,
                    self.current_false_target,
                );
            }
        } else {
            let left = expr.left;
            let right = expr.right;
            self.bind(left);
            self.bind(expr.type_);
            if operator == SyntaxKind::CommaToken {
                self.maybe_bind_expression_flow_if_call(left);
            }
            self.bind(expr.operator_token);
            self.bind(right);
            self.bind_binary_expression_flow_end(node, expr);
        }
    }

    /// The end of the non-logical arm of `bind_binary_expression_flow`,
    /// after Right is bound (Go binder.go:2268-2279): the comma call flow,
    /// then the assignment flow.
    #[inline]
    fn bind_binary_expression_flow_end(&mut self, node: Node, expr: &BinaryView) {
        let operator = expr.operator;
        let left = expr.left;
        if operator == SyntaxKind::CommaToken {
            self.maybe_bind_expression_flow_if_call(expr.right);
        }
        if is_assignment_operator(operator) && !is_assignment_target(node) {
            self.bind_assignment_target_flow(left);
            if operator == SyntaxKind::EqualsToken
                && left.kind() == SyntaxKind::ElementAccessExpression
            {
                if is_narrowable_operand(left.expression()) {
                    self.current_flow = self.create_flow_mutation(
                        FlowFlags::ARRAY_MUTATION,
                        self.current_flow,
                        node,
                    );
                }
            }
        }
    }

    /// The end of `bind_logical_like_expression`, after Right is bound (Go
    /// binder.go:2295-2297): a logical assignment adds its assignment flow
    /// and its own condition flows.
    #[inline]
    fn bind_logical_like_expression_end(
        &mut self,
        node: Node,
        expr: &BinaryView,
        true_target: FlowNodeId,
        false_target: FlowNodeId,
    ) {
        if is_logical_or_coalescing_assignment_operator(expr.operator) {
            self.bind_assignment_target_flow(expr.left);
            let true_flow =
                self.create_flow_condition(FlowFlags::TRUE_CONDITION, self.current_flow, node);
            self.add_antecedent(true_target, true_flow);
            let false_flow =
                self.create_flow_condition(FlowFlags::FALSE_CONDITION, self.current_flow, node);
            self.add_antecedent(false_target, false_flow);
        }
    }

    // Go: binder/binder.go:2283 bindLogicalLikeExpression
    // PERF: binderview1. `expr` holds the fields of `node`, read once.
    pub fn bind_logical_like_expression(
        &mut self,
        node: Node,
        expr: &BinaryView,
        true_target: FlowNodeId,
        false_target: FlowNodeId,
    ) {
        let pre_right_label = self.create_branch_label();
        if expr.operator == SyntaxKind::AmpersandAmpersandToken
            || expr.operator == SyntaxKind::AmpersandAmpersandEqualsToken
        {
            self.bind_condition(expr.left, pre_right_label, false_target);
        } else {
            self.bind_condition(expr.left, true_target, pre_right_label);
        }
        self.current_flow = self.finish_flow_label(pre_right_label);
        self.bind(expr.operator_token);
        if is_logical_or_coalescing_assignment_operator(expr.operator) {
            self.do_with_conditional_branches(
                &mut Binder::bind,
                expr.right,
                true_target,
                false_target,
            );
        } else {
            self.bind_condition(expr.right, true_target, false_target);
        }
        self.bind_logical_like_expression_end(node, expr, true_target, false_target);
    }

    // Go: binder/binder.go:2303 bindDeleteExpressionFlow
    pub fn bind_delete_expression_flow(&mut self, node: Node) {
        self.bind_each_child(node);
        if node.expression().kind() == SyntaxKind::PropertyAccessExpression {
            self.bind_assignment_target_flow(node.expression());
        }
    }

    // Go: binder/binder.go:2311 bindConditionalExpressionFlow
    pub fn bind_conditional_expression_flow(&mut self, node: Node) {
        let true_label = self.create_branch_label();
        let false_label = self.create_branch_label();
        let post_expression_label = self.create_branch_label();
        let save_current_flow = self.current_flow;
        let save_has_flow_effects = self.has_flow_effects;
        self.has_flow_effects = false;
        self.bind_condition(node.condition(), true_label, false_label);
        self.current_flow = self.finish_flow_label(true_label);
        self.bind(node.question_token());
        self.bind(node.when_true());
        self.add_antecedent(post_expression_label, self.current_flow);
        self.current_flow = self.finish_flow_label(false_label);
        self.bind(node.colon_token());
        self.bind(node.when_false());
        self.add_antecedent(post_expression_label, self.current_flow);
        if self.has_flow_effects {
            self.current_flow = self.finish_flow_label(post_expression_label);
        } else {
            self.current_flow = save_current_flow;
        }
        self.has_flow_effects = self.has_flow_effects || save_has_flow_effects;
    }

    // Go: binder/binder.go:2336 bindVariableDeclarationFlow
    pub fn bind_variable_declaration_flow(&mut self, node: Node) {
        self.bind_each_child(node);
        if node.initializer().is_some() || is_for_in_or_of_statement(node.parent().parent()) {
            self.bind_initialized_variable_flow(node);
        }
    }

    // Go: binder/binder.go:2343 bindInitializedVariableFlow
    pub fn bind_initialized_variable_flow(&mut self, node: Node) {
        let mut name = Node::NIL;
        match node.kind() {
            SyntaxKind::VariableDeclaration => {
                name = node.name();
            }
            SyntaxKind::BindingElement => {
                name = node.name();
            }
            _ => {}
        }
        if name.is_some() && is_binding_pattern(name) {
            for child in name.elements().iter() {
                self.bind_initialized_variable_flow(child);
            }
        } else {
            self.current_flow =
                self.create_flow_mutation(FlowFlags::ASSIGNMENT, self.current_flow, node);
        }
    }

    // Go: binder/binder.go:2360 bindAccessExpressionFlow
    pub fn bind_access_expression_flow(&mut self, node: Node) {
        if is_optional_chain(node) {
            self.bind_optional_chain_flow(node);
        } else {
            self.bind_each_child(node);
        }
    }

    // Go: binder/binder.go:2368 bindOptionalChainFlow
    pub fn bind_optional_chain_flow(&mut self, node: Node) {
        if is_top_level_logical_expression(node) {
            let post_expression_label = self.create_branch_label();
            let save_current_flow = self.current_flow;
            let save_has_flow_effects = self.has_flow_effects;
            self.bind_optional_chain(node, post_expression_label, post_expression_label);
            if self.has_flow_effects {
                self.current_flow = self.finish_flow_label(post_expression_label);
            } else {
                self.current_flow = save_current_flow;
            }
            self.has_flow_effects = self.has_flow_effects || save_has_flow_effects;
        } else {
            self.bind_optional_chain(node, self.current_true_target, self.current_false_target);
        }
    }

    // Go: binder/binder.go:2385 bindOptionalChain
    pub fn bind_optional_chain(
        &mut self,
        node: Node,
        true_target: FlowNodeId,
        false_target: FlowNodeId,
    ) {
        // For an optional chain, we emulate the behavior of a logical expression:
        //
        // a?.b         -> a && a.b
        // a?.b.c       -> a && a.b.c
        // a?.b?.c      -> a && a.b && a.b.c
        // a?.[x = 1]   -> a && a[x = 1]
        //
        // To do this we descend through the chain until we reach the root of a chain (the expression with a `?.`)
        // and build it's CFA graph as if it were the first condition (`a && ...`). Then we bind the rest
        // of the node as part of the "true" branch, and continue to do so as we ascend back up to the outermost
        // chain node. We then treat the entire node as the right side of the expression.
        let mut pre_chain_label = FlowNodeId::NIL;
        if is_optional_chain_root(node) {
            pre_chain_label = self.create_branch_label();
        }
        let optional_true_target = if pre_chain_label.is_some() {
            pre_chain_label
        } else {
            true_target
        };
        self.bind_optional_expression(node.expression(), optional_true_target, false_target);
        if pre_chain_label.is_some() {
            self.current_flow = self.finish_flow_label(pre_chain_label);
        }
        self.do_with_conditional_branches(
            &mut Binder::bind_optional_chain_rest,
            node,
            true_target,
            false_target,
        );
        if is_outermost_optional_chain(node) {
            let true_flow =
                self.create_flow_condition(FlowFlags::TRUE_CONDITION, self.current_flow, node);
            self.add_antecedent(true_target, true_flow);
            let false_flow =
                self.create_flow_condition(FlowFlags::FALSE_CONDITION, self.current_flow, node);
            self.add_antecedent(false_target, false_flow);
        }
    }

    // Go: binder/binder.go:2412 bindOptionalExpression
    pub fn bind_optional_expression(
        &mut self,
        node: Node,
        true_target: FlowNodeId,
        false_target: FlowNodeId,
    ) {
        self.do_with_conditional_branches(&mut Binder::bind, node, true_target, false_target);
        if !is_optional_chain(node) || is_outermost_optional_chain(node) {
            let true_flow =
                self.create_flow_condition(FlowFlags::TRUE_CONDITION, self.current_flow, node);
            self.add_antecedent(true_target, true_flow);
            let false_flow =
                self.create_flow_condition(FlowFlags::FALSE_CONDITION, self.current_flow, node);
            self.add_antecedent(false_target, false_flow);
        }
    }

    // Go: binder/binder.go:2420 bindOptionalChainRest
    pub fn bind_optional_chain_rest(&mut self, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::PropertyAccessExpression => {
                self.bind(node.question_dot_token());
                self.bind(node.name());
            }
            SyntaxKind::ElementAccessExpression => {
                self.bind(node.question_dot_token());
                self.bind(node.argument_expression());
            }
            SyntaxKind::CallExpression => {
                self.bind(node.question_dot_token());
                self.bind_node_list(node.type_argument_list());
                self.bind_each(&node.arguments().to_vec());
            }
            _ => {}
        }
        false
    }

    // Go: binder/binder.go:2436 bindCallExpressionFlow
    pub fn bind_call_expression_flow(&mut self, node: Node) {
        let call_expression = node.expression();
        if is_optional_chain(node) {
            self.bind_optional_chain_flow(node);
        } else {
            // If the target of the call expression is a function expression or arrow function we have
            // an immediately invoked function expression (IIFE). Initialize the flowNode property to
            // the current control flow (which includes evaluation of the IIFE arguments).
            let expr = skip_parentheses(call_expression);
            if expr.kind() == SyntaxKind::FunctionExpression
                || expr.kind() == SyntaxKind::ArrowFunction
            {
                self.bind_node_list(node.type_argument_list());
                self.bind_each(&node.arguments().to_vec());
                self.bind(call_expression);
            } else {
                self.bind_each_child(node);
                if call_expression.kind() == SyntaxKind::SuperKeyword {
                    self.current_flow = self.create_flow_call(self.current_flow, node);
                }
            }
        }
        if is_property_access_expression(call_expression) {
            let access = call_expression;
            if is_identifier(access.name())
                && is_narrowable_operand(access.expression())
                && is_push_or_unshift_identifier(access.name())
            {
                self.current_flow =
                    self.create_flow_mutation(FlowFlags::ARRAY_MUTATION, self.current_flow, node);
            }
        }
    }

    // Go: binder/binder.go:2464 bindNonNullExpressionFlow
    pub fn bind_non_null_expression_flow(&mut self, node: Node) {
        if is_optional_chain(node) {
            self.bind_optional_chain_flow(node);
        } else {
            self.bind_each_child(node);
        }
    }

    // Go: binder/binder.go:2472 bindBindingElementFlow
    pub fn bind_binding_element_flow(&mut self, node: Node) {
        // When evaluating a binding pattern, the initializer is evaluated before the binding pattern, per:
        // - https://tc39.es/ecma262/#sec-destructuring-binding-patterns-runtime-semantics-iteratorbindinginitialization
        //   - `BindingElement: BindingPattern Initializer?`
        // - https://tc39.es/ecma262/#sec-runtime-semantics-keyedbindinginitialization
        //   - `BindingElement: BindingPattern Initializer?`
        self.bind(node.dot_dot_dot_token());
        self.bind(node.property_name());
        self.bind_initializer(node.initializer());
        self.bind(node.name());
    }

    // Go: binder/binder.go:2485 bindParameterFlow
    pub fn bind_parameter_flow(&mut self, node: Node) {
        // PERF: query Q7-3. The node data is looked up once for the
        // `_in` field reads. Binding never changes node data.
        let d = parsed_node_data(node);
        self.bind_modifiers(node.modifiers_in(d));
        self.bind(node.dot_dot_dot_token());
        self.bind(node.question_token_in(d));
        self.bind(node.type_());
        self.bind_initializer(node.initializer_in(d));
        self.bind(node.name_in(d));
    }

    // Go: binder/binder.go:2496 bindInitializer
    // a BindingElement/Parameter does not have side effects if initializers are not evaluated and used. (see GH#49759)
    pub fn bind_initializer(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }
        let entry_flow = self.current_flow;
        self.bind(node);
        if entry_flow == self.unreachable_flow || entry_flow == self.current_flow {
            return;
        }
        let exit_flow = self.create_branch_label();
        self.add_antecedent(exit_flow, entry_flow);
        self.add_antecedent(exit_flow, self.current_flow);
        self.current_flow = self.finish_flow_label(exit_flow);
    }

    // Go: binder/binder.go:2511 setFlowNode
    // PORT: Go package function that writes to the node. Binder output lives
    // in the binder until the file is bound, so this is a `Binder` method.
    pub fn set_flow_node(&mut self, node: Node, flow_node: FlowNodeId) {
        if p3_has_flow_node_data(node) {
            self.p3_set_node_flow(node, flow_node);
        }
    }

    // Go: binder/binder.go:2518 setReturnFlowNode
    // PORT: Binder method for the same reason as `set_flow_node`.
    pub fn set_return_flow_node(&mut self, node: Node, return_flow_node: FlowNodeId) {
        match node.kind() {
            SyntaxKind::Constructor
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ClassStaticBlockDeclaration => {
                self.set_node_return_flow_node(node, return_flow_node);
            }
            _ => {}
        }
    }
}

/// A BinaryExpression on the binder's work stack (`Binder::binary_frames`,
/// issue #99). The frames bind a chain of BinaryExpressions with the calls
/// of Go's recursion, in Go's order: `bind` (its kind arm, then the parse
/// error flags around `bindChildren`), `bindChildren` (`inAssignmentPattern`,
/// the unreachable path), `bindBinaryExpressionFlow`, and for a logical
/// operator `bindLogicalLikeExpression` and the `bindCondition` or
/// `doWithConditionalBranches` call of each operand. Only an operand that is
/// a BinaryExpression gets a frame; any other operand is bound by the usual
/// call.
#[derive(Clone, Copy)]
pub struct BindBinaryFrame {
    node: Node,
    expr: BinaryView,
    stage: BindStage,
    /// The targets of Go `bindLogicalLikeExpression` (logical operators).
    true_target: FlowNodeId,
    false_target: FlowNodeId,
    /// Go `preRightLabel` of `bindLogicalLikeExpression`.
    pre_right_label: FlowNodeId,
    /// What Go `bindBinaryExpressionFlow` saved for a top-level logical
    /// expression.
    top_level: Option<TopLevelLogical>,
    /// What the operand's call, `bind` and `bindChildren` saved. None for
    /// the frame of the node that `bind_binary_expression_flow` got: its
    /// callers run those steps.
    entered: Option<EnteredOperand>,
}

/// The next step of a `BindBinaryFrame`.
#[derive(Clone, Copy)]
enum BindStage {
    /// Bind Left (Go binder.go:2261).
    Left,
    /// Left is bound: Type, the comma call flow of Left, the operator, then
    /// Right (binder.go:2262-2267).
    Right,
    /// Right is bound (binder.go:2268-2279).
    End,
    /// Bind Left as a condition (binder.go:2285-2290).
    LogicalLeft,
    /// Left is bound: finish the pre-right label, the operator, then Right
    /// (binder.go:2291-2294, 2299).
    LogicalRight,
    /// Right is bound (binder.go:2295-2297), then the end of a top-level
    /// logical expression (binder.go:2251-2256).
    LogicalEnd,
}

/// Go `bindBinaryExpressionFlow` for a top-level logical expression.
#[derive(Clone, Copy)]
struct TopLevelLogical {
    post_expression_label: FlowNodeId,
    save_current_flow: FlowNodeId,
    save_has_flow_effects: bool,
}

/// How a frame binds an operand that is a BinaryExpression.
#[derive(Clone, Copy)]
enum OperandCall {
    /// Go `b.bind(operand)`.
    Bind,
    /// Go `b.bindCondition(operand, trueTarget, falseTarget)`.
    Condition(FlowNodeId, FlowNodeId),
    /// Go `b.doWithConditionalBranches((*Binder).bind, operand, trueTarget, falseTarget)`.
    Branches(FlowNodeId, FlowNodeId),
}

/// What a frame's operand call saved before it got to Go
/// `bindBinaryExpressionFlow` of the operand.
#[derive(Clone, Copy)]
struct EnteredOperand {
    call: OperandCall,
    /// `doWithConditionalBranches` (also in `bindCondition`). NIL for a
    /// Bind call, which does not restore them.
    save_true_target: FlowNodeId,
    save_false_target: FlowNodeId,
    /// `bind`.
    this_node_or_any_subnodes_has_error: bool,
    save_seen_parse_error: bool,
    /// `bindChildren`.
    save_in_assignment_pattern: bool,
}

/// The binder with its binary frames from `base` up. A drop truncates the
/// frames to `base`, so a panic (the LSP catches one for each request)
/// leaves none behind.
struct BindFrames<'a> {
    b: &'a mut Binder,
    base: usize,
}

impl Drop for BindFrames<'_> {
    fn drop(&mut self) {
        self.b.binary_frames.truncate(self.base);
    }
}

impl<'a> BindFrames<'a> {
    fn new(b: &'a mut Binder) -> Self {
        let base = b.binary_frames.len();
        Self { b, base }
    }

    /// Go `bindBinaryExpressionFlow(node)` up to its first operand, and for
    /// a logical operator `bindLogicalLikeExpression` up to Left: pushes the
    /// frame of `node`.
    fn push_flow(&mut self, node: Node, expr: BinaryView, entered: Option<EnteredOperand>) {
        let b = &mut *self.b;
        let mut frame = BindBinaryFrame {
            node,
            expr,
            stage: BindStage::Left,
            true_target: FlowNodeId::NIL,
            false_target: FlowNodeId::NIL,
            pre_right_label: FlowNodeId::NIL,
            top_level: None,
            entered,
        };
        let operator = expr.operator;
        if is_logical_or_coalescing_binary_operator(operator)
            || is_logical_or_coalescing_assignment_operator(operator)
        {
            if is_top_level_logical_expression(node) {
                let post_expression_label = b.create_branch_label();
                frame.top_level = Some(TopLevelLogical {
                    post_expression_label,
                    save_current_flow: b.current_flow,
                    save_has_flow_effects: b.has_flow_effects,
                });
                b.has_flow_effects = false;
                frame.true_target = post_expression_label;
                frame.false_target = post_expression_label;
            } else {
                frame.true_target = b.current_true_target;
                frame.false_target = b.current_false_target;
            }
            frame.pre_right_label = b.create_branch_label();
            frame.stage = BindStage::LogicalLeft;
        }
        b.binary_frames.push(frame);
    }

    /// Runs the frames above `base` until none is left.
    fn run(&mut self) {
        while self.b.binary_frames.len() > self.base {
            let top = self.b.binary_frames.len() - 1;
            let frame = self.b.binary_frames[top];
            let expr = frame.expr;
            match frame.stage {
                BindStage::Left => {
                    self.b.binary_frames[top].stage = BindStage::Right;
                    self.bind_operand(expr.left, OperandCall::Bind);
                }
                BindStage::Right => {
                    let b = &mut *self.b;
                    b.binary_frames[top].stage = BindStage::End;
                    b.bind(expr.type_);
                    if expr.operator == SyntaxKind::CommaToken {
                        b.maybe_bind_expression_flow_if_call(expr.left);
                    }
                    b.bind(expr.operator_token);
                    self.bind_operand(expr.right, OperandCall::Bind);
                }
                BindStage::End => {
                    self.b.binary_frames.pop();
                    self.b.bind_binary_expression_flow_end(frame.node, &expr);
                    self.leave(frame);
                }
                BindStage::LogicalLeft => {
                    self.b.binary_frames[top].stage = BindStage::LogicalRight;
                    let call = if expr.operator == SyntaxKind::AmpersandAmpersandToken
                        || expr.operator == SyntaxKind::AmpersandAmpersandEqualsToken
                    {
                        OperandCall::Condition(frame.pre_right_label, frame.false_target)
                    } else {
                        OperandCall::Condition(frame.true_target, frame.pre_right_label)
                    };
                    self.bind_operand(expr.left, call);
                }
                BindStage::LogicalRight => {
                    let b = &mut *self.b;
                    b.binary_frames[top].stage = BindStage::LogicalEnd;
                    b.current_flow = b.finish_flow_label(frame.pre_right_label);
                    b.bind(expr.operator_token);
                    let call = if is_logical_or_coalescing_assignment_operator(expr.operator) {
                        OperandCall::Branches(frame.true_target, frame.false_target)
                    } else {
                        OperandCall::Condition(frame.true_target, frame.false_target)
                    };
                    self.bind_operand(expr.right, call);
                }
                BindStage::LogicalEnd => {
                    let b = &mut *self.b;
                    b.binary_frames.pop();
                    b.bind_logical_like_expression_end(
                        frame.node,
                        &expr,
                        frame.true_target,
                        frame.false_target,
                    );
                    if let Some(top_level) = frame.top_level {
                        if b.has_flow_effects {
                            b.current_flow = b.finish_flow_label(top_level.post_expression_label);
                        } else {
                            b.current_flow = top_level.save_current_flow;
                        }
                        b.has_flow_effects = b.has_flow_effects || top_level.save_has_flow_effects;
                    }
                    self.leave(frame);
                }
            }
        }
    }

    /// Binds `operand` as `call` does. An operand that is a BinaryExpression
    /// runs Go `bind` and `bindChildren` up to `bindBinaryExpressionFlow`,
    /// which gets a frame. The unreachable path and a destructuring
    /// assignment run `bind_children_of_kind` (Go `bindChildren`) as it is.
    fn bind_operand(&mut self, operand: Node, call: OperandCall) {
        let b = &mut *self.b;
        if operand.kind() != SyntaxKind::BinaryExpression {
            match call {
                OperandCall::Bind => {
                    b.bind(operand);
                }
                OperandCall::Condition(true_target, false_target) => {
                    b.bind_condition(operand, true_target, false_target);
                }
                OperandCall::Branches(true_target, false_target) => {
                    b.do_with_conditional_branches(
                        &mut Binder::bind,
                        operand,
                        true_target,
                        false_target,
                    );
                }
            }
            return;
        }
        // Go `doWithConditionalBranches` (binder.go:1790). A Bind call does
        // not save or set the targets.
        let (save_true_target, save_false_target) = match call {
            OperandCall::Bind => (FlowNodeId::NIL, FlowNodeId::NIL),
            OperandCall::Condition(true_target, false_target)
            | OperandCall::Branches(true_target, false_target) => {
                let saved = (b.current_true_target, b.current_false_target);
                b.current_true_target = true_target;
                b.current_false_target = false_target;
                saved
            }
        };
        // Go `bind` (binder.go:577) of a BinaryExpression, up to
        // `bindChildren`. Its container flags are None.
        let expr = b.binary_view(operand);
        b.bind_binary_expression_kind(operand, &expr);
        let this_node_or_any_subnodes_has_error = b
            .node_flags(operand)
            .intersects(NodeFlags::THIS_NODE_HAS_ERROR);
        let save_seen_parse_error = b.seen_parse_error;
        b.seen_parse_error = false;
        let entered = EnteredOperand {
            call,
            save_true_target,
            save_false_target,
            this_node_or_any_subnodes_has_error,
            save_seen_parse_error,
            save_in_assignment_pattern: b.in_assignment_pattern,
        };
        if b.current_flow == b.unreachable_flow || expr.is_destructuring_assignment() {
            // Go `bindChildren` as it is. It ends with `inAssignmentPattern`
            // as it found it, as `leave_operand` sets it.
            b.bind_children_of_kind(operand, SyntaxKind::BinaryExpression, Some(&expr));
            b.leave_operand(operand, entered);
            return;
        }
        // Go `bindChildren` (binder.go:1646) up to `bindBinaryExpressionFlow`.
        b.in_assignment_pattern = false;
        self.push_flow(operand, expr, Some(entered));
    }

    /// The end of a frame: for an operand, the end of Go `bindChildren`,
    /// `bind` and the operand's call.
    fn leave(&mut self, frame: BindBinaryFrame) {
        if let Some(entered) = frame.entered {
            self.b.leave_operand(frame.node, entered);
        }
    }
}

impl Binder {
    /// The end of Go `bindChildren` (binder.go:1742), `bind`
    /// (binder.go:740-748) and the operand's call, for a BinaryExpression
    /// operand of a binary frame.
    fn leave_operand(&mut self, node: Node, entered: EnteredOperand) {
        self.in_assignment_pattern = entered.save_in_assignment_pattern;
        let mut this_node_or_any_subnodes_has_error = entered.this_node_or_any_subnodes_has_error;
        if self.seen_parse_error {
            this_node_or_any_subnodes_has_error = true;
        }
        self.seen_parse_error = entered.save_seen_parse_error;
        if this_node_or_any_subnodes_has_error {
            self.add_node_flags(node, NodeFlags::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR);
            self.seen_parse_error = true;
        }
        match entered.call {
            OperandCall::Bind => {}
            OperandCall::Branches(..) => {
                self.current_true_target = entered.save_true_target;
                self.current_false_target = entered.save_false_target;
            }
            OperandCall::Condition(true_target, false_target) => {
                self.current_true_target = entered.save_true_target;
                self.current_false_target = entered.save_false_target;
                self.bind_condition_flows(node, true_target, false_target);
            }
        }
    }
}

// Go: binder/binder.go:2531 isGeneratorFunctionExpression
pub fn is_generator_function_expression(node: Node) -> bool {
    is_function_expression(node) && node.asterisk_token().is_some()
}

impl Binder {
    // Go: binder/binder.go:2535 addToContainerChain
    pub fn add_to_container_chain(&mut self, next: Node) {
        if self.last_container.is_some() {
            let last_container = self.last_container;
            self.set_next_container(last_container, next);
        }
        self.last_container = next;
    }

    // Go: binder/binder.go:2542 addDeclarationToSymbol
    // PERF: one `&mut Symbol` for the whole function, as Go writes the fields
    // of one struct (5 to 7 arena lookups before). The single declaration is
    // pushed in place: built in a local and stored, the 16-byte list went
    // through the stack with two 8-byte stores and one 16-byte load (a store
    // forwarding stall on every new symbol).
    pub fn add_declaration_to_symbol(
        &mut self,
        symbol: SymbolId,
        node: Node,
        symbol_flags: SymbolFlags,
    ) {
        self.set_node_symbol(node, symbol);
        let s = self.symbols.sym_mut(symbol);
        s.flags |= symbol_flags;
        // PORT: Go `newSingleDeclaration` for a nil list, else
        // `core.AppendIfUnique`. A push on the empty list makes the same
        // single declaration (`new_single_declaration`).
        if !s.declarations.contains(&node) {
            s.declarations.push(node);
        }
        // On merge of const enum module with class or function, reset const enum only flag (namespaces will already recalculate)
        if s.flags.intersects(SymbolFlags::CONST_ENUM_ONLY_MODULE)
            && s.flags
                .intersects(SymbolFlags::FUNCTION | SymbolFlags::CLASS | SymbolFlags::REGULAR_ENUM)
        {
            s.flags = s.flags.without(SymbolFlags::CONST_ENUM_ONLY_MODULE);
            self.not_const_enum_only_modules.insert(symbol);
        }
        // Go `SetValueDeclaration(symbol, node)`.
        if symbol_flags.intersects(SymbolFlags::VALUE)
            && takes_value_declaration(s.value_declaration, node)
        {
            s.value_declaration = node;
        }
    }
}

impl Binder {
    /// Go `newSymbol(flags, name)`, then `addDeclarationToSymbol(symbol,
    /// node, includes)` and `symbol.Parent = parent`: `declare_symbol_ex`
    /// for a name that its table does not hold yet. The caller writes the
    /// table.
    // PERF: parsefast1 item 2. The symbol is pushed with all its fields, as
    // Go writes the fields of one struct, and not made empty and then
    // written field by field through the arena.
    pub(crate) fn new_declared_symbol(
        &mut self,
        name: Name,
        flags: SymbolFlags,
        node: Node,
        includes: SymbolFlags,
        parent: SymbolId,
    ) -> SymbolId {
        self.symbol_count += 1;
        let mut flags = flags | includes;
        // On merge of const enum module with class or function, reset const enum only flag (namespaces will already recalculate)
        let const_enum_reset = flags.intersects(SymbolFlags::CONST_ENUM_ONLY_MODULE)
            && flags
                .intersects(SymbolFlags::FUNCTION | SymbolFlags::CLASS | SymbolFlags::REGULAR_ENUM);
        if const_enum_reset {
            flags = flags.without(SymbolFlags::CONST_ENUM_ONLY_MODULE);
        }
        let declarations = self.new_single_declaration(node);
        // Go `SetValueDeclaration` on a symbol with no value declaration.
        let value_declaration = if includes.intersects(SymbolFlags::VALUE) {
            node
        } else {
            Node::NIL
        };
        let symbol = self.symbols.push_symbol(Symbol {
            flags,
            data: SymbolData {
                name,
                declarations,
                value_declaration,
                parent,
                ..SymbolData::default()
            },
            ..Symbol::default()
        });
        if const_enum_reset {
            self.not_const_enum_only_modules.insert(symbol);
        }
        self.set_node_symbol(node, symbol);
        symbol
    }
}

// Go: binder/binder.go:2560 SetValueDeclaration
pub fn set_value_declaration(symbols: &mut SymbolArena, symbol: SymbolId, node: Node) {
    if takes_value_declaration(symbols.sym(symbol).value_declaration, node) {
        symbols.sym_mut(symbol).value_declaration = node;
    }
}

/// The test of Go `SetValueDeclaration`: whether `node` replaces the value
/// declaration `value_declaration` of its symbol.
// Go: binder/binder.go:2560 SetValueDeclaration
// PERF: reads each node kind once (Go reads `Kind` fields; a port kind read
// goes through the node store).
#[inline]
fn takes_value_declaration(value_declaration: Node, node: Node) -> bool {
    if value_declaration.is_nil() {
        return true;
    }
    let value_kind = value_declaration.kind();
    let node_kind = node.kind();
    // Non-assignment declarations take precedence over assignment declarations and
    // non-namespace declarations take precedence over namespace declarations.
    is_assignment_declaration_kind(value_kind) && !is_assignment_declaration_kind(node_kind)
        || value_kind != node_kind && is_effective_module_declaration_kind(value_kind)
}

// Go: binder/binder.go:2580 GetContainerFlags
pub fn get_container_flags(node: Node) -> ContainerFlags {
    get_container_flags_of_kind(node, node.kind())
}

/// `get_container_flags` for a caller that already read `node.kind()`.
pub fn get_container_flags_of_kind(node: Node, kind: SyntaxKind) -> ContainerFlags {
    match kind {
        SyntaxKind::ClassExpression
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::EnumDeclaration
        | SyntaxKind::ObjectLiteralExpression
        | SyntaxKind::TypeLiteral
        | SyntaxKind::JsxAttributes => {
            return ContainerFlags::IS_CONTAINER;
        }
        SyntaxKind::InterfaceDeclaration => {
            return ContainerFlags::IS_CONTAINER | ContainerFlags::IS_INTERFACE;
        }
        SyntaxKind::ModuleDeclaration
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::MappedType
        | SyntaxKind::IndexSignature => {
            return ContainerFlags::IS_CONTAINER | ContainerFlags::HAS_LOCALS;
        }
        SyntaxKind::SourceFile => {
            return ContainerFlags::IS_CONTAINER
                | ContainerFlags::IS_CONTROL_FLOW_CONTAINER
                | ContainerFlags::HAS_LOCALS;
        }
        SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::Constructor
        | SyntaxKind::FunctionDeclaration
        | SyntaxKind::ClassStaticBlockDeclaration => {
            if matches!(
                kind,
                SyntaxKind::GetAccessor | SyntaxKind::SetAccessor | SyntaxKind::MethodDeclaration
            ) && is_object_literal_or_class_expression_method_or_accessor(node)
            {
                return ContainerFlags::IS_CONTAINER
                    | ContainerFlags::IS_CONTROL_FLOW_CONTAINER
                    | ContainerFlags::HAS_LOCALS
                    | ContainerFlags::IS_FUNCTION_LIKE
                    | ContainerFlags::IS_OBJECT_LITERAL_OR_CLASS_EXPRESSION_METHOD_OR_ACCESSOR
                    | ContainerFlags::IS_THIS_CONTAINER;
            }
            // Go: fallthrough
            return ContainerFlags::IS_CONTAINER
                | ContainerFlags::IS_CONTROL_FLOW_CONTAINER
                | ContainerFlags::HAS_LOCALS
                | ContainerFlags::IS_FUNCTION_LIKE
                | ContainerFlags::IS_THIS_CONTAINER;
        }
        SyntaxKind::MethodSignature
        | SyntaxKind::CallSignature
        | SyntaxKind::FunctionType
        | SyntaxKind::ConstructSignature
        | SyntaxKind::ConstructorType => {
            return ContainerFlags::IS_CONTAINER
                | ContainerFlags::IS_CONTROL_FLOW_CONTAINER
                | ContainerFlags::HAS_LOCALS
                | ContainerFlags::IS_FUNCTION_LIKE
                | ContainerFlags::PROPAGATES_THIS_KEYWORD;
        }
        SyntaxKind::FunctionExpression => {
            return ContainerFlags::IS_CONTAINER
                | ContainerFlags::IS_CONTROL_FLOW_CONTAINER
                | ContainerFlags::HAS_LOCALS
                | ContainerFlags::IS_FUNCTION_LIKE
                | ContainerFlags::IS_FUNCTION_EXPRESSION
                | ContainerFlags::IS_THIS_CONTAINER;
        }
        SyntaxKind::ArrowFunction => {
            return ContainerFlags::IS_CONTAINER
                | ContainerFlags::IS_CONTROL_FLOW_CONTAINER
                | ContainerFlags::HAS_LOCALS
                | ContainerFlags::IS_FUNCTION_LIKE
                | ContainerFlags::IS_FUNCTION_EXPRESSION
                | ContainerFlags::PROPAGATES_THIS_KEYWORD;
        }
        SyntaxKind::ModuleBlock => {
            return ContainerFlags::IS_CONTROL_FLOW_CONTAINER;
        }
        SyntaxKind::PropertyDeclaration => {
            if node.initializer().is_some() {
                return ContainerFlags::IS_CONTROL_FLOW_CONTAINER
                    | ContainerFlags::IS_THIS_CONTAINER;
            } else {
                return ContainerFlags::NONE;
            }
        }
        SyntaxKind::CatchClause
        | SyntaxKind::ForStatement
        | SyntaxKind::ForInStatement
        | SyntaxKind::ForOfStatement
        | SyntaxKind::CaseBlock => {
            return ContainerFlags::IS_BLOCK_SCOPED_CONTAINER | ContainerFlags::HAS_LOCALS;
        }
        SyntaxKind::Block => {
            if is_function_like(node.parent()) || is_class_static_block_declaration(node.parent()) {
                return ContainerFlags::NONE;
            } else {
                return ContainerFlags::IS_BLOCK_SCOPED_CONTAINER | ContainerFlags::HAS_LOCALS;
            }
        }
        _ => {}
    }
    ContainerFlags::NONE
}

// Go: binder/binder.go:2624 isNarrowingExpression
pub fn is_narrowing_expression(expr: Node) -> bool {
    match expr.kind() {
        SyntaxKind::Identifier | SyntaxKind::ThisKeyword => true,
        SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
            contains_narrowable_reference(expr)
        }
        SyntaxKind::CallExpression => has_narrowable_argument(expr),
        SyntaxKind::ParenthesizedExpression
        | SyntaxKind::NonNullExpression
        | SyntaxKind::TypeOfExpression => is_narrowing_expression(expr.expression()),
        SyntaxKind::BinaryExpression => is_narrowing_binary_expression(&BinaryView::new(expr)),
        SyntaxKind::PrefixUnaryExpression => {
            expr.operator() == SyntaxKind::ExclamationToken
                && is_narrowing_expression(expr.operand())
        }
        _ => false,
    }
}

// Go: binder/binder.go:2642 containsNarrowableReference
pub fn contains_narrowable_reference(expr: Node) -> bool {
    if is_narrowable_reference(expr) {
        return true;
    }
    if expr.flags().intersects(NodeFlags::OPTIONAL_CHAIN) {
        match expr.kind() {
            SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::CallExpression
            | SyntaxKind::NonNullExpression => {
                return contains_narrowable_reference(expr.expression());
            }
            _ => {}
        }
    }
    false
}

// Go: binder/binder.go:2655 isNarrowableReference
pub fn is_narrowable_reference(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::Identifier
        | SyntaxKind::ThisKeyword
        | SyntaxKind::SuperKeyword
        | SyntaxKind::MetaProperty => true,
        SyntaxKind::PropertyAccessExpression
        | SyntaxKind::ParenthesizedExpression
        | SyntaxKind::NonNullExpression => is_narrowable_reference(node.expression()),
        // PERF: binderview1. Go `IsStringOrNumericLiteralLike(arg) ||
        // IsEntityNameExpression(arg) && isNarrowableReference(Expression)`
        // with the kind of `arg` read once: a literal is no entity name, and
        // an entity name is an identifier or a property access.
        SyntaxKind::ElementAccessExpression => {
            let argument_expression = node.argument_expression();
            match argument_expression.kind() {
                SyntaxKind::StringLiteral
                | SyntaxKind::NoSubstitutionTemplateLiteral
                | SyntaxKind::NumericLiteral => true,
                SyntaxKind::Identifier => is_narrowable_reference(node.expression()),
                SyntaxKind::PropertyAccessExpression => {
                    is_entity_name_expression(argument_expression)
                        && is_narrowable_reference(node.expression())
                }
                _ => false,
            }
        }
        SyntaxKind::BinaryExpression => {
            let expr = BinaryView::new(node);
            expr.operator == SyntaxKind::CommaToken && is_narrowable_reference(expr.right)
                || is_assignment_operator(expr.operator) && is_left_hand_side_expression(expr.left)
        }
        _ => false,
    }
}

// Go: binder/binder.go:2673 hasNarrowableArgument
pub fn has_narrowable_argument(expr: Node) -> bool {
    for argument in expr.arguments().iter() {
        if contains_narrowable_reference(argument) {
            return true;
        }
    }
    let call_expression = expr.expression();
    if is_property_access_expression(call_expression) {
        if contains_narrowable_reference(call_expression.expression()) {
            return true;
        }
    }
    false
}

// Go: binder/binder.go:2688 isNarrowingBinaryExpression
// PORT: Go takes `*ast.BinaryExpression`; this takes its fields, read once
// (`BinaryView`).
pub fn is_narrowing_binary_expression(expr: &BinaryView) -> bool {
    match expr.operator {
        SyntaxKind::EqualsToken
        | SyntaxKind::BarBarEqualsToken
        | SyntaxKind::AmpersandAmpersandEqualsToken
        | SyntaxKind::QuestionQuestionEqualsToken => contains_narrowable_reference(expr.left),
        SyntaxKind::EqualsEqualsToken
        | SyntaxKind::ExclamationEqualsToken
        | SyntaxKind::EqualsEqualsEqualsToken
        | SyntaxKind::ExclamationEqualsEqualsToken => {
            let left = skip_parentheses(expr.left);
            let right = skip_parentheses(expr.right);
            is_narrowable_operand(left)
                || is_narrowable_operand(right)
                || is_narrowing_type_of_operands(right, left)
                || is_narrowing_type_of_operands(left, right)
                || (is_boolean_literal(right) && is_narrowing_expression(left)
                    || is_boolean_literal(left) && is_narrowing_expression(right))
        }
        SyntaxKind::InstanceOfKeyword => is_narrowable_operand(expr.left),
        SyntaxKind::InKeyword => is_narrowing_expression(expr.right),
        SyntaxKind::CommaToken => is_narrowing_expression(expr.right),
        _ => false,
    }
}

// Go: binder/binder.go:2708 isNarrowableOperand
pub fn is_narrowable_operand(expr: Node) -> bool {
    match expr.kind() {
        SyntaxKind::ParenthesizedExpression => {
            return is_narrowable_operand(expr.expression());
        }
        SyntaxKind::BinaryExpression => {
            let binary = BinaryView::new(expr);
            match binary.operator {
                SyntaxKind::EqualsToken => {
                    return is_narrowable_operand(binary.left);
                }
                SyntaxKind::CommaToken => {
                    return is_narrowable_operand(binary.right);
                }
                _ => {}
            }
        }
        _ => {}
    }
    contains_narrowable_reference(expr)
}

// Go: binder/binder.go:2724 isNarrowingTypeOfOperands
pub fn is_narrowing_type_of_operands(expr1: Node, expr2: Node) -> bool {
    is_type_of_expression(expr1)
        && is_narrowable_operand(expr1.expression())
        && is_string_literal_like(expr2)
}

impl Binder {
    // Go: binder/binder.go:2728 errorOnNode
    pub fn error_on_node(
        &mut self,
        node: Node,
        message: &'static crate::diagnostics::Message,
        args: Vec<String>,
    ) {
        let diagnostic = self.create_diagnostic_for_node(node, message, args);
        self.add_diagnostic(diagnostic);
    }

    // Go: binder/binder.go:2732 errorOnFirstToken
    pub fn error_on_first_token(
        &mut self,
        node: Node,
        message: &'static crate::diagnostics::Message,
        args: Vec<String>,
    ) {
        let span = get_range_of_token_at_position(self.file, node.pos());
        self.add_diagnostic(new_diagnostic(self.file, span, message, args));
    }

    // Go: binder/binder.go:2740 createDiagnosticForNode
    // Inside the binder, we may create a diagnostic for an as-yet unbound node (with potentially no parent pointers, implying no accessible source file)
    // If so, the node _must_ be in the current file (as that's the only way anything could have traversed to it to yield it as the error node)
    // This version of `createDiagnosticForNode` uses the binder's context to account for this, and always yields correct diagnostics even in these situations.
    pub fn create_diagnostic_for_node(
        &mut self,
        node: Node,
        message: &'static crate::diagnostics::Message,
        args: Vec<String>,
    ) -> Diagnostic {
        new_diagnostic(
            self.file,
            get_error_range_for_node(self.file, node),
            message,
            args,
        )
    }

    // Go: binder/binder.go:2744 addDiagnostic
    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.file_bind.bind_diagnostics.push(diagnostic);
    }
}

// Go: binder/binder.go:2756 getOptionalSymbolFlagForNode
// PERF: query Q7-3. `d` is the data of `node`, which the caller already
// loaded with `parsed_node_data`.
pub fn get_optional_symbol_flag_for_node(node: Node, d: LoadedData) -> SymbolFlags {
    let postfix_token = node.postfix_token_in(d);
    if postfix_token.is_some() && postfix_token.kind() == SyntaxKind::QuestionToken {
        SymbolFlags::OPTIONAL
    } else {
        SymbolFlags::NONE
    }
}

// Go: binder/binder.go:2761 isFunctionSymbol
pub fn is_function_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    let d = symbols.sym(symbol).value_declaration;
    if d.is_some() {
        if is_function_declaration(d) {
            return true;
        }
        if is_variable_declaration(d) {
            let initializer = d.initializer();
            if initializer.is_some() {
                return is_function_like(initializer);
            }
        }
    }
    false
}

// Go: binder/binder.go:2777 isStatementCondition
pub fn is_statement_condition(node: Node) -> bool {
    let parent = node.parent();
    match parent.kind() {
        SyntaxKind::IfStatement | SyntaxKind::WhileStatement | SyntaxKind::DoStatement => {
            parent.expression() == node
        }
        SyntaxKind::ForStatement => parent.condition() == node,
        SyntaxKind::ConditionalExpression => parent.condition() == node,
        _ => false,
    }
}

// Go: binder/binder.go:2789 isTopLevelLogicalExpression
// PERF: parsefast1 item 2. Each parent is read once (Go reads the
// `Parent` field each time).
pub fn is_top_level_logical_expression(node: Node) -> bool {
    let mut node = node;
    let mut parent = node.parent();
    while is_parenthesized_expression(parent)
        || is_prefix_unary_expression(parent) && parent.operator() == SyntaxKind::ExclamationToken
    {
        node = parent;
        parent = node.parent();
    }
    !is_statement_condition(node)
        && !is_logical_expression(parent)
        && !(is_optional_chain(parent) && parent.expression() == node)
}

// Go: binder/binder.go:2796 isAssignmentDeclaration
pub fn is_assignment_declaration(decl: Node) -> bool {
    is_assignment_declaration_kind(decl.kind())
}

/// `is_assignment_declaration` of a node of kind `kind`: binary, access,
/// identifier or call expression.
// Go: binder/binder.go:2796 isAssignmentDeclaration
// Go: ast/utilities.go:528 IsAccessExpression
#[inline]
fn is_assignment_declaration_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::BinaryExpression
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::Identifier
            | SyntaxKind::CallExpression
    )
}

// Go: binder/binder.go:2800 isEffectiveModuleDeclaration
pub fn is_effective_module_declaration(node: Node) -> bool {
    is_effective_module_declaration_kind(node.kind())
}

/// `is_effective_module_declaration` of a node of kind `kind`.
// Go: binder/binder.go:2800 isEffectiveModuleDeclaration
#[inline]
fn is_effective_module_declaration_kind(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::ModuleDeclaration | SyntaxKind::Identifier)
}
