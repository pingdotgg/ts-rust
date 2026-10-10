//! Port of Effect-TS/tsgo `internal/typeparser/effect_fn_opportunity.go`.

use crate::effect::typeparser::*;
use crate::prelude::*;

/// EffectFnOpportunityResult represents a function that can be converted to Effect.fn.
// Go: typeparser/effect_fn_opportunity.go EffectFnOpportunityResult
#[derive(Clone)]
pub struct EffectFnOpportunityResult {
    /// The function node being reported
    pub target_node: Node,
    /// The discovered name node for the function
    pub name_identifier: Node,
    /// Non-nil for gen opportunity, nil for regular
    pub generator_function: Node,
    /// Pipe args from piped Effect.gen (may be empty)
    pub pipe_arguments: Vec<Node>,
    /// Span name from Effect.withSpan if last pipe arg, or nil
    pub explicit_trace_expression: Node,
    /// The local function/variable name for suggested span
    pub suggested_trace_name: String,
    /// Context-aware name (e.g., "ServiceTag.member" or exported name)
    pub inferred_trace_name: String,
    /// The Effect module identifier
    pub effect_module: Node,
    /// True for gen opportunity, false for regular
    pub has_gen_body: bool,
    /// True when the target is a property value inside a Layer service definition
    pub is_layer_member: bool,
}

impl TypeParser<'_> {
    /// IsInsideEffectFn checks if a function node is already the first argument
    /// of an Effect.fn, Effect.fnGen, or Effect.fnUntraced call.
    // Go: typeparser/effect_fn_opportunity.go IsInsideEffectFn
    pub fn is_inside_effect_fn(&mut self, fn_node: Node) -> bool {
        if fn_node.is_nil() {
            return false;
        }

        let parent = fn_node.parent();
        if parent.is_nil() || parent.kind() != SyntaxKind::CallExpression {
            return false;
        }

        let parent_call = parent;
        if parent_call.argument_list().is_nil() || parent_call.arguments().is_empty() {
            return false;
        }

        // The function must be the first argument
        if parent_call.arguments().get(0) != fn_node {
            return false;
        }

        if self.effect_fn_call(parent).is_some() {
            return true;
        }

        false
    }

    /// ParseEffectFnOpportunity detects whether a function node can be converted to Effect.fn.
    /// Returns nil if the node is not an eligible candidate.
    // Go: typeparser/effect_fn_opportunity.go ParseEffectFnOpportunity
    pub fn parse_effect_fn_opportunity(
        &mut self,
        node: Node,
    ) -> Option<Rc<EffectFnOpportunityResult>> {
        if node.is_nil() {
            return None;
        }

        // Go `Cached(&tp.links.ParseEffectFnOpportunity, node, ..)`.
        if let Some(cached) = self.links().parse_effect_fn_opportunity.get(node) {
            return cached;
        }
        let result = self.parse_effect_fn_opportunity_inner(node);
        self.links()
            .parse_effect_fn_opportunity
            .insert(node, result.clone());
        result
    }

    /// parseEffectFnOpportunityInner contains the actual parsing logic for ParseEffectFnOpportunity.
    // Go: typeparser/effect_fn_opportunity.go parseEffectFnOpportunityInner
    fn parse_effect_fn_opportunity_inner(
        &mut self,
        node: Node,
    ) -> Option<Rc<EffectFnOpportunityResult>> {
        // Step 1: Filter by node kind
        match node.kind() {
            SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::FunctionDeclaration => {
                // OK
            }
            _ => return None,
        }

        // Step 2: Reject generators
        if node.kind() == SyntaxKind::FunctionExpression {
            let fn_ = node;
            if fn_.asterisk_token().is_some() {
                return None;
            }
        }
        if node.kind() == SyntaxKind::FunctionDeclaration {
            let fn_ = node;
            if fn_.asterisk_token().is_some() {
                return None;
            }
        }

        // Step 3: Reject named function expressions (typically used for recursion)
        if node.kind() == SyntaxKind::FunctionExpression {
            let fn_ = node;
            if fn_.name().is_some() {
                return None;
            }
        }

        // Step 4: Reject functions with return type annotations
        if has_return_type_annotation(node) {
            return None;
        }

        // Step 5: Reject if already inside Effect.fn
        if self.is_inside_effect_fn(node) {
            return None;
        }

        // Step 6: Get function type, require exactly 1 call signature
        let function_type = self.get_type_at_location(node);
        if function_type.is_nil() {
            return None;
        }
        let call_signatures = self
            .checker
            .get_signatures_of_type_exported(function_type, SignatureKind::CALL);
        if call_signatures.len() != 1 {
            return None;
        }

        // Step 7: Get return type, unroll union members, require all are strict Effect types
        let return_type = self
            .checker
            .get_return_type_of_signature_exported(call_signatures[0]);
        if return_type.is_nil() {
            return None;
        }
        let union_members = self.unroll_union_members(return_type);
        if union_members.is_empty() {
            return None;
        }
        for member in union_members {
            if self.strict_effect_type(member).is_none() {
                return None;
            }
        }

        // Step 8: Extract name identifier from context
        let mut name_identifier = get_name_of_declaration(node);
        // GetNameOfDeclaration doesn't cover property declaration parents for arrow/function expressions
        if name_identifier.is_nil()
            && node.parent().is_some()
            && node.parent().kind() == SyntaxKind::PropertyDeclaration
        {
            let pd = node.parent();
            name_identifier = pd.name();
        }
        // Also check PropertyAssignment parents (e.g., object literal members like { query: (...) => Effect... })
        if name_identifier.is_nil()
            && node.parent().is_some()
            && node.parent().kind() == SyntaxKind::PropertyAssignment
        {
            let pa = node.parent();
            if pa.initializer() == node {
                name_identifier = pa.name();
            }
        }
        if name_identifier.is_some()
            && name_identifier.kind() != SyntaxKind::Identifier
            && name_identifier.kind() != SyntaxKind::StringLiteral
        {
            name_identifier = Node::NIL;
        }
        if name_identifier.is_nil() {
            return None;
        }
        let trace_name = get_text_of_node(name_identifier);
        if trace_name.is_empty() {
            return None;
        }

        // Step 8b: Compute suggestedTraceName (local name) and inferredTraceName (context-aware name)
        let suggested_trace_name = trace_name;
        let inferred_trace_name = self.get_inferred_trace_name(node, &suggested_trace_name);

        // Detect whether the target function is a property value inside a Layer service definition
        let is_layer_member =
            !inferred_trace_name.is_empty() && inferred_trace_name != suggested_trace_name;

        // Converting a hoisted declaration to a const must not invalidate earlier uses.
        // Keep this in the shared parser so neither the diagnostic nor the fix is offered.
        if node.kind() == SyntaxKind::FunctionDeclaration
            && self.is_function_referenced_before_declaration(node)
        {
            return None;
        }

        // Step 9: Try gen opportunity first
        if let Some(result) = self.try_parse_gen_opportunity(node) {
            // Safety check: reject if function parameters are referenced in pipe arguments
            if !result.pipe_arguments.is_empty()
                && self.are_parameters_referenced_in(node, &result.pipe_arguments)
            {
                return None;
            }
            return Some(Rc::new(EffectFnOpportunityResult {
                target_node: node,
                name_identifier,
                generator_function: result.generator_function,
                pipe_arguments: result.pipe_arguments.clone(),
                explicit_trace_expression: result.explicit_trace_expression,
                suggested_trace_name,
                inferred_trace_name,
                effect_module: result.effect_module,
                has_gen_body: true,
                is_layer_member,
            }));
        }

        // Step 10: Try regular opportunity (block body with >5 statements, relaxed in Layer context)
        if try_parse_regular_opportunity(node, is_layer_member) {
            return Some(Rc::new(EffectFnOpportunityResult {
                target_node: node,
                name_identifier,
                generator_function: Node::NIL,
                pipe_arguments: Vec::new(),
                explicit_trace_expression: Node::NIL,
                suggested_trace_name,
                inferred_trace_name,
                effect_module: Node::NIL,
                has_gen_body: false,
                is_layer_member,
            }));
        }

        None
    }

    /// isFunctionReferencedBeforeDeclaration checks the preceding syntax in the
    /// declaration's scope. Earlier closures are conservatively included: they may
    /// be invoked before the replacement const is initialized.
    // Go: typeparser/effect_fn_opportunity.go isFunctionReferencedBeforeDeclaration
    fn is_function_referenced_before_declaration(&mut self, node: Node) -> bool {
        let name = node.name();
        if name.is_nil() || node.parent().is_nil() {
            return false;
        }
        let symbol = self.get_symbol_at_location(name);
        if symbol.is_nil() {
            return false;
        }

        let mut scope = node.parent();
        // Switch clauses share the enclosing case block's lexical scope.
        if scope.kind() == SyntaxKind::CaseClause || scope.kind() == SyntaxKind::DefaultClause {
            scope = scope.parent();
        }
        if scope.is_nil() {
            return false;
        }

        // PORT: Go's recursive `visit` closure.
        fn visit(
            tp: &mut TypeParser<'_>,
            node: Node,
            name: Node,
            symbol: SymbolId,
            current: Node,
        ) -> bool {
            if current.pos() >= node.pos()
                || is_type_node(current)
                || current.kind() == SyntaxKind::ExportDeclaration
            {
                return false;
            }
            if current.kind() == SyntaxKind::ShorthandPropertyAssignment
                && current.name().text() == name.text()
            {
                let value_symbol = tp.checker.get_shorthand_assignment_value_symbol(current);
                if same_symbol_reference(tp.checker, value_symbol, symbol) {
                    return true;
                }
            }
            if current.kind() == SyntaxKind::Identifier && current.text() == name.text() {
                let current_symbol = tp.get_symbol_at_location(current);
                if same_symbol_reference(tp.checker, current_symbol, symbol) {
                    return true;
                }
            }
            current.for_each_child(|child| visit(tp, node, name, symbol, child))
        }
        scope.for_each_child(|child| visit(self, node, name, symbol, child))
    }
}

/// hasReturnTypeAnnotation checks if a function node has an explicit return type annotation.
// Go: typeparser/effect_fn_opportunity.go hasReturnTypeAnnotation
fn has_return_type_annotation(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ArrowFunction => {
            let fn_ = node;
            if fn_.type_().is_some() {
                return true;
            }
        }
        SyntaxKind::FunctionExpression => {
            let fn_ = node;
            if fn_.type_().is_some() {
                return true;
            }
        }
        SyntaxKind::FunctionDeclaration => {
            let fn_ = node;
            if fn_.type_().is_some() {
                return true;
            }
        }
        _ => {}
    }
    false
}

/// genOpportunityResult holds the parsed gen opportunity data.
// Go: typeparser/effect_fn_opportunity.go genOpportunityResult
#[derive(Clone)]
pub struct GenOpportunityResult {
    pub effect_module: Node,
    pub generator_function: Node,
    pub pipe_arguments: Vec<Node>,
    pub explicit_trace_expression: Node,
}

impl TypeParser<'_> {
    /// tryParseGenOpportunity attempts to parse a function as a gen opportunity.
    /// The function body must contain a single return statement (or expression body for arrows)
    /// that is an Effect.gen call (possibly piped).
    // Go: typeparser/effect_fn_opportunity.go tryParseGenOpportunity
    fn try_parse_gen_opportunity(&mut self, fn_node: Node) -> Option<Rc<GenOpportunityResult>> {
        let body_expr = get_body_expression(fn_node);
        if body_expr.is_nil() {
            return None;
        }

        // Try to parse as a pipe call first to get subject and pipe args
        let subject: Node;
        let mut outer_pipe_args: Vec<Node> = Vec::new();

        if let Some(pipe_result) = self.parse_pipe_call(body_expr) {
            subject = pipe_result.subject;
            outer_pipe_args = pipe_result.args.clone();
        } else {
            subject = body_expr;
        }

        // The subject must be an Effect.gen call
        let gen_result = self.effect_gen_call(subject)?;

        // with no this binding
        if gen_result.options_node.is_some() {
            return None;
        }
        let mut pipe_args: Vec<Node> = gen_result.pipe_arguments.clone();
        pipe_args.extend(outer_pipe_args);
        for &arg in &pipe_args {
            // A spread can contain bare pipeables, but cannot itself be wrapped as
            // a unary callback. Keep the original pipe call in this case.
            if arg.kind() == SyntaxKind::SpreadElement {
                return None;
            }
        }

        // Check if the last pipe argument is Effect.withSpan
        let mut explicit_trace_expression = Node::NIL;
        if !pipe_args.is_empty() {
            let last_arg = pipe_args[pipe_args.len() - 1];
            let with_span_expr = self.try_extract_with_span_expression(last_arg);
            if with_span_expr.is_some() {
                explicit_trace_expression = with_span_expr;
            }
        }

        Some(Rc::new(GenOpportunityResult {
            effect_module: gen_result.effect_module,
            generator_function: gen_result.generator_function,
            pipe_arguments: pipe_args,
            explicit_trace_expression,
        }))
    }
}

/// getBodyExpression gets the single return expression from a function body.
/// For arrow functions with expression bodies, returns the expression directly.
/// For block bodies, requires exactly one return statement.
// Go: typeparser/effect_fn_opportunity.go getBodyExpression
fn get_body_expression(fn_node: Node) -> Node {
    match fn_node.kind() {
        SyntaxKind::ArrowFunction => {
            let fn_ = fn_node;
            if fn_.body().is_nil() {
                return Node::NIL;
            }
            if fn_.body().kind() == SyntaxKind::Block {
                return find_single_return_expression(fn_.body());
            }
            fn_.body()
        }

        SyntaxKind::FunctionExpression => {
            let fn_ = fn_node;
            if fn_.body().is_nil() {
                return Node::NIL;
            }
            find_single_return_expression(fn_.body())
        }

        SyntaxKind::FunctionDeclaration => {
            let fn_ = fn_node;
            if fn_.body().is_nil() {
                return Node::NIL;
            }
            find_single_return_expression(fn_.body())
        }
        _ => Node::NIL,
    }
}

/// findSingleReturnExpression finds the expression from a single return statement in a block.
// Go: typeparser/effect_fn_opportunity.go findSingleReturnExpression
fn find_single_return_expression(body: Node) -> Node {
    if body.is_nil() || body.kind() != SyntaxKind::Block {
        return Node::NIL;
    }
    let block = body;
    if block.statement_list().is_nil() || block.statements().len() != 1 {
        return Node::NIL;
    }
    let stmt = block.statements().get(0);
    if stmt.is_nil() || stmt.kind() != SyntaxKind::ReturnStatement {
        return Node::NIL;
    }
    stmt.expression()
}

/// tryParseRegularOpportunity checks if a function has a block body with more than 5 statements.
/// When hasStrictLayerInferredName is true, the >5 statement requirement and the block body
/// requirement for arrow functions are relaxed (any Effect-returning function in Layer context qualifies).
// Go: typeparser/effect_fn_opportunity.go tryParseRegularOpportunity
fn try_parse_regular_opportunity(fn_node: Node, has_strict_layer_inferred_name: bool) -> bool {
    match fn_node.kind() {
        SyntaxKind::ArrowFunction => {
            let fn_ = fn_node;
            if fn_.body().is_nil() {
                return false;
            }
            // Arrow with concise body (expression, no block): only allowed in Layer context
            if fn_.body().kind() != SyntaxKind::Block {
                return has_strict_layer_inferred_name;
            }
            if has_strict_layer_inferred_name {
                return true;
            }
            let block = fn_.body();
            !block.statement_list().is_nil() && block.statements().len() > 5
        }

        SyntaxKind::FunctionExpression => {
            let fn_ = fn_node;
            if fn_.body().is_nil() || fn_.body().kind() != SyntaxKind::Block {
                return false;
            }
            if has_strict_layer_inferred_name {
                return true;
            }
            let block = fn_.body();
            !block.statement_list().is_nil() && block.statements().len() > 5
        }

        SyntaxKind::FunctionDeclaration => {
            let fn_ = fn_node;
            if fn_.body().is_nil() || fn_.body().kind() != SyntaxKind::Block {
                return false;
            }
            if has_strict_layer_inferred_name {
                return true;
            }
            let block = fn_.body();
            !block.statement_list().is_nil() && block.statements().len() > 5
        }
        _ => false,
    }
}

impl TypeParser<'_> {
    /// tryExtractWithSpanExpression checks if an expression is a call to Effect.withSpan
    /// and extracts the span name expression (the first argument).
    // Go: typeparser/effect_fn_opportunity.go tryExtractWithSpanExpression
    fn try_extract_with_span_expression(&mut self, expr: Node) -> Node {
        if expr.is_nil() || expr.kind() != SyntaxKind::CallExpression {
            return Node::NIL;
        }

        let call = expr;
        if call.expression().is_nil() {
            return Node::NIL;
        }

        if !self.is_node_reference_to_effect_module_api(call.expression(), "withSpan") {
            return Node::NIL;
        }

        // withSpan has at least one argument (the span name)
        if call.argument_list().is_nil() || call.arguments().is_empty() {
            return Node::NIL;
        }

        call.arguments().get(0)
    }

    /// areParametersReferencedIn checks if any of the function's parameter symbols
    /// are referenced within the given nodes. Uses declaration position range checking.
    // Go: typeparser/effect_fn_opportunity.go areParametersReferencedIn
    fn are_parameters_referenced_in(&mut self, fn_node: Node, nodes: &[Node]) -> bool {
        if nodes.is_empty() {
            return false;
        }

        let params = get_function_parameters(fn_node);
        if params.is_empty() {
            return false;
        }

        // Get the position range of all parameters
        let first_param = params[0];
        let last_param = params[params.len() - 1];
        let params_start = first_param.pos();
        let params_end = last_param.end();

        // Walk all nodes looking for symbols declared in the function parameters
        let mut queue: Vec<Node> = Vec::with_capacity(nodes.len());
        queue.extend_from_slice(nodes);
        // PORT: Go pops the front of the slice; an index into the Vec gives the
        // same order.
        let mut head = 0;

        while head < queue.len() {
            let current = queue[head];
            head += 1;

            if current.is_nil() {
                continue;
            }

            // Check identifiers
            if current.kind() == SyntaxKind::Identifier {
                let sym = self.get_symbol_at_location(current);
                if sym.is_some()
                    && is_symbol_declared_in_range(self.checker, sym, params_start, params_end)
                {
                    return true;
                }
            }

            // Check shorthand property assignments like { a, b }
            if current.kind() == SyntaxKind::ShorthandPropertyAssignment {
                let value_sym = self.checker.get_shorthand_assignment_value_symbol(current);
                if value_sym.is_some()
                    && is_symbol_declared_in_range(
                        self.checker,
                        value_sym,
                        params_start,
                        params_end,
                    )
                {
                    return true;
                }
            }

            current.for_each_child(|child| {
                queue.push(child);
                false
            });
        }

        false
    }
}

/// isSymbolDeclaredInRange checks if any of the symbol's declarations fall within the given range.
// PORT: Go reads `sym.Declarations` directly; the symbol arena is on the checker.
// Go: typeparser/effect_fn_opportunity.go isSymbolDeclaredInRange
fn is_symbol_declared_in_range(c: &Checker, sym: SymbolId, start: i32, end: i32) -> bool {
    for &decl in c.sym(sym).declarations.iter() {
        if decl.is_some() && decl.pos() >= start && decl.end() <= end {
            return true;
        }
    }
    false
}

/// getFunctionParameters returns the parameter nodes of a function.
// Go: typeparser/effect_fn_opportunity.go getFunctionParameters
fn get_function_parameters(fn_node: Node) -> Vec<Node> {
    match fn_node.kind() {
        SyntaxKind::ArrowFunction => {
            let fn_ = fn_node;
            if !fn_.parameter_list().is_nil() {
                return fn_.parameters().to_vec();
            }
        }
        SyntaxKind::FunctionExpression => {
            let fn_ = fn_node;
            if !fn_.parameter_list().is_nil() {
                return fn_.parameters().to_vec();
            }
        }
        SyntaxKind::FunctionDeclaration => {
            let fn_ = fn_node;
            if !fn_.parameter_list().is_nil() {
                return fn_.parameters().to_vec();
            }
        }
        _ => {}
    }
    Vec::new()
}

impl TypeParser<'_> {
    /// tryGetLayerApiMethod checks if a node references Layer.effect, Layer.succeed, or Layer.sync.
    /// Returns the method name ("effect", "succeed", "sync") or empty string.
    // Go: typeparser/effect_fn_opportunity.go tryGetLayerApiMethod
    fn try_get_layer_api_method(&mut self, node: Node) -> &'static str {
        if node.is_nil() {
            return "";
        }
        if self.is_node_reference_to_effect_layer_module_api(node, "effect") {
            return "effect";
        }
        if self.is_node_reference_to_effect_layer_module_api(node, "succeed") {
            return "succeed";
        }
        if self.is_node_reference_to_effect_layer_module_api(node, "sync") {
            return "sync";
        }
        ""
    }
}

/// layerServiceNameFromExpression resolves an expression to a service name string.
/// When the expression is the `this` keyword, it walks up the AST to find the enclosing
/// class declaration and returns the class name instead of literal "this".
// Go: typeparser/effect_fn_opportunity.go layerServiceNameFromExpression
fn layer_service_name_from_expression(expr: Node) -> String {
    if expr.is_nil() {
        return String::new();
    }
    if expr.kind() == SyntaxKind::ThisKeyword {
        let class_decl = find_ancestor_kind(expr, SyntaxKind::ClassDeclaration);
        if class_decl.is_some() && class_decl.name().is_some() {
            return get_text_of_node(class_decl.name());
        }
    }
    get_text_of_node(expr)
}

impl TypeParser<'_> {
    /// verifyLayerMethodAtCall checks if a call expression is a Layer method call (direct or curried)
    /// and returns the tag text (e.g., "MyService") or empty string.
    /// Direct form: Layer.method(tag, impl) where impl === implementationExpr
    /// Curried form: Layer.method(tag)(impl) where impl === implementationExpr
    // Go: typeparser/effect_fn_opportunity.go verifyLayerMethodAtCall
    fn verify_layer_method_at_call(
        &mut self,
        call_expr: Node,
        method: &str,
        implementation_expr: Node,
    ) -> String {
        if call_expr.is_nil() || call_expr.expression().is_nil() {
            return String::new();
        }

        // Check direct form: Layer.method(tag, impl)
        let direct_method = self.try_get_layer_api_method(call_expr.expression());
        if direct_method == method
            && !call_expr.argument_list().is_nil()
            && call_expr.arguments().len() >= 2
            && call_expr.arguments().get(1) == implementation_expr
        {
            return layer_service_name_from_expression(call_expr.arguments().get(0));
        }

        // Check curried form: Layer.method(tag)(impl)
        if call_expr.expression().kind() == SyntaxKind::CallExpression {
            let inner_call = call_expr.expression();
            if inner_call.expression().is_some() {
                let inner_method = self.try_get_layer_api_method(inner_call.expression());
                if inner_method == method
                    && !inner_call.argument_list().is_nil()
                    && !inner_call.arguments().is_empty()
                    && !call_expr.argument_list().is_nil()
                    && !call_expr.arguments().is_empty()
                    && call_expr.arguments().get(0) == implementation_expr
                {
                    return layer_service_name_from_expression(inner_call.arguments().get(0));
                }
            }
        }

        String::new()
    }

    /// tryMatchLayerSucceedInference checks if an object literal is a direct argument to Layer.succeed.
    /// Returns the service tag text or empty string.
    // Go: typeparser/effect_fn_opportunity.go tryMatchLayerSucceedInference
    fn try_match_layer_succeed_inference(&mut self, object_literal: Node) -> String {
        if object_literal.is_nil()
            || object_literal.parent().is_nil()
            || object_literal.parent().kind() != SyntaxKind::CallExpression
        {
            return String::new();
        }
        let call_expr = object_literal.parent();
        self.verify_layer_method_at_call(call_expr, "succeed", object_literal)
    }

    /// tryMatchLayerSyncInference checks if an object literal is returned from a lazy function
    /// that is passed to Layer.sync.
    /// Pattern: Layer.sync(tag)(() => { return { ... } }) or Layer.sync(tag, () => { return { ... } })
    // Go: typeparser/effect_fn_opportunity.go tryMatchLayerSyncInference
    fn try_match_layer_sync_inference(&mut self, object_literal: Node) -> String {
        if object_literal.is_nil() || object_literal.parent().is_nil() {
            return String::new();
        }
        // objectLiteral -> ReturnStatement
        let return_stmt = object_literal.parent();
        if return_stmt.kind() != SyntaxKind::ReturnStatement {
            return String::new();
        }
        // ReturnStatement -> Block
        let block = return_stmt.parent();
        if block.is_nil() || block.kind() != SyntaxKind::Block {
            return String::new();
        }
        // Block -> ArrowFunction or FunctionExpression (the lazy function)
        let lazy_fn = block.parent();
        if lazy_fn.is_nil()
            || (lazy_fn.kind() != SyntaxKind::ArrowFunction
                && lazy_fn.kind() != SyntaxKind::FunctionExpression)
        {
            return String::new();
        }
        // LazyFunction -> CallExpression
        let call_node = lazy_fn.parent();
        if call_node.is_nil() || call_node.kind() != SyntaxKind::CallExpression {
            return String::new();
        }
        let call_expr = call_node;
        self.verify_layer_method_at_call(call_expr, "sync", lazy_fn)
    }

    /// tryMatchLayerEffectInference checks if an object literal is returned from a generator
    /// inside Effect.gen that is passed to Layer.effect.
    /// Pattern: Layer.effect(tag)(Effect.gen(function*() { return { ... } }))
    // Go: typeparser/effect_fn_opportunity.go tryMatchLayerEffectInference
    fn try_match_layer_effect_inference(&mut self, object_literal: Node) -> String {
        if object_literal.is_nil() || object_literal.parent().is_nil() {
            return String::new();
        }
        // objectLiteral -> ReturnStatement
        let return_stmt = object_literal.parent();
        if return_stmt.kind() != SyntaxKind::ReturnStatement {
            return String::new();
        }
        // ReturnStatement -> Block (generator body)
        let gen_body = return_stmt.parent();
        if gen_body.is_nil() || gen_body.kind() != SyntaxKind::Block {
            return String::new();
        }
        // Block -> FunctionExpression (generator function, must have asteriskToken)
        let gen_fn_node = gen_body.parent();
        if gen_fn_node.is_nil() || gen_fn_node.kind() != SyntaxKind::FunctionExpression {
            return String::new();
        }
        let gen_fn = gen_fn_node;
        if gen_fn.asterisk_token().is_nil() {
            return String::new();
        }
        // GeneratorFunction -> CallExpression (Effect.gen call)
        let gen_call_node = gen_fn_node.parent();
        if gen_call_node.is_nil() || gen_call_node.kind() != SyntaxKind::CallExpression {
            return String::new();
        }
        // Verify this is actually an Effect.gen call with our generator
        let parsed_gen = self.effect_gen_call(gen_call_node);
        match parsed_gen {
            Some(parsed_gen) if parsed_gen.generator_function == gen_fn => {}
            _ => return String::new(),
        }
        // Effect.gen(...) -> CallExpression (Layer.effect call)
        let layer_call_node = gen_call_node.parent();
        if layer_call_node.is_nil() || layer_call_node.kind() != SyntaxKind::CallExpression {
            return String::new();
        }
        let layer_call = layer_call_node;
        self.verify_layer_method_at_call(layer_call, "effect", gen_call_node)
    }

    /// tryGetLayerInferredTraceName checks if a function is inside a property assignment within
    /// an object literal that is a Layer service definition, returning "ServiceTag.memberName" format.
    // Go: typeparser/effect_fn_opportunity.go tryGetLayerInferredTraceName
    fn try_get_layer_inferred_trace_name(
        &mut self,
        node: Node,
        suggested_trace_name: &str,
    ) -> String {
        if suggested_trace_name.is_empty() || node.is_nil() || node.parent().is_nil() {
            return String::new();
        }
        // The function must be the initializer of a PropertyAssignment
        if node.parent().kind() != SyntaxKind::PropertyAssignment {
            return String::new();
        }
        let pa = node.parent();
        if pa.initializer() != node {
            return String::new();
        }
        // The PropertyAssignment must be inside an ObjectLiteralExpression
        if node.parent().parent().is_nil()
            || node.parent().parent().kind() != SyntaxKind::ObjectLiteralExpression
        {
            return String::new();
        }
        let object_literal = node.parent().parent();

        // Try each Layer pattern in order
        let service_name = self.try_match_layer_succeed_inference(object_literal);
        if !service_name.is_empty() {
            return service_name + "." + suggested_trace_name;
        }
        let service_name = self.try_match_layer_sync_inference(object_literal);
        if !service_name.is_empty() {
            return service_name + "." + suggested_trace_name;
        }
        let service_name = self.try_match_layer_effect_inference(object_literal);
        if !service_name.is_empty() {
            return service_name + "." + suggested_trace_name;
        }
        let service_name = self.try_match_of_inference(object_literal);
        if !service_name.is_empty() {
            return service_name + "." + suggested_trace_name;
        }
        let service_name = self.try_match_service_map_make_inference(object_literal);
        if !service_name.is_empty() {
            return service_name + "." + suggested_trace_name;
        }
        String::new()
    }

    /// tryMatchOfInference checks if an object literal is the first argument to a Service.of({ ... }) call,
    /// where the service expression is a ContextTag or ServiceType.
    /// Returns the service tag text or empty string.
    // Go: typeparser/effect_fn_opportunity.go tryMatchOfInference
    fn try_match_of_inference(&mut self, object_literal: Node) -> String {
        if object_literal.is_nil()
            || object_literal.parent().is_nil()
            || object_literal.parent().kind() != SyntaxKind::CallExpression
        {
            return String::new();
        }
        let call_expr = object_literal.parent();
        if call_expr.argument_list().is_nil() || call_expr.arguments().is_empty() {
            return String::new();
        }
        // objectLiteral must be the first argument
        if call_expr.arguments().get(0) != object_literal {
            return String::new();
        }
        // The call expression must be a PropertyAccessExpression with name "of"
        if call_expr.expression().is_nil()
            || call_expr.expression().kind() != SyntaxKind::PropertyAccessExpression
        {
            return String::new();
        }
        let prop_access = call_expr.expression();
        if prop_access.name().is_nil() {
            return String::new();
        }
        if get_text_of_node(prop_access.name()) != "of" {
            return String::new();
        }
        // Get the service tag expression (the object before .of)
        let service_tag_expression = prop_access.expression();
        if service_tag_expression.is_nil() {
            return String::new();
        }
        let service_tag_type = self.get_type_at_location(service_tag_expression);
        if service_tag_type.is_nil() {
            return String::new();
        }
        if !self.is_context_tag(service_tag_type) && !self.is_service_type(service_tag_type) {
            return String::new();
        }
        layer_service_name_from_expression(service_tag_expression)
    }

    /// tryMatchServiceMapMakeInference checks if an object literal is returned from a generator
    /// inside Effect.gen that is the "make" property of a class extending Context.Service.
    /// Returns the class name or empty string.
    // Go: typeparser/effect_fn_opportunity.go tryMatchServiceMapMakeInference
    fn try_match_service_map_make_inference(&mut self, object_literal: Node) -> String {
        if object_literal.is_nil() || object_literal.parent().is_nil() {
            return String::new();
        }
        // objectLiteral -> ReturnStatement
        let return_stmt = object_literal.parent();
        if return_stmt.kind() != SyntaxKind::ReturnStatement {
            return String::new();
        }
        // ReturnStatement -> Block (generator body)
        let gen_body = return_stmt.parent();
        if gen_body.is_nil() || gen_body.kind() != SyntaxKind::Block {
            return String::new();
        }
        // Block -> FunctionExpression (generator function, must have asteriskToken)
        let gen_fn_node = gen_body.parent();
        if gen_fn_node.is_nil() || gen_fn_node.kind() != SyntaxKind::FunctionExpression {
            return String::new();
        }
        let gen_fn = gen_fn_node;
        if gen_fn.asterisk_token().is_nil() {
            return String::new();
        }
        // GeneratorFunction -> CallExpression (Effect.gen call)
        let gen_call_node = gen_fn_node.parent();
        if gen_call_node.is_nil() || gen_call_node.kind() != SyntaxKind::CallExpression {
            return String::new();
        }
        // Verify this is actually an Effect.gen call with our generator
        let parsed_gen = self.effect_gen_call(gen_call_node);
        match parsed_gen {
            Some(parsed_gen) if parsed_gen.generator_function == gen_fn => {}
            _ => return String::new(),
        }
        // Effect.gen(...) -> PropertyAssignment with name "make" and initializer == genCall
        let make_property = gen_call_node.parent();
        if make_property.is_nil() || make_property.kind() != SyntaxKind::PropertyAssignment {
            return String::new();
        }
        let pa = make_property;
        if pa.initializer() != gen_call_node {
            return String::new();
        }
        if pa.name().is_nil()
            || pa.name().kind() != SyntaxKind::Identifier
            || get_text_of_node(pa.name()) != "make"
        {
            return String::new();
        }
        // Walk ancestors from PropertyAssignment.Parent to find a ClassDeclaration
        let mut current_node = make_property.parent();
        while current_node.is_some() {
            if current_node.kind() == SyntaxKind::ClassDeclaration {
                break;
            }
            current_node = current_node.parent();
        }
        if current_node.is_nil() || current_node.name().is_nil() {
            return String::new();
        }
        // Verify the class extends Context.Service
        if self.extends_context_service(current_node).is_none() {
            return String::new();
        }
        get_text_of_node(current_node.name())
    }

    /// getInferredTraceName computes a context-aware inferred trace name for a function.
    /// It checks (in priority order):
    /// 1. Layer service context (ServiceTag.memberName)
    /// 2. Exported function declarations — returns the function name
    /// 3. Exported const variable initializers — returns the variable name
    /// Returns "" if no context-aware name can be inferred.
    // Go: typeparser/effect_fn_opportunity.go getInferredTraceName
    fn get_inferred_trace_name(&mut self, node: Node, suggested_trace_name: &str) -> String {
        if suggested_trace_name.is_empty() {
            return String::new();
        }

        // Layer-based inferred trace name takes priority
        let inferred_from_layer =
            self.try_get_layer_inferred_trace_name(node, suggested_trace_name);
        if !inferred_from_layer.is_empty() {
            return inferred_from_layer;
        }

        // Check exported function declaration
        if node.kind() == SyntaxKind::FunctionDeclaration {
            if has_syntactic_modifier(node, ModifierFlags::EXPORT) {
                return suggested_trace_name.to_string();
            }
        }

        // Check exported const variable initializer
        if node.parent().is_some() && node.parent().kind() == SyntaxKind::VariableDeclaration {
            let vd = node.parent();
            if vd.initializer() == node
                && vd.name().is_some()
                && vd.name().kind() == SyntaxKind::Identifier
            {
                // Walk up: VariableDeclaration -> VariableDeclarationList -> VariableStatement
                let decl_list = node.parent().parent();
                if decl_list.is_some() && decl_list.kind() == SyntaxKind::VariableDeclarationList {
                    let var_stmt = decl_list.parent();
                    if var_stmt.is_some() && var_stmt.kind() == SyntaxKind::VariableStatement {
                        if has_syntactic_modifier(var_stmt, ModifierFlags::EXPORT)
                            && decl_list.flags().intersects(NodeFlags::CONST)
                        {
                            return suggested_trace_name.to_string();
                        }
                    }
                }
            }
        }

        String::new()
    }
}
