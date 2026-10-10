//! Rust port of the Go tests of `internal/project` (with `ata`,
//! `background`, `dirty`, `logging`), `internal/lsp` and
//! `internal/ls/autoimport`, and their helpers `projecttestutil`,
//! `lsptestutil` and `autoimporttestutil`. The `internal/api` session tests
//! that use `projecttestutil` are here too (`api_session_*_test`, with the
//! helpers `api_util`), and the `internal/api/requestfilesystem` tests
//! (`requestfilesystem_*test`), which use `support::vfstest`.
//!
//! PORT: each Go leaf subtest (`t.Run`) is one `#[test]`, so a port bug
//! can be marked `#[ignore = "bug: S4-..."]` on the one subtest that shows
//! it. `t.Parallel()` is dropped. The expected values are the Go test
//! literals.
//!
//! A test that builds a program runs in a child process of its own
//! (`child_test!`, `support::child::run_test_in_child`): parse workers and
//! module specifier code read `osvfs_fs()`, and the OS override that
//! points it at the test map file system is for the whole process
//! (`projecttestutil::install_fs_override`).
//!
//! Test names are `project_lsp::<go file>::<go subtest path>`.

/// A `#[test]` that runs its body in a child process with the map file
/// system override (see the module comment). The libtest name comes from
/// `module_path!()` without the crate name.
macro_rules! child_test {
    ($(#[$meta:meta])* fn $name:ident() $body:block) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            let path = concat!(module_path!(), "::", stringify!($name));
            let test = path.split_once("::").map_or(path, |(_, rest)| rest);
            crate::support::child::run_test_in_child(test, || {
                crate::project_lsp::projecttestutil::install_fs_override();
                $body
            });
        }
    };
    // The child runs with the environment variables `$env` set.
    (env $env:expr; $(#[$meta:meta])* fn $name:ident() $body:block) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            let path = concat!(module_path!(), "::", stringify!($name));
            let test = path.split_once("::").map_or(path, |(_, rest)| rest);
            crate::support::child::run_test_in_child_with_env(test, $env, || {
                crate::project_lsp::projecttestutil::install_fs_override();
                $body
            });
        }
    };
}

pub(crate) mod api_util;
pub(crate) mod autoimporttestutil;
pub(crate) mod lsptestutil;
pub(crate) mod projecttestutil;
pub(crate) mod util;

mod api_createsourcefile_freeable_test;
mod api_createsourcefile_stdio_test;
mod api_session_apistate_test;
mod api_session_batch_test;
mod api_session_completion_test;
mod api_session_createprogram_test;
mod api_session_createsourcefile_test;
mod api_session_crossproject_test;
mod api_session_diagnostics_test;
mod api_session_effect_test;
mod api_session_misuse_test;
mod api_session_module_resolution_test;
mod api_session_requestfilesystem_test;
mod api_session_symbolresponse_test;
mod ata_discovertypings_test;
mod ata_installnpmpackages_test;
mod ata_test;
mod ata_validatepackagename_test;
mod autoimport_aliasresolver_crash_test;
mod autoimport_index_test;
mod autoimport_registry_test;
mod autoimport_util_test;
mod background_queue_test;
mod bulkcache_test;
mod checkerpool_test;
mod configfilechanges_test;
mod contentmapper_test;
mod customconfigfilename_test;
mod dirty_syncmap_test;
mod extendedconfigcache_test;
mod file_version_test;
mod logging_logtree_test;
mod lsp_dynamic_queue_test;
mod lsp_progress_test;
mod lsp_replay_test;
mod lsp_server_apisession_test;
mod lsp_server_completion_test;
mod lsp_server_contentmapper_internal_test;
mod lsp_server_contentmapper_test;
mod lsp_server_flakydiagnostics_test;
mod lsp_server_progress_test;
mod lsp_server_projectinfo_test;
mod lsp_server_projectreference_updates_test;
mod lsp_server_semantictokens_test;
mod lsp_server_shutdown_test;
mod lsp_stack_sanitizer_test;
mod overlayfs_test;
mod parseahead_test;
mod project_test;
mod projectcollectionbuilder_test;
mod projectcollectiondefaultproject_test;
mod projectlifetime_test;
mod projectreferencesprogram_test;
mod refcountcache_test;
mod released_program_test;
mod requestfilesystem_filechanges_test;
mod requestfilesystem_pathtree_test;
mod requestfilesystem_test;
mod resolveahead_test;
mod selectionranges_test;
mod session_test;
mod sharedtext_test;
mod snapshot_task_program_test;
mod snapshot_test;
mod snapshotfs_test;
mod stringliteralranges_test;
mod untitled_test;
mod version_tables_test;
mod watch_test;
mod watchtimeout_test;
