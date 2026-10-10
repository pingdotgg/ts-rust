//! Port of Effect-TS/tsgo `internal/typeparser/piping_flow.go`.

use crate::effect::typeparser::*;
use crate::prelude::*;

/// Go `TransformationKind` (a Go `string` type).
/// TransformationKind represents how a transformation was expressed in source code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransformationKind {
    /// Go `TransformationKindPipe` ("pipe").
    Pipe,
    /// Go `TransformationKindPipeable` ("pipeable").
    Pipeable,
    /// Go `TransformationKindDataFirst` ("dataFirst").
    DataFirst,
    /// Go `TransformationKindDataLast` ("dataLast").
    DataLast,
    /// Go `TransformationKindCall` ("call").
    Call,
    /// Go `TransformationKindEffectFn` ("effectFn").
    EffectFn,
    /// Go `TransformationKindEffectFnUntraced` ("effectFnUntraced").
    EffectFnUntraced,
}

impl TransformationKind {
    /// The Go string value of the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            TransformationKind::Pipe => "pipe",
            TransformationKind::Pipeable => "pipeable",
            TransformationKind::DataFirst => "dataFirst",
            TransformationKind::DataLast => "dataLast",
            TransformationKind::Call => "call",
            TransformationKind::EffectFn => "effectFn",
            TransformationKind::EffectFnUntraced => "effectFnUntraced",
        }
    }
}

/// Go `PipingFlowTransformation`.
/// PipingFlowTransformation represents a single transformation step in a piping flow.
#[derive(Clone, Debug)]
pub struct PipingFlowTransformation {
    /// How the transformation was expressed
    pub kind: TransformationKind,
    /// The function being applied (e.g., Effect.map)
    pub callee: Node,
    /// Explicit type arguments to the transformation call, if any
    pub type_arguments: NodeList,
    /// Arguments to the transformation, or nil for constants/single-arg calls
    pub args: Vec<Node>,
    /// The resulting type after this transformation (may be nil)
    pub out_type: TypeId,
}

/// Go `PipingFlowSubject`.
/// PipingFlowSubject is the starting expression of a piping flow.
#[derive(Clone, Copy, Debug, Default)]
pub struct PipingFlowSubject {
    /// The expression node
    pub node: Node,
    /// The type of the subject expression (may be nil)
    pub out_type: TypeId,
}

/// Go `PartialPipingFlow`.
/// PartialPipingFlow represents a logical piping flow that may not correspond to
/// a complete source expression, such as a prefix of a pipe call.
#[derive(Clone, Debug, Default)]
pub struct PartialPipingFlow {
    /// The starting expression and its type
    pub subject: PipingFlowSubject,
    /// Ordered list of transformations
    pub transformations: Vec<PipingFlowTransformation>,
}

/// Go `func(*PipingFlowSubject) bool` predicate.
// PORT: Go predicates close over the type parser; Rust closures cannot all
// hold `&mut TypeParser`, so the flow passes it in (as the main contract does
// with `&mut Checker` for `func(*Type) bool`).
pub type PipingFlowSubjectPredicate<'a> =
    dyn FnMut(&mut TypeParser<'_>, &PipingFlowSubject) -> bool + 'a;

/// Go `func(*PipingFlowTransformation) bool` predicate.
pub type PipingFlowTransformationPredicate<'a> =
    dyn FnMut(&mut TypeParser<'_>, &PipingFlowTransformation) -> bool + 'a;

impl PartialPipingFlow {
    // Go: typeparser/piping_flow.go PartialPipingFlow.MatchesPrefix
    /// MatchesPrefix checks the subject and leading transformations, allowing extra
    /// steps afterward. With no transformation predicates it checks only the subject.
    /// A nil flow returns false. Predicates must not mutate the flow.
    pub fn matches_prefix(
        &self,
        tp: &mut TypeParser<'_>,
        subject: &mut PipingFlowSubjectPredicate<'_>,
        transformations: &mut [&mut PipingFlowTransformationPredicate<'_>],
    ) -> bool {
        self.matches_shape(tp, false, subject, transformations)
    }

    // Go: typeparser/piping_flow.go PartialPipingFlow.MatchesExactly
    /// MatchesExactly checks the subject and all transformations. The number of
    /// transformations must equal the number of transformation predicates.
    /// A nil flow returns false. Predicates must not mutate the flow.
    pub fn matches_exactly(
        &self,
        tp: &mut TypeParser<'_>,
        subject: &mut PipingFlowSubjectPredicate<'_>,
        transformations: &mut [&mut PipingFlowTransformationPredicate<'_>],
    ) -> bool {
        self.matches_shape(tp, true, subject, transformations)
    }

    // Go: typeparser/piping_flow.go PartialPipingFlow.matchesShape
    pub fn matches_shape(
        &self,
        tp: &mut TypeParser<'_>,
        exact: bool,
        subject: &mut PipingFlowSubjectPredicate<'_>,
        transformations: &mut [&mut PipingFlowTransformationPredicate<'_>],
    ) -> bool {
        if self.transformations.len() < transformations.len()
            || exact && self.transformations.len() != transformations.len()
        {
            return false;
        }
        if !subject(&mut *tp, &self.subject) {
            return false;
        }
        for (i, predicate) in transformations.iter_mut().enumerate() {
            if !predicate(&mut *tp, &self.transformations[i]) {
                return false;
            }
        }
        true
    }
}

/// Go `TransformationSequenceMatch`.
/// TransformationSequenceMatch identifies consecutive steps in a piping flow.
/// Transformations is a read-only view into the original flow's slice.
// PORT: Go keeps a subslice view; the port keeps a copy of those steps.
#[derive(Clone, Debug)]
pub struct TransformationSequenceMatch {
    pub start: i32,
    pub transformations: Vec<PipingFlowTransformation>,
}

impl PartialPipingFlow {
    // Go: typeparser/piping_flow.go PartialPipingFlow.FindTransformationSequences
    /// FindTransformationSequences returns all consecutive matches in source order,
    /// including overlaps. It returns nil for a nil flow, no predicates, or no match.
    /// Predicates must not mutate the flow or depend on earlier predicate calls.
    pub fn find_transformation_sequences(
        &self,
        tp: &mut TypeParser<'_>,
        predicates: &mut [&mut PipingFlowTransformationPredicate<'_>],
    ) -> Vec<TransformationSequenceMatch> {
        if predicates.is_empty() {
            return Vec::new();
        }
        let mut matches = Vec::new();
        let mut start: i32 = 0;
        while start <= self.transformations.len() as i32 - predicates.len() as i32 {
            let mut matched = true;
            for (offset, predicate) in predicates.iter_mut().enumerate() {
                if !predicate(&mut *tp, &self.transformations[start as usize + offset]) {
                    matched = false;
                    break;
                }
            }
            if matched {
                matches.push(TransformationSequenceMatch {
                    start,
                    transformations: self.transformations
                        [start as usize..start as usize + predicates.len()]
                        .to_vec(),
                });
            }
            start += 1;
        }
        matches
    }

    // Go: typeparser/piping_flow.go PartialPipingFlow.CopyPrefix
    /// CopyPrefix returns a copy containing the first transformationCount
    /// transformations. It returns nil when transformationCount is out of bounds.
    pub fn copy_prefix(&self, transformation_count: i32) -> Option<Rc<PartialPipingFlow>> {
        if transformation_count < 0 || transformation_count as usize > self.transformations.len() {
            return None;
        }
        let transformations = self.transformations[..transformation_count as usize].to_vec();
        Some(Rc::new(PartialPipingFlow {
            subject: self.subject,
            transformations,
        }))
    }

    // Go: typeparser/piping_flow.go PartialPipingFlow.TransformationInputType
    /// TransformationInputType returns the type immediately before the indexed step.
    /// Missing types and indices outside the flow return nil.
    pub fn transformation_input_type(&self, index: i32) -> TypeId {
        if index < 0 || index as usize >= self.transformations.len() {
            return TypeId::NIL;
        }
        if index == 0 {
            return self.subject.out_type;
        }
        self.transformations[index as usize - 1].out_type
    }

    // Go: typeparser/piping_flow.go PartialPipingFlow.TransformationInputNode
    /// TransformationInputNode returns the standalone source expression that feeds
    /// the transformation at index, when that expression is represented directly
    /// in the syntax tree.
    pub fn transformation_input_node(&self, index: i32) -> Node {
        if index < 0 || index as usize >= self.transformations.len() {
            return Node::NIL;
        }
        let transformation = &self.transformations[index as usize];
        let mut call = piping_flow_transformation_call(Some(transformation));
        if transformation.kind == TransformationKind::Call {
            // Curried factories decompose into a factory call and a trailing
            // application; the input feeds the outermost applied call.
            call = applied_call_of(call);
        }
        if call.is_some() && !call.argument_list().is_nil() {
            let arguments = call.arguments();
            match transformation.kind {
                TransformationKind::Call => {
                    if arguments.len() == 1 {
                        return arguments.get(0);
                    }
                }
                TransformationKind::DataFirst => {
                    if !arguments.is_empty() {
                        return arguments.get(0);
                    }
                }
                TransformationKind::DataLast => {
                    if !arguments.is_empty() {
                        return arguments.get(arguments.len() - 1);
                    }
                }
                _ => {}
            }
        }
        if index == 0 {
            return self.subject.node;
        }
        Node::NIL
    }
}

// Go: typeparser/piping_flow.go appliedCallOf
/// appliedCallOf walks out of callee positions, returning the outermost call
/// that applies the given call's result to its subject.
pub fn applied_call_of(call: Node) -> Node {
    if call.is_nil() {
        return Node::NIL;
    }
    let mut current = call;
    while current.is_some() && current.parent().is_some() && is_call_expression(current.parent()) {
        let parent = current.parent();
        if parent.is_nil() || parent.expression() != current {
            break;
        }
        current = parent;
    }
    if current.is_nil() {
        return Node::NIL;
    }
    current
}

// Go: typeparser/piping_flow.go pipingFlowTransformationCall
pub fn piping_flow_transformation_call(transformation: Option<&PipingFlowTransformation>) -> Node {
    let Some(transformation) = transformation else {
        return Node::NIL;
    };
    if transformation.callee.is_nil()
        || transformation.callee.parent().is_nil()
        || !is_call_expression(transformation.callee.parent())
    {
        return Node::NIL;
    }
    let call = transformation.callee.parent();
    if call.is_nil() || call.expression() != transformation.callee {
        return Node::NIL;
    }
    call
}

/// Go `PipingFlow`.
/// PipingFlow represents a complete piping flow rooted at a source expression.
// PORT: Go embeds `PartialPipingFlow`; the port nests it as
// `partial_piping_flow`, and `Deref` gives Go's promoted fields and methods.
#[derive(Clone, Debug, Default)]
pub struct PipingFlow {
    /// The outermost expression encompassing the entire flow
    pub node: Node,
    pub partial_piping_flow: PartialPipingFlow,
}

impl std::ops::Deref for PipingFlow {
    type Target = PartialPipingFlow;
    fn deref(&self) -> &PartialPipingFlow {
        &self.partial_piping_flow
    }
}

impl std::ops::DerefMut for PipingFlow {
    fn deref_mut(&mut self) -> &mut PartialPipingFlow {
        &mut self.partial_piping_flow
    }
}

// PORT: Go defines these `*PipingFlow` methods only to accept a nil
// receiver. A Rust `PipingFlow` is never nil (Go nil is `None` of
// `Option<Rc<PipingFlow>>`), so each forwards to the embedded flow.
impl PipingFlow {
    // Go: typeparser/piping_flow.go PipingFlow.MatchesPrefix
    /// MatchesPrefix checks the subject and leading transformations, allowing extra steps.
    /// It is defined explicitly to support a nil *PipingFlow.
    pub fn matches_prefix(
        &self,
        tp: &mut TypeParser<'_>,
        subject: &mut PipingFlowSubjectPredicate<'_>,
        transformations: &mut [&mut PipingFlowTransformationPredicate<'_>],
    ) -> bool {
        self.partial_piping_flow
            .matches_prefix(tp, subject, transformations)
    }

    // Go: typeparser/piping_flow.go PipingFlow.MatchesExactly
    /// MatchesExactly checks the subject and all transformations, requiring an exact length.
    /// It is defined explicitly to support a nil *PipingFlow.
    pub fn matches_exactly(
        &self,
        tp: &mut TypeParser<'_>,
        subject: &mut PipingFlowSubjectPredicate<'_>,
        transformations: &mut [&mut PipingFlowTransformationPredicate<'_>],
    ) -> bool {
        self.partial_piping_flow
            .matches_exactly(tp, subject, transformations)
    }

    // Go: typeparser/piping_flow.go PipingFlow.FindTransformationSequences
    /// FindTransformationSequences returns all consecutive matches, including overlaps.
    /// It is defined explicitly to support a nil *PipingFlow.
    pub fn find_transformation_sequences(
        &self,
        tp: &mut TypeParser<'_>,
        predicates: &mut [&mut PipingFlowTransformationPredicate<'_>],
    ) -> Vec<TransformationSequenceMatch> {
        self.partial_piping_flow
            .find_transformation_sequences(tp, predicates)
    }

    // Go: typeparser/piping_flow.go PipingFlow.CopyPrefix
    /// CopyPrefix returns a partial copy containing the first transformationCount
    /// transformations. It is defined explicitly to support a nil *PipingFlow.
    pub fn copy_prefix(&self, transformation_count: i32) -> Option<Rc<PartialPipingFlow>> {
        self.partial_piping_flow.copy_prefix(transformation_count)
    }

    // Go: typeparser/piping_flow.go PipingFlow.TransformationInputType
    /// TransformationInputType returns the type immediately before the indexed step.
    pub fn transformation_input_type(&self, index: i32) -> TypeId {
        self.partial_piping_flow.transformation_input_type(index)
    }

    // Go: typeparser/piping_flow.go PipingFlow.TransformationInputNode
    /// TransformationInputNode returns the source expression that feeds the
    /// transformation at index.
    pub fn transformation_input_node(&self, index: i32) -> Node {
        self.partial_piping_flow.transformation_input_node(index)
    }
}

/// Go `ParsedPipeCallResult`.
/// ParsedPipeCallResult is the result of parsing a pipe or pipeable call.
#[derive(Clone, Debug)]
pub struct ParsedPipeCallResult {
    pub node: Node,
    pub subject: Node,
    pub args: Vec<Node>,
    pub kind: TransformationKind,
    pub subject_type: TypeId,
    pub args_out_type: Vec<TypeId>,
}

/// Go `parsedSingleArgCallResult`.
/// parsedSingleArgCallResult is the internal result of parsing a single-argument call.
#[derive(Clone, Debug)]
pub struct ParsedSingleArgCallResult {
    /// the applied call
    pub node: Node,
    /// the applied function, decomposed for curried factories
    pub callee: Node,
    /// curried factory arguments, or nil
    pub args: Vec<Node>,
    /// the node carrying the transformation type arguments
    pub type_arguments_src: Node,
    pub subject: Node,
}

impl TypeParser<'_> {
    // Go: typeparser/piping_flow.go TypeParser.ParsePipeCall
    /// ParsePipeCall detects pipe() and .pipe() call patterns.
    /// Returns nil when the node is not a recognized pipe call.
    pub fn parse_pipe_call(&mut self, node: Node) -> Option<Rc<ParsedPipeCallResult>> {
        if node.is_nil() || node.kind() != SyntaxKind::CallExpression {
            return None;
        }

        cached!(self, parse_pipe_call, node, 'compute: {
            let call = node;
            if call.expression().is_nil() {
                break 'compute None;
            }

            // Case 1: PropertyAccessExpression — either Namespace.pipe(...) or expr.pipe(...)
            if call.expression().kind() == SyntaxKind::PropertyAccessExpression {
                let prop_access = call.expression();
                if prop_access.name().is_nil() {
                    break 'compute None;
                }

                let name_text = get_text_of_node(prop_access.name());
                if name_text != "pipe" {
                    break 'compute None;
                }

                // Check if this is Function.pipe from "effect" package
                if self.is_node_reference_to_effect_package_export(call.expression(), "pipe") {
                    // This is pipe(subject, f1, f2, ...) via namespace access (e.g., Function.pipe)
                    if call.argument_list().is_nil() || call.arguments().is_empty() {
                        break 'compute None;
                    }
                    let arguments = call.arguments().to_vec();
                    break 'compute Some(self.build_parsed_pipe_call_result(
                        call,
                        arguments[0],
                        &arguments[1..],
                        TransformationKind::Pipe,
                    ));
                }

                // Not from "effect" package — this is a .pipe() pipeable method call
                // Any .pipe() call is treated as a pipeable chain
                let subject = prop_access.expression();
                if subject.is_nil() {
                    break 'compute None;
                }
                let mut args: Vec<Node> = Vec::new();
                if !call.argument_list().is_nil() {
                    args = call.arguments().to_vec();
                }
                break 'compute Some(self.build_parsed_pipe_call_result(
                    call,
                    subject,
                    &args,
                    TransformationKind::Pipeable,
                ));
            }

            // Case 2: Identifier — bare pipe(subject, f1, f2, ...)
            if call.expression().kind() == SyntaxKind::Identifier {
                if !self.is_node_reference_to_effect_package_export(call.expression(), "pipe") {
                    break 'compute None;
                }

                if call.argument_list().is_nil() || call.arguments().is_empty() {
                    break 'compute None;
                }
                let arguments = call.arguments().to_vec();
                break 'compute Some(self.build_parsed_pipe_call_result(
                    call,
                    arguments[0],
                    &arguments[1..],
                    TransformationKind::Pipe,
                ));
            }

            None
        })
    }

    // Go: typeparser/piping_flow.go TypeParser.buildParsedPipeCallResult
    pub fn build_parsed_pipe_call_result(
        &mut self,
        call: Node,
        subject: Node,
        args: &[Node],
        kind: TransformationKind,
    ) -> Rc<ParsedPipeCallResult> {
        let subject_type = self.get_type_at_location(subject);
        let mut result = ParsedPipeCallResult {
            node: call,
            subject,
            args: args.to_vec(),
            kind,
            subject_type,
            args_out_type: vec![TypeId::NIL; args.len()],
        };

        let sig = self.checker.get_resolved_signature_exported(call);
        if sig.is_nil() {
            return Rc::new(result);
        }

        let type_args = get_type_arguments_for_resolved_signature(self.checker, sig);
        for i in 0..args.len() {
            if i + 1 < type_args.len() {
                result.args_out_type[i] = type_args[i + 1];
            }
        }

        Rc::new(result)
    }

    // Go: typeparser/piping_flow.go TypeParser.parseSingleArgCall
    /// parseSingleArgCall detects single-argument call patterns like f(arg).
    /// Returns nil when the node is not a single-argument call.
    pub fn parse_single_arg_call(&mut self, node: Node) -> Option<Rc<ParsedSingleArgCallResult>> {
        if node.is_nil() || node.kind() != SyntaxKind::CallExpression {
            return None;
        }

        let call = node;
        if call.expression().is_nil() || call.argument_list().is_nil() {
            return None;
        }

        if call.arguments().len() != 1 {
            return None;
        }

        // Spread elements like f(...args) should not be treated as single-arg calls
        if call.arguments().get(0).kind() == SyntaxKind::SpreadElement {
            return None;
        }

        let mut result = ParsedSingleArgCallResult {
            node: call,
            callee: call.expression(),
            args: Vec::new(),
            type_arguments_src: call,
            subject: call.arguments().get(0),
        };
        self.decompose_curried_pipeable(Some(&mut result));
        Some(Rc::new(result))
    }

    // Go: typeparser/piping_flow.go TypeParser.decomposeCurriedPipeable
    /// decomposeCurriedPipeable exposes the factory arguments only when the inner
    /// call is the pipeable counterpart of a data-first overload of the same symbol.
    pub fn decompose_curried_pipeable(&mut self, result: Option<&mut ParsedSingleArgCallResult>) {
        let Some(result) = result else {
            return;
        };
        if result.node.is_nil() {
            return;
        }
        let factory = skip_parentheses(result.node.expression());
        if factory.is_nil() || factory.kind() != SyntaxKind::CallExpression {
            return;
        }
        let factory_call = factory;
        if factory_call.expression().is_nil()
            || factory_call.argument_list().is_nil()
            || factory_call.arguments().is_empty()
        {
            return;
        }

        let pipeable = self.checker.get_resolved_signature_exported(factory);
        if pipeable.is_nil() {
            return;
        }
        let pipeable_declaration = {
            let raw = raw_signature(self.checker, pipeable);
            self.checker.sig(raw).declaration()
        };
        if pipeable_declaration.is_nil() {
            return;
        }
        let pipeable_symbol = self.checker.get_symbol_of_declaration(pipeable_declaration);
        if pipeable_symbol.is_nil() {
            return;
        }

        let callee_type = self.get_type_at_location(factory_call.expression());
        let subject_type = self.get_type_at_location(result.subject);
        if callee_type.is_nil() || subject_type.is_nil() {
            return;
        }
        let factory_arguments = factory_call.arguments().to_vec();
        let mut argument_types: Vec<TypeId> = Vec::with_capacity(factory_arguments.len());
        for &argument in &factory_arguments {
            argument_types.push(self.get_type_at_location(argument));
        }
        let witness = PipeableSignatureWitness {
            argument_types,
            subject_type,
        };

        let data_firsts = self
            .checker
            .get_signatures_of_type_exported(callee_type, SignatureKind::CALL);
        for data_first in data_firsts {
            if data_first.is_nil() || data_first == pipeable {
                continue;
            }
            let declaration = {
                let raw = raw_signature(self.checker, data_first);
                self.checker.sig(raw).declaration()
            };
            if declaration.is_nil() {
                continue;
            }
            let symbol = self.checker.get_symbol_of_declaration(declaration);
            if symbol.is_nil()
                || self
                    .checker
                    .get_symbol_if_same_reference(pipeable_symbol, symbol)
                    .is_nil()
            {
                continue;
            }
            let parameters_len = self.checker.sig(data_first).parameters().len() as i32;
            for subject_index in [0, parameters_len - 1] {
                if matches_pipeable_signature(
                    self.checker,
                    data_first,
                    pipeable,
                    subject_index,
                    Some(&witness),
                ) {
                    result.callee = factory_call.expression();
                    result.args = factory_arguments;
                    result.type_arguments_src = factory;
                    return;
                }
            }
        }
    }
}

/// Go `parsedEffectFnCallResult`.
/// parsedEffectFnCallResult is the internal result of parsing an Effect.fn or Effect.fnUntraced call
/// with trailing transformation arguments.
#[derive(Clone, Debug)]
pub struct ParsedEffectFnCallResult {
    /// the outer call expression
    pub node: Node,
    /// function or generator argument node
    pub body_node: Node,
    /// arguments after the function body
    pub trailing_args: Vec<Node>,
    pub trailing_args_out_type: Vec<TypeId>,
    /// starting arg index of trailingArgs in node.Arguments
    pub trailing_start_index: i32,
    /// effectFn or effectFnUntraced
    pub kind: TransformationKind,
}

impl TypeParser<'_> {
    // Go: typeparser/piping_flow.go TypeParser.parseEffectFnCall
    /// parseEffectFnCall detects Effect.fn-family calls with trailing transformation arguments.
    /// It reuses the dedicated Effect.fn parsers and only adapts their results for piping-flow analysis.
    pub fn parse_effect_fn_call(&mut self, node: Node) -> Option<Rc<ParsedEffectFnCallResult>> {
        if node.is_nil() || node.kind() != SyntaxKind::CallExpression {
            return None;
        }

        let call = node;
        if call.expression().is_nil()
            || call.argument_list().is_nil()
            || call.arguments().is_empty()
        {
            return None;
        }
        if let Some(result) = self.effect_fn_call(node)
            && !result.pipe_arguments.is_empty()
        {
            let mut kind = TransformationKind::EffectFn;
            if result.variant == EffectFnVariant::FnUntraced
                || result.variant == EffectFnVariant::FnUntracedEager
            {
                kind = TransformationKind::EffectFnUntraced;
            }
            return Some(Rc::new(ParsedEffectFnCallResult {
                node: result.call,
                body_node: result.function_node,
                trailing_args: result.pipe_arguments.clone(),
                trailing_args_out_type: result.pipe_args_out_type.clone(),
                trailing_start_index: result.call.arguments().len() as i32
                    - result.pipe_arguments.len() as i32,
                kind,
            }));
        }
        None
    }
}

/// Go `workItem`.
/// workItem represents a node to process in the PipingFlows work queue.
#[derive(Clone)]
pub struct WorkItem {
    pub node: Node,
    /// non-nil when traversing subject chain for flattening
    pub parent_flow: Option<Rc<RefCell<PipingFlow>>>,
}

/// Go `enqueueChild` closure of `PipingFlows`.
fn enqueue_child(queue: &mut Vec<WorkItem>, child: Node) -> bool {
    queue.push(WorkItem {
        node: child,
        parent_flow: None,
    });
    false
}

impl TypeParser<'_> {
    // Go: typeparser/piping_flow.go TypeParser.PipingFlows
    /// PipingFlows returns all piping flows found in a source file, sorted by source position.
    /// Each source transformation occurrence belongs to at most one returned flow.
    pub fn piping_flows(&mut self, sf: Node, include_effect_fn: bool) -> Rc<Vec<Rc<PipingFlow>>> {
        if sf.is_nil() {
            return Rc::new(Vec::new());
        }

        if include_effect_fn {
            cached!(self, piping_flows_with_effect_fn, sf, {
                self.compute_piping_flows(sf, include_effect_fn)
            })
        } else {
            cached!(self, piping_flows_without_effect_fn, sf, {
                self.compute_piping_flows(sf, include_effect_fn)
            })
        }
    }

    /// The `Cached` compute function of Go `PipingFlows`.
    // PORT: Go picks the cache store (`store := &links.PipingFlowsWithoutEffectFn`
    // or `&links.PipingFlowsWithEffectFn`) and passes one closure; the
    // `cached!` macro names the store field, so the closure body is this
    // function. Go mutates the parent flow in place while it walks the
    // subject chain: `Rc<RefCell<PipingFlow>>` until the result is sorted.
    fn compute_piping_flows(
        &mut self,
        sf: Node,
        include_effect_fn: bool,
    ) -> Rc<Vec<Rc<PipingFlow>>> {
        let mut result: Vec<Rc<RefCell<PipingFlow>>> = Vec::new();

        // Initialize work queue with all children of the source file
        let mut queue: Vec<WorkItem> = Vec::new();
        sf.for_each_child(|child| enqueue_child(&mut queue, child));

        while let Some(item) = queue.pop() {
            // Pop from end (stack behavior for depth-first)
            let node = item.node;
            if node.is_nil() {
                continue;
            }
            let unwrapped = skip_parentheses(node);
            if unwrapped != node {
                queue.push(WorkItem {
                    node: unwrapped,
                    parent_flow: item.parent_flow.clone(),
                });
                continue;
            }

            if node.kind() == SyntaxKind::CallExpression {
                // Try Effect.fn call first (must be before pipe and singleArg)
                if include_effect_fn && let Some(efn_result) = self.parse_effect_fn_call(node) {
                    let (transformations, subject_type) =
                        self.build_effect_fn_transformations(&efn_result);
                    let flow = PipingFlow {
                        node,
                        partial_piping_flow: PartialPipingFlow {
                            subject: PipingFlowSubject {
                                node,
                                out_type: subject_type,
                            },
                            transformations,
                        },
                    };
                    result.push(Rc::new(RefCell::new(flow)));

                    // If we were building a parent flow, finalize it with this node as subject
                    if let Some(parent_flow) = &item.parent_flow {
                        let out_type = self.get_type_at_location(node);
                        parent_flow.borrow_mut().partial_piping_flow.subject =
                            PipingFlowSubject { node, out_type };
                        result.push(parent_flow.clone());
                    }

                    // Queue function body argument children for independent inner flow traversal
                    if efn_result.body_node.is_some() {
                        efn_result
                            .body_node
                            .for_each_child(|child| enqueue_child(&mut queue, child));
                    }
                    // Queue trailing arg children for independent inner flow traversal
                    for &arg in &efn_result.trailing_args {
                        if arg.is_some() {
                            arg.for_each_child(|child| enqueue_child(&mut queue, child));
                        }
                    }
                    continue;
                }

                // Try pipe call
                if let Some(pipe_result) = self.parse_pipe_call(node) {
                    let transformations = self.build_pipe_transformations(&pipe_result);
                    let flow_node = pipe_result.node;

                    if let Some(parent_flow) = &item.parent_flow {
                        // Extend parent flow: prepend our transformations
                        {
                            let mut parent = parent_flow.borrow_mut();
                            let mut prepended = transformations;
                            prepended.append(&mut parent.partial_piping_flow.transformations);
                            parent.partial_piping_flow.transformations = prepended;
                        }
                        // Continue traversing the subject for further flattening
                        queue.push(WorkItem {
                            node: pipe_result.subject,
                            parent_flow: Some(parent_flow.clone()),
                        });
                    } else {
                        // Start a new flow
                        let new_flow = PipingFlow {
                            node: flow_node,
                            partial_piping_flow: PartialPipingFlow {
                                subject: PipingFlowSubject::default(),
                                transformations,
                            },
                        };
                        queue.push(WorkItem {
                            node: pipe_result.subject,
                            parent_flow: Some(Rc::new(RefCell::new(new_flow))),
                        });
                    }

                    // Queue transformation argument children for independent inner flow traversal
                    for &arg in &pipe_result.args {
                        if arg.is_some() {
                            arg.for_each_child(|child| enqueue_child(&mut queue, child));
                        }
                    }
                    continue;
                }

                // Try single-arg call
                if let Some(data_first_result) = self.data_first_or_last_call(node) {
                    let call_out_type = self.get_type_at_location(node);
                    let mut kind = TransformationKind::DataFirst;
                    if data_first_result.subject_index != 0 {
                        kind = TransformationKind::DataLast;
                    }
                    let transformation = PipingFlowTransformation {
                        kind,
                        callee: data_first_result.callee,
                        type_arguments: piping_flow_type_arguments(node, data_first_result.callee),
                        args: data_first_result.args.clone(),
                        out_type: call_out_type,
                    };

                    if let Some(parent_flow) = &item.parent_flow {
                        parent_flow
                            .borrow_mut()
                            .partial_piping_flow
                            .transformations
                            .insert(0, transformation);
                        queue.push(WorkItem {
                            node: data_first_result.subject,
                            parent_flow: Some(parent_flow.clone()),
                        });
                    } else {
                        let new_flow = PipingFlow {
                            node,
                            partial_piping_flow: PartialPipingFlow {
                                subject: PipingFlowSubject::default(),
                                transformations: vec![transformation],
                            },
                        };
                        queue.push(WorkItem {
                            node: data_first_result.subject,
                            parent_flow: Some(Rc::new(RefCell::new(new_flow))),
                        });
                    }

                    data_first_result
                        .callee
                        .for_each_child(|child| enqueue_child(&mut queue, child));
                    for &arg in &data_first_result.args {
                        if arg.is_some() {
                            arg.for_each_child(|child| enqueue_child(&mut queue, child));
                        }
                    }
                    continue;
                }

                // Try single-arg call
                if let Some(single_result) = self.parse_single_arg_call(node) {
                    let mut call_out_type = TypeId::NIL;
                    let call_sig = self.checker.get_resolved_signature_exported(node);
                    if call_sig.is_some() {
                        call_out_type =
                            self.checker.get_return_type_of_signature_exported(call_sig);
                    }
                    let transformation = PipingFlowTransformation {
                        kind: TransformationKind::Call,
                        callee: single_result.callee,
                        type_arguments: piping_flow_type_arguments(
                            single_result.type_arguments_src,
                            single_result.callee,
                        ),
                        args: single_result.args.clone(),
                        out_type: call_out_type,
                    };

                    if let Some(parent_flow) = &item.parent_flow {
                        // Extend parent flow: prepend this transformation
                        parent_flow
                            .borrow_mut()
                            .partial_piping_flow
                            .transformations
                            .insert(0, transformation);
                        // Continue traversing the subject
                        queue.push(WorkItem {
                            node: single_result.subject,
                            parent_flow: Some(parent_flow.clone()),
                        });
                    } else {
                        // Start a new flow
                        let new_flow = PipingFlow {
                            node,
                            partial_piping_flow: PartialPipingFlow {
                                subject: PipingFlowSubject::default(),
                                transformations: vec![transformation],
                            },
                        };
                        queue.push(WorkItem {
                            node: single_result.subject,
                            parent_flow: Some(Rc::new(RefCell::new(new_flow))),
                        });
                    }

                    // Queue callee children for independent inner flow traversal
                    if !single_result.args.is_empty() {
                        // Curried factory: the applied function and the factory
                        // arguments belong to this transformation's subtree.
                        enqueue_child(&mut queue, single_result.callee);
                        for &arg in &single_result.args {
                            if arg.is_some() {
                                enqueue_child(&mut queue, arg);
                            }
                        }
                    } else {
                        single_result
                            .callee
                            .for_each_child(|child| enqueue_child(&mut queue, child));
                    }
                    continue;
                }
            }

            // Node is not a parseable pipe/call
            if let Some(parent_flow) = &item.parent_flow {
                // Subject chain terminated — finalize the flow
                let out_type = self.get_type_at_location(node);
                parent_flow.borrow_mut().partial_piping_flow.subject =
                    PipingFlowSubject { node, out_type };
                result.push(parent_flow.clone());
            }
            // Queue children for further traversal (independent inner flows)
            node.for_each_child(|child| enqueue_child(&mut queue, child));
        }

        // Sort by source position
        crate::gostd::slices::sort_slice(&mut result, |a, b| {
            a.borrow().node.pos() < b.borrow().node.pos()
        });

        Rc::new(
            result
                .into_iter()
                .map(|flow| {
                    Rc::new(
                        Rc::try_unwrap(flow)
                            .map(RefCell::into_inner)
                            .unwrap_or_else(|flow| flow.borrow().clone()),
                    )
                })
                .collect(),
        )
    }

    // Go: typeparser/piping_flow.go TypeParser.LongestPipingFlowAt
    /// LongestPipingFlowAt returns the longest normalized piping flow rooted at node.
    /// Unlike PipingFlows, which discovers complete flows across a source file, this
    /// method can start at an expression nested inside another flow without returning
    /// the enclosing flow.
    pub fn longest_piping_flow_at(
        &mut self,
        node: Node,
        include_effect_fn: bool,
    ) -> Option<Rc<PipingFlow>> {
        if node.is_nil() || !is_expression(node) {
            return None;
        }

        let node = skip_parentheses(node);
        if include_effect_fn && let Some(result) = self.parse_effect_fn_call(node) {
            let (transformations, subject_type) = self.build_effect_fn_transformations(&result);
            return Some(Rc::new(PipingFlow {
                node,
                partial_piping_flow: PartialPipingFlow {
                    subject: PipingFlowSubject {
                        node,
                        out_type: subject_type,
                    },
                    transformations,
                },
            }));
        }

        if let Some(result) = self.parse_pipe_call(node) {
            let mut flow = self.piping_flow_subject_at(result.subject, include_effect_fn);
            let pipe_transformations = self.build_pipe_transformations(&result);
            let flow_mut = Rc::make_mut(&mut flow);
            flow_mut.node = node;
            flow_mut
                .partial_piping_flow
                .transformations
                .extend(pipe_transformations);
            return Some(flow);
        }

        if let Some(result) = self.data_first_or_last_call(node) {
            let mut flow = self.piping_flow_subject_at(result.subject, include_effect_fn);
            let mut kind = TransformationKind::DataFirst;
            if result.subject_index != 0 {
                kind = TransformationKind::DataLast;
            }
            let out_type = self.get_type_at_location(node);
            let flow_mut = Rc::make_mut(&mut flow);
            flow_mut.node = node;
            flow_mut
                .partial_piping_flow
                .transformations
                .push(PipingFlowTransformation {
                    kind,
                    callee: result.callee,
                    type_arguments: piping_flow_type_arguments(node, result.callee),
                    args: result.args.clone(),
                    out_type,
                });
            return Some(flow);
        }

        if let Some(result) = self.parse_single_arg_call(node) {
            let mut flow = self.piping_flow_subject_at(result.subject, include_effect_fn);
            let mut out_type = TypeId::NIL;
            let signature = self.checker.get_resolved_signature_exported(node);
            if signature.is_some() {
                out_type = self
                    .checker
                    .get_return_type_of_signature_exported(signature);
            }
            let flow_mut = Rc::make_mut(&mut flow);
            flow_mut.node = node;
            flow_mut
                .partial_piping_flow
                .transformations
                .push(PipingFlowTransformation {
                    kind: TransformationKind::Call,
                    callee: result.callee,
                    type_arguments: piping_flow_type_arguments(
                        result.type_arguments_src,
                        result.callee,
                    ),
                    args: result.args.clone(),
                    out_type,
                });
            return Some(flow);
        }

        let out_type = self.get_type_at_location(node);
        Some(Rc::new(PipingFlow {
            node,
            partial_piping_flow: PartialPipingFlow {
                subject: PipingFlowSubject { node, out_type },
                transformations: Vec::new(),
            },
        }))
    }

    // Go: typeparser/piping_flow.go TypeParser.pipingFlowSubjectAt
    // PORT: Go never returns nil here, so the port returns the flow itself.
    // The caller changes it with `Rc::make_mut` (one owner, no copy).
    pub fn piping_flow_subject_at(
        &mut self,
        node: Node,
        include_effect_fn: bool,
    ) -> Rc<PipingFlow> {
        if let Some(flow) = self.longest_piping_flow_at(node, include_effect_fn) {
            return flow;
        }
        let out_type = self.get_type_at_location(node);
        Rc::new(PipingFlow {
            node,
            partial_piping_flow: PartialPipingFlow {
                subject: PipingFlowSubject { node, out_type },
                transformations: Vec::new(),
            },
        })
    }

    // Go: typeparser/piping_flow.go TypeParser.buildPipeTransformations
    /// buildPipeTransformations builds PipingFlowTransformation slices from pipe call arguments.
    pub fn build_pipe_transformations(
        &mut self,
        result: &ParsedPipeCallResult,
    ) -> Vec<PipingFlowTransformation> {
        let mut transformations = Vec::with_capacity(result.args.len());
        for (i, &arg) in result.args.iter().enumerate() {
            if arg.is_nil() {
                continue;
            }

            let mut out_type = TypeId::NIL;
            if i < result.args_out_type.len() {
                out_type = result.args_out_type[i];
            }

            let callee: Node;
            let mut args: Vec<Node> = Vec::new();

            if arg.kind() == SyntaxKind::CallExpression {
                let call_expr = arg;
                callee = call_expr.expression();
                if !call_expr.argument_list().is_nil() && !call_expr.arguments().is_empty() {
                    args = call_expr.arguments().to_vec();
                }
            } else {
                // Constant (e.g., Effect.asVoid used as a bare reference)
                callee = arg;
            }

            transformations.push(PipingFlowTransformation {
                kind: result.kind,
                callee,
                type_arguments: piping_flow_type_arguments(arg, callee),
                args,
                out_type,
            });
        }

        transformations
    }

    // Go: typeparser/piping_flow.go TypeParser.buildEffectFnTransformations
    /// buildEffectFnTransformations builds PipingFlowTransformation slices from Effect.fn trailing arguments.
    /// It also returns the subject type (from the first transformation's input parameter type).
    pub fn build_effect_fn_transformations(
        &mut self,
        result: &ParsedEffectFnCallResult,
    ) -> (Vec<PipingFlowTransformation>, TypeId) {
        let mut transformations = Vec::with_capacity(result.trailing_args.len());
        let mut subject_type = TypeId::NIL;

        let call_node = result.node;

        for (i, &arg) in result.trailing_args.iter().enumerate() {
            if arg.is_nil() {
                continue;
            }

            // Get the contextual type of the argument within the Effect.fn call.
            let arg_index = result.trailing_start_index + i as i32;
            let contextual_type = self
                .checker
                .get_contextual_type_for_argument_at_index_exported(call_node, arg_index);

            let mut out_type = TypeId::NIL;
            if i < result.trailing_args_out_type.len() {
                out_type = result.trailing_args_out_type[i];
            }
            if contextual_type.is_some() {
                let call_sigs = self
                    .checker
                    .get_signatures_of_type_exported(contextual_type, SignatureKind::CALL);
                if !call_sigs.is_empty() {
                    // For the first transformation, extract the subject type from the first parameter
                    if i == 0 {
                        let params = self.checker.sig(call_sigs[0]).parameters().to_vec();
                        if !params.is_empty() {
                            subject_type = self.checker.get_type_of_symbol_exported(params[0]);
                        }
                    }
                }
            }

            let callee: Node;
            let mut args: Vec<Node> = Vec::new();

            if arg.kind() == SyntaxKind::CallExpression {
                let call_expr = arg;
                callee = call_expr.expression();
                if !call_expr.argument_list().is_nil() && !call_expr.arguments().is_empty() {
                    args = call_expr.arguments().to_vec();
                }
            } else {
                callee = arg;
            }

            transformations.push(PipingFlowTransformation {
                kind: result.kind,
                callee,
                type_arguments: piping_flow_type_arguments(arg, callee),
                args,
                out_type,
            });
        }

        (transformations, subject_type)
    }
}

// Go: typeparser/piping_flow.go callTypeArguments
pub fn call_type_arguments(node: Node) -> NodeList {
    // Parenthesized call arguments (e.g. pipe(x, (Effect.as<...>(v)))) keep the
    // type arguments of the wrapped call.
    let node = skip_parentheses(node);
    if node.is_nil() || node.kind() != SyntaxKind::CallExpression {
        return NodeList::NIL;
    }
    node.type_argument_list()
}

// Go: typeparser/piping_flow.go pipingFlowTypeArguments
pub fn piping_flow_type_arguments(node: Node, callee: Node) -> NodeList {
    let type_arguments = call_type_arguments(node);
    if !type_arguments.is_nil() {
        return type_arguments;
    }
    call_type_arguments(callee)
}

/// Effect patch `003-checker-exports` `Checker.GetTypeArgumentsForResolvedSignature`:
/// the instantiated type arguments for a resolved (non-generic) signature.
/// Returns nil if the signature has no mapper.
// PORT: the Rust checker has no port of this patched export, so its body is
// here.
pub(crate) fn get_type_arguments_for_resolved_signature(
    c: &mut Checker,
    sig: SignatureId,
) -> Vec<TypeId> {
    if sig.is_nil() || c.sig(sig).mapper.is_nil() {
        return Vec::new();
    }
    let mut type_params = c.sig(sig).type_parameters.clone();
    let target = c.sig(sig).target;
    if target.is_some() {
        type_params = c.sig(target).type_parameters.clone();
    }
    if type_params.is_empty() {
        return Vec::new();
    }
    let mapper = c.sig(sig).mapper;
    c.instantiate_types(&type_params, mapper)
}
