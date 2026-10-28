"use strict";

const {
    moduleDetectionKindToNative,
    moduleKindToNative,
    newLineKindToNative,
    scriptTargetToNative,
} = require("./flags.js");
const { getTs6Message, toTsDiagnostic } = require("./diagnostics.js");

// The compiler options tsc-rs reads from a createSnapshot `compilerOptions` object
// (crates/ts_goport/src/frontend/tsoptions/decls.rs, finalize_raw_compiler_options). The
// options that only steer the command line (help, watch, listFiles, pretty, locale, traces and
// profiles) stay in JS, as TS 6.0 programs ignore them too.
const NATIVE_OPTION_NAMES = new Set([
    "allowArbitraryExtensions", "allowImportingTsExtensions", "allowJs",
    "allowSyntheticDefaultImports", "allowUmdGlobalAccess", "allowUnreachableCode",
    "allowUnusedLabels", "alwaysStrict", "assumeChangesOnlyAffectDirectDependencies", "baseUrl",
    "checkJs", "checkers", "composite", "customConditions", "declaration", "declarationDir",
    "declarationMap", "deduplicatePackages", "disableReferencedProjectLoad",
    "disableSizeLimit", "disableSolutionSearching", "disableSourceOfProjectReferenceRedirect",
    "downlevelIteration", "emitBOM", "emitDeclarationOnly", "emitDecoratorMetadata",
    "erasableSyntaxOnly", "esModuleInterop", "exactOptionalPropertyTypes", "experimentalDecorators",
    "forceConsistentCasingInFileNames", "ignoreDeprecations",
    "importHelpers", "incremental", "inlineSourceMap", "inlineSources", "isolatedDeclarations",
    "isolatedModules", "jsx", "jsxFactory", "jsxFragmentFactory", "jsxImportSource", "lib",
    "libReplacement", "mapRoot", "maxNodeModuleJsDepth",
    "module", "moduleDetection", "moduleResolution", "moduleSuffixes", "newLine", "noCheck",
    "noEmit", "noEmitHelpers", "noEmitOnError", "noErrorTruncation", "noFallthroughCasesInSwitch",
    "noImplicitAny", "noImplicitOverride", "noImplicitReturns", "noImplicitThis", "noLib",
    "noPropertyAccessFromIndexSignature", "noResolve", "noUncheckedIndexedAccess",
    "noUncheckedSideEffectImports", "noUnusedLocals", "noUnusedParameters", "outDir", "outFile",
    "paths", "plugins", "preserveConstEnums", "preserveSymlinks",
    "quiet", "reactNamespace", "removeComments", "resolveJsonModule", "resolvePackageJsonExports",
    "resolvePackageJsonImports", "rewriteRelativeImportExtensions", "rootDir", "rootDirs",
    "runExternalCode", "singleThreaded", "skipDefaultLibCheck", "skipLibCheck", "sourceMap",
    "sourceRoot", "stableTypeOrdering", "strict", "strictBindCallApply",
    "strictBuiltinIteratorReturn", "strictFunctionTypes", "strictNullChecks",
    "strictPropertyInitialization", "stripInternal", "target", "tsBuildInfoFile",
    "typeRoots", "types", "useDefineForClassFields", "useUnknownInCatchVariables",
    "verbatimModuleSyntax",
    // Internal options tsc-rs reads (TS 6.0 sets them too).
    "configFilePath", "allowNonTsExtensions", "suppressOutputPathCheck",
]);

// Path options tsc-rs roots against its current directory; TS 6.0 config parsing makes them
// absolute already, programmatic options may be relative to the host's current directory.
const PATH_OPTIONS = ["configFilePath", "declarationDir", "outDir", "outFile", "rootDir", "tsBuildInfoFile"];
const PATH_LIST_OPTIONS = ["rootDirs", "typeRoots"];

// Options that tsc-rs does not number like TS 6.0, or that this module emulates.
const TRANSLATED_OPTIONS = new Set([
    "allowSyntheticDefaultImports", "alwaysStrict", "baseUrl", "downlevelIteration", "esModuleInterop",
    "lib", "module", "moduleDetection", "moduleResolution", "newLine", "paths", "target",
]);

// tsc-rs renamed these TS 6.0 lib files (esnext.* moved to es2026.*); their lib names still work.
const RENAMED_LIB_FILES = new Map([
    ["lib.esnext.array.d.ts", "esnext.array"],
    ["lib.esnext.collection.d.ts", "esnext.collection"],
    ["lib.esnext.error.d.ts", "esnext.error"],
    ["lib.esnext.typedarrays.d.ts", "esnext.typedarrays"],
]);

// Translates TS 6.0 CompilerOptions into the raw compiler options of a tsc-rs createSnapshot
// program, and returns the TS 6.0 option diagnostics that tsc-rs will not report itself.
// `context.currentDirectory` roots relative path options (the TS 6.0 program's current
// directory); `context.typeScriptVersion` is CreateProgramOptions.typeScriptVersion.
function toNativeOptions(ts, options, context = {}) {
    const native = {};
    for (const [name, value] of Object.entries(options)) {
        if (value === undefined || value === null || !NATIVE_OPTION_NAMES.has(name) || TRANSLATED_OPTIONS.has(name)) {
            continue;
        }
        const nativeValue = toNativeValue(ts, name, value);
        if (nativeValue !== undefined) {
            native[name] = nativeValue;
        }
    }
    translateEnums(ts, options, native);
    translateAlwaysOnOptions(options, native);
    translateLib(options, native);
    translatePaths(ts, options, native, context.currentDirectory);
    rootPathOptions(ts, native, context.currentDirectory);
    return { options: native, diagnostics: getOptionDiagnostics(ts, options, context) };
}

// tsc-rs reads a boolean only from `true`/`false` and fails on an enum that is not a number,
// where TS 6.0 tests truthiness and string enum values pass through programmatic options.
function toNativeValue(ts, name, value) {
    const type = getOptionType(ts, name, value);
    if (type instanceof Map) {
        return getEnumValue(ts, name, value);
    }
    if (type === "boolean") {
        return Boolean(value);
    }
    if (type === "string") {
        return typeof value === "string" ? value : undefined;
    }
    if (type === "number") {
        return typeof value === "number" ? value : undefined;
    }
    if (type === "list" || type === "listOrElement") {
        return Array.isArray(value) ? value : undefined;
    }
    return value;
}

// TS 7-only options are not in TS 6.0's table.
function getOptionType(ts, name, value) {
    const declaration = ts.getOptionsNameMap().optionsNameMap.get(name.toLowerCase());
    if (declaration === undefined) {
        return typeof value === "boolean" ? "boolean" : undefined;
    }
    return declaration.type;
}

// A TS 6.0 enum value: a number, or a name from the option's map (programmatic options).
function getEnumValue(ts, name, value) {
    if (typeof value === "number") {
        return value;
    }
    if (typeof value !== "string") {
        return undefined;
    }
    const declaration = ts.getOptionsNameMap().optionsNameMap.get(name.toLowerCase());
    const map = declaration?.type;
    return map instanceof Map ? map.get(value.toLowerCase()) : undefined;
}

function translateEnums(ts, options, native) {
    // TS 6.0 and tsc-rs give an unset target different defaults (ES2025 against ES2026), so the
    // TS 6.0 default is sent; it also keeps TS 6.0's default lib. TS 6.0 reads ES3 as unset.
    native.target = scriptTargetToNative(ts.getEmitScriptTarget({ target: getEnumValue(ts, "target", options.target) }));
    setDefined(native, "module", moduleKindToNative(getEnumValue(ts, "module", options.module)));
    const moduleResolution = getEnumValue(ts, "moduleResolution", options.moduleResolution);
    // node10 falls back to tsc-rs's default resolution; TS 6.0's deprecation error is reported here.
    if (moduleResolution !== ts.ModuleResolutionKind.Node10) {
        setDefined(native, "moduleResolution", moduleResolution);
    }
    const newLine = getEnumValue(ts, "newLine", options.newLine);
    setDefined(native, "newLine", newLine === undefined ? undefined : newLineKindToNative(newLine));
    const moduleDetection = getEnumValue(ts, "moduleDetection", options.moduleDetection);
    setDefined(native, "moduleDetection", moduleDetection === undefined ? undefined : moduleDetectionKindToNative(moduleDetection));
}

// tsc-rs always enables these options and reports their `false` as removed; only `true` is sent.
// downlevelIteration, which has no effect from ES2015 on, is never sent.
function translateAlwaysOnOptions(options, native) {
    for (const name of ["alwaysStrict", "esModuleInterop", "allowSyntheticDefaultImports"]) {
        if (options[name] !== undefined && options[name] !== null && options[name] !== false) {
            native[name] = Boolean(options[name]);
        }
    }
}

// TS 6.0 config parsing stores lib file names (lib.es2020.d.ts), which tsc-rs also accepts.
function translateLib(options, native) {
    if (!Array.isArray(options.lib)) {
        return;
    }
    native.lib = options.lib.filter(lib => typeof lib === "string").map(lib => RENAMED_LIB_FILES.get(lib.toLowerCase()) ?? lib);
}

// TS 6.0 resolves `paths` substitutions against baseUrl, else pathsBasePath (the tsconfig
// directory), else the current directory, and looks up non-relative names under baseUrl when no
// pattern matched. tsc-rs has no baseUrl and resolves against its own current directory, so the
// substitutions are made absolute and baseUrl becomes a last `*` pattern.
// TS 6.0: src/compiler/moduleNameResolver.ts tryLoadModuleUsingOptionalResolutionSettings
function translatePaths(ts, options, native, currentDirectory) {
    const baseUrl = typeof options.baseUrl === "string" ? options.baseUrl : undefined;
    const paths = isPlainObject(options.paths) ? options.paths : undefined;
    if (baseUrl === undefined && paths === undefined) {
        return;
    }
    const pathsBasePath = typeof options.pathsBasePath === "string" ? options.pathsBasePath : undefined;
    const baseDirectory = rootPath(ts, baseUrl ?? pathsBasePath ?? currentDirectory ?? ".", currentDirectory);
    const nativePaths = {};
    for (const [pattern, substitutions] of Object.entries(paths ?? {})) {
        nativePaths[pattern] = Array.isArray(substitutions)
            ? substitutions.map(substitution => rootSubstitution(ts, substitution, baseDirectory, baseUrl !== undefined))
            : substitutions;
    }
    if (baseUrl !== undefined && !Object.hasOwn(nativePaths, "*")) {
        nativePaths["*"] = [ts.ensurePathIsNonModuleName(ts.combinePaths(baseDirectory, "*"))];
    }
    native.paths = nativePaths;
}

// Without baseUrl, TS 6.0 also reports non-relative substitutions (5090); tsc-rs reports them too
// when they reach it unchanged. A base that stays relative (no current directory is known) keeps
// a "./" so that tsc-rs reads it as relative too.
function rootSubstitution(ts, substitution, baseDirectory, hasBaseUrl) {
    if (typeof substitution !== "string" || !hasBaseUrl && !ts.pathIsRelative(substitution)) {
        return substitution;
    }
    return ts.ensurePathIsNonModuleName(ts.getNormalizedAbsolutePath(substitution, baseDirectory));
}

function rootPathOptions(ts, native, currentDirectory) {
    if (currentDirectory === undefined) {
        return;
    }
    for (const name of PATH_OPTIONS) {
        if (typeof native[name] === "string" && native[name] !== "") {
            native[name] = rootPath(ts, native[name], currentDirectory);
        }
    }
    for (const name of PATH_LIST_OPTIONS) {
        if (Array.isArray(native[name])) {
            native[name] = native[name].map(path => typeof path === "string" ? rootPath(ts, path, currentDirectory) : path);
        }
    }
}

function rootPath(ts, path, currentDirectory) {
    return currentDirectory === undefined ? ts.normalizePath(path) : ts.getNormalizedAbsolutePath(path, currentDirectory);
}

function isPlainObject(value) {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function setDefined(object, name, value) {
    if (value !== undefined) {
        object[name] = value;
    }
}

// TS 6.0's deprecation diagnostics for the options that tsc-rs drops or emulates here. tsc-rs
// itself reports "has been removed" for target ES5, module AMD/UMD/System, outFile and
// moduleResolution classic, which pass through unchanged.
// TS 6.0: src/compiler/program.ts:4478 verifyDeprecatedCompilerOptions
function getOptionDiagnostics(ts, options, context) {
    const reporter = createOptionReporter(ts, options);
    const ignoreDeprecations = getIgnoreDeprecationsVersion(ts, options, reporter);
    const typeScriptVersion = new ts.Version(context.typeScriptVersion || ts.versionMajorMinor);

    checkDeprecations(ts, "5.0", "5.5", typeScriptVersion, ignoreDeprecations, reporter, deprecated => {
        if (getEnumValue(ts, "target", options.target) === ts.ScriptTarget.ES3) {
            deprecated("target", "ES3");
        }
        for (const name of ["noImplicitUseStrict", "keyofStringsOnly", "suppressExcessPropertyErrors", "suppressImplicitAnyIndexErrors", "noStrictGenericChecks", "charset", "out"]) {
            if (options[name]) {
                deprecated(name);
            }
        }
        for (const name of ["importsNotUsedAsValues", "preserveValueImports"]) {
            if (options[name]) {
                deprecated(name, undefined, "verbatimModuleSyntax");
            }
        }
    });

    checkDeprecations(ts, "6.0", "7.0", typeScriptVersion, ignoreDeprecations, reporter, deprecated => {
        if (options.alwaysStrict === false) {
            deprecated("alwaysStrict", "false");
        }
        if (getEnumValue(ts, "moduleResolution", options.moduleResolution) === ts.ModuleResolutionKind.Node10) {
            deprecated("moduleResolution", "node10", undefined, ts.Diagnostics.Visit_https_Colon_Slash_Slashaka_ms_Slashts6_for_migration_information);
        }
        if (options.baseUrl !== undefined) {
            deprecated("baseUrl", undefined, undefined, ts.Diagnostics.Visit_https_Colon_Slash_Slashaka_ms_Slashts6_for_migration_information);
        }
        if (options.esModuleInterop === false) {
            deprecated("esModuleInterop", "false");
        }
        if (options.allowSyntheticDefaultImports === false) {
            deprecated("allowSyntheticDefaultImports", "false");
        }
        if (options.downlevelIteration !== undefined) {
            deprecated("downlevelIteration");
        }
    });

    if (getEnumValue(ts, "moduleResolution", options.moduleResolution) === ts.ModuleResolutionKind.Node10) {
        verifyNode10Options(ts, options, reporter);
    }

    // tsc-rs reads ModuleKind 0 as "not set", so it cannot report module None; it is reported the
    // way tsc-rs reports the other removed module kinds.
    if (getEnumValue(ts, "module", options.module) === ts.ModuleKind.None) {
        reporter.removed("module", "None");
    }
    return reporter.diagnostics;
}

// tsc-rs resolves node10 as its default resolution, so it does not report what TS 6.0 reports
// for node10 itself.
// TS 6.0: src/compiler/program.ts:4356 verifyCompilerOptions
function verifyNode10Options(ts, options, reporter) {
    for (const name of ["resolvePackageJsonExports", "resolvePackageJsonImports", "customConditions"]) {
        if (options[name]) {
            reporter.optionName(ts.Diagnostics.Option_0_can_only_be_used_when_moduleResolution_is_set_to_node16_nodenext_or_bundler, name);
        }
    }
    const moduleKind = ts.getEmitModuleKind(options);
    if (ts.ModuleKind[moduleKind] && ts.ModuleKind.Node16 <= moduleKind && moduleKind <= ts.ModuleKind.NodeNext) {
        const moduleKindName = ts.ModuleKind[moduleKind];
        const moduleResolutionName = ts.ModuleResolutionKind[moduleKindName] ? moduleKindName : "Node16";
        reporter.optionValue("moduleResolution", ts.Diagnostics.Option_moduleResolution_must_be_set_to_0_or_left_unspecified_when_option_module_is_set_to_1, moduleResolutionName, moduleKindName);
    }
}

// TS 6.0: src/compiler/program.ts:4431 getIgnoreDeprecationsVersion
function getIgnoreDeprecationsVersion(ts, options, reporter) {
    const ignoreDeprecations = options.ignoreDeprecations;
    if (!ignoreDeprecations) {
        return ts.Version.zero;
    }
    if (ignoreDeprecations === "5.0" || ignoreDeprecations === "6.0") {
        return new ts.Version(ignoreDeprecations);
    }
    reporter.optionValue("ignoreDeprecations", ts.Diagnostics.Invalid_value_for_ignoreDeprecations);
    return ts.Version.zero;
}

// TS 6.0: src/compiler/program.ts:4442 checkDeprecations
function checkDeprecations(ts, deprecatedIn, removedIn, typeScriptVersion, ignoreDeprecations, reporter, check) {
    const deprecatedInVersion = new ts.Version(deprecatedIn);
    const removedInVersion = new ts.Version(removedIn);
    const mustBeRemoved = removedInVersion.compareTo(typeScriptVersion) !== ts.Comparison.GreaterThan;
    const canBeSilenced = !mustBeRemoved && ignoreDeprecations.compareTo(deprecatedInVersion) === ts.Comparison.LessThan;
    if (!mustBeRemoved && !canBeSilenced) {
        return;
    }
    check((name, value, useInstead, related) => {
        if (mustBeRemoved) {
            reporter.deprecated(name, value, useInstead, related, value === undefined
                ? [ts.Diagnostics.Option_0_has_been_removed_Please_remove_it_from_your_configuration, name]
                : [ts.Diagnostics.Option_0_1_has_been_removed_Please_remove_it_from_your_configuration, name, value]);
            return;
        }
        reporter.deprecated(name, value, useInstead, related, value === undefined
            ? [ts.Diagnostics.Option_0_is_deprecated_and_will_stop_functioning_in_TypeScript_1_Specify_compilerOption_ignoreDeprecations_Colon_2_to_silence_this_error, name, removedIn, deprecatedIn]
            : [ts.Diagnostics.Option_0_1_is_deprecated_and_will_stop_functioning_in_TypeScript_2_Specify_compilerOption_ignoreDeprecations_Colon_3_to_silence_this_error, name, value, removedIn, deprecatedIn]);
    });
}

// Places option diagnostics like TS 6.0: on the option (its key or its value) in the tsconfig's
// compilerOptions when options.configFile has it, else on the compilerOptions key, else without a
// file. `place(diagnosticAt)` calls `diagnosticAt(node)` for each place, or with undefined.
// TS 6.0: src/compiler/program.ts:4680 createDiagnosticForOption
function createOptionLocator(ts, options) {
    const configFile = options.configFile?.kind === ts.SyntaxKind.SourceFile ? options.configFile : undefined;
    const compilerOptionsProperty = configFile === undefined
        ? undefined
        : ts.forEachPropertyAssignment(ts.getTsConfigObjectLiteralExpression(configFile), "compilerOptions", property => property);
    const compilerOptionsObject = compilerOptionsProperty === undefined ? undefined : ts.tryCast(compilerOptionsProperty.initializer, ts.isObjectLiteralExpression);

    function placeOption(onKey, key1, key2, diagnosticAt) {
        const nodes = [];
        if (compilerOptionsObject !== undefined) {
            ts.forEachPropertyAssignment(compilerOptionsObject, key1, property => {
                nodes.push(onKey ? property.name : property.initializer);
            }, key2);
        }
        return placeNodes(nodes, diagnosticAt);
    }

    // TS 6.0: src/compiler/program.ts:4617 createDiagnosticForOptionPathKeyValue
    function placePathsValue(key, valueIndex, diagnosticAt) {
        const nodes = [];
        if (compilerOptionsObject !== undefined) {
            ts.forEachPropertyAssignment(compilerOptionsObject, "paths", pathsProperty => {
                if (!ts.isObjectLiteralExpression(pathsProperty.initializer)) {
                    return;
                }
                ts.forEachPropertyAssignment(pathsProperty.initializer, key, keyProperty => {
                    const initializer = keyProperty.initializer;
                    if (ts.isArrayLiteralExpression(initializer) && initializer.elements.length > valueIndex) {
                        nodes.push(initializer.elements[valueIndex]);
                    }
                });
            });
        }
        return placeNodes(nodes, diagnosticAt);
    }

    // TS 6.0: src/compiler/program.ts:4635 createDiagnosticForOptionPaths
    function placePathsKey(onKey, key, diagnosticAt) {
        const nodes = [];
        if (compilerOptionsObject !== undefined) {
            ts.forEachPropertyAssignment(compilerOptionsObject, "paths", pathsProperty => {
                if (ts.isObjectLiteralExpression(pathsProperty.initializer)) {
                    ts.forEachPropertyAssignment(pathsProperty.initializer, key, keyProperty => {
                        nodes.push(onKey ? keyProperty.name : keyProperty.initializer);
                    });
                }
            });
        }
        return placeNodes(nodes, diagnosticAt);
    }

    function placeNodes(nodes, diagnosticAt) {
        if (nodes.length === 0 && compilerOptionsProperty !== undefined) {
            nodes.push(compilerOptionsProperty.name);
        }
        return nodes.length === 0 ? [diagnosticAt(undefined)] : nodes.map(node => diagnosticAt(node));
    }

    return { configFile, placeOption, placePathsKey, placePathsValue };
}

// TS 6.0 diagnostics for options, created and placed the way TS 6.0 program.ts does.
function createOptionReporter(ts, options) {
    const diagnostics = [];
    const locator = createOptionLocator(ts, options);

    // `message` is a DiagnosticMessageChain, or a DiagnosticMessage followed by its arguments.
    function diagnosticAt(message, args) {
        const isChain = "messageText" in message;
        return node => {
            if (node === undefined) {
                return isChain ? ts.createCompilerDiagnosticFromMessageChain(message) : ts.createCompilerDiagnostic(message, ...args);
            }
            return isChain
                ? ts.createDiagnosticForNodeFromMessageChain(locator.configFile, node, message)
                : ts.createDiagnosticForNodeInSourceFile(locator.configFile, node, message, ...args);
        };
    }

    function add(onKey, key1, key2, message, ...args) {
        diagnostics.push(...locator.placeOption(onKey, key1, key2, diagnosticAt(message, args)));
    }

    // TS 6.0: src/compiler/program.ts:4479 verifyDeprecatedCompilerOptions createDiagnostic
    function deprecated(name, value, useInstead, related, [message, ...args]) {
        let details = useInstead === undefined ? undefined : ts.chainDiagnosticMessages(undefined, ts.Diagnostics.Use_0_instead, useInstead);
        if (related !== undefined) {
            details = ts.chainDiagnosticMessages(details, related);
        }
        add(!value, name, undefined, ts.chainDiagnosticMessages(details, message, ...args));
    }

    function removed(name, value) {
        deprecated(name, value, undefined, undefined, [ts.Diagnostics.Option_0_1_has_been_removed_Please_remove_it_from_your_configuration, name, value]);
    }

    // TS 6.0: src/compiler/program.ts:4661 createDiagnosticForOptionName
    function optionName(message, option1, option2, option3) {
        add(true, option1, option2, message, option1, option2, option3);
    }

    // TS 6.0: src/compiler/program.ts:4666 createOptionValueDiagnostic
    function optionValue(name, message, ...args) {
        add(false, name, undefined, message, ...args);
    }

    return { diagnostics, deprecated, removed, optionName, optionValue };
}

// tsc-rs programs have no tsconfig syntax, so tsc-rs reports option diagnostics without a file.
// These are placed in options.configFile where TS 6.0 places them, and the ones TS 6.0 would not
// report for the user's options (they follow from a translated option) are dropped.
// `responses` are tsc-rs DiagnosticResponses of the program; diagnostics with a file pass through.
function toTsOptionDiagnostics(ts, options, responses, getSourceFile) {
    const locator = createOptionLocator(ts, options);
    const invalidSubstitutions = getNonRelativeSubstitutions(ts, options);
    const rules = getPlacementRules(ts);
    const result = [];
    let placedSubstitutions = false;
    for (const response of responses ?? []) {
        if (isTranslationArtifact(ts, options, response)) {
            continue;
        }
        const diagnostic = toTsDiagnostic(ts, response, getSourceFile);
        if (diagnostic.file !== undefined || locator.configFile === undefined) {
            result.push(diagnostic);
            continue;
        }
        const at = node => {
            if (node === undefined) {
                return diagnostic;
            }
            const span = ts.getErrorSpanForNode(locator.configFile, node);
            return { ...diagnostic, file: locator.configFile, start: span.start, length: span.length };
        };
        // tsc-rs reports 5090 once per substitution, without a position, so its identical
        // diagnostics collapse into one; each one goes to its substitution, as TS 6.0 places them.
        const nonRelativePaths = response.code === ts.Diagnostics.Non_relative_paths_are_not_allowed_when_baseUrl_is_not_set_Did_you_forget_a_leading_Slash.code;
        if (nonRelativePaths && (invalidSubstitutions.length > 0 || placedSubstitutions)) {
            for (const [key, valueIndex] of invalidSubstitutions.splice(0)) {
                result.push(...locator.placePathsValue(key, valueIndex, at));
            }
            placedSubstitutions = true;
            continue;
        }
        const rule = rules.get(response.code);
        const args = rule === undefined ? undefined : getMessageArguments(ts, response);
        const place = args === undefined ? undefined : rule(args, options);
        if (place === undefined) {
            result.push(diagnostic);
        }
        else if (place.pathsKey !== undefined) {
            result.push(...locator.placePathsKey(place.onKey, place.pathsKey, at));
        }
        else {
            result.push(...locator.placeOption(place.onKey, place.key1, place.key2, at));
        }
    }
    return result;
}

// tsc-rs resolves module AMD/UMD/System and the dropped node10 as bundler, and reports 5095 for
// that; TS 6.0 reports 5095 only for its own effective bundler resolution.
// TS 6.0: src/compiler/program.ts:4369 verifyCompilerOptions
function isTranslationArtifact(ts, options, response) {
    if (response.code !== ts.Diagnostics.Option_0_can_only_be_used_when_module_is_set_to_preserve_commonjs_or_es2015_or_later.code) {
        return false;
    }
    const moduleKind = ts.getEmitModuleKind(options);
    return !(ts.getEmitModuleResolutionKind(options) === ts.ModuleResolutionKind.Bundler
        && !ts.emitModuleKindIsNonNodeESM(moduleKind)
        && moduleKind !== ts.ModuleKind.Preserve
        && moduleKind !== ts.ModuleKind.CommonJS);
}

// The `paths` substitutions TS 6.0 and tsc-rs report 5090 for, in report order.
// TS 6.0: src/compiler/program.ts:4157 verifyCompilerOptions
function getNonRelativeSubstitutions(ts, options) {
    const result = [];
    if (options.baseUrl || !isPlainObject(options.paths)) {
        return result;
    }
    for (const [key, substitutions] of Object.entries(options.paths)) {
        if (!Array.isArray(substitutions)) {
            continue;
        }
        substitutions.forEach((substitution, index) => {
            if (typeof substitution === "string" && !ts.pathIsRelative(substitution) && !ts.pathIsAbsolute(substitution)) {
                result.push([key, index]);
            }
        });
    }
    return result;
}

const placementRulesByTs = new WeakMap();

// Where TS 6.0's verifyCompilerOptions puts each option diagnostic, from the message arguments.
// TS 6.0: src/compiler/program.ts:4075 verifyCompilerOptions
function getPlacementRules(ts) {
    let rules = placementRulesByTs.get(ts);
    if (rules !== undefined) {
        return rules;
    }
    const d = ts.Diagnostics;
    const byNames = args => ({ onKey: true, key1: args[0], key2: args[1] });
    const onKey = (key1, key2) => () => ({ onKey: true, key1, key2 });
    const onValue = key1 => () => ({ onKey: false, key1 });
    const pathsKey = onKeyOfPattern => args => ({ onKey: onKeyOfPattern, pathsKey: args[0] });
    rules = new Map([
        [d.Option_0_cannot_be_specified_without_specifying_option_1, byNames],
        [d.Option_0_cannot_be_specified_with_option_1, byNames],
        [d.Option_0_cannot_be_specified_without_specifying_option_1_or_option_2, byNames],
        [d.Option_0_can_only_be_used_when_either_option_inlineSourceMap_or_option_sourceMap_is_provided, byNames],
        [d.Option_0_can_only_be_used_when_moduleResolution_is_set_to_node16_nodenext_or_bundler, byNames],
        [d.Option_0_cannot_be_specified_when_option_jsx_is_1, byNames],
        [d.Option_preserveConstEnums_cannot_be_disabled_when_0_is_enabled, args => ({ onKey: true, key1: args[0], key2: "preserveConstEnums" })],
        [d.Option_0_has_been_removed_Please_remove_it_from_your_configuration, args => ({ onKey: true, key1: args[0] })],
        [d.Option_0_1_has_been_removed_Please_remove_it_from_your_configuration, args => ({ onKey: false, key1: args[0] })],
        [d.Option_0_can_only_be_used_when_module_is_set_to_preserve_commonjs_or_es2015_or_later, onValue("moduleResolution")],
        [d.Option_moduleResolution_must_be_set_to_0_or_left_unspecified_when_option_module_is_set_to_1, onValue("moduleResolution")],
        [d.Option_module_must_be_set_to_0_when_option_moduleResolution_is_set_to_1, onValue("module")],
        [d.Option_allowImportingTsExtensions_can_only_be_used_when_one_of_noEmit_emitDeclarationOnly_or_rewriteRelativeImportExtensions_is_set, onValue("allowImportingTsExtensions")],
        [d.Invalid_value_for_jsxFactory_0_is_not_a_valid_identifier_or_qualified_name, onValue("jsxFactory")],
        [d.Invalid_value_for_jsxFragmentFactory_0_is_not_a_valid_identifier_or_qualified_name, onValue("jsxFragmentFactory")],
        [d.Invalid_value_for_reactNamespace_0_is_not_a_valid_identifier, onValue("reactNamespace")],
        [d.Option_verbatimModuleSyntax_cannot_be_used_when_module_is_set_to_UMD_AMD_or_System, onKey("verbatimModuleSyntax")],
        [d.Option_resolveJsonModule_cannot_be_specified_when_moduleResolution_is_set_to_classic, onKey("resolveJsonModule")],
        [d.Option_resolveJsonModule_cannot_be_specified_when_module_is_set_to_none_system_or_umd, onKey("resolveJsonModule", "module")],
        [d.Option_isolatedModules_can_only_be_used_when_either_option_module_is_provided_or_option_target_is_ES2015_or_higher, onKey("isolatedModules", "target")],
        [d.Only_amd_and_system_modules_are_supported_alongside_0, onKey("outFile", "module")],
        [d.Composite_projects_may_not_disable_declaration_emit, onKey("declaration")],
        [d.Composite_projects_may_not_disable_incremental_compilation, onKey("declaration")],
        [d.Cannot_find_the_common_subdirectory_path_for_the_input_files, onKey("outDir")],
        [d.The_common_source_directory_of_0_is_1_The_rootDir_setting_must_be_explicitly_set_to_this_or_another_path_to_adjust_your_output_s_file_layout, (_args, options) => ({
            onKey: true,
            key1: options.outFile ? "outFile" : options.outDir ? "outDir" : "declarationDir",
            key2: !options.outFile && options.outDir ? "declarationDir" : undefined,
        })],
        [d.Pattern_0_can_have_at_most_one_Asterisk_character, pathsKey(true)],
        [d.Substitutions_for_pattern_0_should_be_an_array, pathsKey(false)],
        [d.Substitutions_for_pattern_0_shouldn_t_be_an_empty_array, pathsKey(false)],
    ].map(([message, rule]) => [message.code, rule]));
    placementRulesByTs.set(ts, rules);
    return rules;
}

// The arguments of a tsc-rs message, read back against TS 6.0's text of the same code.
function getMessageArguments(ts, response) {
    const message = getTs6Message(ts, response.code);
    if (message === undefined) {
        return undefined;
    }
    const order = [];
    const pattern = message.message.replace(/[.*+?^$()|[\]\\]|\{(\d+)\}|[{}]/g, (match, index) => {
        if (index !== undefined) {
            order.push(Number(index));
            return "(.*?)";
        }
        return "\\" + match;
    });
    const match = new RegExp(`^${pattern}$`, "s").exec(response.text);
    if (match === null) {
        return undefined;
    }
    const args = [];
    order.forEach((index, i) => {
        args[index] = match[i + 1];
    });
    return args;
}

module.exports = {
    toNativeOptions,
    toTsOptionDiagnostics,
    NATIVE_OPTION_NAMES,
    RENAMED_LIB_FILES,
};
