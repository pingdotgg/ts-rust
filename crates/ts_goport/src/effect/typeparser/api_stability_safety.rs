//! Port of Effect-TS/tsgo `internal/typeparser/api_stability_safety.go` at
//! `@effect/tsgo@0.51.1` (`47cb1ed7`): the pre-flight recursion guard of the
//! API stability analysis.
//!
//! PORT: signature conventions (as in `api_stability_p1.rs`): analysis and
//! scan methods take
//! `tp: &mut TypeParser<'_>`; Go
//! `apiStabilitySubstitution` is `&ApiStabilitySubstitution`; Go
//! `*apiStabilitySafetyBinding` is `Option<&Rc<ApiStabilitySafetyBinding>>`;
//! Go `*ast.NodeList` is `NodeList`; Go `*ast.FunctionLikeBase` is the
//! function-like `Node`; Go `map[*T]bool` work sets are `&mut FxHashMap<_, bool>`
//! (`scope` is only read: `&FxHashMap<TypeId, bool>`).

use crate::effect::typeparser::*;
use crate::prelude::*;

/// Go `apiStabilitySafety`.
/// apiStabilitySafety is the verdict of the pre-flight recursion guard that runs
/// before a potentially recursive lazy checker read. The guard inspects the
/// represented compiler type graph and the raw declaration structure without
/// performing the guarded resolution itself. Safe authorizes the ordinary lazy
/// read, Recursive and Unknown never do.
///
/// The guard is a pure safety analysis: it decides only whether a read may
/// expand a recursive alias without a lazy memo boundary. It never contributes
/// stability findings and it never suppresses compiler diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ApiStabilitySafety {
    /// Go `apiStabilitySafetySafe`.
    #[default]
    Safe = 0,
    /// Go `apiStabilitySafetyRecursive`.
    Recursive = 1,
    /// Go `apiStabilitySafetyUnknown`.
    Unknown = 2,
}

/// apiStabilitySafetyMaxWork bounds one guard run. Exhausting it yields
/// Unknown, which blocks the read; it is never memoized as safe.
pub const API_STABILITY_SAFETY_MAX_WORK: i32 = 16_384;
/// apiStabilitySafetyMaxDepth bounds nested alias applications on one guard
/// path. Exceeding it yields Unknown.
pub const API_STABILITY_SAFETY_MAX_DEPTH: i32 = 64;

/// Go `apiStabilitySafetyOperation`.
/// apiStabilitySafetyOperation identifies the guarded read family a completed
/// verdict belongs to, so a Safe result is only reused for the same operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ApiStabilitySafetyOperation {
    /// Go `apiStabilitySafetyOperationMemberTable`.
    #[default]
    MemberTable = 0,
    /// Go `apiStabilitySafetyOperationBaseTypes`.
    BaseTypes = 1,
    /// Go `apiStabilitySafetyOperationSymbolType`.
    SymbolType = 2,
    /// Go `apiStabilitySafetyOperationSignatureMembers`.
    SignatureMembers = 3,
    /// Go `apiStabilitySafetyOperationSignatureReturn`.
    SignatureReturn = 4,
    /// Go `apiStabilitySafetyOperationDeclaredType`.
    DeclaredType = 5,
    /// Go `apiStabilitySafetyOperationAnnotation`.
    Annotation = 6,
}

/// Go `apiStabilitySafetyVerdictKey`.
/// apiStabilitySafetyVerdictKey is the concrete identity of one guard request:
/// the read operation, the represented compiler object or raw node it resolves,
/// and the interned carrier of the substitution it is read under. The carrier
/// is the identity of the whole substitution chain, so two requests that would
/// resolve different represented components never share a verdict. Relation
/// operand and projection scans remain internal to a request, so they cannot
/// collide with a completed top-level verdict. No serialized context, string or
/// raw declaration target is part of a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApiStabilitySafetyVerdictKey {
    pub operation: ApiStabilitySafetyOperation,
    pub t: TypeId,
    pub signature: SignatureId,
    pub symbol: SymbolId,
    pub node: Node,
    pub carrier: ApiStabilityCarrierId,
}

/// Go `apiStabilitySafetyArg`.
/// apiStabilitySafetyArg is one concrete argument of a represented generic
/// application. Either the checker already represented the argument type, or the
/// raw argument node is kept together with the environment it was written in so
/// a type parameter argument can still be followed through an outer binding.
#[derive(Clone, Debug, Default)]
pub struct ApiStabilitySafetyArg {
    pub t: TypeId,
    pub node: Node,
    pub subst: ApiStabilitySubstitution,
    pub bindings: Option<Rc<ApiStabilitySafetyBinding>>,
}

impl ApiStabilitySafetyArg {
    // Go: typeparser/api_stability_safety.go apiStabilitySafetyArg.empty
    pub fn empty(&self) -> bool {
        self.t.is_nil() && self.node.is_nil()
    }
}

// Go: typeparser/api_stability_safety.go safetyArgsEqual
pub fn safety_args_equal(left: &[ApiStabilitySafetyArg], right: &[ApiStabilitySafetyArg]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    for (left, right) in left.iter().zip(right) {
        if left.t.is_some() || right.t.is_some() {
            if left.t != right.t {
                return false;
            }
            continue;
        }
        if left.node != right.node {
            return false;
        }
    }
    true
}

/// Go `apiStabilitySafetyBinding`.
/// apiStabilitySafetyBinding is one generic-application frame: the declared type
/// parameters of a class, interface or alias bound to the arguments of one
/// application. Frames chain so a nested application keeps the outer bindings.
#[derive(Clone, Debug, Default)]
pub struct ApiStabilitySafetyBinding {
    pub parent: Option<Rc<ApiStabilitySafetyBinding>>,
    pub params: Vec<TypeId>,
    pub args: Vec<ApiStabilitySafetyArg>,
}

/// Go `apiStabilitySafetyApplication`.
/// apiStabilitySafetyApplication is one alias application on the current guard
/// path. The concrete argument list is compared by identity so a repeated
/// application of the same alias with the same represented arguments is
/// recognized as an evaluated cycle.
#[derive(Clone, Debug, Default)]
pub struct ApiStabilitySafetyApplication {
    pub symbol: SymbolId,
    pub args: Vec<ApiStabilitySafetyArg>,
}

/// Go `apiStabilitySafetyScan`.
/// apiStabilitySafetyScan is the mutable state of one guard run. The verdict is
/// computed by traversing raw annotations, represented types and declaration
/// structure; a work and depth bound makes the traversal total and fails closed.
/// All nested scans share one analysis-level work counter so a chain of nested
/// argument verifications cannot run away.
///
/// PORT: Go `a *apiStabilityAnalysis` is a borrow of the analysis for the scan.
/// Go nil maps are empty maps.
pub struct ApiStabilitySafetyScan<'a> {
    pub a: &'a mut ApiStabilityAnalysis,

    pub stack: Vec<ApiStabilitySafetyApplication>,

    // relationOperands marks a nested scan that verifies a conditional operand
    // before the compiler relation is asked to compare it. The relation compares
    // the full projected member surface of both operands (properties, call and
    // construct returns, index infos) and instantiates generic members, so the
    // operand scan evaluates return annotations and refuses any component the
    // relation could instantiate (an unbound type parameter, a deferral, an
    // inferred return).
    pub relation_operands: bool,

    pub active_declarations: FxHashMap<SymbolId, bool>,
    pub materializing: FxHashMap<TypeId, bool>,
    pub materializing_nodes: FxHashMap<Node, bool>,
}

/// The `Safe`, `Recursive` and `Unknown` verdicts, for short.
use ApiStabilitySafety::{Recursive, Safe, Unknown};

impl ApiStabilitySafetyScan<'_> {
    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.exceeded
    pub fn exceeded(&mut self) -> bool {
        self.a.safety_work += 1;
        self.a.safety_work > API_STABILITY_SAFETY_MAX_WORK
    }
}

// Go: typeparser/api_stability_safety.go combineSafety
/// combineSafety merges two verdicts of one read: any recursion makes the whole
/// read recursive, and any unknown makes it unknown.
pub fn combine_safety(left: ApiStabilitySafety, right: ApiStabilitySafety) -> ApiStabilitySafety {
    if left == Recursive || right == Recursive {
        return Recursive;
    }
    if left == Unknown || right == Unknown {
        return Unknown;
    }
    Safe
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.symbolTypeResolutionIsSafe
    /// symbolTypeResolutionIsSafe reports whether resolving an unmaterialized
    /// symbol's type through the ordinary lazy accessor is bounded. An annotation or
    /// a function-like declaration is read through the recursion guard; an inferred
    /// declaration (an initializer, a parameter default or an accessor body) is read
    /// through the checker's ordinary lazy inference. Inference is the compiler's own
    /// lazy read: it attributes any diagnostics it produces to the declaring file's
    /// own expressions, so a later check of that file observes exactly the same
    /// diagnostics and types.
    pub fn symbol_type_resolution_is_safe(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
    ) -> bool {
        if symbol.is_nil() || tp.checker.sym(symbol).declarations.is_empty() {
            return false;
        }
        let mut subst = ApiStabilitySubstitution::default();
        let mapper = checker_integration::get_instantiated_symbol_mapper(tp.checker, symbol);
        if mapper.is_some() {
            subst = self.extend_mapper_substitution(
                &ApiStabilitySubstitution::default(),
                TypeId::NIL,
                mapper,
            );
        }
        let key = ApiStabilitySafetyVerdictKey {
            operation: ApiStabilitySafetyOperation::SymbolType,
            symbol,
            carrier: subst.carrier(),
            ..Default::default()
        };
        if self.safety_safe_verdict(&key) {
            return true;
        }
        if self.safety_budget_exhausted() {
            return false;
        }
        let declarations = tp.checker.sym(symbol).declarations.to_vec();
        let mut s = self.new_safety_scan();
        for declaration in declarations {
            if declaration.is_nil() {
                continue;
            }
            if api_stability_has_function_like_data(declaration) {
                // Resolving a function-like value creates its signature: parameter
                // types and type parameter constraints are resolved eagerly, the
                // return type stays lazy and is gated by its own read. An accessor
                // that must infer its type from its body checks the body exactly
                // like function and initializer inference does: its diagnostics
                // belong to the declaring file's own expressions, and it is only
                // read through the native source-check lifecycle.
                if matches!(
                    declaration.kind(),
                    SyntaxKind::GetAccessor | SyntaxKind::SetAccessor
                ) && !s.a.inferred_component_resolution_is_safe(tp, declaration)
                {
                    return false;
                }
                if s.function_like_resolution(tp, declaration, &subst, None) != Safe {
                    return false;
                }
                continue;
            }
            let annotation = declaration_annotation_node_of(declaration);
            if annotation.is_nil() {
                // An inferred declaration is read through the ordinary lazy
                // accessor. The checker attributes the diagnostics inference
                // produces to the declaring file's expressions, so the checked
                // program keeps them exactly. Inference is only read while the
                // checker is inside its source-file check lifecycle; outside it a
                // speculative inference is refused.
                if !s.a.inferred_component_resolution_is_safe(tp, declaration) {
                    return false;
                }
                continue;
            }
            if s.type_node(tp, annotation, &subst, None, false) != Safe {
                return false;
            }
        }
        self.record_safety_safe_verdict(key);
        true
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.signatureMemberResolutionIsSafe
    /// signatureMemberResolutionIsSafe reports whether materializing the raw
    /// signatures of a signature member symbol is bounded. It only inspects
    /// parameter annotations, which signature creation resolves eagerly.
    pub fn signature_member_resolution_is_safe(
        &mut self,
        tp: &mut TypeParser<'_>,
        member: SymbolId,
        subst: &ApiStabilitySubstitution,
    ) -> bool {
        if member.is_nil() {
            return false;
        }
        let key = ApiStabilitySafetyVerdictKey {
            operation: ApiStabilitySafetyOperation::SignatureMembers,
            symbol: member,
            carrier: subst.carrier(),
            ..Default::default()
        };
        if self.safety_safe_verdict(&key) {
            return true;
        }
        if self.safety_budget_exhausted() {
            return false;
        }
        let declarations = tp.checker.sym(member).declarations.to_vec();
        let mut s = self.new_safety_scan();
        for declaration in declarations {
            if declaration.is_nil() || !api_stability_has_function_like_data(declaration) {
                continue;
            }
            if s.function_like_resolution(tp, declaration, subst, None) != Safe {
                return false;
            }
        }
        self.record_safety_safe_verdict(key);
        true
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.annotationResolutionIsSafe
    /// annotationResolutionIsSafe reports whether resolving a raw annotation node
    /// through the ordinary lazy type accessor is bounded. The substitution context
    /// is the concrete enclosing application whose mapper will be applied when the
    /// annotation is instantiated.
    pub fn annotation_resolution_is_safe(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
    ) -> bool {
        if node.is_nil() {
            return false;
        }
        let key = ApiStabilitySafetyVerdictKey {
            operation: ApiStabilitySafetyOperation::Annotation,
            node,
            carrier: subst.carrier(),
            ..Default::default()
        };
        if self.safety_safe_verdict(&key) {
            return true;
        }
        if self.safety_budget_exhausted() {
            return false;
        }
        let mut s = self.new_safety_scan();
        if s.type_node(tp, node, subst, None, false) != Safe {
            return false;
        }
        self.record_safety_safe_verdict(key);
        true
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.declaredTypeResolutionIsSafe
    /// declaredTypeResolutionIsSafe reports whether resolving a symbol's declared
    /// type through the ordinary lazy accessor is bounded. Only a type alias
    /// actually resolves its right-hand side; a class or interface declared type is
    /// created without resolving members.
    pub fn declared_type_resolution_is_safe(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
    ) -> bool {
        if symbol.is_nil() {
            return false;
        }
        let flags = tp.checker.sym(symbol).flags;
        if flags.intersects(SymbolFlags::ALIAS) {
            if has_alias_declaration(tp.checker, symbol) {
                let target = api_stability_immediate_aliased_symbol(tp.checker, symbol);
                if target.is_some() && target != symbol {
                    return self.declared_type_resolution_is_safe(tp, target);
                }
            }
            return true;
        }
        if !flags.intersects(SymbolFlags::TYPE_ALIAS) {
            return true;
        }
        let key = ApiStabilitySafetyVerdictKey {
            operation: ApiStabilitySafetyOperation::DeclaredType,
            symbol,
            ..Default::default()
        };
        if self.safety_safe_verdict(&key) {
            return true;
        }
        if self.safety_budget_exhausted() {
            return false;
        }
        let declaration = api_stability_type_alias_declaration(tp.checker, symbol);
        if declaration.is_nil() {
            return false;
        }
        let mut s = self.new_safety_scan();
        if s.type_node(
            tp,
            declaration.type_(),
            &ApiStabilitySubstitution::default(),
            None,
            false,
        ) != Safe
        {
            return false;
        }
        self.record_safety_safe_verdict(key);
        true
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.signatureReturnResolutionIsSafe
    /// signatureReturnResolutionIsSafe reports whether materializing an
    /// unmaterialized signature return through the ordinary lazy accessor is
    /// bounded. An inferred return has no annotation: it is read through the
    /// checker's ordinary lazy inference, which attributes the diagnostics it
    /// produces to the declaring file's own expressions, so a later check of that
    /// file observes exactly the same diagnostics and types. An accessor's inferred
    /// return checks the accessor body exactly like an inferred component, so it is
    /// read through the same native source-check lifecycle.
    pub fn signature_return_resolution_is_safe(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
        subst: &ApiStabilitySubstitution,
    ) -> bool {
        if signature.is_nil() {
            return false;
        }
        let node = signature_return_type_node(tp.checker, signature);
        if node.is_nil() {
            let declaration = tp.checker.sig(signature).declaration();
            if declaration.is_nil() {
                return false;
            }
            return self.inferred_component_resolution_is_safe(tp, declaration);
        }
        let key = ApiStabilitySafetyVerdictKey {
            operation: ApiStabilitySafetyOperation::SignatureReturn,
            signature,
            carrier: subst.carrier(),
            ..Default::default()
        };
        if self.safety_safe_verdict(&key) {
            return true;
        }
        if self.safety_budget_exhausted() {
            return false;
        }
        let mut s = self.new_safety_scan();
        if s.type_node(tp, node, subst, None, false) != Safe {
            return false;
        }
        self.record_safety_safe_verdict(key);
        true
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.inferredComponentResolutionIsSafe
    /// inferredComponentResolutionIsSafe reports whether the native lazy inference
    /// of an unmaterialized component declared in a source file may be read. The
    /// ordinary lazy accessors attribute the diagnostics inference produces to the
    /// declaring file's own expressions, exactly as that file's ordinary check
    /// would, but that attribution is only established while the checker is inside
    /// its source-file check lifecycle (the after-check callback). A speculative
    /// read outside that lifecycle keeps refusing inferred components.
    pub fn inferred_component_resolution_is_safe(
        &mut self,
        tp: &mut TypeParser<'_>,
        declaration: Node,
    ) -> bool {
        if declaration.is_nil() {
            return false;
        }
        if checker_integration::is_source_file_type_checked(
            tp.checker,
            get_source_file_of_node(declaration),
        ) {
            return true;
        }
        checker_integration::is_checking_source_file(tp.checker)
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.baseTypesResolutionIsSafe
    /// baseTypesResolutionIsSafe reports whether materializing an unmaterialized
    /// heritage surface is bounded. Resolving base types evaluates the heritage type
    /// argument annotations; the base declarations' own member surfaces are gated by
    /// their own reads.
    pub fn base_types_resolution_is_safe(
        &mut self,
        tp: &mut TypeParser<'_>,
        declaration: TypeId,
        subst: &ApiStabilitySubstitution,
    ) -> bool {
        if declaration.is_nil() {
            return false;
        }
        let symbol = tp.checker.ty(declaration).symbol();
        if symbol.is_nil() {
            return false;
        }
        let key = ApiStabilitySafetyVerdictKey {
            operation: ApiStabilitySafetyOperation::BaseTypes,
            t: declaration,
            carrier: subst.carrier(),
            ..Default::default()
        };
        if self.safety_safe_verdict(&key) {
            return true;
        }
        if self.safety_budget_exhausted() {
            return false;
        }
        let mut s = self.new_safety_scan();
        if s.heritage(tp, symbol, subst, None, false) != Safe {
            return false;
        }
        self.record_safety_safe_verdict(key);
        true
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.memberTableResolutionIsSafe
    /// memberTableResolutionIsSafe reports whether resolving a structured type's
    /// member table (properties, signatures or index infos) through the ordinary
    /// lazy accessors is bounded. Member resolution eagerly instantiates declared
    /// index signature annotations, creates the declared call and construct
    /// signatures, and recursively resolves inherited base member tables.
    pub fn member_table_resolution_is_safe(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
    ) -> bool {
        if t.is_nil() {
            return false;
        }
        if !tp.checker.ty(t).flags().intersects(TypeFlags::OBJECT) {
            return true;
        }
        let key = ApiStabilitySafetyVerdictKey {
            operation: ApiStabilitySafetyOperation::MemberTable,
            t,
            carrier: subst.carrier(),
            ..Default::default()
        };
        if self.safety_safe_verdict(&key) {
            return true;
        }
        if self.safety_budget_exhausted() {
            return false;
        }
        let mut s = self.new_safety_scan();
        if s.member_table(tp, t, subst, None) != Safe {
            return false;
        }
        self.record_safety_safe_verdict(key);
        true
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.safetyBudgetExhausted
    /// safetyBudgetExhausted reports whether this analysis has already consumed its
    /// shared guard budget. A guard request after exhaustion is refused immediately
    /// without allocating scan state; the traversal still collects represented
    /// dependencies and declaration metadata.
    pub fn safety_budget_exhausted(&self) -> bool {
        self.safety_work > API_STABILITY_SAFETY_MAX_WORK
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.safetySafeVerdict
    /// safetySafeVerdict reports whether an identical guard request already
    /// completed safely in this analysis. Only completed Safe verdicts are reused:
    /// Unknown, recursion and exhaustion stay retryable by later scans and later
    /// analyses.
    pub fn safety_safe_verdict(&self, key: &ApiStabilitySafetyVerdictKey) -> bool {
        self.safety_safe_memo.contains(key)
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.recordSafetySafeVerdict
    /// recordSafetySafeVerdict records one completed Safe guard verdict.
    pub fn record_safety_safe_verdict(&mut self, key: ApiStabilitySafetyVerdictKey) {
        self.safety_safe_memo.insert(key);
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.symbolAtTypeNameNode
    /// symbolAtTypeNameNode returns the symbol named by a raw type-name, heritage,
    /// qualifier or declaration-name node, caching successful lookups by AST
    /// identity for this analysis. A failed lookup is never cached. Only the symbol
    /// lookup is cached: no resolved type, binding-dependent member identity or
    /// context string is ever stored here.
    pub fn symbol_at_type_name_node(&mut self, tp: &mut TypeParser<'_>, node: Node) -> SymbolId {
        if node.is_nil() {
            return SymbolId::NIL;
        }
        if let Some(&symbol) = self.type_name_symbols.get(&node) {
            return symbol;
        }
        let symbol = tp.checker.get_symbol_at_location_exported(node);
        if symbol.is_some() {
            self.type_name_symbols.insert(node, symbol);
        }
        symbol
    }

    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.newSafetyScan
    pub fn new_safety_scan(&mut self) -> ApiStabilitySafetyScan<'_> {
        ApiStabilitySafetyScan {
            a: self,
            stack: Vec::new(),
            relation_operands: false,
            active_declarations: FxHashMap::default(),
            materializing: FxHashMap::default(),
            materializing_nodes: FxHashMap::default(),
        }
    }
}

impl ApiStabilitySafetyScan<'_> {
    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeNode
    /// typeNode scans one raw type annotation node. project marks a position whose
    /// members are evaluated because the surrounding construct projects into them
    /// (an indexed access or a keyof target), so nested property annotations are
    /// evaluated as well.
    pub fn type_node(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        if node.is_nil() {
            return Safe;
        }
        if self.exceeded() {
            return Unknown;
        }
        match node.kind() {
            SyntaxKind::AnyKeyword
            | SyntaxKind::UnknownKeyword
            | SyntaxKind::NeverKeyword
            | SyntaxKind::VoidKeyword
            | SyntaxKind::UndefinedKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::StringKeyword
            | SyntaxKind::NumberKeyword
            | SyntaxKind::BigIntKeyword
            | SyntaxKind::BooleanKeyword
            | SyntaxKind::SymbolKeyword
            | SyntaxKind::ObjectKeyword
            | SyntaxKind::ThisType
            | SyntaxKind::IntrinsicKeyword
            | SyntaxKind::LiteralType => Safe,
            SyntaxKind::ParenthesizedType
            | SyntaxKind::OptionalType
            | SyntaxKind::RestType
            | SyntaxKind::NamedTupleMember => {
                self.type_node(tp, node.type_(), subst, bindings, project)
            }
            SyntaxKind::ArrayType => {
                self.type_node(tp, node.element_type(), subst, bindings, project)
            }
            SyntaxKind::TypeOperator => {
                if node.operator() == SyntaxKind::KeyOfKeyword {
                    // `keyof T` resolves the keys of T's member surface.
                    return self.type_node(tp, node.type_(), subst, bindings, true);
                }
                self.type_node(tp, node.type_(), subst, bindings, project)
            }
            SyntaxKind::TupleType => {
                let mut verdict = Safe;
                for element in node.elements().iter() {
                    verdict = combine_safety(
                        verdict,
                        self.type_node(tp, element, subst, bindings, project),
                    );
                }
                verdict
            }
            SyntaxKind::UnionType | SyntaxKind::IntersectionType => {
                let mut verdict = Safe;
                for member in node.types().nodes().iter() {
                    verdict = combine_safety(
                        verdict,
                        self.type_node(tp, member, subst, bindings, project),
                    );
                }
                verdict
            }
            SyntaxKind::TemplateLiteralType => {
                let mut verdict = Safe;
                for span in node.template_spans().nodes().iter() {
                    if span.is_nil() || span.kind() != SyntaxKind::TemplateLiteralTypeSpan {
                        continue;
                    }
                    verdict = combine_safety(
                        verdict,
                        self.type_node(tp, span.type_(), subst, bindings, project),
                    );
                }
                verdict
            }
            SyntaxKind::FunctionType | SyntaxKind::ConstructorType => {
                self.function_like_resolution(tp, node, subst, bindings)
            }
            SyntaxKind::TypeLiteral => self.type_literal(tp, node, subst, bindings, project),
            SyntaxKind::MappedType => self.mapped_type_node(tp, node, subst, bindings, project),
            SyntaxKind::ConditionalType => {
                self.conditional_node(tp, node, subst, bindings, project)
            }
            SyntaxKind::IndexedAccessType => {
                let verdict = self.type_node(tp, node.object_type(), subst, bindings, true);
                combine_safety(
                    verdict,
                    self.type_node(tp, node.index_type(), subst, bindings, project),
                )
            }
            SyntaxKind::TypeReference => self.type_reference(tp, node, subst, bindings, project),
            SyntaxKind::ExpressionWithTypeArguments => {
                let arguments = node.type_argument_list();
                let verdict = self.type_arguments(tp, arguments, subst, bindings);
                if !project {
                    return verdict;
                }
                let symbol = self.a.symbol_at_type_name_node(tp, node.expression());
                combine_safety(
                    verdict,
                    self.declaration_members(tp, symbol, arguments, subst, bindings, true),
                )
            }
            SyntaxKind::TypeQuery => self.type_query(tp, node, subst, bindings, project),
            SyntaxKind::ImportType => {
                let arguments = node.type_argument_list();
                let verdict = self.type_arguments(tp, arguments, subst, bindings);
                let qualifier = node.qualifier();
                if qualifier.is_nil() {
                    return combine_safety(verdict, Unknown);
                }
                let symbol = self.a.symbol_at_type_name_node(tp, qualifier);
                if symbol.is_nil() {
                    return combine_safety(verdict, Unknown);
                }
                if !project {
                    return verdict;
                }
                combine_safety(
                    verdict,
                    self.declaration_members(tp, symbol, arguments, subst, bindings, true),
                )
            }
            SyntaxKind::InferType => Safe,
            _ => Unknown,
        }
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeArguments
    /// typeArguments scans the eagerly evaluated type arguments of a reference.
    pub fn type_arguments(
        &mut self,
        tp: &mut TypeParser<'_>,
        arguments: NodeList,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
    ) -> ApiStabilitySafety {
        let mut verdict = Safe;
        for argument in arguments.nodes().iter() {
            verdict = combine_safety(
                verdict,
                self.type_node(tp, argument, subst, bindings, false),
            );
        }
        verdict
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeReference
    /// typeReference scans an application or declaration reference. A reference to a
    /// bound type parameter follows its concrete binding; a type alias application
    /// follows the alias declaration with a new binding frame and detects an
    /// evaluated cycle; a class or interface reference evaluates its type arguments
    /// and, when projected, its declaration member surface.
    pub fn type_reference(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let symbol = self.a.symbol_at_type_name_node(tp, node.type_name());
        if symbol.is_nil() {
            return Unknown;
        }
        let arguments = node.type_argument_list();
        let flags = tp.checker.sym(symbol).flags;
        if flags.intersects(SymbolFlags::TYPE_PARAMETER) {
            return self.type_parameter_reference(tp, symbol, subst, bindings, project);
        }
        if flags.intersects(SymbolFlags::ALIAS) {
            if has_alias_declaration(tp.checker, symbol) {
                let target = api_stability_immediate_aliased_symbol(tp.checker, symbol);
                if target.is_some() && target != symbol {
                    return self
                        .reference_to_symbol(tp, target, arguments, subst, bindings, project);
                }
            }
            return Unknown;
        }
        if flags.intersects(SymbolFlags::TYPE_ALIAS) {
            return self.alias_application(tp, symbol, arguments, subst, bindings, project);
        }
        if flags.intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE | SymbolFlags::ENUM) {
            let mut verdict = self.type_arguments(tp, arguments, subst, bindings);
            verdict = combine_safety(
                verdict,
                self.omitted_argument_defaults(tp, symbol, arguments, subst, bindings),
            );
            if !project {
                return verdict;
            }
            return combine_safety(
                verdict,
                self.declaration_members(tp, symbol, arguments, subst, bindings, true),
            );
        }
        Unknown
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.referenceToSymbol
    /// referenceToSymbol resolves an import alias reference to its target symbol.
    pub fn reference_to_symbol(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
        arguments: NodeList,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let flags = tp.checker.sym(symbol).flags;
        if flags.intersects(SymbolFlags::TYPE_ALIAS) {
            return self.alias_application(tp, symbol, arguments, subst, bindings, project);
        }
        if flags.intersects(SymbolFlags::TYPE_PARAMETER) {
            return self.type_parameter_reference(tp, symbol, subst, bindings, project);
        }
        if flags.intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE | SymbolFlags::ENUM) {
            let mut verdict = self.type_arguments(tp, arguments, subst, bindings);
            verdict = combine_safety(
                verdict,
                self.omitted_argument_defaults(tp, symbol, arguments, subst, bindings),
            );
            if !project {
                return verdict;
            }
            return combine_safety(
                verdict,
                self.declaration_members(tp, symbol, arguments, subst, bindings, true),
            );
        }
        Unknown
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.omittedArgumentDefaults
    /// omittedArgumentDefaults scans the declared default annotations the checker
    /// evaluates when a generic application omits arguments. Later defaults are
    /// scanned with the earlier parameters bound, so a default that references an
    /// earlier parameter still follows its concrete argument. A required parameter
    /// the application omits is an error the guard refuses to evaluate.
    ///
    /// PORT: Go fills one `args` slice that its binding frame shares, so each
    /// default is scanned with the defaults before it (and itself) bound. The
    /// port makes a frame per default from the filled prefix: nothing keeps a
    /// frame past its scan, so the views are the same.
    pub fn omitted_argument_defaults(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
        arguments: NodeList,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
    ) -> ApiStabilitySafety {
        let defaults = safety_type_parameter_defaults(tp.checker, symbol);
        let params = tp
            .checker
            .get_local_type_parameters_of_class_or_interface_or_type_alias_exported(symbol);
        if defaults.is_empty() || params.is_empty() {
            return Safe;
        }
        let argument_nodes = arguments.nodes();
        let provided = argument_nodes.len();
        if provided >= params.len() {
            return Safe;
        }
        let mut args = vec![ApiStabilitySafetyArg::default(); params.len()];
        for (index, arg) in args.iter_mut().enumerate().take(provided) {
            let argument = argument_nodes.get(index);
            if argument.is_some() {
                *arg = ApiStabilitySafetyArg {
                    node: argument,
                    subst: subst.clone(),
                    bindings: bindings.cloned(),
                    ..Default::default()
                };
            }
        }
        let mut verdict = Safe;
        for index in provided..params.len() {
            if index >= defaults.len() || defaults[index].is_nil() {
                return Unknown;
            }
            args[index] = ApiStabilitySafetyArg {
                node: defaults[index],
                ..Default::default()
            };
            let frame = Rc::new(ApiStabilitySafetyBinding {
                parent: bindings.cloned(),
                params: params.clone(),
                args: args.clone(),
            });
            verdict = combine_safety(
                verdict,
                self.type_node(tp, defaults[index], subst, Some(&frame), false),
            );
        }
        verdict
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeParameterReference
    /// typeParameterReference follows a type parameter reference through the active
    /// substitution and binding frames. An unbound parameter keeps the annotation
    /// deferred, so the guarded read evaluates nothing for that reference and it is
    /// safe. A relation-operand scan refuses an unbound parameter instead: the
    /// relation may instantiate it against the other operand.
    pub fn type_parameter_reference(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        self.a.note_materializing_symbol_read(
            ApiStabilityMaterializationReadKind::DeclaredType,
            symbol,
        );
        let parameter = tp.checker.get_declared_type_of_symbol_exported(symbol);
        if parameter.is_nil() {
            return Unknown;
        }
        let (argument, ok) = safety_binding_lookup(tp.checker, bindings, parameter);
        if ok {
            if argument.empty() {
                return Unknown;
            }
            if argument.t.is_some() {
                return self.type_value(
                    tp,
                    argument.t,
                    &argument.subst,
                    argument.bindings.as_ref(),
                    project,
                );
            }
            return self.type_node(
                tp,
                argument.node,
                &argument.subst,
                argument.bindings.as_ref(),
                project,
            );
        }
        let (mapped, parent, replaced) = self.a.substitute(tp, subst, parameter);
        if replaced {
            if mapped.is_nil() {
                // The parameter was replaced by an argument the checker has not
                // represented; the applied value is unknown.
                return Unknown;
            }
            if mapped == parameter {
                if self.relation_operands {
                    return Unknown;
                }
                return Safe;
            }
            return self.type_value(tp, mapped, &parent, bindings, project);
        }
        if self.relation_operands {
            return Unknown;
        }
        Safe
    }
}

// Go: typeparser/api_stability_safety.go safetyBindingLookup
/// safetyBindingLookup finds the binding of a declared type parameter in a
/// binding chain.
// PORT: Go reads `parameter.Symbol()` without a checker; here it takes `c`.
pub fn safety_binding_lookup(
    c: &Checker,
    bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
    parameter: TypeId,
) -> (ApiStabilitySafetyArg, bool) {
    if parameter.is_nil() {
        return (ApiStabilitySafetyArg::default(), false);
    }
    let symbol = c.ty(parameter).symbol();
    let mut frame = bindings;
    while let Some(current) = frame {
        for (index, &candidate) in current.params.iter().enumerate() {
            if index >= current.args.len() {
                continue;
            }
            if candidate == parameter || symbol.is_some() && c.ty(candidate).symbol() == symbol {
                return (current.args[index].clone(), true);
            }
        }
        frame = current.parent.as_ref();
    }
    (ApiStabilitySafetyArg::default(), false)
}

impl ApiStabilitySafetyScan<'_> {
    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.aliasApplication
    /// aliasApplication follows one application of a type alias. The alias
    /// declaration is scanned with a binding frame for its declared parameters. A
    /// repeated application of the same alias with the same concrete arguments on
    /// the eager path is an evaluated cycle; a cycle crossed only through lazy
    /// positions is never scanned, so reaching it here means the cycle is
    /// evaluated.
    pub fn alias_application(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
        arguments: NodeList,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let declaration = api_stability_type_alias_declaration(tp.checker, symbol);
        if declaration.is_nil() {
            return Unknown;
        }
        let params = tp
            .checker
            .get_local_type_parameters_of_class_or_interface_or_type_alias_exported(symbol);
        let mut args = vec![ApiStabilitySafetyArg::default(); params.len()];
        let mut verdict = Safe;
        for (index, argument) in arguments.nodes().iter().enumerate() {
            if argument.is_nil() {
                continue;
            }
            // Every explicit argument is evaluated eagerly when the checker
            // creates the application, even when the alias right-hand side
            // ignores it. Scan each provided argument in the environment it was
            // written in.
            verdict = combine_safety(
                verdict,
                self.type_node(tp, argument, subst, bindings, false),
            );
            if index >= params.len() {
                continue;
            }
            args[index] = ApiStabilitySafetyArg {
                node: argument,
                subst: subst.clone(),
                bindings: bindings.cloned(),
                ..Default::default()
            };
        }

        // Omitted arguments are filled from declared defaults, which the checker
        // evaluates when it creates the application.
        let mut defaults = Vec::new();
        if !params.is_empty() {
            let default_nodes = safety_type_parameter_defaults(tp.checker, symbol);
            for (index, arg) in args.iter_mut().enumerate() {
                if !arg.empty() {
                    continue;
                }
                if index < default_nodes.len() && default_nodes[index].is_some() {
                    *arg = ApiStabilitySafetyArg {
                        node: default_nodes[index],
                        ..Default::default()
                    };
                    defaults.push(default_nodes[index]);
                    continue;
                }
                // A required argument the application omits is an error the guard
                // refuses to evaluate.
                return Unknown;
            }
        }
        for application in &self.stack {
            if application.symbol == symbol && safety_args_equal(&application.args, &args) {
                return Recursive;
            }
        }
        if self.stack.len() >= API_STABILITY_SAFETY_MAX_DEPTH as usize {
            return Unknown;
        }
        let frame = Rc::new(ApiStabilitySafetyBinding {
            parent: bindings.cloned(),
            params,
            args: args.clone(),
        });
        self.stack
            .push(ApiStabilitySafetyApplication { symbol, args });
        for default_node in defaults {
            verdict = combine_safety(
                verdict,
                self.type_node(tp, default_node, subst, Some(&frame), project),
            );
        }
        let result = combine_safety(
            verdict,
            self.type_node(tp, declaration.type_(), subst, Some(&frame), project),
        );
        self.stack.pop();
        result
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeValue
    /// typeValue scans one represented type. The represented graph is the primary
    /// carrier: it is followed through its concrete structure, and any type
    /// parameter is resolved through the active substitution or binding frames.
    pub fn type_value(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        if t.is_nil() {
            return Safe;
        }
        if self.exceeded() {
            return Unknown;
        }
        let t = non_distributed_parameter(tp.checker, t);
        let flags = tp.checker.ty(t).flags();
        if flags.intersects(TypeFlags::TYPE_PARAMETER) {
            let (parameter, replaced) = self.mapped_parameter_value(tp, t, subst, bindings);
            if replaced {
                if parameter.is_nil() {
                    return Unknown;
                }
                if parameter == t {
                    return Safe;
                }
                return self.type_value(tp, parameter, subst, bindings, project);
            }
            return self.constraint_safety(tp, t, subst, bindings, project);
        }
        if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let mut verdict = Safe;
            let members = tp.checker.ty(t).types().to_vec();
            for member in members {
                verdict = combine_safety(
                    verdict,
                    self.type_value(tp, member, subst, bindings, project),
                );
            }
            return verdict;
        }
        if flags.intersects(TypeFlags::CONDITIONAL) {
            return self.conditional_value(tp, t, subst, bindings, project);
        }
        if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            let indexed = tp.checker.ty(t).as_indexed_access_type();
            let (object_type, index_type) = (indexed.object_type, indexed.index_type);
            let verdict = self.type_value(tp, object_type, subst, bindings, true);
            return combine_safety(
                verdict,
                self.type_value(tp, index_type, subst, bindings, project),
            );
        }
        if flags.intersects(TypeFlags::INDEX) {
            let target = tp.checker.ty(t).as_index_type().target;
            return self.type_value(tp, target, subst, bindings, true);
        }
        if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let mut verdict = Safe;
            let parts = tp.checker.ty(t).as_template_literal_type().types.to_vec();
            for part in parts {
                verdict =
                    combine_safety(verdict, self.type_value(tp, part, subst, bindings, project));
            }
            return verdict;
        }
        if flags.intersects(TypeFlags::STRING_MAPPING) {
            let target = tp.checker.ty(t).as_string_mapping_type().target;
            return self.type_value(tp, target, subst, bindings, project);
        }
        if flags.intersects(TypeFlags::SUBSTITUTION) {
            let substitution = tp.checker.ty(t).as_substitution_type();
            let (base_type, constraint) = (substitution.base_type, substitution.constraint);
            let verdict = self.type_value(tp, base_type, subst, bindings, project);
            return combine_safety(
                verdict,
                self.type_value(tp, constraint, subst, bindings, project),
            );
        }
        if flags.intersects(TypeFlags::OBJECT) {
            return self.object_value(tp, t, subst, bindings, project);
        }
        Safe
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.mappedParameterValue
    /// mappedParameterValue resolves a type parameter through the binding chain or
    /// the active substitution frames. The boolean reports whether the parameter is
    /// replaced.
    pub fn mapped_parameter_value(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
    ) -> (TypeId, bool) {
        let (argument, ok) = safety_binding_lookup(tp.checker, bindings, t);
        if ok {
            return (self.materialize_argument(tp, &argument), true);
        }
        let (mapped, _, replaced) = self.a.substitute(tp, subst, t);
        if replaced {
            return (mapped, true);
        }
        (TypeId::NIL, false)
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.materializeArgument
    /// materializeArgument returns the represented type of a captured application
    /// argument without performing the guarded resolution: a represented argument is
    /// resolved through its own environment, a primitive or literal argument is read
    /// from the checker intrinsics, and a type parameter argument is followed
    /// through its outer binding. Anything else stays nil.
    pub fn materialize_argument(
        &mut self,
        tp: &mut TypeParser<'_>,
        argument: &ApiStabilitySafetyArg,
    ) -> TypeId {
        if argument.t.is_some() {
            if !tp
                .checker
                .ty(argument.t)
                .flags()
                .intersects(TypeFlags::TYPE_PARAMETER)
            {
                return argument.t;
            }
            if self
                .materializing
                .get(&argument.t)
                .copied()
                .unwrap_or(false)
            {
                return TypeId::NIL;
            }
            self.materializing.insert(argument.t, true);
            let (mapped, replaced) = self.mapped_parameter_value(
                tp,
                argument.t,
                &argument.subst,
                argument.bindings.as_ref(),
            );
            self.materializing.remove(&argument.t);
            if replaced {
                return mapped;
            }
            return argument.t;
        }
        if argument.node.is_nil() {
            return TypeId::NIL;
        }
        let represented =
            checker_integration::get_resolved_type_from_type_node(tp.checker, argument.node);
        if represented.is_some() {
            let (mapped, replaced) = self.mapped_parameter_value(
                tp,
                represented,
                &argument.subst,
                argument.bindings.as_ref(),
            );
            if replaced {
                return mapped;
            }
            return represented;
        }
        let intrinsic = api_stability_intrinsic_type_of_node(tp.checker, argument.node);
        if intrinsic.is_some() {
            return intrinsic;
        }
        if argument.node.kind() == SyntaxKind::TypeReference {
            let symbol = self
                .a
                .symbol_at_type_name_node(tp, argument.node.type_name());
            if symbol.is_some()
                && tp
                    .checker
                    .sym(symbol)
                    .flags
                    .intersects(SymbolFlags::TYPE_PARAMETER)
            {
                self.a.note_materializing_symbol_read(
                    ApiStabilityMaterializationReadKind::DeclaredType,
                    symbol,
                );
                let parameter = tp.checker.get_declared_type_of_symbol_exported(symbol);
                if parameter.is_some() {
                    if self.materializing.get(&parameter).copied().unwrap_or(false) {
                        return TypeId::NIL;
                    }
                    self.materializing.insert(parameter, true);
                    let (mapped, replaced) = self.mapped_parameter_value(
                        tp,
                        parameter,
                        &argument.subst,
                        argument.bindings.as_ref(),
                    );
                    self.materializing.remove(&parameter);
                    if replaced {
                        return mapped;
                    }
                }
            }
        }
        // A concrete argument annotation (a reference to a declaration, an
        // application, a literal) is only resolved when the guard establishes that
        // its own resolution is bounded. The nested verification shares this
        // analysis's work counter and never recurses into the same argument.
        if self
            .materializing_nodes
            .get(&argument.node)
            .copied()
            .unwrap_or(false)
        {
            return TypeId::NIL;
        }
        self.materializing_nodes.insert(argument.node, true);
        let relation_operands = self.relation_operands;
        let safe = {
            let mut nested = self.a.new_safety_scan();
            nested.relation_operands = relation_operands;
            nested.type_node(
                tp,
                argument.node,
                &argument.subst,
                argument.bindings.as_ref(),
                false,
            ) == Safe
        };
        self.materializing_nodes.remove(&argument.node);
        if safe {
            self.a.note_materializing_node_read(argument.node);
            return tp.checker.get_type_from_type_node_exported(argument.node);
        }
        TypeId::NIL
    }
}

// Go: typeparser/api_stability_safety.go apiStabilityIntrinsicTypeOfNode
/// apiStabilityIntrinsicTypeOfNode returns the checker intrinsic for a primitive
/// keyword annotation. The mapping is a direct field read; no annotation is
/// resolved.
pub fn api_stability_intrinsic_type_of_node(c: &Checker, node: Node) -> TypeId {
    if node.is_nil() {
        return TypeId::NIL;
    }
    match node.kind() {
        SyntaxKind::AnyKeyword => c.get_any_type(),
        SyntaxKind::UnknownKeyword => c.get_unknown_type(),
        SyntaxKind::NeverKeyword => c.get_never_type(),
        SyntaxKind::VoidKeyword => c.get_void_type(),
        SyntaxKind::UndefinedKeyword => c.get_undefined_type(),
        SyntaxKind::NullKeyword => c.get_null_type(),
        SyntaxKind::StringKeyword => c.get_string_type(),
        SyntaxKind::NumberKeyword => c.get_number_type(),
        SyntaxKind::BigIntKeyword => c.get_big_int_type(),
        SyntaxKind::BooleanKeyword => c.get_boolean_type(),
        SyntaxKind::SymbolKeyword => c.get_es_symbol_type(),
        SyntaxKind::ObjectKeyword => checker_integration::get_non_primitive_type(c),
        _ => TypeId::NIL,
    }
}

impl ApiStabilitySafetyScan<'_> {
    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.constraintSafety
    /// constraintSafety follows the constraint of an unbound type parameter when the
    /// guarded read may resolve it (an apparent-type request on a type parameter). A
    /// relation-operand scan refuses an unbound parameter instead: the relation may
    /// instantiate it against the other operand.
    pub fn constraint_safety(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        if self.relation_operands {
            return Unknown;
        }
        let annotation = type_parameter_annotation_node(tp.checker, t, true);
        if annotation.is_nil() {
            return Safe;
        }
        self.type_node(tp, annotation, subst, bindings, project)
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.objectValue
    /// objectValue scans a represented object type.
    pub fn object_value(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let flags = tp.checker.ty(t).object_flags();
        if flags.intersects(ObjectFlags::REVERSE_MAPPED | ObjectFlags::EVOLVING_ARRAY) {
            return Safe;
        }
        if flags.intersects(ObjectFlags::MAPPED) {
            return self.mapped_value(tp, t, subst, bindings, project);
        }
        if flags.intersects(ObjectFlags::REFERENCE) {
            let target = tp.checker.ty(t).target();
            if target.is_some() && target != t {
                let arguments = checker_integration::get_resolved_type_arguments(tp.checker, t);
                let mut verdict = Safe;
                for &argument in &arguments {
                    verdict = combine_safety(
                        verdict,
                        self.type_value(tp, argument, subst, bindings, project),
                    );
                }
                if !project {
                    return verdict;
                }
                if self.relation_operands {
                    // Bind the reference's own represented arguments so a member
                    // annotation that mentions the target's type parameters resolves to
                    // the concrete argument the relation will compare.
                    return combine_safety(
                        verdict,
                        self.declaration_members_of_represented_arguments(
                            tp, target, &arguments, subst, bindings, true,
                        ),
                    );
                }
                let target_symbol = tp.checker.ty(target).symbol();
                return combine_safety(
                    verdict,
                    self.declaration_members(
                        tp,
                        target_symbol,
                        NodeList::NIL,
                        subst,
                        bindings,
                        true,
                    ),
                );
            }
        }
        let symbol = tp.checker.ty(t).symbol();
        if symbol.is_nil() {
            return Safe;
        }
        self.declaration_structure(tp, symbol, subst, bindings, project)
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.mappedValue
    /// mappedValue scans a represented mapped type through its represented mapped
    /// components.
    pub fn mapped_value(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let constraint = checker_integration::get_mapped_type_constraint_type(tp.checker, t);
        let mut verdict = self.type_value(tp, constraint, subst, bindings, project);
        let name = checker_integration::get_mapped_type_name_type(tp.checker, t);
        verdict = combine_safety(verdict, self.type_value(tp, name, subst, bindings, project));
        let template = checker_integration::get_mapped_type_template_type(tp.checker, t);
        combine_safety(
            verdict,
            self.type_value(tp, template, subst, bindings, project),
        )
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.conditionalValue
    /// conditionalValue scans a represented conditional type. A deferred
    /// conditional whose operands still mention an unbound parameter is not
    /// evaluated by the guarded read. A concrete conditional must be resolved: the
    /// guard selects the branch with the compiler's own relation instead of
    /// guessing, and refuses to evaluate a conditional whose outcome the relation
    /// cannot establish without inference.
    pub fn conditional_value(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let conditional = tp.checker.ty(t).as_conditional_type();
        let (check, extends) = (conditional.check_type, conditional.extends_type);
        let check_safe = self.type_value(tp, check, subst, bindings, project);
        let extends_safe = self.type_value(tp, extends, subst, bindings, project);
        if check_safe != Safe || extends_safe != Safe {
            return combine_safety(check_safe, extends_safe);
        }
        if self.contains_unbound_parameter(tp, check, subst, bindings, &mut FxHashMap::default())
            || self.contains_unbound_parameter(
                tp,
                extends,
                subst,
                bindings,
                &mut FxHashMap::default(),
            )
        {
            return Safe;
        }
        let selection = self.select_conditional_branch(tp, check, extends, subst, bindings);
        if selection == BranchSelection::UNKNOWN {
            return Unknown;
        }
        let mut verdict = Safe;
        if selection.intersects(BranchSelection::TRUE) {
            verdict = combine_safety(
                verdict,
                self.conditional_branch_value(tp, t, true, subst, bindings, project),
            );
        }
        if selection.intersects(BranchSelection::FALSE) {
            verdict = combine_safety(
                verdict,
                self.conditional_branch_value(tp, t, false, subst, bindings, project),
            );
        }
        verdict
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.conditionalBranchValue
    /// conditionalBranchValue scans one represented branch of a conditional. The
    /// materialized branch is preferred; otherwise the declared branch annotation is
    /// scanned with the conditional's own mapper bindings.
    pub fn conditional_branch_value(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        true_branch: bool,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let represented =
            checker_integration::get_resolved_conditional_type_branch(tp.checker, t, true_branch);
        if represented.is_some() {
            return self.type_value(tp, represented, subst, bindings, project);
        }
        let node =
            checker_integration::get_conditional_type_branch_node(tp.checker, t, true_branch);
        if node.is_nil() {
            return Unknown;
        }
        self.type_node(tp, node, subst, bindings, project)
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.conditionalNode
    /// conditionalNode scans a raw conditional annotation. Operands that mention an
    /// unbound parameter keep the conditional deferred and safe: constructing a
    /// deferred conditional never evaluates its branches, even when an infer binder
    /// appears. A concrete conditional is resolved through the compiler relation; an
    /// infer binder in its extends type has an inferred outcome the relation cannot
    /// establish without instantiating the application, so the guard refuses it.
    pub fn conditional_node(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let (check_type, extends_type) = (node.check_type(), node.extends_type());
        let check_safe = self.type_node(tp, check_type, subst, bindings, project);
        let extends_safe = self.type_node(tp, extends_type, subst, bindings, project);
        if check_safe != Safe || extends_safe != Safe {
            return combine_safety(check_safe, extends_safe);
        }
        if self.type_node_mentions_unbound_parameter(
            tp,
            check_type,
            subst,
            bindings,
            &mut FxHashMap::default(),
        ) || self.type_node_mentions_unbound_parameter(
            tp,
            extends_type,
            subst,
            bindings,
            &mut FxHashMap::default(),
        ) {
            return Safe;
        }
        if self.type_node_mentions_infer(tp, extends_type, &mut FxHashMap::default()) {
            return Unknown;
        }
        let check = self.operand_type(tp, check_type, subst, bindings);
        let extends = self.operand_type(tp, extends_type, subst, bindings);
        let selection = self.select_conditional_branch(tp, check, extends, subst, bindings);
        if selection == BranchSelection::UNKNOWN {
            return Unknown;
        }
        let mut verdict = Safe;
        if selection.intersects(BranchSelection::TRUE) {
            verdict = combine_safety(
                verdict,
                self.type_node(tp, node.true_type(), subst, bindings, project),
            );
        }
        if selection.intersects(BranchSelection::FALSE) {
            verdict = combine_safety(
                verdict,
                self.type_node(tp, node.false_type(), subst, bindings, project),
            );
        }
        verdict
    }
}

// Go: typeparser/api_stability_safety.go branchSelection
// branchSelection is a bit set of conditional branches the compiler relation
// evaluates.
crate::flags_macros::go_flags!(BranchSelection, u8 {
    NONE = 0;
    /// Go `branchUnknown`.
    UNKNOWN = 1 << 0;
    /// Go `branchTrue`.
    TRUE = 1 << 1;
    /// Go `branchFalse`.
    FALSE = 1 << 2;
});

impl ApiStabilitySafetyScan<'_> {
    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.selectConditionalBranch
    /// selectConditionalBranch asks the compiler relation which branch a concrete
    /// conditional evaluates. The check type distributes over unions, so a mixed
    /// distribution evaluates both branches. The relation is only asked when both
    /// operands are fully represented and free of type parameters the active
    /// environment replaces, and when a relation-operand pre-flight establishes that
    /// resolving their full projected surfaces is bounded. The guard never
    /// instantiates a branch to answer the question and never evaluates an
    /// unresolved compound substitution.
    pub fn select_conditional_branch(
        &mut self,
        tp: &mut TypeParser<'_>,
        check: TypeId,
        extends: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
    ) -> BranchSelection {
        if check.is_nil() || extends.is_nil() {
            return BranchSelection::UNKNOWN;
        }
        if self.exceeded() {
            return BranchSelection::UNKNOWN;
        }
        let any_type = tp.checker.get_any_type();
        if any_type == check {
            // `any extends X ? A : B` evaluates to `A | B`.
            return BranchSelection::TRUE | BranchSelection::FALSE;
        }
        if tp.checker.ty(check).flags().intersects(TypeFlags::NEVER) {
            // `never` is assignable to every target, and a distributive check over
            // `never` evaluates no branch at all. Scanning only the true branch is
            // sound for both outcomes and never authorizes the false branch.
            return BranchSelection::TRUE;
        }
        if check == extends {
            // Identical represented types are assignable without evaluating any
            // member surface.
            return BranchSelection::TRUE;
        }
        if extends == any_type || extends == tp.checker.get_unknown_type() {
            // Every type is assignable to `any` and to `unknown`.
            return BranchSelection::TRUE;
        }
        if self.contains_replaced_parameter(tp, check, subst, bindings, &mut FxHashMap::default())
            || self.contains_replaced_parameter(
                tp,
                extends,
                subst,
                bindings,
                &mut FxHashMap::default(),
            )
        {
            // An operand still mentions a type parameter the active environment
            // replaces. The checker would instantiate the compound operand before
            // relating it; the guard never authorizes evaluating an unresolved
            // compound substitution.
            return BranchSelection::UNKNOWN;
        }
        // The relation compares the full projected member surfaces of both operands:
        // property types, call and construct signature returns and index infos are
        // all resolved while relating. Unless every one of those components is
        // itself established as bounded by a relation-operand scan, the relation
        // must not be asked, because it would expand them eagerly.
        let operands_safe = {
            let mut operands = self.a.new_safety_scan();
            operands.relation_operands = true;
            operands.type_value(tp, check, subst, bindings, true) == Safe
                && operands.type_value(tp, extends, subst, bindings, true) == Safe
        };
        if !operands_safe {
            return BranchSelection::UNKNOWN;
        }
        let check_members = if tp.checker.ty(check).flags().intersects(TypeFlags::UNION) {
            tp.checker.ty(check).types().to_vec()
        } else {
            vec![check]
        };
        let mut selection = BranchSelection::NONE;
        for member in check_members {
            if tp.checker.is_type_assignable_to(member, extends) {
                selection |= BranchSelection::TRUE;
            } else {
                selection |= BranchSelection::FALSE;
            }
        }
        selection
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.operandType
    /// operandType returns the represented type of a concrete conditional operand,
    /// following only a top-level parameter reference through the active
    /// environment. A compound operand that still mentions a substituted parameter
    /// keeps that parameter and is refused by the relation-operand pre-flight; the
    /// guard never instantiates a compound substitution itself.
    pub fn operand_type(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
    ) -> TypeId {
        if node.is_nil() {
            return TypeId::NIL;
        }
        let mut represented =
            checker_integration::get_resolved_type_from_type_node(tp.checker, node);
        if represented.is_nil() {
            self.a.note_materializing_node_read(node);
            represented = tp.checker.get_type_from_type_node_exported(node);
        }
        if represented.is_nil() {
            return TypeId::NIL;
        }
        let (mapped, replaced) = self.mapped_parameter_value(tp, represented, subst, bindings);
        if replaced {
            return mapped;
        }
        represented
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.containsUnboundParameter
    /// containsUnboundParameter reports whether a represented type still mentions a
    /// type parameter that no active frame replaces.
    // PORT: Go makes `active` when it is nil; callers pass an empty map. Go
    // `defer delete(active, t)` removes the type before normalization.
    pub fn contains_unbound_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        active: &mut FxHashMap<TypeId, bool>,
    ) -> bool {
        if t.is_nil() {
            return false;
        }
        if active.get(&t).copied().unwrap_or(false) {
            return false;
        }
        active.insert(t, true);
        let original = t;
        let result = 'body: {
            let t = non_distributed_parameter(tp.checker, t);
            let flags = tp.checker.ty(t).flags();
            if flags.intersects(TypeFlags::TYPE_PARAMETER) {
                let (parameter, replaced) = self.mapped_parameter_value(tp, t, subst, bindings);
                if replaced {
                    if parameter.is_nil() {
                        break 'body true;
                    }
                    if parameter == t {
                        break 'body false;
                    }
                    break 'body self
                        .contains_unbound_parameter(tp, parameter, subst, bindings, active);
                }
                break 'body true;
            }
            if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                let members = tp.checker.ty(t).types().to_vec();
                for member in members {
                    if self.contains_unbound_parameter(tp, member, subst, bindings, active) {
                        break 'body true;
                    }
                }
            } else if flags.intersects(TypeFlags::OBJECT)
                && tp
                    .checker
                    .ty(t)
                    .object_flags()
                    .intersects(ObjectFlags::REFERENCE)
            {
                for argument in checker_integration::get_resolved_type_arguments(tp.checker, t) {
                    if self.contains_unbound_parameter(tp, argument, subst, bindings, active) {
                        break 'body true;
                    }
                }
            } else if flags.intersects(TypeFlags::CONDITIONAL) {
                let conditional = tp.checker.ty(t).as_conditional_type();
                let (check, extends) = (conditional.check_type, conditional.extends_type);
                break 'body self.contains_unbound_parameter(tp, check, subst, bindings, active)
                    || self.contains_unbound_parameter(tp, extends, subst, bindings, active);
            } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
                let indexed = tp.checker.ty(t).as_indexed_access_type();
                let (object_type, index_type) = (indexed.object_type, indexed.index_type);
                break 'body self.contains_unbound_parameter(
                    tp,
                    object_type,
                    subst,
                    bindings,
                    active,
                ) || self
                    .contains_unbound_parameter(tp, index_type, subst, bindings, active);
            } else if flags.intersects(TypeFlags::INDEX) {
                let target = tp.checker.ty(t).as_index_type().target;
                break 'body self.contains_unbound_parameter(tp, target, subst, bindings, active);
            } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                let parts = tp.checker.ty(t).as_template_literal_type().types.to_vec();
                for part in parts {
                    if self.contains_unbound_parameter(tp, part, subst, bindings, active) {
                        break 'body true;
                    }
                }
            }
            false
        };
        active.remove(&original);
        result
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.containsReplacedParameter
    /// containsReplacedParameter reports whether a represented type still mentions a
    /// type parameter the active environment replaces. The compiler would
    /// instantiate such a compound operand before evaluating a relation over it, so
    /// the guard treats it as unknown instead of guessing a substitution. Reference
    /// arguments are followed directly; an anonymous compound the checker has not
    /// instantiated is inspected through its raw declarations, because its
    /// unsubstituted representation does not show the parameter its resolved members
    /// would carry.
    // PORT: as `contains_unbound_parameter` for `active`.
    pub fn contains_replaced_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        active: &mut FxHashMap<TypeId, bool>,
    ) -> bool {
        if t.is_nil() {
            return false;
        }
        if active.get(&t).copied().unwrap_or(false) {
            return false;
        }
        active.insert(t, true);
        let original = t;
        let result = 'body: {
            let t = non_distributed_parameter(tp.checker, t);
            let flags = tp.checker.ty(t).flags();
            if flags.intersects(TypeFlags::TYPE_PARAMETER) {
                let (_, replaced) = self.mapped_parameter_value(tp, t, subst, bindings);
                break 'body replaced;
            }
            if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                let members = tp.checker.ty(t).types().to_vec();
                for member in members {
                    if self.contains_replaced_parameter(tp, member, subst, bindings, active) {
                        break 'body true;
                    }
                }
            } else if flags.intersects(TypeFlags::OBJECT) {
                let object_flags = tp.checker.ty(t).object_flags();
                if object_flags.intersects(ObjectFlags::REFERENCE) {
                    for argument in checker_integration::get_resolved_type_arguments(tp.checker, t)
                    {
                        if self.contains_replaced_parameter(tp, argument, subst, bindings, active) {
                            break 'body true;
                        }
                    }
                    break 'body false;
                }
                if object_flags.intersects(ObjectFlags::INSTANTIATED) {
                    // The represented instantiation substitutes its members through
                    // its own mapper; the relation-operand scan refuses any raw member
                    // parameter it cannot bind, so no additional raw detection applies.
                    break 'body false;
                }
                // An anonymous compound the checker has not instantiated still
                // contains its raw member annotations. A replaced parameter inside one
                // of them is instantiated before the relation compares the operand,
                // which the represented structure does not show, so the operand is
                // refused.
                let symbol = tp.checker.ty(t).symbol();
                break 'body self
                    .raw_declarations_mention_replaced_parameter(tp, symbol, subst, bindings);
            } else if flags.intersects(TypeFlags::CONDITIONAL) {
                let conditional = tp.checker.ty(t).as_conditional_type();
                let (check, extends) = (conditional.check_type, conditional.extends_type);
                break 'body self.contains_replaced_parameter(tp, check, subst, bindings, active)
                    || self.contains_replaced_parameter(tp, extends, subst, bindings, active);
            } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
                let indexed = tp.checker.ty(t).as_indexed_access_type();
                let (object_type, index_type) = (indexed.object_type, indexed.index_type);
                break 'body self.contains_replaced_parameter(
                    tp,
                    object_type,
                    subst,
                    bindings,
                    active,
                ) || self
                    .contains_replaced_parameter(tp, index_type, subst, bindings, active);
            } else if flags.intersects(TypeFlags::INDEX) {
                let target = tp.checker.ty(t).as_index_type().target;
                break 'body self.contains_replaced_parameter(tp, target, subst, bindings, active);
            } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                let parts = tp.checker.ty(t).as_template_literal_type().types.to_vec();
                for part in parts {
                    if self.contains_replaced_parameter(tp, part, subst, bindings, active) {
                        break 'body true;
                    }
                }
            } else if flags.intersects(TypeFlags::SUBSTITUTION) {
                let substitution = tp.checker.ty(t).as_substitution_type();
                let (base_type, constraint) = (substitution.base_type, substitution.constraint);
                break 'body self
                    .contains_replaced_parameter(tp, base_type, subst, bindings, active)
                    || self.contains_replaced_parameter(tp, constraint, subst, bindings, active);
            }
            false
        };
        active.remove(&original);
        result
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeNodeMentionsUnboundParameter
    /// typeNodeMentionsUnboundParameter reports whether a raw annotation mentions a
    /// type parameter no active frame replaces. An infer binder is bound by its own
    /// conditional, and a callable's own type parameters are bound by the callable,
    /// so neither makes the surrounding conditional deferred. A conditional whose
    /// operands still mention an unbound parameter is deferred and never evaluated
    /// by an ordinary read.
    pub fn type_node_mentions_unbound_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        visiting: &mut FxHashMap<Node, bool>,
    ) -> bool {
        self.type_node_mentions_unbound_parameter_scoped(
            tp,
            node,
            subst,
            bindings,
            visiting,
            &FxHashMap::default(),
        )
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeNodeMentionsUnboundParameterScoped
    /// typeNodeMentionsUnboundParameterScoped carries the type parameters the
    /// enclosing callables bind so a reference to one of them is not mistaken for an
    /// unbound parameter of the surrounding conditional.
    pub fn type_node_mentions_unbound_parameter_scoped(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        visiting: &mut FxHashMap<Node, bool>,
        scope: &FxHashMap<TypeId, bool>,
    ) -> bool {
        if node.is_nil() || visiting.get(&node).copied().unwrap_or(false) {
            return false;
        }
        visiting.insert(node, true);
        let result = 'body: {
            match node.kind() {
                SyntaxKind::InferType => break 'body false,
                SyntaxKind::TypeReference => {
                    let symbol = self.a.symbol_at_type_name_node(tp, node.type_name());
                    if symbol.is_some()
                        && tp
                            .checker
                            .sym(symbol)
                            .flags
                            .intersects(SymbolFlags::TYPE_PARAMETER)
                    {
                        self.a.note_materializing_symbol_read(
                            ApiStabilityMaterializationReadKind::DeclaredType,
                            symbol,
                        );
                        let parameter = tp.checker.get_declared_type_of_symbol_exported(symbol);
                        if parameter.is_some() && !scope.get(&parameter).copied().unwrap_or(false) {
                            let (_, replaced) =
                                self.mapped_parameter_value(tp, parameter, subst, bindings);
                            if !replaced {
                                break 'body true;
                            }
                        }
                    }
                    for argument in node.type_argument_list().nodes().iter() {
                        if self.type_node_mentions_unbound_parameter_scoped(
                            tp, argument, subst, bindings, visiting, scope,
                        ) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::ParenthesizedType
                | SyntaxKind::OptionalType
                | SyntaxKind::RestType
                | SyntaxKind::TypeOperator
                | SyntaxKind::NamedTupleMember
                | SyntaxKind::PropertySignature
                | SyntaxKind::PropertyDeclaration => {
                    break 'body self.type_node_mentions_unbound_parameter_scoped(
                        tp,
                        node.type_(),
                        subst,
                        bindings,
                        visiting,
                        scope,
                    );
                }
                SyntaxKind::ArrayType => {
                    break 'body self.type_node_mentions_unbound_parameter_scoped(
                        tp,
                        node.element_type(),
                        subst,
                        bindings,
                        visiting,
                        scope,
                    );
                }
                SyntaxKind::TupleType => {
                    for element in node.elements().iter() {
                        if self.type_node_mentions_unbound_parameter_scoped(
                            tp, element, subst, bindings, visiting, scope,
                        ) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::UnionType | SyntaxKind::IntersectionType => {
                    for member in node.types().nodes().iter() {
                        if self.type_node_mentions_unbound_parameter_scoped(
                            tp, member, subst, bindings, visiting, scope,
                        ) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::TemplateLiteralType => {
                    for span in node.template_spans().nodes().iter() {
                        if span.is_nil() || span.kind() != SyntaxKind::TemplateLiteralTypeSpan {
                            continue;
                        }
                        if self.type_node_mentions_unbound_parameter_scoped(
                            tp,
                            span.type_(),
                            subst,
                            bindings,
                            visiting,
                            scope,
                        ) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::IndexedAccessType => {
                    break 'body self.type_node_mentions_unbound_parameter_scoped(
                        tp,
                        node.object_type(),
                        subst,
                        bindings,
                        visiting,
                        scope,
                    ) || self.type_node_mentions_unbound_parameter_scoped(
                        tp,
                        node.index_type(),
                        subst,
                        bindings,
                        visiting,
                        scope,
                    );
                }
                SyntaxKind::ConditionalType => {
                    break 'body self.type_node_mentions_unbound_parameter_scoped(
                        tp,
                        node.check_type(),
                        subst,
                        bindings,
                        visiting,
                        scope,
                    ) || self.type_node_mentions_unbound_parameter_scoped(
                        tp,
                        node.extends_type(),
                        subst,
                        bindings,
                        visiting,
                        scope,
                    );
                }
                SyntaxKind::TypeLiteral => {
                    for member in node.members().iter() {
                        if member.is_nil() {
                            continue;
                        }
                        if api_stability_has_function_like_data(member) {
                            if self.function_like_mentions_unbound_parameter(
                                tp, member, subst, bindings, visiting, scope,
                            ) {
                                break 'body true;
                            }
                            continue;
                        }
                        if self.type_node_mentions_unbound_parameter_scoped(
                            tp, member, subst, bindings, visiting, scope,
                        ) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::IndexSignature => {
                    let parameters = node.parameters();
                    if parameters.len() == 1
                        && parameters.get(0).kind() == SyntaxKind::Parameter
                        && self.type_node_mentions_unbound_parameter_scoped(
                            tp,
                            parameters.get(0).type_(),
                            subst,
                            bindings,
                            visiting,
                            scope,
                        )
                    {
                        break 'body true;
                    }
                    break 'body self.type_node_mentions_unbound_parameter_scoped(
                        tp,
                        node.type_(),
                        subst,
                        bindings,
                        visiting,
                        scope,
                    );
                }
                SyntaxKind::MappedType => {
                    if self.type_node_mentions_unbound_parameter_scoped(
                        tp,
                        node.name_type(),
                        subst,
                        bindings,
                        visiting,
                        scope,
                    ) || self.type_node_mentions_unbound_parameter_scoped(
                        tp,
                        node.type_(),
                        subst,
                        bindings,
                        visiting,
                        scope,
                    ) {
                        break 'body true;
                    }
                    let type_parameter = node.type_parameter();
                    if type_parameter.is_some() {
                        break 'body self.type_node_mentions_unbound_parameter_scoped(
                            tp,
                            type_parameter.constraint(),
                            subst,
                            bindings,
                            visiting,
                            scope,
                        );
                    }
                }
                SyntaxKind::FunctionType | SyntaxKind::ConstructorType => {
                    break 'body self.function_like_mentions_unbound_parameter(
                        tp, node, subst, bindings, visiting, scope,
                    );
                }
                _ => {}
            }
            false
        };
        visiting.remove(&node);
        result
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.functionLikeMentionsUnboundParameter
    /// functionLikeMentionsUnboundParameter walks the constraints, parameter and
    /// return annotations of a function-like declaration looking for an unbound
    /// parameter. The declaration's own type parameters are bound by the callable
    /// itself and are excluded through the scope, so a generic operand is not
    /// mistaken for a deferred one.
    pub fn function_like_mentions_unbound_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        function_like: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        visiting: &mut FxHashMap<Node, bool>,
        scope: &FxHashMap<TypeId, bool>,
    ) -> bool {
        if function_like.is_nil() {
            return false;
        }
        let type_parameters = function_like.type_parameter_list();
        let mut own_scope = None;
        if type_parameters.is_some() {
            let mut own = scope.clone();
            for type_parameter in type_parameters.nodes().iter() {
                if type_parameter.is_nil() || type_parameter.kind() != SyntaxKind::TypeParameter {
                    continue;
                }
                let name = get_name_of_declaration(type_parameter);
                if name.is_some() {
                    let symbol = self.a.symbol_at_type_name_node(tp, name);
                    if symbol.is_some() {
                        // The reference symbol of a type parameter can be a
                        // distinct instance; the declared type is the shared
                        // identity, so it is the scope key.
                        self.a.note_materializing_symbol_read(
                            ApiStabilityMaterializationReadKind::DeclaredType,
                            symbol,
                        );
                        let declared = tp.checker.get_declared_type_of_symbol_exported(symbol);
                        if declared.is_some() {
                            own.insert(declared, true);
                        }
                    }
                }
            }
            for type_parameter in type_parameters.nodes().iter() {
                if type_parameter.is_nil() || type_parameter.kind() != SyntaxKind::TypeParameter {
                    continue;
                }
                // A constraint is evaluated in the callable's own scope, so an
                // earlier own type parameter referenced by a later constraint is
                // bound as well.
                if self.type_node_mentions_unbound_parameter_scoped(
                    tp,
                    type_parameter.constraint(),
                    subst,
                    bindings,
                    visiting,
                    &own,
                ) {
                    return true;
                }
            }
            own_scope = Some(own);
        }
        let own = own_scope.as_ref().unwrap_or(scope);
        for parameter in function_like_parameter_nodes(function_like) {
            if parameter.is_some()
                && parameter.kind() == SyntaxKind::Parameter
                && self.type_node_mentions_unbound_parameter_scoped(
                    tp,
                    parameter.type_(),
                    subst,
                    bindings,
                    visiting,
                    own,
                )
            {
                return true;
            }
        }
        let return_type = function_like.type_();
        return_type.is_some()
            && self.type_node_mentions_unbound_parameter_scoped(
                tp,
                return_type,
                subst,
                bindings,
                visiting,
                own,
            )
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeNodeMentionsInfer
    /// typeNodeMentionsInfer reports whether an annotation mentions an infer binder.
    pub fn type_node_mentions_infer(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        visiting: &mut FxHashMap<Node, bool>,
    ) -> bool {
        if node.is_nil() || visiting.get(&node).copied().unwrap_or(false) {
            return false;
        }
        visiting.insert(node, true);
        let result = 'body: {
            match node.kind() {
                SyntaxKind::InferType => break 'body true,
                SyntaxKind::ParenthesizedType
                | SyntaxKind::OptionalType
                | SyntaxKind::RestType
                | SyntaxKind::TypeOperator
                | SyntaxKind::NamedTupleMember => {
                    break 'body self.type_node_mentions_infer(tp, node.type_(), visiting);
                }
                SyntaxKind::ArrayType => {
                    break 'body self.type_node_mentions_infer(tp, node.element_type(), visiting);
                }
                SyntaxKind::TupleType => {
                    for element in node.elements().iter() {
                        if self.type_node_mentions_infer(tp, element, visiting) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::UnionType | SyntaxKind::IntersectionType => {
                    for member in node.types().nodes().iter() {
                        if self.type_node_mentions_infer(tp, member, visiting) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::TemplateLiteralType => {
                    for span in node.template_spans().nodes().iter() {
                        if span.is_some()
                            && span.kind() == SyntaxKind::TemplateLiteralTypeSpan
                            && self.type_node_mentions_infer(tp, span.type_(), visiting)
                        {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::IndexedAccessType => {
                    break 'body self.type_node_mentions_infer(tp, node.object_type(), visiting)
                        || self.type_node_mentions_infer(tp, node.index_type(), visiting);
                }
                SyntaxKind::ConditionalType => {
                    break 'body self.type_node_mentions_infer(tp, node.check_type(), visiting)
                        || self.type_node_mentions_infer(tp, node.extends_type(), visiting)
                        || self.type_node_mentions_infer(tp, node.true_type(), visiting)
                        || self.type_node_mentions_infer(tp, node.false_type(), visiting);
                }
                SyntaxKind::TypeReference => {
                    for argument in node.type_argument_list().nodes().iter() {
                        if self.type_node_mentions_infer(tp, argument, visiting) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::FunctionType | SyntaxKind::ConstructorType => {
                    for parameter in function_like_parameter_nodes(node) {
                        if parameter.is_some()
                            && parameter.kind() == SyntaxKind::Parameter
                            && self.type_node_mentions_infer(tp, parameter.type_(), visiting)
                        {
                            break 'body true;
                        }
                    }
                    let return_type = node.type_();
                    if return_type.is_some()
                        && self.type_node_mentions_infer(tp, return_type, visiting)
                    {
                        break 'body true;
                    }
                }
                _ => {}
            }
            false
        };
        visiting.remove(&node);
        result
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.rawDeclarationsMentionReplacedParameter
    /// rawDeclarationsMentionReplacedParameter reports whether the raw declarations
    /// of a symbol contain an annotation that mentions a type parameter the active
    /// environment replaces. It is a non-evaluating declaration-structure walk used
    /// to refuse a relation operand whose represented compound does not show the
    /// substitution its resolved members would carry. A compound without a
    /// declaration cannot be established as safe and is refused.
    pub fn raw_declarations_mention_replaced_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
    ) -> bool {
        if symbol.is_nil() || tp.checker.sym(symbol).declarations.is_empty() {
            return true;
        }
        let mut visiting = FxHashMap::default();
        let declarations = tp.checker.sym(symbol).declarations.to_vec();
        for declaration in declarations {
            if declaration.is_nil() {
                continue;
            }
            if self.declaration_mentions_replaced_parameter(
                tp,
                declaration,
                subst,
                bindings,
                &mut visiting,
            ) {
                return true;
            }
        }
        false
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.declarationMentionsReplacedParameter
    /// declarationMentionsReplacedParameter walks the public annotations of one raw
    /// declaration: function-like parameters and returns, type literal and heritage
    /// member annotations, index signatures and mapped components. It resolves
    /// nothing.
    pub fn declaration_mentions_replaced_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        declaration: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        visiting: &mut FxHashMap<Node, bool>,
    ) -> bool {
        if declaration.is_nil() {
            return false;
        }
        match declaration.kind() {
            SyntaxKind::InterfaceDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::EnumDeclaration => {
                for member in declaration.members().iter() {
                    if member.is_some()
                        && self.type_node_mentions_replaced_parameter(
                            tp, member, subst, bindings, visiting,
                        )
                    {
                        return true;
                    }
                }
                false
            }
            SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration => self
                .type_node_mentions_replaced_parameter(
                    tp,
                    declaration.type_(),
                    subst,
                    bindings,
                    visiting,
                ),
            SyntaxKind::VariableDeclaration
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::Parameter => self.type_node_mentions_replaced_parameter(
                tp,
                declaration_annotation_node_of(declaration),
                subst,
                bindings,
                visiting,
            ),
            _ => self.type_node_mentions_replaced_parameter(
                tp,
                declaration,
                subst,
                bindings,
                visiting,
            ),
        }
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.functionLikeMentionsReplacedParameter
    /// functionLikeMentionsReplacedParameter walks the parameter, type parameter
    /// constraint and return annotations of a function-like declaration.
    pub fn function_like_mentions_replaced_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        function_like: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        visiting: &mut FxHashMap<Node, bool>,
    ) -> bool {
        if function_like.is_nil() {
            return false;
        }
        for parameter in function_like_parameter_nodes(function_like) {
            if parameter.is_nil() || parameter.kind() != SyntaxKind::Parameter {
                continue;
            }
            if self.type_node_mentions_replaced_parameter(
                tp,
                parameter.type_(),
                subst,
                bindings,
                visiting,
            ) {
                return true;
            }
        }
        for type_parameter in function_like.type_parameter_list().nodes().iter() {
            if type_parameter.is_nil() || type_parameter.kind() != SyntaxKind::TypeParameter {
                continue;
            }
            if self.type_node_mentions_replaced_parameter(
                tp,
                type_parameter.constraint(),
                subst,
                bindings,
                visiting,
            ) {
                return true;
            }
        }
        self.type_node_mentions_replaced_parameter(
            tp,
            function_like.type_(),
            subst,
            bindings,
            visiting,
        )
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeNodeMentionsReplacedParameter
    /// typeNodeMentionsReplacedParameter reports whether a raw annotation mentions a
    /// type parameter the active environment replaces anywhere inside its structure.
    /// It is a non-evaluating syntax gate: a conditional operand that still contains
    /// such a parameter is instantiated by the checker before a relation compares
    /// it, so the guard refuses to ask the relation instead of guessing the
    /// substituted value. An `infer` binder is scoped by its own conditional and is
    /// not an environment parameter.
    pub fn type_node_mentions_replaced_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        visiting: &mut FxHashMap<Node, bool>,
    ) -> bool {
        if node.is_nil() || visiting.get(&node).copied().unwrap_or(false) {
            return false;
        }
        if api_stability_has_function_like_data(node) {
            return self
                .function_like_mentions_replaced_parameter(tp, node, subst, bindings, visiting);
        }
        visiting.insert(node, true);
        let result = 'body: {
            match node.kind() {
                SyntaxKind::InferType => break 'body false,
                SyntaxKind::TypeReference => {
                    let symbol = self.a.symbol_at_type_name_node(tp, node.type_name());
                    if symbol.is_some()
                        && tp
                            .checker
                            .sym(symbol)
                            .flags
                            .intersects(SymbolFlags::TYPE_PARAMETER)
                    {
                        self.a.note_materializing_symbol_read(
                            ApiStabilityMaterializationReadKind::DeclaredType,
                            symbol,
                        );
                        let parameter = tp.checker.get_declared_type_of_symbol_exported(symbol);
                        if parameter.is_some() {
                            let (_, replaced) =
                                self.mapped_parameter_value(tp, parameter, subst, bindings);
                            if replaced {
                                break 'body true;
                            }
                        }
                    }
                    for argument in node.type_argument_list().nodes().iter() {
                        if self.type_node_mentions_replaced_parameter(
                            tp, argument, subst, bindings, visiting,
                        ) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::ParenthesizedType
                | SyntaxKind::OptionalType
                | SyntaxKind::RestType
                | SyntaxKind::TypeOperator
                | SyntaxKind::NamedTupleMember
                | SyntaxKind::PropertySignature
                | SyntaxKind::PropertyDeclaration
                | SyntaxKind::Parameter => {
                    break 'body self.type_node_mentions_replaced_parameter(
                        tp,
                        node.type_(),
                        subst,
                        bindings,
                        visiting,
                    );
                }
                SyntaxKind::ArrayType => {
                    break 'body self.type_node_mentions_replaced_parameter(
                        tp,
                        node.element_type(),
                        subst,
                        bindings,
                        visiting,
                    );
                }
                SyntaxKind::TupleType => {
                    for element in node.elements().iter() {
                        if self.type_node_mentions_replaced_parameter(
                            tp, element, subst, bindings, visiting,
                        ) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::UnionType | SyntaxKind::IntersectionType => {
                    for member in node.types().nodes().iter() {
                        if self.type_node_mentions_replaced_parameter(
                            tp, member, subst, bindings, visiting,
                        ) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::TemplateLiteralType => {
                    for span in node.template_spans().nodes().iter() {
                        if span.is_nil() || span.kind() != SyntaxKind::TemplateLiteralTypeSpan {
                            continue;
                        }
                        if self.type_node_mentions_replaced_parameter(
                            tp,
                            span.type_(),
                            subst,
                            bindings,
                            visiting,
                        ) {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::IndexedAccessType => {
                    break 'body self.type_node_mentions_replaced_parameter(
                        tp,
                        node.object_type(),
                        subst,
                        bindings,
                        visiting,
                    ) || self.type_node_mentions_replaced_parameter(
                        tp,
                        node.index_type(),
                        subst,
                        bindings,
                        visiting,
                    );
                }
                SyntaxKind::ConditionalType => {
                    break 'body self.type_node_mentions_replaced_parameter(
                        tp,
                        node.check_type(),
                        subst,
                        bindings,
                        visiting,
                    ) || self.type_node_mentions_replaced_parameter(
                        tp,
                        node.extends_type(),
                        subst,
                        bindings,
                        visiting,
                    ) || self.type_node_mentions_replaced_parameter(
                        tp,
                        node.true_type(),
                        subst,
                        bindings,
                        visiting,
                    ) || self.type_node_mentions_replaced_parameter(
                        tp,
                        node.false_type(),
                        subst,
                        bindings,
                        visiting,
                    );
                }
                SyntaxKind::TypeLiteral => {
                    for member in node.members().iter() {
                        if member.is_some()
                            && self.type_node_mentions_replaced_parameter(
                                tp, member, subst, bindings, visiting,
                            )
                        {
                            break 'body true;
                        }
                    }
                }
                SyntaxKind::MappedType => {
                    if self.type_node_mentions_replaced_parameter(
                        tp,
                        node.name_type(),
                        subst,
                        bindings,
                        visiting,
                    ) || self.type_node_mentions_replaced_parameter(
                        tp,
                        node.type_(),
                        subst,
                        bindings,
                        visiting,
                    ) {
                        break 'body true;
                    }
                    let type_parameter = node.type_parameter();
                    if type_parameter.is_some() {
                        break 'body self.type_node_mentions_replaced_parameter(
                            tp,
                            type_parameter.constraint(),
                            subst,
                            bindings,
                            visiting,
                        );
                    }
                }
                // PORT: Go keeps an `IndexSignature` case here, but an index
                // signature has function-like data, so the check above takes it.
                _ => {}
            }
            false
        };
        visiting.remove(&node);
        result
    }
}

impl ApiStabilitySafetyScan<'_> {
    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeLiteral
    /// typeLiteral scans a type literal annotation. Index signatures are evaluated
    /// eagerly by member resolution; property and method annotations are evaluated
    /// only when the literal is projected into.
    pub fn type_literal(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let mut verdict = Safe;
        for member in node.members().iter() {
            if member.is_nil() {
                continue;
            }
            verdict = combine_safety(
                verdict,
                self.member_annotation(tp, member, subst, bindings, project),
            );
        }
        verdict
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.mappedTypeNode
    /// mappedTypeNode scans a mapped type annotation through its eagerly evaluated
    /// constraint, name and template annotations.
    pub fn mapped_type_node(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let mut verdict = self.type_node(tp, node.name_type(), subst, bindings, project);
        verdict = combine_safety(
            verdict,
            self.type_node(tp, node.type_(), subst, bindings, project),
        );
        let type_parameter = node.type_parameter();
        if type_parameter.is_some() {
            verdict = combine_safety(
                verdict,
                self.type_node(tp, type_parameter.constraint(), subst, bindings, project),
            );
        }
        verdict
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.memberAnnotation
    /// memberAnnotation scans one class, interface or type literal member. Index
    /// signatures and declared call and construct signatures are always evaluated by
    /// member resolution; a named property or method is evaluated only when the
    /// member surface is projected into.
    pub fn member_annotation(
        &mut self,
        tp: &mut TypeParser<'_>,
        member: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        match member.kind() {
            SyntaxKind::IndexSignature => {
                let mut verdict = Safe;
                let parameters = member.parameters();
                if parameters.len() == 1 && parameters.get(0).kind() == SyntaxKind::Parameter {
                    verdict = self.type_node(tp, parameters.get(0).type_(), subst, bindings, true);
                }
                combine_safety(
                    verdict,
                    self.type_node(tp, member.type_(), subst, bindings, true),
                )
            }
            SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::Constructor => {
                if self.member_has_late_bound_name(member) {
                    return Unknown;
                }
                self.function_like_resolution(tp, member, subst, bindings)
            }
            SyntaxKind::PropertySignature | SyntaxKind::PropertyDeclaration => {
                if !project {
                    return Safe;
                }
                if self.member_has_late_bound_name(member) {
                    return Unknown;
                }
                let annotation = member.type_();
                if annotation.is_nil() && self.relation_operands {
                    // The relation compares the member's resolved type. A property
                    // without an annotation is only resolvable by inference (an
                    // initializer or an implicit `any`), which the guard refuses to
                    // force unless the checker already materialized the component.
                    let symbol = self.member_symbol_of_declaration(tp, member);
                    if symbol.is_some() {
                        let materialized =
                            checker_integration::get_resolved_type_of_symbol_if_materialized(
                                tp.checker, symbol,
                            );
                        if materialized.is_some() {
                            return self.type_value(tp, materialized, subst, bindings, true);
                        }
                    }
                    return Unknown;
                }
                self.type_node(tp, annotation, subst, bindings, true)
            }
            SyntaxKind::MethodSignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor => {
                if self.member_has_late_bound_name(member) {
                    return Unknown;
                }
                if !project {
                    return Safe;
                }
                self.function_like_resolution(tp, member, subst, bindings)
            }
            _ => {
                if self.member_has_late_bound_name(member) {
                    return Unknown;
                }
                Safe
            }
        }
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.functionLikeResolution
    /// functionLikeResolution scans the parameter and type parameter constraint
    /// annotations a function-like declaration resolves eagerly. The return
    /// annotation is lazy and gated by its own read; a relation-operand scan reads
    /// it too, and projects parameter and return member surfaces, because the
    /// relation compares them structurally and would instantiate them. A relation
    /// operand whose inferred return the relation would have to infer from a body is
    /// refused.
    pub fn function_like_resolution(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
    ) -> ApiStabilitySafety {
        if !api_stability_has_function_like_data(node) {
            return Safe;
        }
        // The relation resolves the full projected member surfaces of parameters
        // and returns while it compares them, so a relation-operand scan projects
        // into both positions exactly as the relation would.
        let project = self.relation_operands;
        let mut verdict = Safe;
        for parameter in function_like_parameter_nodes(node) {
            if parameter.is_nil() || parameter.kind() != SyntaxKind::Parameter {
                continue;
            }
            verdict = combine_safety(
                verdict,
                self.type_node(tp, parameter.type_(), subst, bindings, project),
            );
        }
        for type_parameter in node.type_parameter_list().nodes().iter() {
            if type_parameter.is_nil() || type_parameter.kind() != SyntaxKind::TypeParameter {
                continue;
            }
            if self.relation_operands {
                // The relation instantiates a generic signature in the context
                // of the other operand and resolves the declaration's
                // constraints and defaults while inferring. A constraint or
                // default whose raw structure mentions a replaced parameter, or
                // whose projected surface would expand a recursive alias, is
                // refused rather than evaluated.
                let mut visiting = FxHashMap::default();
                if self.type_node_mentions_replaced_parameter(
                    tp,
                    type_parameter.constraint(),
                    subst,
                    bindings,
                    &mut visiting,
                ) || self.type_node_mentions_replaced_parameter(
                    tp,
                    type_parameter.default_type(),
                    subst,
                    bindings,
                    &mut visiting,
                ) {
                    return Unknown;
                }
            }
            verdict = combine_safety(
                verdict,
                self.type_node(tp, type_parameter.constraint(), subst, bindings, project),
            );
            if self.relation_operands {
                verdict = combine_safety(
                    verdict,
                    self.type_node(tp, type_parameter.default_type(), subst, bindings, project),
                );
            }
        }
        if self.relation_operands {
            let return_type = node.type_();
            if return_type.is_nil() && !node_is_missing(node.body()) {
                // The relation would infer the return type from the body, which the
                // guard refuses to force.
                return Unknown;
            }
            verdict = combine_safety(
                verdict,
                self.type_node(tp, return_type, subst, bindings, project),
            );
        }
        verdict
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.memberSymbolOfDeclaration
    /// memberSymbolOfDeclaration returns the checker symbol a member declaration
    /// names, or nil when the declaration does not name one. It only looks up the
    /// declared name; no member type is resolved.
    pub fn member_symbol_of_declaration(
        &mut self,
        tp: &mut TypeParser<'_>,
        member: Node,
    ) -> SymbolId {
        if member.is_nil() {
            return SymbolId::NIL;
        }
        let name = get_name_of_declaration(member);
        if name.is_nil() || name.kind() == SyntaxKind::ComputedPropertyName {
            return SymbolId::NIL;
        }
        self.a.symbol_at_type_name_node(tp, name)
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.memberHasLateBoundName
    /// memberHasLateBoundName reports whether a member's declared name must be
    /// evaluated to resolve the member table. A computed name whose expression is an
    /// entity name is late-bindable and is refused rather than checked, except for
    /// the well-known `Symbol.*` names whose evaluation is a pure library lookup.
    pub fn member_has_late_bound_name(&mut self, member: Node) -> bool {
        let name = get_name_of_declaration(member);
        if name.is_nil() || name.kind() != SyntaxKind::ComputedPropertyName {
            return false;
        }
        let expression = name.expression();
        if expression.is_nil() || !is_entity_name_expression(expression) {
            return false;
        }
        !api_stability_well_known_symbol_name(expression)
    }
}

// Go: typeparser/api_stability_safety.go apiStabilityWellKnownSymbolName
/// apiStabilityWellKnownSymbolName reports whether an entity-name expression is
/// a `Symbol.<well-known>` reference that resolves to a library unique symbol
/// without checking program code.
pub fn api_stability_well_known_symbol_name(expression: Node) -> bool {
    if expression.is_nil() || expression.kind() != SyntaxKind::PropertyAccessExpression {
        return false;
    }
    let (object, name) = (expression.expression(), expression.name());
    if object.is_nil() || name.is_nil() {
        return false;
    }
    if object.kind() != SyntaxKind::Identifier || object.text() != "Symbol" {
        return false;
    }
    matches!(
        name.text(),
        "iterator"
            | "asyncIterator"
            | "hasInstance"
            | "isConcatSpreadable"
            | "match"
            | "matchAll"
            | "replace"
            | "search"
            | "species"
            | "split"
            | "toPrimitive"
            | "toStringTag"
            | "unscopables"
    )
}

// Go: typeparser/api_stability_safety.go apiStabilityWellKnownSymbolDisplayName
/// apiStabilityWellKnownSymbolDisplayName renders a well-known `Symbol.<name>`
/// computed-name expression for diagnostics, or "" when the expression is not a
/// well-known symbol reference. It reads the expression's identifiers only and
/// never evaluates it.
pub fn api_stability_well_known_symbol_display_name(expression: Node) -> String {
    if !api_stability_well_known_symbol_name(expression) {
        return String::new();
    }
    format!("[Symbol.{}]", expression.name().text())
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability_safety.go apiStabilityAnalysis.symbolHasLateBoundMembers
    /// symbolHasLateBoundMembers reports whether any declaration member of a symbol
    /// still carries the binder's unresolved computed-name placeholder. Such a
    /// member is absent from the early member table and only appears once the
    /// checker late-binds its name, so a surface read from the early table is
    /// incomplete. A computed member the binder could name with a literal (or a
    /// literal type) is already in the early table and does not set the flag.
    pub fn symbol_has_late_bound_members(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
    ) -> bool {
        if symbol.is_nil() {
            return false;
        }
        let c = &*tp.checker;
        for &declaration in &c.sym(symbol).declarations {
            if declaration.is_nil() {
                continue;
            }
            if !matches!(
                declaration.kind(),
                SyntaxKind::InterfaceDeclaration
                    | SyntaxKind::ClassDeclaration
                    | SyntaxKind::ClassExpression
                    | SyntaxKind::TypeLiteral
                    | SyntaxKind::EnumDeclaration
            ) {
                continue;
            }
            for member in declaration.members().iter() {
                if member.is_nil() {
                    continue;
                }
                let member_symbol = member.symbol();
                if member_symbol.is_some()
                    && c.sym(member_symbol).name == INTERNAL_SYMBOL_NAME_COMPUTED
                {
                    return true;
                }
            }
        }
        false
    }
}

impl ApiStabilitySafetyScan<'_> {
    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.declarationMembers
    /// declarationMembers scans the member resolution of a declaration symbol: index
    /// signature annotations are evaluated eagerly, heritage is resolved
    /// recursively, and a projected read evaluates the member annotations too.
    pub fn declaration_members(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
        arguments: NodeList,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        if symbol.is_nil() {
            return Unknown;
        }
        let params = tp
            .checker
            .get_local_type_parameters_of_class_or_interface_or_type_alias_exported(symbol);
        if params.is_empty() {
            return self.declaration_structure(tp, symbol, subst, bindings, project);
        }
        let mut args = vec![ApiStabilitySafetyArg::default(); params.len()];
        for (index, argument) in arguments.nodes().iter().enumerate() {
            if index >= params.len() || argument.is_nil() {
                continue;
            }
            args[index] = ApiStabilitySafetyArg {
                node: argument,
                subst: subst.clone(),
                bindings: bindings.cloned(),
                ..Default::default()
            };
        }
        let frame = Rc::new(ApiStabilitySafetyBinding {
            parent: bindings.cloned(),
            params,
            args,
        });
        self.declaration_structure(tp, symbol, subst, Some(&frame), project)
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.declarationMembersOfRepresentedArguments
    /// declarationMembersOfRepresentedArguments scans a reference target's
    /// declaration surface with the reference's represented arguments bound as
    /// concrete values. A nil argument stays an empty binding, so a parameter the
    /// checker has not represented cannot fall back to its constraint.
    pub fn declaration_members_of_represented_arguments(
        &mut self,
        tp: &mut TypeParser<'_>,
        target: TypeId,
        arguments: &[TypeId],
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        if target.is_nil() {
            return Unknown;
        }
        let symbol = tp.checker.ty(target).symbol();
        if symbol.is_nil() {
            return Unknown;
        }
        let params = tp
            .checker
            .get_local_type_parameters_of_class_or_interface_or_type_alias_exported(symbol);
        if params.is_empty() {
            return self.declaration_structure(tp, symbol, subst, bindings, project);
        }
        let args = (0..params.len())
            .map(|index| match arguments.get(index) {
                Some(&argument) if argument.is_some() => ApiStabilitySafetyArg {
                    t: argument,
                    ..Default::default()
                },
                _ => ApiStabilitySafetyArg::default(),
            })
            .collect();
        let frame = Rc::new(ApiStabilitySafetyBinding {
            parent: bindings.cloned(),
            params,
            args,
        });
        self.declaration_structure(tp, symbol, subst, Some(&frame), project)
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.declarationStructure
    /// declarationStructure scans a declaration symbol's own member and heritage
    /// structure.
    pub fn declaration_structure(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        if symbol.is_nil() {
            return Unknown;
        }
        if self.relation_operands && !self.a.should_inspect_symbol(tp, symbol) {
            // A default-library declaration is fixed and never references a program
            // alias; the relation may compare it freely. Only program symbols
            // reached through its represented arguments can carry a recursive
            // alias.
            return Safe;
        }
        if self
            .active_declarations
            .get(&symbol)
            .copied()
            .unwrap_or(false)
        {
            return Unknown;
        }
        if self.exceeded() {
            return Unknown;
        }
        self.active_declarations.insert(symbol, true);

        let mut verdict = Safe;
        let declarations = tp.checker.sym(symbol).declarations.to_vec();
        for declaration in declarations {
            if declaration.is_nil() {
                continue;
            }
            match declaration.kind() {
                SyntaxKind::InterfaceDeclaration
                | SyntaxKind::ClassDeclaration
                | SyntaxKind::ClassExpression
                | SyntaxKind::TypeLiteral
                | SyntaxKind::EnumDeclaration => {
                    for member in declaration.members().iter() {
                        verdict = combine_safety(
                            verdict,
                            self.member_annotation(tp, member, subst, bindings, project),
                        );
                    }
                    verdict = combine_safety(
                        verdict,
                        self.heritage(tp, symbol, subst, bindings, project),
                    );
                }
                SyntaxKind::MappedType => {
                    verdict = combine_safety(
                        verdict,
                        self.mapped_type_node(tp, declaration, subst, bindings, project),
                    );
                }
                _ => {
                    if api_stability_has_function_like_data(declaration) && self.relation_operands {
                        verdict = combine_safety(
                            verdict,
                            self.function_like_resolution(tp, declaration, subst, bindings),
                        );
                        continue;
                    }
                    let annotation = declaration_annotation_node_of(declaration);
                    verdict = combine_safety(
                        verdict,
                        self.type_node(tp, annotation, subst, bindings, project),
                    );
                }
            }
        }
        self.active_declarations.remove(&symbol);
        verdict
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.heritage
    /// heritage scans the heritage clause type arguments a base-type resolution
    /// evaluates, and recursively inspects the base declarations' own member
    /// structure because member resolution walks the whole inheritance chain. The
    /// projection flag is propagated: when a projected read evaluates the derived
    /// member surface, inherited member annotations are evaluated with it.
    pub fn heritage(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        if symbol.is_nil() {
            return Safe;
        }
        let mut verdict = Safe;
        let declarations = tp.checker.sym(symbol).declarations.to_vec();
        for declaration in declarations {
            if declaration.is_nil() {
                continue;
            }
            let clauses = match declaration.kind() {
                SyntaxKind::ClassDeclaration
                | SyntaxKind::ClassExpression
                | SyntaxKind::InterfaceDeclaration => declaration.heritage_clauses(),
                _ => NodeList::NIL,
            };
            for clause in clauses.nodes().iter() {
                if clause.is_nil() {
                    continue;
                }
                for type_node in clause.types().nodes().iter() {
                    if type_node.is_nil() {
                        continue;
                    }
                    // An interface heritage entry is written as a type reference,
                    // a class heritage entry as an expression with type arguments.
                    let (base_symbol, arguments) = match type_node.kind() {
                        SyntaxKind::TypeReference => (
                            self.a.symbol_at_type_name_node(tp, type_node.type_name()),
                            type_node.type_argument_list(),
                        ),
                        SyntaxKind::ExpressionWithTypeArguments => (
                            self.a.symbol_at_type_name_node(tp, type_node.expression()),
                            type_node.type_argument_list(),
                        ),
                        _ => continue,
                    };
                    if base_symbol.is_nil() {
                        verdict = combine_safety(verdict, Unknown);
                        continue;
                    }
                    if arguments.is_some() {
                        verdict = combine_safety(
                            verdict,
                            self.type_arguments(tp, arguments, subst, bindings),
                        );
                    }
                    let base_params = tp
                        .checker
                        .get_local_type_parameters_of_class_or_interface_or_type_alias_exported(
                            base_symbol,
                        );
                    if base_params.is_empty() {
                        verdict = combine_safety(
                            verdict,
                            self.declaration_structure(tp, base_symbol, subst, bindings, project),
                        );
                        continue;
                    }
                    let mut args = vec![ApiStabilitySafetyArg::default(); base_params.len()];
                    for (index, argument) in arguments.nodes().iter().enumerate() {
                        if index >= base_params.len() || argument.is_nil() {
                            continue;
                        }
                        args[index] = ApiStabilitySafetyArg {
                            node: argument,
                            subst: subst.clone(),
                            bindings: bindings.cloned(),
                            ..Default::default()
                        };
                    }
                    let frame = Rc::new(ApiStabilitySafetyBinding {
                        parent: bindings.cloned(),
                        params: base_params,
                        args,
                    });
                    verdict = combine_safety(
                        verdict,
                        self.declaration_structure(tp, base_symbol, subst, Some(&frame), project),
                    );
                }
            }
        }
        verdict
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.memberTable
    /// memberTable scans a represented type that is about to have its member table
    /// resolved.
    pub fn member_table(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
    ) -> ApiStabilitySafety {
        if t.is_nil() {
            return Safe;
        }
        if self.exceeded() {
            return Unknown;
        }
        let t = non_distributed_parameter(tp.checker, t);
        let flags = tp.checker.ty(t).flags();
        if flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.constraint_safety(tp, t, subst, bindings, true);
        }
        if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let mut verdict = Safe;
            let members = tp.checker.ty(t).types().to_vec();
            for member in members {
                verdict = combine_safety(verdict, self.member_table(tp, member, subst, bindings));
            }
            return verdict;
        }
        if flags.intersects(TypeFlags::OBJECT) {
            let object_flags = tp.checker.ty(t).object_flags();
            if object_flags.intersects(ObjectFlags::MAPPED) {
                return self.mapped_value(tp, t, subst, bindings, false);
            }
            if object_flags.intersects(ObjectFlags::REFERENCE) {
                let target = tp.checker.ty(t).target();
                if target.is_some() && target != t {
                    let mut verdict = Safe;
                    for argument in checker_integration::get_resolved_type_arguments(tp.checker, t)
                    {
                        verdict = combine_safety(
                            verdict,
                            self.type_value(tp, argument, subst, bindings, false),
                        );
                    }
                    let target_symbol = tp.checker.ty(target).symbol();
                    return combine_safety(
                        verdict,
                        self.declaration_structure(tp, target_symbol, subst, bindings, false),
                    );
                }
            }
            let symbol = tp.checker.ty(t).symbol();
            if symbol.is_some() {
                return self.declaration_structure(tp, symbol, subst, bindings, false);
            }
            return Safe;
        }
        Safe
    }

    // Go: typeparser/api_stability_safety.go apiStabilitySafetyScan.typeQuery
    /// typeQuery scans a `typeof` annotation through the value symbol it names.
    pub fn type_query(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
        bindings: Option<&Rc<ApiStabilitySafetyBinding>>,
        project: bool,
    ) -> ApiStabilitySafety {
        let expr_name = node.expr_name();
        if expr_name.is_nil() {
            return Unknown;
        }
        let symbol = self.a.symbol_at_type_name_node(tp, expr_name);
        if symbol.is_nil() {
            return Unknown;
        }
        let materialized =
            checker_integration::get_resolved_type_of_symbol_if_materialized(tp.checker, symbol);
        if materialized.is_some() {
            return self.type_value(tp, materialized, subst, bindings, project);
        }
        let declarations = tp.checker.sym(symbol).declarations.to_vec();
        for declaration in declarations {
            if declaration.is_nil() {
                continue;
            }
            if api_stability_has_function_like_data(declaration) {
                let verdict = self.function_like_resolution(tp, declaration, subst, bindings);
                if verdict != Safe {
                    return verdict;
                }
                continue;
            }
            let annotation = declaration_annotation_node_of(declaration);
            if annotation.is_nil() {
                if self.relation_operands {
                    // The relation would resolve the inferred value's type.
                    return Unknown;
                }
                continue;
            }
            let verdict = self.type_node(tp, annotation, subst, bindings, project);
            if verdict != Safe {
                return verdict;
            }
        }
        Safe
    }
}

// Go: typeparser/api_stability_safety.go nonDistributedParameter
/// nonDistributedParameter normalizes a distributed conditional's cloned check
/// parameter back to its declared binder.
pub fn non_distributed_parameter(c: &Checker, t: TypeId) -> TypeId {
    if t.is_nil() || !c.ty(t).flags().intersects(TypeFlags::TYPE_PARAMETER) {
        return t;
    }
    let normalized = checker_integration::get_non_distributed_type_parameter(c, t);
    if normalized.is_some() && normalized != t {
        return normalized;
    }
    t
}

// Go: typeparser/api_stability_safety.go safetyTypeParameterDefaults
/// safetyTypeParameterDefaults returns the declared default annotations of a
/// symbol's type parameters, aligned with the symbol's local type parameter
/// order.
// PORT: Go reads `symbol.Declarations` without a checker; here it takes `c`.
pub fn safety_type_parameter_defaults(c: &Checker, symbol: SymbolId) -> Vec<Node> {
    if symbol.is_nil() {
        return Vec::new();
    }
    for &declaration in &c.sym(symbol).declarations {
        if declaration.is_nil() {
            continue;
        }
        let type_parameters = match declaration.kind() {
            SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression => declaration.type_parameter_list(),
            _ => NodeList::NIL,
        };
        if type_parameters.is_nil() {
            continue;
        }
        return type_parameters
            .nodes()
            .iter()
            .map(|type_parameter| {
                if type_parameter.is_nil() || type_parameter.kind() != SyntaxKind::TypeParameter {
                    Node::NIL
                } else {
                    type_parameter.default_type()
                }
            })
            .collect();
    }
    Vec::new()
}

// Go: typeparser/api_stability_safety.go functionLikeParameterNodes
/// functionLikeParameterNodes returns the declared parameters of a function-like
/// node, or nil when it declares none.
pub fn function_like_parameter_nodes(function_like: Node) -> Vec<Node> {
    if function_like.is_nil() {
        return Vec::new();
    }
    function_like.parameter_list().nodes().iter().collect()
}

// Go: typeparser/api_stability_safety.go declarationAnnotationNodeOf
/// declarationAnnotationNodeOf returns the type annotation node a declaration
/// resolves, or nil for an inferred declaration.
pub fn declaration_annotation_node_of(declaration: Node) -> Node {
    if declaration.is_nil() {
        return Node::NIL;
    }
    match declaration.kind() {
        SyntaxKind::VariableDeclaration
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::Parameter
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::GetAccessor => declaration.type_(),
        SyntaxKind::SetAccessor => {
            for parameter in function_like_parameter_nodes(declaration) {
                if parameter.is_some() && parameter.kind() == SyntaxKind::Parameter {
                    return parameter.type_();
                }
            }
            Node::NIL
        }
        _ => Node::NIL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};

    fn parse(source: &'static str) -> Node {
        let opts = SourceFileParseOptions {
            file_name: "/test.ts".to_string(),
            ..Default::default()
        };
        parse_source_file(&opts, source, ScriptKind::TS).root
    }

    #[test]
    fn combine_safety_order() {
        assert_eq!(combine_safety(Safe, Safe), Safe);
        assert_eq!(combine_safety(Safe, Unknown), Unknown);
        assert_eq!(combine_safety(Unknown, Recursive), Recursive);
        assert_eq!(combine_safety(Recursive, Safe), Recursive);
    }

    /// The computed names of an interface: `Symbol.iterator` is well known,
    /// `Symbol.other` and `key` are not.
    #[test]
    fn well_known_symbol_names() {
        let root = parse(
            "interface I { [Symbol.iterator](): void; [Symbol.other]: 1; [key]: 2; [Other.iterator]: 3 }",
        );
        let members = root.statements().get(0).members();
        let expression = |i: usize| members.get(i).name().expression();
        assert!(api_stability_well_known_symbol_name(expression(0)));
        assert_eq!(
            api_stability_well_known_symbol_display_name(expression(0)),
            "[Symbol.iterator]"
        );
        for i in 1..4 {
            assert!(!api_stability_well_known_symbol_name(expression(i)));
            assert_eq!(
                api_stability_well_known_symbol_display_name(expression(i)),
                ""
            );
        }
    }

    #[test]
    fn declaration_annotation_nodes() {
        let root = parse(
            "class C { get a(): string { return '' } set b(v: number) {} c = 1 }\ntype T<A, B = string> = A",
        );
        let members = root.statements().get(0).members();
        assert_eq!(
            declaration_annotation_node_of(members.get(0)).kind(),
            SyntaxKind::StringKeyword
        );
        assert_eq!(
            declaration_annotation_node_of(members.get(1)).kind(),
            SyntaxKind::NumberKeyword
        );
        assert!(declaration_annotation_node_of(members.get(2)).is_nil());
        let alias = root.statements().get(1);
        assert_eq!(
            declaration_annotation_node_of(alias).kind(),
            SyntaxKind::TypeReference
        );
        let parameters = alias.type_parameter_list().nodes();
        assert!(parameters.get(0).default_type().is_nil());
        assert_eq!(
            parameters.get(1).default_type().kind(),
            SyntaxKind::StringKeyword
        );
    }
}
