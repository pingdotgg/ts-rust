"use strict";

const { toTsDiagnostics } = require("./diagnostics.js");
const { transformOutputs, splitSourceMappingUrl, fromInlineSourceMap, isDeclarationOutput } = require("./transformers.js");

// TS 6.0: src/compiler/types.ts EmitOnly
const EmitOnly = { Js: 0, Dts: 1, BuilderSignature: 2 };
// tsc-rs: crates/ts_goport/src/api/session_p2.rs getEmitOnly
const nativeEmitOnly = { all: 0, js: 1, dts: 2 };
const byteOrderMark = "\uFEFF";
const noOutputs = { emitSkipped: false, diagnostics: [], outputFiles: [] };

// TS 6.0: src/compiler/program.ts:2688 emit and emitWorker. tsc-rs emits every output; each one
// then goes to `writeFile` (or the host's) in TS 6.0's order and with TS 6.0's arguments.
function emit(program, targetSourceFile, writeFile, cancellationToken, emitOnly, customTransformers, forceDtsEmit, skipBuildInfo) {
    if (!forceDtsEmit) {
        const result = handleNoEmitOptions(program, targetSourceFile, writeFile, cancellationToken);
        if (result !== undefined) {
            return result;
        }
    }
    cancellationToken?.throwIfCancellationRequested();
    const emitted = targetSourceFile === undefined && !forceDtsEmit && emitOnly !== EmitOnly.BuilderSignature
        ? emitAll(program, emitOnly)
        : emitSelected(program, targetSourceFile, emitOnly, forceDtsEmit);
    const transformers = customTransformersFor(customTransformers, emitOnly);
    if (transformers !== undefined) {
        transformOutputs(program.ts, {
            options: program.options,
            emitHost: createEmitHost(program),
            files: emitted.files,
            customTransformers: transformers,
            reemit: texts => reemitScripts(program, texts),
            cancellationToken,
            directory: nativeDirectoryOf(program),
        });
    }
    return writeEmitted(program, emitted, writeFile, cancellationToken, targetSourceFile === undefined && !skipBuildInfo);
}

// TS 6.0: src/compiler/program.ts:5059 handleNoEmitOptions
function handleNoEmitOptions(program, sourceFile, writeFile, cancellationToken) {
    const { ts } = program;
    const options = program.getCompilerOptions();
    if (options.noEmit) {
        return sourceFile === undefined ? emitBuildInfo(program, writeFile, cancellationToken) : ts.emitSkippedWithNoDiagnostics;
    }
    if (!options.noEmitOnError) {
        return undefined;
    }
    let diagnostics = [
        ...program.getOptionsDiagnostics(cancellationToken),
        ...program.getSyntacticDiagnostics(sourceFile, cancellationToken),
        ...program.getGlobalDiagnostics(cancellationToken),
        ...program.getSemanticDiagnostics(sourceFile, cancellationToken),
    ];
    if (diagnostics.length === 0 && ts.getEmitDeclarations(options)) {
        diagnostics = program.getDeclarationDiagnostics(/*sourceFile*/ undefined, cancellationToken);
    }
    if (diagnostics.length === 0) {
        return undefined;
    }
    let emittedFiles;
    if (sourceFile === undefined) {
        const result = emitBuildInfo(program, writeFile, cancellationToken);
        diagnostics = [...diagnostics, ...result.diagnostics];
        emittedFiles = result.emittedFiles;
    }
    return { diagnostics, sourceMaps: undefined, emittedFiles, emitSkipped: true };
}

// TS 6.0: src/compiler/program.ts:2650 emitBuildInfo. A program that is not a builder program
// writes the build info that TS 6.0 writes for it: just the version.
function emitBuildInfo(program, writeFile) {
    const result = newResult(program);
    writeBuildInfo(program, result, writeFile ?? hostWriteFile(program));
    return finishResult(program.ts, result);
}

function newResult(program) {
    const { ts, options } = program;
    return {
        emitSkipped: false,
        diagnostics: [],
        emittedFiles: options.listEmittedFiles ? [] : undefined,
        sourceMaps: options.sourceMap || options.inlineSourceMap || ts.getAreDeclarationMapsEnabled(options) ? [] : undefined,
    };
}

function finishResult(ts, result) {
    return { ...result, diagnostics: [...ts.sortAndDeduplicateDiagnostics(result.diagnostics)] };
}

// TS 6.0: src/compiler/emitter.ts:808 emitBuildInfo
function writeBuildInfo(program, result, write) {
    const { ts, options } = program;
    const buildInfoPath = ts.getTsBuildInfoEmitOutputFilePath(options);
    if (buildInfoPath === undefined) {
        return;
    }
    if (isEmitBlocked(program, buildInfoPath)) {
        result.emitSkipped = true;
        return;
    }
    const buildInfo = program.getBuildInfo?.() ?? { version: ts.version };
    writeOutput(ts, result, write, buildInfoPath, ts.getBuildInfoText(buildInfo), false, undefined, { buildInfo });
    result.emittedFiles?.push(buildInfoPath);
}

// TS 6.0: src/compiler/utilities.ts:6703 writeFile
function writeOutput(ts, result, write, fileName, text, writeByteOrderMark, sourceFiles, data) {
    const onError = message => {
        result.diagnostics.push(ts.createCompilerDiagnostic(ts.Diagnostics.Could_not_write_file_0_Colon_1, fileName, message));
    };
    write(fileName, text, writeByteOrderMark, onError, sourceFiles, data);
}

function hostWriteFile(program) {
    return (fileName, text, writeByteOrderMark, onError, sourceFiles, data) => program.host.writeFile(fileName, text, writeByteOrderMark, onError, sourceFiles, data);
}

// TS 6.0: src/compiler/transformer.ts:120 getTransformers (script transformers only run when the
// emit is not limited to declarations).
function customTransformersFor(customTransformers, emitOnly) {
    if (customTransformers === undefined) {
        return undefined;
    }
    const before = emitOnly ? [] : customTransformers.before ?? [];
    const after = emitOnly ? [] : customTransformers.after ?? [];
    const afterDeclarations = customTransformers.afterDeclarations ?? [];
    if (before.length === 0 && after.length === 0 && afterDeclarations.length === 0) {
        return undefined;
    }
    return { before, after, afterDeclarations };
}

// The whole program: tsc-rs decides what to emit, as for `tsc`.
function emitAll(program, emitOnly) {
    const { ts, options } = program;
    const response = program.request("emitToString", { emitOnly: toNativeEmitOnly(emitOnly) });
    // TS 6.0: src/compiler/emitter.ts:886 emitDeclarationFileOrBundle skips the emit of a file it
    // has to write a declaration for when declarations are off.
    const noDeclarations = emitOnly !== EmitOnly.Js && (!!emitOnly || !!options.emitDeclarationOnly) && !ts.getEmitDeclarations(options);
    const host = noDeclarations ? createEmitHost(program) : undefined;
    return {
        emitSkipped: response.emitSkipped || (noDeclarations && program.getSourceFiles().some(file => ts.sourceFileMayBeEmitted(file, host, false))),
        diagnostics: response.diagnostics ?? [],
        ...groupOutputs(program, response.outputFiles ?? []),
    };
}

function toNativeEmitOnly(emitOnly) {
    if (emitOnly === EmitOnly.Js) {
        return nativeEmitOnly.js;
    }
    return emitOnly ? nativeEmitOnly.dts : nativeEmitOnly.all;
}

// One file, or every file with forceDtsEmit: TS 6.0 decides which outputs it writes
// (src/compiler/emitter.ts emitJsFileOrBundle and emitDeclarationFileOrBundle) and tsc-rs makes
// them.
function emitSelected(program, targetSourceFile, emitOnly, forceDtsEmit) {
    const { ts, options } = program;
    const host = createEmitHost(program);
    const candidates = targetSourceFile === undefined ? program.getSourceFiles() : [targetSourceFile];
    const sourceFiles = candidates.filter(file => ts.sourceFileMayBeEmitted(file, host, forceDtsEmit));
    const scripts = !emitOnly && !options.emitDeclarationOnly ? sourceFiles : [];
    const declarations = emitOnly !== EmitOnly.Js && (forceDtsEmit || ts.getEmitDeclarations(options))
        ? sourceFiles.filter(file => forceDtsEmit || !ts.isJsonSourceFile(file))
        : [];
    let emitSkipped = emitOnly !== EmitOnly.Js && (!!emitOnly || !!options.emitDeclarationOnly) && declarations.length < sourceFiles.length;
    if (scripts.length !== 0 && options.noEmit) {
        emitSkipped = true;
    }
    const toNative = files => files.map(file => program.toNativeFileName(file.fileName));
    const javaScript = scripts.length !== 0 && !options.noEmit ? program.request("getJavaScriptEmit", { files: toNative(scripts) }) : noOutputs;
    const declaration = declarations.length !== 0 ? program.request("getDeclarationEmit", { files: toNative(declarations) }) : noOutputs;

    const declarationDiagnostics = new Map();
    for (const diagnostic of declaration.diagnostics ?? []) {
        const list = declarationDiagnostics.get(diagnostic.fileName) ?? [];
        list.push(diagnostic);
        declarationDiagnostics.set(diagnostic.fileName, list);
    }
    const signatureOnly = emitOnly === EmitOnly.BuilderSignature;
    const declarationMaps = ts.getAreDeclarationMapsEnabled(options);
    const grouped = groupOutputs(program, [...javaScript.outputFiles ?? [], ...declaration.outputFiles ?? []]);
    for (const file of grouped.files) {
        const diagnostics = declarationDiagnostics.get(file.nativeFileName) ?? [];
        file.declarationDiagnostics = toTsDiagnostics(ts, diagnostics, name => program.sourceFileOfNative(name));
        const declarationOutput = file.outputs.find(output => isDeclarationOutput(ts, output.fileName));
        // TS 6.0: src/compiler/emitter.ts:917 declBlocked
        const declarationBlocked = declarationOutput !== undefined
            && (diagnostics.length !== 0 || isEmitBlocked(program, declarationOutput.fileName) || !!options.noEmit);
        const scriptBlocked = file.outputs.some(output => !isDeclarationRelated(ts, output.fileName) && !output.fileName.endsWith(".map") && isEmitBlocked(program, output.fileName));
        emitSkipped ||= declarationBlocked || scriptBlocked;
        file.outputs = file.outputs.filter(output => {
            if (isDeclarationRelated(ts, output.fileName)) {
                return (!declarationBlocked || !!forceDtsEmit) && (!output.fileName.endsWith(".map") || (declarationMaps && !signatureOnly));
            }
            return !scriptBlocked;
        });
        if (signatureOnly && declarationOutput !== undefined) {
            // TS 6.0 prints a builder signature without a map, but lists the map it would write.
            declarationOutput.text = splitSourceMappingUrl(declarationOutput.text).body;
            file.unwrittenDeclarationMap = declarationMaps ? `${declarationOutput.fileName}.map` : undefined;
        }
    }
    return {
        emitSkipped,
        diagnostics: [...javaScript.diagnostics ?? [], ...declaration.diagnostics ?? []],
        ...grouped,
    };
}

function isDeclarationRelated(ts, fileName) {
    return isDeclarationOutput(ts, fileName.endsWith(".map") ? fileName.slice(0, -".map".length) : fileName);
}

// Outputs by source file, in program file order.
function groupOutputs(program, outputFiles) {
    const bySourceFile = new Map();
    const loose = [];
    for (const { fileName, text, sourceFileName } of outputFiles) {
        if (sourceFileName === undefined) {
            loose.push({ fileName, text });
            continue;
        }
        const outputs = bySourceFile.get(sourceFileName) ?? [];
        outputs.push({ fileName, text });
        bySourceFile.set(sourceFileName, outputs);
    }
    const files = [];
    for (const nativeFileName of program.nativeFileNames) {
        const outputs = bySourceFile.get(nativeFileName);
        if (outputs !== undefined) {
            files.push({ nativeFileName, sourceFile: program.sourceFileOfNative(nativeFileName), outputs });
            bySourceFile.delete(nativeFileName);
        }
    }
    for (const outputs of bySourceFile.values()) {
        loose.push(...outputs);
    }
    return { files, loose };
}

// The JS of files whose `before` transformers changed them, emitted by tsc-rs from their new text
// in a snapshot layered over the program's (the same program otherwise).
function reemitScripts(program, texts) {
    return program.hostResolver === undefined ? reemitLayered(program, texts) : program.hostResolver.whileResolving(() => reemitLayered(program, texts));
}

function reemitLayered(program, texts) {
    const { session } = program;
    program.assertNative?.("emit");
    const layered = program.withFileSystem(() =>
        session.request("updateSnapshot", {
            snapshot: program.snapshot,
            changes: { ensurePrograms: true, fileSystem: { kind: "layer", files: Object.fromEntries(texts) } },
        })
    );
    try {
        const response = program.withFileSystem(() =>
            session.request("getJavaScriptEmit", { snapshot: layered.snapshot, project: program.project, files: [...texts.keys()] })
        );
        const outputs = new Map();
        for (const { fileName, text, sourceFileName } of response.outputFiles ?? []) {
            const list = outputs.get(sourceFileName) ?? [];
            list.push({ fileName, text });
            outputs.set(sourceFileName, list);
        }
        return outputs;
    }
    finally {
        session.request("release", { snapshot: layered.snapshot });
    }
}

// TS 6.0: src/compiler/emitter.ts:752 emitFiles. For each file: the JS map, the JS, the
// declaration map, the declaration; then the build info.
function writeEmitted(program, emitted, writeFile, cancellationToken, includeBuildInfo) {
    const { ts, options } = program;
    const write = writeFile ?? hostWriteFile(program);
    const result = newResult(program);
    result.emitSkipped = emitted.emitSkipped;
    result.diagnostics.push(...toTsDiagnostics(ts, emitted.diagnostics, name => program.sourceFileOfNative(name)));
    const names = new OutputNames(program);
    for (const file of emitted.files) {
        cancellationToken?.throwIfCancellationRequested();
        const sourceFiles = [file.sourceFile];
        const { script, scriptMap, declaration, declarationMap } = splitOutputs(ts, file.outputs);
        const parts = [[script, scriptMap, file.scriptDiagnostics], [declaration, declarationMap, file.declarationDiagnostics]];
        for (const [output, map, diagnostics] of parts) {
            if (output === undefined) {
                continue;
            }
            const fileName = names.get(file.sourceFile, output.fileName);
            const mapFileName = map === undefined ? undefined : names.get(file.sourceFile, map.fileName);
            const sourceMap = sourceMapOf(output.text, map);
            if (sourceMap !== undefined) {
                result.sourceMaps?.push({ inputSourceFileNames: [file.sourceFile.fileName], sourceMap });
            }
            if (map !== undefined) {
                writeOutput(ts, result, write, mapFileName, map.text, false, sourceFiles, undefined);
            }
            const text = output.text.startsWith(byteOrderMark) ? output.text.slice(1) : output.text;
            const data = { sourceMapUrlPos: sourceMapUrlPos(text), diagnostics: diagnostics ?? [] };
            writeOutput(ts, result, write, fileName, text, !!options.emitBOM, sourceFiles, data);
            if (output === script || !data.skippedDtsWrite) {
                result.emittedFiles?.push(fileName);
            }
            if (mapFileName !== undefined) {
                result.emittedFiles?.push(mapFileName);
            }
            if (output === declaration && file.unwrittenDeclarationMap !== undefined) {
                result.emittedFiles?.push(names.get(file.sourceFile, file.unwrittenDeclarationMap));
            }
        }
    }
    for (const output of emitted.loose) {
        writeOutput(ts, result, write, output.fileName, output.text, false, undefined, undefined);
        result.emittedFiles?.push(output.fileName);
    }
    if (includeBuildInfo) {
        writeBuildInfo(program, result, write);
    }
    return finishResult(ts, result);
}

function splitOutputs(ts, outputs) {
    const parts = {};
    for (const output of outputs) {
        const isMap = output.fileName.endsWith(".map");
        const isDeclaration = isDeclarationRelated(ts, output.fileName);
        const key = isDeclaration ? (isMap ? "declarationMap" : "declaration") : (isMap ? "scriptMap" : "script");
        parts[key] ??= output;
    }
    return parts;
}

function sourceMapOf(text, map) {
    if (map !== undefined) {
        return JSON.parse(map.text);
    }
    const { url } = splitSourceMappingUrl(text.startsWith(byteOrderMark) ? text.slice(1) : text);
    return url === undefined ? undefined : fromInlineSourceMap(url);
}

function sourceMapUrlPos(text) {
    const { url, body } = splitSourceMappingUrl(text);
    return url === undefined ? undefined : body.length;
}

// Output file names as TS 6.0 writes them: when a program file name or an output directory is
// relative, TS 6.0's output names are relative too (src/compiler/emitter.ts getOutputPathsFor).
class OutputNames {
    constructor(program) {
        const { ts, options } = program;
        this.program = program;
        this.relativeOptions = [options.outDir, options.declarationDir].some(dir => dir !== undefined && !ts.isRootedDiskPath(dir));
        this.byFile = new Map();
        this.host = undefined;
    }

    get(sourceFile, nativeFileName) {
        const { ts } = this.program;
        if (!this.relativeOptions && ts.isRootedDiskPath(sourceFile.fileName)) {
            return nativeFileName;
        }
        const directory = nativeDirectoryOf(this.program);
        let paths = this.byFile.get(sourceFile);
        if (paths === undefined) {
            this.host ??= createEmitHost(this.program);
            const names = ts.getOutputPathsFor(sourceFile, this.host, /*forceDtsPaths*/ true);
            paths = [names.jsFilePath, names.sourceMapFilePath, names.declarationFilePath, names.declarationMapPath].filter(name => name !== undefined);
            this.byFile.set(sourceFile, paths);
        }
        return paths.find(name => ts.getNormalizedAbsolutePath(name, directory) === nativeFileName) ?? nativeFileName;
    }
}

// The absolute directory tsc-rs resolves the program's relative names against (a host's current
// directory may be "").
function nativeDirectoryOf(program) {
    return program.nativeDirectory ?? program.currentDirectory;
}

// TS 6.0: src/compiler/program.ts isEmitBlocked. TS 6.0 blocks outputs that would overwrite a
// program file.
function isEmitBlocked(program, fileName) {
    const { ts } = program;
    const directory = nativeDirectoryOf(program);
    const toPath = name => ts.toPath(ts.getNormalizedAbsolutePath(name, directory), directory, program.getCanonicalFileName);
    let paths = sourcePaths.get(program);
    if (paths === undefined) {
        paths = new Set(program.nativeFileNames.map(name => toPath(program.toJsFileName(name))));
        sourcePaths.set(program, paths);
    }
    return paths.has(toPath(fileName));
}

const sourcePaths = new WeakMap();

// TS 6.0: src/compiler/program.ts:2598 getEmitHost, for transformation contexts and TS 6.0's
// output path functions.
function createEmitHost(program) {
    const { ts } = program;
    let commonSourceDirectory;
    return {
        getCanonicalFileName: program.getCanonicalFileName,
        getCommonSourceDirectory: () => commonSourceDirectory ??= program.getCommonSourceDirectory(),
        getCompilerOptions: () => program.options,
        getCurrentDirectory: () => program.currentDirectory,
        getSourceFile: fileName => program.getSourceFile(fileName),
        getSourceFileByPath: path => program.getSourceFileByPath(path),
        getSourceFiles: () => program.getSourceFiles(),
        isSourceFileFromExternalLibrary: file => program.isSourceFileFromExternalLibrary(file),
        getRedirectFromSourceFile: () => undefined,
        isSourceOfProjectReferenceRedirect: () => false,
        writeFile: hostWriteFile(program),
        isEmitBlocked: fileName => isEmitBlocked(program, fileName),
        getEmitModuleFormatOfFile: file => ts.getEmitModuleFormatOfFileWorker(file, program.options),
        getDefaultResolutionModeForFile: file => ts.getDefaultResolutionModeForFileWorker(file, program.options),
        getModeForResolutionAtIndex: (file, index) => program.getModeForResolutionAtIndex(file, index),
        readFile: fileName => program.host.readFile(fileName),
        fileExists: fileName => program.host.fileExists(fileName),
        useCaseSensitiveFileNames: () => program.useCaseSensitiveFileNames(),
        getBuildInfo: () => program.getBuildInfo?.(),
        redirectTargetsMap: new Map(),
        getFileIncludeReasons: () => program.getFileIncludeReasons(),
    };
}

module.exports = { emit, emitBuildInfo, handleNoEmitOptions, createEmitHost, EmitOnly };
