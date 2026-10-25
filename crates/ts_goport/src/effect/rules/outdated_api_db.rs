//! Port of Effect-TS/tsgo `internal/rules/outdated_api_db.go`: the Effect v3
//! to v4 migration table that the `outdatedApi` rule reads.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;
use std::sync::LazyLock;

/// Go `MigrationTag`: identifies the type of migration for an API. The
/// variants are the Go consts `MigrationTagUnchanged` and so on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MigrationTag {
    Unchanged,
    RenamedSameBehaviour,
    RenamedAndNeedsOptions,
    Removed,
}

impl MigrationTag {
    /// The Go string value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MigrationTag::Unchanged => "Unchanged",
            MigrationTag::RenamedSameBehaviour => "RenamedSameBehaviour",
            MigrationTag::RenamedAndNeedsOptions => "RenamedAndNeedsOptions",
            MigrationTag::Removed => "Removed",
        }
    }
}

/// Go `Migration`: describes how an Effect v3 API maps to v4.
// PORT: the table is static data, so the Go strings are `&'static str`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Migration {
    pub tag: MigrationTag,
    /// for RenamedSameBehaviour and RenamedAndNeedsOptions
    pub new_name: &'static str,
    /// for RenamedAndNeedsOptions
    pub options_instructions: &'static str,
    /// for Removed
    pub alternative_pattern: &'static str,
}

// Go: unchanged
#[must_use]
pub const fn unchanged() -> Migration {
    Migration {
        tag: MigrationTag::Unchanged,
        new_name: "",
        options_instructions: "",
        alternative_pattern: "",
    }
}

// Go: renamedSameBehaviour
#[must_use]
pub const fn renamed_same_behaviour(new_name: &'static str) -> Migration {
    Migration {
        tag: MigrationTag::RenamedSameBehaviour,
        new_name,
        options_instructions: "",
        alternative_pattern: "",
    }
}

// Go: renamedAndNeedsOptions
#[must_use]
pub const fn renamed_and_needs_options(
    new_name: &'static str,
    options_instructions: &'static str,
) -> Migration {
    Migration {
        tag: MigrationTag::RenamedAndNeedsOptions,
        new_name,
        options_instructions,
        alternative_pattern: "",
    }
}

// Go: removed
#[must_use]
pub const fn removed(alternative_pattern: &'static str) -> Migration {
    Migration {
        tag: MigrationTag::Removed,
        new_name: "",
        options_instructions: "",
        alternative_pattern,
    }
}

/// Go `EffectModuleMigrationDb`: maps Effect v3 API names to their
/// migration status in v4.
// PORT: Go only looks keys up in this map; it never iterates it. The
// entries are static data in Go's order; the map is made on first use.
pub static EFFECT_MODULE_MIGRATION_DB: LazyLock<FxHashMap<&'static str, Migration>> = LazyLock::new(
    || {
        static ENTRIES: &[(&str, Migration)] = &[
            // Common APIs (in both v3 and v4, unchanged)
            ("acquireRelease", unchanged()),
            ("acquireUseRelease", unchanged()),
            ("addFinalizer", unchanged()),
            ("all", unchanged()),
            ("andThen", unchanged()),
            ("annotateCurrentSpan", unchanged()),
            ("annotateLogs", unchanged()),
            ("annotateSpans", unchanged()),
            ("as", unchanged()),
            ("asSome", unchanged()),
            ("asVoid", unchanged()),
            ("cached", unchanged()),
            ("cachedInvalidateWithTTL", unchanged()),
            ("cachedWithTTL", unchanged()),
            ("catch", unchanged()),
            ("catchIf", unchanged()),
            ("catchTag", unchanged()),
            ("catchTags", unchanged()),
            ("clockWith", unchanged()),
            ("currentParentSpan", unchanged()),
            ("currentSpan", unchanged()),
            ("delay", unchanged()),
            ("die", unchanged()),
            ("ensuring", unchanged()),
            ("eventually", unchanged()),
            ("exit", unchanged()),
            ("fail", unchanged()),
            ("failCause", unchanged()),
            ("failCauseSync", unchanged()),
            ("failSync", unchanged()),
            ("fiberId", unchanged()),
            ("filter", unchanged()),
            ("filterMap", unchanged()),
            ("filterOrElse", unchanged()),
            ("filterOrFail", unchanged()),
            ("flatMap", unchanged()),
            ("flatten", unchanged()),
            ("flip", unchanged()),
            ("fn", unchanged()),
            ("fnUntraced", unchanged()),
            ("forEach", unchanged()),
            ("forever", unchanged()),
            ("forkIn", unchanged()),
            ("forkScoped", unchanged()),
            ("gen", unchanged()),
            ("ignore", unchanged()),
            ("interrupt", unchanged()),
            ("interruptible", unchanged()),
            ("interruptibleMask", unchanged()),
            ("isEffect", unchanged()),
            ("isFailure", unchanged()),
            ("isSuccess", unchanged()),
            ("linkSpans", unchanged()),
            ("log", unchanged()),
            ("logDebug", unchanged()),
            ("logError", unchanged()),
            ("logFatal", unchanged()),
            ("logInfo", unchanged()),
            ("logTrace", unchanged()),
            ("logWarning", unchanged()),
            ("logWithLevel", unchanged()),
            (
                "makeLatch",
                removed("Use Latch.make instead of Effect.makeLatch."),
            ),
            (
                "makeSemaphore",
                removed("Use Semaphore.make instead of Effect.makeSemaphore."),
            ),
            ("makeSpan", unchanged()),
            ("makeSpanScoped", unchanged()),
            ("map", unchanged()),
            ("mapBoth", unchanged()),
            ("mapError", unchanged()),
            ("match", unchanged()),
            ("matchCause", unchanged()),
            ("matchCauseEffect", unchanged()),
            ("matchEffect", unchanged()),
            ("never", unchanged()),
            ("onError", unchanged()),
            ("onExit", unchanged()),
            ("onInterrupt", unchanged()),
            ("option", unchanged()),
            ("orDie", unchanged()),
            ("orElseSucceed", unchanged()),
            ("promise", unchanged()),
            ("provide", unchanged()),
            ("provideService", unchanged()),
            ("provideServiceEffect", unchanged()),
            ("race", unchanged()),
            ("raceAll", unchanged()),
            ("raceFirst", unchanged()),
            ("repeat", unchanged()),
            ("repeatOrElse", unchanged()),
            ("replicate", unchanged()),
            ("replicateEffect", unchanged()),
            ("request", unchanged()),
            ("retry", unchanged()),
            ("retryOrElse", unchanged()),
            ("runCallback", unchanged()),
            ("runFork", unchanged()),
            ("runPromise", unchanged()),
            ("runPromiseExit", unchanged()),
            ("runSync", unchanged()),
            ("runSyncExit", unchanged()),
            ("sandbox", unchanged()),
            ("schedule", unchanged()),
            ("scheduleFrom", unchanged()),
            ("scope", unchanged()),
            ("scoped", unchanged()),
            ("scopedWith", unchanged()),
            ("serviceOption", unchanged()),
            ("sleep", unchanged()),
            ("spanAnnotations", unchanged()),
            ("spanLinks", unchanged()),
            ("succeed", unchanged()),
            ("succeedNone", unchanged()),
            ("succeedSome", unchanged()),
            ("suspend", unchanged()),
            ("sync", unchanged()),
            ("tap", unchanged()),
            ("tapDefect", unchanged()),
            ("tapError", unchanged()),
            ("tapErrorTag", unchanged()),
            ("timed", unchanged()),
            ("timeout", unchanged()),
            ("timeoutOption", unchanged()),
            ("tracer", unchanged()),
            ("try", unchanged()),
            ("tryPromise", unchanged()),
            ("uninterruptible", unchanged()),
            ("uninterruptibleMask", unchanged()),
            ("updateService", unchanged()),
            ("useSpan", unchanged()),
            ("void", unchanged()),
            ("when", unchanged()),
            ("whileLoop", unchanged()),
            ("withConcurrency", unchanged()),
            ("withExecutionPlan", unchanged()),
            ("withLogSpan", unchanged()),
            ("withParentSpan", unchanged()),
            ("withSpan", unchanged()),
            ("withSpanScoped", unchanged()),
            ("withTracer", unchanged()),
            ("withTracerEnabled", unchanged()),
            ("withTracerTiming", unchanged()),
            ("yieldNow", unchanged()),
            ("zip", unchanged()),
            ("zipWith", unchanged()),
            // Renamed APIs (v3 name → v4 name)
            ("catchAll", renamed_same_behaviour("catch")),
            ("catchAllCause", renamed_same_behaviour("catchCause")),
            ("catchAllDefect", renamed_same_behaviour("catchDefect")),
            (
                "catchSome",
                removed(
                    "Use Effect.catchIf instead. Note: the API shape changed from returning Option<Effect> to taking a predicate and handler separately.",
                ),
            ),
            (
                "catchSomeCause",
                removed(
                    "Use Effect.catchCauseIf instead. Note: the API shape changed from returning Option<Effect> to taking a predicate and handler separately.",
                ),
            ),
            (
                "ensureErrorType",
                renamed_same_behaviour("satisfiesErrorType"),
            ),
            (
                "ensureRequirementsType",
                renamed_same_behaviour("satisfiesServicesType"),
            ),
            (
                "ensureSuccessType",
                renamed_same_behaviour("satisfiesSuccessType"),
            ),
            ("fork", renamed_same_behaviour("forkChild")),
            ("forkDaemon", renamed_same_behaviour("forkDetach")),
            ("scopeWith", renamed_same_behaviour("scopedWith")),
            ("serviceOptional", renamed_same_behaviour("serviceOption")),
            ("tapErrorCause", renamed_same_behaviour("tapCause")),
            // APIs that are unchanged between v3 and v4 (post-fix from commit 0154667)
            ("annotateLogsScoped", unchanged()),
            ("awaitAllChildren", unchanged()),
            ("bind", unchanged()),
            ("bindTo", unchanged()),
            ("Do", unchanged()),
            ("let", unchanged()),
            ("partition", unchanged()),
            ("validate", unchanged()),
            // Removed APIs
            (
                "catchSomeDefect",
                removed("Use Effect.catchDefect or Effect.matchCause to handle specific defects."),
            ),
            (
                "forkAll",
                removed(
                    "Fork effects individually with Effect.forEach and Effect.forkChild, or use Effect.all with concurrency options.",
                ),
            ),
            (
                "forkWithErrorHandler",
                removed(
                    "Fork the effect with Effect.forkChild and observe the fiber result via Fiber.join or Fiber.await.",
                ),
            ),
            ("Tag", removed("Use Context.Service instead of Effect.Tag.")),
            (
                "Service",
                removed("Use Context.Service instead of Effect.Service."),
            ),
            (
                "runtime",
                removed(
                    "Runtime has been removed in Effect v4. Use Effect.context to grab services and then run using Effect.runPromiseWith.",
                ),
            ),
            (
                "bindAll",
                removed("Use Effect.gen instead of Effect.bindAll."),
            ),
            (
                "EffectTypeId",
                removed("EffectTypeId has been removed in Effect v4."),
            ),
            (
                "acquireReleaseInterruptible",
                removed("Use Effect.acquireRelease instead."),
            ),
            (
                "allSuccesses",
                removed("Use Effect.all with the { mode: 'either' } option and filter successes."),
            ),
            ("allWith", removed("Use Effect.all with options instead.")),
            ("allowInterrupt", removed("Use Effect.yieldNow instead.")),
            (
                "ap",
                removed("Use Effect.map and Effect.flatMap to apply functions instead."),
            ),
            (
                "asSomeError",
                removed("Use Effect.mapError(Option.some) instead."),
            ),
            (
                "async",
                renamed_and_needs_options(
                    "callback",
                    "Note: in v4 the callback receives a Scheduler as 'this' context.",
                ),
            ),
            (
                "asyncEffect",
                removed("Use Effect.suspend combined with Effect.promise instead."),
            ),
            (
                "blocked",
                removed("The request batching API has been reworked in Effect v4."),
            ),
            (
                "cacheRequestResult",
                removed("The request batching API has been reworked in Effect v4."),
            ),
            (
                "cachedFunction",
                removed("Use Effect.cached or implement caching with a Ref instead."),
            ),
            (
                "cause",
                removed("Use Effect.matchCause or Effect.sandbox to access the cause."),
            ),
            (
                "checkInterruptible",
                removed("Interruption checking has been removed in Effect v4."),
            ),
            ("clock", removed("Use Effect.clockWith instead.")),
            (
                "configProviderWith",
                removed("ConfigProvider access has been reworked in Effect v4."),
            ),
            (
                "console",
                removed("Use Effect.consoleWith or the Console service directly."),
            ),
            (
                "consoleWith",
                removed("Console access has been reworked in Effect v4."),
            ),
            ("context", unchanged()),
            ("contextWith", unchanged()),
            (
                "contextWithEffect",
                removed("Use Effect.flatMap with service access instead."),
            ),
            (
                "custom",
                removed("Use Effect.suspend or Effect.sync to create custom effects."),
            ),
            (
                "daemonChildren",
                removed("Use Effect.forkDetach to fork detached fibers instead."),
            ),
            (
                "descriptor",
                removed("Fiber descriptor access has been removed in Effect v4."),
            ),
            (
                "descriptorWith",
                removed("Fiber descriptor access has been removed in Effect v4."),
            ),
            (
                "dieMessage",
                removed("Use Effect.die(new Error(message)) instead."),
            ),
            (
                "dieSync",
                removed("Use Effect.die with a lazily evaluated value instead."),
            ),
            (
                "diffFiberRefs",
                removed("FiberRef has been replaced by Context.Reference in Effect v4."),
            ),
            (
                "disconnect",
                removed("Fiber disconnect has been removed in Effect v4."),
            ),
            (
                "dropUntil",
                removed("Use Array.dropUntil and Effect.forEach instead."),
            ),
            (
                "dropWhile",
                removed("Use Array.dropWhile and Effect.forEach instead."),
            ),
            (
                "either",
                renamed_and_needs_options(
                    "result",
                    "Note: returns Result.Result<A, E> instead of Either<E, A>. Use the Result module to work with the result.",
                ),
            ),
            (
                "ensuringChild",
                removed("Use Effect.onExit to manage child fiber cleanup instead."),
            ),
            (
                "ensuringChildren",
                removed("Use Effect.onExit to manage child fiber cleanup instead."),
            ),
            (
                "every",
                removed("Use Effect.forEach with a predicate check instead."),
            ),
            (
                "exists",
                removed("Use Effect.forEach with a predicate check instead."),
            ),
            ("fiberIdWith", removed("Use Effect.fiberId instead.")),
            (
                "filterEffectOrElse",
                removed("Use Effect.filterOrElse with an effectful predicate instead."),
            ),
            (
                "filterEffectOrFail",
                removed("Use Effect.filterOrFail with an effectful predicate instead."),
            ),
            (
                "filterOrDie",
                removed("Use Effect.filterOrFail and Effect.orDie instead."),
            ),
            (
                "filterOrDieMessage",
                removed("Use Effect.filterOrFail and Effect.orDie instead."),
            ),
            (
                "finalizersMask",
                removed("Finalizer masking has been removed in Effect v4."),
            ),
            ("findFirst", unchanged()),
            ("firstSuccessOf", unchanged()),
            (
                "flipWith",
                removed("Use Effect.flip combined with the desired transformation instead."),
            ),
            ("fromFiber", removed("Use Fiber.join instead.")),
            (
                "fromFiberEffect",
                removed("Use Effect.flatMap with Fiber.join instead."),
            ),
            (
                "fromNullable",
                removed("Use Effect.suspend with null checks instead."),
            ),
            ("functionWithSpan", removed("Use Effect.withSpan instead.")),
            (
                "getFiberRefs",
                removed("FiberRef has been replaced by Context.Reference in Effect v4."),
            ),
            (
                "getRuntimeFlags",
                removed("Runtime flags have been removed in Effect v4."),
            ),
            (
                "head",
                removed("Use Array.head and Effect.flatMap instead."),
            ),
            ("if", removed("Use Effect.when instead of Effect.if.")),
            (
                "ignoreLogged",
                removed("Logging configuration has been reworked in Effect v4."),
            ),
            (
                "inheritFiberRefs",
                removed("FiberRef has been replaced by Context.Reference in Effect v4."),
            ),
            ("interruptWith", removed("Use Effect.interrupt instead.")),
            ("intoDeferred", removed("Use Deferred.complete instead.")),
            (
                "iterate",
                removed("Use Effect.whileLoop or recursion with Effect.suspend instead."),
            ),
            (
                "labelMetrics",
                removed("Metric labeling has been reworked in Effect v4."),
            ),
            (
                "labelMetricsScoped",
                removed("Metric labeling has been reworked in Effect v4."),
            ),
            ("liftPredicate", removed("Use Effect.filterOrFail instead.")),
            ("linkSpanCurrent", removed("Use Effect.linkSpans instead.")),
            (
                "locally",
                removed(
                    "FiberRef.locally has been removed. Use Effect.provideService with Context.Reference instead.",
                ),
            ),
            (
                "locallyScoped",
                removed(
                    "FiberRef.locally has been removed. Use Effect.provideService with Context.Reference instead.",
                ),
            ),
            (
                "locallyScopedWith",
                removed(
                    "FiberRef.locally has been removed. Use Effect.provideService with Context.Reference instead.",
                ),
            ),
            (
                "locallyWith",
                removed(
                    "FiberRef.locally has been removed. Use Effect.provideService with Context.Reference instead.",
                ),
            ),
            (
                "logAnnotations",
                removed("Use Effect.annotateLogs instead."),
            ),
            (
                "loop",
                removed("Use Effect.whileLoop or recursion with Effect.suspend instead."),
            ),
            (
                "mapAccum",
                removed("Use Effect.gen with a mutable accumulator instead."),
            ),
            (
                "mapErrorCause",
                removed("Use Effect.sandbox and Effect.mapError instead."),
            ),
            (
                "mapInputContext",
                removed("Use Effect.provide with a layer instead."),
            ),
            ("merge", removed("Use Effect.match or Effect.exit instead.")),
            (
                "mergeAll",
                removed("Use Effect.forEach with a mutable accumulator instead."),
            ),
            (
                "metricLabels",
                removed("Metric labeling has been reworked in Effect v4."),
            ),
            (
                "negate",
                removed("Use Effect.map with boolean negation instead."),
            ),
            (
                "none",
                removed("Use Effect.filterOrFail or Effect.option instead."),
            ),
            ("once", removed("Use Effect.cached instead.")),
            ("optionFromOptional", removed("Use Effect.option instead.")),
            (
                "orDieWith",
                removed("Use Effect.orDie or Effect.mapError combined with Effect.orDie instead."),
            ),
            (
                "orElse",
                removed("Use Effect.catch or Effect.matchEffect instead."),
            ),
            ("orElseFail", removed("Use Effect.mapError instead.")),
            (
                "parallelErrors",
                removed("Use Effect.all with concurrency options instead."),
            ),
            (
                "parallelFinalizers",
                removed("Finalizer ordering configuration has been removed in Effect v4."),
            ),
            (
                "patchFiberRefs",
                removed("FiberRef has been replaced by Context.Reference in Effect v4."),
            ),
            (
                "patchRuntimeFlags",
                removed("Runtime flags have been removed in Effect v4."),
            ),
            (
                "raceWith",
                removed("Use Effect.race or Effect.raceFirst instead."),
            ),
            ("random", removed("Use Effect.randomWith instead.")),
            (
                "randomWith",
                removed("Random access has been reworked in Effect v4."),
            ),
            (
                "reduce",
                removed("Use Effect.forEach with a mutable accumulator or Effect.gen instead."),
            ),
            (
                "reduceEffect",
                removed("Use Effect.forEach with a mutable accumulator or Effect.gen instead."),
            ),
            (
                "reduceRight",
                removed("Use Effect.forEach with a mutable accumulator or Effect.gen instead."),
            ),
            (
                "reduceWhile",
                removed("Use Effect.gen with early return instead."),
            ),
            (
                "repeatN",
                removed("Use Effect.repeat with a schedule instead."),
            ),
            (
                "runRequestBlock",
                removed("The request batching API has been reworked in Effect v4."),
            ),
            (
                "scheduleForked",
                removed("Use Effect.schedule combined with Effect.forkChild instead."),
            ),
            (
                "sequentialFinalizers",
                removed("Finalizer ordering configuration has been removed in Effect v4."),
            ),
            (
                "serviceConstants",
                removed(
                    "Service helpers have been removed. Use Context.Service and yield* to access services.",
                ),
            ),
            (
                "serviceFunction",
                removed(
                    "Service helpers have been removed. Use Context.Service and yield* to access services.",
                ),
            ),
            (
                "serviceFunctionEffect",
                removed(
                    "Service helpers have been removed. Use Context.Service and yield* to access services.",
                ),
            ),
            (
                "serviceFunctions",
                removed(
                    "Service helpers have been removed. Use Context.Service and yield* to access services.",
                ),
            ),
            (
                "serviceMembers",
                removed(
                    "Service helpers have been removed. Use Context.Service and yield* to access services.",
                ),
            ),
            (
                "setFiberRefs",
                removed("FiberRef has been replaced by Context.Reference in Effect v4."),
            ),
            (
                "step",
                removed("The request batching API has been reworked in Effect v4."),
            ),
            (
                "summarized",
                removed("Use Effect.gen to capture before/after state instead."),
            ),
            (
                "supervised",
                removed("Supervision has been reworked in Effect v4."),
            ),
            (
                "tagMetrics",
                removed("Metric labeling has been reworked in Effect v4."),
            ),
            (
                "tagMetricsScoped",
                removed("Metric labeling has been reworked in Effect v4."),
            ),
            (
                "takeUntil",
                removed("Use Array.takeUntil and Effect.forEach instead."),
            ),
            (
                "takeWhile",
                removed("Use Array.takeWhile and Effect.forEach instead."),
            ),
            (
                "tapBoth",
                removed("Use Effect.tap and Effect.tapError instead."),
            ),
            ("timedWith", removed("Use Effect.timed instead.")),
            (
                "timeoutFail",
                removed(
                    "Use Effect.timeout combined with Effect.catchTag for TimeoutException instead.",
                ),
            ),
            (
                "timeoutFailCause",
                removed("Use Effect.timeout combined with Effect.sandbox instead."),
            ),
            (
                "timeoutTo",
                removed("Use Effect.timeoutOption and Effect.map instead."),
            ),
            ("tracerWith", removed("Use Effect.tracer instead.")),
            (
                "transplant",
                removed("Fiber transplanting has been removed in Effect v4."),
            ),
            (
                "transposeMapOption",
                removed("Use Effect.map with Option operations instead."),
            ),
            ("transposeOption", removed("Use Effect.option instead.")),
            (
                "tryMap",
                removed("Use Effect.map inside Effect.try instead."),
            ),
            ("tryMapPromise", removed("Use Effect.tryPromise instead.")),
            (
                "unless",
                removed("Use Effect.when with a negated condition instead."),
            ),
            (
                "unlessEffect",
                removed("Use Effect.when with a negated effectful condition instead."),
            ),
            ("unsafeMakeLatch", removed("Use Effect.makeLatch instead.")),
            (
                "unsafeMakeSemaphore",
                removed("Use Effect.makeSemaphore instead."),
            ),
            ("unsandbox", removed("Use Effect.catchCause instead.")),
            (
                "updateFiberRefs",
                removed("FiberRef has been replaced by Context.Reference in Effect v4."),
            ),
            ("using", removed("Use Effect.scoped instead.")),
            (
                "validateAll",
                removed("Use Effect.all with { mode: 'validate' } instead."),
            ),
            (
                "validateFirst",
                removed("Use Effect.all with { mode: 'validate' } instead."),
            ),
            (
                "validateWith",
                removed("Use Effect.all with { mode: 'validate' } instead."),
            ),
            (
                "whenEffect",
                removed("Use Effect.when with an effectful condition via Effect.flatMap instead."),
            ),
            (
                "whenFiberRef",
                removed("FiberRef has been replaced by Context.Reference in Effect v4."),
            ),
            (
                "whenLogLevel",
                removed("Log level checking has been reworked in Effect v4."),
            ),
            ("whenRef", removed("Use Ref.get and Effect.when instead.")),
            (
                "withClock",
                removed("Clock configuration has been reworked in Effect v4."),
            ),
            (
                "withClockScoped",
                removed("Clock configuration has been reworked in Effect v4."),
            ),
            (
                "withConfigProvider",
                removed("ConfigProvider configuration has been reworked in Effect v4."),
            ),
            (
                "withConfigProviderScoped",
                removed("ConfigProvider configuration has been reworked in Effect v4."),
            ),
            (
                "withConsole",
                removed("Console configuration has been reworked in Effect v4."),
            ),
            (
                "withConsoleScoped",
                removed("Console configuration has been reworked in Effect v4."),
            ),
            (
                "withEarlyRelease",
                removed("Use Effect.scoped with manual resource management instead."),
            ),
            (
                "withFiberRuntime",
                renamed_and_needs_options(
                    "withFiber",
                    "Note: in v4, only the Fiber is provided (not the full FiberRuntime with status). The callback receives (fiber: Fiber<unknown, unknown>) instead of (fiber: FiberRuntime<A, E>, status: FiberStatus).",
                ),
            ),
            (
                "withMaxOpsBeforeYield",
                removed("Use Context.Reference for MaxOpsBeforeYield configuration instead."),
            ),
            (
                "withMetric",
                removed("Metric configuration has been reworked in Effect v4."),
            ),
            (
                "withRandom",
                removed("Random configuration has been reworked in Effect v4."),
            ),
            (
                "withRandomFixed",
                removed("Random configuration has been reworked in Effect v4."),
            ),
            (
                "withRandomScoped",
                removed("Random configuration has been reworked in Effect v4."),
            ),
            (
                "withRequestBatching",
                removed("Request batching configuration has been reworked in Effect v4."),
            ),
            (
                "withRequestCache",
                removed("Request caching configuration has been reworked in Effect v4."),
            ),
            (
                "withRequestCaching",
                removed("Request caching configuration has been reworked in Effect v4."),
            ),
            (
                "withRuntimeFlagsPatch",
                removed("Runtime flags have been removed in Effect v4."),
            ),
            (
                "withRuntimeFlagsPatchScoped",
                removed("Runtime flags have been removed in Effect v4."),
            ),
            (
                "withScheduler",
                removed("Use Context.Reference for Scheduler configuration instead."),
            ),
            (
                "withSchedulingPriority",
                removed("Scheduling priority configuration has been removed in Effect v4."),
            ),
            (
                "withTracerScoped",
                removed("Use Effect.withTracer instead."),
            ),
            (
                "withUnhandledErrorLogLevel",
                removed("Use Context.Reference for UnhandledLogLevel configuration instead."),
            ),
            (
                "zipLeft",
                removed("Use Effect.tap instead of Effect.zipLeft."),
            ),
            (
                "zipRight",
                removed("Use Effect.andThen instead of Effect.zipRight."),
            ),
        ];
        ENTRIES.iter().copied().collect()
    },
);
