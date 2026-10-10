//! Short forms of the Go expressions that the `internal/api` session tests
//! repeat (`api_session_*_test`, `api_server_test`). No Go file: each helper
//! is one Go expression (named in its comment).

use std::rc::Rc;

use ts_goport::api::{self, CreateSnapshotProgramParams, DocumentIdentifier, SnapshotID};
use ts_goport::frontend::compiler::NewProgram;
use ts_goport::gostd::GoError;
use ts_goport::options::{CompilerOptions, Tristate};
use ts_goport::project;

/// Go `assert.NilError(t, err)` on a result.
pub fn nil_error<T>(result: Result<T, GoError>) -> T {
    result.unwrap_or_else(|err| panic!("unexpected error: {}", err.error()))
}

/// Go `assert.ErrorContains(t, err, want)`.
pub fn error_contains<T>(result: Result<T, GoError>, want: &str) {
    match result {
        Ok(_) => panic!("expected an error containing {want:?}, got nil"),
        Err(err) => assert!(
            err.error().contains(want),
            "expected an error containing {want:?}, got {:?}",
            err.error()
        ),
    }
}

/// Go `DocumentIdentifier{FileName: name}`.
pub fn doc(name: &str) -> DocumentIdentifier {
    DocumentIdentifier {
        file_name: name.to_string(),
        ..Default::default()
    }
}

/// Go `core.CompilerOptions{NoLib: core.TSTrue}`.
pub fn no_lib() -> CompilerOptions {
    CompilerOptions {
        no_lib: Tristate::True,
        ..Default::default()
    }
}

/// Go `&CreateSnapshotProgramParams{RootFiles: {{FileName: name}...}, CompilerOptions: options}`.
pub fn program_params(
    names: &[&str],
    compiler_options: CompilerOptions,
) -> CreateSnapshotProgramParams {
    CreateSnapshotProgramParams {
        root_files: names.iter().map(|name| doc(name)).collect(),
        compiler_options,
        compiler_options_input: None,
        options: None,
    }
}

/// Go `session.snapshots[id].snapshot`.
pub fn snapshot_of(session: &api::Session, id: SnapshotID) -> Rc<project::Snapshot> {
    session
        .snapshots
        .borrow()
        .get(&id)
        .unwrap_or_else(|| panic!("no snapshot {}", id.0))
        .snapshot
        .clone()
}

/// Go `snapshot.ProjectCollection.GetProject(project.ID(id)).GetProgram()`.
pub fn project_program(snapshot: &project::Snapshot, id: &str) -> Rc<NewProgram> {
    snapshot
        .project_collection
        .get_project(&project::ID(id.to_string()))
        .unwrap_or_else(|| panic!("no project {id}"))
        .borrow()
        .get_program()
        .unwrap_or_else(|| panic!("project {id} has no program"))
}
