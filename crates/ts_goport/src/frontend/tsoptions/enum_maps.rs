use crate::frontend::prelude::*;
use std::sync::LazyLock;

// Since ts#64457 Go generates the option maps into
// tsoptions/declarations_generated.go (from tools/scripts/tsc/options.ts);
// the rest of enummaps.go stays. The watch option maps and core/watchoptions.go
// are gone.

/// Go `*collections.OrderedMap[string, any]` used by the option enum maps.
pub type CommandLineOptionEnumMap = IndexMap<String, CompilerOptionsValue>;

// Go: tsoptions/declarations_generated.go:1172 LibMap
// PORT: Go package-level vars built at init are `LazyLock` statics. The Go
// `any` values are `CompilerOptionsValue`. LibMap values are Go strings.
pub static LIB_MAP: LazyLock<CommandLineOptionEnumMap> = LazyLock::new(|| {
    let entries: &[(&str, &str)] = &[
        // JavaScript only
        ("es5", "lib.es5.d.ts"),
        ("es6", "lib.es2015.d.ts"),
        ("es2015", "lib.es2015.d.ts"),
        ("es7", "lib.es2016.d.ts"),
        ("es2016", "lib.es2016.d.ts"),
        ("es2017", "lib.es2017.d.ts"),
        ("es2018", "lib.es2018.d.ts"),
        ("es2019", "lib.es2019.d.ts"),
        ("es2020", "lib.es2020.d.ts"),
        ("es2021", "lib.es2021.d.ts"),
        ("es2022", "lib.es2022.d.ts"),
        ("es2023", "lib.es2023.d.ts"),
        ("es2024", "lib.es2024.d.ts"),
        ("es2025", "lib.es2025.d.ts"),
        ("es2026", "lib.es2026.d.ts"),
        ("esnext", "lib.esnext.d.ts"),
        // Host only
        ("dom", "lib.dom.d.ts"),
        ("dom.iterable", "lib.dom.iterable.d.ts"),
        ("dom.asynciterable", "lib.dom.asynciterable.d.ts"),
        ("webworker", "lib.webworker.d.ts"),
        (
            "webworker.importscripts",
            "lib.webworker.importscripts.d.ts",
        ),
        ("webworker.iterable", "lib.webworker.iterable.d.ts"),
        (
            "webworker.asynciterable",
            "lib.webworker.asynciterable.d.ts",
        ),
        ("scripthost", "lib.scripthost.d.ts"),
        // ES2015 and later By-feature options
        ("es2015.core", "lib.es2015.core.d.ts"),
        ("es2015.collection", "lib.es2015.collection.d.ts"),
        ("es2015.generator", "lib.es2015.generator.d.ts"),
        ("es2015.iterable", "lib.es2015.iterable.d.ts"),
        ("es2015.promise", "lib.es2015.promise.d.ts"),
        ("es2015.proxy", "lib.es2015.proxy.d.ts"),
        ("es2015.reflect", "lib.es2015.reflect.d.ts"),
        ("es2015.symbol", "lib.es2015.symbol.d.ts"),
        (
            "es2015.symbol.wellknown",
            "lib.es2015.symbol.wellknown.d.ts",
        ),
        ("es2016.array.include", "lib.es2016.array.include.d.ts"),
        ("es2016.intl", "lib.es2016.intl.d.ts"),
        ("es2017.arraybuffer", "lib.es2017.arraybuffer.d.ts"),
        ("es2017.date", "lib.es2017.date.d.ts"),
        ("es2017.object", "lib.es2017.object.d.ts"),
        ("es2017.sharedmemory", "lib.es2017.sharedmemory.d.ts"),
        ("es2017.string", "lib.es2017.string.d.ts"),
        ("es2017.intl", "lib.es2017.intl.d.ts"),
        ("es2017.typedarrays", "lib.es2017.typedarrays.d.ts"),
        ("es2018.asyncgenerator", "lib.es2018.asyncgenerator.d.ts"),
        ("es2018.asynciterable", "lib.es2018.asynciterable.d.ts"),
        ("es2018.intl", "lib.es2018.intl.d.ts"),
        ("es2018.promise", "lib.es2018.promise.d.ts"),
        ("es2018.regexp", "lib.es2018.regexp.d.ts"),
        ("es2019.array", "lib.es2019.array.d.ts"),
        ("es2019.object", "lib.es2019.object.d.ts"),
        ("es2019.string", "lib.es2019.string.d.ts"),
        ("es2019.symbol", "lib.es2019.symbol.d.ts"),
        ("es2019.intl", "lib.es2019.intl.d.ts"),
        ("es2020.bigint", "lib.es2020.bigint.d.ts"),
        ("es2020.date", "lib.es2020.date.d.ts"),
        ("es2020.promise", "lib.es2020.promise.d.ts"),
        ("es2020.sharedmemory", "lib.es2020.sharedmemory.d.ts"),
        ("es2020.string", "lib.es2020.string.d.ts"),
        (
            "es2020.symbol.wellknown",
            "lib.es2020.symbol.wellknown.d.ts",
        ),
        ("es2020.intl", "lib.es2020.intl.d.ts"),
        ("es2020.number", "lib.es2020.number.d.ts"),
        ("es2021.promise", "lib.es2021.promise.d.ts"),
        ("es2021.string", "lib.es2021.string.d.ts"),
        ("es2021.weakref", "lib.es2021.weakref.d.ts"),
        ("es2021.intl", "lib.es2021.intl.d.ts"),
        ("es2022.array", "lib.es2022.array.d.ts"),
        ("es2022.error", "lib.es2022.error.d.ts"),
        ("es2022.intl", "lib.es2022.intl.d.ts"),
        ("es2022.object", "lib.es2022.object.d.ts"),
        ("es2022.string", "lib.es2022.string.d.ts"),
        ("es2022.regexp", "lib.es2022.regexp.d.ts"),
        ("es2023.array", "lib.es2023.array.d.ts"),
        ("es2023.collection", "lib.es2023.collection.d.ts"),
        ("es2023.intl", "lib.es2023.intl.d.ts"),
        ("es2024.arraybuffer", "lib.es2024.arraybuffer.d.ts"),
        ("es2024.collection", "lib.es2024.collection.d.ts"),
        ("es2024.object", "lib.es2024.object.d.ts"),
        ("es2024.promise", "lib.es2024.promise.d.ts"),
        ("es2024.regexp", "lib.es2024.regexp.d.ts"),
        ("es2024.sharedmemory", "lib.es2024.sharedmemory.d.ts"),
        ("es2024.string", "lib.es2024.string.d.ts"),
        ("es2025.collection", "lib.es2025.collection.d.ts"),
        ("es2025.float16", "lib.es2025.float16.d.ts"),
        ("es2025.intl", "lib.es2025.intl.d.ts"),
        ("es2025.iterator", "lib.es2025.iterator.d.ts"),
        ("es2025.promise", "lib.es2025.promise.d.ts"),
        ("es2025.regexp", "lib.es2025.regexp.d.ts"),
        ("es2026.array", "lib.es2026.array.d.ts"),
        ("es2026.collection", "lib.es2026.collection.d.ts"),
        ("es2026.error", "lib.es2026.error.d.ts"),
        ("es2026.iterator", "lib.es2026.iterator.d.ts"),
        ("es2026.json", "lib.es2026.json.d.ts"),
        ("es2026.math", "lib.es2026.math.d.ts"),
        ("es2026.typedarrays", "lib.es2026.typedarrays.d.ts"),
        // Fallback for backward compatibility
        ("esnext.asynciterable", "lib.es2018.asynciterable.d.ts"),
        ("esnext.symbol", "lib.es2019.symbol.d.ts"),
        ("esnext.bigint", "lib.es2020.bigint.d.ts"),
        ("esnext.weakref", "lib.es2021.weakref.d.ts"),
        ("esnext.object", "lib.es2024.object.d.ts"),
        ("esnext.regexp", "lib.es2024.regexp.d.ts"),
        ("esnext.string", "lib.es2024.string.d.ts"),
        ("esnext.float16", "lib.es2025.float16.d.ts"),
        ("esnext.array", "lib.es2026.array.d.ts"),
        ("esnext.collection", "lib.es2026.collection.d.ts"),
        ("esnext.error", "lib.es2026.error.d.ts"),
        ("esnext.iterator", "lib.es2026.iterator.d.ts"),
        ("esnext.typedarrays", "lib.es2026.typedarrays.d.ts"),
        // ts#64093
        ("esnext.promise", "lib.esnext.promise.d.ts"),
        // ESNext By-feature options
        ("esnext.date", "lib.esnext.date.d.ts"),
        ("esnext.decorators", "lib.esnext.decorators.d.ts"),
        ("esnext.disposable", "lib.esnext.disposable.d.ts"),
        ("esnext.intl", "lib.esnext.intl.d.ts"),
        // ts#63915
        ("esnext.modulesource", "lib.esnext.modulesource.d.ts"),
        ("esnext.sharedmemory", "lib.esnext.sharedmemory.d.ts"),
        ("esnext.temporal", "lib.esnext.temporal.d.ts"),
        // Decorators
        ("decorators", "lib.decorators.d.ts"),
        ("decorators.legacy", "lib.decorators.legacy.d.ts"),
    ];
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), CompilerOptionsValue::String(v.to_string())))
        .collect()
});

// Go: tsoptions/enummaps.go:12 Libs
pub static LIBS: LazyLock<Vec<String>> = LazyLock::new(|| LIB_MAP.keys().cloned().collect());

// Go: tsoptions/enummaps.go:13 LibFilesSet
pub static LIB_FILES_SET: LazyLock<FxHashSet<String>> = LazyLock::new(|| {
    LIB_MAP
        .values()
        .map(|s| match s {
            CompilerOptionsValue::String(s) => s.clone(),
            _ => panic!("LibMap value is not a string"),
        })
        .collect()
});

// Go: tsoptions/enummaps.go:16 GetLibFileName
#[must_use]
pub fn get_lib_file_name(lib_name: &str) -> (String, bool) {
    // checks if the libName is a valid lib name or file name and converts the lib name to the filename if needed
    let lib_name = to_file_name_lower_case(lib_name);
    if LIB_FILES_SET.contains(&lib_name) {
        return (lib_name, true);
    }
    let Some(lib) = LIB_MAP.get(&lib_name) else {
        return (String::new(), false);
    };
    match lib {
        CompilerOptionsValue::String(s) => (s.clone(), true),
        _ => panic!("LibMap value is not a string"),
    }
}

/// Builds an enum map from `(key, value)` pairs in Go order.
fn enum_map(entries: Vec<(&str, CompilerOptionsValue)>) -> CommandLineOptionEnumMap {
    entries
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

// Go: tsoptions/declarations_generated.go:1291 moduleResolutionOptionMap
pub static MODULE_RESOLUTION_OPTION_MAP: LazyLock<CommandLineOptionEnumMap> = LazyLock::new(|| {
    use CompilerOptionsValue::ModuleResolutionKind as V;
    enum_map(vec![
        ("node16", V(ModuleResolutionKind::NODE16)),
        ("nodenext", V(ModuleResolutionKind::NODE_NEXT)),
        ("bundler", V(ModuleResolutionKind::BUNDLER)),
        ("classic", V(ModuleResolutionKind::CLASSIC)),
        ("node", V(ModuleResolutionKind::NODE10)),
        ("node10", V(ModuleResolutionKind::NODE10)),
    ])
});

// Go: tsoptions/declarations_generated.go:1317 targetOptionMap
pub static TARGET_OPTION_MAP: LazyLock<CommandLineOptionEnumMap> = LazyLock::new(|| {
    use CompilerOptionsValue::ScriptTarget as V;
    enum_map(vec![
        ("es5", V(ScriptTarget::ES5)),
        ("es6", V(ScriptTarget::ES2015)),
        ("es2015", V(ScriptTarget::ES2015)),
        ("es2016", V(ScriptTarget::ES2016)),
        ("es2017", V(ScriptTarget::ES2017)),
        ("es2018", V(ScriptTarget::ES2018)),
        ("es2019", V(ScriptTarget::ES2019)),
        ("es2020", V(ScriptTarget::ES2020)),
        ("es2021", V(ScriptTarget::ES2021)),
        ("es2022", V(ScriptTarget::ES2022)),
        ("es2023", V(ScriptTarget::ES2023)),
        ("es2024", V(ScriptTarget::ES2024)),
        ("es2025", V(ScriptTarget::ES2025)),
        ("es2026", V(ScriptTarget::ES2026)),
        ("esnext", V(ScriptTarget::ES_NEXT)),
    ])
});

// Go: tsoptions/declarations_generated.go:1300 moduleOptionMap
pub static MODULE_OPTION_MAP: LazyLock<CommandLineOptionEnumMap> = LazyLock::new(|| {
    use CompilerOptionsValue::ModuleKind as V;
    enum_map(vec![
        ("commonjs", V(ModuleKind::COMMON_JS)),
        ("amd", V(ModuleKind::AMD)),
        ("system", V(ModuleKind::SYSTEM)),
        ("umd", V(ModuleKind::UMD)),
        ("es6", V(ModuleKind::ES2015)),
        ("es2015", V(ModuleKind::ES2015)),
        ("es2020", V(ModuleKind::ES2020)),
        ("es2022", V(ModuleKind::ES2022)),
        ("esnext", V(ModuleKind::ES_NEXT)),
        ("node16", V(ModuleKind::NODE16)),
        ("node18", V(ModuleKind::NODE18)),
        ("node20", V(ModuleKind::NODE20)),
        ("nodenext", V(ModuleKind::NODE_NEXT)),
        ("preserve", V(ModuleKind::PRESERVE)),
    ])
});

// Go: tsoptions/declarations_generated.go:1335 moduleDetectionOptionMap
pub static MODULE_DETECTION_OPTION_MAP: LazyLock<CommandLineOptionEnumMap> = LazyLock::new(|| {
    use CompilerOptionsValue::ModuleDetectionKind as V;
    enum_map(vec![
        ("auto", V(ModuleDetectionKind::AUTO)),
        ("legacy", V(ModuleDetectionKind::LEGACY)),
        ("force", V(ModuleDetectionKind::FORCE)),
    ])
});

// Go: tsoptions/declarations_generated.go:1341 jsxOptionMap
pub static JSX_OPTION_MAP: LazyLock<CommandLineOptionEnumMap> = LazyLock::new(|| {
    use CompilerOptionsValue::JsxEmit as V;
    enum_map(vec![
        ("preserve", V(JsxEmit::PRESERVE)),
        ("react-native", V(JsxEmit::REACT_NATIVE)),
        ("react-jsx", V(JsxEmit::REACT_JSX)),
        ("react-jsxdev", V(JsxEmit::REACT_JSX_DEV)),
        ("react", V(JsxEmit::REACT)),
    ])
});

// Go: tsoptions/declarations_generated.go:1349 newLineOptionMap
pub static NEW_LINE_OPTION_MAP: LazyLock<CommandLineOptionEnumMap> = LazyLock::new(|| {
    use CompilerOptionsValue::NewLineKind as V;
    enum_map(vec![
        ("crlf", V(NewLineKind::CRLF)),
        ("lf", V(NewLineKind::LF)),
    ])
});

// Go: tsoptions/declarations_generated.go:1370 targetToLibMap
pub static TARGET_TO_LIB_MAP: LazyLock<FxHashMap<ScriptTarget, String>> = LazyLock::new(|| {
    let entries: [(ScriptTarget, &str); 13] = [
        (ScriptTarget::ES_NEXT, "lib.esnext.full.d.ts"),
        (ScriptTarget::ES2026, "lib.es2026.full.d.ts"),
        (ScriptTarget::ES2025, "lib.es2025.full.d.ts"),
        (ScriptTarget::ES2024, "lib.es2024.full.d.ts"),
        (ScriptTarget::ES2023, "lib.es2023.full.d.ts"),
        (ScriptTarget::ES2022, "lib.es2022.full.d.ts"),
        (ScriptTarget::ES2021, "lib.es2021.full.d.ts"),
        (ScriptTarget::ES2020, "lib.es2020.full.d.ts"),
        (ScriptTarget::ES2019, "lib.es2019.full.d.ts"),
        (ScriptTarget::ES2018, "lib.es2018.full.d.ts"),
        (ScriptTarget::ES2017, "lib.es2017.full.d.ts"),
        (ScriptTarget::ES2016, "lib.es2016.full.d.ts"),
        (ScriptTarget::ES2015, "lib.es6.d.ts"), // We don't use lib.es2015.full.d.ts due to breaking change.
    ];
    entries
        .into_iter()
        .map(|(t, s)| (t, s.to_string()))
        .collect()
});

// Go: tsoptions/enummaps.go:29 TargetToLibMap
#[must_use]
pub fn target_to_lib_map() -> &'static FxHashMap<ScriptTarget, String> {
    &TARGET_TO_LIB_MAP
}

// Go: tsoptions/enummaps.go:33 GetDefaultLibFileName
#[must_use]
pub fn get_default_lib_file_name(options: &CompilerOptions) -> String {
    let Some(name) = TARGET_TO_LIB_MAP.get(&options.get_emit_script_target()) else {
        return "lib.d.ts".to_string();
    };
    name.clone()
}
