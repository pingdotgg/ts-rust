//! Go: tsoptions/parsedoptions.go (tsgo#4712 moves `ParsedOptions` here
//! from core/parsedoptions.go and adds `ContentMappers`).

use crate::contentmapper::Mapper;
use crate::frontend::prelude::*;

// Go: tsoptions/parsedoptions.go:11 ParsedOptions
// PORT: Go `*CompilerOptions` is `Rc<CompilerOptions>`, so copies of the
// struct share it like Go pointers do. Go `*TypeAcquisition` is an
// `Option`. Go `[]*ProjectReference` is `Option<Vec<ProjectReference>>`:
// `None` is Go nil (no `references` in the config) and `Some(vec![])` is
// Go `"references": []`. The build checks that difference.
// PORT: Go `[]*contentmapper.Mapper` is `Vec<Rc<Mapper>>`; a nil slice is
// empty. Go compares and keys mappers by pointer (`Rc::ptr_eq`).
// PORT: `PartialEq` is Go `reflect.DeepEqual` (execute/watcher.go
// recheckTsConfig). The `Option` keeps the Go nil and empty slice apart for
// `project_references`; other `Vec` fields have no nil, so those two compare
// equal there.
#[derive(Clone, Debug, Default)]
pub struct ParsedOptions {
    pub compiler_options: Rc<CompilerOptions>,
    pub type_acquisition: Option<TypeAcquisition>,

    pub file_names: Vec<String>,
    pub project_references: Option<Vec<ProjectReference>>,
    // tsgo#4712
    pub content_mappers: Vec<Rc<Mapper>>,
}

// PORT: the derived `==` with `CompilerOptions::deep_equal` for the options,
// so a config whose `paths` order changes is a changed config, as in Go.
impl PartialEq for ParsedOptions {
    fn eq(&self, other: &Self) -> bool {
        let Self {
            compiler_options,
            type_acquisition,
            file_names,
            project_references,
            content_mappers,
        } = self;
        compiler_options.deep_equal(&other.compiler_options)
            && *type_acquisition == other.type_acquisition
            && *file_names == other.file_names
            && *project_references == other.project_references
            && *content_mappers == other.content_mappers
    }
}
