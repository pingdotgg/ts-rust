//! Port of Effect-TS/tsgo `internal/typeparser/api_stability.go` lines
//! 1154-2682 at `@effect/tsgo@0.51.1` (`47cb1ed7`): the work bound, the settle
//! fixpoint, the session stores, the per-checker shared surfaces, the visit
//! keys, the materialization epoch, the symbol surface and its safe lazy reads,
//! the type surface, conditional and object surfaces, and the concrete
//! reference surface. See `api_stability_p1.rs` for the split and the
//! signature conventions.

use crate::effect::typeparser::*;
use crate::prelude::*;

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.consumeWork
    pub fn consume_work(&mut self) -> bool {
        self.work += 1;
        self.work <= API_STABILITY_MAX_WORK
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.settle
    /// settle resolves cycle cuts with a bounded monotone fixpoint, then marks
    /// every result that was established complete as settled within this analysis.
    /// Findings are recomputed until they stop changing; a recomputed surface is
    /// unioned into the stored one so a dependency that is itself still settling can
    /// never erase a finding an earlier round established. Results whose only
    /// incompleteness came from cycle back-edges are then established complete,
    /// because a back-edge's own direct findings are collected when the cut target
    /// is first entered. Results that remain blocked (unavailable components or an
    /// unevaluable conditional outcome) are never promoted, so a partial result is
    /// never reported as stable; a later export or call retries from a new session.
    ///
    /// After the fixpoint converges, an exhausted analysis runs bounded extra
    /// rounds. A recollection in the final round may itself have materialized a
    /// component or advanced the epoch, making results collected earlier stale
    /// again; the extra rounds recollect exactly those stale results. A blocked
    /// result can only gain findings this way; it is never promoted. A result the
    /// extra rounds do not recover stays blocked, and the analysis discards it with
    /// its session, so a later export or call retries it.
    ///
    /// Once settling is done, every complete, context-free type and signature
    /// result is published to the per-checker shared caches. A result that is still
    /// in progress, blocked or cycle-cut before promotion is never published; a
    /// promoted cycle result is published only here, after the fixpoint established
    /// that its findings no longer change.
    pub fn settle(&mut self, tp: &mut TypeParser<'_>) {
        if self.has_incomplete() {
            let mut converged = false;
            for _ in 0..API_STABILITY_MAX_SETTLE_ROUNDS {
                if !self.settle_round(tp) {
                    converged = true;
                    break;
                }
                // A change to any surface can propagate into a result that was
                // collected earlier; advancing the epoch makes exactly those
                // earlier results stale, while a result collected after the change
                // has already observed it.
                self.materialization_epoch += 1;
            }
            if converged {
                if self.safety_budget_exhausted() {
                    for _ in 0..API_STABILITY_MAX_SETTLE_SWEEPS {
                        if !self.settle_round(tp) {
                            break;
                        }
                        // A recovery round can change a surface exactly like a
                        // fixpoint round: earlier blocked parents may now be
                        // stale, so they must be revisited within the bounded
                        // sweeps. This is the same propagation the fixpoint loop
                        // performs; it authorizes no additional guard read.
                        self.materialization_epoch += 1;
                    }
                }
                self.promote_settled();
            }
        }
        self.publish_complete_surfaces(tp);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.storeSymbolSurface
    /// storeSymbolSurface records a symbol result and its first-insertion order.
    pub fn store_symbol_surface(&mut self, symbol: SymbolId, surface: ApiStabilitySurface) {
        if !self.session_symbols.contains_key(&symbol) {
            self.settle_symbols.push(symbol);
        }
        self.session_symbols.insert(symbol, surface);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.storeTypeSurface
    /// storeTypeSurface records a type result and its first-insertion order.
    pub fn store_type_surface(&mut self, key: ApiStabilityTypeKey, surface: ApiStabilitySurface) {
        if !self.session_types.contains_key(&key) {
            self.settle_types.push(key);
        }
        self.session_types.insert(key, surface);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.storeComponentSurface
    /// storeComponentSurface records an object-component result and its
    /// first-insertion order.
    pub fn store_component_surface(
        &mut self,
        key: ApiStabilityTypeKey,
        surface: ApiStabilitySurface,
    ) {
        if !self.session_components.contains_key(&key) {
            self.settle_components.push(key);
        }
        self.session_components.insert(key, surface);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.storeSignatureSurface
    /// storeSignatureSurface records a signature result and its first-insertion
    /// order.
    pub fn store_signature_surface(
        &mut self,
        key: ApiStabilitySignatureKey,
        surface: ApiStabilitySurface,
    ) {
        if !self.session_signatures.contains_key(&key) {
            self.settle_signatures.push(key);
        }
        self.session_signatures.insert(key, surface);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.restoreOwnTagFinding
    /// restoreOwnTagFinding re-adds one finding a published snapshot removed as the
    /// component's own declared tag. The declared level and provenance come from
    /// the per-checker declared caches — the snapshot stores only the concrete
    /// identity — so composition is always "declared separately". The removed key
    /// was produced by exactly one of the three record helpers, which re-derives
    /// the same level and declaration from the same caches.
    pub fn restore_own_tag_finding(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        key: ApiStabilityFindingKey,
    ) {
        if key.signature.is_some() {
            self.record_signature_stability(tp, surface, key.signature);
        } else if key.symbol.is_some() {
            self.record_symbol(tp, surface, key.symbol);
        } else if key.declaration.is_some() {
            self.record_declaration_stability(tp, surface, key.declaration);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.sharedTypeSurface
    /// sharedTypeSurface returns a fresh analysis-local copy of the published
    /// complete surface of a concrete context-free type, re-adding the component's
    /// own declared-tag findings from the declared caches. It reports false when no
    /// entry exists, so the caller computes locally. The copy keeps the published
    /// snapshot immutable while the caller composes and mutates its own surface.
    // PORT: Go `(apiStabilitySurface, bool)` is `Option`. Go `a.tp == nil ||
    // a.tp.links == nil` cannot happen: `TypeParser::new` makes the links.
    pub fn shared_type_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        inspect: ApiStabilityInspection,
    ) -> Option<ApiStabilitySurface> {
        if t.is_nil() {
            return None;
        }
        let (findings, removed) = {
            let entry = tp
                .links()
                .api_stability_surface_type
                .get(&ApiStabilitySurfaceTypeKey { t, inspect })?;
            (entry.findings.clone(), entry.removed.clone())
        };
        let mut surface = new_api_stability_surface();
        surface.findings.extend(findings);
        for key in removed {
            self.restore_own_tag_finding(tp, &mut surface, key);
        }
        Some(surface)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.sharedSignatureSurface
    /// sharedSignatureSurface is sharedTypeSurface for a concrete signature
    /// identity.
    pub fn shared_signature_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
    ) -> Option<ApiStabilitySurface> {
        if signature.is_nil() {
            return None;
        }
        let (findings, removed) = {
            let entry = tp.links().api_stability_surface_signature.get(&signature)?;
            (entry.findings.clone(), entry.removed.clone())
        };
        let mut surface = new_api_stability_surface();
        surface.findings.extend(findings);
        for key in removed {
            self.restore_own_tag_finding(tp, &mut surface, key);
        }
        Some(surface)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.publishTypeSurface
    /// publishTypeSurface publishes one complete, settled, context-free type surface
    /// as an immutable root-excluding snapshot. Every finding that matches the
    /// component's own declared tag is removed and its concrete identity recorded;
    /// the declared level stays in the per-checker declared caches. An in-progress
    /// result is never stored; a cycle-cut result only reaches complete after the
    /// settle fixpoint promoted it; a blocked or otherwise incomplete result is
    /// never published. The first entry for a key wins: complete recomputations over
    /// the same checker state agree, so overwriting could not add information and
    /// the snapshot stays deterministic.
    pub fn publish_type_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        inspect: ApiStabilityInspection,
        surface: &ApiStabilitySurface,
    ) {
        if t.is_nil() || !surface.complete || surface.cut_cycle || surface.blocked {
            return;
        }
        let key = ApiStabilitySurfaceTypeKey { t, inspect };
        if tp.links().api_stability_surface_type.contains_key(&key) {
            return;
        }
        let own = self.type_own_tag(tp, t);
        let mut entry = ApiStabilitySharedSurface {
            findings: ApiStabilityFindings::with_capacity_and_hasher(
                surface.findings.len(),
                Default::default(),
            ),
            removed: Vec::new(),
        };
        for (finding_key, finding) in &surface.findings {
            if own.excludes(tp.checker, finding_key, finding) {
                entry.removed.push(*finding_key);
                continue;
            }
            entry.findings.insert(*finding_key, *finding);
        }
        tp.links().api_stability_surface_type.insert(key, entry);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.publishSignatureSurface
    /// publishSignatureSurface publishes one complete, settled, context-free
    /// signature surface with the signature's own declared tag removed exactly like
    /// publishTypeSurface does for a type.
    pub fn publish_signature_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
        surface: &ApiStabilitySurface,
    ) {
        if signature.is_nil() || !surface.complete || surface.cut_cycle || surface.blocked {
            return;
        }
        if tp
            .links()
            .api_stability_surface_signature
            .contains_key(&signature)
        {
            return;
        }
        let own = self.signature_own_tag(tp, signature);
        let mut entry = ApiStabilitySharedSurface {
            findings: ApiStabilityFindings::with_capacity_and_hasher(
                surface.findings.len(),
                Default::default(),
            ),
            removed: Vec::new(),
        };
        for (finding_key, finding) in &surface.findings {
            if own.excludes(tp.checker, finding_key, finding) {
                entry.removed.push(*finding_key);
                continue;
            }
            entry.findings.insert(*finding_key, *finding);
        }
        tp.links()
            .api_stability_surface_signature
            .insert(signature, entry);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.publishCompleteSurfaces
    /// publishCompleteSurfaces pushes every complete, settled, context-free session
    /// result into the per-checker shared caches. It runs after the settle fixpoint
    /// (including cycle promotion) so an in-progress or cycle-cut surface is never
    /// published. Results computed under a substitution carrier stay analysis-local.
    /// Symbol surfaces stay analysis-local too: a symbol root composes its own
    /// declared identity with the reusable concrete type and signature surfaces.
    pub fn publish_complete_surfaces(&mut self, tp: &mut TypeParser<'_>) {
        let types: Vec<(ApiStabilityTypeKey, ApiStabilitySurface)> = self
            .session_types
            .iter()
            .filter(|(key, _)| key.carrier.is_nil())
            .map(|(key, surface)| (*key, surface.clone()))
            .collect();
        for (key, surface) in types {
            self.publish_type_surface(tp, key.t, key.inspect, &surface);
        }
        let signatures: Vec<(ApiStabilitySignatureKey, ApiStabilitySurface)> = self
            .session_signatures
            .iter()
            .filter(|(key, _)| key.carrier.is_nil())
            .map(|(key, surface)| (*key, surface.clone()))
            .collect();
        for (key, surface) in signatures {
            self.publish_signature_surface(tp, key.signature, &surface);
        }
    }
}

// Go: typeparser/api_stability.go apiStabilityNodeVisitKey
/// apiStabilityNodeVisitKey renders a stable visit key for a declaration node.
pub fn api_stability_node_visit_key(node: Node) -> String {
    if node.is_nil() {
        return String::new();
    }
    let mut file = "";
    let source_file = get_source_file_of_node(node);
    if source_file.is_some() {
        file = source_file_file_name(source_file);
    }
    format!("{}#{}", file, node.pos())
}

// Go: typeparser/api_stability.go apiStabilitySymbolVisitKey
/// apiStabilitySymbolVisitKey renders a stable visit key for a symbol result.
pub fn api_stability_symbol_visit_key(c: &Checker, symbol: SymbolId) -> String {
    if symbol.is_nil() {
        return String::new();
    }
    let name = c.sym(symbol).name.as_str();
    for &declaration in c.sym(symbol).declarations.iter() {
        if declaration.is_some() {
            return format!("{}@{}", name, api_stability_node_visit_key(declaration));
        }
    }
    name.to_string()
}

// Go: typeparser/api_stability.go apiStabilityTypeVisitKey
/// apiStabilityTypeVisitKey renders a best-effort stable visit key for a
/// represented type result. It only orders the rare results that settlement had
/// to reconcile because they were stored without the store helpers.
pub fn api_stability_type_visit_key(c: &Checker, key: &ApiStabilityTypeKey) -> String {
    let mut name = String::new();
    if key.t.is_some() {
        let symbol = c.ty(key.t).symbol();
        if symbol.is_some() {
            name = c.sym(symbol).name.to_string();
        }
        if name.is_empty()
            && let Some(alias) = c.ty(key.t).alias()
            && alias.symbol().is_some()
        {
            name = c.sym(alias.symbol()).name.to_string();
        }
        if name.is_empty()
            && c.ty(key.t)
                .object_flags()
                .intersects(ObjectFlags::REFERENCE)
            && c.ty(key.t).target().is_some()
        {
            let symbol = c.ty(c.ty(key.t).target()).symbol();
            if symbol.is_some() {
                name = c.sym(symbol).name.to_string();
            }
        }
    }
    format!("{}@{}", name, key.inspect as i32)
}

// Go: typeparser/api_stability.go apiStabilitySignatureVisitKey
/// apiStabilitySignatureVisitKey renders a stable visit key for a signature
/// result.
pub fn api_stability_signature_visit_key(c: &Checker, key: &ApiStabilitySignatureKey) -> String {
    if key.signature.is_nil() {
        return String::new();
    }
    let raw = raw_signature(c, key.signature);
    if raw.is_nil() {
        return String::new();
    }
    api_stability_node_visit_key(c.sig(raw).declaration())
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.reconcileSettleOrder
    /// reconcileSettleOrder enrolls session results that were stored without the
    /// store helpers, so settlement still visits them. Production results always go
    /// through the helpers, whose size fast path keeps the order slices in sync; an
    /// analysis-local seeded result (a test that installs a cycle cut directly) is
    /// appended here in a canonical order so the visit sequence stays
    /// deterministic.
    // PORT: Go `sort.Slice` is not stable; the port sorts stably by the Go
    // bytes of the visit keys.
    pub fn reconcile_settle_order(&mut self, tp: &mut TypeParser<'_>) {
        let c: &Checker = &*tp.checker;
        if self.session_symbols.len() != self.settle_symbols.len() {
            let seen: FxHashSet<SymbolId> = self.settle_symbols.iter().copied().collect();
            let mut missing: Vec<SymbolId> = self
                .session_symbols
                .keys()
                .copied()
                .filter(|symbol| !seen.contains(symbol))
                .collect();
            missing.sort_by(|&left, &right| {
                crate::scanner_util::compare_go_strings(
                    &api_stability_symbol_visit_key(c, left),
                    &api_stability_symbol_visit_key(c, right),
                )
            });
            self.settle_symbols.extend(missing);
        }
        if self.session_types.len() != self.settle_types.len() {
            let seen: FxHashSet<ApiStabilityTypeKey> = self.settle_types.iter().copied().collect();
            let mut missing: Vec<ApiStabilityTypeKey> = self
                .session_types
                .keys()
                .copied()
                .filter(|key| !seen.contains(key))
                .collect();
            missing.sort_by(|left, right| {
                crate::scanner_util::compare_go_strings(
                    &api_stability_type_visit_key(c, left),
                    &api_stability_type_visit_key(c, right),
                )
            });
            self.settle_types.extend(missing);
        }
        if self.session_components.len() != self.settle_components.len() {
            let seen: FxHashSet<ApiStabilityTypeKey> =
                self.settle_components.iter().copied().collect();
            let mut missing: Vec<ApiStabilityTypeKey> = self
                .session_components
                .keys()
                .copied()
                .filter(|key| !seen.contains(key))
                .collect();
            missing.sort_by(|left, right| {
                crate::scanner_util::compare_go_strings(
                    &api_stability_type_visit_key(c, left),
                    &api_stability_type_visit_key(c, right),
                )
            });
            self.settle_components.extend(missing);
        }
        if self.session_signatures.len() != self.settle_signatures.len() {
            let seen: FxHashSet<ApiStabilitySignatureKey> =
                self.settle_signatures.iter().copied().collect();
            let mut missing: Vec<ApiStabilitySignatureKey> = self
                .session_signatures
                .keys()
                .copied()
                .filter(|key| !seen.contains(key))
                .collect();
            missing.sort_by(|left, right| {
                crate::scanner_util::compare_go_strings(
                    &api_stability_signature_visit_key(c, left),
                    &api_stability_signature_visit_key(c, right),
                )
            });
            self.settle_signatures.extend(missing);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.settleRound
    /// settleRound recollects every incomplete surface once and reports whether any
    /// surface changed. Complete results are always skipped: a completed result is
    /// never recomputed. A blocked result is skipped only when it was already
    /// collected at the current advancement epoch, so a materializing read or a
    /// surface change that happened after its last collection is always revisited.
    /// Results are visited in their first-insertion order so the bounded fixpoint
    /// spends its rounds deterministically; a recollection that adds a new result
    /// appends it to the end of the order and is visited in the same round, and a
    /// result stored without the store helpers is reconciled into the order before
    /// the round.
    pub fn settle_round(&mut self, tp: &mut TypeParser<'_>) -> bool {
        self.reconcile_settle_order(tp);
        let mut changed = false;
        // The order slices can grow while a round runs: recollecting a result may
        // store new results through the store helpers. The dynamic bound visits
        // them in the same round, which the range-over-int rewrite would not.
        let mut index = 0;
        while index < self.settle_types.len() {
            let key = self.settle_types[index];
            index += 1;
            let surface = self.session_types.get(&key).cloned().unwrap_or_default();
            if surface.complete || self.settle_cannot_advance(&surface) {
                continue;
            }
            let start_epoch = self.materialization_epoch;
            let frame = self.type_frames.get(&key).cloned().flatten();
            let recomputed = self.collect_type_surface(
                tp,
                key.t,
                key.inspect,
                &ApiStabilitySubstitution { frame },
            );
            let mut merged = grow_api_stability_surface(&surface, &recomputed);
            if !merged.equals(&surface) {
                changed = true;
            }
            merged.observed_epoch = start_epoch;
            self.session_types.insert(key, merged);
        }
        let mut index = 0;
        while index < self.settle_components.len() {
            let key = self.settle_components[index];
            index += 1;
            let surface = self
                .session_components
                .get(&key)
                .cloned()
                .unwrap_or_default();
            if surface.complete || self.settle_cannot_advance(&surface) {
                continue;
            }
            let start_epoch = self.materialization_epoch;
            let frame = self.type_frames.get(&key).cloned().flatten();
            let recomputed = self.collect_object_component_surface(
                tp,
                key.t,
                key.inspect,
                &ApiStabilitySubstitution { frame },
            );
            let mut merged = grow_api_stability_surface(&surface, &recomputed);
            if !merged.equals(&surface) {
                changed = true;
            }
            merged.observed_epoch = start_epoch;
            self.session_components.insert(key, merged);
        }
        let mut index = 0;
        while index < self.settle_signatures.len() {
            let key = self.settle_signatures[index];
            index += 1;
            let surface = self
                .session_signatures
                .get(&key)
                .cloned()
                .unwrap_or_default();
            if surface.complete || self.settle_cannot_advance(&surface) {
                continue;
            }
            let start_epoch = self.materialization_epoch;
            let frame = self.signature_frames.get(&key).cloned().flatten();
            let raw = raw_signature(tp.checker, key.signature);
            let recomputed = self.collect_signature_surface(
                tp,
                raw,
                key.signature,
                &ApiStabilitySubstitution { frame },
            );
            let mut merged = grow_api_stability_surface(&surface, &recomputed);
            if !merged.equals(&surface) {
                changed = true;
            }
            merged.observed_epoch = start_epoch;
            self.session_signatures.insert(key, merged);
        }
        let mut index = 0;
        while index < self.settle_symbols.len() {
            let symbol = self.settle_symbols[index];
            index += 1;
            let surface = self
                .session_symbols
                .get(&symbol)
                .cloned()
                .unwrap_or_default();
            if surface.complete || self.settle_cannot_advance(&surface) {
                continue;
            }
            let start_epoch = self.materialization_epoch;
            let recomputed = self.collect_symbol_surface(tp, symbol);
            let mut merged = grow_api_stability_surface(&surface, &recomputed);
            if !merged.equals(&surface) {
                changed = true;
            }
            merged.observed_epoch = start_epoch;
            self.session_symbols.insert(symbol, merged);
        }
        changed
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.settleCannotAdvance
    /// settleCannotAdvance reports whether a fixpoint recollection of an incomplete
    /// surface provably cannot add anything. A blocked surface is only skipped once
    /// the guard budget is exhausted and the surface was last collected at the
    /// current advancement epoch: no later analysis read could have materialized one
    /// of its components since, and no later surface change could have propagated a
    /// represented dependency finding into it. The surface stays blocked and is
    /// never promoted; a later export or call retries it after the checker
    /// materializes more of the program; an exhausted analysis also runs bounded
    /// extra rounds after the fixpoint converges, so a result that a final
    /// recollection made stale is revisited. Cycle-cut surfaces always keep
    /// recomputing: represented dependencies can still propagate even while the
    /// guard refuses new reads.
    pub fn settle_cannot_advance(&self, surface: &ApiStabilitySurface) -> bool {
        if !surface.blocked || !self.safety_budget_exhausted() {
            return false;
        }
        surface.observed_epoch == self.materialization_epoch
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.noteMaterializingRead
    /// noteMaterializingRead records that the analysis is about to perform a lazy
    /// checker read that can materialize represented components. Settlement uses the
    /// resulting epoch to decide whether a blocked result may have become
    /// collectable without authorizing any new guard read.
    pub fn note_materializing_read(&mut self) {
        self.materialization_epoch += 1;
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.noteMaterializingSymbolRead
    /// noteMaterializingSymbolRead advances the materialization epoch for the first
    /// lazy read of one symbol operation and is a no-op for repeats: a repeat cannot
    /// materialize anything new, and letting it keep the epoch moving would mark
    /// every blocked result stale forever.
    pub fn note_materializing_symbol_read(
        &mut self,
        kind: ApiStabilityMaterializationReadKind,
        symbol: SymbolId,
    ) {
        let key = ApiStabilityMaterializationReadKey {
            kind,
            symbol,
            ..Default::default()
        };
        if !self.materialization_reads.insert(key) {
            return;
        }
        self.note_materializing_read();
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.noteMaterializingNodeRead
    /// noteMaterializingNodeRead advances the materialization epoch for the first
    /// lazy read of one annotation node and is a no-op for repeats.
    pub fn note_materializing_node_read(&mut self, node: Node) {
        let key = ApiStabilityMaterializationReadKey {
            kind: ApiStabilityMaterializationReadKind::NodeType,
            node,
            ..Default::default()
        };
        if !self.materialization_reads.insert(key) {
            return;
        }
        self.note_materializing_read();
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.materializedMembersOfSymbol
    /// materializedMembersOfSymbol resolves a symbol's member table through the
    /// checker's ordinary lazy accessor after the caller established that the read
    /// is safe. The read is recorded as materializing.
    pub fn materialized_members_of_symbol(
        &mut self,
        c: &mut Checker,
        symbol: SymbolId,
    ) -> SymbolTable {
        self.note_materializing_symbol_read(ApiStabilityMaterializationReadKind::Members, symbol);
        c.get_members_of_symbol(symbol)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.materializedSignaturesOfSymbol
    /// materializedSignaturesOfSymbol resolves a signature member's signatures
    /// through the checker's ordinary lazy accessor after the caller established
    /// that the read is safe. The read is recorded as materializing.
    pub fn materialized_signatures_of_symbol(
        &mut self,
        c: &mut Checker,
        symbol: SymbolId,
    ) -> Vec<SignatureId> {
        self.note_materializing_symbol_read(
            ApiStabilityMaterializationReadKind::Signatures,
            symbol,
        );
        c.get_signatures_of_symbol(symbol)
    }
}

// Go: typeparser/api_stability.go growApiStabilitySurface
/// growApiStabilitySurface unions a recomputed surface into the previously
/// stored one. Findings only grow across fixpoint rounds, so a result
/// established in an earlier round is never lost when a later round observes a
/// dependency that is itself still settling; incompleteness is latched until
/// promotion, and a blocked result is never promoted.
pub fn grow_api_stability_surface(
    previous: &ApiStabilitySurface,
    recomputed: &ApiStabilitySurface,
) -> ApiStabilitySurface {
    let mut merged = previous.clone();
    merged.findings = ApiStabilityFindings::with_capacity_and_hasher(
        previous.findings.len() + recomputed.findings.len(),
        Default::default(),
    );
    merged.findings.extend(
        previous
            .findings
            .iter()
            .map(|(key, finding)| (*key, *finding)),
    );
    for (key, finding) in &recomputed.findings {
        match merged.findings.get(key) {
            Some(existing) if finding.level <= existing.level => {}
            _ => {
                merged.findings.insert(*key, *finding);
            }
        }
    }
    if !recomputed.complete {
        merged.complete = false;
        merged.cut_cycle = merged.cut_cycle || recomputed.cut_cycle;
        merged.blocked = merged.blocked || recomputed.blocked;
    }
    merged
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.promoteSettled
    /// promoteSettled marks cycle-cut results as complete after the fixpoint
    /// converged. A blocked result is never promoted.
    pub fn promote_settled(&mut self) {
        for surface in self.session_symbols.values_mut() {
            if !surface.complete && !surface.blocked {
                surface.complete = true;
                surface.cut_cycle = false;
            }
        }
        for surface in self.session_types.values_mut() {
            if !surface.complete && !surface.blocked {
                surface.complete = true;
                surface.cut_cycle = false;
            }
        }
        for surface in self.session_components.values_mut() {
            if !surface.complete && !surface.blocked {
                surface.complete = true;
                surface.cut_cycle = false;
            }
        }
        for surface in self.session_signatures.values_mut() {
            if !surface.complete && !surface.blocked {
                surface.complete = true;
                surface.cut_cycle = false;
            }
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.hasIncomplete
    pub fn has_incomplete(&self) -> bool {
        self.session_symbols
            .values()
            .any(|surface| !surface.complete)
            || self.session_types.values().any(|surface| !surface.complete)
            || self
                .session_components
                .values()
                .any(|surface| !surface.complete)
            || self
                .session_signatures
                .values()
                .any(|surface| !surface.complete)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.symbolSurface
    /// symbolSurface computes the dependency surface of an export symbol in this
    /// analysis. A complete result is reused by later queries of the same session; a
    /// result blocked by an unavailable component is kept, but never promoted.
    pub fn symbol_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
    ) -> ApiStabilitySurface {
        if symbol.is_nil() {
            return new_api_stability_surface();
        }
        if let Some(cached) = self.session_symbols.get(&symbol) {
            return cached.clone();
        }
        let start_epoch = self.materialization_epoch;
        let mut surface = self.collect_symbol_surface(tp, symbol);
        if surface.blocked {
            surface.observed_epoch = start_epoch;
        }
        self.store_symbol_surface(symbol, surface.clone());
        surface
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectSymbolSurface
    /// collectSymbolSurface selects the represented sources of a symbol's public
    /// surface. A value symbol's type, a class/interface/enum declared type and a
    /// type alias's declared type are read through the checker: an already computed
    /// result is preferred, and a public component the checker has not computed is
    /// materialized through the ordinary lazy accessors. Materializing one public
    /// type or signature does not recursively expand referenced types; shallow
    /// named references still use their declaration surface only. Declared member
    /// and signature `@stability` tags are metadata and are read from the
    /// declaration structure without resolving any type.
    pub fn collect_symbol_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
    ) -> ApiStabilitySurface {
        let mut surface = new_api_stability_surface();
        let empty = ApiStabilitySubstitution::default();
        let flags = tp.checker.sym(symbol).flags;
        let mut handled = false;

        // A default (or `export =`) assignment of an inline expression has no alias
        // target. The expression's represented checker type is the public surface;
        // it is materialized lazily when the checker has not computed it yet,
        // unless the expression would expand a recursive alias.
        let declarations: Vec<Node> = tp.checker.sym(symbol).declarations.to_vec();
        for declaration in declarations {
            if declaration.is_nil() || declaration.kind() != SyntaxKind::ExportAssignment {
                continue;
            }
            // PORT: Go `assignment == nil` cannot happen for an export assignment.
            let expression = declaration.expression();
            if expression.is_nil() {
                continue;
            }
            let represented = self.type_from_node_safely(tp, expression, &empty);
            if represented.is_some() {
                let child =
                    self.type_surface(tp, represented, ApiStabilityInspection::Expand, &empty);
                surface.merge(child);
                handled = true;
            }
        }

        if handled {
        } else if flags.intersects(SymbolFlags::TYPE_ALIAS) {
            let declared = self.declared_type_of_symbol(tp, symbol);
            if declared.is_some() {
                let child = self.type_surface(tp, declared, ApiStabilityInspection::Expand, &empty);
                surface.merge(child);
                handled = true;
            } else {
                handled =
                    self.collect_type_alias_declaration_surface(tp, &mut surface, symbol, &empty);
            }
        } else if flags.intersects(SymbolFlags::INTERFACE) {
            // Building an interface's declared type is lazy and never instantiates
            // a generic application; members stay unresolved until a member type is
            // requested.
            let declared = self.declared_type_of_symbol(tp, symbol);
            if declared.is_some() {
                let child = self.type_surface(tp, declared, ApiStabilityInspection::Expand, &empty);
                surface.merge(child);
                handled = true;
            }
        } else if flags.intersects(SymbolFlags::CLASS | SymbolFlags::ENUM) {
            let declared = self.declared_type_of_symbol(tp, symbol);
            if declared.is_some() {
                let child = self.type_surface(tp, declared, ApiStabilityInspection::Expand, &empty);
                surface.merge(child);
                handled = true;
            }
            let value = self.value_type_of_symbol(tp, symbol);
            if value.is_some() {
                let child = self.type_surface(tp, value, ApiStabilityInspection::Expand, &empty);
                surface.merge(child);
            }
        } else {
            let value = self.value_type_of_symbol(tp, symbol);
            if value.is_some() {
                let child = self.type_surface(tp, value, ApiStabilityInspection::Expand, &empty);
                surface.merge(child);
                handled = true;
            }
        }

        if !handled {
            let declarations: Vec<Node> = tp.checker.sym(symbol).declarations.to_vec();
            for declaration in declarations {
                let annotation = self.declaration_annotation_node(declaration);
                self.collect_declared_tag_surface(tp, &mut surface, annotation);
            }
            surface.block();
        }

        // Erased alias provenance: pair each declaration's annotation nodes with
        // the represented type the checker recorded for its annotation. The walk
        // only records alias symbols that survive in the represented component.
        let declarations: Vec<Node> = tp.checker.sym(symbol).declarations.to_vec();
        for declaration in declarations {
            if declaration.is_nil() || api_stability_has_function_like_data(declaration) {
                continue;
            }
            let represented = self.declaration_represented_type(tp, symbol, declaration);
            self.collect_declaration_provenance(tp, &mut surface, declaration, represented, &empty);
        }
        surface
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.typeOfSymbolSafely
    /// typeOfSymbolSafely returns a symbol's checker type, preferring an already
    /// computed result and otherwise materializing it through the ordinary lazy
    /// accessor when the pre-flight recursion guard establishes that the resolution
    /// is bounded. An inferred type (an initializer or a function body) is read
    /// through the checker's own lazy inference: diagnostics inference produces are
    /// attributed to the declaring file's expressions, exactly as that file's
    /// ordinary check would attribute them, so the read neither consumes nor
    /// duplicates them.
    pub fn type_of_symbol_safely(&mut self, tp: &mut TypeParser<'_>, symbol: SymbolId) -> TypeId {
        if symbol.is_nil() {
            return TypeId::NIL;
        }
        let cached =
            checker_integration::get_resolved_type_of_symbol_if_materialized(tp.checker, symbol);
        if cached.is_some() {
            return cached;
        }
        if !self.symbol_type_resolution_is_safe(tp, symbol) {
            return TypeId::NIL;
        }
        self.note_materializing_read();
        tp.checker.get_type_of_symbol_exported(symbol)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.typeFromNodeSafely
    /// typeFromNodeSafely returns the checker type of an annotation node, preferring
    /// an already resolved node and otherwise resolving it through the ordinary lazy
    /// accessor when the recursion guard establishes that the resolution is bounded.
    pub fn type_from_node_safely(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
    ) -> TypeId {
        if node.is_nil() {
            return TypeId::NIL;
        }
        let cached = checker_integration::get_resolved_type_from_type_node(tp.checker, node);
        if cached.is_some() {
            return cached;
        }
        if !self.annotation_resolution_is_safe(tp, node, subst) {
            return TypeId::NIL;
        }
        self.note_materializing_read();
        tp.checker.get_type_from_type_node_exported(node)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.declaredTypeSafely
    /// declaredTypeSafely returns a symbol's declared type, preferring an already
    /// computed result and otherwise materializing it through the ordinary lazy
    /// accessor when the recursion guard establishes that the declaration resolution
    /// is bounded.
    pub fn declared_type_safely(&mut self, tp: &mut TypeParser<'_>, symbol: SymbolId) -> TypeId {
        if symbol.is_nil() {
            return TypeId::NIL;
        }
        let cached = checker_integration::get_resolved_declared_type_of_symbol_if_materialized(
            tp.checker, symbol,
        );
        if cached.is_some() {
            return cached;
        }
        if !self.declared_type_resolution_is_safe(tp, symbol) {
            return TypeId::NIL;
        }
        self.note_materializing_symbol_read(
            ApiStabilityMaterializationReadKind::DeclaredType,
            symbol,
        );
        tp.checker.get_declared_type_of_symbol_exported(symbol)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.returnTypeSafely
    /// returnTypeSafely returns a signature's return type, preferring an already
    /// computed result and otherwise materializing it through the ordinary lazy
    /// accessor when the pre-flight recursion guard establishes that the resolution
    /// is bounded. An inferred return is read through the checker's own lazy
    /// inference; its diagnostics belong to the declaring file exactly as its
    /// ordinary check would report them.
    pub fn return_type_safely(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
        subst: &ApiStabilitySubstitution,
    ) -> TypeId {
        if signature.is_nil() {
            return TypeId::NIL;
        }
        let cached = checker_integration::get_resolved_return_type_of_signature_if_materialized(
            tp.checker, signature,
        );
        if cached.is_some() {
            return cached;
        }
        if !self.signature_return_resolution_is_safe(tp, signature, subst) {
            return TypeId::NIL;
        }
        self.note_materializing_read();
        tp.checker.get_return_type_of_signature_exported(signature)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.baseTypesSafely
    /// baseTypesSafely returns a declaration's base types, preferring already
    /// resolved base types and otherwise materializing the heritage through the
    /// ordinary lazy accessor when the recursion guard establishes that the
    /// resolution is bounded.
    pub fn base_types_safely(
        &mut self,
        tp: &mut TypeParser<'_>,
        declaration: TypeId,
        subst: &ApiStabilitySubstitution,
    ) -> (Vec<TypeId>, bool) {
        if declaration.is_nil() {
            return (Vec::new(), false);
        }
        let (bases, resolved) =
            checker_integration::get_resolved_base_types_of_type_if_materialized(
                tp.checker,
                declaration,
            );
        if resolved {
            return (bases, true);
        }
        if !self.base_types_resolution_is_safe(tp, declaration, subst) {
            return (Vec::new(), false);
        }
        self.note_materializing_read();
        (tp.checker.get_base_types_exported(declaration), true)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.declaredTypeOfSymbol
    /// declaredTypeOfSymbol returns the declared type of a class, interface, enum
    /// or type alias, preferring an already computed result and otherwise
    /// materializing it through the ordinary lazy accessor when the recursion guard
    /// establishes that the declaration resolution is bounded. Building a declared
    /// type does not expand its members; member and signature components are only
    /// read when the public surface actually reaches them.
    pub fn declared_type_of_symbol(&mut self, tp: &mut TypeParser<'_>, symbol: SymbolId) -> TypeId {
        self.declared_type_safely(tp, symbol)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.valueTypeOfSymbol
    /// valueTypeOfSymbol returns the value type of a symbol, preferring an already
    /// computed result and otherwise materializing it through the ordinary lazy
    /// accessor when the recursion guard establishes that the annotation resolution
    /// is bounded. It is only called for symbols with a value side, so a type-only
    /// export is never forced into a value lookup.
    pub fn value_type_of_symbol(&mut self, tp: &mut TypeParser<'_>, symbol: SymbolId) -> TypeId {
        if symbol.is_nil() || !tp.checker.sym(symbol).flags.intersects(SymbolFlags::VALUE) {
            return TypeId::NIL;
        }
        self.type_of_symbol_safely(tp, symbol)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.representedTypeFromNode
    /// representedTypeFromNode returns the checker type of an annotation node under
    /// the analysis safety guard. The node is never used as a syntax-level dependency
    /// source: the returned represented type is what the surface traversal walks.
    pub fn represented_type_from_node(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        subst: &ApiStabilitySubstitution,
    ) -> TypeId {
        self.type_from_node_safely(tp, node, subst)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectTypeAliasDeclarationSurface
    /// collectTypeAliasDeclarationSurface reads the public surface of a type alias
    /// whose declared type is not available from the checker. Only declaration
    /// `@stability` tags are metadata-read here; the surface stays incomplete
    /// because the represented type the traversal needs is genuinely unavailable.
    pub fn collect_type_alias_declaration_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        symbol: SymbolId,
        subst: &ApiStabilitySubstitution,
    ) -> bool {
        let declaration = api_stability_type_alias_declaration(tp.checker, symbol);
        if declaration.is_nil() {
            return false;
        }
        let rhs = declaration.type_();
        if rhs.is_nil() {
            return false;
        }
        let represented = self.represented_type_from_node(tp, rhs, subst);
        if represented.is_some() {
            let child = self.type_surface(tp, represented, ApiStabilityInspection::Expand, subst);
            surface.merge(child);
            return true;
        }
        self.collect_declared_tag_surface(tp, surface, rhs);
        false
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectDeclaredTagSurface
    /// collectDeclaredTagSurface reads `@stability` tags from the declaration
    /// structure of an annotation whose represented type is unavailable. Tags are
    /// metadata: no annotation is resolved, no referenced declaration is followed,
    /// and no substitution is built. Conditional branches are visited only when the
    /// conditional is deferred, so a tag that the compiler eliminated on a concrete
    /// conditional is never reported.
    pub fn collect_declared_tag_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        node: Node,
    ) {
        if node.is_nil() {
            return;
        }
        match node.kind() {
            SyntaxKind::ParenthesizedType => {
                self.collect_declared_tag_surface(tp, surface, node.type_());
            }
            SyntaxKind::TypeLiteral => {
                for member in node.members() {
                    if member.is_nil() || api_stability_property_declaration_is_internal(member) {
                        continue;
                    }
                    if api_stability_has_function_like_data(member) {
                        self.record_declaration_stability(tp, surface, member);
                        continue;
                    }
                    match member.kind() {
                        SyntaxKind::PropertySignature => {
                            self.record_declaration_stability(tp, surface, member);
                            self.collect_declared_tag_surface(tp, surface, member.type_());
                        }
                        SyntaxKind::IndexSignature => {
                            self.record_declaration_stability(tp, surface, member);
                        }
                        _ => {}
                    }
                }
            }
            SyntaxKind::FunctionType | SyntaxKind::ConstructorType => {
                self.record_declaration_stability(tp, surface, node);
            }
            SyntaxKind::ConditionalType => {
                if !self.conditional_declaration_is_deferred(tp, node) {
                    return;
                }
                self.collect_declared_tag_surface(tp, surface, node.true_type());
                self.collect_declared_tag_surface(tp, surface, node.false_type());
            }
            SyntaxKind::UnionType | SyntaxKind::IntersectionType => {
                let list = node.types();
                if list.is_some() {
                    for member in list.nodes() {
                        self.collect_declared_tag_surface(tp, surface, member);
                    }
                }
            }
            SyntaxKind::ArrayType => {
                self.collect_declared_tag_surface(tp, surface, node.element_type());
            }
            SyntaxKind::TupleType => {
                for element in node.elements() {
                    self.collect_declared_tag_surface(tp, surface, element);
                }
            }
            SyntaxKind::NamedTupleMember
            | SyntaxKind::OptionalType
            | SyntaxKind::RestType
            | SyntaxKind::TypeOperator => {
                self.collect_declared_tag_surface(tp, surface, node.type_());
            }
            _ => {}
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.typeSurface
    /// typeSurface computes the surface of a represented type in this analysis. A
    /// complete, context-free result is first looked up in the per-checker shared
    /// caches and composed from the immutable snapshot plus the component's declared
    /// tag when one exists; a result computed under a substitution carrier or cut by
    /// an active type or by the analysis bound stays analysis-local, is returned
    /// incomplete where relevant, and is settled before it is reused.
    pub fn type_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) -> ApiStabilitySurface {
        if t.is_nil() {
            return new_api_stability_surface();
        }
        let key = ApiStabilityTypeKey {
            t,
            inspect,
            carrier: subst.carrier(),
        };
        if let Some(cached) = self.session_types.get(&key) {
            return cached.clone();
        }
        if key.carrier.is_nil()
            && let Some(shared) = self.shared_type_surface(tp, t, inspect)
        {
            self.store_type_surface(key, shared.clone());
            return shared;
        }
        self.type_frames.insert(key, subst.frame.clone());
        if self.active_types.get(&key).copied().unwrap_or(false) {
            return cycle_cut_surface();
        }
        if !self.consume_work() {
            return blocked_surface();
        }
        self.active_types.insert(key, true);
        let start_epoch = self.materialization_epoch;
        let mut surface = self.collect_type_surface(tp, t, inspect, subst);
        self.active_types.remove(&key);
        if surface.blocked {
            surface.observed_epoch = start_epoch;
        }
        self.store_type_surface(key, surface.clone());
        surface
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectTypeSurface
    // PORT: Go `defer delete(a.activeAliases, aliasKey)` runs after the flag
    // switch; the switch is the labeled block below, and the key is removed
    // after it.
    pub fn collect_type_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) -> ApiStabilitySurface {
        let mut surface = new_api_stability_surface();
        if t.is_nil() || !self.consume_work() {
            if t.is_some() {
                surface.block();
            }
            return surface;
        }

        // A component reached through the shallow named-reference boundary honors
        // its own explicit declared tag: the tag contributes at its declared level
        // and the component's internals are not expanded. Independently represented
        // type arguments stay exposed. Roots are never reached with the shallow
        // boundary, so a root tag never stops the root walk.
        if inspect == ApiStabilityInspection::Shallow {
            let identity = self.type_child_boundary_symbol(tp, t);
            if identity.is_some() {
                self.record_symbol(tp, &mut surface, identity);
                self.collect_child_type_arguments(tp, &mut surface, t, subst);
                return surface;
            }
        }

        // An alias keeps its symbol and represented type arguments. The underlying
        // representation is still inspected below; the alias is recorded first so an
        // erased or shallow alias can still be named. A repeated application of the
        // same concrete alias object in the same concrete carrier exposes nothing
        // new, which terminates recursive aliases such as `Deep<T>` without
        // evaluating them.
        let mut deferred_alias_key = None;
        if let Some(alias) = tp.checker.ty(t).alias() {
            self.record_symbol(tp, &mut surface, alias.symbol());
            if !self.alias_arguments_are_self(tp, Some(&alias))
                && api_stability_represented_surface_accepts_arguments(tp.checker, t)
            {
                for &argument in alias.type_arguments() {
                    let child =
                        self.type_surface(tp, argument, ApiStabilityInspection::Shallow, subst);
                    surface.merge(child);
                }
            }
            let alias_key = ApiStabilityAliasKey {
                t,
                carrier: subst.carrier(),
            };
            if self
                .active_aliases
                .get(&alias_key)
                .copied()
                .unwrap_or(false)
            {
                surface.cut_by_cycle();
                return surface;
            }
            self.active_aliases.insert(alias_key, true);
            deferred_alias_key = Some(alias_key);
        }

        let flags = tp.checker.ty(t).flags();
        'switch: {
            if flags.intersects(TypeFlags::TYPE_PARAMETER) {
                let symbol = tp.checker.ty(t).symbol();
                self.record_symbol(tp, &mut surface, symbol);
                let (mapped, parent, ok) = self.substitute(tp, subst, t);
                if ok {
                    if mapped.is_nil() {
                        // The parameter was replaced by a represented argument the
                        // checker has not materialized. The component is unknown, so
                        // the result is incomplete and never promoted.
                        surface.block();
                        break 'switch;
                    }
                    let child =
                        self.type_surface(tp, mapped, ApiStabilityInspection::Shallow, &parent);
                    surface.merge(child);
                    break 'switch;
                }
                self.collect_type_parameter_components(tp, &mut surface, t, subst);
            } else if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                let members = tp.checker.ty(t).types().to_vec();
                for member in members {
                    let child =
                        self.type_surface(tp, member, ApiStabilityInspection::Shallow, subst);
                    surface.merge(child);
                }
            } else if flags.intersects(TypeFlags::CONDITIONAL) {
                self.collect_conditional_surface(tp, &mut surface, t, subst);
            } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
                // PORT: Go `indexed != nil` always holds for an indexed access type.
                let indexed = tp.checker.ty(t).as_indexed_access_type();
                let (object_type, index_type) = (indexed.object_type, indexed.index_type);
                let child =
                    self.type_surface(tp, object_type, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
                let child =
                    self.type_surface(tp, index_type, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
                // The projected result of an indexed access is a compiler outcome that
                // the represented operands do not establish; inspecting the operands
                // alone can never complete the surface.
                surface.block();
            } else if flags.intersects(TypeFlags::INDEX) {
                let target = tp.checker.ty(t).as_index_type().target;
                let child = self.type_surface(tp, target, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
            } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                let parts = tp.checker.ty(t).as_template_literal_type().types.to_vec();
                for part in parts {
                    let child = self.type_surface(tp, part, ApiStabilityInspection::Shallow, subst);
                    surface.merge(child);
                }
            } else if flags.intersects(TypeFlags::STRING_MAPPING) {
                let symbol = tp.checker.ty(t).symbol();
                self.record_symbol(tp, &mut surface, symbol);
                let target = tp.checker.ty(t).as_string_mapping_type().target;
                let child = self.type_surface(tp, target, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
            } else if flags.intersects(TypeFlags::SUBSTITUTION) {
                let substitution = tp.checker.ty(t).as_substitution_type();
                let (base_type, constraint) = (substitution.base_type, substitution.constraint);
                let child =
                    self.type_surface(tp, base_type, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
                let child =
                    self.type_surface(tp, constraint, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
            } else if flags.intersects(TypeFlags::OBJECT) {
                self.collect_object_surface(tp, &mut surface, t, inspect, subst);
            } else if flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL | TypeFlags::ENUM) {
                let symbol = tp.checker.ty(t).symbol();
                self.record_symbol(tp, &mut surface, symbol);
            }
        }
        if let Some(alias_key) = deferred_alias_key {
            self.active_aliases.remove(&alias_key);
        }
        surface
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectTypeParameterComponents
    /// collectTypeParameterComponents records the constraint and default of a type
    /// parameter, preferring cached checker materializations and otherwise
    /// materializing the annotation through the ordinary lazy accessor. An
    /// annotation that would expand a recursive alias, or one that is genuinely
    /// unavailable, leaves the surface incomplete.
    pub fn collect_type_parameter_components(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        let mut constraint =
            checker_integration::get_resolved_constraint_of_type_parameter_if_materialized(
                tp.checker, t,
            );
        if constraint.is_nil() {
            let node = type_parameter_annotation_node(tp.checker, t, true);
            constraint = self.represented_type_from_node(tp, node, subst);
        }
        if constraint.is_some() {
            let child = self.type_surface(tp, constraint, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
        } else if type_parameter_annotation_node(tp.checker, t, true).is_some() {
            surface.block();
        }
        let mut default_type =
            checker_integration::get_resolved_default_from_type_parameter_if_materialized(
                tp.checker, t,
            );
        if default_type.is_nil() {
            let node = type_parameter_annotation_node(tp.checker, t, false);
            default_type = self.represented_type_from_node(tp, node, subst);
        }
        if default_type.is_some() {
            let child = self.type_surface(tp, default_type, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
        } else if type_parameter_annotation_node(tp.checker, t, false).is_some() {
            surface.block();
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectConditionalSurface
    /// collectConditionalSurface inspects a represented conditional type without
    /// guessing its selected branch. A conditional whose operands are substituted by
    /// an active concrete application, or whose operands are fully concrete, has an
    /// outcome the compiler relation selects; unless that outcome is already
    /// represented on the type, the surface is blocked instead of guessed. A
    /// deferred symbolic conditional (its operands still mention an unbound type
    /// parameter) keeps its operands and both declared branches; the declared branch
    /// components are materialized lazily through the checker's ordinary reads.
    // PORT: Go `defer delete(a.activeConditionals, conditionalKey)`: the body
    // after the insert is the labeled block, and the key is removed after it.
    pub fn collect_conditional_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        // PORT: Go `conditional == nil` cannot happen for a conditional type.
        // A conditional re-entered on the same recursion path (a recursive alias
        // whose represented branch resolves back to the same type) has already
        // contributed its direct components at the first occurrence. The cut stays
        // incomplete until the settle fixpoint has unified the findings.
        let conditional_key = ApiStabilityAliasKey {
            t,
            carrier: subst.carrier(),
        };
        if self
            .active_conditionals
            .get(&conditional_key)
            .copied()
            .unwrap_or(false)
        {
            surface.cut_by_cycle();
            return;
        }
        self.active_conditionals.insert(conditional_key, true);

        'body: {
            let conditional = tp.checker.ty(t).as_conditional_type();
            let (check, extends) = (conditional.check_type, conditional.extends_type);
            if tp.checker.ty(t).mapper().is_some()
                || self.contains_substituted_parameter(tp, subst, check, &mut FxHashMap::default())
                || self.contains_substituted_parameter(
                    tp,
                    subst,
                    extends,
                    &mut FxHashMap::default(),
                )
            {
                // A concrete application: the selected branch is a compiler outcome
                // that is not represented on the raw conditional. Never guess it.
                surface.block();
                break 'body;
            }
            if !self.represented_contains_type_parameter(tp, check, &mut FxHashMap::default())
                && !self.represented_contains_type_parameter(tp, extends, &mut FxHashMap::default())
            {
                // A conditional over fully concrete operands also has an outcome that
                // is not represented; refuse to guess as well.
                surface.block();
                break 'body;
            }
            let child = self.type_surface(tp, check, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
            let child = self.type_surface(tp, extends, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
            self.collect_conditional_branch_surface(tp, surface, t, true, subst);
            self.collect_conditional_branch_surface(tp, surface, t, false, subst);
        }
        self.active_conditionals.remove(&conditional_key);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.containsSubstitutedParameter
    /// containsSubstitutedParameter reports whether any type parameter reachable in
    /// the represented operand graph is replaced by the active substitution. It only
    /// reads represented components; it never evaluates a relation.
    // PORT: Go `defer delete(active, t)`: the body after the insert is the
    // labeled block, and `t` is removed after it.
    pub fn contains_substituted_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        subst: &ApiStabilitySubstitution,
        t: TypeId,
        active: &mut FxHashMap<TypeId, bool>,
    ) -> bool {
        if t.is_nil() || active.get(&t).copied().unwrap_or(false) {
            return false;
        }
        active.insert(t, true);
        let result = 'body: {
            let flags = tp.checker.ty(t).flags();
            if flags.intersects(TypeFlags::TYPE_PARAMETER) {
                let (_, _, ok) = self.substitute(tp, subst, t);
                break 'body ok;
            }
            if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                let members = tp.checker.ty(t).types().to_vec();
                for member in members {
                    if self.contains_substituted_parameter(tp, subst, member, active) {
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
                    if self.contains_substituted_parameter(tp, subst, argument, active) {
                        break 'body true;
                    }
                }
            } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
                let indexed = tp.checker.ty(t).as_indexed_access_type();
                let (object_type, index_type) = (indexed.object_type, indexed.index_type);
                break 'body self.contains_substituted_parameter(tp, subst, object_type, active)
                    || self.contains_substituted_parameter(tp, subst, index_type, active);
            } else if flags.intersects(TypeFlags::INDEX) {
                let target = tp.checker.ty(t).as_index_type().target;
                break 'body self.contains_substituted_parameter(tp, subst, target, active);
            } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                let parts = tp.checker.ty(t).as_template_literal_type().types.to_vec();
                for part in parts {
                    if self.contains_substituted_parameter(tp, subst, part, active) {
                        break 'body true;
                    }
                }
            } else if flags.intersects(TypeFlags::CONDITIONAL) {
                let conditional = tp.checker.ty(t).as_conditional_type();
                let (check, extends) = (conditional.check_type, conditional.extends_type);
                break 'body self.contains_substituted_parameter(tp, subst, check, active)
                    || self.contains_substituted_parameter(tp, subst, extends, active);
            }
            false
        };
        active.remove(&t);
        result
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.representedContainsTypeParameter
    /// representedContainsTypeParameter reports whether an unbound type parameter is
    /// a reachable component of the represented operand graph.
    // PORT: Go `defer delete(active, t)`, as in `contains_substituted_parameter`.
    pub fn represented_contains_type_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        active: &mut FxHashMap<TypeId, bool>,
    ) -> bool {
        if t.is_nil() || active.get(&t).copied().unwrap_or(false) {
            return false;
        }
        active.insert(t, true);
        let result = 'body: {
            let flags = tp.checker.ty(t).flags();
            if flags.intersects(TypeFlags::TYPE_PARAMETER) {
                break 'body true;
            }
            if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                let members = tp.checker.ty(t).types().to_vec();
                for member in members {
                    if self.represented_contains_type_parameter(tp, member, active) {
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
                    if self.represented_contains_type_parameter(tp, argument, active) {
                        break 'body true;
                    }
                }
            } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
                let indexed = tp.checker.ty(t).as_indexed_access_type();
                let (object_type, index_type) = (indexed.object_type, indexed.index_type);
                break 'body self.represented_contains_type_parameter(tp, object_type, active)
                    || self.represented_contains_type_parameter(tp, index_type, active);
            } else if flags.intersects(TypeFlags::INDEX) {
                let target = tp.checker.ty(t).as_index_type().target;
                break 'body self.represented_contains_type_parameter(tp, target, active);
            } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                let parts = tp.checker.ty(t).as_template_literal_type().types.to_vec();
                for part in parts {
                    if self.represented_contains_type_parameter(tp, part, active) {
                        break 'body true;
                    }
                }
            } else if flags.intersects(TypeFlags::CONDITIONAL) {
                let conditional = tp.checker.ty(t).as_conditional_type();
                let (check, extends) = (conditional.check_type, conditional.extends_type);
                break 'body self.represented_contains_type_parameter(tp, check, active)
                    || self.represented_contains_type_parameter(tp, extends, active);
            } else if flags.intersects(TypeFlags::SUBSTITUTION) {
                let substitution = tp.checker.ty(t).as_substitution_type();
                let (base_type, constraint) = (substitution.base_type, substitution.constraint);
                break 'body self.represented_contains_type_parameter(tp, base_type, active)
                    || self.represented_contains_type_parameter(tp, constraint, active);
            }
            false
        };
        active.remove(&t);
        result
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectConditionalBranchSurface
    /// collectConditionalBranchSurface inspects one declared branch of a deferred
    /// conditional. The checker's already resolved branch components are preferred;
    /// otherwise the declared branch annotation is materialized lazily through the
    /// ordinary type accessor and traversed as a represented component. A primitive
    /// keyword branch exposes nothing. A genuinely unavailable branch leaves the
    /// surface incomplete.
    pub fn collect_conditional_branch_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        true_branch: bool,
        subst: &ApiStabilitySubstitution,
    ) {
        let node =
            checker_integration::get_conditional_type_branch_node(tp.checker, t, true_branch);
        if node.is_nil() {
            surface.block();
            return;
        }
        let mut represented =
            checker_integration::get_resolved_conditional_type_branch(tp.checker, t, true_branch);
        if represented.is_nil() {
            represented = self.represented_type_from_node(tp, node, subst);
        }
        if represented.is_nil() {
            if api_stability_primitive_type_node(node) {
                return;
            }
            surface.block();
            return;
        }
        let child = self.type_surface(tp, represented, ApiStabilityInspection::Shallow, subst);
        surface.merge(child);
        self.collect_type_node_provenance(tp, surface, node, represented, subst);
    }
}

// Go: typeparser/api_stability.go apiStabilityPrimitiveTypeNode
/// apiStabilityPrimitiveTypeNode reports whether a type node resolves to a
/// primitive singleton that carries no symbol or member surface. Those nodes do
/// not cache a resolved type and expose nothing.
pub fn api_stability_primitive_type_node(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    matches!(
        node.kind(),
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
    )
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectObjectSurface
    /// collectObjectSurface handles object types with the represented structure
    /// rules used by the data-first matcher. A class or interface declaration is
    /// inspected through its declared members and heritage clauses; an expanded
    /// reference prefers its concrete materialized surface and otherwise
    /// materializes it lazily. A named reference reached in a shallow position
    /// keeps its declaration, represented arguments and callable signatures and its
    /// members are not expanded. Recursive alias expansions are refused by the
    /// lazy guards.
    pub fn collect_object_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) {
        let flags = tp.checker.ty(t).object_flags();
        if flags.intersects(ObjectFlags::REVERSE_MAPPED | ObjectFlags::EVOLVING_ARRAY) {
            return;
        }
        if flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE) {
            // An instantiation expression such as `identity<E>` represents an
            // explicitly instantiated callable value whose signatures are safe to
            // read from the checker's represented members.
            self.collect_function_signatures(tp, surface, t, t, subst);
            return;
        }
        // A class declared instance type is a class-or-interface surface inspected
        // through its declaration. The class static side is a separate anonymous
        // object type whose symbol is the class.
        if flags.intersects(ObjectFlags::CLASS_OR_INTERFACE) {
            self.collect_declaration_surface(tp, surface, t, inspect, subst);
            return;
        }
        if flags.intersects(ObjectFlags::ANONYMOUS) {
            let symbol = tp.checker.ty(t).symbol();
            if symbol.is_some() && tp.checker.sym(symbol).flags.intersects(SymbolFlags::CLASS) {
                self.collect_class_static_surface(tp, surface, t, inspect, subst);
                return;
            }
        }
        // A module namespace is a named reference to a whole module's exports, not a
        // finite anonymous surface. Recording the module symbol keeps the traversal
        // bounded instead of enumerating and reporting every exported member; the
        // rule enumerates namespace members explicitly.
        let symbol = tp.checker.ty(t).symbol();
        if symbol.is_some()
            && tp.checker.sym(symbol).flags.intersects(
                SymbolFlags::MODULE | SymbolFlags::VALUE_MODULE | SymbolFlags::NAMESPACE_MODULE,
            )
        {
            self.record_symbol(tp, surface, symbol);
            return;
        }
        if flags.intersects(ObjectFlags::REFERENCE)
            && tp.checker.ty(t).target().is_some()
            && tp.checker.ty(t).target() != t
        {
            let target = tp.checker.ty(t).target();
            let target_symbol = tp.checker.ty(target).symbol();
            if self.should_inspect_symbol(tp, target_symbol) {
                self.record_symbol(tp, surface, target_symbol);
            }
            let arguments = self.reference_arguments(tp, t, target);
            for &argument in &arguments {
                if argument.is_nil() {
                    continue;
                }
                let child = self.type_surface(tp, argument, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
            }
            if tp
                .checker
                .ty(target)
                .object_flags()
                .intersects(ObjectFlags::CLASS_OR_INTERFACE)
            {
                if !self.should_inspect_symbol(tp, target_symbol) {
                    // A default-library target contributes only its represented
                    // type arguments; its member surface is never expanded.
                    return;
                }
                if inspect == ApiStabilityInspection::Shallow {
                    // The shallow boundary: declared symbol, represented type
                    // arguments and directly exposed call and construct signatures
                    // only. The raw declaration surface is read with the reference
                    // substitution, so an instantiated signature argument or return
                    // stays represented without expanding arbitrary members.
                    let parameters = self.reference_type_parameters(tp, target);
                    let reference_subst =
                        self.extend_parameters_substitution(subst, t, &parameters, &arguments);
                    self.collect_declaration_surface(
                        tp,
                        surface,
                        target,
                        ApiStabilityInspection::Shallow,
                        &reference_subst,
                    );
                    return;
                }
                self.collect_concrete_reference_surface(tp, surface, t);
                return;
            }
            return;
        }
        if flags.intersects(ObjectFlags::MAPPED) {
            self.collect_mapped_surface(tp, surface, t, subst);
            return;
        }
        let child = self.component_surface(tp, t, inspect, subst);
        surface.merge(child);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectConcreteReferenceSurface
    /// collectConcreteReferenceSurface inspects the concrete public surface of a
    /// class or interface reference in an expanded context. The checker's resolved
    /// member table is preferred and read directly; the pre-flight recursion guard
    /// is only consulted when an individual accessor has not been materialized yet.
    /// Each unresolved member, signature and index read shares one completed Safe
    /// verdict for the same reference and substitution, so an already resolved
    /// accessor never consumes safety work. An unmaterialized member table whose
    /// member resolution is not established as bounded falls back to the target
    /// declaration under the reference substitution with its own gated reads, and a
    /// surface whose member or index resolution would evaluate a recursive alias is
    /// left incomplete. Concrete members are already instantiated, so no
    /// substitution applies to them; nested named references reached inside those
    /// members still use the shallow boundary.
    pub fn collect_concrete_reference_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        reference: TypeId,
    ) {
        let empty = ApiStabilitySubstitution::default();
        let mut target = TypeId::NIL;
        let mut reference_subst = ApiStabilitySubstitution::default();
        if tp
            .checker
            .ty(reference)
            .object_flags()
            .intersects(ObjectFlags::REFERENCE)
        {
            target = tp.checker.ty(reference).target();
        }
        let owner = tp.checker.ty(reference).symbol();
        if target.is_some()
            && target != reference
            && tp
                .checker
                .ty(target)
                .object_flags()
                .intersects(ObjectFlags::CLASS_OR_INTERFACE)
        {
            let parameters = self.reference_type_parameters(tp, target);
            let arguments = self.reference_arguments(tp, reference, target);
            reference_subst =
                self.extend_parameters_substitution(&empty, reference, &parameters, &arguments);
        }
        let (members, resolved) = checker_integration::get_resolved_members_of_type_if_materialized(
            tp.checker, reference,
        );
        if resolved {
            self.collect_resolved_member_table(tp, surface, members, owner, &empty);
        } else {
            let structured_safe =
                self.member_table_resolution_is_safe(tp, reference, &reference_subst);
            if structured_safe {
                self.note_materializing_read();
                for member in tp.checker.get_properties_of_type_exported(reference) {
                    if member.is_nil()
                        || tp
                            .checker
                            .sym(member)
                            .flags
                            .intersects(SymbolFlags::TYPE_PARAMETER)
                        || api_stability_symbol_is_non_public(tp.checker, member)
                    {
                        continue;
                    }
                    if self.optional_tagged_inherited_member(tp, member, owner) {
                        continue;
                    }
                    let boundary = self.inherited_child_boundary_symbol(tp, member, owner);
                    if boundary.is_some() {
                        self.record_symbol(tp, surface, boundary);
                        continue;
                    }
                    let child = self.member_surface(tp, member, &empty);
                    surface.merge(child);
                }
            } else if target.is_some()
                && target != reference
                && tp
                    .checker
                    .ty(target)
                    .object_flags()
                    .intersects(ObjectFlags::CLASS_OR_INTERFACE)
            {
                // The declaration surface under the reference substitution never
                // resolves the instantiated member table; each member and index read
                // is gated on its own.
                self.collect_declaration_surface(
                    tp,
                    surface,
                    target,
                    ApiStabilityInspection::Expand,
                    &reference_subst,
                );
                return;
            } else {
                surface.block();
            }
        }
        // The resolved member table has already flattened inherited members; the
        // tagged ancestors still expose their independently represented type
        // arguments even though their members stay bounded.
        self.collect_tagged_heritage_arguments(
            tp,
            surface,
            target,
            &reference_subst,
            &mut FxHashMap::default(),
        );
        for kind in [SignatureKind::CALL, SignatureKind::CONSTRUCT] {
            let (signatures, ok) =
                checker_integration::get_resolved_signatures_of_type_if_materialized(
                    tp.checker, reference, kind,
                );
            if ok {
                for signature in signatures {
                    if self.child_signature_boundary(tp, surface, signature, owner) {
                        continue;
                    }
                    let child = self.signature_surface(tp, signature, &empty);
                    surface.merge(child);
                }
                continue;
            }
            if !self.member_table_resolution_is_safe(tp, reference, &reference_subst) {
                surface.block();
                continue;
            }
            self.note_materializing_read();
            for signature in tp.checker.get_signatures_of_type_exported(reference, kind) {
                if self.child_signature_boundary(tp, surface, signature, owner) {
                    continue;
                }
                let child = self.signature_surface(tp, signature, &empty);
                surface.merge(child);
            }
        }
        let (infos, ok) = checker_integration::get_resolved_index_infos_of_type_if_materialized(
            tp.checker, reference,
        );
        if ok {
            for info in infos {
                if info.is_nil() {
                    continue;
                }
                self.collect_index_info_surface(tp, surface, info, &empty, owner);
            }
            return;
        }
        if !self.member_table_resolution_is_safe(tp, reference, &reference_subst) {
            surface.block();
            return;
        }
        self.note_materializing_read();
        for info in tp.checker.get_index_infos_of_type_exported(reference) {
            if info.is_nil() {
                continue;
            }
            self.collect_index_info_surface(tp, surface, info, &empty, owner);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectResolvedMemberTable
    /// collectResolvedMemberTable merges the public members of an already resolved
    /// member table with the given substitution. Compiler-reserved internal entries
    /// are skipped, while a late-bound computed member is traversed like any other
    /// member: it is the resolved spelling of a public computed declaration and its
    /// visibility is decided by its declaration provenance. A member the compiler
    /// flattened into this table from an explicitly tagged base is a child boundary:
    /// the base's declared level contributes and the member's surface is not
    /// expanded. A same-named own override keeps the owner as its declaring symbol
    /// and stays inspected.
    pub fn collect_resolved_member_table(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        members: SymbolTable,
        owner: SymbolId,
        subst: &ApiStabilitySubstitution,
    ) {
        for name in sorted_symbol_table_keys(tp.checker, members) {
            let member = tp.checker.symbols.get(members, &name);
            if member.is_nil()
                || tp
                    .checker
                    .sym(member)
                    .flags
                    .intersects(SymbolFlags::TYPE_PARAMETER)
                || api_stability_symbol_is_non_public(tp.checker, member)
            {
                continue;
            }
            if !api_stability_member_table_name_is_visible(tp.checker, &name, member) {
                continue;
            }
            if self.optional_tagged_inherited_member(tp, member, owner) {
                continue;
            }
            let boundary = self.inherited_child_boundary_symbol(tp, member, owner);
            if boundary.is_some() {
                self.record_symbol(tp, surface, boundary);
                continue;
            }
            let child = self.member_surface(tp, member, subst);
            surface.merge(child);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.referenceArguments
    /// referenceArguments returns the represented type arguments of a reference,
    /// filling omitted arguments from the target's already materialized type
    /// parameter defaults. A parameter the checker left unrepresented stays nil so
    /// the substitution machinery blocks instead of assuming a constraint or
    /// default.
    pub fn reference_arguments(
        &mut self,
        tp: &mut TypeParser<'_>,
        reference: TypeId,
        target: TypeId,
    ) -> Vec<TypeId> {
        if target.is_nil()
            || !tp.checker.ty(target).flags().intersects(TypeFlags::OBJECT)
            || !tp
                .checker
                .ty(target)
                .object_flags()
                .intersects(ObjectFlags::CLASS_OR_INTERFACE)
        {
            return checker_integration::get_resolved_type_arguments(tp.checker, reference);
        }
        let parameters = self.reference_type_parameters(tp, target);
        if parameters.is_empty() {
            return Vec::new();
        }
        let resolved = checker_integration::get_resolved_type_arguments(tp.checker, reference);
        let mut arguments = vec![TypeId::NIL; parameters.len()];
        for (index, &parameter) in parameters.iter().enumerate() {
            if index < resolved.len() && resolved[index].is_some() {
                arguments[index] = resolved[index];
                continue;
            }
            let default_type =
                checker_integration::get_resolved_default_from_type_parameter_if_materialized(
                    tp.checker, parameter,
                );
            if default_type.is_some() {
                arguments[index] = default_type;
            }
        }
        arguments
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.referenceTypeParameters
    /// referenceTypeParameters returns the type parameters of a class or interface
    /// reference target. Other reference targets (tuples, arrays of primitives) have
    /// no bindable type parameters and return nil.
    pub fn reference_type_parameters(&mut self, tp: &mut TypeParser<'_>, t: TypeId) -> Vec<TypeId> {
        if t.is_nil()
            || !tp.checker.ty(t).flags().intersects(TypeFlags::OBJECT)
            || !tp
                .checker
                .ty(t)
                .object_flags()
                .intersects(ObjectFlags::CLASS_OR_INTERFACE)
        {
            return Vec::new();
        }
        tp.checker
            .ty(t)
            .as_interface_type()
            .type_parameters()
            .to_vec()
    }
}
