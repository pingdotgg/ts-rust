//! Go: tsoptions/contentmappers.go (tsgo#4712).

use crate::contentmapper::Manifest;
use crate::frontend::prelude::*;

// Go: tsoptions/contentmappers.go:31 resolveContentMapperManifest
// resolveContentMapperManifest locates packageName in node_modules (walking up from the directory of
// containingFile via node module resolution) and reads its package.json to produce the mapper's manifest
// and package directory. It never executes the package. On failure it returns a diagnostic describing why
// the mapper could not be resolved; on success the diagnostic is nil.
// PORT: Go nil `*ast.Diagnostic` is `None`. Go passes the `ParseConfigHost`
// as the resolution host; `ResolverHost` keeps its `FS()` and
// `GetCurrentDirectory()` values, as for `extends`.
pub fn resolve_content_mapper_manifest(
    host: &dyn ParseConfigHost,
    containing_file: &str,
    package_name: &str,
) -> (Manifest, String, Option<Diagnostic>) {
    let resolver_host: Rc<dyn ResolutionHost> = Rc::new(ResolverHost {
        fs: host.fs(),
        current_directory: host.get_current_directory(),
    });
    let resolver = new_resolver(ResolverOptions {
        host: Some(resolver_host),
        compiler_options: Some(Rc::new(CompilerOptions {
            module_resolution: ModuleResolutionKind::BUNDLER,
            ..Default::default()
        })),
        ..Default::default()
    });
    let resolved = resolver.resolve_package_directory(
        package_name,
        containing_file,
        RESOLUTION_MODE_NONE,
        None,
    );
    let Some(resolved) = resolved.filter(|resolved| !resolved.resolved_file_name.is_empty()) else {
        return (
            Manifest::default(),
            String::new(),
            Some(new_compiler_diagnostic(
                diag::The_content_mapper_package_0_could_not_be_resolved,
                args![package_name],
            )),
        );
    };
    let package_directory = resolved.resolved_file_name;

    let package_json_path = combine_paths(&package_directory, &["package.json"]);
    let (contents, ok) = host.fs().read_file(&package_json_path);
    if !ok {
        return (
            Manifest::default(),
            package_directory,
            Some(new_compiler_diagnostic(
                diag::The_content_mapper_package_0_could_not_be_resolved,
                args![package_name],
            )),
        );
    }
    // PORT: `contents` is the port form of the Go text; Go parses its bytes.
    let Ok(fields) = crate::frontend::packagejson::parse(&go_string_bytes(&contents)) else {
        return (
            Manifest::default(),
            package_directory,
            Some(new_compiler_diagnostic(
                diag::The_package_json_of_the_content_mapper_package_0_could_not_be_parsed,
                args![package_name],
            )),
        );
    };
    let (name, _) = fields.header_fields.name.get_value();
    if name.is_empty() {
        return (
            Manifest::default(),
            package_directory,
            Some(new_compiler_diagnostic(
                diag::The_package_json_of_the_content_mapper_package_0_does_not_specify_a_name,
                args![package_name],
            )),
        );
    }
    let (version, _) = fields.header_fields.version.get_value();

    // A content mapper package must declare how to run it: a "typescript.contentMapper" object with a non-empty
    // "exec" array of strings.
    let (cm, ok) = fields.content_mapper.get_value();
    if !ok {
        return (
            Manifest::default(),
            package_directory,
            Some(new_compiler_diagnostic(
                diag::The_package_json_of_the_content_mapper_package_0_does_not_declare_a_typescript_contentMapper_object,
                args![package_name],
            )),
        );
    }
    let (exec, ok) = cm.exec.get_value();
    if !ok || exec.is_empty() {
        return (
            Manifest::default(),
            package_directory,
            Some(new_compiler_diagnostic(
                diag::The_typescript_contentMapper_exec_of_the_content_mapper_package_0_must_be_a_non_empty_array_of_strings,
                args![package_name],
            )),
        );
    }
    let (compiler_options, _) = cm.compiler_options.get_value();
    let (dynamic_config, _) = cm.dynamic_config.get_value();
    (
        Manifest {
            name,
            version,
            exec,
            compiler_options,
            dynamic_config,
        },
        package_directory,
        None,
    )
}
