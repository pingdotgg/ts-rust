//! Go `internal/project/overlayfs.go`.
//!
//! PORT: one thread (project/dirty/interfaces.rs). Go `sync.Once` + value is
//! a `OnceCell`; `mu` is dropped. Go `xxh3.Uint128` is `u128`. A Go
//! `FileHandle` is `Rc<dyn FileHandle>` (nil is `None`): an `*Overlay` is
//! `Rc<Overlay>` and a `*cachedFile` is `Rc<RefCell<CachedFile>>` (the dirty maps
//! change it after sharing). Go `string` results are owned `String`s, except
//! the file text: a file keeps it as an `Arc<str>`, which a Go string is (an
//! immutable shared value), so a reader that only reads it takes
//! `shared_content` and does not copy it.

use crate::project::prelude::*;

use crate::frontend::core_ext::get_script_kind_from_file_name;
use crate::frontend::core_textchange::TextChange;
use std::cell::{Cell, OnceCell};
use std::sync::Arc;
use xxhash_rust::xxh3::xxh3_128;

// Go: project/overlayfs.go:22 FileContent
pub trait FileContent {
    /// Go `Content`: an owned copy of the text.
    fn content(&self) -> String {
        self.shared_content().to_string()
    }
    /// Go `Content` without the copy: the text that the file holds. Use it
    /// where the text is read per result (a language service reads a file
    /// once per location it converts).
    fn shared_content(&self) -> Arc<str>;
    fn hash(&self) -> u128;
}

// Go: project/overlayfs.go:27 FileHandle
pub trait FileHandle: FileContent {
    fn file_name(&self) -> String;
    fn version(&self) -> i32;
    fn matches_disk_text(&self) -> bool;
    fn is_overlay(&self) -> bool;
    fn lsp_line_map(&self) -> Rc<lsconv::LSPLineMap>;
    fn ecma_line_info(&self) -> Rc<sourcemap::lineinfo::ECMALineInfo>;
    fn kind(&self) -> ScriptKind;
}

// Go: project/overlayfs.go:38 fileBase
// PORT: `hash` is a `Cell` because Go writes it on an overlay that may be
// shared (processChanges). `content` is an `Arc<str>`: a Go string is shared,
// and a clone of the file (Go `Clone`, a new overlay with the same text)
// shares it too.
#[derive(Debug, Default)]
pub struct FileBase {
    pub file_name: String,
    pub content: Arc<str>,
    pub hash: Cell<u128>,

    pub line_map: OnceCell<Rc<lsconv::LSPLineMap>>,
    pub line_info: OnceCell<Rc<sourcemap::lineinfo::ECMALineInfo>>,
}

impl FileBase {
    // Go: project/overlayfs.go:49 fileBase.FileName
    pub fn file_name(&self) -> String {
        self.file_name.clone()
    }

    // Go: project/overlayfs.go:53 fileBase.Hash
    pub fn hash(&self) -> u128 {
        self.hash.get()
    }

    // Go: project/overlayfs.go:57 fileBase.Content
    pub fn content(&self) -> String {
        self.content.to_string()
    }

    /// Go `fileBase.Content` without the copy (`FileContent::shared_content`).
    pub fn shared_content(&self) -> Arc<str> {
        Arc::clone(&self.content)
    }

    // Go: project/overlayfs.go:61 fileBase.LSPLineMap
    pub fn lsp_line_map(&self) -> Rc<lsconv::LSPLineMap> {
        self.line_map
            .get_or_init(|| lsconv::compute_lsp_line_starts(&self.content))
            .clone()
    }

    // Go: project/overlayfs.go:68 fileBase.ECMALineInfo
    pub fn ecma_line_info(&self) -> Rc<sourcemap::lineinfo::ECMALineInfo> {
        self.line_info
            .get_or_init(|| {
                let line_starts = compute_ecma_line_starts(&self.content);
                Rc::new(sourcemap::lineinfo::create_ecma_line_info(
                    &self.content,
                    line_starts,
                ))
            })
            .clone()
    }
}

// Go: project/overlayfs.go:76 cachedFile (ts#64291: was diskFile)
// PORT: Go embeds `fileBase`; here it is the field `file_base`.
#[derive(Debug, Default)]
pub struct CachedFile {
    pub file_base: FileBase,
    pub needs_reload: bool,
    pub realpath_path: tspath::Path,
}

// Go: project/overlayfs.go:82 newCachedFile
// PORT: the file keeps `content`. An `Arc<str>` is shared, a `String` or
// `&str` is copied once.
pub fn new_cached_file(file_name: &str, content: impl Into<Arc<str>>) -> Rc<RefCell<CachedFile>> {
    let content: Arc<str> = content.into();
    let hash = xxh3_128(content.as_bytes());
    Rc::new(RefCell::new(CachedFile {
        file_base: FileBase {
            file_name: file_name.to_string(),
            content,
            hash: Cell::new(hash),
            ..FileBase::default()
        },
        ..CachedFile::default()
    }))
}

// Go: project/overlayfs.go:90 NewCachedFileHandle (ts#64291)
pub fn new_cached_file_handle(file_name: &str, content: impl Into<Arc<str>>) -> Rc<dyn FileHandle> {
    new_cached_file(file_name, content)
}

impl CachedFile {
    // Go: project/overlayfs.go:96 cachedFile.Version
    pub fn version(&self) -> i32 {
        0
    }

    // Go: project/overlayfs.go:100 cachedFile.MatchesDiskText
    pub fn matches_disk_text(&self) -> bool {
        !self.needs_reload
    }

    // Go: project/overlayfs.go:104 cachedFile.IsOverlay
    pub fn is_overlay(&self) -> bool {
        false
    }

    // Go: project/overlayfs.go:108 cachedFile.Kind
    // PORT: tsgo #4712 reverts #4628 here. An extensionless file keeps
    // `ScriptKind::UNKNOWN`; `new_parse_cache_key` picks TS for the parse.
    pub fn kind(&self) -> ScriptKind {
        get_script_kind_from_file_name(&self.file_base.file_name)
    }

    // Go: project/overlayfs.go:112 cachedFile.Clone
    // PORT: Go `Clone`; `clone_` keeps it apart from `std::clone::Clone`.
    pub fn clone_(&self) -> Rc<RefCell<CachedFile>> {
        Rc::new(RefCell::new(CachedFile {
            realpath_path: self.realpath_path.clone(),
            file_base: FileBase {
                file_name: self.file_base.file_name.clone(),
                content: Arc::clone(&self.file_base.content),
                hash: Cell::new(self.file_base.hash.get()),
                ..FileBase::default()
            },
            ..CachedFile::default()
        }))
    }
}

// PORT: the dirty maps hold `*cachedFile` and call its `Clone`.
impl dirty::Cloneable for Rc<RefCell<CachedFile>> {
    fn clone_(&self) -> Self {
        self.borrow().clone_()
    }
}

// Go: project/overlayfs.go:96 `var _ FileHandle = (*cachedFile)(nil)`
// PORT: the methods of `fileBase` are promoted through the embedding.
impl FileContent for RefCell<CachedFile> {
    fn shared_content(&self) -> Arc<str> {
        self.borrow().file_base.shared_content()
    }

    fn hash(&self) -> u128 {
        self.borrow().file_base.hash()
    }
}

impl FileHandle for RefCell<CachedFile> {
    fn file_name(&self) -> String {
        self.borrow().file_base.file_name()
    }

    fn version(&self) -> i32 {
        self.borrow().version()
    }

    fn matches_disk_text(&self) -> bool {
        self.borrow().matches_disk_text()
    }

    fn is_overlay(&self) -> bool {
        self.borrow().is_overlay()
    }

    fn lsp_line_map(&self) -> Rc<lsconv::LSPLineMap> {
        self.borrow().file_base.lsp_line_map()
    }

    fn ecma_line_info(&self) -> Rc<sourcemap::lineinfo::ECMALineInfo> {
        self.borrow().file_base.ecma_line_info()
    }

    fn kind(&self) -> ScriptKind {
        self.borrow().kind()
    }
}

// Go: project/overlayfs.go:123 Overlay
// PORT: Go embeds `fileBase`; here it is the field `file_base`. `version`
// and `matches_disk_text` are `Cell`s because Go writes them on an overlay
// that may be shared (processChanges).
#[derive(Debug, Default)]
pub struct Overlay {
    pub file_base: FileBase,
    pub version: Cell<i32>,
    pub kind: ScriptKind,
    pub matches_disk_text: Cell<bool>,
}

// Go: project/overlayfs.go:130 newOverlay
// PORT: the overlay keeps `content`. An `Arc<str>` is shared, a `String` or
// `&str` is copied once.
pub fn new_overlay(
    file_name: &str,
    content: impl Into<Arc<str>>,
    version: i32,
    kind: ScriptKind,
) -> Overlay {
    let content: Arc<str> = content.into();
    let hash = xxh3_128(content.as_bytes());
    Overlay {
        file_base: FileBase {
            file_name: file_name.to_string(),
            content,
            hash: Cell::new(hash),
            ..FileBase::default()
        },
        version: Cell::new(version),
        kind,
        ..Overlay::default()
    }
}

impl Overlay {
    // Go: project/overlayfs.go:144 Overlay.Text
    pub fn text(&self) -> String {
        self.file_base.content.to_string()
    }

    // Go: project/overlayfs.go:163 Overlay.computeMatchesDiskText
    // !!! optimization: incorporate mtime
    // PORT: Go named results `(matchesDiskText bool, exists bool)`.
    pub fn compute_matches_disk_text(&self, fs: &dyn vfs::Fs) -> (bool, bool) {
        if tspath::is_dynamic_file_name(&self.file_base.file_name) {
            return (false, false);
        }
        let (disk_content, ok) = fs.read_file(&self.file_base.file_name);
        if !ok {
            return (false, false);
        }
        (
            xxh3_128(disk_content.as_bytes()) == self.file_base.hash(),
            true,
        )
    }
}

// Go: project/overlayfs.go:116 `var _ FileHandle = (*Overlay)(nil)`
// PORT: the methods of `fileBase` are promoted through the embedding.
impl FileContent for Overlay {
    fn shared_content(&self) -> Arc<str> {
        self.file_base.shared_content()
    }

    fn hash(&self) -> u128 {
        self.file_base.hash()
    }
}

impl FileHandle for Overlay {
    fn file_name(&self) -> String {
        self.file_base.file_name()
    }

    // Go: project/overlayfs.go:140 Overlay.Version
    fn version(&self) -> i32 {
        self.version.get()
    }

    // Go: project/overlayfs.go:158 Overlay.MatchesDiskText
    // MatchesDiskText may return false negatives, but never false positives.
    fn matches_disk_text(&self) -> bool {
        self.matches_disk_text.get()
    }

    // Go: project/overlayfs.go:174 Overlay.IsOverlay
    fn is_overlay(&self) -> bool {
        true
    }

    fn lsp_line_map(&self) -> Rc<lsconv::LSPLineMap> {
        self.file_base.lsp_line_map()
    }

    fn ecma_line_info(&self) -> Rc<sourcemap::lineinfo::ECMALineInfo> {
        self.file_base.ecma_line_info()
    }

    // Go: project/overlayfs.go:178 Overlay.Kind
    fn kind(&self) -> ScriptKind {
        self.kind
    }
}

// PORT: Go passes an `*Overlay` to `lsconv.FromLSPRange` as an
// `lsconv.Script` (its `FileName` and `Text` methods and the tsgo#4712
// methods below).
impl lsconv::Script for Overlay {
    fn file_name(&self) -> &str {
        &self.file_base.file_name
    }

    fn text(&self) -> lsconv::ScriptText<'_> {
        lsconv::ScriptText::Borrowed(&self.file_base.content)
    }

    // Go: project/overlayfs.go:148 Overlay.OriginalFileName (tsgo#4712)
    fn original_file_name(&self) -> &str {
        &self.file_base.file_name
    }

    // Go: project/overlayfs.go:153 Overlay.SpanMap (tsgo#4712)
    // SpanMap and OriginalText satisfy lsconv.Script. An overlay holds the editor's raw text (for a
    // content-mapped file, that is the original foreign text, not the transformed output), so it never
    // carries a span map and its original text is its own text.
    fn span_map(&self) -> Option<&crate::spanmap::SpanMap> {
        None
    }

    // Go: project/overlayfs.go:155 Overlay.OriginalText (tsgo#4712)
    fn original_text(&self) -> lsconv::ScriptText<'_> {
        lsconv::ScriptText::Borrowed(&self.file_base.content)
    }
}

// Go: project/overlayfs.go:182 overlayFS
// PORT: `mu` is dropped; `overlays` and `overlayDirectories` are replaced
// after sharing, so they are `RefCell`s. Go `map[tspath.Path]*Overlay` is an
// `IndexMap` (insertion order; PORT: Go map order is random), because the
// project collection ranges over the overlays when it picks inferred project
// roots. Go shares the map (a reference), so it is an `Rc` map here.
pub struct OverlayFS {
    pub to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    pub host: Rc<dyn vfs::Fs>,
    pub position_encoding: lsproto::PositionEncodingKind,

    pub overlays: RefCell<Rc<IndexMap<tspath::Path, Rc<Overlay>>>>,
    pub overlay_directories: RefCell<Rc<OverlayDirectories>>,
}

/// Go `map[tspath.Path]map[tspath.Path]string` of `overlayFS.overlayDirectories`.
// PORT: Go ranges over a directory map (`GetAccessibleEntries`). A Go map
// of at most 8 entries is one group of 8 slots: an insert takes the first
// free slot, a delete frees its slot, and a range starts at a random slot
// and wraps. Go makes the directory maps again from `overlays` at each
// change (`createOverlayDirectories`) and never deletes from them. So the
// slots hold the children in the order of the range over `overlays`, and Go
// gives a rotation of that order. The inner map is an `IndexMap` (insertion
// order), filled in the order of the port's `overlays`. When that order is
// Go's slot order of `overlays`, the port gives the rotation that starts at
// the first slot, which Go can give. Go can differ in two cases:
// - A close and then an open: Go puts the new overlay in the free slot of
//   the closed one, but the port puts it last (`shift_remove`, then insert).
// - Above 8 entries (in `overlays` or in one directory): Go's order depends
//   on the hash seed of the map.
// An FxHashMap gave orders that Go never gives, also with no close
// (editfuzz2 R1).
pub type OverlayDirectories = FxHashMap<tspath::Path, IndexMap<tspath::Path, String>>;

// Go: project/overlayfs.go:192 LayeredFileSystem (ts#64291)
pub trait LayeredFileSystem: vfs::Fs + FileHandleSource {
    fn overlays(&self) -> Rc<IndexMap<tspath::Path, Rc<Overlay>>>;
}

// Go: project/overlayfs.go:198 RebasableFileSystem (ts#64291)
pub trait RebasableFileSystem: vfs::Fs {
    fn base_file_system(&self) -> Rc<dyn vfs::Fs>;
    fn with_base_file_system(&self, base: Rc<dyn vfs::Fs>) -> Rc<dyn LayeredFileSystem>;
}

/// PORT: the Go type assertions on a `vfs.FS` value that the file system
/// layers use (ts#64291): `fs.(FileHandleSource)`, `fs.(LayeredFileSystem)`,
/// `fs.(RebasableFileSystem)`, `fs.(FileChangeExpander)` and
/// `fs.(*overlayFS)` (and, through `as_any`, the other packages' concrete
/// layer types). `as_fs_layer` returns this view for a layer; each
/// default is a failed assertion.
pub trait FsLayer {
    fn as_file_handle_source(&self) -> Option<&dyn FileHandleSource> {
        None
    }
    fn as_layered_file_system(&self) -> Option<&dyn LayeredFileSystem> {
        None
    }
    fn as_rebasable_file_system(&self) -> Option<&dyn RebasableFileSystem> {
        None
    }
    fn as_file_change_expander(&self) -> Option<&dyn FileChangeExpander> {
        None
    }
    fn as_overlay_fs(&self) -> Option<&OverlayFS> {
        None
    }
    /// A concrete layer type (Go `fs.(*T)` in another package).
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }
}

/// PORT: Go type assertions on a `vfs.FS` value (ts#64291). `vfs::Fs` is in
/// `goport_util`, which cannot name the layer types, so a layer returns itself
/// from `vfs::Fs::as_any` and this downcasts it to each concrete layer type.
/// Wrappers (`CachedFs`, `TrackingFs`, `wrapvfs`) keep the `None` default, as
/// Go asserts on the wrapper's own type.
pub fn as_fs_layer(fs: &dyn vfs::Fs) -> Option<&dyn FsLayer> {
    let fs = fs.as_any()?;
    if let Some(fs) = fs.downcast_ref::<OverlayFS>() {
        return Some(fs);
    }
    if let Some(fs) = fs.downcast_ref::<CachedLayeredFileSystem>() {
        return Some(fs);
    }
    if let Some(fs) = fs.downcast_ref::<crate::api::requestfilesystem::RequestFileSystemImpl>() {
        return Some(fs);
    }
    None
}

/// Go `fs.(FileHandleSource)` on a `vfs.FS`.
pub fn as_file_handle_source(fs: &dyn vfs::Fs) -> Option<&dyn FileHandleSource> {
    as_fs_layer(fs)?.as_file_handle_source()
}

/// Go `fs.(LayeredFileSystem)` on a `vfs.FS`.
pub fn as_layered_file_system(fs: &dyn vfs::Fs) -> Option<&dyn LayeredFileSystem> {
    as_fs_layer(fs)?.as_layered_file_system()
}

/// Go `fs.(RebasableFileSystem)` on a `vfs.FS`.
pub fn as_rebasable_file_system(fs: &dyn vfs::Fs) -> Option<&dyn RebasableFileSystem> {
    as_fs_layer(fs)?.as_rebasable_file_system()
}

/// Go `fs.(FileChangeExpander)` on a `vfs.FS`.
pub fn as_file_change_expander(fs: &dyn vfs::Fs) -> Option<&dyn FileChangeExpander> {
    as_fs_layer(fs)?.as_file_change_expander()
}

/// Go `fs.(*overlayFS)` on a `vfs.FS`.
pub fn as_overlay_fs(fs: &dyn vfs::Fs) -> Option<&OverlayFS> {
    as_fs_layer(fs)?.as_overlay_fs()
}

// Go: project/overlayfs.go:204 newOverlayFS
pub fn new_overlay_fs(
    fs: Rc<dyn vfs::Fs>,
    overlays: IndexMap<tspath::Path, Rc<Overlay>>,
    position_encoding: lsproto::PositionEncodingKind,
    to_path: Rc<dyn Fn(&str) -> tspath::Path>,
) -> Rc<OverlayFS> {
    let overlay_directories = create_overlay_directories(&overlays);
    Rc::new(OverlayFS {
        host: fs,
        position_encoding,
        overlays: RefCell::new(Rc::new(overlays)),
        overlay_directories: RefCell::new(Rc::new(overlay_directories)),
        to_path,
    })
}

impl OverlayFS {
    // Go: project/overlayfs.go:220 overlayFS.Overlays
    pub fn overlays(&self) -> Rc<IndexMap<tspath::Path, Rc<Overlay>>> {
        self.overlays.borrow().clone()
    }

    // Go: project/overlayfs.go:243 overlayFS.GetFile (ts#64291: was getFile)
    pub fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>> {
        self.get_file_by_path(file_name, &(self.to_path)(file_name))
    }

    // Go: project/overlayfs.go:247 overlayFS.GetFileByPath
    pub fn get_file_by_path(
        &self,
        file_name: &str,
        path: &tspath::Path,
    ) -> Option<Rc<dyn FileHandle>> {
        let overlay = self.overlays.borrow().get(path).cloned();
        let directory = self.overlay_directories.borrow().contains_key(path);
        if let Some(overlay) = overlay {
            return Some(overlay);
        }
        if directory {
            return None;
        }

        if let Some(source) = as_file_handle_source(&*self.host) {
            return source.get_file_by_path(file_name, path);
        }
        let (content, ok) = self.host.read_file(file_name);
        if !ok {
            return None;
        }
        Some(new_cached_file(file_name, content))
    }
}

// Go: project/overlayfs.go:226 layerOverlayFileSystem (ts#64291)
pub fn layer_overlay_file_system(
    file_system: Rc<dyn vfs::Fs>,
    overlays: IndexMap<tspath::Path, Rc<Overlay>>,
    position_encoding: lsproto::PositionEncodingKind,
    to_path: Rc<dyn Fn(&str) -> tspath::Path>,
) -> Rc<dyn LayeredFileSystem> {
    let mut base = file_system.clone();
    let layer = as_rebasable_file_system(&*file_system);
    if let Some(candidate) = layer {
        base = candidate.base_file_system();
    }
    if let Some(previous) = as_overlay_fs(&*base) {
        let host = previous.host.clone();
        base = host;
    }
    let overlay = new_overlay_fs(base, overlays, position_encoding, to_path);
    match layer {
        None => overlay,
        Some(layer) => layer.with_base_file_system(overlay),
    }
}

// Go: project/overlayfs.go:218 `_ FileHandleSource = (*overlayFS)(nil)`
impl FileHandleSource for OverlayFS {
    fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>> {
        OverlayFS::get_file(self, file_name)
    }

    fn get_file_by_path(&self, file_name: &str, path: &tspath::Path) -> Option<Rc<dyn FileHandle>> {
        OverlayFS::get_file_by_path(self, file_name, path)
    }
}

// Go: project/overlayfs.go:219 `_ LayeredFileSystem = (*overlayFS)(nil)`
impl LayeredFileSystem for OverlayFS {
    fn overlays(&self) -> Rc<IndexMap<tspath::Path, Rc<Overlay>>> {
        OverlayFS::overlays(self)
    }
}

impl FsLayer for OverlayFS {
    fn as_file_handle_source(&self) -> Option<&dyn FileHandleSource> {
        Some(self)
    }
    fn as_layered_file_system(&self) -> Option<&dyn LayeredFileSystem> {
        Some(self)
    }
    fn as_overlay_fs(&self) -> Option<&OverlayFS> {
        Some(self)
    }
}

// Go: project/overlayfs.go:217 `_ vfs.FS = (*overlayFS)(nil)`
impl vfs::Fs for OverlayFS {
    // Go: project/overlayfs.go:269 overlayFS.UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        self.host.use_case_sensitive_file_names()
    }

    // Go: project/overlayfs.go:271 overlayFS.FileExists
    fn file_exists(&self, file_name: &str) -> bool {
        let path = (self.to_path)(file_name);
        let file = self.overlays.borrow().contains_key(&path);
        let directory = self.overlay_directories.borrow().contains_key(&path);
        file || !directory && self.host.file_exists(file_name)
    }

    // Go: project/overlayfs.go:280 overlayFS.ReadFile
    fn read_file(&self, file_name: &str) -> (String, bool) {
        if let Some(file) = OverlayFS::get_file(self, file_name) {
            return (file.content(), true);
        }
        (String::new(), false)
    }

    // Go: project/overlayfs.go:287 overlayFS.WriteFile
    fn write_file(&self, path: &str, data: &str) -> Result<(), vfs::FsError> {
        self.host.write_file(path, data)
    }

    // Go: project/overlayfs.go:289 overlayFS.AppendFile
    fn append_file(&self, path: &str, data: &str) -> Result<(), vfs::FsError> {
        self.host.append_file(path, data)
    }

    // Go: project/overlayfs.go:292 overlayFS.Remove
    fn remove(&self, path: &str) -> Result<(), vfs::FsError> {
        self.host.remove(path)
    }

    // Go: project/overlayfs.go:293 overlayFS.Chtimes
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<std::time::SystemTime>,
        m_time: Option<std::time::SystemTime>,
    ) -> Result<(), vfs::FsError> {
        self.host.chtimes(path, a_time, m_time)
    }

    // Go: project/overlayfs.go:297 overlayFS.DirectoryExists
    fn directory_exists(&self, directory_name: &str) -> bool {
        let path = (self.to_path)(directory_name);
        let file = self.overlays.borrow().contains_key(&path);
        let directory = self.overlay_directories.borrow().contains_key(&path);
        directory || !file && self.host.directory_exists(directory_name)
    }

    // Go: project/overlayfs.go:306 overlayFS.GetAccessibleEntries
    // PORT: Go ranges over the directory map (random order); insertion order
    // here (`OverlayDirectories`).
    fn get_accessible_entries(&self, directory_name: &str) -> vfs::Entries {
        let path = (self.to_path)(directory_name);
        let file = self.overlays.borrow().contains_key(&path);
        let directory = self.overlay_directories.borrow().get(&path).cloned();
        let overlays = self.overlays.borrow().clone();
        if file {
            return vfs::Entries::default();
        }
        let host_entries = self.host.get_accessible_entries(directory_name);
        let mut entries = vfs::Entries {
            files: host_entries.files.clone(),
            directories: host_entries.directories.clone(),
            symlinks: host_entries.symlinks,
        };
        let use_case_sensitive_file_names = vfs::Fs::use_case_sensitive_file_names(self);
        let equal_name = |left: &str, right: &str| -> bool {
            tspath::get_canonical_file_name(left, use_case_sensitive_file_names)
                == tspath::get_canonical_file_name(right, use_case_sensitive_file_names)
        };
        for (child_path, child_name) in directory.iter().flatten() {
            entries.files.retain(|name| !equal_name(name, child_name));
            entries
                .directories
                .retain(|name| !equal_name(name, child_name));
            if let Some(symlinks) = &mut entries.symlinks {
                symlinks.retain(|name| !equal_name(name, child_name));
            }
            if overlays.contains_key(child_path) {
                entries.files.push(child_name.clone());
            } else {
                entries.directories.push(child_name.clone());
            }
        }
        entries
    }

    // Go: project/overlayfs.go:345 overlayFS.Stat
    fn stat(&self, path: &str) -> Option<vfs::FileInfo> {
        let canonical_path = (self.to_path)(path);
        let overlay = self.overlays.borrow().get(&canonical_path).cloned();
        let directory = self
            .overlay_directories
            .borrow()
            .contains_key(&canonical_path);
        if let Some(overlay) = overlay {
            return Some(overlay_file_info(&overlay));
        }
        if directory {
            return Some(overlay_directory_info(tspath::get_base_file_name(path)));
        }
        self.host.stat(path)
    }

    // Go: project/overlayfs.go:360 overlayFS.Realpath
    fn realpath(&self, path: &str) -> String {
        self.host.realpath(path)
    }

    // PORT: Go type assertions on a `vfs.FS` (see `as_fs_layer`).
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

// Go: project/overlayfs.go:362 overlayFileInfo (ts#64291)
// PORT: Go `vfs.FileInfo` is the `vfs::FileInfo` value of the Go methods:
// Name, Size, Mode 0o444, zero ModTime.
pub fn overlay_file_info(overlay: &Overlay) -> vfs::FileInfo {
    vfs::FileInfo {
        name: tspath::get_base_file_name(&overlay.file_base.file_name),
        size: overlay.file_base.content.len() as i64,
        mode: vfs::FileMode(0o444),
        mod_time: None,
    }
}

// Go: project/overlayfs.go:373 overlayDirectoryInfo (ts#64291)
// PORT: Mode is `ModeDir | 0o555`, zero ModTime.
pub fn overlay_directory_info(name: String) -> vfs::FileInfo {
    vfs::FileInfo {
        name,
        size: 0,
        mode: vfs::FileMode(vfs::FileMode::DIR.0 | 0o555),
        mod_time: None,
    }
}

// Go: project/overlayfs.go:384 createOverlayDirectories (ts#64291)
// PORT: Go ranges over the overlay map (random order); the IndexMap order
// here. It is the insertion order of each directory map
// (`OverlayDirectories`), so a directory lists its open files in the order
// they were opened.
pub fn create_overlay_directories(
    overlays: &IndexMap<tspath::Path, Rc<Overlay>>,
) -> OverlayDirectories {
    let mut overlay_directories: OverlayDirectories = FxHashMap::default();
    for (path, overlay) in overlays {
        let mut child_path = path.clone();
        let mut child = overlay.file_base.file_name.clone();
        loop {
            let parent_path = child_path.get_directory_path();
            let parent = tspath::get_directory_path(&child);
            if child_path == parent_path {
                break;
            }
            if let Some(directory) = overlay_directories.get_mut(&parent_path) {
                directory.insert(child_path.clone(), tspath::get_base_file_name(&child));
            } else {
                let mut directory: IndexMap<tspath::Path, String> = IndexMap::new();
                directory.insert(child_path.clone(), tspath::get_base_file_name(&child));
                overlay_directories.insert(parent_path.clone(), directory);
            }
            child_path = parent_path;
            child = parent;
        }
    }
    overlay_directories
}

impl OverlayFS {
    // Go: project/overlayfs.go:407 overlayFS.processChanges
    // PORT: Go takes the slice; here a borrowed slice. The per-file events
    // keep references into it where Go keeps pointers to copies. Go ranges
    // over `fileEventMap` (random order); the port keeps the order in which
    // each URI first appears.
    pub fn process_changes(
        &self,
        changes: &[FileChange],
    ) -> (FileChangeSummary, IndexMap<tspath::Path, Rc<Overlay>>) {
        let mut result = FileChangeSummary::default();
        let mut new_overlays: IndexMap<tspath::Path, Rc<Overlay>> =
            (**self.overlays.borrow()).clone();

        // Reduced collection of changes that occurred on a single file
        #[derive(Default)]
        struct FileEvents<'a> {
            open_change: Option<&'a FileChange>,
            close_change: Option<&'a FileChange>,
            watch_changed: bool,
            changes: Vec<&'a FileChange>,
            saved: bool,
            created: bool,
            deleted: bool,
        }

        let mut file_event_map: IndexMap<lsproto::DocumentUri, FileEvents<'_>> = IndexMap::new();

        for change in changes {
            let uri = &change.uri;
            if let Some(events) = file_event_map.get(uri) {
                if events.open_change.is_some() {
                    crate::core::go_panic("should see no changes after open".to_string());
                }
            } else {
                file_event_map.insert(uri.clone(), FileEvents::default());
            }
            let events = file_event_map
                .get_mut(uri)
                .expect("events were stored above");

            if !result.includes_watch_change_outside_node_modules
                && change.kind.is_watch_kind()
                && !uri.0.contains("/node_modules/")
            {
                result.includes_watch_change_outside_node_modules = true;
            }

            match change.kind {
                FileChangeKind::OPEN => {
                    if events.close_change.is_some() {
                        events.close_change = None;
                    }
                    events.open_change = Some(change);
                    events.watch_changed = false;
                    events.changes = Vec::new();
                    events.saved = false;
                    events.created = false;
                    events.deleted = false;
                }
                FileChangeKind::CLOSE => {
                    events.close_change = Some(change);
                    events.changes = Vec::new();
                    events.saved = false;
                    events.watch_changed = false;
                }
                FileChangeKind::CHANGE => {
                    if events.close_change.is_some() {
                        crate::core::go_panic("should see no changes after close".to_string());
                    }
                    events.changes.push(change);
                    events.saved = false;
                    events.watch_changed = false;
                }
                FileChangeKind::SAVE => {
                    events.saved = true;
                }
                FileChangeKind::WATCH_CREATE => {
                    if events.deleted {
                        // Delete followed by create becomes a change
                        events.deleted = false;
                        events.watch_changed = true;
                    } else {
                        events.created = true;
                    }
                }
                FileChangeKind::WATCH_CHANGE => {
                    if !events.created {
                        events.watch_changed = true;
                        events.saved = false;
                    }
                }
                FileChangeKind::WATCH_DELETE => {
                    events.watch_changed = false;
                    events.saved = false;
                    // Delete after create cancels out
                    if events.created {
                        events.created = false;
                    } else {
                        events.deleted = true;
                    }
                }
                _ => {}
            }
        }

        // Process deduplicated events per file
        for (uri, events) in &file_event_map {
            let path = uri.path(self.host.use_case_sensitive_file_names());
            let mut o: Option<Rc<Overlay>> = new_overlays.get(&path).cloned();

            if let Some(open_change) = events.open_change {
                if !result.opened.0.is_empty() || !result.reopened.0.is_empty() {
                    crate::core::go_panic(
                        "can only process one file open event at a time".to_string(),
                    );
                }
                if o.as_ref()
                    .is_some_and(|o| *o.file_base.content != *open_change.content)
                {
                    result.changed.insert(uri.clone());
                } else if o.is_none() {
                    result.opened = uri.clone();
                } else {
                    result.reopened = uri.clone();
                }
                let mut script_kind =
                    lsconv::language_kind_to_script_kind(&open_change.language_kind);
                if script_kind == ScriptKind::UNKNOWN {
                    script_kind = get_script_kind_from_file_name(&uri.file_name());
                }
                new_overlays.insert(
                    path,
                    Rc::new(new_overlay(
                        &uri.file_name(),
                        open_change.content.as_str(),
                        open_change.version,
                        script_kind,
                    )),
                );
                continue;
            }

            // ts#64036: a close or change of a file with no overlay is
            // ignored (stale document notifications), not a panic.
            if events.close_change.is_some() && o.is_some() {
                result.closed.insert(uri.clone());
                new_overlays.shift_remove(&path);
                o = None;
            }

            if events.watch_changed {
                if let Some(cur) = o.clone() {
                    if !events.saved {
                        let (matches_disk_text, _) = cur.compute_matches_disk_text(&*self.host);
                        if matches_disk_text != cur.matches_disk_text.get() {
                            let next = new_overlay(
                                &cur.file_base.file_name,
                                Arc::clone(&cur.file_base.content),
                                cur.version.get(),
                                cur.kind,
                            );
                            next.matches_disk_text.set(matches_disk_text);
                            let next = Rc::new(next);
                            new_overlays.insert(path.clone(), next.clone());
                            o = Some(next);
                        }
                    }
                } else {
                    result.changed.insert(uri.clone());
                }
            }

            if !events.changes.is_empty() && o.is_some() {
                result.changed.insert(uri.clone());
                // PORT: the Go line map closure captures the variable `o`,
                // which the loop below reassigns; `o_cell` is that variable.
                let o_cell: Rc<RefCell<Option<Rc<Overlay>>>> = Rc::new(RefCell::new(o.clone()));
                for change in &events.changes {
                    let o_for_line_map = o_cell.clone();
                    let converters = lsconv::new_converters(
                        self.position_encoding.clone(),
                        move |_file_name: &str| -> Option<Rc<lsconv::LSPLineMap>> {
                            Some(
                                o_for_line_map
                                    .borrow()
                                    .as_ref()
                                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                                    .file_base
                                    .lsp_line_map(),
                            )
                        },
                    );
                    for text_change in &change.changes {
                        let cur = o_cell
                            .borrow()
                            .clone()
                            .unwrap_or_else(|| crate::core::go_nil_dereference());
                        if let Some(partial_change) = &text_change.partial {
                            // tsgo#4712
                            let ranges = lsconv::from_lsp_range(
                                &converters,
                                &*cur,
                                partial_change.range,
                                crate::spanmap::Feature::ALL,
                            );
                            crate::go_assert!(
                                ranges.len() == 1,
                                "expected exactly one range for partial change"
                            );
                            let text_change = TextChange {
                                text_range: ranges[0].span,
                                new_text: partial_change.text.clone(),
                            };
                            let new_content = text_change.apply_to(&cur.file_base.content);
                            *o_cell.borrow_mut() = Some(Rc::new(new_overlay(
                                &cur.file_base.file_name,
                                new_content,
                                change.version,
                                cur.kind,
                            )));
                        } else if let Some(whole_change) = &text_change.whole_document {
                            *o_cell.borrow_mut() = Some(Rc::new(new_overlay(
                                &cur.file_base.file_name,
                                whole_change.text.as_str(),
                                change.version,
                                cur.kind,
                            )));
                        }
                    }
                    if !change.changes.is_empty() {
                        let cur = o_cell
                            .borrow()
                            .clone()
                            .unwrap_or_else(|| crate::core::go_nil_dereference());
                        cur.version.set(change.version);
                        cur.file_base
                            .hash
                            .set(xxh3_128(cur.file_base.content.as_bytes()));
                        cur.matches_disk_text.set(false);
                        new_overlays.insert(path.clone(), cur);
                    }
                }
                o = o_cell.borrow().clone();
            }

            if events.saved {
                if let Some(cur) = o.clone() {
                    let next = new_overlay(
                        &cur.file_base.file_name,
                        Arc::clone(&cur.file_base.content),
                        cur.version.get(),
                        cur.kind,
                    );
                    next.matches_disk_text.set(true);
                    let next = Rc::new(next);
                    new_overlays.insert(path.clone(), next.clone());
                    o = Some(next);
                } else if !events.watch_changed {
                    // File was saved but never opened via didOpen; treat as a disk change.
                    result.changed.insert(uri.clone());
                }
            }

            if events.created && o.is_none() {
                result.created.insert(uri.clone());
            }

            if events.deleted && o.is_none() {
                result.deleted.insert(uri.clone());
            }
        }

        *self.overlay_directories.borrow_mut() = Rc::new(create_overlay_directories(&new_overlays));
        *self.overlays.borrow_mut() = Rc::new(new_overlays.clone());
        (result, new_overlays)
    }
}
