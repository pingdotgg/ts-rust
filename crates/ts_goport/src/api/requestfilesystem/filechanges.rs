//! Port of Go `internal/api/requestfilesystem/filechanges.go` (ts#64115,
//! ts#64291).

use crate::prelude::*;

use super::requestfilesystem::{RequestFileSystem, RequestFileSystemImpl, get_request_file_system};
use crate::frontend::tspath;
use crate::frontend::vfs;
use crate::ls::lsconv;
use crate::lsp::lsproto;
use crate::project::{self, FileHandle as _};

// Go: api/requestfilesystem/filechanges.go requestFileSystem.ExpandFileChanges
// (project.FileChangeExpander)
impl project::FileChangeExpander for RequestFileSystemImpl {
    fn expand_file_changes(
        &self,
        mut summary: project::FileChangeSummary,
    ) -> project::FileChangeSummary {
        let expand = |uris: &mut FxHashSet<lsproto::DocumentUri>| {
            let mut additional: FxHashSet<lsproto::DocumentUri> = FxHashSet::default();
            for uri in uris.iter() {
                for alias in self.aliases_for_path(&uri.file_name()) {
                    additional.insert(lsconv::file_name_to_document_uri(&alias));
                }
            }
            for uri in additional {
                uris.insert(uri);
            }
        };
        expand(&mut summary.changed);
        expand(&mut summary.created);
        expand(&mut summary.deleted);
        summary
    }
}

// Go: api/requestfilesystem/filechanges.go addFileChanges
// PORT: Go map order is random; the names are walked in sorted order.
pub fn add_file_changes(
    summary: &mut project::FileChangeSummary,
    request: &RequestFileSystem,
    base_fs: &dyn vfs::Fs,
    file_system: &RequestFileSystemImpl,
    current_directory: &str,
) {
    let to_path = |file_name: &str| {
        tspath::to_path(
            file_name,
            current_directory,
            base_fs.use_case_sensitive_file_names(),
        )
    };
    let base_request_fs = get_request_file_system(base_fs);
    let add_change = |summary: &mut project::FileChangeSummary, file_name: &str, deleted: bool| {
        let uri = lsconv::file_name_to_document_uri(file_name);
        if deleted {
            if base_fs.file_exists(file_name) || base_fs.directory_exists(file_name) {
                summary.deleted.insert(uri);
            }
            return;
        }
        if base_fs.file_exists(file_name) {
            summary.changed.insert(uri);
        } else {
            summary.created.insert(uri);
        }
    };
    let add_change_and_aliases =
        |summary: &mut project::FileChangeSummary, file_name: &str, deleted: bool| {
            add_change(summary, file_name, deleted);
            if let Some(base_request_fs) = &base_request_fs {
                for alias in base_request_fs.aliases_for_path(file_name) {
                    add_change(summary, &alias, deleted);
                }
            }
        };
    let mut file_names: Vec<&String> = request.files.keys().collect();
    file_names.sort();
    let mut overlay_files: FxHashSet<tspath::Path> =
        FxHashSet::with_capacity_and_hasher(request.files.len(), Default::default());
    for file_name in file_names {
        // ts#64159 (Go N' filechanges.go:57): `tspath.ToRootedFilePath`.
        let absolute_file_name = crate::api::to_rooted_path(file_name, current_directory);
        overlay_files.insert(to_path(&absolute_file_name));
        add_change_and_aliases(summary, &absolute_file_name, false);
    }
    for removed_path in &request.removed_paths {
        // ts#64159 (Go N' filechanges.go:62): `tspath.ToRootedPath`.
        let absolute_file_name = crate::api::to_rooted_path(removed_path, current_directory);
        if overlay_files.contains(&to_path(&absolute_file_name)) {
            continue;
        }
        add_change_and_aliases(summary, &absolute_file_name, true);
    }
    // Replacing a listing or a symlink can change every cached descendant.
    // Delete events expand through the snapshot's cached directory tree and create
    // events that refresh wildcard roots and previously missing module resolutions.
    let add_replacement = |summary: &mut project::FileChangeSummary, path: &str| {
        // ts#64159 (Go N' filechanges.go:72): `tspath.ToRootedPath`.
        let absolute_path = crate::api::to_rooted_path(path, current_directory);
        add_change_and_aliases(summary, &absolute_path, true);
        summary
            .created
            .insert(lsconv::file_name_to_document_uri(&absolute_path));
        if let Some(base_request_fs) = &base_request_fs {
            for alias in base_request_fs.aliases_for_path(&absolute_path) {
                summary
                    .created
                    .insert(lsconv::file_name_to_document_uri(&alias));
            }
        }
    };
    let mut directory_names: Vec<&String> = request.directories.keys().collect();
    directory_names.sort();
    for directory_name in directory_names {
        add_replacement(summary, directory_name);
    }
    let mut link_names: Vec<&String> = request.symlinks.keys().collect();
    link_names.sort();
    for link_name in link_names {
        add_replacement(summary, link_name);
    }
    if let Some(layered_base) = project::as_layered_file_system(base_fs) {
        let overlays = project::LayeredFileSystem::overlays(file_system);
        for (path, overlay) in layered_base.overlays().iter() {
            if overlays.contains_key(path) {
                continue;
            }
            let uri = lsconv::file_name_to_document_uri(&overlay.file_name());
            if summary.closed.contains(&uri) {
                continue;
            }
            summary.created.remove(&uri);
            if vfs::Fs::file_exists(file_system, &overlay.file_name()) {
                summary.deleted.remove(&uri);
                summary.changed.insert(uri);
            } else {
                summary.changed.remove(&uri);
                summary.deleted.insert(uri);
            }
        }
    }
    if summary.changed.len() + summary.created.len() + summary.deleted.len() > 0 {
        summary.includes_watch_change_outside_node_modules = true;
    }
}
