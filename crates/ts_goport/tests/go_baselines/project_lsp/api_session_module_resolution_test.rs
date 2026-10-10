//! Port of Go `internal/api/session_module_resolution_test.go` (ts#64299).
//!
//! PORT: the tests are in `project_lsp` because they use `projecttestutil`
//! and `child_test!`. A Go `defer ...Close()` is a call at the end of the
//! test, in the Go defer order.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ts_goport::api::{
    self, CreateModuleResolverParams, CreateProgramOptions, CreateSnapshotParams,
    CreateSnapshotProgramParams, GetCurrentLanguageServerSnapshotParams, GetSourceFileNamesParams,
    LanguageServerSnapshotChanges, ModuleResolutionEntry, ModuleResolutionFallback,
    ModuleResolutionSpec, ModuleResolverID, ModuleResolverRegistration, PackageId, ResolutionMode,
    ResolveModuleNameParams, SnapshotRequestChangesParams, StaticModuleResolution,
};
use ts_goport::flags::{ModuleKind, ModuleResolutionKind};
use ts_goport::frontend::json_ext::{AnyValue, JsonValue};
use ts_goport::frontend::module;
use ts_goport::frontend::vfs;
use ts_goport::gostd::{Context, GoError, context, errors};
use ts_goport::ipc;
use ts_goport::options::{CompilerOptions, Tristate};

use super::api_util::{doc, error_contains, nil_error};
use super::projecttestutil::{self, files};
use super::util::bg;

// Go: session_module_resolution_test.go:16 failingModuleResolutionConn
struct FailingModuleResolutionConn {
    calls: Cell<usize>,
    // ts#64519
    contexts: RefCell<Vec<Context>>,
}

impl ipc::Conn for FailingModuleResolutionConn {
    // Go: session_module_resolution_test.go:21 failingModuleResolutionConn.Run
    fn run(&self, _ctx: &Context) -> Result<(), GoError> {
        Ok(())
    }

    // Go: session_module_resolution_test.go:25 failingModuleResolutionConn.Call
    fn call(
        &self,
        ctx: &Context,
        _method: &str,
        _params: Option<Box<dyn AnyValue>>,
    ) -> Result<JsonValue, GoError> {
        self.calls.set(self.calls.get() + 1);
        self.contexts.borrow_mut().push(ctx.clone());
        Err(errors::new("callback error"))
    }

    // Go: session_module_resolution_test.go:31 failingModuleResolutionConn.Notify
    fn notify(
        &self,
        _ctx: &Context,
        _method: &str,
        _params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        Ok(())
    }
}

/// Go `module.ResolverOptions{Host: session}`: the session is the Go
/// `module.ResolutionHost` (its `FS` and `GetCurrentDirectory`).
struct SessionResolutionHost {
    fs: Rc<dyn vfs::Fs>,
    current_directory: String,
}

impl module::ResolutionHost for SessionResolutionHost {
    fn fs(&self) -> &dyn vfs::Fs {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        &self.current_directory
    }
}

/// Go `core.CompilerOptions{NoLib: core.TSTrue, Module: core.ModuleKindNodeNext, ModuleResolution: core.ModuleResolutionKindNodeNext}`.
fn no_lib_node_next() -> CompilerOptions {
    CompilerOptions {
        no_lib: Tristate::True,
        module: ModuleKind::NODE_NEXT,
        module_resolution: ModuleResolutionKind::NODE_NEXT,
        ..Default::default()
    }
}

// Go: session_module_resolution_test.go:35 TestModuleResolverUsesSnapshotFileSystem
child_test! {
    fn module_resolver_uses_snapshot_file_system() {
        let (project_session, _) = projecttestutil::setup(files(&[
            (
                "/home/projects/p/node_modules/pkg/package.json",
                r#"{"name":"pkg","version":"1.0.0","exports":{".":{"types":"./index.d.ts","default":"./index.js"}}}"#,
            ),
            (
                "/home/projects/p/node_modules/pkg/index.d.ts",
                "export declare const value: string;",
            ),
            (
                "/home/projects/p/node_modules/pkg/index.js",
                r#"exports.value = "value";"#,
            ),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let snapshot =
            nil_error(session.handle_create_snapshot(&bg(), &CreateSnapshotParams::default()));
        let resolver = nil_error(session.handle_create_module_resolver(&CreateModuleResolverParams {
            compiler_options: CompilerOptions {
                module: ModuleKind::NODE_NEXT,
                module_resolution: ModuleResolutionKind::NODE_NEXT,
                trace_resolution: Tristate::True,
                ..Default::default()
            },
            ..Default::default()
        }));
        let result = nil_error(session.handle_resolve_module_name(
            &bg(),
            &ResolveModuleNameParams {
                snapshot: snapshot.snapshot,
                resolver,
                module_name: "pkg".to_string(),
                containing_directory: doc("/home/projects/p/src"),
                ..Default::default()
            },
        ));
        let resolved_module = result.resolved_module.as_ref().unwrap();
        assert_eq!(
            resolved_module.resolved_file_name,
            "/home/projects/p/node_modules/pkg/index.d.ts"
        );
        assert_eq!(resolved_module.package_id.as_ref().unwrap().name, "pkg");
        assert!(!result.trace.is_empty());
        session.close();
        project_session.close();
    }
}

// Go: session_module_resolution_test.go:69 TestStaticModuleResolutionSpecificityAndLifetime
child_test! {
    fn static_module_resolution_specificity_and_lifetime() {
        let (project_session, _) = projecttestutil::setup(files(&[
            ("/home/projects/p/global.d.ts", r#"export declare const value: "global";"#),
            ("/home/projects/p/mode.d.ts", r#"export declare const value: "mode";"#),
            ("/home/projects/p/dir.d.ts", r#"export declare const value: "dir";"#),
            ("/home/projects/p/exact.d.ts", r#"export declare const value: "exact";"#),
            ("/home/projects/p/default.d.ts", r#"export declare const value: "default";"#),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let snapshot =
            nil_error(session.handle_create_snapshot(&bg(), &CreateSnapshotParams::default()));
        let esm = ModuleKind::ES_NEXT;
        let spec = ModuleResolutionSpec {
            fallback: ModuleResolutionFallback::UNRESOLVED,
            entries: vec![
                Some(static_resolution_entry("pkg", "", None, "/home/projects/p/global.d.ts")),
                Some(static_resolution_entry("pkg", "", Some(esm), "/home/projects/p/mode.d.ts")),
                Some(static_resolution_entry(
                    "pkg",
                    "/home/projects/p/src",
                    None,
                    "/home/projects/p/dir.d.ts",
                )),
                Some(static_resolution_entry(
                    "pkg",
                    "/home/projects/p/src",
                    Some(esm),
                    "/home/projects/p/exact.d.ts",
                )),
            ],
        };
        let resolver_id = nil_error(session.handle_create_module_resolver(&CreateModuleResolverParams {
            compiler_options: CompilerOptions {
                module_resolution: ModuleResolutionKind::NODE_NEXT,
                ..Default::default()
            },
            module_resolutions: Some(spec),
            ..Default::default()
        }));

        let assert_resolution = |directory: &str, mode: ModuleKind, expected: &str| {
            let resolution_mode = ResolutionMode(mode.0);
            let result = nil_error(session.handle_resolve_module_name(
                &bg(),
                &ResolveModuleNameParams {
                    snapshot: snapshot.snapshot,
                    resolver: resolver_id,
                    module_name: "pkg".to_string(),
                    containing_directory: doc(directory),
                    resolution_mode: Some(resolution_mode),
                    ..Default::default()
                },
            ));
            assert_eq!(
                result.resolved_module.as_ref().unwrap().resolved_file_name,
                expected
            );
            assert_eq!(result.trace.len(), 0);
        };
        assert_resolution("/home/projects/p/src", ModuleKind::ES_NEXT, "/home/projects/p/exact.d.ts");
        assert_resolution("/home/projects/p/src", ModuleKind::COMMON_JS, "/home/projects/p/dir.d.ts");
        assert_resolution("/home/projects/p/other", ModuleKind::ES_NEXT, "/home/projects/p/mode.d.ts");
        assert_resolution(
            "/home/projects/p/other",
            ModuleKind::COMMON_JS,
            "/home/projects/p/global.d.ts",
        );

        let unresolved = nil_error(session.handle_resolve_module_name(
            &bg(),
            &ResolveModuleNameParams {
                snapshot: snapshot.snapshot,
                resolver: resolver_id,
                module_name: "other".to_string(),
                containing_directory: doc("/home/projects/p/src"),
                ..Default::default()
            },
        ));
        assert!(unresolved.resolved_module.is_none());

        assert_resolution("/home/projects/p/src", ModuleKind::ES_NEXT, "/home/projects/p/exact.d.ts");
        session.close();
        project_session.close();
    }
}

// Go: session_module_resolution_test.go:132 TestCreateProgramUsesStaticModuleResolutions
child_test! {
    fn create_program_uses_static_module_resolutions() {
        const ROOT: &str = "/home/projects/p/src/index.ts";
        const PROVIDED: &str = "/home/projects/p/provided.d.ts";
        let (project_session, _) = projecttestutil::setup(files(&[
            (ROOT, r#"import { value } from "pkg"; export { value };"#),
            (PROVIDED, "export declare const value: string;"),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);
        let resolver = nil_error(session.handle_create_module_resolver(&CreateModuleResolverParams {
            compiler_options: no_lib_node_next(),
            module_resolutions: Some(ModuleResolutionSpec {
                fallback: ModuleResolutionFallback::UNRESOLVED,
                entries: vec![Some(static_resolution_entry("pkg", "", None, PROVIDED))],
            }),
            ..Default::default()
        }));

        let response = nil_error(session.handle_create_snapshot(
            &bg(),
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    create_programs: Some(vec![Some(CreateSnapshotProgramParams {
                        root_files: vec![doc(ROOT)],
                        compiler_options: no_lib_node_next(),
                        options: Some(CreateProgramOptions {
                            module_resolver: resolver,
                            ..Default::default()
                        }),
                    })]),
                    ..Default::default()
                },
                ..Default::default()
            },
        ));
        let project_id =
            response.operation.as_ref().unwrap().created_programs.as_ref().unwrap()[0].as_id();
        let file_names = nil_error(session.handle_get_source_file_names(
            &bg(),
            &GetSourceFileNamesParams {
                snapshot: response.snapshot,
                project: project_id,
            },
        ));
        assert_eq!(file_names, vec![PROVIDED, ROOT]);
        session.close();
        project_session.close();
    }
}

/// Go `callbackTestConn{responses: ...}` (callbackfs_test.go:16): each call
/// answers the JSON value of its method.
struct FixedResponseConn {
    responses: Vec<(&'static str, String)>,
}

impl ipc::Conn for FixedResponseConn {
    fn run(&self, _ctx: &Context) -> Result<(), GoError> {
        Ok(())
    }

    fn call(
        &self,
        _ctx: &Context,
        method: &str,
        _params: Option<Box<dyn AnyValue>>,
    ) -> Result<JsonValue, GoError> {
        let response = self
            .responses
            .iter()
            .find(|(name, _)| *name == method)
            .map_or(String::new(), |(_, value)| value.clone());
        Ok(JsonValue(response.into_bytes()))
    }

    fn notify(
        &self,
        _ctx: &Context,
        _method: &str,
        _params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        Ok(())
    }
}

// Go: session_module_resolution_test.go:184 TestCustomModuleResolutionsSkipUnsafeRewriteDiagnostic (ts#64638)
child_test! {
    fn custom_module_resolutions_skip_unsafe_rewrite_diagnostic() {
        const ROOT: &str = "/home/projects/p/src/a.ts";
        const STATIC_TARGET: &str = "/home/projects/p/src/b.ts";
        const CALLBACK_TARGET: &str = "/home/projects/p/src/c.ts";
        let (project_session, _) = projecttestutil::setup(files(&[
            (
                ROOT,
                r#"import { b } from "./b.ts"; import { c } from "./c.ts"; export const a = b + c;"#,
            ),
            (STATIC_TARGET, "export const b = 1;"),
            (CALLBACK_TARGET, "export const c = 2;"),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);
        *session.conn.borrow_mut() = Some(Rc::new(FixedResponseConn {
            responses: vec![(
                "resolveModuleName/1",
                format!(r#"{{"resolvedFileName":"{CALLBACK_TARGET}"}}"#),
            )],
        }));
        let compiler_options = || CompilerOptions {
            no_lib: Tristate::True,
            module: ModuleKind::NODE_NEXT,
            module_resolution: ModuleResolutionKind::NODE_NEXT,
            rewrite_relative_import_extensions: Tristate::True,
            out_dir: "/home/projects/p/out".to_string(),
            ..Default::default()
        };
        let resolver = nil_error(session.handle_create_module_resolver(&CreateModuleResolverParams {
            compiler_options: compiler_options(),
            module_resolutions: Some(ModuleResolutionSpec {
                fallback: ModuleResolutionFallback::RESOLVE,
                entries: vec![Some(static_resolution_entry("./b.ts", "", None, STATIC_TARGET))],
            }),
            resolve_module_name_callback: "resolveModuleName/1".to_string(),
        }));

        let response = nil_error(session.handle_create_snapshot(
            &bg(),
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    create_programs: Some(vec![Some(CreateSnapshotProgramParams {
                        root_files: vec![doc(ROOT), doc(STATIC_TARGET), doc(CALLBACK_TARGET)],
                        compiler_options: compiler_options(),
                        options: Some(CreateProgramOptions {
                            module_resolver: resolver,
                            ..Default::default()
                        }),
                    })]),
                    ..Default::default()
                },
                ..Default::default()
            },
        ));
        let diagnostics = nil_error(session.handle_get_semantic_diagnostics(
            &bg(),
            &api::GetDiagnosticsParams {
                snapshot: response.snapshot,
                project: response.operation.as_ref().unwrap().created_programs.as_ref().unwrap()[0]
                    .as_id(),
                files: Some(vec![doc(ROOT)]),
            },
        ));
        if let Some(diagnostic) = diagnostics.first() {
            panic!(
                "handleGetSemanticDiagnostics({ROOT}) reported TS{} at {}: {}",
                diagnostic.code, diagnostic.pos, diagnostic.text
            );
        }
        session.close();
        project_session.close();
    }
}

// Go: session_module_resolution_test.go:243 TestStaticModuleResolutionPreservesStaticIdentity
child_test! {
    fn static_module_resolution_preserves_static_identity() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let snapshot =
            nil_error(session.handle_create_snapshot(&bg(), &CreateSnapshotParams::default()));
        let resolver = nil_error(session.handle_create_module_resolver(&CreateModuleResolverParams {
            compiler_options: CompilerOptions {
                module_resolution: ModuleResolutionKind::NODE_NEXT,
                ..Default::default()
            },
            module_resolutions: Some(ModuleResolutionSpec {
                fallback: ModuleResolutionFallback::UNRESOLVED,
                entries: vec![Some(ModuleResolutionEntry {
                    module_name: "pkg".to_string(),
                    result: Some(StaticModuleResolution {
                        resolved_file_name: Some(doc("/store/pkg/index.d.ts")),
                        original_path: Some(doc("/node_modules/pkg/index.d.ts")),
                        package_id: Some(PackageId {
                            name: "pkg".to_string(),
                            sub_module_name: String::new(),
                            version: "1.2.3".to_string(),
                            ..Default::default()
                        }),
                    }),
                    ..Default::default()
                })],
            }),
            ..Default::default()
        }));
        let result = nil_error(session.handle_resolve_module_name(
            &bg(),
            &ResolveModuleNameParams {
                snapshot: snapshot.snapshot,
                resolver,
                module_name: "pkg".to_string(),
                containing_directory: doc("/src"),
                ..Default::default()
            },
        ));
        let resolved_module = result.resolved_module.as_ref().unwrap();
        assert_eq!(resolved_module.original_path, "/node_modules/pkg/index.d.ts");
        assert_eq!(resolved_module.package_id.as_ref().unwrap().name, "pkg");
        assert_eq!(resolved_module.package_id.as_ref().unwrap().version, "1.2.3");
        assert!(resolved_module.is_external_library_import);
        session.close();
        project_session.close();
    }
}

// Go: session_module_resolution_test.go:287 TestModuleResolutionCallbackErrorsAreReturned
// PORT: the Go test builds the unexported `moduleResolverFactory` with its
// fields. The port's factory fields are private; the test registers the
// same registration (id 1) and connection on the session and takes the
// factory from `Session.moduleResolverFactory`, which fills the same fields
// (the session's current directory is "/"). Go `registration.compilerOptions`
// is nil; the port's is the empty options.
child_test! {
    fn module_resolution_callback_errors_are_returned() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);
        let conn = Rc::new(FailingModuleResolutionConn {
            calls: Cell::new(0),
            contexts: RefCell::new(Vec::new()),
        });
        let registration = Rc::new(ModuleResolverRegistration {
            id: ModuleResolverID(1),
            compiler_options: Rc::new(CompilerOptions::default()),
            resolutions: None,
            resolve_module_name_callback: "resolveModuleName/1".to_string(),
        });
        session
            .module_resolvers
            .borrow_mut()
            .insert(registration.id, registration.clone());
        *session.conn.borrow_mut() = Some(conn.clone());
        assert_eq!(session.get_current_directory(), "/");
        let factory = nil_error(session.module_resolver_factory(
            &CreateProgramOptions {
                module_resolver: registration.id,
                ..Default::default()
            },
        ))
        .unwrap();
        let (provider, cleanup) = factory.new_resolver(&bg(), module::ResolverOptions {
            host: Some(Rc::new(SessionResolutionHost {
                fs: session.fs(),
                current_directory: session.get_current_directory(),
            })),
            compiler_options: Some(Rc::new(CompilerOptions::default())),
            ..Default::default()
        });
        for _ in 0..2 {
            let (_, _, err) =
                provider.resolve_module_name_from_directory("pkg", "/src", ModuleKind::ESM);
            error_contains(err.map_or(Ok(()), Err), "callback error");
        }
        assert_eq!(conn.calls.get(), 2);
        assert_eq!(session.program_resolution_contexts.contexts.borrow().len(), 1);
        cleanup();
        assert_eq!(session.program_resolution_contexts.contexts.borrow().len(), 0);
        session.close();
        project_session.close();
    }
}

// Go: session_module_resolution_test.go:319 TestModuleResolutionFactoryUsesCurrentContext (ts#64519)
// PORT: the factory comes from the session as in
// `module_resolution_callback_errors_are_returned`. Go compares the context
// values; here their Done channels (each `with_cancel` context has its own).
// Go `t.Context()` is a second cancel context.
child_test! {
    fn module_resolution_factory_uses_current_context() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);
        let conn = Rc::new(FailingModuleResolutionConn {
            calls: Cell::new(0),
            contexts: RefCell::new(Vec::new()),
        });
        let registration = Rc::new(ModuleResolverRegistration {
            id: ModuleResolverID(1),
            compiler_options: Rc::new(CompilerOptions::default()),
            resolutions: None,
            resolve_module_name_callback: "resolveModuleName/1".to_string(),
        });
        session
            .module_resolvers
            .borrow_mut()
            .insert(registration.id, registration.clone());
        *session.conn.borrow_mut() = Some(conn.clone());
        let factory = nil_error(session.module_resolver_factory(&CreateProgramOptions {
            module_resolver: registration.id,
            ..Default::default()
        }))
        .unwrap();
        let (old_context, cancel) = context::with_cancel(&bg());
        let (test_context, cancel_test) = context::with_cancel(&bg());
        for ctx in [old_context, test_context] {
            let (resolver, cleanup) = factory.new_resolver(
                &ctx,
                module::ResolverOptions {
                    host: Some(Rc::new(SessionResolutionHost {
                        fs: session.fs(),
                        current_directory: session.get_current_directory(),
                    })),
                    compiler_options: Some(Rc::new(CompilerOptions::default())),
                    ..Default::default()
                },
            );
            let (_, _, err) =
                resolver.resolve_module_name_from_directory("pkg", "/src", ModuleKind::ESM);
            error_contains(err.map_or(Ok(()), Err), "callback error");
            let last = conn.contexts.borrow().last().cloned().unwrap();
            assert!(last.done().unwrap().ptr_eq(&ctx.done().unwrap()));
            cleanup();
            cancel();
        }
        assert_eq!(session.program_resolution_contexts.contexts.borrow().len(), 0);
        cancel_test();
        session.close();
        project_session.close();
    }
}

// Go: session_module_resolution_test.go:348 TestModuleResolutionCallbackErrorRejectsLanguageServerUpdate
child_test! {
    fn module_resolution_callback_error_rejects_language_server_update() {
        let (project_session, _) = projecttestutil::setup(files(&[("/src/index.ts", r#"import "pkg";"#)]));
        let session = api::new_lsp_session(project_session.clone(), None);
        *session.conn.borrow_mut() = Some(Rc::new(FailingModuleResolutionConn {
            calls: Cell::new(0),
            contexts: RefCell::new(Vec::new()),
        }));
        let resolver = nil_error(session.handle_create_module_resolver(&CreateModuleResolverParams {
            compiler_options: no_lib_node_next(),
            resolve_module_name_callback: "resolveModuleName/1".to_string(),
            ..Default::default()
        }));
        let base_snapshot = project_session.snapshot();

        error_contains(
            session.handle_get_current_language_server_snapshot(
                &bg(),
                &GetCurrentLanguageServerSnapshotParams {
                    changes: Some(LanguageServerSnapshotChanges {
                        snapshot_request_changes_params: SnapshotRequestChangesParams {
                            create_programs: Some(vec![Some(CreateSnapshotProgramParams {
                                root_files: vec![doc("/src/index.ts")],
                                compiler_options: no_lib_node_next(),
                                options: Some(CreateProgramOptions {
                                    module_resolver: resolver,
                                    ..Default::default()
                                }),
                            })]),
                            ..Default::default()
                        },
                    }),
                    ..Default::default()
                },
            ),
            "callback error",
        );
        // ts#64519
        assert_eq!(session.program_resolution_contexts.contexts.borrow().len(), 0);
        assert!(Rc::ptr_eq(&project_session.snapshot(), &base_snapshot));
        assert_eq!(
            project_session
                .snapshot()
                .project_collection
                .synthetic_projects()
                .len(),
            0
        );
        session.close();
        project_session.close();
    }
}

// Go: session_module_resolution_test.go:388 staticResolutionEntry
fn static_resolution_entry(
    module_name: &str,
    directory: &str,
    mode: Option<ModuleKind>,
    file_name: &str,
) -> ModuleResolutionEntry {
    let mut entry = ModuleResolutionEntry {
        module_name: module_name.to_string(),
        result: Some(StaticModuleResolution {
            resolved_file_name: Some(doc(file_name)),
            ..Default::default()
        }),
        ..Default::default()
    };
    if let Some(mode) = mode {
        entry.resolution_mode = Some(ResolutionMode(mode.0));
    }
    if !directory.is_empty() {
        entry.containing_directory = Some(doc(directory));
    }
    entry
}
