//! Port of Go `internal/project/checkerpool_test.go`.
//!
//! PORT: the Rust pool lives on one thread (see `project::checkerpool`):
//! a second request for a full slot is `unreachable!` there, so the
//! blocked goroutines of the Go contention tests (`QueryContention`,
//! `DiagnosticsContention`, `CrossReleaseAffinityWithContention`, the 4th
//! request of `MultipleConcurrentQueryCheckers`) have no port; those tests
//! check the rest. Go runs the timer tests in `synctest` bubbles with fake
//! time; the idle-cleanup timer here is a `gostd::local` timer that waits
//! in real time and fires in `run_pending`, so the timer tests
//! (`IdleCleanup`, `FileAssociationCleanup`, `StaggeredIdleCleanup`,
//! `DiagnosticsRecreatedAfterIdleDisposal`) use a short real timeout (see
//! `IDLE` at the end of the file), and the long fake sleeps of the other
//! tests are left out. `DoubleReleaseSafe` can not call a release twice:
//! `Release::call` takes `self`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ts_goport::checker::Checker;
use ts_goport::frontend::bundled;
use ts_goport::frontend::core_context::{self, CheckerLifetime};
use ts_goport::gostd::{Context, context};
use ts_goport::lsp::lsproto;
use ts_goport::program::ls_program::CheckerPool as _;
use ts_goport::project::{
    self, CheckerPool, CheckerPoolOptions, Session, logging, new_checker_pool,
};

use super::projecttestutil::{self, files};
use super::util::*;

type CheckerRc = Rc<RefCell<Checker>>;

fn opts(max_checkers: i32, idle_secs: u64) -> CheckerPoolOptions {
    CheckerPoolOptions {
        max_checkers,
        idle_timeout: Duration::from_secs(idle_secs),
    }
}

// Go: checkerpool_test.go:20 setupCheckerPoolSession
fn setup_checker_pool_session(opts: CheckerPoolOptions) -> (Rc<Session>, Rc<CheckerPool>) {
    let (_, fs) = projecttestutil::wrapped_map_fs(
        files(&[
            (
                "/src/tsconfig.json",
                r#"{ "compilerOptions": { "noLib": true } }"#,
            ),
            ("/src/index.ts", "export const x: number = 1;"),
        ]),
        false,
    );
    let logger: Rc<dyn logging::Logger> = logging::new_test_logger();
    let session = project::new_session(&project::SessionInit {
        background_ctx: bg(),
        options: Rc::new(project::SessionOptions {
            current_directory: "/".to_string(),
            default_library_path: bundled::lib_path(),
            typings_location: String::new(),
            position_encoding: lsproto::PositionEncodingKind::UTF8,
            watch_enabled: false,
            logging_enabled: true,
            checker_pool_options: opts,
            ..projecttestutil::session_options("/")
        }),
        fs,
        client: None,
        logger: Some(logger),
        npm_executor: None,
        spawner: None,
        content_mapper_logger: None,
        parse_cache: None,
        content_mapped_parse_cache: None,
    });
    open(
        &session,
        "file:///src/index.ts",
        "export const x: number = 1;",
    );

    let project =
        configured_project(&session, "/src/tsconfig.json").expect("expected configured project");
    let pool = project
        .borrow()
        .checker_pool
        .clone()
        .expect("expected checker pool");
    (session, pool)
}

// Go: checkerpool_test.go:55 newTestCheckerPool
fn new_test_checker_pool(
    program: &Rc<ts_goport::frontend::compiler::NewProgram>,
    opts: CheckerPoolOptions,
) -> Rc<CheckerPool> {
    new_checker_pool(opts, Rc::clone(program), Some(Rc::new(|_: &str| {})))
}

/// The session's program and a fresh test pool on it (the start of most Go tests).
fn test_pool(
    session_opts: CheckerPoolOptions,
    pool_opts: CheckerPoolOptions,
) -> (Rc<Session>, Rc<CheckerPool>) {
    let (session, _) = setup_checker_pool_session(session_opts);
    let p = program(&session, "file:///src/index.ts");
    let pool = new_test_checker_pool(&p, pool_opts);
    (session, pool)
}

/// Go `core.WithCheckerLifetime(core.WithRequestID(parent, id), lifetime)`.
fn req(parent: &Context, id: &str, lifetime: CheckerLifetime) -> Context {
    core_context::with_checker_lifetime(&core_context::with_request_id(parent, id), lifetime)
}

fn checker_at(pool: &CheckerPool, index: usize) -> Option<CheckerRc> {
    pool.checkers.borrow()[index].clone()
}

fn same(a: &CheckerRc, b: &CheckerRc) -> bool {
    Rc::ptr_eq(a, b)
}

/// The query slot (1+) that holds `c`, or 0.
fn query_index(pool: &CheckerPool, c: &CheckerRc) -> usize {
    let checkers = pool.checkers.borrow();
    (1..checkers.len())
        .find(|&i| checkers[i].as_ref().is_some_and(|x| same(x, c)))
        .unwrap_or(0)
}

const NIL: ts_goport::core::Node = ts_goport::core::Node::NIL;

child_test! {
    // Go: checkerpool_test.go:59 TestCheckerPoolDiagnosticsRouting
    fn diagnostics_routing() {
        let (_session, pool) = setup_checker_pool_session(opts(4, 10));

        // Diagnostics requests should get checker at index 0.
        let ctx = req(&bg(), "diag-req-1", CheckerLifetime::DIAGNOSTICS);
        let (c, release) = pool.get_checker(&ctx, NIL);
        assert!(checker_at(&pool, 0).is_some_and(|x| same(&x, &c)), "diagnostics should use checker index 0");
        release.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:72 TestCheckerPoolQueryRouting
    fn query_routing() {
        let (_session, pool) = setup_checker_pool_session(opts(4, 10));

        // Query requests should get a checker at index > 0.
        let ctx = req(&bg(), "query-req-1", CheckerLifetime::TEMPORARY);
        let (c, release) = pool.get_checker(&ctx, NIL);

        // Verify it's not the diagnostics checker slot.
        assert!(
            !checker_at(&pool, 0).is_some_and(|x| same(&x, &c)),
            "query should not use checker index 0"
        );
        release.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:88 TestCheckerPoolRequestAffinity (ts#64543)
    fn request_affinity() {
        let (_session, pool) = setup_checker_pool_session(opts(4, 10));

        let (req_ctx, cancel) = context::with_cancel(&bg());
        let ctx = req(&req_ctx, "req-affinity", CheckerLifetime::TEMPORARY);

        // First call acquires.
        let (c1, release1) = pool.get_checker(&ctx, NIL);

        release1.call();

        // After release, same request should still get the same checker (cross-release affinity).
        let (c2, release2) = pool.get_checker(&ctx, NIL);
        release2.call();

        assert!(same(&c1, &c2), "same request ID should return the same checker after release");
        cancel();
    }
}

child_test! {
    // Go: checkerpool_test.go:109 TestCheckerPoolSameRequestContention (ts#64543)
    // PORT: one thread. Go's second acquisition blocks in a goroutine until
    // the first is released; here it is `unreachable!` on the full
    // semaphore (the persistent one for "api"), so the test checks that it
    // does not return the held checker, then that the acquisition after the
    // release gets the same checker. Go runs the subtests in parallel.
    fn same_request_contention() {
        let (session, _) = setup_checker_pool_session(opts(2, 10));
        let p = program(&session, "file:///src/index.ts");
        for (name, lifetime) in [
            ("diagnostics", CheckerLifetime::DIAGNOSTICS),
            ("query", CheckerLifetime::TEMPORARY),
            ("api", CheckerLifetime::API),
        ] {
            let pool = new_test_checker_pool(&p, opts(2, 0));
            let (req_ctx, cancel) = context::with_cancel(&bg());
            let ctx = req(&req_ctx, "same-request", lifetime);

            let (c1, release1) = pool.get_checker(&ctx, NIL);
            let blocked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pool.get_checker(&ctx, NIL)
            }));
            assert!(
                blocked.is_err(),
                "{name}: the request ID must not bypass exclusive acquisition"
            );
            release1.call();
            let (c2, release2) = pool.get_checker(&ctx, NIL);
            assert!(same(&c1, &c2), "{name}");
            release2.call();
            cancel();
        }
    }
}

child_test! {
    // Go: checkerpool_test.go:151 TestCheckerPoolSameRequestConcurrentQueries (ts#64543)
    // PORT: the semaphore is private; Go's `len(pool.querySem) == 2` is
    // checked as two held query slots.
    fn same_request_concurrent_queries() {
        let (_session, pool) = setup_checker_pool_session(opts(3, 10));
        let (req_ctx, cancel) = context::with_cancel(&bg());
        let ctx = req(&req_ctx, "same-request", CheckerLifetime::TEMPORARY);

        let (c1, release1) = pool.get_checker(&ctx, NIL);
        let (c2, release2) = pool.get_checker(&ctx, NIL);
        assert!(!same(&c1, &c2), "overlapping acquisitions must use different checkers");
        let held = pool.held_by.borrow().iter().skip(1).filter(|h| !h.is_empty()).count();
        assert_eq!(held, 2, "each acquisition must hold its own slot");
        release2.call();
        release1.call();
        cancel();
    }
}

child_test! {
    // Go: checkerpool_test.go:204 TestCheckerPoolMinCheckers
    fn min_checkers() {
        // Requesting maxCheckers=1 should be clamped to 2.
        let (_session, pool) = setup_checker_pool_session(opts(1, 10));
        assert_eq!(pool.opts.max_checkers, 2);
        assert_eq!(pool.checkers.borrow().len(), 2);
    }
}

child_test! {
    // Go: checkerpool_test.go:212 TestCheckerPoolDefaultIdleTimeout
    fn default_idle_timeout() {
        // Zero idle timeout should default to 30s.
        let (_session, pool) = setup_checker_pool_session(opts(4, 0));
        assert_eq!(pool.opts.idle_timeout, Duration::from_secs(30));
    }
}

child_test! {
    // Go: checkerpool_test.go:303 TestCheckerPoolCanceledCheckerDisposal
    fn canceled_checker_disposal() {
        let (_session, pool) = test_pool(opts(2, 10), opts(4, 30));
        let source_file = pool.program.get_source_file("/src/index.ts").expect("source file").root;
        let _guard = ts_goport::program::ls_program::enter(&pool.program);

        // Acquire a query checker and cancel it.
        let ctx = req(&bg(), "cancel-test", CheckerLifetime::TEMPORARY);
        let (c, release) = pool.get_checker(&ctx, NIL);

        let (canceled_ctx, cancel) = context::with_cancel(&bg());
        cancel();
        c.borrow_mut().get_diagnostics_exported(&canceled_ctx, source_file);
        assert!(c.borrow().was_canceled());

        // Release should dispose the canceled checker.
        release.call();

        // Next request should get a fresh checker.
        let ctx2 = req(&bg(), "after-cancel", CheckerLifetime::TEMPORARY);
        let (c2, release2) = pool.get_checker(&ctx2, NIL);
        assert!(!same(&c2, &c), "should get a new checker, not the canceled one");
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:339 TestCheckerPoolRequestAssociationCleanupOnDisposal
    fn request_association_cleanup_on_disposal() {
        let (_session, pool) = test_pool(opts(2, 10), opts(4, 5));
        let _guard = ts_goport::program::ls_program::enter(&pool.program);

        // Create a query checker with a request association.
        let (req_ctx, req_cancel) = context::with_cancel(&bg());
        let ctx = req(&req_ctx, "assoc-cleanup-req", CheckerLifetime::TEMPORARY);
        let (c, release) = pool.get_checker(&ctx, NIL);

        // Cancel the checker to trigger disposal on release.
        let (canceled_ctx, cancel) = context::with_cancel(&bg());
        cancel();
        let source_file = pool.program.get_source_file("/src/index.ts").expect("source file").root;
        c.borrow_mut().get_diagnostics_exported(&canceled_ctx, source_file);
        assert!(c.borrow().was_canceled());

        release.call();

        // Request association should be cleared after checker disposal.
        assert!(
            !pool.request_associations.borrow().contains_key("assoc-cleanup-req"),
            "request association should be cleared after checker disposal"
        );
        // Go: defer reqCancel()
        req_cancel();
    }
}

child_test! {
    // Go: checkerpool_test.go:375 TestCheckerPoolRequestAssociationCleanupOnContextDone
    // PORT: Go waits for the AfterFunc goroutine with `synctest.Wait()`. The
    // port runs the delete at the next `mu_lock` and starts no thread. The
    // live second request is port-only.
    fn request_association_cleanup_on_context_done() {
        let (_session, pool) = test_pool(opts(2, 10), opts(4, 30));
        let threads_before = context::AFTER_FUNC_GOROUTINES.load(Ordering::Relaxed);

        // Create a cancellable context to simulate request lifecycle.
        let (req_ctx, req_cancel) = context::with_cancel(&bg());
        let ctx = req(&req_ctx, "ctx-cleanup-req", CheckerLifetime::TEMPORARY);
        let (live_ctx, live_cancel) = context::with_cancel(&bg());
        let live = req(&live_ctx, "live-req", CheckerLifetime::TEMPORARY);

        let (_c, release) = pool.get_checker(&ctx, NIL);
        release.call();
        let (live_c, live_release) = pool.get_checker(&live, NIL);
        live_release.call();

        // Association should still exist after release.
        pool.mu_lock();
        assert!(
            pool.request_associations.borrow().contains_key("ctx-cleanup-req"),
            "request association should persist after release"
        );

        // Cancel the request context — association should be cleaned up.
        req_cancel();
        pool.mu_lock();
        assert!(
            !pool.request_associations.borrow().contains_key("ctx-cleanup-req"),
            "request association should be cleaned up after context cancellation"
        );

        // The live request keeps its association and its checker.
        let (live_again, live_release) = pool.get_checker(&live, NIL);
        assert!(same(&live_again, &live_c), "a live request should keep its checker");
        live_release.call();
        live_cancel();
        pool.mu_lock();
        assert!(pool.request_associations.borrow().is_empty());

        assert_eq!(
            context::AFTER_FUNC_GOROUTINES.load(Ordering::Relaxed),
            threads_before,
            "request cleanup should start no thread"
        );
    }
}

child_test! {
    // Go: checkerpool_test.go:502 TestCheckerPoolLifetimeMismatchIgnoresAssociation
    fn lifetime_mismatch_ignores_association() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        let (req_ctx, req_cancel) = context::with_cancel(&bg());

        // Acquire a diagnostics checker with request ID "mixed".
        let ctx_diag = req(&req_ctx, "mixed", CheckerLifetime::DIAGNOSTICS);
        let (c_diag, release_diag) = pool.get_checker(&ctx_diag, NIL);
        assert!(
            checker_at(&pool, 0).is_some_and(|x| same(&x, &c_diag)),
            "diagnostics checker should be at index 0"
        );
        release_diag.call();

        // Now use the same request ID but with query purpose.
        let ctx_query = req(&req_ctx, "mixed", CheckerLifetime::TEMPORARY);
        let (c_query, release_query) = pool.get_checker(&ctx_query, NIL);
        assert!(!same(&c_query, &c_diag), "query should not reuse the diagnostics checker");

        assert!(
            !checker_at(&pool, 0).is_some_and(|x| same(&x, &c_query)),
            "query checker should not be at diagnostics index 0"
        );
        release_query.call();
        req_cancel();
    }
}

child_test! {
    // Go: checkerpool_test.go:543 TestCheckerPoolNoRequestID
    fn no_request_id() {
        let (_session, pool) = setup_checker_pool_session(opts(4, 10));

        // Calls without a request ID should still work (e.g., callhierarchy uses context.Background()).
        let ctx = bg();

        let (_c1, release1) = pool.get_checker(&ctx, NIL);
        release1.call();

        let (_c2, release2) = pool.get_checker(&ctx, NIL);
        release2.call();

        // Without request ID, no affinity guarantee — just verify it doesn't crash.
    }
}

child_test! {
    // Go: checkerpool_test.go:561 TestCheckerPoolDiagnosticsCrossReleaseAffinity
    fn diagnostics_cross_release_affinity() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        let (req_ctx, req_cancel) = context::with_cancel(&bg());
        let ctx = req(&req_ctx, "diag-affinity", CheckerLifetime::DIAGNOSTICS);

        let (c1, release1) = pool.get_checker(&ctx, NIL);
        assert!(checker_at(&pool, 0).is_some_and(|x| same(&x, &c1)), "should be the diagnostics checker");
        release1.call();

        // Same request reacquiring diagnostics should get the same checker.
        let (c2, release2) = pool.get_checker(&ctx, NIL);
        assert!(same(&c2, &c1), "same diagnostics request should get the same checker after release");
        release2.call();
        req_cancel();
    }
}

child_test! {
    // Go: checkerpool_test.go:589 TestCheckerPoolDiscardKeepsIdleCheckers
    // PORT: the Go 60 s fake sleep at the end is left out (see the module comment).
    fn discard_keeps_idle_checkers() {
        let (_session, pool) = test_pool(opts(2, 10), opts(4, 30));

        // Create both a diagnostics and a query checker.
        let (c1, release1) = pool.get_checker(&req(&bg(), "obs-diag", CheckerLifetime::DIAGNOSTICS), NIL);
        release1.call();

        let (c2, release2) = pool.get_checker(&req(&bg(), "obs-query", CheckerLifetime::TEMPORARY), NIL);
        release2.call();

        // Both checkers should exist before Discard.
        assert!(checker_at(&pool, 0).is_some(), "diagnostics checker should exist");

        // Discard should keep idle checkers alive and just stop the cleanup timer.
        pool.discard();

        assert!(
            checker_at(&pool, 0).is_some_and(|x| same(&x, &c1)),
            "diagnostics checker should survive Discard"
        );
        assert!(query_index(&pool, &c2) > 0, "query checker should survive Discard");
        assert!(pool.cleanup_timer.borrow().is_none(), "cleanup timer should be stopped after Discard");
    }
}

child_test! {
    // Go: checkerpool_test.go:646 TestCheckerPoolDiscardHeldCheckerSurvivesRelease
    // PORT: the Go 60 s fake sleep at the end is left out.
    fn discard_held_checker_survives_release() {
        let (_session, pool) = test_pool(opts(2, 10), opts(4, 30));

        // Acquire a checker and hold it.
        let (c, release) = pool.get_checker(&req(&bg(), "held-obs", CheckerLifetime::TEMPORARY), NIL);

        // Find which slot it's in.
        let held_index = query_index(&pool, &c);
        assert!(held_index > 0, "should find the held checker");

        // Discard while checker is held — should NOT dispose it.
        pool.discard();

        assert!(
            checker_at(&pool, held_index).is_some_and(|x| same(&x, &c)),
            "held checker should survive Discard"
        );

        // Release — checker should remain alive on a discarded pool.
        release.call();

        assert!(
            checker_at(&pool, held_index).is_some_and(|x| same(&x, &c)),
            "checker should persist after release on discarded pool"
        );
    }
}

child_test! {
    // Go: checkerpool_test.go:699 TestCheckerPoolDiscardStillFunctional
    fn discard_still_functional() {
        let (_session, pool) = test_pool(opts(2, 10), opts(4, 30));
        pool.discard();

        // Pool should still work — GetChecker should create a fresh checker.
        let (c, release) = pool.get_checker(&req(&bg(), "post-obs", CheckerLifetime::TEMPORARY), NIL);

        // Find the slot.
        let idx = query_index(&pool, &c);
        assert!(idx > 0, "checker should be in a query slot");

        // Release — checker should persist on discarded pool (no cleanup timer).
        release.call();

        assert!(
            checker_at(&pool, idx).is_some_and(|x| same(&x, &c)),
            "checker should persist after release on discarded pool"
        );

        // Re-acquire — should get the same checker back.
        let (c2, release2) = pool.get_checker(&req(&bg(), "post-obs-2", CheckerLifetime::TEMPORARY), NIL);
        assert!(same(&c2, &c), "should get the same checker on discarded pool");
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:745 TestCheckerPoolDiagnosticsCheckerStableIdentity
    fn diagnostics_checker_stable_identity() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        // Acquire the diagnostics checker.
        let (c1, release1) = pool.get_checker(&req(&bg(), "diag-stable-1", CheckerLifetime::DIAGNOSTICS), NIL);
        release1.call();

        // Re-acquire before idle timeout — should be the same instance.
        let (c2, release2) = pool.get_checker(&req(&bg(), "diag-stable-2", CheckerLifetime::DIAGNOSTICS), NIL);
        assert!(same(&c2, &c1), "diagnostics checker should be the same instance before idle timeout");
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:772 TestCheckerPoolDiagnosticsCheckerSurvivesDiscard
    fn diagnostics_checker_survives_discard() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        // Create the diagnostics checker.
        let (c, release) = pool.get_checker(&req(&bg(), "diag-discard", CheckerLifetime::DIAGNOSTICS), NIL);
        release.call();

        pool.discard();

        // Diagnostics checker should survive Discard.
        assert!(
            checker_at(&pool, 0).is_some_and(|x| same(&x, &c)),
            "diagnostics checker should survive Discard"
        );

        // Should still be acquirable and be the same instance.
        let (c2, release2) = pool.get_checker(&req(&bg(), "diag-discard-2", CheckerLifetime::DIAGNOSTICS), NIL);
        assert!(same(&c2, &c), "diagnostics checker identity should be stable after Discard");
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:806 TestCheckerPoolDiagnosticsCheckerIndependentFromQuery
    fn diagnostics_checker_independent_from_query() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        // Acquire diagnostics and query checkers.
        let (diag_c, diag_release) = pool.get_checker(&req(&bg(), "diag-indep", CheckerLifetime::DIAGNOSTICS), NIL);
        let (query_c, query_release) = pool.get_checker(&req(&bg(), "query-indep", CheckerLifetime::TEMPORARY), NIL);

        // They should be different checker instances.
        assert!(!same(&diag_c, &query_c), "diagnostics and query checkers should be different");

        diag_release.call();
        query_release.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:833 TestCheckerPoolAPICheckerStableIdentity
    // PORT: the Go 60 s fake sleep (and the third acquire after it) is left out.
    fn api_checker_stable_identity() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        let ctx = core_context::with_checker_lifetime(&bg(), CheckerLifetime::API);
        let (c1, release1) = pool.get_checker(&ctx, NIL);
        release1.call();

        let (c2, release2) = pool.get_checker(&ctx, NIL);
        assert!(same(&c2, &c1), "API checker should be the same instance");
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:862 TestCheckerPoolAPICheckerSurvivesDiscard
    fn api_checker_survives_discard() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        let ctx = core_context::with_checker_lifetime(&bg(), CheckerLifetime::API);
        let (c, release) = pool.get_checker(&ctx, NIL);
        release.call();

        pool.discard();

        assert!(
            pool.persistent_checker.borrow().as_ref().is_some_and(|x| same(x, &c)),
            "API checker should survive Discard"
        );

        let (c2, release2) = pool.get_checker(&ctx, NIL);
        assert!(same(&c2, &c), "API checker identity should be stable after Discard");
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:889 TestCheckerPoolAllThreeIndependent
    fn all_three_independent() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        let ded_ctx = req(&bg(), "ded-req", CheckerLifetime::DIAGNOSTICS);
        let tmp_ctx = req(&bg(), "tmp-req", CheckerLifetime::TEMPORARY);
        let per_ctx = core_context::with_checker_lifetime(&bg(), CheckerLifetime::API);

        let (ded_c, ded_release) = pool.get_checker(&ded_ctx, NIL);
        let (tmp_c, tmp_release) = pool.get_checker(&tmp_ctx, NIL);
        let (per_c, per_release) = pool.get_checker(&per_ctx, NIL);

        assert!(!same(&ded_c, &tmp_c), "diagnostics and temporary should be different");
        assert!(!same(&ded_c, &per_c), "diagnostics and API should be different");
        assert!(!same(&tmp_c, &per_c), "temporary and API should be different");

        ded_release.call();
        tmp_release.call();
        per_release.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:919 TestCheckerPoolFileAffinity
    fn file_affinity() {
        let (session, pool) = test_pool(opts(4, 10), opts(4, 30));
        let source_file = program(&session, "file:///src/index.ts")
            .get_source_file("/src/index.ts")
            .expect("source file")
            .root;

        // First query with a file should create a checker and associate it.
        let (c1, release1) = pool.get_checker(&req(&bg(), "file-aff-1", CheckerLifetime::TEMPORARY), source_file);
        release1.call();

        // Second query with the same file (different request) should get the same checker via file affinity.
        let (c2, release2) = pool.get_checker(&req(&bg(), "file-aff-2", CheckerLifetime::TEMPORARY), source_file);
        assert!(same(&c2, &c1), "same file should return the same checker via file affinity");
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:949 TestCheckerPoolMultipleConcurrentQueryCheckers
    // PORT: the blocked 4th request is left out (see the module comment).
    fn multiple_concurrent_query_checkers() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        // Acquire 3 query checkers concurrently (all slots).
        let (c1, release1) = pool.get_checker(&req(&bg(), "multi-q-1", CheckerLifetime::TEMPORARY), NIL);
        let (c2, release2) = pool.get_checker(&req(&bg(), "multi-q-2", CheckerLifetime::TEMPORARY), NIL);
        let (c3, release3) = pool.get_checker(&req(&bg(), "multi-q-3", CheckerLifetime::TEMPORARY), NIL);

        // All three should be distinct checkers.
        assert!(!same(&c1, &c2), "concurrent query checkers should be distinct (1 vs 2)");
        assert!(!same(&c1, &c3), "concurrent query checkers should be distinct (1 vs 3)");
        assert!(!same(&c2, &c3), "concurrent query checkers should be distinct (2 vs 3)");

        // None should be the diagnostics checker at index 0.
        let diag = checker_at(&pool, 0);
        assert!(
            !diag.as_ref().is_some_and(|d| same(d, &c1) || same(d, &c2) || same(d, &c3)),
            "query checkers should not occupy the diagnostics slot"
        );

        release1.call();
        release2.call();
        release3.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:1032 TestCheckerPoolDefaultMaxCheckers
    // PORT: Go also checks `cap(pool.querySem) == 3`; the Rust semaphore is
    // private.
    fn default_max_checkers() {
        // Zero MaxCheckers should default to 4.
        let (_session, pool) = setup_checker_pool_session(opts(0, 10));
        assert_eq!(pool.opts.max_checkers, 4);
        assert_eq!(pool.checkers.borrow().len(), 4);
    }
}

child_test! {
    // Go: checkerpool_test.go:1106 TestCheckerPoolDiscardIdempotent
    fn discard_idempotent() {
        let (_session, pool) = test_pool(opts(2, 10), opts(4, 30));

        // Create a checker so there's something to discard.
        let (_c, release) = pool.get_checker(&req(&bg(), "idem-q", CheckerLifetime::TEMPORARY), NIL);
        release.call();

        // First discard should keep idle checkers alive.
        pool.discard();
        let has_checker = pool.checkers.borrow().iter().any(Option::is_some);
        assert!(has_checker, "first Discard should keep idle checkers alive");

        // Second discard should be a no-op (no panic, no state corruption).
        pool.discard();

        // Pool should still be functional after double Discard.
        let (_c2, release2) = pool.get_checker(&req(&bg(), "post-idem", CheckerLifetime::TEMPORARY), NIL);
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:1149 TestCheckerPoolGetGlobalDiagnosticsEmpty
    fn get_global_diagnostics_empty() {
        let (_session, pool) = setup_checker_pool_session(opts(4, 10));
        // Before any checker is used, global diagnostics should be empty.
        assert_eq!(pool.get_global_diagnostics().len(), 0, "global diagnostics should be empty initially");
    }
}

child_test! {
    // Go: checkerpool_test.go:1151 TestCheckerPoolTakeNewGlobalDiagnostics (ts#64452: the diagnostics checker)
    fn take_new_global_diagnostics() {
        let (_session, pool) = setup_checker_pool_session(opts(4, 10));

        // Initially, no new globals.
        assert!(!pool.take_new_global_diagnostics(), "should report no new globals initially");

        // Use a checker and trigger diagnostics, then release to run the merge.
        let ctx = req(&bg(), "global-diag-req", CheckerLifetime::DIAGNOSTICS);
        let source_file = pool.program.get_source_file("/src/index.ts").expect("source file").root;
        {
            let _guard = ts_goport::program::ls_program::enter(&pool.program);
            let (c, release) = pool.get_checker(&ctx, source_file);
            c.borrow_mut().get_diagnostics_exported(&ctx, source_file);
            release.call();
        }

        assert!(
            pool.take_new_global_diagnostics(),
            "diagnostics checker should publish missing-lib globals"
        );

        // After taking, a second call should always return false (flag is reset).
        assert!(
            !pool.take_new_global_diagnostics(),
            "TakeNewGlobalDiagnostics should reset after first call"
        );

        // Releasing the same checker again with the same state should not set the flag.
        let ctx2 = req(&bg(), "global-diag-req-2", CheckerLifetime::DIAGNOSTICS);
        {
            let _guard = ts_goport::program::ls_program::enter(&pool.program);
            let (c2, release2) = pool.get_checker(&ctx2, source_file);
            c2.borrow_mut().get_diagnostics_exported(&ctx2, source_file);
            release2.call();
        }

        assert!(
            !pool.take_new_global_diagnostics(),
            "should not report new globals when checker state is unchanged"
        );
    }
}

child_test! {
    // Go: checkerpool_test.go:1195 TestCheckerPoolAPICheckerDisposedOnCancel
    fn api_checker_disposed_on_cancel() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));
        let source_file = pool.program.get_source_file("/src/index.ts").expect("source file").root;
        let _guard = ts_goport::program::ls_program::enter(&pool.program);

        let ctx = core_context::with_checker_lifetime(&bg(), CheckerLifetime::API);
        let (c, release) = pool.get_checker(&ctx, NIL);

        // Cancel the API checker.
        let (canceled_ctx, cancel) = context::with_cancel(&bg());
        cancel();
        c.borrow_mut().get_diagnostics_exported(&canceled_ctx, source_file);
        assert!(c.borrow().was_canceled());

        // Releasing a canceled API checker must drop it so it isn't reused.
        release.call();
        assert!(
            pool.persistent_checker.borrow().is_none(),
            "canceled API checker should be dropped on release"
        );

        // Next API acquisition gets a fresh, usable checker rather than panicking.
        let (c2, release2) = pool.get_checker(&ctx, NIL);
        assert!(!same(&c2, &c), "should get a fresh API checker after cancellation");
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:1232 TestCheckerPoolNonCancelableContextNoAffinity
    fn non_cancelable_context_no_affinity() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        // A context that carries a request ID but can never be canceled
        // (ctx.Done() == nil) must not register a request association.
        let ctx = req(&bg(), "uncancelable-req", CheckerLifetime::TEMPORARY);
        assert!(ctx.done().is_none(), "test precondition: context must be non-cancelable");

        let (_c, release) = pool.get_checker(&ctx, NIL);
        release.call();

        assert_eq!(
            pool.request_associations.borrow().len(),
            0,
            "non-cancelable context must not grow requestAssociations"
        );
    }
}

child_test! {
    // Go: checkerpool_test.go:1260 TestCheckerPoolCleanupAfterDiscardIsNoop
    fn cleanup_after_discard_is_noop() {
        let (_session, pool) = test_pool(opts(4, 10), opts(4, 30));

        let (_c, release) = pool.get_checker(&req(&bg(), "discard-cleanup", CheckerLifetime::TEMPORARY), NIL);
        release.call();

        pool.discard();

        // Simulate the timer callback firing after Discard(). It must be a no-op and must not
        // re-arm the cleanup timer, which would keep the discarded pool alive.
        pool.cleanup_idle_checkers();

        assert!(
            pool.cleanup_timer.borrow().is_none(),
            "cleanup must not reschedule a timer on a discarded pool"
        );
        let has_checker = pool.checkers.borrow().iter().any(Option::is_some);
        assert!(has_checker, "idle checkers must survive cleanup on a discarded pool");
    }
}

// The Go tests below run in `synctest` bubbles with fake time. The port's
// idle-cleanup timer is a `gostd::local` timer: it waits in real time and
// its callback runs only in `local::run_pending` on this thread. So the
// tests use real time with a short idle timeout (`IDLE`), Go
// `synctest.Wait()` is `run_pending()`, and Go `time.Sleep(d);
// synctest.Wait()` is `sleep_then_run(d)`.

/// The idle timeout of the timer tests. Go uses 5 s and 10 s of fake time.
/// It is long enough that a loaded host does not reach it between a release
/// and the next check.
const IDLE: Duration = Duration::from_millis(1000);

fn idle_opts(max_checkers: i32, idle_timeout: Duration) -> CheckerPoolOptions {
    CheckerPoolOptions {
        max_checkers,
        idle_timeout,
    }
}

/// Go `synctest.Wait()`: runs the ready timer callbacks of this thread.
fn run_pending() {
    ts_goport::gostd::local::run_pending();
}

/// Go `time.Sleep(d); synctest.Wait()`: sleeps `d` in real time, then runs
/// the timer callbacks that became due. `wait_pending` waits for a due timer
/// whose thread has not queued it yet.
fn sleep_then_run(d: Duration) {
    std::thread::sleep(d);
    ts_goport::gostd::local::wait_pending();
    run_pending();
}

child_test! {
    // Go: checkerpool_test.go:113 TestCheckerPoolIdleCleanup
    // PORT: real time (see above); Go IdleTimeout 5s is `IDLE`.
    fn idle_cleanup() {
        let (_session, pool) = test_pool(opts(2, 10), idle_opts(4, IDLE));

        // Create a checker via a diagnostics request.
        let ctx = req(&bg(), "diag-cleanup", CheckerLifetime::DIAGNOSTICS);
        let (_c, release) = pool.get_checker(&ctx, NIL);
        release.call();
        run_pending();

        // Create a query checker as well.
        let ctx2 = req(&bg(), "query-cleanup", CheckerLifetime::TEMPORARY);
        let (_c2, release2) = pool.get_checker(&ctx2, NIL);
        release2.call();
        run_pending();

        // Both checkers should exist.
        pool.mu_lock();
        assert!(checker_at(&pool, 0).is_some(), "diagnostics checker should exist");
        let len = pool.checkers.borrow().len();
        let query_idx = (1..len).find(|&i| checker_at(&pool, i).is_some()).unwrap_or(0);
        assert!(query_idx > 0, "query checker should exist");

        // Advance past idle timeout.
        sleep_then_run(IDLE);

        // After cleanup, both checkers should be disposed.
        pool.mu_lock();
        assert!(checker_at(&pool, 0).is_none(), "diagnostics checker should be disposed after idle timeout");
        assert!(checker_at(&pool, query_idx).is_none(), "query checker should be disposed after idle timeout");
    }
}

child_test! {
    // Go: checkerpool_test.go:166 TestCheckerPoolFileAssociationCleanup
    // PORT: real time (see above); Go IdleTimeout 5s is `IDLE`.
    fn file_association_cleanup() {
        let (session, pool) = test_pool(opts(2, 10), idle_opts(4, IDLE));
        let source_file = program(&session, "file:///src/index.ts")
            .get_source_file("/src/index.ts")
            .expect("source file")
            .root;

        // Create a query checker with file affinity.
        let ctx = req(&bg(), "file-assoc-req", CheckerLifetime::TEMPORARY);
        let (_c, release) = pool.get_checker(&ctx, source_file);
        release.call();
        run_pending();

        // File association should exist.
        pool.mu_lock();
        let has_assoc = pool.file_associations.borrow().contains_key(&source_file);
        assert!(has_assoc, "file should have a checker association");

        // Advance past idle timeout.
        sleep_then_run(IDLE);

        // File association should be cleared.
        pool.mu_lock();
        let has_assoc = pool.file_associations.borrow().contains_key(&source_file);
        assert!(!has_assoc, "file association should be cleared after checker disposal");
    }
}

child_test! {
    // Go: checkerpool_test.go:219 TestCheckerPoolQueryContention
    // PORT: on the dispatch thread a request can not wait for a slot that
    // another request holds (see the module comment), so the blocked
    // goroutine has no port. The test checks what the Go wait depends on:
    // the only query slot is held by the first request, and after its
    // release the second request gets a checker.
    fn query_contention() {
        // maxCheckers=2 means 1 diagnostics + 1 query checker slot.
        let (_session, pool) = test_pool(opts(2, 10), opts(2, 30));

        // Acquire the only query checker slot.
        let ctx1 = req(&bg(), "query-hold", CheckerLifetime::TEMPORARY);
        let (c1, release1) = pool.get_checker(&ctx1, NIL);
        run_pending();
        assert_eq!(pool.checkers.borrow().len(), 2);
        assert!(checker_at(&pool, 1).is_some_and(|x| same(&x, &c1)));
        assert_eq!(
            pool.held_by.borrow()[1],
            project::checkerpool::CHECKER_HELD_ANONYMOUS,
            "the only query slot should be held while the first request runs"
        );

        // Release the first checker — second should acquire it.
        release1.call();
        run_pending();
        let ctx2 = req(&bg(), "query-wait", CheckerLifetime::TEMPORARY);
        let (_c2, release2) = pool.get_checker(&ctx2, NIL);
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:258 TestCheckerPoolDiagnosticsContention
    // PORT: the blocked second diagnostics request has no port (see
    // TestCheckerPoolQueryContention above); the concurrent query request
    // and the acquire after release are checked.
    fn diagnostics_contention() {
        let (_session, pool) = test_pool(opts(2, 10), opts(2, 30));

        // Acquire the diagnostics checker.
        let ctx1 = req(&bg(), "diag-hold", CheckerLifetime::DIAGNOSTICS);
        let (c1, release1) = pool.get_checker(&ctx1, NIL);
        run_pending();
        assert!(
            !pool.held_by.borrow()[0].is_empty(),
            "the diagnostics checker should be held while the first request runs"
        );

        // A query request should NOT be blocked (separate slot).
        let ctx3 = req(&bg(), "query-concurrent", CheckerLifetime::TEMPORARY);
        let (c3, release3) = pool.get_checker(&ctx3, NIL);
        assert!(!same(&c3, &c1), "query checker should be different from diagnostics checker");
        release3.call();

        // Release the diagnostics checker — second diag request should acquire it.
        release1.call();
        run_pending();
        let ctx2 = req(&bg(), "diag-wait", CheckerLifetime::DIAGNOSTICS);
        let (_c2, release2) = pool.get_checker(&ctx2, NIL);
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:412 TestCheckerPoolDiagnosticsRecreatedAfterIdleDisposal
    // PORT: real time (see above); Go IdleTimeout 5s is `IDLE`.
    fn diagnostics_recreated_after_idle_disposal() {
        let (_session, pool) = test_pool(opts(2, 10), idle_opts(4, IDLE));

        // Create and release diagnostics checker.
        let ctx = req(&bg(), "diag-recreate-1", CheckerLifetime::DIAGNOSTICS);
        let (c1, release1) = pool.get_checker(&ctx, NIL);
        release1.call();
        run_pending();

        // Advance past idle timeout — diagnostics checker should be disposed.
        sleep_then_run(IDLE);

        pool.mu_lock();
        assert!(checker_at(&pool, 0).is_none(), "diagnostics checker should be disposed");

        // Request diagnostics checker again — should get a fresh one.
        let ctx2 = req(&bg(), "diag-recreate-2", CheckerLifetime::DIAGNOSTICS);
        let (c2, release2) = pool.get_checker(&ctx2, NIL);
        assert!(!same(&c2, &c1), "should be a new checker instance");
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:448 TestCheckerPoolCrossReleaseAffinityWithContention
    // PORT: request A's blocked reacquire has no port (see
    // TestCheckerPoolQueryContention above). A reacquires after B releases,
    // and must get its own checker back.
    fn cross_release_affinity_with_contention() {
        // maxCheckers=2: 1 diagnostics + 1 query slot.
        let (_session, pool) = test_pool(opts(2, 10), opts(2, 30));

        let (req_ctx, req_cancel) = context::with_cancel(&bg());

        // Request A acquires the only query slot.
        let ctx_a = req(&req_ctx, "req-A", CheckerLifetime::TEMPORARY);
        let (c_a, release_a) = pool.get_checker(&ctx_a, NIL);
        release_a.call();
        run_pending();

        // Request B takes the query slot while A is released.
        let ctx_b = req(&bg(), "req-B", CheckerLifetime::TEMPORARY);
        let (_c_b, release_b) = pool.get_checker(&ctx_b, NIL);
        assert!(
            !pool.held_by.borrow()[1].is_empty(),
            "request B should hold the only query slot"
        );

        // Release B — A should get the same checker.
        release_b.call();
        run_pending();
        let (c_a2, release) = pool.get_checker(&ctx_a, NIL);
        assert!(same(&c_a2, &c_a), "request A should get the same checker on reacquire");
        release.call();
        // Go: defer reqCancel()
        req_cancel();
    }
}

child_test! {
    // Go: checkerpool_test.go:1010 TestCheckerPoolDoubleReleaseSafe
    // PORT: Go `release` is `sync.OnceFunc`. `Release::call` takes `self`,
    // so a second call does not compile and the drop after the call does
    // nothing. The test checks that the pool works after the release.
    fn double_release_safe() {
        let (_session, pool) = setup_checker_pool_session(opts(4, 10));

        let ctx = req(&bg(), "double-release", CheckerLifetime::TEMPORARY);
        let (_c, release) = pool.get_checker(&ctx, NIL);

        // First release should work normally.
        release.call();

        // Pool should still be functional after the release.
        let ctx2 = req(&bg(), "after-double", CheckerLifetime::TEMPORARY);
        let (_c2, release2) = pool.get_checker(&ctx2, NIL);
        release2.call();
    }
}

child_test! {
    // Go: checkerpool_test.go:1041 TestCheckerPoolStaggeredIdleCleanup
    // PORT: real time (see above). Go times scale by `IDLE` / 10 s: A is
    // released at 0, B at 0.6 * IDLE, the check is before A's deadline, and
    // the last check is at 1.7 * IDLE, after both deadlines.
    fn staggered_idle_cleanup() {
        let (_session, pool) = test_pool(opts(4, 10), idle_opts(4, IDLE));

        // Acquire checker A and hold it.
        let ctx_a = req(&bg(), "stagger-A", CheckerLifetime::TEMPORARY);
        let (c_a, release_a) = pool.get_checker(&ctx_a, NIL);

        // While A is held, acquire a second checker B.
        let ctx_b = req(&bg(), "stagger-B", CheckerLifetime::TEMPORARY);
        let (c_b, release_b) = pool.get_checker(&ctx_b, NIL);
        assert!(!same(&c_b, &c_a), "B should be a different checker since A is held");

        // Find their indices.
        pool.mu_lock();
        let idx_a = query_index(&pool, &c_a);
        let idx_b = query_index(&pool, &c_b);
        assert!(idx_a > 0);
        assert!(idx_b > 0);

        // Release A first. Timer is set for t=IDLE.
        release_a.call();
        run_pending();

        // Release B 0.6 * IDLE later.
        std::thread::sleep(IDLE * 6 / 10);
        release_b.call();
        run_pending();

        // Before A's deadline, both should still exist (timer hasn't fired).
        pool.mu_lock();
        assert!(checker_at(&pool, idx_a).is_some(), "checker A should still exist before timer fires");
        assert!(checker_at(&pool, idx_b).is_some(), "checker B should still exist before timer fires");

        // Advance past both deadlines. Both should be disposed.
        sleep_then_run(IDLE * 11 / 10);

        pool.mu_lock();
        assert!(checker_at(&pool, idx_a).is_none(), "checker A should be disposed after timer fires");
        assert!(checker_at(&pool, idx_b).is_none(), "checker B should be disposed after timer fires");
    }
}
