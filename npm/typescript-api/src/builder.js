"use strict";

// TS 6.0's builder programs (src/compiler/builder.ts, builderState.ts, builderPublic.ts) over a
// tsc-rs NativeProgram. Every TS 6.0 builder creator (createSemanticDiagnosticsBuilderProgram,
// createEmitAndSemanticDiagnosticsBuilderProgram, createIncrementalProgram, the watch and solution
// builder hosts) ends in the createBuilderProgram hook, so TS 6.0's own watch and tsc -b code runs
// on these. The affected-file state machine is TS 6.0's. What TS 6.0 asks its checker for comes
// from elsewhere: file references from ambient module declarations and the tsc-rs program's module
// resolutions, file shapes from tsc-rs declaration emit. No .tsbuildinfo is read or written: a
// TS 6.0 one cannot seed a tsc-rs program, and a file in TS 6.0's format with tsc-rs signatures and
// diagnostics would mislead a real TS 6.0 that reads it (see emitBuildInfo).

const { NativeProgram } = require("./program.js");
const { toTsDiagnostics } = require("./diagnostics.js");

const bundledLibs = "bundled:///libs/";
const nativeState = Symbol("tsc-rs builder state");

// TS 6.0: src/compiler/builder.ts:187 BuilderFileEmit
const BuilderFileEmit = {
    None: 0,
    Js: 1,
    JsMap: 2,
    JsInlineMap: 4,
    DtsErrors: 8,
    DtsEmit: 16,
    DtsMap: 32,
    Dts: 24,
    AllJs: 7,
    AllDtsEmit: 48,
    AllDts: 56,
    All: 63,
};

// The resolution modes a tsc-rs program keys its module resolutions by (none, CommonJS, ESNext).
const resolutionModes = [0, 1, 99];

// TS 6.0: src/compiler/builder.ts:1673 createBuilderProgram
function createBuilderProgram(ts, kind, { newProgram, host = {}, oldProgram, configFileParsingDiagnostics }) {
    const oldState = oldProgram?.state;
    if (oldState !== undefined && newProgram === oldState.program && configFileParsingDiagnostics === newProgram.getConfigFileParsingDiagnostics()) {
        return oldProgram;
    }

    const state = createBuilderProgramState(ts, newProgram, oldState?.[nativeState] ? oldState : undefined, host);
    const builderProgram = {
        state,
        getState: () => state,
        getProgram,
        getProgramOrUndefined: () => state.program,
        releaseProgram: () => releaseCache(state),
        getCompilerOptions: () => state.compilerOptions,
        getSourceFile: fileName => getProgram().getSourceFile(fileName),
        getSourceFiles: () => getProgram().getSourceFiles(),
        getOptionsDiagnostics: cancellationToken => getProgram().getOptionsDiagnostics(cancellationToken),
        getGlobalDiagnostics: cancellationToken => getProgram().getGlobalDiagnostics(cancellationToken),
        getConfigFileParsingDiagnostics: () => configFileParsingDiagnostics,
        getSyntacticDiagnostics: (sourceFile, cancellationToken) => getProgram().getSyntacticDiagnostics(sourceFile, cancellationToken),
        getDeclarationDiagnostics,
        getSemanticDiagnostics,
        emit,
        emitBuildInfo,
        getAllDependencies: sourceFile => getAllDependencies(ts, state, sourceFile),
        getCurrentDirectory: () => getProgram().getCurrentDirectory(),
        hasChangedEmitSignature: () => !!state.hasChangedEmitSignature,
        close() {},
    };
    if (kind === ts.BuilderProgramKind.SemanticDiagnosticsBuilderProgram) {
        builderProgram.getSemanticDiagnosticsOfNextAffectedFile = getSemanticDiagnosticsOfNextAffectedFile;
    }
    else if (kind === ts.BuilderProgramKind.EmitAndSemanticDiagnosticsBuilderProgram) {
        builderProgram.getSemanticDiagnosticsOfNextAffectedFile = getSemanticDiagnosticsOfNextAffectedFile;
        builderProgram.emitNextAffectedFile = emitNextAffectedFile;
    }
    else {
        throw new Error(`tsc-rs: createBuilderProgram is not supported: unknown builder kind ${kind}`);
    }
    return builderProgram;

    function getProgram() {
        return ts.Debug.checkDefined(state.program);
    }

    // TS 6.0: src/compiler/builder.ts:1720 emitBuildInfo. TS 6.0 writes .tsbuildinfo here when
    // incremental or composite is set; these builders write none (see the file header), so the
    // pending build info step only completes.
    function emitBuildInfo() {
        ts.Debug.assert(state.program !== undefined);
        if (!state.buildInfoEmitPending) {
            return ts.emitSkippedWithNoDiagnostics;
        }
        state.buildInfoEmitPending = false;
        return { emitSkipped: false, diagnostics: ts.emptyArray, emittedFiles: state.compilerOptions.listEmittedFiles ? [] : undefined, sourceMaps: undefined };
    }

    // TS 6.0: src/compiler/builder.ts:1741 emitNextAffectedFileOrDtsErrors
    function emitNextAffectedFileOrDtsErrors(writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers, isForDtsErrors) {
        ts.Debug.assert(state.program !== undefined);
        let affected = getNextAffectedFile(ts, state, cancellationToken, host);
        const programEmitKind = ts.getBuilderFileEmit(state.compilerOptions);
        let emitKind = !isForDtsErrors
            ? emitOnlyDtsFiles ? programEmitKind & BuilderFileEmit.AllDts : programEmitKind
            : BuilderFileEmit.DtsErrors;
        if (!affected) {
            if (!state.compilerOptions.outFile) {
                const pendingAffectedFile = getNextAffectedFilePendingEmit(ts, state, emitOnlyDtsFiles, isForDtsErrors);
                if (pendingAffectedFile) {
                    ({ affectedFile: affected, emitKind } = pendingAffectedFile);
                }
                else {
                    const pendingForDiagnostics = getNextPendingEmitDiagnosticsFile(ts, state, isForDtsErrors);
                    if (pendingForDiagnostics) {
                        (state.seenEmittedFiles ??= new Map()).set(
                            pendingForDiagnostics.affectedFile.resolvedPath,
                            pendingForDiagnostics.seenKind | getBuilderFileEmitAllDts(isForDtsErrors),
                        );
                        return {
                            result: { emitSkipped: true, diagnostics: pendingForDiagnostics.diagnostics },
                            affected: pendingForDiagnostics.affectedFile,
                        };
                    }
                }
            }
            else {
                if (state.programEmitPending) {
                    emitKind = ts.getPendingEmitKindWithSeen(state.programEmitPending, state.seenProgramEmit, emitOnlyDtsFiles, isForDtsErrors);
                    if (emitKind) {
                        affected = state.program;
                    }
                }
                if (!affected && state.emitDiagnosticsPerFile?.size) {
                    const seenKind = state.seenProgramEmit || BuilderFileEmit.None;
                    if (!(seenKind & getBuilderFileEmitAllDts(isForDtsErrors))) {
                        state.seenProgramEmit = getBuilderFileEmitAllDts(isForDtsErrors) | seenKind;
                        const diagnostics = [];
                        state.emitDiagnosticsPerFile.forEach(list => ts.addRange(diagnostics, list));
                        return { result: { emitSkipped: true, diagnostics }, affected: state.program };
                    }
                }
            }
            if (!affected) {
                if (isForDtsErrors || !state.buildInfoEmitPending) {
                    return undefined;
                }
                return { result: emitBuildInfo(), affected: state.program };
            }
        }

        const result = !isForDtsErrors
            ? emitFile(affected === state.program ? undefined : affected, getWriteFileCallback(writeFile, customTransformers), cancellationToken, emitKind, customTransformers)
            : { emitSkipped: true, diagnostics: getDeclarationDiagnosticsOf(affected === state.program ? undefined : affected, cancellationToken) };
        if (affected !== state.program) {
            const affectedPath = affected.resolvedPath;
            (state.seenAffectedFiles ??= new Set()).add(affectedPath);
            if (state.affectedFilesIndex !== undefined) {
                state.affectedFilesIndex++;
            }
            state.buildInfoEmitPending = true;
            const existing = state.seenEmittedFiles?.get(affectedPath) || BuilderFileEmit.None;
            (state.seenEmittedFiles ??= new Map()).set(affectedPath, emitKind | existing);
            const existingPending = state.affectedFilesPendingEmit?.get(affectedPath) || programEmitKind;
            const pendingKind = getPendingEmitKind(ts, existingPending, emitKind | existing);
            if (pendingKind) {
                (state.affectedFilesPendingEmit ??= new Map()).set(affectedPath, pendingKind);
            }
            else {
                state.affectedFilesPendingEmit?.delete(affectedPath);
            }
            if (result.diagnostics.length) {
                (state.emitDiagnosticsPerFile ??= new Map()).set(affectedPath, result.diagnostics);
            }
        }
        else {
            state.changedFilesSet.clear();
            state.programEmitPending = state.changedFilesSet.size
                ? getPendingEmitKind(ts, programEmitKind, emitKind)
                : state.programEmitPending ? getPendingEmitKind(ts, state.programEmitPending, emitKind) : undefined;
            state.seenProgramEmit = emitKind | (state.seenProgramEmit || BuilderFileEmit.None);
            setEmitDiagnosticsPerFile(result.diagnostics);
            state.buildInfoEmitPending = true;
        }
        return { result, affected };
    }

    // TS 6.0: src/compiler/builder.ts:1876 setEmitDiagnosticsPerFile
    function setEmitDiagnosticsPerFile(diagnostics) {
        let emitDiagnosticsPerFile;
        for (const diagnostic of diagnostics) {
            if (!diagnostic.file) {
                continue;
            }
            let list = emitDiagnosticsPerFile?.get(diagnostic.file.resolvedPath);
            if (!list) {
                (emitDiagnosticsPerFile ??= new Map()).set(diagnostic.file.resolvedPath, list = []);
            }
            list.push(diagnostic);
        }
        if (emitDiagnosticsPerFile) {
            state.emitDiagnosticsPerFile = emitDiagnosticsPerFile;
        }
    }

    function emitNextAffectedFile(writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers) {
        return emitNextAffectedFileOrDtsErrors(writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers, /*isForDtsErrors*/ false);
    }

    // TS 6.0: src/compiler/builder.ts:1909 getWriteFileCallback. The declaration file signatures
    // that it records are what tsc -b compares to skip the projects that depend on this one.
    function getWriteFileCallback(writeFile, customTransformers) {
        ts.Debug.assert(state.program !== undefined);
        if (!ts.getEmitDeclarations(state.compilerOptions)) {
            return writeFile || host.writeFile?.bind(host);
        }
        return (fileName, text, writeByteOrderMark, onError, sourceFiles, data) => {
            if (ts.isDeclarationFileName(fileName)) {
                if (!state.compilerOptions.outFile) {
                    const file = sourceFiles?.length === 1 ? sourceFiles[0] : undefined;
                    const info = file && state.fileInfos.get(file.resolvedPath);
                    let emitSignature;
                    if (info && !customTransformers && info.signature === info.version) {
                        const signature = ts.computeSignatureWithDiagnostics(state.program, file, text, host, data);
                        if (!data?.diagnostics?.length) {
                            emitSignature = signature;
                        }
                        if (signature !== info.version) {
                            if (host.storeSignatureInfo) {
                                (state.signatureInfo ??= new Map()).set(file.resolvedPath, ts.SignatureInfo.StoredSignatureAtEmit);
                            }
                            if (state.affectedFiles) {
                                const existing = state.oldSignatures?.get(file.resolvedPath);
                                if (existing === undefined) {
                                    (state.oldSignatures ??= new Map()).set(file.resolvedPath, info.signature || false);
                                }
                            }
                            info.signature = signature;
                        }
                    }
                    if (state.compilerOptions.composite) {
                        if (file === undefined) {
                            state.hasChangedEmitSignature = true;
                            state.latestChangedDtsFile = fileName;
                        }
                        else {
                            const filePath = file.resolvedPath;
                            emitSignature = handleNewSignature(state.emitSignatures?.get(filePath), emitSignature);
                            if (!emitSignature) {
                                if (data) {
                                    data.skippedDtsWrite = true;
                                }
                                return;
                            }
                            (state.emitSignatures ??= new Map()).set(filePath, emitSignature);
                        }
                    }
                }
                else if (state.compilerOptions.composite) {
                    const newSignature = handleNewSignature(state.outSignature, /*newSignature*/ undefined);
                    if (!newSignature) {
                        if (data) {
                            data.skippedDtsWrite = true;
                        }
                        return;
                    }
                    state.outSignature = newSignature;
                }
            }
            if (writeFile) {
                writeFile(fileName, text, writeByteOrderMark, onError, sourceFiles, data);
            }
            else if (host.writeFile) {
                host.writeFile(fileName, text, writeByteOrderMark, onError, sourceFiles, data);
            }
            else {
                state.program.host.writeFile(fileName, text, writeByteOrderMark, onError, sourceFiles, data);
            }

            function handleNewSignature(oldSignatureFormat, newSignature) {
                const oldSignature = !oldSignatureFormat || ts.isString(oldSignatureFormat) ? oldSignatureFormat : oldSignatureFormat[0];
                newSignature ??= (host.createHash ?? ts.generateDjb2Hash)(text);
                if (newSignature === oldSignature) {
                    if (oldSignatureFormat === oldSignature) {
                        return undefined;
                    }
                    if (data) {
                        data.differsOnlyInMap = true;
                    }
                    else {
                        data = { differsOnlyInMap: true };
                    }
                }
                else {
                    state.hasChangedEmitSignature = true;
                    state.latestChangedDtsFile = fileName;
                }
                return newSignature;
            }
        };
    }

    // TS 6.0: src/compiler/builder.ts:2005 emit
    function emit(targetSourceFile, writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers) {
        ts.Debug.assert(state.program !== undefined);
        if (kind === ts.BuilderProgramKind.EmitAndSemanticDiagnosticsBuilderProgram) {
            assertSourceFileOkWithoutNextAffectedCall(ts, state, targetSourceFile);
        }
        const result = ts.handleNoEmitOptions(builderProgram, targetSourceFile, writeFile, cancellationToken);
        if (result) {
            return result;
        }

        if (!targetSourceFile) {
            if (kind === ts.BuilderProgramKind.EmitAndSemanticDiagnosticsBuilderProgram) {
                const whole = emitPendingFilesTogether(writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers);
                if (whole) {
                    return whole;
                }
                let sourceMaps = [];
                let emitSkipped = false;
                let diagnostics;
                let emittedFiles = [];
                let affectedEmitResult;
                while ((affectedEmitResult = emitNextAffectedFile(writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers))) {
                    emitSkipped = emitSkipped || affectedEmitResult.result.emitSkipped;
                    diagnostics = ts.addRange(diagnostics, affectedEmitResult.result.diagnostics);
                    emittedFiles = ts.addRange(emittedFiles, affectedEmitResult.result.emittedFiles);
                    sourceMaps = ts.addRange(sourceMaps, affectedEmitResult.result.sourceMaps);
                }
                return { emitSkipped, diagnostics: diagnostics || ts.emptyArray, emittedFiles, sourceMaps };
            }
            clearAffectedFilesPendingEmit(state, emitOnlyDtsFiles, /*isForDtsErrors*/ false);
        }
        const programEmitKind = ts.getBuilderFileEmit(state.compilerOptions);
        const emitResult = emitFile(
            targetSourceFile,
            getWriteFileCallback(writeFile, customTransformers),
            cancellationToken,
            emitOnlyDtsFiles ? programEmitKind & BuilderFileEmit.AllDts : programEmitKind,
            customTransformers,
        );
        handleNonEmitBuilderWithEmitOrDtsErrors(targetSourceFile, emitOnlyDtsFiles, /*isForDtsErrors*/ false, emitResult.diagnostics);
        return emitResult;
    }

    // When every file that can be emitted waits for the same full emit (a first build, where TS 6.0
    // would emit them one by one), tsc-rs emits the whole program in one request. The state after
    // it is the state TS 6.0 reaches after its loop.
    function emitPendingFilesTogether(writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers) {
        // Taking the affected files out of the change set before emitting them gives TS 6.0's
        // result only while this builder has emitted nothing.
        if (state.compilerOptions.outFile || state.seenEmittedFiles?.size) {
            return undefined;
        }
        let affected;
        while ((affected = getNextAffectedFile(ts, state, cancellationToken, host))) {
            state.seenAffectedFiles.add(affected.resolvedPath);
            state.affectedFilesIndex++;
            state.buildInfoEmitPending = true;
        }
        const programEmitKind = ts.getBuilderFileEmit(state.compilerOptions);
        const emitKind = emitOnlyDtsFiles ? programEmitKind & BuilderFileEmit.AllDts : programEmitKind;
        const files = getAllFilesExcludingDefaultLibraryFile(ts, state, undefined).filter(file => mayBeEmitted(ts, state, file));
        if (files.length < 2) {
            return undefined;
        }
        for (const file of files) {
            const pending = state.affectedFilesPendingEmit?.get(file.resolvedPath);
            if (pending === undefined || ts.getPendingEmitKindWithSeen(pending, state.seenEmittedFiles?.get(file.resolvedPath), emitOnlyDtsFiles, false) !== emitKind) {
                return undefined;
            }
        }
        const result = emitFile(undefined, getWriteFileCallback(writeFile, customTransformers), cancellationToken, emitKind, customTransformers);
        const emittedPaths = new Set(files.map(file => file.resolvedPath));
        for (const path of state.affectedFilesPendingEmit.keys()) {
            if (!emittedPaths.has(path)) {
                state.affectedFilesPendingEmit.delete(path);
            }
        }
        for (const file of files) {
            const path = file.resolvedPath;
            const existing = state.seenEmittedFiles?.get(path) || BuilderFileEmit.None;
            (state.seenEmittedFiles ??= new Map()).set(path, emitKind | existing);
            const pendingKind = getPendingEmitKind(ts, state.affectedFilesPendingEmit.get(path) || programEmitKind, emitKind | existing);
            if (pendingKind) {
                state.affectedFilesPendingEmit.set(path, pendingKind);
            }
            else {
                state.affectedFilesPendingEmit.delete(path);
            }
            state.emitDiagnosticsPerFile?.delete(path);
        }
        for (const diagnostic of result.diagnostics) {
            if (diagnostic.file && state.fileInfos.has(diagnostic.file.resolvedPath)) {
                const list = state.emitDiagnosticsPerFile?.get(diagnostic.file.resolvedPath);
                if (list) {
                    list.push(diagnostic);
                }
                else {
                    (state.emitDiagnosticsPerFile ??= new Map()).set(diagnostic.file.resolvedPath, [diagnostic]);
                }
            }
        }
        // TS 6.0's loop ends with the build info step.
        state.buildInfoEmitPending = false;
        return {
            emitSkipped: result.emitSkipped,
            diagnostics: result.diagnostics,
            emittedFiles: result.emittedFiles ?? [],
            sourceMaps: result.sourceMaps ?? [],
        };
    }

    // TS 6.0: src/compiler/builder.ts:1828 state.program.emit, for one file or (undefined) the
    // program, writing only the outputs that `emitKind` asks for.
    function emitFile(sourceFile, writeFile, cancellationToken, emitKind, customTransformers) {
        const { program, compilerOptions } = state;
        if (sourceFile !== undefined && !compilerOptions.noEmit && !compilerOptions.noEmitOnError && !mayBeEmitted(ts, state, sourceFile)) {
            return { emitSkipped: false, diagnostics: ts.emptyArray, emittedFiles: compilerOptions.listEmittedFiles ? [] : undefined, sourceMaps: undefined };
        }
        let emitOnly;
        if (emitKind & BuilderFileEmit.AllJs) {
            emitOnly = ts.EmitOnly.Js;
        }
        if (emitKind & BuilderFileEmit.AllDts) {
            emitOnly = emitOnly === undefined ? ts.EmitOnly.Dts : undefined;
        }
        if (emitOnly === ts.EmitOnly.Js && !ts.getEmitDeclarations(compilerOptions)) {
            emitOnly = undefined;
        }
        const writesJs = (emitKind & BuilderFileEmit.AllJs) !== 0;
        const writesDts = (emitKind & BuilderFileEmit.AllDtsEmit) !== 0 && ts.getEmitDeclarations(compilerOptions);
        const buildInfoPath = ts.getTsBuildInfoEmitOutputFilePath(compilerOptions);
        const skippedOutputs = new Set();
        const write = (fileName, text, writeByteOrderMark, onError, sourceFiles, data) => {
            // TS 6.0 builders emit with skipBuildInfo; these write no .tsbuildinfo (see emitBuildInfo).
            if (fileName === buildInfoPath || data?.buildInfo !== undefined || !(isDeclarationOutput(ts, fileName) ? writesDts : writesJs)) {
                skippedOutputs.add(fileName);
                return;
            }
            const outputSourceFiles = sourceFiles ?? (sourceFile !== undefined ? [sourceFile] : declarationSourceFiles(fileName));
            const callback = writeFile ?? ((...args) => program.host.writeFile(...args));
            callback(fileName, text, writeByteOrderMark, onError, outputSourceFiles, data ?? {});
        };
        const result = program.emit(sourceFile, write, cancellationToken, emitOnly, customTransformers, /*forceDtsEmit*/ undefined, /*skipBuildInfo*/ true);
        if (skippedOutputs.size === 0 || result.emittedFiles === undefined) {
            return result;
        }
        return { ...result, emittedFiles: result.emittedFiles.filter(fileName => !skippedOutputs.has(fileName)) };
    }

    // The source file of a declaration output of a whole-program emit (TS 6.0's emitter passes it to
    // writeFile; tsc-rs names only the output).
    function declarationSourceFiles(fileName) {
        if (!ts.isDeclarationFileName(fileName)) {
            return undefined;
        }
        const { program, compilerOptions } = state;
        if (state.declarationOutputs === undefined) {
            const files = getAllFilesExcludingDefaultLibraryFile(ts, state, undefined).filter(file => mayBeEmitted(ts, state, file));
            // TS 6.0: src/compiler/program.ts getCommonSourceDirectory, over the emitted files.
            const commonSourceDirectory = ts.getCommonSourceDirectory(
                compilerOptions,
                () => files.filter(file => !file.isDeclarationFile).map(file => file.fileName),
                program.getCurrentDirectory(),
                program.getCanonicalFileName,
            );
            const emitHost = {
                getCurrentDirectory: () => program.getCurrentDirectory(),
                getCommonSourceDirectory: () => commonSourceDirectory,
                getCanonicalFileName: program.getCanonicalFileName,
            };
            state.declarationOutputs = new Map(files.map(file => [program.toPath(ts.getDeclarationEmitOutputFilePathWorker(file.fileName, compilerOptions, emitHost)), file]));
        }
        const file = state.declarationOutputs.get(program.toPath(fileName));
        return file === undefined ? undefined : [file];
    }

    // TS 6.0: src/compiler/builder.ts:2074 handleNonEmitBuilderWithEmitOrDtsErrors
    function handleNonEmitBuilderWithEmitOrDtsErrors(targetSourceFile, emitOnlyDtsFiles, isForDtsErrors, diagnostics) {
        if (!targetSourceFile && kind !== ts.BuilderProgramKind.EmitAndSemanticDiagnosticsBuilderProgram) {
            clearAffectedFilesPendingEmit(state, emitOnlyDtsFiles, isForDtsErrors);
            setEmitDiagnosticsPerFile(diagnostics);
        }
    }

    function getDeclarationDiagnosticsOf(sourceFile, cancellationToken) {
        if (sourceFile !== undefined && !mayBeEmitted(ts, state, sourceFile)) {
            return ts.emptyArray;
        }
        return state.program.getDeclarationDiagnostics(sourceFile, cancellationToken);
    }

    // TS 6.0: src/compiler/builder.ts:2086 getDeclarationDiagnostics
    function getDeclarationDiagnostics(sourceFile, cancellationToken) {
        ts.Debug.assert(state.program !== undefined);
        if (kind === ts.BuilderProgramKind.EmitAndSemanticDiagnosticsBuilderProgram) {
            assertSourceFileOkWithoutNextAffectedCall(ts, state, sourceFile);
            let affectedEmitResult;
            let diagnostics;
            while ((affectedEmitResult = emitNextAffectedFileOrDtsErrors(undefined, cancellationToken, undefined, undefined, /*isForDtsErrors*/ true))) {
                if (!sourceFile) {
                    diagnostics = ts.addRange(diagnostics, affectedEmitResult.result.diagnostics);
                }
            }
            return (!sourceFile ? diagnostics : state.emitDiagnosticsPerFile?.get(sourceFile.resolvedPath)) || ts.emptyArray;
        }
        const result = state.program.getDeclarationDiagnostics(sourceFile, cancellationToken);
        handleNonEmitBuilderWithEmitOrDtsErrors(sourceFile, /*emitOnlyDtsFiles*/ undefined, /*isForDtsErrors*/ true, result);
        return result;
    }

    // TS 6.0: src/compiler/builder.ts:2126 getSemanticDiagnosticsOfNextAffectedFile
    function getSemanticDiagnosticsOfNextAffectedFile(cancellationToken, ignoreSourceFile) {
        ts.Debug.assert(state.program !== undefined);
        while (true) {
            const affected = getNextAffectedFile(ts, state, cancellationToken, host);
            let result;
            if (!affected) {
                if (state.checkPending && !state.compilerOptions.noCheck) {
                    state.checkPending = undefined;
                    state.buildInfoEmitPending = true;
                }
                return undefined;
            }
            else if (affected !== state.program) {
                if (!ignoreSourceFile || !ignoreSourceFile(affected)) {
                    result = getSemanticDiagnosticsOfFile(ts, state, affected.resolvedPath, cancellationToken);
                }
                state.seenAffectedFiles.add(affected.resolvedPath);
                state.affectedFilesIndex++;
                state.buildInfoEmitPending = true;
                if (!result) {
                    continue;
                }
            }
            else {
                let diagnostics;
                const semanticDiagnosticsPerFile = new Map();
                for (const path of state.fileInfos.keys()) {
                    diagnostics = ts.addRange(diagnostics, getSemanticDiagnosticsOfFile(ts, state, path, cancellationToken, semanticDiagnosticsPerFile));
                }
                state.semanticDiagnosticsPerFile = semanticDiagnosticsPerFile;
                result = diagnostics || ts.emptyArray;
                state.changedFilesSet.clear();
                state.programEmitPending = ts.getBuilderFileEmit(state.compilerOptions);
                if (!state.compilerOptions.noCheck) {
                    state.checkPending = undefined;
                }
                state.buildInfoEmitPending = true;
            }
            return { result, affected };
        }
    }

    // TS 6.0: src/compiler/builder.ts:2187 getSemanticDiagnostics
    function getSemanticDiagnostics(sourceFile, cancellationToken) {
        ts.Debug.assert(state.program !== undefined);
        assertSourceFileOkWithoutNextAffectedCall(ts, state, sourceFile);
        if (sourceFile) {
            if (!state.fileInfos.has(sourceFile.resolvedPath)) {
                return state.program.getSemanticDiagnostics(sourceFile, cancellationToken);
            }
            return getSemanticDiagnosticsOfFile(ts, state, sourceFile.resolvedPath, cancellationToken);
        }
        while (true) {
            const affectedResult = getSemanticDiagnosticsOfNextAffectedFile(cancellationToken);
            if (!affectedResult) {
                break;
            }
            if (affectedResult.affected === state.program) {
                return affectedResult.result;
            }
        }
        let diagnostics;
        for (const path of state.fileInfos.keys()) {
            diagnostics = ts.addRange(diagnostics, getSemanticDiagnosticsOfFile(ts, state, path, cancellationToken));
        }
        if (state.checkPending && !state.compilerOptions.noCheck) {
            state.checkPending = undefined;
            state.buildInfoEmitPending = true;
        }
        return diagnostics || ts.emptyArray;
    }
}

// TS 6.0: src/compiler/builder.ts:321 createBuilderProgramState with
// src/compiler/builderState.ts:308 BuilderState.create
function createBuilderProgramState(ts, newProgram, oldState, host) {
    const compilerOptions = newProgram.getCompilerOptions();
    const files = programFiles(ts, newProgram, host);
    let referencedMap = compilerOptions.module !== ts.ModuleKind.None && !compilerOptions.outFile
        ? ts.BuilderState.createManyToManyPathMap()
        : undefined;
    const moduleReferences = referencedMap === undefined ? undefined : addReferences(ts, newProgram, files, referencedMap, oldState);
    if (moduleReferences === undefined) {
        // Without module resolutions every file counts as depending on every other one.
        referencedMap = undefined;
    }
    // TS 6.0: src/compiler/builderState.ts canReuseOldState
    const useOldState = oldState !== undefined && !oldState.referencedMap === !referencedMap;
    const fileInfos = new Map();
    for (const file of files) {
        const oldUncommittedSignature = useOldState ? oldState.oldSignatures?.get(file.path) : undefined;
        const signature = oldUncommittedSignature === undefined
            ? useOldState ? oldState.fileInfos.get(file.path)?.signature : undefined
            : oldUncommittedSignature || undefined;
        fileInfos.set(file.path, {
            version: file.version,
            signature,
            affectsGlobalScope: !compilerOptions.outFile ? file.affectsGlobalScope || undefined : undefined,
            impliedFormat: file.impliedFormat,
        });
    }

    const state = {
        [nativeState]: true,
        fileInfos,
        referencedMap,
        useFileVersionAsSignature: !useOldState,
        program: newProgram,
        compilerOptions,
        semanticDiagnosticsPerFile: new Map(),
        changedFilesSet: new Set(),
        affectedFiles: undefined,
        affectedFilesIndex: undefined,
        seenAffectedFiles: undefined,
        // TS 6.0 computes an unset value from whether the program has errors, for the build info;
        // with no build info written, unset is not pending.
        buildInfoEmitPending: false,
        seenEmittedFiles: undefined,
        seenProgramEmit: undefined,
        latestChangedDtsFile: compilerOptions.composite ? oldState?.latestChangedDtsFile : undefined,
        checkPending: compilerOptions.noCheck ? true : undefined,
        moduleReferences,
        nativeFileNames: new Map(files.map(file => [file.path, file.nativeFileName])),
        defaultLibraryPaths: new Set(files.filter(file => file.isDefaultLibrary).map(file => file.path)),
        freshSemanticDiagnostics: new Map(),
        externalLibraryFiles: new Map(),
        declarationOutputs: undefined,
    };
    const outFilePath = compilerOptions.outFile;
    if (outFilePath && compilerOptions.composite && oldState?.outSignature && outFilePath === oldState.compilerOptions.outFile) {
        state.outSignature = getEmitSignatureFromOldSignature(ts, compilerOptions, oldState.compilerOptions, oldState.outSignature);
    }

    const oldCompilerOptions = useOldState ? oldState.compilerOptions : undefined;
    let canCopySemanticDiagnostics = useOldState && !ts.compilerOptionsAffectSemanticDiagnostics(compilerOptions, oldCompilerOptions);
    const canCopyEmitSignatures = compilerOptions.composite
        && oldState?.emitSignatures
        && !outFilePath
        && !ts.compilerOptionsAffectDeclarationPath(compilerOptions, oldState.compilerOptions);
    let canCopyEmitDiagnostics = true;
    if (useOldState) {
        oldState.changedFilesSet?.forEach(path => state.changedFilesSet.add(path));
        if (!outFilePath && oldState.affectedFilesPendingEmit?.size) {
            state.affectedFilesPendingEmit = new Map(oldState.affectedFilesPendingEmit);
            state.seenAffectedFiles = new Set();
        }
        state.programEmitPending = oldState.programEmitPending;
        if (outFilePath && state.changedFilesSet.size) {
            canCopySemanticDiagnostics = false;
            canCopyEmitDiagnostics = false;
        }
        state.hasErrorsFromOldState = oldState.hasErrors;
    }
    else {
        state.buildInfoEmitPending = ts.isIncrementalCompilation(compilerOptions);
    }

    const oldReferencedMap = useOldState ? oldState.referencedMap : undefined;
    const copyDeclarationFileDiagnostics = canCopySemanticDiagnostics && !compilerOptions.skipLibCheck === !oldCompilerOptions.skipLibCheck;
    const copyLibFileDiagnostics = copyDeclarationFileDiagnostics && !compilerOptions.skipDefaultLibCheck === !oldCompilerOptions.skipDefaultLibCheck;
    for (const file of files) {
        const info = fileInfos.get(file.path);
        let oldInfo;
        let newReferences;
        if (
            !useOldState
            || !(oldInfo = oldState.fileInfos.get(file.path))
            || oldInfo.version !== info.version
            || oldInfo.impliedFormat !== info.impliedFormat
            || !hasSameKeys(newReferences = referencedMap?.getValues(file.path), oldReferencedMap?.getValues(file.path))
            || newReferences && ts.forEachKey(newReferences, path => !fileInfos.has(path) && oldState.fileInfos.has(path))
        ) {
            addFileToChangeSet(file.path);
        }
        else {
            const emitDiagnostics = canCopyEmitDiagnostics ? oldState.emitDiagnosticsPerFile?.get(file.path) : undefined;
            if (emitDiagnostics) {
                (state.emitDiagnosticsPerFile ??= new Map()).set(file.path, repopulateDiagnostics(newProgram, emitDiagnostics));
            }
            if (canCopySemanticDiagnostics && (!file.isDeclarationFile || copyDeclarationFileDiagnostics) && (!file.isDefaultLibrary || copyLibFileDiagnostics)) {
                const diagnostics = oldState.semanticDiagnosticsPerFile.get(file.path);
                if (diagnostics) {
                    state.semanticDiagnosticsPerFile.set(file.path, repopulateDiagnostics(newProgram, diagnostics));
                    (state.semanticDiagnosticsFromOldState ??= new Set()).add(file.path);
                }
            }
        }
        if (canCopyEmitSignatures) {
            const oldEmitSignature = oldState.emitSignatures.get(file.path);
            if (oldEmitSignature) {
                (state.emitSignatures ??= new Map()).set(file.path, getEmitSignatureFromOldSignature(ts, compilerOptions, oldState.compilerOptions, oldEmitSignature));
            }
        }
    }

    if (
        useOldState && ts.forEachEntry(oldState.fileInfos, (info, path) => {
            if (fileInfos.has(path)) {
                return false;
            }
            if (info.affectsGlobalScope) {
                return true;
            }
            state.buildInfoEmitPending = true;
            return !!outFilePath;
        })
    ) {
        for (const file of getAllFilesExcludingDefaultLibraryFile(ts, state, undefined)) {
            addFileToChangeSet(file.resolvedPath);
        }
    }
    else if (oldCompilerOptions) {
        const pendingEmitKind = ts.compilerOptionsAffectEmit(compilerOptions, oldCompilerOptions)
            ? ts.getBuilderFileEmit(compilerOptions)
            : getPendingEmitKind(ts, compilerOptions, oldCompilerOptions);
        if (pendingEmitKind !== BuilderFileEmit.None) {
            if (!outFilePath) {
                for (const path of fileInfos.keys()) {
                    if (!state.changedFilesSet.has(path)) {
                        addToAffectedFilesPendingEmit(state, path, pendingEmitKind);
                    }
                }
                state.seenAffectedFiles ??= new Set();
            }
            else if (!state.changedFilesSet.size) {
                state.programEmitPending = state.programEmitPending ? state.programEmitPending | pendingEmitKind : pendingEmitKind;
            }
            state.buildInfoEmitPending = true;
        }
    }
    if (useOldState && state.semanticDiagnosticsPerFile.size !== fileInfos.size && oldState.checkPending !== state.checkPending) {
        state.buildInfoEmitPending = true;
    }
    return state;

    function addFileToChangeSet(path) {
        state.changedFilesSet.add(path);
        if (outFilePath) {
            canCopySemanticDiagnostics = false;
            canCopyEmitDiagnostics = false;
            state.semanticDiagnosticsFromOldState = undefined;
            state.semanticDiagnosticsPerFile.clear();
            state.emitDiagnosticsPerFile = undefined;
        }
        state.buildInfoEmitPending = true;
        state.programEmitPending = undefined;
    }
}

// The files of a program with what the builder keeps about them. Default library files are
// tsc-rs's bundled ones: they never change, so their TS 6.0 source files are not made for this.
function programFiles(ts, program, host) {
    return program.nativeFileNames.map(nativeFileName => {
        const path = program.toPath(program.toJsFileName(nativeFileName));
        if (nativeFileName.startsWith(bundledLibs)) {
            return { path, nativeFileName, version: nativeFileName, affectsGlobalScope: libraryHasStatements(ts, program, nativeFileName), impliedFormat: undefined, isDefaultLibrary: true, isDeclarationFile: true };
        }
        const sourceFile = program.sourceFileOfNative(nativeFileName);
        const references = collectExternalModuleReferences(ts, sourceFile);
        return {
            path,
            nativeFileName,
            sourceFile,
            references,
            // TS 6.0 builders require host-set versions; files that tsc-rs read itself get one here.
            version: sourceFile.version ?? ts.getSourceFileVersionAsHashFromText(host, sourceFile.text),
            affectsGlobalScope: isFileAffectingGlobalScope(ts, sourceFile, references),
            impliedFormat: sourceFile.impliedNodeFormat,
            isDefaultLibrary: false,
            isDeclarationFile: sourceFile.isDeclarationFile,
        };
    });
}

// A default library file is a script, so TS 6.0 takes it to affect every file unless it has no
// statements (the files that only reference other libraries). Bundled libraries never change, so
// each is looked at once per process.
const librariesWithStatements = new Map();

function libraryHasStatements(ts, program, nativeFileName) {
    let hasStatements = librariesWithStatements.get(nativeFileName);
    if (hasStatements === undefined) {
        const text = ts.sys.readFile(program.toJsFileName(nativeFileName)) ?? program.nativeFile(nativeFileName).text;
        const scanner = ts.createScanner(ts.ScriptTarget.Latest, /*skipTrivia*/ true, ts.LanguageVariant.Standard, text);
        hasStatements = scanner.scan() !== ts.SyntaxKind.EndOfFileToken;
        librariesWithStatements.set(nativeFileName, hasStatements);
    }
    return hasStatements;
}

// TS 6.0: src/compiler/program.ts:3318 collectExternalModuleReferences, without the synthesized
// tslib and JSX runtime imports. Ambient module names are kept for every script file.
function collectExternalModuleReferences(ts, file) {
    const isExternalModuleFile = ts.isExternalModule(file);
    const imports = [];
    const moduleAugmentations = [];
    const ambientModuleNames = [];
    const collectModuleReferences = (node, inAmbientModule) => {
        if (ts.isAnyImportOrReExport(node)) {
            const moduleNameExpr = ts.getExternalModuleName(node);
            if (moduleNameExpr && ts.isStringLiteral(moduleNameExpr) && moduleNameExpr.text && (!inAmbientModule || !ts.isExternalModuleNameRelative(moduleNameExpr.text))) {
                imports.push(moduleNameExpr);
            }
            return;
        }
        if (!ts.isModuleDeclaration(node) || !ts.isAmbientModule(node)) {
            return;
        }
        if (!inAmbientModule && !ts.hasSyntacticModifier(node, ts.ModifierFlags.Ambient) && !file.isDeclarationFile) {
            return;
        }
        const nameText = ts.getTextOfIdentifierOrLiteral(node.name);
        if (isExternalModuleFile || inAmbientModule && !ts.isExternalModuleNameRelative(nameText)) {
            moduleAugmentations.push(node.name);
            return;
        }
        if (inAmbientModule) {
            return;
        }
        ambientModuleNames.push(nameText);
        for (const statement of node.body?.statements ?? ts.emptyArray) {
            collectModuleReferences(statement, /*inAmbientModule*/ true);
        }
    };
    for (const statement of file.statements) {
        collectModuleReferences(statement, /*inAmbientModule*/ false);
    }
    if ((file.flags & ts.NodeFlags.PossiblyContainsDynamicImport) || ts.isSourceFileJS(file)) {
        ts.forEachDynamicImportOrRequireCall(file, /*includeTypeSpaceImports*/ true, /*requireStringLiteralLikeArgument*/ true, (_node, moduleSpecifier) => {
            imports.push(moduleSpecifier);
        });
    }
    return { imports, moduleAugmentations, ambientModuleNames };
}

// TS 6.0: src/compiler/builderState.ts:541 isFileAffectingGlobalScope
function isFileAffectingGlobalScope(ts, sourceFile, references) {
    return references.moduleAugmentations.some(name => ts.isGlobalScopeAugmentation(name.parent))
        || !ts.isExternalOrCommonJsModule(sourceFile) && !ts.isJsonSourceFile(sourceFile) && !sourceFile.statements.every(ts.isModuleWithStringLiteralName);
}

// TS 6.0: src/compiler/builderState.ts:215 getReferencedFiles. TS 6.0 asks its checker which files
// declare each imported module; here the files come from the ambient module declarations of the
// program's files and from the tsc-rs program's module resolutions. Returns the files that each
// file's module names and type reference directives resolve to, or undefined when tsc-rs cannot
// answer. A file whose text did not change keeps the old state's resolutions when the program has
// the same files and resolution options.
function addReferences(ts, program, files, referencedMap, oldState) {
    const options = program.getCompilerOptions();
    const reusable = oldState?.moduleReferences !== undefined
        && oldState.fileInfos.size === files.length
        && files.every(file => oldState.fileInfos.has(file.path))
        && !ts.changesAffectModuleResolution(oldState.compilerOptions, options);
    const moduleReferences = new Map();
    const lookups = [];
    // TS 6.0 adds the files of every ambient module with more than one declaration (from its
    // checker's getAmbientModules) to the references of every file.
    const ambientModules = new Map();
    const addAmbientModuleDeclaration = (name, path) => {
        const ambientModule = ambientModules.get(name) ?? { declarations: 0, files: new Set() };
        ambientModule.declarations++;
        ambientModule.files.add(path);
        ambientModules.set(name, ambientModule);
    };
    for (const file of files) {
        const { sourceFile, references } = file;
        if (sourceFile === undefined) {
            continue;
        }
        if (!ts.isExternalModule(sourceFile)) {
            for (const name of references.ambientModuleNames) {
                addAmbientModuleDeclaration(name, file.path);
            }
        }
        const oldInfo = reusable ? oldState.fileInfos.get(file.path) : undefined;
        const oldReferences = oldState?.moduleReferences?.get(file.path);
        if (oldInfo?.version === file.version && oldInfo.impliedFormat === file.impliedFormat && oldReferences !== undefined) {
            moduleReferences.set(file.path, oldReferences);
            continue;
        }
        file.resolved = [];
        for (const name of references.imports) {
            lookups.push({
                file,
                method: "getResolvedModule",
                params: { file: file.nativeFileName, moduleName: name.text },
                mode: ts.getModeForUsageLocation(sourceFile, name, options) ?? 0,
            });
        }
        for (const name of references.moduleAugmentations.filter(ts.isStringLiteral)) {
            lookups.push({
                file,
                method: "getResolvedModule",
                params: { file: file.nativeFileName, moduleName: name.text },
                mode: ts.getModeForUsageLocation(sourceFile, name, options) ?? 0,
                augmentation: name.text,
            });
        }
        for (const reference of sourceFile.typeReferenceDirectives) {
            lookups.push({
                file,
                method: "getResolvedTypeReferenceDirective",
                params: { file: file.nativeFileName, typeDirectiveName: ts.toFileNameLowerCase(reference.fileName) },
                mode: ts.getModeForFileReference(reference, sourceFile.impliedNodeFormat) ?? 0,
            });
        }
    }
    if (!resolveLookups(program, lookups)) {
        return undefined;
    }
    // An augmentation that resolves to no file augments an ambient module.
    for (const lookup of lookups) {
        if (lookup.augmentation !== undefined && !lookup.found) {
            addAmbientModuleDeclaration(lookup.augmentation, lookup.file.path);
        }
    }

    const programPaths = new Set(files.map(file => file.path));
    const pathOfResolution = resolution => {
        for (const fileName of [resolution.resolvedFileName, resolution.originalPath]) {
            const path = fileName && program.toPath(program.toJsFileName(fileName));
            if (path && programPaths.has(path)) {
                return path;
            }
            // A source of a referenced project resolves to its declaration output in the program
            // (TS 6.0: src/compiler/program.ts getRedirectFromSourceFile).
            const outputDts = path && program.getRedirectFromSourceFile(fileName)?.outputDts;
            const outputPath = outputDts && program.toPath(outputDts);
            if (outputPath && programPaths.has(outputPath)) {
                return outputPath;
            }
        }
        return undefined;
    };
    const mergedAmbientModuleFiles = new Set();
    for (const ambientModule of ambientModules.values()) {
        if (ambientModule.declarations > 1) {
            ambientModule.files.forEach(path => mergedAmbientModuleFiles.add(path));
        }
    }
    for (const file of files) {
        const { sourceFile } = file;
        if (sourceFile === undefined) {
            continue;
        }
        if (file.resolved !== undefined) {
            const resolvedPaths = new Set();
            for (const resolution of file.resolved) {
                const path = pathOfResolution(resolution);
                if (path !== undefined) {
                    resolvedPaths.add(path);
                }
            }
            moduleReferences.set(file.path, resolvedPaths);
            file.resolved = undefined;
        }
        const referencedFiles = new Set(moduleReferences.get(file.path));
        for (const name of file.references.imports) {
            ambientModules.get(name.text)?.files.forEach(path => referencedFiles.add(path));
        }
        for (const name of file.references.moduleAugmentations) {
            ambientModules.get(name.text)?.files.forEach(path => path !== file.path && referencedFiles.add(path));
        }
        const sourceFileDirectory = ts.getDirectoryPath(file.path);
        for (const reference of sourceFile.referencedFiles) {
            referencedFiles.add(ts.toPath(reference.fileName, sourceFileDirectory, program.getCanonicalFileName));
        }
        for (const path of mergedAmbientModuleFiles) {
            if (path !== file.path) {
                referencedFiles.add(path);
            }
        }
        if (referencedFiles.size) {
            referencedMap.set(file.path, referencedFiles);
        }
    }
    return moduleReferences;
}

// Asks tsc-rs for every lookup in one batch, then again with the other resolution modes for the
// ones that found nothing (a program keys its resolutions by the mode it resolved them in).
function resolveLookups(program, lookups) {
    let pending = lookups.map(lookup => ({ lookup, modes: [lookup.mode, ...resolutionModes.filter(mode => mode !== lookup.mode)] }));
    while (pending.length) {
        const responses = batchRequests(program, pending.map(({ lookup, modes }) => ({
            method: lookup.method,
            params: { snapshot: program.snapshot, project: program.project, ...lookup.params, mode: modes[0] },
        })));
        const next = [];
        for (const [index, entry] of pending.entries()) {
            const response = responses[index];
            if (response?.error) {
                return false;
            }
            if (response?.result) {
                entry.lookup.found = true;
                entry.lookup.file.resolved.push(response.result);
            }
            else if (entry.modes.length > 1) {
                next.push({ lookup: entry.lookup, modes: entry.modes.slice(1) });
            }
        }
        pending = next;
    }
    return true;
}

function batchRequests(program, requests) {
    return program.withFileSystem(() => {
        let response = program.session.request("batchRequests", { requests });
        const responses = response.responses;
        while (response.continuationToken) {
            response = program.session.request("batchRequests", { requests: [], continuationToken: response.continuationToken });
            responses.push(...response.responses);
        }
        return responses;
    });
}

// TS 6.0: src/compiler/builder.ts repopulateDiagnostics: diagnostics kept from the old program
// point at the new program's source files.
function repopulateDiagnostics(program, diagnostics) {
    const fileOf = file => (file === undefined ? undefined : program.getSourceFileByPath(file.resolvedPath) ?? file);
    return diagnostics.map(diagnostic => {
        const file = fileOf(diagnostic.file);
        const relatedInformation = diagnostic.relatedInformation?.map(related => {
            const relatedFile = fileOf(related.file);
            return relatedFile === related.file ? related : { ...related, file: relatedFile };
        });
        if (file === diagnostic.file && relatedInformation?.every((related, index) => related === diagnostic.relatedInformation[index]) !== false) {
            return diagnostic;
        }
        return { ...diagnostic, file, relatedInformation };
    });
}

// TS 6.0: src/compiler/builder.ts getEmitSignatureFromOldSignature
function getEmitSignatureFromOldSignature(ts, options, oldOptions, oldEmitSignature) {
    if (!!options.declarationMap === !!oldOptions.declarationMap) {
        return oldEmitSignature;
    }
    return ts.isString(oldEmitSignature) ? [oldEmitSignature] : oldEmitSignature[0];
}

// TS 6.0: src/compiler/builder.ts:291 getPendingEmitKind
function getPendingEmitKind(ts, optionsOrEmitKind, oldOptionsOrEmitKind) {
    return ts.getPendingEmitKindWithSeen(optionsOrEmitKind, oldOptionsOrEmitKind, /*emitOnlyDtsFiles*/ false, /*isForDtsErrors*/ false);
}

function hasSameKeys(map1, map2) {
    return map1 === map2 || map1 !== undefined && map2 !== undefined && map1.size === map2.size && [...map1.keys()].every(key => map2.has(key));
}

function releaseCache(state) {
    state.allFilesExcludingDefaultLibraryFile = undefined;
    state.allFileNames = undefined;
    state.program = undefined;
    state.freshSemanticDiagnostics = new Map();
    state.declarationOutputs = undefined;
}

// TS 6.0: src/compiler/builder.ts assertSourceFileOkWithoutNextAffectedCall
function assertSourceFileOkWithoutNextAffectedCall(ts, state, sourceFile) {
    ts.Debug.assert(!sourceFile || !state.affectedFiles || state.affectedFiles[state.affectedFilesIndex - 1] !== sourceFile || !state.semanticDiagnosticsPerFile.has(sourceFile.resolvedPath));
}

// TS 6.0: src/compiler/builder.ts:635 getNextAffectedFile
function getNextAffectedFile(ts, state, cancellationToken, host) {
    while (true) {
        const { affectedFiles } = state;
        if (affectedFiles) {
            const seenAffectedFiles = state.seenAffectedFiles;
            let affectedFilesIndex = state.affectedFilesIndex;
            while (affectedFilesIndex < affectedFiles.length) {
                const affectedFile = affectedFiles[affectedFilesIndex];
                if (!seenAffectedFiles.has(affectedFile.resolvedPath)) {
                    state.affectedFilesIndex = affectedFilesIndex;
                    addToAffectedFilesPendingEmit(state, affectedFile.resolvedPath, ts.getBuilderFileEmit(state.compilerOptions));
                    handleDtsMayChangeOfAffectedFile(ts, state, affectedFile, cancellationToken, host);
                    return affectedFile;
                }
                affectedFilesIndex++;
            }
            state.changedFilesSet.delete(state.currentChangedFilePath);
            state.currentChangedFilePath = undefined;
            state.oldSignatures?.clear();
            state.affectedFiles = undefined;
        }
        const nextKey = state.changedFilesSet.keys().next();
        if (nextKey.done) {
            return undefined;
        }
        if (state.compilerOptions.outFile) {
            return state.program;
        }
        state.affectedFiles = getFilesAffectedByWithOldState(ts, state, nextKey.value, cancellationToken, host);
        state.currentChangedFilePath = nextKey.value;
        state.affectedFilesIndex = 0;
        state.seenAffectedFiles ??= new Set();
    }
}

// TS 6.0: src/compiler/builder.ts:700 clearAffectedFilesPendingEmit
function clearAffectedFilesPendingEmit(state, emitOnlyDtsFiles, isForDtsErrors) {
    if (!state.affectedFilesPendingEmit?.size && !state.programEmitPending) {
        return;
    }
    if (!emitOnlyDtsFiles && !isForDtsErrors) {
        state.affectedFilesPendingEmit = undefined;
        state.programEmitPending = undefined;
    }
    const keptKinds = !isForDtsErrors ? BuilderFileEmit.AllJs : BuilderFileEmit.AllJs | BuilderFileEmit.AllDtsEmit;
    state.affectedFilesPendingEmit?.forEach((emitKind, path) => {
        const pending = emitKind & keptKinds;
        if (!pending) {
            state.affectedFilesPendingEmit.delete(path);
        }
        else {
            state.affectedFilesPendingEmit.set(path, pending);
        }
    });
    if (state.programEmitPending) {
        const pending = state.programEmitPending & keptKinds;
        state.programEmitPending = pending || undefined;
    }
}

function getBuilderFileEmitAllDts(isForDtsErrors) {
    return !isForDtsErrors ? BuilderFileEmit.AllDts : BuilderFileEmit.DtsErrors;
}

// TS 6.0: src/compiler/builder.ts:751 getNextAffectedFilePendingEmit
function getNextAffectedFilePendingEmit(ts, state, emitOnlyDtsFiles, isForDtsErrors) {
    if (!state.affectedFilesPendingEmit?.size) {
        return undefined;
    }
    return ts.forEachEntry(state.affectedFilesPendingEmit, (emitKind, path) => {
        const affectedFile = state.defaultLibraryPaths.has(path) ? undefined : state.program.getSourceFileByPath(path);
        if (!affectedFile || !mayBeEmitted(ts, state, affectedFile)) {
            state.affectedFilesPendingEmit.delete(path);
            return undefined;
        }
        const seenKind = state.seenEmittedFiles?.get(affectedFile.resolvedPath);
        const pendingKind = ts.getPendingEmitKindWithSeen(emitKind, seenKind, emitOnlyDtsFiles, isForDtsErrors);
        return pendingKind ? { affectedFile, emitKind: pendingKind } : undefined;
    });
}

// TS 6.0: src/compiler/builder.ts:774 getNextPendingEmitDiagnosticsFile
function getNextPendingEmitDiagnosticsFile(ts, state, isForDtsErrors) {
    if (!state.emitDiagnosticsPerFile?.size) {
        return undefined;
    }
    return ts.forEachEntry(state.emitDiagnosticsPerFile, (diagnostics, path) => {
        const affectedFile = state.defaultLibraryPaths.has(path) ? undefined : state.program.getSourceFileByPath(path);
        if (!affectedFile || !mayBeEmitted(ts, state, affectedFile)) {
            state.emitDiagnosticsPerFile.delete(path);
            return undefined;
        }
        const seenKind = state.seenEmittedFiles?.get(affectedFile.resolvedPath) || BuilderFileEmit.None;
        return !(seenKind & getBuilderFileEmitAllDts(isForDtsErrors)) ? { affectedFile, diagnostics, seenKind } : undefined;
    });
}

// TS 6.0: src/compiler/builder.ts:790 removeDiagnosticsOfLibraryFiles; default libraries are
// skipped from checking with skipLibCheck or skipDefaultLibCheck (skipTypeCheckingIgnoringNoCheck).
function removeDiagnosticsOfLibraryFiles(state) {
    if (state.cleanedDiagnosticsOfLibFiles) {
        return;
    }
    state.cleanedDiagnosticsOfLibFiles = true;
    if (state.compilerOptions.skipLibCheck || state.compilerOptions.skipDefaultLibCheck) {
        return;
    }
    for (const path of state.defaultLibraryPaths) {
        removeSemanticDiagnosticsOf(state, path);
    }
}

// TS 6.0: src/compiler/builder.ts:805 handleDtsMayChangeOfAffectedFile
function handleDtsMayChangeOfAffectedFile(ts, state, affectedFile, cancellationToken, host) {
    removeSemanticDiagnosticsOf(state, affectedFile.resolvedPath);
    if (state.allFilesExcludingDefaultLibraryFile === state.affectedFiles) {
        removeDiagnosticsOfLibraryFiles(state);
        updateShapeSignature(ts, state, affectedFile.resolvedPath, affectedFile, cancellationToken, host);
        return;
    }
    if (state.compilerOptions.assumeChangesOnlyAffectDirectDependencies) {
        return;
    }
    handleDtsMayChangeOfReferencingExportOfAffectedFile(ts, state, affectedFile, cancellationToken, host);
}

// TS 6.0: src/compiler/builder.ts:841 handleDtsMayChangeOf
function handleDtsMayChangeOf(ts, state, path, invalidateJsFiles, cancellationToken, host) {
    removeSemanticDiagnosticsOf(state, path);
    if (state.changedFilesSet.has(path) || state.defaultLibraryPaths.has(path)) {
        return;
    }
    const sourceFile = state.program.getSourceFileByPath(path);
    if (!sourceFile) {
        return;
    }
    updateShapeSignature(ts, state, path, sourceFile, cancellationToken, host, /*useFileVersionAsSignature*/ true);
    if (invalidateJsFiles) {
        addToAffectedFilesPendingEmit(state, path, ts.getBuilderFileEmit(state.compilerOptions));
    }
    else if (ts.getEmitDeclarations(state.compilerOptions)) {
        addToAffectedFilesPendingEmit(state, path, state.compilerOptions.declarationMap ? BuilderFileEmit.AllDts : BuilderFileEmit.Dts);
    }
}

// TS 6.0: src/compiler/builder.ts:889 removeSemanticDiagnosticsOf
function removeSemanticDiagnosticsOf(state, path) {
    if (!state.semanticDiagnosticsFromOldState) {
        return true;
    }
    state.semanticDiagnosticsFromOldState.delete(path);
    state.semanticDiagnosticsPerFile.delete(path);
    return !state.semanticDiagnosticsFromOldState.size;
}

// TS 6.0: src/compiler/builder.ts:898 isChangedSignature
function isChangedSignature(ts, state, path) {
    const oldSignature = ts.Debug.checkDefined(state.oldSignatures).get(path) || undefined;
    const newSignature = ts.Debug.checkDefined(state.fileInfos.get(path)).signature;
    return newSignature !== oldSignature;
}

// TS 6.0: src/compiler/builder.ts:904 handleDtsMayChangeOfGlobalScope
function handleDtsMayChangeOfGlobalScope(ts, state, filePath, invalidateJsFiles, cancellationToken, host) {
    if (!state.fileInfos.get(filePath)?.affectsGlobalScope) {
        return false;
    }
    for (const file of getAllFilesExcludingDefaultLibraryFile(ts, state, undefined)) {
        handleDtsMayChangeOf(ts, state, file.resolvedPath, invalidateJsFiles, cancellationToken, host);
    }
    removeDiagnosticsOfLibraryFiles(state);
    return true;
}

// TS 6.0: src/compiler/builder.ts:933 handleDtsMayChangeOfReferencingExportOfAffectedFile
function handleDtsMayChangeOfReferencingExportOfAffectedFile(ts, state, affectedFile, cancellationToken, host) {
    if (!state.referencedMap || !state.changedFilesSet.has(affectedFile.resolvedPath)) {
        return;
    }
    if (!isChangedSignature(ts, state, affectedFile.resolvedPath)) {
        return;
    }
    if (ts.getIsolatedModules(state.compilerOptions)) {
        const seenFileNamesMap = new Map([[affectedFile.resolvedPath, true]]);
        const queue = getReferencedByPaths(state, affectedFile.resolvedPath);
        while (queue.length > 0) {
            const currentPath = queue.pop();
            if (!seenFileNamesMap.has(currentPath)) {
                seenFileNamesMap.set(currentPath, true);
                if (handleDtsMayChangeOfGlobalScope(ts, state, currentPath, /*invalidateJsFiles*/ false, cancellationToken, host)) {
                    return;
                }
                handleDtsMayChangeOf(ts, state, currentPath, /*invalidateJsFiles*/ false, cancellationToken, host);
                if (isChangedSignature(ts, state, currentPath)) {
                    queue.push(...getReferencedByPaths(state, currentPath));
                }
            }
        }
    }

    const seenFileAndExportsOfFile = new Set();
    // TS 6.0 asks its checker whether the file exports a const enum declared in it; any top-level
    // const enum counts here.
    const invalidateJsFiles = affectedFile.statements.some(statement => ts.isEnumDeclaration(statement) && ts.isEnumConst(statement));
    state.referencedMap.getKeys(affectedFile.resolvedPath)?.forEach(exportedFromPath => {
        if (handleDtsMayChangeOfGlobalScope(ts, state, exportedFromPath, invalidateJsFiles, cancellationToken, host)) {
            return true;
        }
        const references = state.referencedMap.getKeys(exportedFromPath);
        return references && ts.forEachKey(references, filePath =>
            handleDtsMayChangeOfFileAndExportsOfFile(ts, state, filePath, invalidateJsFiles, seenFileAndExportsOfFile, cancellationToken, host));
    });
}

// TS 6.0: src/compiler/builder.ts:1004 handleDtsMayChangeOfFileAndExportsOfFile
function handleDtsMayChangeOfFileAndExportsOfFile(ts, state, filePath, invalidateJsFiles, seenFileAndExportsOfFile, cancellationToken, host) {
    if (!ts.tryAddToSet(seenFileAndExportsOfFile, filePath)) {
        return undefined;
    }
    if (handleDtsMayChangeOfGlobalScope(ts, state, filePath, invalidateJsFiles, cancellationToken, host)) {
        return true;
    }
    handleDtsMayChangeOf(ts, state, filePath, invalidateJsFiles, cancellationToken, host);
    state.referencedMap.getKeys(filePath)?.forEach(referencingFilePath =>
        handleDtsMayChangeOfFileAndExportsOfFile(ts, state, referencingFilePath, invalidateJsFiles, seenFileAndExportsOfFile, cancellationToken, host));
    return undefined;
}

// TS 6.0: src/compiler/builder.ts:1035 getSemanticDiagnosticsOfFile and :1052
// getBinderAndCheckerDiagnosticsOfFile
function getSemanticDiagnosticsOfFile(ts, state, path, cancellationToken, semanticDiagnosticsPerFile = state.semanticDiagnosticsPerFile) {
    if (state.compilerOptions.noCheck) {
        return ts.emptyArray;
    }
    const cachedDiagnostics = semanticDiagnosticsPerFile.get(path);
    if (cachedDiagnostics) {
        return ts.filterSemanticDiagnostics(cachedDiagnostics, state.compilerOptions);
    }
    cancellationToken?.throwIfCancellationRequested();
    const diagnostics = getProgramSemanticDiagnostics(ts, state, path);
    semanticDiagnosticsPerFile.set(path, diagnostics);
    state.buildInfoEmitPending = true;
    return ts.filterSemanticDiagnostics(diagnostics, state.compilerOptions);
}

// The program's semantic diagnostics of one file. The first time a file that is not a default
// library is asked for, the other such files whose diagnostics the state does not hold yet are
// asked for in the same request. Default library files go one by one: TS 6.0 checks the big ones
// only when every file's diagnostics are asked for.
function getProgramSemanticDiagnostics(ts, state, path) {
    const fresh = state.freshSemanticDiagnostics.get(path);
    if (fresh !== undefined) {
        return fresh;
    }
    const { program } = state;
    const wanted = state.defaultLibraryPaths.has(path)
        ? [path]
        : [...state.fileInfos.keys()].filter(candidate =>
            candidate === path
            || !state.defaultLibraryPaths.has(candidate) && !state.semanticDiagnosticsPerFile.has(candidate) && !state.freshSemanticDiagnostics.has(candidate));
    const responses = program.request("getSemanticDiagnostics", { files: wanted.map(candidate => state.nativeFileNames.get(candidate)) }) ?? [];
    const byPath = new Map(wanted.map(candidate => [candidate, []]));
    for (const diagnostic of toTsDiagnostics(ts, responses, name => program.sourceFileOfNative(name))) {
        if (diagnostic.file !== undefined) {
            byPath.get(diagnostic.file.resolvedPath)?.push(diagnostic);
        }
    }
    for (const [candidate, diagnostics] of byPath) {
        if (!state.freshSemanticDiagnostics.has(candidate)) {
            state.freshSemanticDiagnostics.set(candidate, diagnostics);
        }
    }
    return state.freshSemanticDiagnostics.get(path);
}

// TS 6.0: src/compiler/builder.ts:2218 addToAffectedFilesPendingEmit
function addToAffectedFilesPendingEmit(state, affectedFilePendingEmit, kind) {
    const existingKind = state.affectedFilesPendingEmit?.get(affectedFilePendingEmit) || BuilderFileEmit.None;
    (state.affectedFilesPendingEmit ??= new Map()).set(affectedFilePendingEmit, existingKind | kind);
    state.emitDiagnosticsPerFile?.delete(affectedFilePendingEmit);
}

// TS 6.0: src/compiler/builderState.ts:375 getFilesAffectedByWithOldState
function getFilesAffectedByWithOldState(ts, state, path, cancellationToken, host) {
    if (!state.fileInfos.has(path)) {
        return ts.emptyArray;
    }
    const isDefaultLibrary = state.defaultLibraryPaths.has(path);
    const sourceFile = isDefaultLibrary ? undefined : state.program.getSourceFileByPath(path);
    if (!isDefaultLibrary && !sourceFile) {
        return ts.emptyArray;
    }
    if (!updateShapeSignature(ts, state, path, sourceFile, cancellationToken, host)) {
        return [sourceFile ?? state.program.getSourceFileByPath(path)];
    }
    if (!state.referencedMap) {
        // TS 6.0: src/compiler/builderState.ts:575 getFilesAffectedByUpdatedShapeWhenNonModuleEmit
        return state.compilerOptions.outFile
            ? [sourceFile ?? state.program.getSourceFileByPath(path)]
            : getAllFilesExcludingDefaultLibraryFile(ts, state, sourceFile);
    }
    return getFilesAffectedByUpdatedShapeWhenModuleEmit(ts, state, path, sourceFile, cancellationToken, host);
}

// TS 6.0: src/compiler/builderState.ts:588 getFilesAffectedByUpdatedShapeWhenModuleEmit
function getFilesAffectedByUpdatedShapeWhenModuleEmit(ts, state, path, sourceFile, cancellationToken, host) {
    if (state.fileInfos.get(path).affectsGlobalScope) {
        return getAllFilesExcludingDefaultLibraryFile(ts, state, sourceFile);
    }
    const file = sourceFile ?? state.program.getSourceFileByPath(path);
    if (ts.getIsolatedModules(state.compilerOptions) || state.compilerOptions.outFile) {
        return [file];
    }
    const seenFileNamesMap = new Map([[path, file]]);
    const queue = getReferencedByPaths(state, path);
    while (queue.length > 0) {
        const currentPath = queue.pop();
        if (!seenFileNamesMap.has(currentPath)) {
            const currentSourceFile = state.defaultLibraryPaths.has(currentPath) ? undefined : state.program.getSourceFileByPath(currentPath);
            seenFileNamesMap.set(currentPath, currentSourceFile);
            if (currentSourceFile && updateShapeSignature(ts, state, currentPath, currentSourceFile, cancellationToken, host)) {
                queue.push(...getReferencedByPaths(state, currentPath));
            }
        }
    }
    return [...seenFileNamesMap.values()].filter(file => file !== undefined);
}

// TS 6.0: src/compiler/builderState.ts:431 updateShapeSignature. `sourceFile` is undefined for a
// default library file.
function updateShapeSignature(ts, state, path, sourceFile, cancellationToken, host, useFileVersionAsSignature = state.useFileVersionAsSignature) {
    if (state.hasCalledUpdateShapeSignature?.has(path)) {
        return false;
    }
    const info = state.fileInfos.get(path);
    const prevSignature = info.signature;
    let latestSignature;
    if (sourceFile !== undefined && !sourceFile.isDeclarationFile && !useFileVersionAsSignature) {
        latestSignature = computeDtsSignature(ts, state.program, sourceFile, cancellationToken, host);
        if (latestSignature !== undefined && host.storeSignatureInfo) {
            (state.signatureInfo ??= new Map()).set(path, ts.SignatureInfo.ComputedDts);
        }
    }
    if (latestSignature === undefined) {
        latestSignature = info.version;
        if (host.storeSignatureInfo) {
            (state.signatureInfo ??= new Map()).set(path, ts.SignatureInfo.UsedVersion);
        }
    }
    (state.oldSignatures ??= new Map()).set(path, prevSignature || false);
    (state.hasCalledUpdateShapeSignature ??= new Set()).add(path);
    info.signature = latestSignature;
    return latestSignature !== prevSignature;
}

// TS 6.0: src/compiler/builderState.ts:399 computeDtsSignature: the hash of the file's declaration
// output (tsc-rs declaration emit, forced like TS 6.0's EmitOnly.BuilderSignature) with its errors.
function computeDtsSignature(ts, program, sourceFile, cancellationToken, host) {
    let text;
    const result = program.emit(
        sourceFile,
        (fileName, outputText) => {
            if (ts.isDeclarationFileName(fileName)) {
                text = outputText;
            }
        },
        cancellationToken,
        ts.EmitOnly.BuilderSignature,
        /*customTransformers*/ undefined,
        /*forceDtsEmit*/ true,
    );
    if (text === undefined) {
        return undefined;
    }
    const diagnostics = result.diagnostics.filter(diagnostic => diagnostic.file !== undefined);
    return ts.computeSignatureWithDiagnostics(program, sourceFile, text, host, diagnostics.length ? { diagnostics } : undefined);
}

// TS 6.0: src/compiler/builderState.ts:465 getAllDependencies
function getAllDependencies(ts, state, sourceFile) {
    const program = ts.Debug.checkDefined(state.program);
    if (state.compilerOptions.outFile || !state.referencedMap || state.fileInfos.get(sourceFile.resolvedPath)?.affectsGlobalScope) {
        return getAllFileNames(state);
    }
    const seenMap = new Set();
    const queue = [sourceFile.resolvedPath];
    while (queue.length) {
        const path = queue.pop();
        if (!seenMap.has(path)) {
            seenMap.add(path);
            state.referencedMap.getValues(path)?.forEach(reference => queue.push(reference));
        }
    }
    return [...seenMap].map(path => {
        const nativeFileName = state.nativeFileNames.get(path);
        return nativeFileName === undefined ? path : program.toJsFileName(nativeFileName);
    });
}

function getAllFileNames(state) {
    state.allFileNames ??= state.program.nativeFileNames.map(name => state.program.toJsFileName(name));
    return state.allFileNames;
}

// TS 6.0: src/compiler/builderState.ts getReferencedByPaths
function getReferencedByPaths(state, referencedFilePath) {
    const keys = state.referencedMap.getKeys(referencedFilePath);
    return keys ? [...keys] : [];
}

// TS 6.0: src/compiler/builderState.ts:549 getAllFilesExcludingDefaultLibraryFile
function getAllFilesExcludingDefaultLibraryFile(ts, state, firstSourceFile) {
    if (state.allFilesExcludingDefaultLibraryFile) {
        return state.allFilesExcludingDefaultLibraryFile;
    }
    const result = [];
    if (firstSourceFile && !state.defaultLibraryPaths.has(firstSourceFile.resolvedPath)) {
        result.push(firstSourceFile);
    }
    for (const path of state.fileInfos.keys()) {
        if (state.defaultLibraryPaths.has(path)) {
            continue;
        }
        const sourceFile = state.program.getSourceFileByPath(path);
        if (sourceFile && sourceFile !== firstSourceFile) {
            result.push(sourceFile);
        }
    }
    state.allFilesExcludingDefaultLibraryFile = result.length ? result : ts.emptyArray;
    return state.allFilesExcludingDefaultLibraryFile;
}

// TS 6.0: src/compiler/utilities.ts sourceFileMayBeEmitted. Sources of referenced projects never
// reach a tsc-rs program (it reads their declaration outputs).
function mayBeEmitted(ts, state, sourceFile) {
    const { program } = state;
    return ts.sourceFileMayBeEmitted(sourceFile, {
        getCompilerOptions: () => state.compilerOptions,
        isSourceFileFromExternalLibrary: file => isExternalLibraryFile(state, file),
        isSourceOfProjectReferenceRedirect: () => false,
        getRedirectFromSourceFile: () => undefined,
        getCurrentDirectory: () => program.getCurrentDirectory(),
        getCanonicalFileName: program.getCanonicalFileName,
        useCaseSensitiveFileNames: () => program.useCaseSensitiveFileNames(),
    });
}

// tsc-rs marks files that it reached through node_modules package lookups.
function isExternalLibraryFile(state, file) {
    const path = file.resolvedPath;
    if (!path.includes("/node_modules/")) {
        return false;
    }
    let isExternal = state.externalLibraryFiles.get(path);
    if (isExternal === undefined) {
        isExternal = state.program.isSourceFileFromExternalLibrary(file);
        state.externalLibraryFiles.set(path, isExternal);
    }
    return isExternal;
}

function isDeclarationOutput(ts, fileName) {
    return ts.isDeclarationFileName(fileName.endsWith(".map") ? fileName.slice(0, -".map".length) : fileName);
}

function install(ts, hooks, originals) {
    hooks.createBuilderProgram = (kind, parameters) => (parameters.newProgram instanceof NativeProgram
        ? createBuilderProgram(ts, kind, parameters)
        : originals.createBuilderProgram(kind, parameters));
    // TS 6.0: src/compiler/watchPublic.ts:106 readBuilderProgram. A TS 6.0 .tsbuildinfo cannot seed a
    // tsc-rs program, and none written by tsc-rs exists.
    hooks.readBuilderProgram = () => undefined;
}

module.exports = { createBuilderProgram, install };
