//! Port of Go `ls/host.go`.

use crate::ls::prelude::*;

// Go: ls/host.go:11 Host
// PORT: Go `*lsconv.Converters` is shared (`Rc`). Go `*sourcemap.ECMALineInfo`
// and `*autoimport.Registry` can be nil, so they are `Option<Rc<..>>`.
// `[]string` params are `&[String]`; a Go `nil` slice is `&[]`. The
// `read_file` text is a `FileText`, shared like a Go string: a language
// service reads a file once per location it converts (`get_script`), and a
// copy of a big file per location made references 7 times slower than Go.
pub trait Host {
    fn use_case_sensitive_file_names(&self) -> bool;
    fn read_file(&self, path: &str) -> (FileText, bool);
    fn converters(&self) -> Rc<lsconv::Converters>;
    fn get_preferences(&self, active_file: &str) -> lsutil::UserPreferences;
    fn get_ecma_line_info(&self, file_name: &str) -> Option<Rc<sourcemap::lineinfo::ECMALineInfo>>;
    fn auto_import_registry(&self) -> Option<Rc<autoimport::Registry>>;

    // Used for module specifier completions.
    // ! Do not use for anything else, as this violates the principle that
    // the host is a snapshot-in-time.
    fn read_directory(
        &self,
        current_dir: &str,
        path: &str,
        extensions: &[String],
        excludes: &[String],
        includes: &[String],
        depth: i32,
    ) -> Vec<String>;
    fn get_directories(&self, path: &str) -> Vec<String>;
    fn directory_exists(&self, path: &str) -> bool;
    fn file_exists(&self, path: &str) -> bool;
}
