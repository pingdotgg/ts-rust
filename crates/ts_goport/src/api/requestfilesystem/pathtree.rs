//! Port of Go `internal/api/requestfilesystem/pathtree.go` (ts#64115,
//! ts#64291).
//!
//! PORT: Go `*requestPathNode` is shared between trees (`composeRequestPaths`
//! copies a node and keeps its children), so a node is
//! `Rc<RefCell<RequestPathNode>>`. Nodes are only written while
//! `newRequestFileSystemWorker` builds a tree; later trees share them.
//! Go map order is random; the code that returns results sorts them, as Go
//! does.

use crate::prelude::*;

use crate::frontend::tspath;
use crate::frontend::vfs;

// Go: api/requestfilesystem/pathtree.go requestFallback
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RequestFallback {
    #[default]
    Inherit,
    Allowed,
    Missing,
}

// Go: api/requestfilesystem/pathtree.go requestEntry
// PORT: the Go interface is an enum of its three pointer types. `None` is
// the nil interface.
#[derive(Clone, Debug)]
pub enum RequestEntry {
    File(Rc<RequestFile>),
    Symlink(Rc<RequestSymlinkEntry>),
    Directory(Rc<RequestDirectory>),
}

// Go: api/requestfilesystem/pathtree.go requestFile
#[derive(Clone, Debug, Default)]
pub struct RequestFile {
    pub file_name: String,
    pub content: String,
}

// Go: api/requestfilesystem/pathtree.go requestSymlink
// PORT: named `RequestSymlinkEntry`; `RequestSymlink` is the exported
// request type (requestfilesystem.go).
#[derive(Clone, Debug, Default)]
pub struct RequestSymlinkEntry {
    pub link_name: String,
    pub target: String,
    pub host: bool,
}

// Go: api/requestfilesystem/pathtree.go requestDirectory
#[derive(Clone, Debug, Default)]
pub struct RequestDirectory {
    pub directory_name: String,
    pub listing: Option<vfs::Entries>,
}

// Go: api/requestfilesystem/pathtree.go requestFile (vfs.FileInfo methods)
// PORT: Go `*requestFile` is a `vfs.FileInfo`; the port makes the value.
impl RequestFile {
    pub fn file_info(&self) -> vfs::FileInfo {
        vfs::FileInfo {
            name: tspath::get_base_file_name(&self.file_name),
            size: self.content.len() as i64,
            mode: vfs::FileMode(0o444),
            mod_time: None,
        }
    }
}

// Go: api/requestfilesystem/pathtree.go requestDirectory (vfs.FileInfo methods)
impl RequestDirectory {
    pub fn file_info(&self) -> vfs::FileInfo {
        vfs::FileInfo {
            name: tspath::get_base_file_name(&self.directory_name),
            size: 0,
            mode: vfs::FileMode::DIR | vfs::FileMode(0o555),
            mod_time: None,
        }
    }
}

/// Go `node.entry.(vfs.FileInfo)`: a file or a directory entry is a
/// `vfs.FileInfo`; a symlink entry is not.
// PORT: the entry stays an entry, so a caller can still take the file
// content (Go `info.(*requestFile)`).
#[derive(Clone, Debug)]
pub enum RequestInfo {
    File(Rc<RequestFile>),
    Directory(Rc<RequestDirectory>),
}

impl RequestInfo {
    pub fn is_dir(&self) -> bool {
        matches!(self, RequestInfo::Directory(_))
    }

    pub fn file_info(&self) -> vfs::FileInfo {
        match self {
            RequestInfo::File(file) => file.file_info(),
            RequestInfo::Directory(directory) => directory.file_info(),
        }
    }
}

pub type RequestPathNodeRef = Rc<RefCell<RequestPathNode>>;

// Go: api/requestfilesystem/pathtree.go requestPathNode
#[derive(Clone, Debug, Default)]
pub struct RequestPathNode {
    pub entry: Option<RequestEntry>,
    pub fallback: RequestFallback,
    // PORT: Go nil map is `None`.
    pub children: Option<FxHashMap<tspath::Path, RequestPathNodeRef>>,
    pub has_symlinks: bool,
}

impl RequestPathNode {
    // Go: api/requestfilesystem/pathtree.go requestPathNode.replacesSubtree
    pub fn replaces_subtree(&self) -> bool {
        matches!(
            self.entry,
            Some(RequestEntry::File(_)) | Some(RequestEntry::Symlink(_))
        )
    }

    fn child(&self, path: &tspath::Path) -> Option<RequestPathNodeRef> {
        self.children.as_ref().and_then(|c| c.get(path)).cloned()
    }
}

// Go: api/requestfilesystem/pathtree.go requestPathAncestors
pub fn request_path_ancestors(path: &tspath::Path) -> Vec<tspath::Path> {
    let mut paths = Vec::new();
    let mut path = path.clone();
    loop {
        paths.push(path.clone());
        let parent = tspath::Path(tspath::get_directory_path(path.as_str()));
        if parent == path {
            break;
        }
        path = parent;
    }
    paths.reverse();
    paths
}

// Go: api/requestfilesystem/pathtree.go requestPathNode.ensure
pub fn ensure(node: &RequestPathNodeRef, path: &tspath::Path) -> RequestPathNodeRef {
    let mut node = node.clone();
    for ancestor in request_path_ancestors(path) {
        let child = {
            let mut n = node.borrow_mut();
            let children = n.children.get_or_insert_with(FxHashMap::default);
            children
                .entry(ancestor)
                .or_insert_with(|| Rc::new(RefCell::new(RequestPathNode::default())))
                .clone()
        };
        node = child;
    }
    node
}

// Go: api/requestfilesystem/pathtree.go requestPathNode.lookup
// PORT: Go nil `node` is `None`.
pub fn lookup(
    node: Option<&RequestPathNodeRef>,
    path: &tspath::Path,
) -> (Option<RequestPathNodeRef>, RequestFallback) {
    let mut fallback = RequestFallback::Inherit;
    let mut node = node.cloned();
    for ancestor in request_path_ancestors(path) {
        let Some(current) = node.clone() else {
            break;
        };
        let current = current.borrow();
        if current.fallback != RequestFallback::Inherit {
            fallback = current.fallback;
        }
        node = current.child(&ancestor);
    }
    if let Some(node) = &node
        && node.borrow().fallback != RequestFallback::Inherit
    {
        fallback = node.borrow().fallback;
    }
    (node, fallback)
}

// Go: api/requestfilesystem/pathtree.go requestPathNode.walkSymlinks
pub fn walk_symlinks(
    node: Option<&RequestPathNodeRef>,
    visit: &mut dyn FnMut(&tspath::Path, &Rc<RequestSymlinkEntry>),
) {
    let Some(node) = node else {
        return;
    };
    let node = node.borrow();
    if !node.has_symlinks {
        return;
    }
    let Some(children) = &node.children else {
        return;
    };
    for (path, child) in children {
        if let Some(RequestEntry::Symlink(symlink)) = &child.borrow().entry {
            visit(path, symlink);
        }
        walk_symlinks(Some(child), visit);
    }
}

// Go: api/requestfilesystem/pathtree.go requestPathNode.entries
pub fn entries(node: Option<&RequestPathNodeRef>) -> (vfs::Entries, bool) {
    let Some(node) = node else {
        return (vfs::Entries::default(), false);
    };
    let node = node.borrow();
    let Some(RequestEntry::Directory(directory)) = &node.entry else {
        return (vfs::Entries::default(), false);
    };
    if let Some(listing) = &directory.listing {
        return (super::requestfilesystem::clone_entries(listing), true);
    }
    let mut entries = vfs::Entries::default();
    if let Some(children) = &node.children {
        for child in children.values() {
            match &child.borrow().entry {
                Some(RequestEntry::File(entry)) => {
                    entries
                        .files
                        .push(tspath::get_base_file_name(&entry.file_name));
                }
                Some(RequestEntry::Directory(entry)) => {
                    entries
                        .directories
                        .push(tspath::get_base_file_name(&entry.directory_name));
                }
                _ => {}
            }
        }
    }
    entries.files.sort();
    entries.directories.sort();
    (entries, true)
}

// Go: api/requestfilesystem/pathtree.go composeRequestPaths
pub fn compose_request_paths(
    base: Option<RequestPathNodeRef>,
    overlay: Option<&RequestPathNodeRef>,
    mut fallback: RequestFallback,
    case_sensitive: bool,
) -> Option<RequestPathNodeRef> {
    let Some(overlay) = overlay else {
        return base;
    };
    let overlay = overlay.borrow();
    let mut base = base;
    if overlay.fallback != RequestFallback::Inherit {
        fallback = overlay.fallback;
        base = None;
    }
    if overlay.replaces_subtree() {
        base = None;
    }
    let mut result = RequestPathNode::default();
    if let Some(base) = &base {
        result = base.borrow().clone();
    }
    // Go `maps.Clone` of the children: the map is copied, the nodes shared.
    if overlay.fallback != RequestFallback::Inherit || overlay.replaces_subtree() {
        result.fallback = fallback;
    }
    let previous_directory = match &result.entry {
        Some(RequestEntry::Directory(directory)) => Some(directory.clone()),
        _ => None,
    };
    let overlay_directory = match &overlay.entry {
        Some(RequestEntry::Directory(directory)) => Some(directory.clone()),
        _ => None,
    };
    if let Some(entry) = &overlay.entry {
        result.entry = Some(entry.clone());
        if let Some(overlay_directory) = &overlay_directory
            && overlay_directory.listing.is_none()
            && let Some(previous_directory) = &previous_directory
        {
            result.entry = Some(RequestEntry::Directory(Rc::new(RequestDirectory {
                directory_name: overlay_directory.directory_name.clone(),
                listing: previous_directory.listing.clone(),
            })));
        }
    }
    if let Some(overlay_children) = &overlay.children {
        for (path, child) in overlay_children {
            let children = result.children.get_or_insert_with(FxHashMap::default);
            let composed = compose_request_paths(
                children.get(path).cloned(),
                Some(child),
                fallback,
                case_sensitive,
            );
            match composed {
                Some(composed) => {
                    children.insert(path.clone(), composed);
                }
                None => {
                    // PORT: Go stores the nil result in the map.
                    children.remove(path);
                }
            }
        }
    }
    if let Some(RequestEntry::Directory(directory)) = &result.entry
        && let Some(listing) = &directory.listing
        && overlay_directory
            .as_ref()
            .is_none_or(|directory| directory.listing.is_none())
    {
        let mut entries = super::requestfilesystem::clone_entries(listing);
        let equal = |left: &str, right: &str| {
            tspath::get_canonical_file_name(left, case_sensitive)
                == tspath::get_canonical_file_name(right, case_sensitive)
        };
        if let Some(overlay_children) = &overlay.children {
            for (path, child) in overlay_children {
                let name = tspath::get_base_file_name(path.as_str());
                let child = child.borrow();
                if child.fallback == RequestFallback::Missing || child.replaces_subtree() {
                    entries.files.retain(|entry| !equal(entry, &name));
                    entries.directories.retain(|entry| !equal(entry, &name));
                    if let Some(symlinks) = &mut entries.symlinks {
                        symlinks.retain(|entry| !equal(entry, &name));
                    }
                }
                match &child.entry {
                    Some(RequestEntry::File(entry)) => {
                        entries = super::requestfilesystem::merge_entries(
                            &entries,
                            &vfs::Entries {
                                files: vec![tspath::get_base_file_name(&entry.file_name)],
                                ..Default::default()
                            },
                            &equal,
                        );
                    }
                    Some(RequestEntry::Directory(entry)) => {
                        entries = super::requestfilesystem::merge_entries(
                            &entries,
                            &vfs::Entries {
                                directories: vec![tspath::get_base_file_name(
                                    &entry.directory_name,
                                )],
                                ..Default::default()
                            },
                            &equal,
                        );
                    }
                    _ => {}
                }
            }
        }
        result.entry = Some(RequestEntry::Directory(Rc::new(RequestDirectory {
            directory_name: directory.directory_name.clone(),
            listing: Some(entries),
        })));
    }
    result.has_symlinks = matches!(result.entry, Some(RequestEntry::Symlink(_)));
    if let Some(children) = &result.children {
        for child in children.values() {
            result.has_symlinks = result.has_symlinks || child.borrow().has_symlinks;
        }
    }
    Some(Rc::new(RefCell::new(result)))
}

// Go: api/requestfilesystem/pathtree.go requestPathNode.firstSymlink
pub fn first_symlink(
    node: Option<&RequestPathNodeRef>,
    path: &tspath::Path,
) -> Option<(tspath::Path, Rc<RequestSymlinkEntry>)> {
    let mut node = node.cloned();
    for ancestor in request_path_ancestors(path) {
        let Some(current) = node.clone() else {
            break;
        };
        node = current.borrow().child(&ancestor);
        if let Some(next) = &node
            && let Some(RequestEntry::Symlink(symlink)) = &next.borrow().entry
        {
            return Some((ancestor, symlink.clone()));
        }
    }
    None
}

// Go: api/requestfilesystem/pathtree.go requestPathNode.containsFileAncestor
pub fn contains_file_ancestor(node: Option<&RequestPathNodeRef>, path: &tspath::Path) -> bool {
    let mut node = node.cloned();
    for ancestor in request_path_ancestors(path) {
        let Some(current) = node.clone() else {
            return false;
        };
        node = current.borrow().child(&ancestor);
        if ancestor != *path
            && let Some(next) = &node
            && matches!(next.borrow().entry, Some(RequestEntry::File(_)))
        {
            return true;
        }
    }
    false
}

// Go: api/requestfilesystem/pathtree.go:265 requestPathContains
pub fn request_path_contains(parent: &tspath::Path, path: &tspath::Path) -> bool {
    parent.contains_path(path)
}
