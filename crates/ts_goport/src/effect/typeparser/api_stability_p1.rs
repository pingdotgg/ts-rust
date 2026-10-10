//! Port of Effect-TS/tsgo `internal/typeparser/api_stability.go` lines 1-1153
//! at `@effect/tsgo@0.51.1` (`47cb1ed7`): the stability levels, the declared
//! lookups, the session API, the result conversion and display names, and the
//! analysis state.
//!
//! The Go file is 5,023 lines, so the port splits it in Go order:
//! `api_stability_p1.rs` (Go 1-1153), `api_stability_p2.rs` (Go 1154-2682,
//! settlement, shared caches, symbol and type surfaces) and
//! `api_stability_p3.rs` (Go 2683-5023, declaration surfaces, provenance,
//! findings and filters).
//!
//! PORT: Go `apiStabilityAnalysis` keeps `tp *TypeParser`. Here a session (and
//! its analysis) lives across rule calls that also use the type parser, so the
//! analysis does not borrow it: every analysis method that reads Go `a.tp`
//! takes `tp: &mut TypeParser<'_>`. Go `a.tp.checker` is `tp.checker`.
//!
//! PORT: Go map iteration order is random. The session result maps and the
//! finding maps are `FxIndexMap` (insertion order), so the port visits them in
//! one fixed order where Go has no order (the results are sorted or order
//! free).

use crate::effect::typeparser::*;
use crate::prelude::*;

/// Go `ApiStabilityLevel`.
/// ApiStabilityLevel orders the Effect stability tiers. A higher level is less
/// stable, so a public surface may only expose dependencies whose level is at
/// most the level declared for the export that owns the surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApiStabilityLevel {
    /// Go `ApiStabilityStable`.
    #[default]
    Stable = 0,
    /// Go `ApiStabilityUnstable`.
    Unstable = 1,
    /// Go `ApiStabilityExperimental`.
    Experimental = 2,
}

// Go: typeparser/api_stability.go ApiStabilityLevelTag
/// ApiStabilityLevelTag returns the `@stability` tag spelling for a level as
/// rendered by the usage diagnostics. The stable tier is spelled as the empty
/// string because it is the implicit default; an explicit `@stability stable`
/// tag is still parsed (see stabilityOfDeclarationTag) and keeps its declaration
/// as provenance, so the declared accessors distinguish it from an absent tag
/// even though this spelling helper returns the same empty string for both.
pub fn api_stability_level_tag(level: ApiStabilityLevel) -> &'static str {
    match level {
        ApiStabilityLevel::Unstable => "unstable",
        ApiStabilityLevel::Experimental => "experimental",
        ApiStabilityLevel::Stable => "",
    }
}

// Go: typeparser/api_stability.go ApiStabilityLevelFromTag
/// ApiStabilityLevelFromTag maps a `@stability` tag spelling to a level. An
/// unrecognized or empty tag is stable.
pub fn api_stability_level_from_tag(tag: &str) -> ApiStabilityLevel {
    api_stability_rank(tag)
}

/// Go `ApiStabilityDeclaration`.
/// ApiStabilityDeclaration is a declared stability tier together with the
/// declaration that carries the tag. The declaration is retained for callers
/// that need provenance (allow-list naming and diagnostic locations). An
/// explicit `@stability stable` tag keeps its declaration, distinct from an
/// untagged default-stable symbol, which has no declaration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApiStabilityDeclaration {
    pub level: ApiStabilityLevel,
    pub declaration: Node,
}

/// Go `ApiStabilityDependency`.
/// ApiStabilityDependency names one exposed component of a public API surface
/// whose declared stability is above stable. The offender is either a symbol
/// (properties, methods, referenced types) or a signature declaration; the
/// declaration carries the `@stability` provenance used for reporting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApiStabilityDependency {
    pub symbol: SymbolId,
    pub signature: SignatureId,
    pub declaration: Node,
    pub level: ApiStabilityLevel,
}

/// Go `ApiStabilityUsed`.
/// ApiStabilityUsed is the computed stability of the components a surface
/// exposes: the minimum guaranteed stability (the least stable dependency found,
/// stable when none was found) and the offending components themselves.
///
/// The surface of a root component (a symbol, a represented type or a signature)
/// excludes the root's own declared tag. Declared stability is a separate
/// lookup: DeclaredApiStabilityOfSymbol and DeclaredApiStabilityOfSignature
/// return the component's own tag and its declaration, are cached per checker,
/// and never trigger a computed surface. The same component reached as a child
/// of another root contributes its own tag to that parent surface; only the
/// root conversion removes it, so the exclusion is a result semantic and never
/// a traversal stop.
///
/// Incomplete reports that the analysis could not establish every component of
/// the surface (a component is unavailable or a guarded read was refused), so
/// the dependency list may be missing findings and must not be treated as a
/// stable result; the incomplete result is discarded with its session and a
/// later export or call retries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApiStabilityUsed {
    pub minimum: ApiStabilityLevel,
    pub dependencies: Vec<ApiStabilityDependency>,
    pub incomplete: bool,
}

/// Go `apiStabilityInspection`.
/// apiStabilityInspection distinguishes how a represented type is inspected. A
/// named class or interface reached as a dependency stays shallow: its symbol
/// and its public call/construct signatures are inspected, but its members are
/// not recursively expanded. The direct surface of an export is expanded: own
/// and inherited members, index signatures and signatures are all inspected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ApiStabilityInspection {
    /// Go `apiStabilityShallow`.
    #[default]
    Shallow = 0,
    /// Go `apiStabilityExpand`.
    Expand = 1,
}

/// Go `apiStabilityFindingKey`.
/// apiStabilityFindingKey identifies one offender inside a surface. A symbol
/// finding and a signature finding are distinct, so a tagged overload of a
/// stable symbol is reported even though the symbol itself is stable. A
/// declaration finding names an offender that has no materialized symbol or
/// signature (a tagged member of a declaration annotation), so its declaration
/// is the identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApiStabilityFindingKey {
    pub symbol: SymbolId,
    pub signature: SignatureId,
    pub declaration: Node,
}

/// Go `apiStabilityFinding`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApiStabilityFinding {
    pub level: ApiStabilityLevel,
    pub declaration: Node,
}

/// Go `map[apiStabilityFindingKey]apiStabilityFinding`.
pub type ApiStabilityFindings = FxIndexMap<ApiStabilityFindingKey, ApiStabilityFinding>;

/// Go `apiStabilitySurface`.
/// apiStabilitySurface is a memoized set of stability findings. complete is
/// false when the result was cut by an in-progress cycle or by a component the
/// checker has not materialized; such a result is never treated as a finished
/// memo entry. cutCycle marks a result that was cut only by cycle back-edges,
/// which the settle fixpoint can establish as complete; blocked marks a result
/// whose components are unavailable or whose compiler outcome must not be
/// guessed, and which is never promoted.
///
/// observedEpoch is analysis-local settlement bookkeeping. It records the
/// analysis materialization epoch at which this result was last collected or
/// recollected, so the settle fixpoint can tell whether a blocked result may
/// have become collectable since (a later analysis read may have materialized a
/// component the guard had refused). It is never part of equality or merge
/// semantics: complete results ignore it, and blocked results are never
/// promoted.
///
/// PORT: a Go surface value shares its findings map with its copies. Go never
/// writes a map that another stored or returned surface still reads (fresh
/// maps in `newApiStabilitySurface`, `growApiStabilitySurface` and the shared
/// snapshot copies), so a clone of the map gives the same results.
#[derive(Clone, Debug, Default)]
pub struct ApiStabilitySurface {
    pub findings: ApiStabilityFindings,
    pub complete: bool,
    pub cut_cycle: bool,
    pub blocked: bool,

    pub observed_epoch: i32,
}

// Go: typeparser/api_stability.go newApiStabilitySurface
pub fn new_api_stability_surface() -> ApiStabilitySurface {
    ApiStabilitySurface {
        findings: ApiStabilityFindings::default(),
        complete: true,
        ..Default::default()
    }
}

// Go: typeparser/api_stability.go cycleCutSurface
/// cycleCutSurface marks a result cut by an in-progress cycle. The settle
/// fixpoint can promote it to complete once findings stop changing, because the
/// back-edge's own direct findings are collected when the cut target is first
/// entered.
pub fn cycle_cut_surface() -> ApiStabilitySurface {
    ApiStabilitySurface {
        findings: ApiStabilityFindings::default(),
        cut_cycle: true,
        ..Default::default()
    }
}

// Go: typeparser/api_stability.go blockedSurface
/// blockedSurface marks a result whose components are not materialized or whose
/// represented outcome is unavailable. It is never promoted to complete, so a
/// later query or session against a checked checker recomputes it.
pub fn blocked_surface() -> ApiStabilitySurface {
    ApiStabilitySurface {
        findings: ApiStabilityFindings::default(),
        blocked: true,
        ..Default::default()
    }
}

impl ApiStabilitySurface {
    // Go: typeparser/api_stability.go apiStabilitySurface.cutByCycle
    /// cutByCycle marks a surface that an in-progress cycle back-edge cut short. The
    /// settle fixpoint can establish it as complete once findings stop changing.
    pub fn cut_by_cycle(&mut self) {
        self.complete = false;
        self.cut_cycle = true;
    }

    // Go: typeparser/api_stability.go apiStabilitySurface.block
    /// block marks a surface whose remaining components are unavailable. A blocked
    /// surface is never promoted to complete, so a later query or session against a
    /// materialized checker recomputes it.
    pub fn block(&mut self) {
        self.complete = false;
        self.blocked = true;
    }

    // Go: typeparser/api_stability.go apiStabilitySurface.merge
    pub fn merge(&mut self, other: ApiStabilitySurface) {
        if !other.complete {
            self.complete = false;
            self.cut_cycle = self.cut_cycle || other.cut_cycle;
            self.blocked = self.blocked || other.blocked;
        }
        for (key, finding) in other.findings {
            match self.findings.get(&key) {
                Some(existing) if finding.level <= existing.level => {}
                _ => {
                    self.findings.insert(key, finding);
                }
            }
        }
    }

    // Go: typeparser/api_stability.go apiStabilitySurface.equals
    /// equals compares the semantic content of two surfaces. observedEpoch is
    /// deliberately excluded: it is settlement bookkeeping, so updating it after a
    /// recollection is not a fixpoint change.
    pub fn equals(&self, other: &ApiStabilitySurface) -> bool {
        // PORT: Go `maps.Equal`; `IndexMap` equality ignores the order.
        self.complete == other.complete
            && self.cut_cycle == other.cut_cycle
            && self.blocked == other.blocked
            && self.findings == other.findings
    }
}

/// Go `*apiStabilityCarrier`: an index into the analysis carrier table
/// (`ApiStabilityAnalysis::carrier_nodes`). Zero is Go nil.
///
/// PORT: Go interns carriers by pointer. Equal chains share one node, so the
/// interned index is the same identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApiStabilityCarrierId(pub u32);

impl ApiStabilityCarrierId {
    /// Go nil.
    pub const NIL: Self = Self(0);

    #[must_use]
    pub const fn is_nil(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn is_some(self) -> bool {
        self.0 != 0
    }
}

/// Go `apiStabilityCarrier`.
/// apiStabilityCarrier is the interned, concrete enclosing identity of an
/// internal raw-component traversal. It is built only from concrete compiler
/// objects: a reference or instantiated owner type, a concrete signature, and
/// the concrete argument types of a represented generic application. Interning
/// in the analysis-local carrier table makes equal chains share one node
/// pointer, so session and cycle keys compare by concrete identity instead of a
/// serialized substitution context. No context string and no raw-target-only key
/// ever participates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApiStabilityCarrier {
    pub parent: ApiStabilityCarrierId,
    pub owner: TypeId,
    pub signature: SignatureId,

    // argument marks a carrier link that contributes one concrete type
    // argument. marker keeps the link distinct even when the argument itself is
    // unrepresented (nil), so two different applications never share a carrier
    // by accident.
    pub argument: TypeId,
    pub marker: bool,
}

/// Go `apiStabilitySubstFrame`.
/// apiStabilitySubstFrame is one link in a composed substitution context. A
/// frame maps a signature's type parameters, a reference's type parameters, or
/// a compiler-provided mapper to represented types. The chain keeps nested
/// substitutions intact: a base type's argument can itself refer to the
/// enclosing type's parameters. carrier is the interned concrete identity of
/// the whole chain.
#[derive(Debug, Default)]
pub struct ApiStabilitySubstFrame {
    pub parent: Option<Rc<ApiStabilitySubstFrame>>,
    pub mapper: MapperId,
    pub signature: SignatureId,

    // parameters and arguments bind a class or interface reference's type
    // parameters to the reference's represented type arguments. A nil argument
    // replaces the parameter without a represented value: dependencies of the
    // replaced parameter are unknown, so the traversal blocks instead of
    // falling back to the declaration's constraint or default.
    pub parameters: Vec<TypeId>,
    pub arguments: Vec<TypeId>,

    pub carrier: ApiStabilityCarrierId,
}

/// Go `apiStabilitySubstitution` (a value holding a frame pointer).
#[derive(Clone, Debug, Default)]
pub struct ApiStabilitySubstitution {
    pub frame: Option<Rc<ApiStabilitySubstFrame>>,
}

impl ApiStabilitySubstitution {
    // Go: typeparser/api_stability.go apiStabilitySubstitution.carrier
    /// carrier returns the concrete enclosing identity of the whole substitution
    /// chain, or nil when the traversal started at a concrete top-level object.
    pub fn carrier(&self) -> ApiStabilityCarrierId {
        match &self.frame {
            None => ApiStabilityCarrierId::NIL,
            Some(frame) => frame.carrier,
        }
    }
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.aliasArgumentsAreSelf
    /// aliasArgumentsAreSelf reports whether an alias's recorded type arguments are
    /// exactly its own declared type parameters. The declared type of a generic
    /// alias is parameterized that way; such arguments are declaration operands and
    /// are never part of an application's exposed surface.
    // PORT: Go `alias == nil` is `None`.
    pub fn alias_arguments_are_self(
        &mut self,
        tp: &mut TypeParser<'_>,
        alias: Option<&Rc<TypeAlias>>,
    ) -> bool {
        let Some(alias) = alias else {
            return true;
        };
        let arguments = alias.type_arguments();
        if arguments.is_empty() {
            return true;
        }
        let symbol = alias.symbol();
        if symbol.is_nil() {
            return false;
        }
        let parameters = tp
            .checker
            .get_local_type_parameters_of_class_or_interface_or_type_alias_exported(symbol);
        if parameters.len() != arguments.len() {
            return false;
        }
        for index in 0..arguments.len() {
            if arguments[index] != parameters[index] {
                return false;
            }
        }
        true
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.internCarrier
    /// internCarrier interns one concrete enclosing identity link into the analysis.
    /// The table is discarded with the analysis, so no carrier survives an export.
    pub fn intern_carrier(
        &mut self,
        parent: ApiStabilityCarrierId,
        owner: TypeId,
        signature: SignatureId,
    ) -> ApiStabilityCarrierId {
        if owner.is_nil() && signature.is_nil() {
            return parent;
        }
        let key = ApiStabilityCarrier {
            parent,
            owner,
            signature,
            ..Default::default()
        };
        self.intern_carrier_key(key)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.internArgumentCarrier
    /// internArgumentCarrier interns one concrete type argument of a represented
    /// generic application. A nil argument still interns a distinct link because the
    /// marker keeps the position of an unrepresented argument part of the identity.
    pub fn intern_argument_carrier(
        &mut self,
        parent: ApiStabilityCarrierId,
        argument: TypeId,
    ) -> ApiStabilityCarrierId {
        let key = ApiStabilityCarrier {
            parent,
            argument,
            marker: true,
            ..Default::default()
        };
        self.intern_carrier_key(key)
    }

    /// PORT: the shared body of the two Go intern functions: Go
    /// `a.carriers[key]` or a new node stored under `key`.
    fn intern_carrier_key(&mut self, key: ApiStabilityCarrier) -> ApiStabilityCarrierId {
        if let Some(&existing) = self.carriers.get(&key) {
            return existing;
        }
        self.carrier_nodes.push(key);
        let carrier = ApiStabilityCarrierId(
            u32::try_from(self.carrier_nodes.len()).expect("carrier overflow"),
        );
        self.carriers.insert(key, carrier);
        carrier
    }

    /// Go `*carrier` for a non-nil interned carrier.
    pub fn carrier_node(&self, carrier: ApiStabilityCarrierId) -> &ApiStabilityCarrier {
        &self.carrier_nodes[carrier.0 as usize - 1]
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.extendParametersSubstitution
    /// extendParametersSubstitution pushes one frame binding a set of type
    /// parameters to represented arguments. The concrete owner is part of the
    /// interned carrier, so the same raw component analyzed under two different
    /// concrete applications never shares a cached result.
    pub fn extend_parameters_substitution(
        &mut self,
        subst: &ApiStabilitySubstitution,
        owner: TypeId,
        parameters: &[TypeId],
        arguments: &[TypeId],
    ) -> ApiStabilitySubstitution {
        let mut carrier = self.intern_carrier(subst.carrier(), owner, SignatureId::NIL);
        for &argument in arguments {
            carrier = self.intern_argument_carrier(carrier, argument);
        }
        ApiStabilitySubstitution {
            frame: Some(Rc::new(ApiStabilitySubstFrame {
                parent: subst.frame.clone(),
                parameters: parameters.to_vec(),
                arguments: arguments.to_vec(),
                carrier,
                ..Default::default()
            })),
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.extendMapperSubstitution
    /// extendMapperSubstitution pushes one frame for a compiler-provided mapper
    /// attached to an instantiated type (an inferred anonymous object, an
    /// instantiated mapped type, or an instantiated conditional).
    pub fn extend_mapper_substitution(
        &mut self,
        subst: &ApiStabilitySubstitution,
        owner: TypeId,
        mapper: MapperId,
    ) -> ApiStabilitySubstitution {
        let carrier = self.intern_carrier(subst.carrier(), owner, SignatureId::NIL);
        ApiStabilitySubstitution {
            frame: Some(Rc::new(ApiStabilitySubstFrame {
                parent: subst.frame.clone(),
                mapper,
                carrier,
                ..Default::default()
            })),
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.extendSignatureSubstitution
    /// extendSignatureSubstitution pushes one frame for a concrete signature. The
    /// raw signature's type parameters are resolved through the concrete
    /// signature's represented type arguments and its instantiated parameter
    /// mappers.
    pub fn extend_signature_substitution(
        &mut self,
        subst: &ApiStabilitySubstitution,
        signature: SignatureId,
    ) -> ApiStabilitySubstitution {
        let carrier = self.intern_carrier(subst.carrier(), TypeId::NIL, signature);
        ApiStabilitySubstitution {
            frame: Some(Rc::new(ApiStabilitySubstFrame {
                parent: subst.frame.clone(),
                signature,
                carrier,
                ..Default::default()
            })),
        }
    }
}

// Cache keys are concrete compiler identities. t and signature are the
// represented objects under inspection; carrier is the interned concrete
// enclosing identity when a raw (uninstantiated) component is walked inside a
// concrete context. No serialized substitution context is ever part of a key.

/// Go `apiStabilityTypeKey`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApiStabilityTypeKey {
    pub t: TypeId,
    pub inspect: ApiStabilityInspection,
    pub carrier: ApiStabilityCarrierId,
}

/// Go `apiStabilitySignatureKey`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApiStabilitySignatureKey {
    pub signature: SignatureId,
    pub carrier: ApiStabilityCarrierId,
}

/// Go `apiStabilitySymbolKey`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApiStabilitySymbolKey {
    pub symbol: SymbolId,
    pub carrier: ApiStabilityCarrierId,
}

/// Go `apiStabilityAliasKey`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApiStabilityAliasKey {
    pub t: TypeId,
    pub carrier: ApiStabilityCarrierId,
}

/// Go `apiStabilitySurfaceTypeKey`.
/// apiStabilitySurfaceTypeKey is the concrete, context-free identity of one
/// shared type-surface entry: the represented type pointer and the fixed
/// inspection policy. The two policies occupy separate entry slots, so an
/// expanded result can never contaminate a shallow one. No serialized context
/// string and no substitution carrier participates; a surface computed under a
/// substitution context stays analysis-local.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApiStabilitySurfaceTypeKey {
    pub t: TypeId,
    pub inspect: ApiStabilityInspection,
}

/// Go `apiStabilitySharedSurface`.
/// apiStabilitySharedSurface is one immutable snapshot of a complete, settled,
/// context-free surface published on the per-checker EffectLinks. findings
/// excludes every finding that matches the component's own declared tag (the
/// same exclusion the root result conversion applies); removed records the
/// concrete identities of those findings so a consumer can re-add them, with
/// their declared level, from the per-checker declared caches. The snapshot is
/// never mutated after publication: a consumer copies the findings into its own
/// analysis-local surface before composing.
#[derive(Clone, Debug, Default)]
pub struct ApiStabilitySharedSurface {
    pub findings: ApiStabilityFindings,
    pub removed: Vec<ApiStabilityFindingKey>,
}

/// Go `apiStabilityMaterializationReadKind`.
/// apiStabilityMaterializationReadKind identifies one lazy read operation whose
/// materialization epoch advance is deduplicated per concrete identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ApiStabilityMaterializationReadKind {
    /// Go `apiStabilityMaterializationReadDeclaredType`.
    #[default]
    DeclaredType = 0,
    /// Go `apiStabilityMaterializationReadSignatures`.
    Signatures = 1,
    /// Go `apiStabilityMaterializationReadMembers`.
    Members = 2,
    /// Go `apiStabilityMaterializationReadNodeType`.
    NodeType = 3,
}

/// Go `apiStabilityMaterializationReadKey`.
/// apiStabilityMaterializationReadKey is the concrete identity of one
/// deduplicated lazy read: the operation and the symbol or annotation node it
/// resolves. It only decides whether the materialization epoch advances once
/// more; it is never a cache identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ApiStabilityMaterializationReadKey {
    pub kind: ApiStabilityMaterializationReadKind,
    pub symbol: SymbolId,
    pub node: Node,
}

/// Go `node.Flags&ast.NodeFlagsPossiblyContainsStabilityTag != 0`.
///
/// PORT: Effect patch 031 (`_patches/typescript/031-stability-tag-fast-path`)
/// adds a scanner token flag (`@stability` in a JSDoc comment of the leading
/// trivia) that the parser copies to a node flag in `withJSDoc`. The port does
/// not change the parser: the API encoder writes raw node flags, so a new flag
/// would change plain API output, and a parser change makes the lib blobs
/// stale. This gives the same answer from the text: Go sets the flag on a node
/// with `HasJSDoc` whose first token's leading trivia (from `node.Pos()`) has a
/// JSDoc comment (`/**`, not `/**/`) that passes `scanJSDocCommentForTags` for
/// `stability` (an `@` followed by `hasJSDocTag(rest, "stability")`). The
/// trivia comments are the trailing and the leading comment ranges at
/// `node.Pos()`. The flag is only read together with the JSDoc tag parse of
/// the same node, so a scan hit without a parsed tag gives "" in both.
pub fn possibly_contains_stability_tag(node: Node) -> bool {
    if node.parser_flags(NodeFlags::HAS_JS_DOC).is_empty() {
        return false;
    }
    let file = get_source_file_of_node(node);
    if file.is_nil() {
        return false;
    }
    let text = source_file_text(file);
    let text: &str = &text;
    let f = NodeFactory::default();
    let mut ranges = crate::frontend::scanner::get_trailing_comment_ranges(&f, text, node.pos());
    ranges.extend(crate::frontend::scanner::get_leading_comment_ranges(
        &f,
        text,
        node.pos(),
    ));
    let bytes = text.as_bytes();
    ranges.iter().any(|comment| {
        let start = comment.pos() as usize;
        let end = comment.end() as usize;
        // Go scanner: a `/*` comment is JSDoc when the next char is `*` and
        // the one after it is not `/`.
        let is_js_doc = end - start >= 3
            && bytes[start + 1] == b'*'
            && bytes[start + 2] == b'*'
            && bytes.get(start + 3) != Some(&b'/');
        is_js_doc && js_doc_comment_has_stability_tag(&text[start..end])
    })
}

/// Go `scanJSDocCommentForTags` for the `stability` tag only (patch 031).
fn js_doc_comment_has_stability_tag(comment_text: &str) -> bool {
    let mut comment_text = comment_text;
    loop {
        let Some(i) = memchr::memchr(b'@', comment_text.as_bytes()) else {
            return false;
        };
        comment_text = &comment_text[i + 1..];
        if crate::frontend::scanner::scanner_p1::has_js_doc_tag(comment_text, &["stability"]) {
            return true;
        }
    }
}

/// PORT: Go `node.FunctionLikeData() != nil`. These are the node kinds whose
/// Go data embeds `FunctionLikeBase` (as `has_function_like_data_p4` in
/// `ast/utilities_p4.rs`, which is private).
pub fn api_stability_has_function_like_data(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::FunctionDeclaration
            | SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::Constructor
            | SyntaxKind::IndexSignature
            | SyntaxKind::MethodSignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::FunctionType
            | SyntaxKind::ConstructorType
            | SyntaxKind::ArrowFunction
            | SyntaxKind::FunctionExpression
            | SyntaxKind::JsDocSignature
    )
}

// Go: typeparser/api_stability.go stabilityOfDeclarationTag
/// stabilityOfDeclarationTag resolves the `@stability` tag of a declaration. A
/// variable's JSDoc is usually attached to its enclosing variable statement.
/// This is the single declaration-level parser shared by the API usage rules and
/// this analysis. An explicit `stable` tag is recognized and is distinct from an
/// absent tag: it overrides an inherited tag instead of being ignored.
pub fn stability_of_declaration_tag(declaration: Node) -> String {
    if declaration.is_nil() {
        return String::new();
    }
    let mut nodes = vec![declaration];
    let parent = declaration.parent();
    if parent.is_some()
        && parent.kind() == SyntaxKind::VariableDeclarationList
        && parent.parent().is_some()
        && parent.parent().kind() == SyntaxKind::VariableStatement
    {
        nodes.push(parent.parent());
    }
    for node in nodes {
        if !possibly_contains_stability_tag(node) {
            continue;
        }
        for doc in node.js_doc(Node::NIL) {
            let tags = doc.tags();
            if tags.is_nil() {
                continue;
            }
            for tag in tags.nodes() {
                if tag.kind() != SyntaxKind::JsDocUnknownTag || tag.tag_name().text() != "stability"
                {
                    continue;
                }
                let comment = tag.comment();
                if comment.is_some() && !comment.nodes().is_empty() {
                    let first = comment.nodes().get(0);
                    if first.kind() == SyntaxKind::JsDocText {
                        let value = first.text().trim();
                        if value == "stable" || value == "unstable" || value == "experimental" {
                            return value.to_string();
                        }
                    }
                }
            }
        }
    }
    String::new()
}

// Go: typeparser/api_stability.go StabilityTagOfDeclaration
/// StabilityTagOfDeclaration exposes the declaration-level `@stability` parser
/// so the existing usage diagnostics reuse one implementation.
pub fn stability_tag_of_declaration(declaration: Node) -> String {
    stability_of_declaration_tag(declaration)
}

// Go: typeparser/api_stability.go InternalTagOfDeclaration
/// InternalTagOfDeclaration reports whether a declaration carries the parsed
/// JSDoc `@internal` tag. Native TypeScript recognizes the same tag for
/// `stripInternal` declaration emit: a tagged declaration is omitted from the
/// emitted declarations, so it is not part of the public API. Only parsed tags
/// match, so prose mentioning the word is never treated as a tag. A variable's
/// JSDoc is usually attached to its enclosing variable statement, so that is
/// checked too, mirroring the `@stability` parser.
pub fn internal_tag_of_declaration(declaration: Node) -> bool {
    if declaration.is_nil() {
        return false;
    }
    let mut nodes = vec![declaration];
    let parent = declaration.parent();
    if parent.is_some()
        && parent.kind() == SyntaxKind::VariableDeclarationList
        && parent.parent().is_some()
        && parent.parent().kind() == SyntaxKind::VariableStatement
    {
        nodes.push(parent.parent());
    }
    for node in nodes {
        if node.parser_flags(NodeFlags::HAS_JS_DOC).is_empty() {
            continue;
        }
        for doc in node.js_doc(Node::NIL) {
            // PORT: Go `doc.AsJSDoc() == nil` cannot happen for a JSDoc node.
            let tags = doc.tags();
            if tags.is_nil() {
                continue;
            }
            for tag in tags.nodes() {
                if tag.kind() == SyntaxKind::JsDocUnknownTag && tag.tag_name().text() == "internal"
                {
                    return true;
                }
            }
        }
    }
    false
}

impl TypeParser<'_> {
    // Go: typeparser/api_stability.go TypeParser.DeclaredApiStability
    /// DeclaredApiStability returns the stability tier declared for a symbol. The
    /// result is cached per checker on EffectLinks so runs over different source
    /// files share the same lookup. Declared lookups never trigger any computed
    /// surface analysis.
    pub fn declared_api_stability(&mut self, symbol: SymbolId) -> ApiStabilityLevel {
        self.declared_api_stability_of_symbol(symbol).level
    }

    // Go: typeparser/api_stability.go TypeParser.DeclaredApiStabilityOfSymbol
    /// DeclaredApiStabilityOfSymbol returns the declared stability of a symbol,
    /// resolving import/export aliases and honouring a tag on the enclosing export
    /// declaration. Untagged symbols are stable. The result is cached per checker.
    pub fn declared_api_stability_of_symbol(
        &mut self,
        symbol: SymbolId,
    ) -> ApiStabilityDeclaration {
        if symbol.is_nil() {
            return ApiStabilityDeclaration::default();
        }
        cached!(self, api_stability_declared_symbol, symbol, {
            declared_stability_of_symbol_chain(self.checker, symbol)
        })
    }

    // Go: typeparser/api_stability.go TypeParser.DeclaredApiStabilityOfSignature
    /// DeclaredApiStabilityOfSignature returns the declared stability carried by a
    /// selected signature declaration. It is cached per checker on the raw
    /// (uninstantiated) signature so different overloads keep distinct tags; a
    /// symbol-level tag must never collapse them. An untagged signature is stable
    /// with no declaration, which lets callers fall back to the symbol.
    pub fn declared_api_stability_of_signature(
        &mut self,
        signature: SignatureId,
    ) -> ApiStabilityDeclaration {
        if signature.is_nil() {
            return ApiStabilityDeclaration::default();
        }
        let raw = raw_signature(self.checker, signature);
        if raw.is_nil() {
            return ApiStabilityDeclaration::default();
        }
        cached!(self, api_stability_declared_signature, raw, {
            let declaration = self.checker.sig(raw).declaration();
            self.declared_api_stability_of_declaration(declaration)
        })
    }

    // Go: typeparser/api_stability.go TypeParser.DeclaredApiStabilityOfDeclaration
    /// DeclaredApiStabilityOfDeclaration returns the stability tag attached to a
    /// declaration and the declaration that carries it. The result is cached per
    /// checker; this is the shared declaration-level lookup used by the symbol and
    /// signature accessors.
    pub fn declared_api_stability_of_declaration(
        &mut self,
        declaration: Node,
    ) -> ApiStabilityDeclaration {
        if declaration.is_nil() {
            return ApiStabilityDeclaration::default();
        }
        cached!(self, api_stability_declared_declaration, declaration, {
            let stability = stability_of_declaration_tag(declaration);
            if !stability.is_empty() {
                ApiStabilityDeclaration {
                    level: api_stability_rank(&stability),
                    declaration,
                }
            } else {
                ApiStabilityDeclaration::default()
            }
        })
    }
}

// Go: typeparser/api_stability.go apiStabilityRank
pub fn api_stability_rank(stability: &str) -> ApiStabilityLevel {
    match stability {
        "unstable" => ApiStabilityLevel::Unstable,
        "experimental" => ApiStabilityLevel::Experimental,
        _ => ApiStabilityLevel::Stable,
    }
}

// Go: typeparser/api_stability.go declaredStabilityOfSymbolChain
/// declaredStabilityOfSymbolChain mirrors the reference resolution used by the
/// stability usage rules: a declaration's own tag wins, then import/export
/// aliases are followed. Alias cycles are broken by symbol identity rather than
/// a fixed hop count, so long re-export chains keep their inherited stability.
pub fn declared_stability_of_symbol_chain(
    c: &mut Checker,
    symbol: SymbolId,
) -> ApiStabilityDeclaration {
    let mut symbol = symbol;
    let mut seen: FxHashSet<SymbolId> = FxHashSet::default();
    while symbol.is_some() {
        if seen.contains(&symbol) {
            break;
        }
        seen.insert(symbol);
        let declarations: Vec<Node> = c.sym(symbol).declarations.to_vec();
        for declaration in declarations {
            let stability = stability_of_declaration_tag(declaration);
            if !stability.is_empty() {
                return ApiStabilityDeclaration {
                    level: api_stability_rank(&stability),
                    declaration,
                };
            }
        }
        if !c.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
            break;
        }
        // The checker synthesizes a declaration-less `default` alias for `export =`
        // and JSON modules and panics when asked for its immediate target.
        if !has_alias_declaration(c, symbol) {
            break;
        }
        let next = api_stability_immediate_aliased_symbol(c, symbol);
        if next == symbol {
            break;
        }
        symbol = next;
    }
    ApiStabilityDeclaration::default()
}

// Go: typeparser/api_stability.go hasAliasDeclaration
pub fn has_alias_declaration(c: &Checker, symbol: SymbolId) -> bool {
    c.sym(symbol)
        .declarations
        .iter()
        .any(|&declaration| is_alias_symbol_declaration(declaration))
}

// Go: typeparser/api_stability.go ApiStabilityImmediateAliasedSymbol
/// ApiStabilityImmediateAliasedSymbol follows one valid import/export alias hop.
/// Immediate target lookup can repeat diagnostics for failed resolutions on
/// the legacy compiler. Skip those cached failures without resolving the alias,
/// then keep the immediate hop so forwarding declarations retain their tags.
/// Neither operation resolves or instantiates types.
pub fn api_stability_immediate_aliased_symbol(c: &mut Checker, symbol: SymbolId) -> SymbolId {
    if symbol.is_nil()
        || !c.sym(symbol).flags.intersects(SymbolFlags::ALIAS)
        || !has_alias_declaration(c, symbol)
    {
        return SymbolId::NIL;
    }
    if checker_integration::is_alias_resolution_failed(c, symbol) {
        return SymbolId::NIL;
    }
    c.get_immediate_aliased_symbol_exported(symbol)
}

/// Go `ApiStabilitySession`.
/// ApiStabilitySession is the analysis-local memo for the computed stability of
/// one export. The rule creates one session per export and queries the export's
/// symbol (and any namespace member) through it, so repeated components are
/// computed once within the export. After settling, the session publishes every
/// complete, context-free concrete type and signature surface to the
/// per-checker shared caches, so a later export, file or TypeParser over the
/// same checker composes that snapshot instead of recomputing; the session's
/// symbol and carrier memos, and every incomplete or blocked result, are
/// discarded with the session. Declared stability is read from the per-checker
/// declared caches and never needs a session.
///
/// PORT: the session does not hold the type parser (see the module doc); its
/// methods take it.
#[derive(Default)]
pub struct ApiStabilitySession {
    pub analysis: Option<Box<ApiStabilityAnalysis>>,
}

impl TypeParser<'_> {
    // Go: typeparser/api_stability.go TypeParser.NewApiStabilitySession
    /// NewApiStabilitySession starts a fresh computed-stability analysis. The
    /// session owns its symbol, signature, concrete type/component and substitution
    /// carrier memos; only the complete context-free surfaces published through
    /// settled snapshots outlive it.
    pub fn new_api_stability_session(&mut self) -> ApiStabilitySession {
        ApiStabilitySession {
            analysis: Some(Box::new(new_api_stability_analysis(self))),
        }
    }
}

impl ApiStabilitySession {
    // Go: typeparser/api_stability.go ApiStabilitySession.UsedBySymbol
    /// UsedBySymbol computes the stability the children of a symbol's public
    /// surface expose inside this session and settles cycle cuts: the root symbol's
    /// own declared tag is excluded from the returned dependencies and minimum. The
    /// own tag is exactly what DeclaredApiStabilityOfSymbol reports, read from the
    /// per-checker declared cache; a tag a function's declaration carries is
    /// recorded as a signature finding and is excluded together with the root
    /// symbol finding, while a tagged overload of another declaration stays an
    /// exposed child. Excluding the root tag is a result conversion only: the
    /// stored surface always keeps it, so the same symbol reached as a child of
    /// another root contributes its own tag to that parent. A complete result is
    /// memoized for later queries of the same session; a result cut by an
    /// unavailable component stays incomplete and is retried by a later query or
    /// session.
    pub fn used_by_symbol(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
    ) -> ApiStabilityUsed {
        let Some(analysis) = self.analysis.as_deref_mut() else {
            return ApiStabilityUsed::default();
        };
        if symbol.is_nil() {
            return ApiStabilityUsed::default();
        }
        analysis.symbol_surface(tp, symbol);
        analysis.settle(tp);
        let surface = analysis
            .session_symbols
            .get(&symbol)
            .cloned()
            .unwrap_or_default();
        let own = analysis.symbol_own_tag(tp, symbol);
        api_stability_used_of_own(tp.checker, &surface, own)
    }

    // Go: typeparser/api_stability.go ApiStabilitySession.UsedByType
    /// UsedByType computes the stability the children of an already represented
    /// checker type expose inside this session. The type's own declared tag is
    /// excluded from the returned dependencies and minimum: a type alias
    /// application's own tag is its alias symbol's, and a plain represented type's
    /// own tag is its own symbol's. An underlying named type an alias resolves to
    /// is an exposed child and keeps contributing. The stored surface is untouched,
    /// so the same type reached as a child of another root still contributes its
    /// own tag to that parent.
    pub fn used_by_type(&mut self, tp: &mut TypeParser<'_>, t: TypeId) -> ApiStabilityUsed {
        let Some(analysis) = self.analysis.as_deref_mut() else {
            return ApiStabilityUsed::default();
        };
        if t.is_nil() {
            return ApiStabilityUsed::default();
        }
        analysis.type_surface(
            tp,
            t,
            ApiStabilityInspection::Expand,
            &ApiStabilitySubstitution::default(),
        );
        analysis.settle(tp);
        let key = ApiStabilityTypeKey {
            t,
            inspect: ApiStabilityInspection::Expand,
            ..Default::default()
        };
        let surface = analysis
            .session_types
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let own = analysis.type_own_tag(tp, t);
        api_stability_used_of_own(tp.checker, &surface, own)
    }

    // Go: typeparser/api_stability.go ApiStabilitySession.UsedBySignature
    /// UsedBySignature computes the stability the children of an already
    /// represented signature expose inside this session. The signature's own
    /// declared tag is excluded from the returned dependencies and minimum; its
    /// parameters, type parameters and return keep contributing. The raw signature
    /// identity makes each overload's own tag distinct from every other overload's.
    pub fn used_by_signature(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
    ) -> ApiStabilityUsed {
        let Some(analysis) = self.analysis.as_deref_mut() else {
            return ApiStabilityUsed::default();
        };
        if signature.is_nil() {
            return ApiStabilityUsed::default();
        }
        analysis.signature_surface(tp, signature, &ApiStabilitySubstitution::default());
        analysis.settle(tp);
        let key = ApiStabilitySignatureKey {
            signature,
            ..Default::default()
        };
        let surface = analysis
            .session_signatures
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let own = analysis.signature_own_tag(tp, signature);
        api_stability_used_of_own(tp.checker, &surface, own)
    }
}

impl TypeParser<'_> {
    // Go: typeparser/api_stability.go TypeParser.ApiStabilityUsedBySymbol
    /// ApiStabilityUsedBySymbol computes the stability the children of a symbol's
    /// public surface expose in a fresh one-call session. The root symbol's own
    /// declared tag is excluded; use DeclaredApiStabilityOfSymbol to read it. The
    /// traversal walks compiler represented type graphs: an exported interface's own
    /// and inherited members, index signatures, call/construct signatures, generic
    /// constraints and defaults, type arguments of represented references, and the
    /// alias symbols written in public annotations that the checker erases. A named
    /// dependency reached inside the surface stays shallow (its own public call
    /// signatures are still inspected), so arbitrary referenced member graphs are
    /// never expanded. A child with an explicit `@stability` tag is a boundary: its
    /// declared level contributes to the parent surface and the child's internals
    /// are not expanded, while the same component keeps being audited when it is
    /// itself the queried root. A child without an explicit tag composes within the
    /// current shallow and anonymous graph boundaries: its public call and construct
    /// signatures, inherited surface, index signatures and represented members are
    /// inspected under the same rules. Each call starts from a fresh analysis, but a
    /// complete, settled, context-free concrete type or signature surface is
    /// published to the per-checker shared caches and reused by a later export, file
    /// or TypeParser over the same checker; symbol-root results and every
    /// incomplete, blocked or substitution-context result stay analysis-local.
    ///
    /// Public components the checker has not computed are materialized through its
    /// ordinary lazy accessors, preferring an already computed result. Before a
    /// potentially recursive read the non-evaluating pre-flight recursion guard
    /// inspects the represented carrier and the raw declarations with the compiler's
    /// own relations; a read whose expansion may evaluate a recursive alias, or
    /// whose safety the guard cannot establish, is refused and leaves the result
    /// incomplete (blocked), which is never reported as stable and is retried by a
    /// later session. An inferred component (a function return or a variable
    /// initializer) is read through the checker's own lazy inference, which
    /// attributes the diagnostics it produces to the declaring file's expressions
    /// exactly as that file's ordinary check would, so the rule never suppresses or
    /// consumes compiler diagnostics and the checked program keeps its own types and
    /// diagnostics exactly.
    pub fn api_stability_used_by_symbol(&mut self, symbol: SymbolId) -> ApiStabilityUsed {
        let mut session = self.new_api_stability_session();
        session.used_by_symbol(self, symbol)
    }

    // Go: typeparser/api_stability.go TypeParser.ApiStabilityUsedByType
    /// ApiStabilityUsedByType computes the stability the children of an already
    /// represented checker type expose in a fresh one-call session. The type's own
    /// declared tag is excluded; use the declared accessors to read it. It switches
    /// on TypeFlags/ObjectFlags and reads represented inner fields; a reference is
    /// inspected through its concrete public surface when expanded and through its
    /// declaration plus represented type arguments when shallow. Recursive alias
    /// applications are never expanded.
    pub fn api_stability_used_by_type(&mut self, t: TypeId) -> ApiStabilityUsed {
        let mut session = self.new_api_stability_session();
        session.used_by_type(self, t)
    }

    // Go: typeparser/api_stability.go TypeParser.ApiStabilityUsedBySignature
    /// ApiStabilityUsedBySignature computes the stability the children of an
    /// already represented signature expose in a fresh one-call session. The
    /// signature's own declared tag is excluded; use DeclaredApiStabilityOfSignature
    /// to read it. A concrete signature's instantiated parameter and return
    /// components are preferred; the uninstantiated target plus the represented
    /// substitution remains the fallback, so a directly exported inferred return
    /// such as `make<E>()` still exposes `E` without expanding a recursive alias.
    pub fn api_stability_used_by_signature(&mut self, signature: SignatureId) -> ApiStabilityUsed {
        let mut session = self.new_api_stability_session();
        session.used_by_signature(self, signature)
    }
}

/// Go `apiStabilityOwnTag`.
/// apiStabilityOwnTag identifies a root component's own declared tag inside a
/// stored surface: the root symbol or raw signature identity together with the
/// declaration that carries the root's declared stability. Stored surfaces
/// always keep a component's own tag so the same component reached as a child
/// contributes it to the parent surface; the result conversion for a root query
/// removes exactly the findings this structure matches. The declaration identity
/// also matches a tagged root signature: a function declaration's tag is
/// recorded as a signature finding, and it is the root's own tag exactly when
/// the declared accessor reports the same declaration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApiStabilityOwnTag {
    pub symbol: SymbolId,
    pub signature: SignatureId,
    pub declaration: Node,
}

impl ApiStabilityOwnTag {
    // Go: typeparser/api_stability.go apiStabilityOwnTag.excludes
    /// excludes reports whether one stored finding is the root's own declared tag.
    /// A nil root field never matches, so a child-only conversion (a component, a
    /// nil owner) excludes nothing.
    // PORT: Go `rawSignature` reads `Target()` without a checker; here it takes `c`.
    pub fn excludes(
        &self,
        c: &Checker,
        key: &ApiStabilityFindingKey,
        finding: &ApiStabilityFinding,
    ) -> bool {
        if self.symbol.is_some() && key.signature.is_nil() && key.symbol == self.symbol {
            return true;
        }
        if self.signature.is_some()
            && key.signature.is_some()
            && raw_signature(c, key.signature) == raw_signature(c, self.signature)
        {
            return true;
        }
        self.declaration.is_some() && finding.declaration == self.declaration
    }
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.symbolOwnTag
    /// symbolOwnTag builds the own-tag identity of a symbol root from the declared
    /// accessor. The declared lookup only reads the per-checker declared caches, so
    /// building the identity never computes a surface.
    pub fn symbol_own_tag(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
    ) -> ApiStabilityOwnTag {
        if symbol.is_nil() {
            return ApiStabilityOwnTag::default();
        }
        let declared = tp.declared_api_stability_of_symbol(symbol);
        ApiStabilityOwnTag {
            symbol,
            declaration: declared.declaration,
            ..Default::default()
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.typeOwnTag
    /// typeOwnTag builds the own-tag identity of a represented type root. A type
    /// alias application's own tag is the alias symbol's; an underlying named type
    /// the alias resolves to stays an exposed child. Any other represented type's
    /// own tag is its own symbol's.
    pub fn type_own_tag(&mut self, tp: &mut TypeParser<'_>, t: TypeId) -> ApiStabilityOwnTag {
        if t.is_nil() {
            return ApiStabilityOwnTag::default();
        }
        if let Some(alias) = tp.checker.ty(t).alias()
            && alias.symbol().is_some()
        {
            return self.symbol_own_tag(tp, alias.symbol());
        }
        let symbol = tp.checker.ty(t).symbol();
        if symbol.is_some() {
            return self.symbol_own_tag(tp, symbol);
        }
        ApiStabilityOwnTag::default()
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.signatureOwnTag
    /// signatureOwnTag builds the own-tag identity of a signature root. The raw
    /// (uninstantiated) signature keeps distinct overload tags distinct.
    pub fn signature_own_tag(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
    ) -> ApiStabilityOwnTag {
        let raw = raw_signature(tp.checker, signature);
        if raw.is_nil() {
            return ApiStabilityOwnTag::default();
        }
        let declared = tp.declared_api_stability_of_signature(raw);
        ApiStabilityOwnTag {
            signature: raw,
            declaration: declared.declaration,
            ..Default::default()
        }
    }
}

// Go: typeparser/api_stability.go apiStabilityUsedOf
/// apiStabilityUsedOf converts a stored symbol surface into the public result
/// for a symbol-rooted query, excluding the root symbol's own finding. It is the
/// settlement-test entry point; the session accessors build the richer own-tag
/// identity, which also excludes a root's tagged signature declaration.
pub fn api_stability_used_of(
    c: &Checker,
    surface: &ApiStabilitySurface,
    owner: SymbolId,
) -> ApiStabilityUsed {
    api_stability_used_of_own(
        c,
        surface,
        ApiStabilityOwnTag {
            symbol: owner,
            ..Default::default()
        },
    )
}

// Go: typeparser/api_stability.go apiStabilityUsedOfOwn
/// apiStabilityUsedOfOwn converts a stored surface into the public result,
/// skipping every finding that is the root's own declared tag. The stored
/// surface itself is never modified.
pub fn api_stability_used_of_own(
    c: &Checker,
    surface: &ApiStabilitySurface,
    own: ApiStabilityOwnTag,
) -> ApiStabilityUsed {
    let mut used = ApiStabilityUsed {
        minimum: ApiStabilityLevel::Stable,
        incomplete: !surface.complete,
        ..Default::default()
    };
    for (key, finding) in &surface.findings {
        if own.excludes(c, key, finding) {
            continue;
        }
        let mut declaration = finding.declaration;
        if declaration.is_nil() {
            declaration = key.declaration;
        }
        used.dependencies.push(ApiStabilityDependency {
            symbol: key.symbol,
            signature: key.signature,
            declaration,
            level: finding.level,
        });
        if finding.level > used.minimum {
            used.minimum = finding.level;
        }
    }
    sort_api_stability_dependencies(c, &mut used.dependencies);
    used
}

// Go: typeparser/api_stability.go sortApiStabilityDependencies
// PORT: Go `sort.Slice` is not stable; equal entries keep an order that
// depends on the random map order. The port sorts stably over the insertion
// order. Names compare by their Go bytes.
pub fn sort_api_stability_dependencies(c: &Checker, deps: &mut [ApiStabilityDependency]) {
    let names: FxHashMap<(SymbolId, SignatureId, Node), String> = deps
        .iter()
        .map(|dependency| {
            (
                (
                    dependency.symbol,
                    dependency.signature,
                    dependency.declaration,
                ),
                api_stability_dependency_sort_name(c, dependency),
            )
        })
        .collect();
    deps.sort_by(|left, right| {
        let left_name = &names[&(left.symbol, left.signature, left.declaration)];
        let right_name = &names[&(right.symbol, right.signature, right.declaration)];
        if left_name != right_name {
            return crate::scanner_util::compare_go_strings(left_name, right_name);
        }
        if left.level != right.level {
            // Go `left.Level > right.Level`.
            return right.level.cmp(&left.level);
        }
        // Go `left.Symbol != nil && right.Symbol == nil`.
        let left_first = left.symbol.is_some() && right.symbol.is_nil();
        let right_first = right.symbol.is_some() && left.symbol.is_nil();
        if left_first {
            std::cmp::Ordering::Less
        } else if right_first {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
}

// Go: typeparser/api_stability.go apiStabilityDependencySortName
pub fn api_stability_dependency_sort_name(
    c: &Checker,
    dependency: &ApiStabilityDependency,
) -> String {
    let name = api_stability_usable_symbol_name(c, dependency.symbol);
    if !name.is_empty() {
        return name;
    }
    if let Some(name) = api_stability_declaration_display_name(c, dependency.declaration) {
        return name;
    }
    let name = api_stability_late_bound_symbol_display_name(c, dependency.symbol);
    if !name.is_empty() {
        return name;
    }
    if dependency.symbol.is_some() && !c.sym(dependency.symbol).name.as_str().is_empty() {
        return c.sym(dependency.symbol).name.to_string();
    }
    if dependency.signature.is_some() {
        return "signature".to_string();
    }
    "<anonymous>".to_string()
}

// Go: typeparser/api_stability.go ApiStabilityDependencyName
/// ApiStabilityDependencyName returns the display name of an exposed offender
/// for diagnostics: the symbol name, the declaration's safely read name, or a
/// descriptive fallback for anonymous signatures. A computed declaration name is
/// never evaluated; its display spelling comes from its declaration symbol or
/// its well-known symbol expression.
pub fn api_stability_dependency_name(c: &Checker, dependency: &ApiStabilityDependency) -> String {
    let name = api_stability_usable_symbol_name(c, dependency.symbol);
    if !name.is_empty() {
        return name;
    }
    if let Some(name) = api_stability_declaration_display_name(c, dependency.declaration) {
        return name;
    }
    let name = api_stability_late_bound_symbol_display_name(c, dependency.symbol);
    if !name.is_empty() {
        return name;
    }
    if dependency.symbol.is_some() && !c.sym(dependency.symbol).name.as_str().is_empty() {
        return c.sym(dependency.symbol).name.to_string();
    }
    if dependency.declaration.is_some() {
        match dependency.declaration.kind() {
            SyntaxKind::CallSignature => return "call signature".to_string(),
            SyntaxKind::ConstructSignature => return "constructor".to_string(),
            SyntaxKind::IndexSignature => return "index signature".to_string(),
            _ => {}
        }
        let name = get_name_of_declaration(dependency.declaration);
        if name.is_some() && name.kind() == SyntaxKind::ComputedPropertyName {
            return "[computed]".to_string();
        }
    }
    if dependency.signature.is_some() {
        if c.sig(dependency.signature)
            .flags()
            .intersects(SignatureFlags::CONSTRUCT)
        {
            return "constructor".to_string();
        }
        return "call signature".to_string();
    }
    "<anonymous>".to_string()
}

// Go: typeparser/api_stability.go apiStabilityUsableSymbolName
/// apiStabilityUsableSymbolName returns a symbol name suitable for display,
/// excluding the binder's `__computed` placeholder and the checker's `__@`
/// late-bound member spellings. A computed declaration renders its own readable
/// spelling instead of exposing those internal names.
pub fn api_stability_usable_symbol_name(c: &Checker, symbol: SymbolId) -> String {
    if symbol.is_nil() {
        return String::new();
    }
    let name = c.sym(symbol).name.as_str();
    if name.is_empty() || name.starts_with(INTERNAL_SYMBOL_NAME_PREFIX) || name.starts_with("__@") {
        return String::new();
    }
    name.to_string()
}

// Go: typeparser/api_stability.go apiStabilityLateBoundSymbolDisplayName
/// apiStabilityLateBoundSymbolDisplayName renders the bound property name of a
/// checker late-bound member (`__@name@id`) for diagnostics and sorting. The
/// checker records that name when it late-binds a computed declaration, so
/// reading it never evaluates the source expression and never leaks the internal
/// spelling. It returns "" for anything that is not a late-bound symbol name.
pub fn api_stability_late_bound_symbol_display_name(c: &Checker, symbol: SymbolId) -> String {
    if symbol.is_nil() || !c.is_known_symbol(symbol) {
        return String::new();
    }
    let full = c.sym(symbol).name.as_str();
    let prefix = format!("{INTERNAL_SYMBOL_NAME_PREFIX}@");
    let mut name = full.strip_prefix(prefix.as_str()).unwrap_or(full);
    if let Some(index) = name.rfind('@') {
        name = &name[..index];
    }
    if name.is_empty() {
        return String::new();
    }
    format!("[{name}]")
}

// Go: typeparser/api_stability.go apiStabilityDeclarationDisplayName
/// apiStabilityDeclarationDisplayName returns the name of a declaration for
/// sorting and display without evaluating the name. A simply named declaration
/// keeps its source spelling. A computed name is never asked for its text: a
/// well-known `Symbol.<name>` expression is rendered from its own identifiers, a
/// symbol the binder already attached to the declaration names it, and any
/// other computed name reports no name so the caller can fall back
/// deterministically. The boolean reports whether a name was available.
// PORT: Go `(string, bool)` is `Option<String>`.
pub fn api_stability_declaration_display_name(c: &Checker, declaration: Node) -> Option<String> {
    if declaration.is_nil() {
        return None;
    }
    let name = get_name_of_declaration(declaration);
    if name.is_nil() {
        return None;
    }
    if let Some(simple) = api_stability_simple_name_text(name) {
        return Some(simple);
    }
    if name.kind() == SyntaxKind::ComputedPropertyName {
        let well_known = api_stability_well_known_symbol_display_name(name.expression());
        if !well_known.is_empty() {
            return Some(well_known);
        }
        // A literal computed name denotes exactly the literal it is written
        // with; reading that literal is not an evaluation. An identifier
        // computed name denotes the identifier's value, not its spelling, so
        // it is never rendered from source text.
        let expression = name.expression();
        if expression.is_some() {
            match expression.kind() {
                SyntaxKind::StringLiteral
                | SyntaxKind::NumericLiteral
                | SyntaxKind::BigIntLiteral
                | SyntaxKind::NoSubstitutionTemplateLiteral => {
                    return Some(expression.text().to_string());
                }
                _ => {}
            }
        }
        let declaration_symbol = declaration.symbol();
        if declaration_symbol.is_some() {
            let symbol_name = c.sym(declaration_symbol).name.as_str();
            if !symbol_name.is_empty() && !symbol_name.starts_with(INTERNAL_SYMBOL_NAME_PREFIX) {
                return Some(symbol_name.to_string());
            }
        }
    }
    None
}

// Go: typeparser/api_stability.go apiStabilitySimpleNameText
/// apiStabilitySimpleNameText returns the source text of a declaration name that
/// has one without evaluation. A computed or otherwise dynamic name reports
/// false; its text is never read.
// PORT: Go `(string, bool)` is `Option<String>`.
pub fn api_stability_simple_name_text(name: Node) -> Option<String> {
    if name.is_nil() {
        return None;
    }
    match name.kind() {
        SyntaxKind::Identifier
        | SyntaxKind::StringLiteral
        | SyntaxKind::NumericLiteral
        | SyntaxKind::BigIntLiteral
        | SyntaxKind::PrivateIdentifier => Some(name.text().to_string()),
        _ => None,
    }
}

/// Go `apiStabilityAnalysis`.
/// apiStabilityAnalysis carries the cycle guards and memo for one computation.
/// Complete symbol, signature, type and component results are reused inside the
/// analysis, results contaminated by a cycle are settled with a bounded
/// fixpoint, and a result whose components are unavailable stays incomplete and
/// is never treated as stable. After the fixpoint has converged, every complete,
/// settled, context-free concrete type and signature result is also published
/// to the per-checker shared caches as a root-excluding snapshot; a result that
/// is still in progress, cycle-cut before promotion, blocked or otherwise
/// incomplete is never published, so a partial result can never leak into
/// another export. Symbol surfaces stay analysis-local: a symbol root composes
/// its own declared identity with the reusable concrete type and signature
/// surfaces.
///
/// PORT: Go `tp *TypeParser` is not a field (see the module doc). Go nil maps
/// (`safetySafeMemo`, `typeNameSymbols`, `optionalTaggedSymbols`,
/// `materializationReads`) are empty maps: Go reads a nil map as empty and
/// makes it on the first write.
#[derive(Default)]
pub struct ApiStabilityAnalysis {
    pub session_symbols: FxIndexMap<SymbolId, ApiStabilitySurface>,
    pub session_types: FxIndexMap<ApiStabilityTypeKey, ApiStabilitySurface>,
    pub session_components: FxIndexMap<ApiStabilityTypeKey, ApiStabilitySurface>,
    pub session_signatures: FxIndexMap<ApiStabilitySignatureKey, ApiStabilitySurface>,
    pub type_frames: FxHashMap<ApiStabilityTypeKey, Option<Rc<ApiStabilitySubstFrame>>>,
    pub signature_frames: FxHashMap<ApiStabilitySignatureKey, Option<Rc<ApiStabilitySubstFrame>>>,

    // carriers interns the concrete enclosing identity of raw-component
    // traversals for this analysis only. Equal chains share one node pointer so
    // cache and cycle keys compare by concrete identity instead of a serialized
    // substitution context; the table is discarded with the analysis.
    pub carriers: FxHashMap<ApiStabilityCarrier, ApiStabilityCarrierId>,
    /// PORT: the interned carrier nodes (Go heap nodes); `ApiStabilityCarrierId(i)`
    /// is `carrier_nodes[i - 1]`.
    pub carrier_nodes: Vec<ApiStabilityCarrier>,

    // settleSymbols, settleTypes, settleComponents and settleSignatures record
    // the first-insertion order of the session results. Settlement recollects
    // incomplete results in that order instead of Go's randomized map order, so
    // the bounded fixpoint visits exactly the same results in the same sequence
    // over the same checker state and the reported findings are reproducible.
    // The order is analysis-local bookkeeping; it is never part of any cache key.
    pub settle_symbols: Vec<SymbolId>,
    pub settle_types: Vec<ApiStabilityTypeKey>,
    pub settle_components: Vec<ApiStabilityTypeKey>,
    pub settle_signatures: Vec<ApiStabilitySignatureKey>,

    pub active_symbols: FxHashMap<ApiStabilitySymbolKey, bool>,
    pub active_types: FxHashMap<ApiStabilityTypeKey, bool>,
    pub active_components: FxHashMap<ApiStabilityTypeKey, bool>,
    pub active_signatures: FxHashMap<ApiStabilitySignatureKey, bool>,
    pub active_aliases: FxHashMap<ApiStabilityAliasKey, bool>,
    pub active_conditionals: FxHashMap<ApiStabilityAliasKey, bool>,

    pub signature_args: FxHashMap<SignatureId, Vec<TypeId>>,
    pub signature_parameter_cache: FxHashMap<SignatureId, FxHashMap<TypeId, TypeId>>,

    // safetySafeMemo records completed Safe guard verdicts for this analysis,
    // keyed by the concrete read identity (operation, represented identity and
    // interned substitution carrier). Only Safe is ever recorded: Unknown,
    // recursion and exhaustion stay retryable by later scans and analyses.
    pub safety_safe_memo: FxHashSet<ApiStabilitySafetyVerdictKey>,

    // typeNameSymbols caches successful symbol lookups for raw type-name,
    // heritage, qualifier and declaration-name nodes by AST identity. A symbol
    // lookup at a node is a pure declaration/scope lookup that does not depend
    // on type bindings; no resolved type is ever cached here.
    pub type_name_symbols: FxHashMap<Node, SymbolId>,

    // Optional tagged base eligibility depends only on declaration metadata.
    pub optional_tagged_symbols: FxHashMap<SymbolId, bool>,

    pub work: i32,
    pub safety_work: i32,

    // materializationReads records the lazy reads whose checker accessor already
    // advanced the materialization epoch in this analysis, keyed by the read
    // operation and its concrete symbol or annotation node. Re-reading the same
    // component cannot materialize anything new (the checker caches it, or the
    // call is a declaration-only lookup), so the epoch only advances once per
    // identity. Without this, a repeated refused or cached lookup would keep
    // every blocked result stale and settlement would recollect surfaces that
    // provably cannot advance.
    pub materialization_reads: FxHashSet<ApiStabilityMaterializationReadKey>,

    // materializationEpoch advances whenever this analysis performs a lazy
    // checker read that can materialize represented components (a guarded read
    // that the pre-flight guard authorized, an inferred read the checker's own
    // check lifecycle allows, or a nested read a guard verification performed)
    // and whenever a settle round changes any surface. A blocked surface that
    // was last collected before the latest advancement may have become
    // collectable or may have a newly changed represented dependency; a surface
    // collected after it provably observed the current state.
    pub materialization_epoch: i32,
}

/// apiStabilityMaxWork bounds one analysis. The guard exists only so a
/// pathological represented graph cannot run away; ordinary surfaces with
/// deeply nested explicit components stay far below it. Exhausting the bound
/// marks the result incomplete, which keeps it out of every cache.
pub const API_STABILITY_MAX_WORK: i32 = 100_000;
/// apiStabilityMaxSettleRounds bounds the cycle fixpoint. Each round can only
/// add findings, so convergence is monotone; the bound is a safety net.
pub const API_STABILITY_MAX_SETTLE_ROUNDS: i32 = 64;
/// apiStabilityMaxSettleSweeps bounds the extra rounds an exhausted analysis
/// runs after the fixpoint converges. A recollection in the final round may
/// itself have advanced the epoch, making earlier results stale; the extra
/// rounds recollect them and stop as soon as a round changes nothing. A
/// result they do not recover stays blocked, and the analysis discards it
/// with its session, so a later export or call retries it.
pub const API_STABILITY_MAX_SETTLE_SWEEPS: i32 = 4;

// Go: typeparser/api_stability.go newApiStabilityAnalysis
// PORT: Go stores `tp`; the port does not (see the module doc), so the
// parameter is only Go's signature.
pub fn new_api_stability_analysis(_tp: &mut TypeParser<'_>) -> ApiStabilityAnalysis {
    ApiStabilityAnalysis::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};

    fn parse_root(file_name: &str, source: &'static str, kind: ScriptKind) -> Node {
        let opts = SourceFileParseOptions {
            file_name: file_name.to_string(),
            ..Default::default()
        };
        parse_source_file(&opts, source, kind).root
    }

    fn first_declaration(source: &'static str) -> Node {
        let root = parse_root("/test.ts", source, ScriptKind::TS);
        let statement = root.statements().get(0);
        statement.declaration_list().declarations().nodes().get(0)
    }

    /// Go: rules/stability_api_usage_test.go TestStabilityOfDeclaration.
    #[test]
    fn stability_of_declaration() {
        let tests: &[(&str, &'static str, &str)] = &[
            (
                "unstable variable",
                "/** @stability unstable */\nexport const api = 1",
                "unstable",
            ),
            (
                "experimental variable",
                "/** @stability experimental */\nexport const api = 1",
                "experimental",
            ),
            (
                "stable variable",
                "/** @stability stable */\nexport const api = 1",
                "stable",
            ),
            ("other tag", "/** @deprecated */\nexport const api = 1", ""),
            ("plain variable", "export const api = 1", ""),
        ];
        for (name, source, want) in tests {
            let declaration = first_declaration(source);
            assert_eq!(
                stability_tag_of_declaration(declaration),
                *want,
                "StabilityTagOfDeclaration ({name})"
            );
        }
    }

    /// Edge cases of the patch 031 text scan (`possibly_contains_stability_tag`)
    /// together with the JSDoc tag parse.
    #[test]
    fn stability_of_declaration_scan_edges() {
        let tests: &[(&str, &'static str, &str)] = &[
            (
                "no space after the comment start",
                "/**@stability unstable*/\nexport const api = 1",
                "unstable",
            ),
            (
                "tab after the tag",
                "/** @stability\tunstable */\nexport const api = 1",
                "unstable",
            ),
            (
                "colon after the tag",
                "/** @stability:unstable */\nexport const api = 1",
                "",
            ),
            (
                "not a JSDoc comment",
                "/* @stability unstable */\nexport const api = 1",
                "",
            ),
            (
                "tag on the declaration",
                "export const /** @stability experimental */ api = 1",
                "experimental",
            ),
            (
                "two comments in the trivia",
                "/** @stability unstable */\n/** other */\nexport const api = 1",
                "unstable",
            ),
            (
                "two comments, tag in the last",
                "/** other */\n/** @stability unstable */\nexport const api = 1",
                "unstable",
            ),
            // The JSDoc parser takes these as whitespace and parses the tag,
            // but the scan's `hasJSDocTag` does not, so the tag is not read
            // (effect-tsgo 0.51.1 gives no usage warning).
            (
                "no-break space after the tag",
                "/** @stability\u{a0}unstable */\nexport const api = 1",
                "",
            ),
            (
                "vertical tab after the tag",
                "/** @stability\u{b}unstable */\nexport const api = 1",
                "",
            ),
            (
                "form feed after the tag",
                "/** @stability\u{c}unstable */\nexport const api = 1",
                "",
            ),
            (
                "line separator after the tag",
                "/** @stability\u{2028}unstable */\nexport const api = 1",
                "",
            ),
            (
                "line feed after the tag",
                "/** @stability\n * unstable */\nexport const api = 1",
                "unstable",
            ),
            (
                "CRLF after the tag",
                "/** @stability\r\n * unstable */\nexport const api = 1",
                "unstable",
            ),
        ];
        for (name, source, want) in tests {
            let declaration = first_declaration(source);
            assert_eq!(
                stability_tag_of_declaration(declaration),
                *want,
                "StabilityTagOfDeclaration ({name})"
            );
        }
    }

    /// A JSDoc comment on the same line as the `{` of a type literal is a
    /// trailing comment of `{`: the scanner sees it, but it is not the JSDoc
    /// of the property signature.
    #[test]
    fn stability_of_same_line_property_signature() {
        let root = parse_root(
            "/test.ts",
            "export interface I { /** @stability unstable */ a: string\n /** @stability experimental */\n b: string }",
            ScriptKind::TS,
        );
        let members = root.statements().get(0).members();
        assert_eq!(stability_tag_of_declaration(members.get(0)), "");
        assert_eq!(stability_tag_of_declaration(members.get(1)), "experimental");
    }

    /// A JS `@typedef` or `@callback` is reparsed into a type alias that
    /// shares the comment's JSDoc, but the scan starts at the alias, inside
    /// the comment, so its `@stability` tag is not read. `ident` reads every
    /// JSDoc comment before it in order, so the first tag (`unstable`) wins.
    /// The answers are effect-tsgo 0.51.1's usage warnings for this file.
    #[test]
    fn stability_of_js_typedef() {
        let root = parse_root(
            "/index.js",
            "/**\n * @typedef {{ a: number }} Foo\n * @stability unstable\n */\n\
             /**\n * @stability unstable\n * @callback Cb\n * @param {number} x\n * @returns {void}\n */\n\
             /**\n * @stability experimental\n * @template T\n * @param {T} x\n * @returns {T}\n */\n\
             export function ident(x) { return x }\n\
             /** @stability unstable */\nexport class K {\n  /** @stability experimental */\n  m() {}\n}\n",
            ScriptKind::JS,
        );
        let mut got = Vec::new();
        for statement in root.statements() {
            got.push((
                statement.name().text().to_string(),
                stability_tag_of_declaration(statement),
            ));
            if statement.kind() == SyntaxKind::ClassDeclaration {
                let member = statement.members().get(0);
                got.push((
                    member.name().text().to_string(),
                    stability_tag_of_declaration(member),
                ));
            }
        }
        let want = [
            ("Foo", ""),
            ("Cb", ""),
            ("ident", "unstable"),
            ("K", "unstable"),
            ("m", "experimental"),
        ];
        let want: Vec<(String, String)> = want
            .iter()
            .map(|(name, tag)| (name.to_string(), tag.to_string()))
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn internal_tag() {
        assert!(internal_tag_of_declaration(first_declaration(
            "/** @internal */\nexport const api = 1"
        )));
        assert!(!internal_tag_of_declaration(first_declaration(
            "/** internal */\nexport const api = 1"
        )));
    }
}
