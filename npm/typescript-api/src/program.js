"use strict";

const { getSession } = require("./session.js");
const { NativeSourceFile, parseHandle } = require("./nodes.js");
const { bundledLibText, fileSystemFor, HostModuleResolver, isDefaultHost, usesHostModuleResolution } = require("./host.js");
const { toNativeOptions, toTsOptionDiagnostics } = require("./options.js");
const { toTsDiagnostics } = require("./diagnostics.js");
const { syntaxKindToNative, syntaxKindToTs6 } = require("./flags.js");

const bundledLibs = "bundled:///libs/";
const maxCachedPaths = 65536;
// Past this many, the paths tsc-rs found missing are forgotten and it reads everything again.
const maxMissingProbes = 131072;

// WeakRefs of the programs that have each SourceFile, newest last. A language service or a
// ts.server project shares unchanged SourceFiles between its programs, as TS 6.0 does, and a host
// may keep SourceFiles (a cache of lib files) for many fresh programs: the lists must not keep
// those programs (and their tsc-rs snapshots) alive. Joining a list compares refs without
// dereferencing them: a deref keeps an unreachable program alive to the end of the job.
const programsOfSourceFiles = new WeakMap();
const maxProgramRefs = 64;

function programOfSourceFile(sourceFile) {
    const refs = programsOfSourceFiles.get(sourceFile) ?? [];
    for (let index = refs.length - 1; index >= 0; index--) {
        const program = refs[index].deref();
        if (program !== undefined && !program.released) {
            return program;
        }
    }
    return undefined;
}

// Native state a program holds: its snapshot and, with a custom module resolver, the resolver.
// A failed release after garbage collection has nobody to report to (and must not end the process).
const releaseNative = new FinalizationRegistry(native => {
    try {
        releaseNativeState(native);
    }
    catch {
        // The session is closed or the snapshot is gone already.
    }
});

function releaseNativeState(native) {
    if (native.released) {
        return;
    }
    native.released = true;
    const { session, snapshot, resolver, externalMemory, lineage } = native;
    session.adjustExternalMemory(-externalMemory);
    if (--lineage.programs === 0) {
        session.adjustExternalMemory(-lineage.externalMemory);
    }
    if (resolver !== undefined) {
        session.unregisterCallback(resolver.callbackName);
    }
    session.request("release", { snapshot });
    if (resolver !== undefined) {
        session.request("releaseModuleResolver", { resolver: resolver.id });
    }
}

// V8 sees only the JS side of a program; it is told roughly what the program's snapshot holds in
// tsc-rs, so that it collects programs (and their snapshots) often enough, and not more often. A
// snapshot updated from another one shares nearly all of its state with it: the programs updated
// from one created snapshot (a lineage) report its size once, while any of them is alive, and
// each adds what it holds alone.
const kib = 1024;

// A created snapshot: a checked program of one file and the default lib takes about 27 MB.
function snapshotSize(fileCount) {
    return 16 * 1024 * kib + fileCount * 128 * kib;
}

// An updated snapshot: its program's tables, its checkers and the files read again. Measured in
// ts.server.ProjectService loops with one changed file per update: about 0.8 MB with 90 files,
// 1 MB with 210.
function updateSize(fileCount, changedCount) {
    return 512 * kib + fileCount * 3 * kib + changedCount * 256 * kib;
}

// TS 6.0: src/compiler/program.ts:1499 createProgram
function createProgram(ts, rootNamesOrOptions, options, host, oldProgram, configFileParsingDiagnostics) {
    const createProgramOptions = ts.isArray(rootNamesOrOptions)
        ? { rootNames: rootNamesOrOptions, options, host, oldProgram, configFileParsingDiagnostics }
        : rootNamesOrOptions;
    // TS 6.0 rejects options read from tsconfig JSON without parsing (`"module": "nodenext"`).
    for (const option of ts.commandLineOptionOfCustomType) {
        if (ts.hasProperty(createProgramOptions.options, option.name) && typeof createProgramOptions.options[option.name] === "string") {
            throw new Error(`${option.name} is a string value; tsconfig JSON must be parsed with parseJsonSourceFileConfigFileContent or getParsedCommandLineOfConfigFile before passing to createProgram`);
        }
    }
    return new NativeProgram(ts, createProgramOptions);
}

// A TS 6.0 Program whose files, checker and emit come from one tsc-rs snapshot. Source files are
// TS 6.0 SourceFile objects: the host's, when it gives them, or parsed in JS from the text that
// tsc-rs compiled.
//
// A program made from an oldProgram of this session updates the old snapshot with the files whose
// text changed instead of reading every file again. A program releases its snapshot when it is
// garbage collected; it tells V8 about the snapshot's memory, so that collections come often enough.
// A program whose construction throws releases what it made at once.
class NativeProgram {
    constructor(ts, { rootNames, options, projectReferences, host, oldProgram, configFileParsingDiagnostics, typeScriptVersion }) {
        // TS 6.0's program members are closures, and TS 6.0 passes some around unbound
        // (src/compiler/utilities.ts setSymlinksFromResolutions(program.forEachResolvedModule, ...)).
        for (const name of programMethods) {
            this[name] = this[name].bind(this);
        }
        this.ts = ts;
        this.selfRef = new WeakRef(this);
        this.rootNames = rootNames;
        this.options = options;
        this.projectReferences = projectReferences;
        this.host = host ?? ts.createCompilerHost(options);
        this.configFileParsingDiagnostics = configFileParsingDiagnostics;
        this.typeScriptVersion = typeScriptVersion;
        this.currentDirectory = this.host.getCurrentDirectory();
        this.nativeDirectory = ts.getNormalizedAbsolutePath(this.currentDirectory, ts.sys.getCurrentDirectory());
        this.relativePrefix = ts.isRootedDiskPath(this.currentDirectory) ? undefined : ts.ensureTrailingDirectorySeparator(this.nativeDirectory);
        this.getCanonicalFileName = ts.createGetCanonicalFileName(this.host.useCaseSensitiveFileNames());
        this.libLocation = ts.normalizePath(this.host.getDefaultLibLocation?.() ?? ts.getDirectoryPath(this.host.getDefaultLibFileName(options)));
        this.session = getSession(ts, this.host.useCaseSensitiveFileNames(), this.nativeDirectory);
        const previous = oldProgram instanceof NativeProgram && oldProgram.session === this.session ? oldProgram : undefined;
        // File name -> path; a program of the same session and directory computes the same paths.
        this.paths = previous?.currentDirectory === this.currentDirectory ? previous.paths : new Map();
        this.usesDefaultHost = isDefaultHost(this.host);
        this.fileSystem = fileSystemFor(ts, this);
        this.released = false;
        this.constructed = false;
        this.structureIsReused = ts.StructureIsReused.Not;
        this.rootNativeToJs = new Map();
        this.hostSourceFiles = new Map();
        this.hostTexts = new Map();
        this.structureKeys = new Map();
        this.foreignTexts = new Set();
        this.sourceFileOptions = new Map();
        this.nativeFiles = new Map();
        this.sourceFiles = new Map();
        this.moduleResolutions = new Map();
        this.typeReferenceResolutions = new Map();
        this.metadata = new Map();
        this.diagnosticsCache = new Map();
        // tsc-rs's diagnostic responses, method -> file -> responses; shared with the programs whose
        // snapshots compile the same thing (see reuseFrom).
        this.nativeDiagnostics = new Map();
        this.checker = undefined;
        this.moduleResolutionCache = undefined;
        // Native path -> whether it is a directory, for what tsc-rs found missing through the host
        // (see createdFiles); shared with the programs updated from this one.
        this.missingProbes = new Map();

        const nativeOptions = toNativeOptions(ts, options, { currentDirectory: this.nativeDirectory, typeScriptVersion });
        // TS 6.0: src/compiler/program.ts useSourceOfProjectReferenceRedirect. A program compiles the
        // declaration outputs of referenced projects unless its host asks for their sources (a language
        // service does); tsc-rs's API compiles the sources unless told otherwise.
        this.useSourceOfProjectReferenceRedirect = this.host.useSourceOfProjectReferenceRedirect?.() === true && !options.disableSourceOfProjectReferenceRedirect;
        nativeOptions.options.disableSourceOfProjectReferenceRedirect = !this.useSourceOfProjectReferenceRedirect;
        this.nativeOptions = nativeOptions.options;
        this.optionDiagnostics = nativeOptions.diagnostics;
        this.nativeRootFiles = rootNames.map(name => {
            const nativeName = ts.getNormalizedAbsolutePath(name, this.nativeDirectory);
            this.rootNativeToJs.set(nativeName, ts.normalizePath(name));
            return nativeName;
        });
        this.checkDefaultLib();
        // TS 6.0: src/compiler/program.ts shouldProgramCreateNewSourceFiles
        this.shouldCreateNewSourceFile = previous !== undefined && ts.optionsHaveChanges(previous.options, options, ts.sourceFileAffectingCompilerOptions);
        const base = previous !== undefined && !previous.released && previous.usesDefaultHost === this.usesDefaultHost ? previous : undefined;
        this.hostResolver = this.fileSystem !== undefined && usesHostModuleResolution(this.host) ? new HostModuleResolver(this) : undefined;
        try {
            this.build(previous, base);
        }
        catch (error) {
            this.abandon();
            throw error;
        }
        this.constructed = true;
        this.joinSourceFiles();
        if (oldProgram instanceof NativeProgram) {
            oldProgram.leaveSourceFiles();
        }
    }

    build(previous, base) {
        // TS 6.0 asks the host for referenced project configs while it builds the program.
        this.getResolvedProjectReferences();
        const changes = base === undefined ? undefined : this.changesFrom(base);
        if (changes !== undefined && !changes.invalidateAll) {
            this.missingProbes = base.missingProbes;
        }
        const response = this.withFileSystem(() => base === undefined ? this.createSnapshot() : this.updateSnapshot(base, changes));
        this.snapshot = response.snapshot;
        this.project = base === undefined ? response.operation.createdPrograms[0] : base.project;
        const lineage = base?.native.lineage ?? { programs: 0, externalMemory: 0 };
        lineage.programs++;
        this.native = { session: this.session, snapshot: this.snapshot, resolver: this.hostResolver?.nativeHandle(), externalMemory: 0, lineage };
        releaseNative.register(this, this.native, this.native);
        const sameFiles = base !== undefined && this.keepsFilesOf(base, changes);
        this.nativeFileNames = sameFiles ? base.nativeFileNames : this.request("getSourceFileNames");
        this.reportExternalMemory(base, changes);
        if (sameFiles) {
            this.shareFileIndex(base);
            this.missingFilePaths = base.missingFilePaths;
        }
        else {
            this.indexFileNames();
        }
        this.nameReferencedFiles();
        if (this.fileSystem !== undefined) {
            this.adoptHostLibFiles(base);
            this.hostResolver?.finish(this.nativeFileNames);
        }
        if (base !== undefined) {
            this.reuseFrom(base, changes);
        }
        if (previous !== undefined) {
            this.releaseOldSourceFiles(previous);
        }
    }

    // What a program whose construction failed made in tsc-rs is released, and its SourceFiles do not
    // name it (programOfSourceFile).
    abandon() {
        this.released = true;
        if (this.hostResolver !== undefined) {
            this.hostResolver.target.resolver = undefined;
        }
        if (this.native !== undefined) {
            releaseNative.unregister(this.native);
            attempt(() => releaseNativeState(this.native));
            return;
        }
        if (this.hostResolver !== undefined) {
            attempt(() => this.hostResolver.release());
        }
        if (this.snapshot !== undefined) {
            attempt(() => this.session.request("release", { snapshot: this.snapshot }));
        }
    }

    // The program's SourceFiles name it (programOfSourceFile) once it is built.
    joinSourceFiles() {
        for (const [nativeFileName, sourceFile] of this.hostSourceFiles) {
            if (sourceFile !== undefined && this.nativeFileNameSet.has(nativeFileName)) {
                this.joinSourceFile(sourceFile);
            }
        }
        for (const sourceFile of this.sourceFiles.values()) {
            this.joinSourceFile(sourceFile);
        }
    }

    joinSourceFile(sourceFile) {
        let refs = programsOfSourceFiles.get(sourceFile);
        if (refs === undefined) {
            refs = [];
            programsOfSourceFiles.set(sourceFile, refs);
        }
        if (refs.at(-1) === this.selfRef) {
            return;
        }
        removeItem(refs, this.selfRef);
        if (refs.length >= maxProgramRefs) {
            refs.splice(0, refs.length - maxProgramRefs / 2);
        }
        refs.push(this.selfRef);
    }

    leaveSourceFiles() {
        const leave = sourceFile => {
            const refs = sourceFile === undefined ? undefined : programsOfSourceFiles.get(sourceFile);
            if (refs !== undefined) {
                removeItem(refs, this.selfRef);
            }
        };
        this.hostSourceFiles.forEach(leave);
        this.sourceFiles.forEach(leave);
    }

    reportExternalMemory(base, changes) {
        const { lineage } = this.native;
        const fileCount = this.nativeFileNames.length;
        const lineageSize = snapshotSize(fileCount);
        if (lineageSize > lineage.externalMemory) {
            this.session.adjustExternalMemory(lineageSize - lineage.externalMemory);
            lineage.externalMemory = lineageSize;
        }
        if (base === undefined) {
            return;
        }
        // A program that reads the disk does not know which files tsc-rs read again.
        const changedCount = changes.invalidateAll ? Math.ceil(fileCount / 16) : changes.changed.length;
        this.native.externalMemory = updateSize(fileCount, changedCount);
        this.session.adjustExternalMemory(this.native.externalMemory);
    }

    // TS 6.0: src/compiler/program.ts createProgram processes host.getDefaultLibFileName(options) as
    // the default library. A host that names none, or cannot supply it, gives a program without
    // one (and a missing file); tsc-rs would compile its bundled libs instead.
    checkDefaultLib() {
        const { ts, options, host } = this;
        if (this.rootNames.length === 0 || options.noLib || options.lib) {
            return;
        }
        const fileName = host.getDefaultLibFileName(options);
        if (fileName && (this.fileSystem === undefined || host.fileExists(fileName) || this.hostDefaultLib(fileName) !== undefined)) {
            return;
        }
        this.nativeOptions = { ...this.nativeOptions, noLib: true };
        if (fileName) {
            this.missingDefaultLib = fileName;
            const reason = ts.fileIncludeReasonToDiagnostics(this, { kind: ts.FileIncludeKind.LibFile });
            const details = ts.chainDiagnosticMessages([reason], ts.Diagnostics.The_file_is_in_the_program_because_Colon);
            this.optionDiagnostics = [...this.optionDiagnostics, ts.createCompilerDiagnosticFromMessageChain(ts.chainDiagnosticMessages(details, ts.Diagnostics.File_0_not_found, fileName))];
        }
    }

    // The host's SourceFile for its default lib, kept for adoptHostLibFiles when it is a lib file.
    hostDefaultLib(fileName) {
        const { ts } = this;
        const sourceFileOptions = this.createSourceFileOptions(fileName);
        const sourceFile = this.getHostSourceFile(fileName, sourceFileOptions);
        if (sourceFile !== undefined && ts.getDirectoryPath(ts.normalizePath(fileName)) === this.libLocation) {
            this.hostLibFiles = new Map([[bundledLibs + ts.getBaseFileName(fileName), { sourceFile, sourceFileOptions }]]);
        }
        return sourceFile;
    }

    programParams() {
        const { ts } = this;
        return {
            rootFiles: this.nativeRootFiles,
            compilerOptions: this.nativeOptions,
            options: {
                projectReferences: this.projectReferences?.map(reference => ({
                    path: ts.getNormalizedAbsolutePath(reference.path, this.nativeDirectory),
                    originalPath: reference.originalPath,
                    circular: reference.circular,
                })),
                moduleResolver: this.hostResolver?.id,
            },
        };
    }

    createSnapshot() {
        return this.session.request("createSnapshot", {
            fileNotifications: { invalidateAll: true },
            createPrograms: [this.programParams()],
        });
    }

    updateSnapshot(base, changes) {
        return this.session.request("updateSnapshot", {
            snapshot: base.snapshot,
            changes: {
                fileNotifications: changes.invalidateAll ? { invalidateAll: true } : fileChanges(changes),
                reconfigurePrograms: [{ id: base.project, ...this.programParams() }],
            },
        });
    }

    // TS 6.0: src/compiler/program.ts tryReuseStructureFromOldProgram asks the host for every file of
    // the old program; the files whose text changed are the ones tsc-rs reads again. Programs that
    // read the disk do not know what changed, so tsc-rs reads every file again.
    changesFrom(base) {
        const changes = { invalidateAll: this.fileSystem === undefined, changed: [], created: [], unchanged: [], fileRemoved: false, resolutionsInvalidated: false, formatChanged: false };
        if (changes.invalidateAll) {
            return changes;
        }
        const hasInvalidatedResolutions = this.host.hasInvalidatedResolutions;
        // TS 6.0 calls a file modified when the host gives another SourceFile for it.
        const modified = [];
        for (const nativeFileName of base.nativeFileNames) {
            if (nativeFileName.startsWith(bundledLibs)) {
                continue;
            }
            const sourceFile = this.adoptHostSourceFile(nativeFileName);
            const baseSourceFile = base.hostSourceFiles.get(nativeFileName);
            if (sourceFile === undefined) {
                changes.fileRemoved = true;
                continue;
            }
            if (sourceFile !== baseSourceFile) {
                modified.push(nativeFileName);
            }
            // A module format that changed (a package.json "type"): tsc-rs reads package.json files
            // again only when it reads everything again.
            if (baseSourceFile !== undefined && sourceFile.impliedNodeFormat !== baseSourceFile.impliedNodeFormat) {
                changes.formatChanged = true;
            }
            if (sourceFile.text !== base.hostTexts.get(nativeFileName)) {
                changes.changed.push(nativeFileName);
            }
            else if (hasInvalidatedResolutions?.(sourceFile.path)) {
                changes.resolutionsInvalidated = true;
            }
            else {
                changes.unchanged.push(nativeFileName);
            }
        }
        // tsc-rs remembers what it found missing; a file that appeared needs everything read again.
        const missingFileAppeared = this.ts.forEachEntry(base.getMissingFilePaths(), fileName => !!this.host.fileExists(fileName)) ?? false;
        changes.invalidateAll = changes.fileRemoved || changes.resolutionsInvalidated || missingFileAppeared || changes.formatChanged || base.missingProbes.size > maxMissingProbes;
        changes.missingFileAppeared = missingFileAppeared;
        if (!changes.invalidateAll) {
            changes.created = this.createdFiles(base, changes, modified);
        }
        // TS 6.0 keeps the module resolutions of the files that did not change.
        if (!changes.invalidateAll && changes.created.length === 0 && !this.ts.changesAffectModuleResolution(base.options, this.options)) {
            for (const nativeFileName of changes.unchanged) {
                const resolutions = base.moduleResolutions.get(nativeFileName);
                if (resolutions !== undefined) {
                    this.moduleResolutions.set(nativeFileName, resolutions);
                }
            }
        }
        return changes;
    }

    // TS 6.0: src/compiler/program.ts resolveNamesReusingOldState resolves again the module names that
    // did not resolve, in the files that changed (in every file when the program's structure
    // changed), so that it finds a file created since. tsc-rs remembers the files and directories it
    // found missing; it is told which of those exist now.
    createdFiles(base, changes, modified) {
        if (base.missingProbes.size === 0 || !this.resolvesAgain(base, changes, modified)) {
            return [];
        }
        const { host } = this;
        const created = [];
        for (const [path, isDirectory] of base.missingProbes) {
            const fileName = this.jsNameOf(path);
            if (isDirectory ? host.directoryExists?.(fileName) : host.fileExists(fileName)) {
                created.push(path);
                base.missingProbes.delete(path);
            }
        }
        return created;
    }

    resolvesAgain(base, changes, modified) {
        if (!this.sameInputsAs(base) || changes.changed.some(name => this.structureKeys.get(name) !== base.structureKeys.get(name))) {
            return true;
        }
        const unknown = modified.filter(nativeFileName => !base.moduleResolutions.has(nativeFileName));
        if (unknown.length !== 0) {
            base.resolveModulesOf(unknown);
        }
        return modified.some(nativeFileName => base.hasUnresolvedModuleNames(nativeFileName));
    }

    // Whether a module name of the file resolved neither to a file nor to an ambient module.
    hasUnresolvedModuleNames(nativeFileName) {
        let unresolved = false;
        this.moduleResolutionsOf(nativeFileName).forEach((resolution, moduleName) => {
            unresolved ||= resolution?.resolvedModule === undefined && !this.isAmbientModuleName(moduleName);
        });
        return unresolved;
    }

    // TS 6.0: src/compiler/checker.ts tryFindAmbientModule, over the `declare module` names of the
    // program's files (a pattern has one "*").
    isAmbientModuleName(moduleName) {
        if (this.ambientModules === undefined) {
            const names = new Set();
            const patterns = [];
            for (const sourceFile of this.hostSourceFiles.values()) {
                for (const name of sourceFile?.ambientModuleNames ?? []) {
                    const parts = name.split("*");
                    if (parts.length === 1) {
                        names.add(name);
                    }
                    else if (parts.length === 2) {
                        patterns.push(parts);
                    }
                }
            }
            this.ambientModules = { names, patterns };
        }
        const { names, patterns } = this.ambientModules;
        return names.has(moduleName)
            || patterns.some(([prefix, suffix]) => moduleName.length >= prefix.length + suffix.length && moduleName.startsWith(prefix) && moduleName.endsWith(suffix));
    }

    // Whether the program compiles the same roots with the same options as `base`.
    sameInputsAs(base) {
        const { ts } = this;
        return this.currentDirectory === base.currentDirectory
            && this.libLocation === base.libLocation
            && ts.arrayIsEqualTo(base.rootNames, this.rootNames)
            && ts.arrayIsEqualTo(base.projectReferences, this.projectReferences, ts.projectReferenceIsEqualTo)
            && JSON.stringify(base.nativeOptions) === JSON.stringify(this.nativeOptions);
    }

    noteMissing(nativeFileName, isDirectory) {
        this.missingProbes.set(nativeFileName, isDirectory);
    }

    // TS 6.0: src/compiler/program.ts createProgram releases the old program's files that this one
    // does not have (a language service's document registry counts them).
    releaseOldSourceFiles(oldProgram) {
        const { host } = this;
        if (host.onReleaseOldSourceFile === undefined || this.fileSystem === undefined) {
            return;
        }
        for (const oldSourceFile of oldProgram.hostSourceFiles.values()) {
            if (oldSourceFile === undefined || oldProgram.getSourceFileByPath(oldSourceFile.resolvedPath) !== oldSourceFile) {
                continue;
            }
            const newFile = this.getSourceFileByPath(oldSourceFile.resolvedPath);
            if (this.shouldCreateNewSourceFile || newFile === undefined || newFile.impliedNodeFormat !== oldSourceFile.impliedNodeFormat) {
                host.onReleaseOldSourceFile(oldSourceFile, oldProgram.options, this.getSourceFileByPath(oldSourceFile.path) !== undefined, newFile);
            }
        }
        if (host.getParsedCommandLine === undefined) {
            oldProgram.forEachResolvedProjectReference(reference => {
                if (this.getResolvedProjectReferenceByPath(reference.sourceFile.path) === undefined) {
                    host.onReleaseOldSourceFile(reference.sourceFile, oldProgram.options, false, undefined);
                }
            });
        }
    }

    // Carries over what the old program knew about files that did not change.
    reuseFrom(base, changes) {
        this.structureIsReused = this.structureReuse(base, changes);
        const libs = this.nativeFileNames.filter(name => name.startsWith(bundledLibs));
        // tsc-rs keeps the syntax trees of unchanged files, so their node handles stay the same.
        // The bundled lib files never change, also when tsc-rs reads every other file again.
        this.reuseNativeFiles(base, changes.invalidateAll ? libs : [...changes.unchanged, ...libs], !changes.invalidateAll);
        // Nothing to compile again: the new snapshot answers diagnostics as the old one does (a
        // global diagnostics request checks every file).
        if (!changes.invalidateAll && this.compilesSameAs(base, changes)) {
            this.nativeDiagnostics = base.nativeDiagnostics;
        }
    }

    reuseNativeFiles(base, nativeFileNames, withMetadata) {
        for (const nativeFileName of nativeFileNames) {
            if (!this.nativeFileNameSet.has(nativeFileName)) {
                continue;
            }
            if (withMetadata && base.metadata.has(nativeFileName)) {
                this.metadata.set(nativeFileName, base.metadata.get(nativeFileName));
            }
            if (base.nativeFiles.has(nativeFileName)) {
                this.nativeFiles.set(nativeFileName, base.nativeFiles.get(nativeFileName));
            }
        }
    }

    compilesSameAs(base, changes) {
        // Each program has its own module resolver; unchanged files keep their resolutions.
        const compiled = program => JSON.stringify({ ...program.programParams(), options: { ...program.programParams().options, moduleResolver: undefined } });
        return changes.changed.length === 0
            && changes.created.length === 0
            && this.structureIsReused === this.ts.StructureIsReused.Completely
            && this.ts.arrayIsEqualTo(base.nativeFileNames, this.nativeFileNames)
            && compiled(base) === compiled(this);
    }

    // TS 6.0: src/compiler/program.ts tryReuseStructureFromOldProgram
    structureReuse(base, changes) {
        const { ts } = this;
        const { StructureIsReused } = ts;
        const sameFiles = ts.arrayIsEqualTo(base.nativeFileNames, this.nativeFileNames);
        if (
            ts.changesAffectModuleResolution(base.options, this.options)
            || !ts.arrayIsEqualTo(base.rootNames, this.rootNames)
            || !ts.arrayIsEqualTo(base.projectReferences, this.projectReferences, ts.projectReferenceIsEqualTo)
            || changes.fileRemoved
            || changes.missingFileAppeared
        ) {
            return StructureIsReused.Not;
        }
        // A program that reads the disk does not see which files changed.
        if (this.fileSystem === undefined) {
            return sameFiles ? StructureIsReused.SafeModules : StructureIsReused.Not;
        }
        if (
            changes.resolutionsInvalidated
            || changes.formatChanged
            || !sameFiles
            || changes.changed.some(name => this.structureKeys.get(name) !== base.structureKeys.get(name))
            || ts.changesAffectingProgramStructure(base.options, this.options)
            || this.host.hasChangedAutomaticTypeDirectiveNames?.()
        ) {
            return StructureIsReused.SafeModules;
        }
        return StructureIsReused.Completely;
    }

    // TS 6.0: src/compiler/program.ts tryReuseStructureFromOldProgram keeps the old program's files
    // when no file changed its imports or references; tsc-rs's program then has the old files too.
    // A host's module resolver may answer differently each time, so its programs ask tsc-rs.
    keepsFilesOf(base, changes) {
        return !changes.invalidateAll
            && changes.created.length === 0
            && this.hostResolver === undefined
            && this.sameInputsAs(base)
            && changes.changed.every(name => this.structureKeys.get(name) === base.structureKeys.get(name))
            && !this.host.hasChangedAutomaticTypeDirectiveNames?.();
    }

    // The indexes of indexFileNames, which keepsFilesOf makes the same as the base program's.
    shareFileIndex(base) {
        this.nativeFileNameSet = base.nativeFileNameSet;
        this.pathToNativeFileName = base.pathToNativeFileName;
        this.nativePathToNativeFileName = base.nativePathToNativeFileName;
    }

    indexFileNames() {
        this.nativeFileNameSet = new Set(this.nativeFileNames);
        this.pathToNativeFileName = new Map();
        this.nativePathToNativeFileName = new Map();
        for (const name of this.nativeFileNames) {
            this.pathToNativeFileName.set(this.toPath(this.toJsFileName(name)), name);
            this.nativePathToNativeFileName.set(this.nativePathOf(name), name);
        }
    }

    // The path key tsc-rs puts in node handles.
    nativePathOf(nativeFileName) {
        return nativeFileName.startsWith(bundledLibs) ? nativeFileName : this.ts.toPath(nativeFileName, this.nativeDirectory, this.getCanonicalFileName);
    }

    // Native requests on a program whose snapshot was released (see the class comment) fail.
    request(method, params) {
        this.assertNative(method);
        return this.withFileSystem(() => this.session.request(method, { snapshot: this.snapshot, project: this.project, ...params }));
    }

    requestBinary(method, params) {
        this.assertNative(method);
        return this.withFileSystem(() => this.session.requestBinary(method, { snapshot: this.snapshot, project: this.project, ...params }));
    }

    requestBatch(method, paramsList) {
        this.assertNative(method);
        const requests = paramsList.map(params => ({ method, params: { snapshot: this.snapshot, project: this.project, ...params } }));
        return this.withFileSystem(() => this.session.requestBatch(requests));
    }

    assertNative(method) {
        if (this.released) {
            throw new Error(`tsc-rs: ${method} is not supported on this program: its compiler state was released`);
        }
    }

    releaseNative() {
        if (this.released) {
            return;
        }
        this.released = true;
        this.leaveSourceFiles();
        releaseNative.unregister(this.native);
        releaseNativeState(this.native);
        this.nativeFiles.clear();
    }

    withFileSystem(run) {
        return this.session.withFileSystem(this.fileSystem, run);
    }

    toPath(fileName) {
        let path = this.paths.get(fileName);
        if (path === undefined) {
            path = this.ts.toPath(fileName, this.currentDirectory, this.getCanonicalFileName);
            if (this.paths.size >= maxCachedPaths) {
                this.paths.clear();
            }
            this.paths.set(fileName, path);
        }
        return path;
    }

    // The name of a program file: root files keep the names the caller passed.
    toJsFileName(nativeFileName) {
        return this.referenceNames?.get(nativeFileName) ?? this.rootNativeToJs.get(nativeFileName) ?? this.jsNameOf(nativeFileName);
    }

    // TS 6.0 names a file by the name it was first found under (src/compiler/program.ts
    // processRootFile, findSourceFileWorker): a `/// <reference path>` from a relatively named root
    // gives a relative name. tsc-rs's names are the absolute forms of the first names it found, so
    // a name whose absolute form is not tsc-rs's name was not the first one.
    nameReferencedFiles() {
        const { ts } = this;
        if (this.options.noResolve || this.rootNames.every(name => ts.isRootedDiskPath(name))) {
            return;
        }
        this.referenceNames = new Map();
        const visit = (nativeFileName, fileName) => {
            if (this.referenceNames.has(nativeFileName)) {
                return;
            }
            this.referenceNames.set(nativeFileName, fileName);
            const text = this.hostSourceFiles.get(nativeFileName)?.text ?? this.textOf(nativeFileName);
            if (ts.isRootedDiskPath(fileName) || !text.includes("reference")) {
                return;
            }
            const sourceFile = this.sourceFileOfNative(nativeFileName);
            this.rename(sourceFile, fileName);
            for (const reference of sourceFile.referencedFiles) {
                const target = ts.resolveTripleslashReference(reference.fileName, fileName);
                const candidate = this.referenceCandidates(target, true).find(name => this.pathToNativeFileName.has(this.toPath(name)));
                if (candidate !== undefined && this.pathToNativeFileName.get(this.toPath(candidate)) === ts.getNormalizedAbsolutePath(candidate, this.nativeDirectory)) {
                    visit(ts.getNormalizedAbsolutePath(candidate, this.nativeDirectory), candidate);
                }
            }
        };
        for (const nativeFileName of this.nativeRootFiles) {
            if (this.nativeFileNameSet.has(nativeFileName)) {
                visit(nativeFileName, this.rootNativeToJs.get(nativeFileName));
            }
        }
        for (const [nativeFileName, fileName] of this.referenceNames) {
            const sourceFile = this.hostSourceFiles.get(nativeFileName) ?? this.sourceFiles.get(nativeFileName);
            if (sourceFile !== undefined) {
                this.rename(sourceFile, fileName);
            }
        }
    }

    rename(sourceFile, fileName) {
        sourceFile.fileName = fileName;
        sourceFile.originalFileName = fileName;
    }

    // The name TS 6.0 computes for a file it finds (resolutions, the host's file system). A host
    // whose current directory is relative ("" in TS 6.0's own tests) gets names relative to it;
    // tsc-rs's are absolute.
    jsNameOf(nativeFileName) {
        if (nativeFileName.startsWith(bundledLibs)) {
            return this.ts.combinePaths(this.libLocation, nativeFileName.slice(bundledLibs.length));
        }
        if (this.relativePrefix === undefined || !nativeFileName.startsWith(this.relativePrefix)) {
            return nativeFileName;
        }
        return this.ts.combinePaths(this.currentDirectory, nativeFileName.slice(this.relativePrefix.length));
    }

    toNativeFileName(fileName) {
        return this.pathToNativeFileName?.get(this.toPath(fileName)) ?? this.ts.getNormalizedAbsolutePath(fileName, this.nativeDirectory);
    }

    nativeFileNameOf(sourceFile) {
        if (sourceFile.redirectInfo !== undefined) {
            return this.nativeFileNameOf(sourceFile.redirectInfo.redirectTarget);
        }
        const nativeFileName = this.pathToNativeFileName.get(sourceFile.path ?? this.toPath(sourceFile.fileName));
        if (nativeFileName === undefined) {
            throw new Error(`tsc-rs: ${sourceFile.fileName} is not a file of this program`);
        }
        return nativeFileName;
    }

    // The binary syntax tree of a program file, for node handles.
    nativeFile(nativeFileName) {
        let file = this.nativeFiles.get(nativeFileName);
        if (file === undefined) {
            const bytes = this.requestBinary("getSourceFile", { file: nativeFileName });
            if (bytes === undefined) {
                return undefined;
            }
            file = new NativeSourceFile(bytes);
            this.nativeFiles.set(nativeFileName, file);
        }
        return file;
    }

    // TS 6.0: src/compiler/program.ts getCreateSourceFileOptions
    createSourceFileOptions(fileName) {
        const { ts, options, host } = this;
        this.moduleResolutionCache ??= host.getModuleResolutionCache?.() ?? ts.createModuleResolutionCache(this.currentDirectory, this.getCanonicalFileName, options);
        const result = ts.getImpliedNodeFormatForFileWorker(ts.getNormalizedAbsolutePath(fileName, this.currentDirectory), this.moduleResolutionCache.getPackageJsonInfoCache(), host, options);
        const languageVersion = ts.getEmitScriptTarget(options);
        const setExternalModuleIndicator = ts.getSetExternalModuleIndicator(options);
        return typeof result === "object"
            ? { ...result, languageVersion, setExternalModuleIndicator, jsDocParsingMode: host.jsDocParsingMode }
            : { languageVersion, impliedNodeFormat: result, setExternalModuleIndicator, jsDocParsingMode: host.jsDocParsingMode };
    }

    // The host's SourceFile for a program file (called while tsc-rs reads it through the host).
    adoptHostSourceFile(nativeFileName) {
        if (this.hostSourceFiles.has(nativeFileName)) {
            return this.hostSourceFiles.get(nativeFileName);
        }
        const fileName = this.toJsFileName(nativeFileName);
        const sourceFileOptions = this.createSourceFileOptions(fileName);
        const sourceFile = this.getHostSourceFile(fileName, sourceFileOptions);
        this.adopt(nativeFileName, fileName, sourceFile, sourceFileOptions);
        return sourceFile;
    }

    getHostSourceFile(fileName, sourceFileOptions) {
        const { host, shouldCreateNewSourceFile } = this;
        return host.getSourceFileByPath === undefined
            ? host.getSourceFile(fileName, sourceFileOptions, undefined, shouldCreateNewSourceFile)
            : host.getSourceFileByPath(fileName, this.toPath(fileName), sourceFileOptions, undefined, shouldCreateNewSourceFile);
    }

    adopt(nativeFileName, fileName, sourceFile, sourceFileOptions) {
        this.hostSourceFiles.set(nativeFileName, sourceFile);
        if (sourceFile === undefined) {
            return;
        }
        this.hostTexts.set(nativeFileName, sourceFile.text);
        this.sourceFileOptions.set(nativeFileName, sourceFileOptions);
        this.prepareSourceFile(sourceFile, nativeFileName, fileName);
        this.structureKeys.set(nativeFileName, structureKeyOf(this.ts, sourceFile));
        if (!nativeFileName.startsWith(bundledLibs)) {
            this.hostResolver?.addFile(nativeFileName, fileName);
        }
    }

    // Lib files come from tsc-rs's bundled libs; TS 6.0 gets them from the host like any file (a
    // language service keeps them in its document registry with their versions). A host lib whose
    // text is not tsc-rs's has no node handles.
    adoptHostLibFiles(base) {
        for (const nativeFileName of this.nativeFileNames) {
            if (!nativeFileName.startsWith(bundledLibs) || this.hostSourceFiles.has(nativeFileName)) {
                continue;
            }
            const fileName = this.toJsFileName(nativeFileName);
            const known = this.hostLibFiles?.get(nativeFileName);
            const sourceFileOptions = known?.sourceFileOptions ?? this.createSourceFileOptions(fileName);
            const sourceFile = known?.sourceFile ?? this.getHostSourceFile(fileName, sourceFileOptions);
            if (sourceFile === undefined) {
                continue;
            }
            if (sourceFile === base?.hostSourceFiles.get(nativeFileName) ? base.foreignTexts.has(nativeFileName) : sourceFile.text !== this.textOf(nativeFileName)) {
                this.foreignTexts.add(nativeFileName);
            }
            this.adopt(nativeFileName, fileName, sourceFile, sourceFileOptions);
        }
    }

    // TS 6.0: src/compiler/program.ts findSourceFileWorker sets these on every program file.
    prepareSourceFile(sourceFile, nativeFileName, fileName) {
        const { ts } = this;
        if (this.constructed) {
            this.joinSourceFile(sourceFile);
        }
        const sourceFileOptions = this.sourceFileOptions.get(nativeFileName);
        sourceFile.fileName = fileName;
        sourceFile.path = this.toPath(fileName);
        sourceFile.resolvedPath = sourceFile.path;
        sourceFile.originalFileName = fileName;
        sourceFile.packageJsonLocations = sourceFileOptions?.packageJsonLocations?.length ? sourceFileOptions.packageJsonLocations : undefined;
        sourceFile.packageJsonScope = sourceFileOptions?.packageJsonScope;
        if (sourceFile.statements.length !== 0 && sourceFile.statements[0].parent === undefined) {
            ts.setParentRecursive(sourceFile, true);
        }
        this.collectExternalModuleReferences(sourceFile);
    }

    sourceFileOfNative(nativeFileName) {
        let sourceFile = this.sourceFiles.get(nativeFileName);
        if (sourceFile !== undefined) {
            return sourceFile;
        }
        if (!this.nativeFileNameSet.has(nativeFileName)) {
            return undefined;
        }
        sourceFile = this.hostSourceFiles.get(nativeFileName);
        if (sourceFile === undefined) {
            const fileName = this.toJsFileName(nativeFileName);
            const sourceFileOptions = this.createSourceFileOptions(fileName);
            this.sourceFileOptions.set(nativeFileName, sourceFileOptions);
            sourceFile = this.ts.createSourceFile(fileName, this.textOf(nativeFileName), sourceFileOptions, true);
            this.prepareSourceFile(sourceFile, nativeFileName, fileName);
        }
        this.sourceFiles.set(nativeFileName, sourceFile);
        return sourceFile;
    }

    // The file tsc-rs compiled where the host's SourceFile has another text (a host's own lib files).
    shadowSourceFile(nativeFileName) {
        this.shadowSourceFiles ??= new Map();
        let sourceFile = this.shadowSourceFiles.get(nativeFileName);
        if (sourceFile === undefined) {
            const fileName = this.toJsFileName(nativeFileName);
            sourceFile = this.ts.createSourceFile(fileName, this.textOf(nativeFileName), this.createSourceFileOptions(fileName), true);
            this.shadowSourceFiles.set(nativeFileName, sourceFile);
        }
        return sourceFile;
    }

    // The text tsc-rs compiled, without the binary syntax tree when the disk or lib/ has it.
    textOf(nativeFileName) {
        if (nativeFileName.startsWith(bundledLibs)) {
            return bundledLibText(nativeFileName) ?? this.nativeFile(nativeFileName).text;
        }
        if (this.usesDefaultHost) {
            const text = this.host.readFile(nativeFileName);
            if (text !== undefined) {
                return text;
            }
        }
        return this.nativeFile(nativeFileName).text;
    }

    // TS 6.0: src/compiler/program.ts collectExternalModuleReferences
    collectExternalModuleReferences(file) {
        if (file.imports) {
            return;
        }
        const { ts, options } = this;
        const isJavaScriptFile = ts.isSourceFileJS(file);
        const isExternalModuleFile = ts.isExternalModule(file);
        let imports;
        let moduleAugmentations;
        let ambientModules;
        if (isJavaScriptFile || (!file.isDeclarationFile && (ts.getIsolatedModules(options) || isExternalModuleFile))) {
            if (options.importHelpers) {
                imports = [createSyntheticImport(ts, ts.externalHelpersModuleNameText, file)];
            }
            const jsxImport = ts.getJSXRuntimeImport(ts.getJSXImplicitImportBase(options, file), options);
            if (jsxImport) {
                (imports ??= []).push(createSyntheticImport(ts, jsxImport, file));
            }
        }
        const collectModuleReferences = (node, inAmbientModule) => {
            if (ts.isAnyImportOrReExport(node)) {
                const moduleNameExpr = ts.getExternalModuleName(node);
                if (moduleNameExpr && ts.isStringLiteral(moduleNameExpr) && moduleNameExpr.text && (!inAmbientModule || !ts.isExternalModuleNameRelative(moduleNameExpr.text))) {
                    ts.setParentRecursive(node, false);
                    (imports ??= []).push(moduleNameExpr);
                }
                return;
            }
            if (!ts.isModuleDeclaration(node) || !ts.isAmbientModule(node) || !(inAmbientModule || ts.hasSyntacticModifier(node, ts.ModifierFlags.Ambient) || file.isDeclarationFile)) {
                return;
            }
            node.name.parent = node;
            const nameText = ts.getTextOfIdentifierOrLiteral(node.name);
            if (isExternalModuleFile || (inAmbientModule && !ts.isExternalModuleNameRelative(nameText))) {
                (moduleAugmentations ??= []).push(node.name);
                return;
            }
            if (inAmbientModule) {
                return;
            }
            if (file.isDeclarationFile) {
                (ambientModules ??= []).push(nameText);
            }
            for (const statement of node.body?.statements ?? []) {
                collectModuleReferences(statement, true);
            }
        };
        for (const node of file.statements) {
            collectModuleReferences(node, false);
        }
        if ((file.flags & ts.NodeFlags.PossiblyContainsDynamicImport) || isJavaScriptFile) {
            ts.forEachDynamicImportOrRequireCall(file, true, true, (node, moduleSpecifier) => {
                ts.setParentRecursive(node, false);
                (imports ??= []).push(moduleSpecifier);
            });
        }
        file.imports = imports ?? ts.emptyArray;
        file.moduleAugmentations = moduleAugmentations ?? ts.emptyArray;
        file.ambientModuleNames = ambientModules ?? ts.emptyArray;
    }

    // TS 6.0: src/compiler/program.ts getModuleNames
    moduleNameLiterals(sourceFile) {
        this.collectExternalModuleReferences(sourceFile);
        const literals = [...sourceFile.imports];
        for (const augmentation of sourceFile.moduleAugmentations) {
            if (augmentation.kind === this.ts.SyntaxKind.StringLiteral) {
                literals.push(augmentation);
            }
        }
        return literals;
    }

    // The tsc-rs node handle of a TS 6.0 node of this program, or undefined.
    nodeToHandle(node) {
        const { ts } = this;
        let parseNode = ts.getParseTreeNode(node);
        if (parseNode === undefined) {
            return undefined;
        }
        if (parseNode.redirectInfo !== undefined) {
            parseNode = parseNode.redirectInfo.redirectTarget;
        }
        const sourceFile = ts.getSourceFileOfNode(parseNode);
        if (sourceFile === undefined || this.getSourceFile(sourceFile.fileName) !== sourceFile) {
            return undefined;
        }
        const nativeFileName = this.toNativeFileName(sourceFile.fileName);
        if (this.foreignTexts.has(nativeFileName)) {
            return undefined;
        }
        if (parseNode === sourceFile) {
            return `1.${syntaxKindToNative(ts.SyntaxKind.SourceFile)}.${this.nativePathOf(nativeFileName)}`;
        }
        const file = this.nativeFile(nativeFileName);
        const index = file.indexOf(syntaxKindToNative(parseNode.kind), parseNode.pos, parseNode.end)
            ?? this.indexOfNativeShape(file, parseNode);
        return index === undefined ? undefined : file.handle(index);
    }

    // A node that tsc-rs parses as another kind over the same range (nativeShapes).
    indexOfNativeShape(file, node) {
        const kind = nativeShapes(this.ts).toNative.get(node.kind);
        return kind === undefined ? undefined : file.indexOf(syntaxKindToNative(kind), node.pos, node.end);
    }

    // The TS 6.0 node for a tsc-rs node handle, or undefined.
    handleToNode(handle) {
        const { index, path } = parseHandle(handle);
        const nativeFileName = this.nativePathToNativeFileName.get(path);
        if (nativeFileName === undefined) {
            return undefined;
        }
        const sourceFile = this.foreignTexts.has(nativeFileName) ? this.shadowSourceFile(nativeFileName) : this.sourceFileOfNative(nativeFileName);
        if (index === 1) {
            return sourceFile;
        }
        const file = this.nativeFile(nativeFileName);
        const kind = syntaxKindToTs6(file.kind(index));
        const found = findNode(this.ts, sourceFile, kind, file.pos(index), file.end(index));
        const jsKind = found === undefined ? nativeShapes(this.ts).toJs.get(kind) : undefined;
        return jsKind === undefined ? found : findNode(this.ts, sourceFile, jsKind, file.pos(index), file.end(index));
    }

    // Diagnostics do not change for a program, so each answer is kept.
    diagnostics(method, sourceFile, cancellationToken) {
        cancellationToken?.throwIfCancellationRequested();
        let byFile = this.diagnosticsCache.get(method);
        if (byFile === undefined) {
            byFile = new Map();
            this.diagnosticsCache.set(method, byFile);
        }
        const key = sourceFile === undefined ? undefined : this.nativeFileNameOf(sourceFile);
        let result = byFile.get(key);
        if (result === undefined) {
            result = this.withJsNames(toTsDiagnostics(this.ts, this.nativeDiagnosticsOf(method, key), name => this.sourceFileOfNative(name)));
            // TS 6.0: src/compiler/program.ts getDiagnosticsHelper sorts by file path (suggestions stay
            // in the checker's order).
            if (method !== "getSuggestionDiagnostics") {
                result = this.ts.sortAndDeduplicateDiagnostics(result);
            }
            byFile.set(key, result);
        }
        return result;
    }

    nativeDiagnosticsOf(method, key) {
        let byFile = this.nativeDiagnostics.get(method);
        if (byFile === undefined) {
            byFile = new Map();
            this.nativeDiagnostics.set(method, byFile);
        }
        if (!byFile.has(key)) {
            byFile.set(key, this.request(method, { files: key === undefined ? undefined : [key] }) ?? []);
        }
        return byFile.get(key);
    }

    // tsc-rs's messages name files absolutely; with a relative current directory TS 6.0's names are
    // relative to it (see jsNameOf).
    withJsNames(diagnostics) {
        const prefix = this.relativePrefix;
        if (prefix === undefined) {
            return diagnostics;
        }
        const replacement = this.currentDirectory === "" ? "" : this.ts.ensureTrailingDirectorySeparator(this.currentDirectory);
        const rename = chain => {
            if (typeof chain === "string") {
                return chain.split(prefix).join(replacement);
            }
            chain.messageText = rename(chain.messageText);
            chain.next?.forEach(rename);
            return chain;
        };
        for (const diagnostic of diagnostics) {
            diagnostic.messageText = rename(diagnostic.messageText);
            diagnostic.relatedInformation?.forEach(related => {
                related.messageText = rename(related.messageText);
            });
        }
        return diagnostics;
    }

    // SourceFileMetadata of program files, fetched for all files at once.
    metadataOf(nativeFileName) {
        if (!this.metadata.has(nativeFileName)) {
            const missing = this.nativeFileNames.filter(name => !this.metadata.has(name));
            const results = this.requestBatch("getSourceFileMetadata", missing.map(file => ({ file })));
            missing.forEach((name, index) => this.metadata.set(name, results[index] instanceof Error ? undefined : results[index]));
        }
        return this.metadata.get(nativeFileName);
    }

    // ScriptReferenceHost

    getCompilerOptions() {
        return this.options;
    }

    getSourceFile(fileName) {
        return this.getSourceFileByPath(this.toPath(fileName));
    }

    getSourceFileByPath(path) {
        const nativeFileName = this.pathToNativeFileName.get(path);
        return nativeFileName === undefined ? this.redirectSourceFile(path) : this.sourceFileOfNative(nativeFileName);
    }

    // TS 6.0 keeps one SourceFile per package name and version: another copy of a package file is
    // a redirect to the first one (src/compiler/program.ts findSourceFileWorker). tsc-rs leaves
    // those copies out of the program; the module resolutions name them.
    redirectSourceFile(path) {
        if (!path.includes(this.ts.nodeModulesPathPart)) {
            return undefined;
        }
        const redirect = this.packageRedirects().get(path);
        if (redirect === undefined) {
            return undefined;
        }
        redirect.sourceFile ??= this.createRedirect(path, redirect);
        return redirect.sourceFile;
    }

    // Package file paths outside the program -> { fileName, target } (the native name of the program
    // file of the same package id), from every module resolution of the program.
    packageRedirects() {
        if (this.redirects !== undefined) {
            return this.redirects;
        }
        const { ts } = this;
        const targets = new Map();
        const copies = [];
        this.forEachResolvedModule(resolution => {
            const resolved = resolution.resolvedModule;
            if (resolved?.packageId === undefined) {
                return;
            }
            const key = ts.packageIdToString(resolved.packageId);
            const path = this.toPath(resolved.resolvedFileName);
            const nativeFileName = this.pathToNativeFileName.get(path);
            if (nativeFileName === undefined) {
                copies.push({ path, key, fileName: resolved.resolvedFileName });
            }
            else if (!targets.has(key)) {
                targets.set(key, nativeFileName);
            }
        });
        this.redirects = new Map();
        for (const { path, key, fileName } of copies) {
            if (targets.has(key) && !this.redirects.has(path)) {
                this.redirects.set(path, { fileName, target: targets.get(key) });
            }
        }
        return this.redirects;
    }

    // TS 6.0: src/compiler/program.ts createRedirectedSourceFile
    createRedirect(path, { fileName, target }) {
        const { ts, host } = this;
        const redirectTarget = this.sourceFileOfNative(target);
        const sourceFileOptions = this.createSourceFileOptions(fileName);
        let unredirected;
        const redirect = ts.parseNodeFactory.createRedirectedSourceFile({
            redirectTarget,
            // TS 6.0 reads the copy itself; only reuse checks look at it.
            get unredirected() {
                unredirected ??= ts.createSourceFile(fileName, host.readFile(fileName) ?? redirectTarget.text, sourceFileOptions, true);
                return unredirected;
            },
        });
        redirect.fileName = fileName;
        redirect.path = path;
        redirect.resolvedPath = path;
        redirect.originalFileName = fileName;
        redirect.packageJsonLocations = sourceFileOptions.packageJsonLocations?.length ? sourceFileOptions.packageJsonLocations : undefined;
        redirect.packageJsonScope = sourceFileOptions.packageJsonScope;
        return redirect;
    }

    getCurrentDirectory() {
        return this.currentDirectory;
    }

    // Program

    getRootFileNames() {
        return this.rootNames;
    }

    getSourceFiles() {
        this.sourceFileList ??= this.nativeFileNames.map(name => this.sourceFileOfNative(name));
        return this.sourceFileList;
    }

    emit(targetSourceFile, writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers, forceDtsEmit, skipBuildInfo) {
        cancellationToken?.throwIfCancellationRequested();
        return require("./emit.js").emit(this, targetSourceFile, writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers, forceDtsEmit, skipBuildInfo);
    }

    // TS 6.0: src/compiler/builder.ts emitBuildInfo; incremental state belongs to builder programs.
    emitBuildInfo(writeFile, cancellationToken) {
        const emit = require("./emit.js");
        if (emit.emitBuildInfo !== undefined) {
            return emit.emitBuildInfo(this, writeFile, cancellationToken);
        }
        return { emitSkipped: true, diagnostics: this.ts.emptyArray };
    }

    getOptionsDiagnostics(cancellationToken) {
        cancellationToken?.throwIfCancellationRequested();
        this.optionsDiagnostics ??= this.ts.sortAndDeduplicateDiagnostics([
            ...this.optionDiagnostics,
            ...this.withJsNames(toTsOptionDiagnostics(this.ts, this.options, this.request("getProgramDiagnostics") ?? [], name => this.sourceFileOfNative(name))),
        ]);
        return this.optionsDiagnostics;
    }

    // tsc-rs's global diagnostics also hold its program diagnostics; TS 6.0 reports those as
    // options diagnostics only.
    getGlobalDiagnostics(cancellationToken) {
        if (this.globalDiagnostics === undefined) {
            const programKeys = new Set(this.diagnostics("getProgramDiagnostics", undefined, cancellationToken).map(diagnosticKey));
            this.globalDiagnostics = this.diagnostics("getGlobalDiagnostics", undefined, cancellationToken).filter(diagnostic => !programKeys.has(diagnosticKey(diagnostic)));
        }
        return this.globalDiagnostics;
    }

    getSyntacticDiagnostics(sourceFile, cancellationToken) {
        return this.diagnostics("getSyntacticDiagnostics", sourceFile, cancellationToken);
    }

    // `nodesToCheck` (the language service's region diagnostics) is answered for the whole file.
    getSemanticDiagnostics(sourceFile, cancellationToken) {
        return this.diagnostics("getSemanticDiagnostics", sourceFile, cancellationToken);
    }

    getCachedSemanticDiagnostics(sourceFile) {
        const byFile = this.diagnosticsCache.get("getSemanticDiagnostics");
        return byFile?.get(sourceFile === undefined ? undefined : this.pathToNativeFileName.get(sourceFile.path));
    }

    getBindAndCheckDiagnostics(sourceFile, cancellationToken) {
        return this.getSemanticDiagnostics(sourceFile, cancellationToken);
    }

    // TS 6.0 reports file processing diagnostics of a file here; tsc-rs reports them as program diagnostics.
    getProgramDiagnostics() {
        return this.ts.emptyArray;
    }

    getSuggestionDiagnostics(sourceFile, cancellationToken) {
        return this.diagnostics("getSuggestionDiagnostics", sourceFile, cancellationToken);
    }

    getDeclarationDiagnostics(sourceFile, cancellationToken) {
        return this.diagnostics("getDeclarationDiagnostics", sourceFile, cancellationToken);
    }

    getConfigFileParsingDiagnostics() {
        return this.configFileParsingDiagnostics ?? this.ts.emptyArray;
    }

    getTypeChecker() {
        this.checker ??= completeChecker(this, require("./checker.js").createChecker(this));
        return this.checker;
    }

    getClassifiableNames() {
        if (this.classifiableNames === undefined) {
            this.classifiableNames = new Set();
            for (const sourceFile of this.getSourceFiles()) {
                sourceFile.identifiers?.forEach((_, name) => this.classifiableNames.add(this.ts.escapeLeadingUnderscores(name)));
            }
        }
        return this.classifiableNames;
    }

    // TS 6.0 counts come from its checker; tsc-rs reports no symbol, type or relation counts.
    getNodeCount() {
        return this.getSourceFiles().reduce((count, file) => count + (file.nodeCount ?? 0), 0);
    }

    getIdentifierCount() {
        return this.getSourceFiles().reduce((count, file) => count + (file.identifierCount ?? 0), 0);
    }

    getSymbolCount() {
        return 0;
    }

    getTypeCount() {
        return 0;
    }

    getInstantiationCount() {
        return 0;
    }

    getRelationCacheSizes() {
        return { assignable: 0, identity: 0, subtype: 0, strictSubtype: 0 };
    }

    getFileProcessingDiagnostics() {
        return undefined;
    }

    // TS 6.0: src/compiler/program.ts automaticTypeDirectiveNames
    getAutomaticTypeDirectiveNames() {
        this.automaticTypeDirectiveNames ??= this.ts.getAutomaticTypeDirectiveNames(this.options, this.host);
        return this.automaticTypeDirectiveNames;
    }

    getAutomaticTypeDirectiveResolutions() {
        if (this.automaticTypeDirectiveResolutions === undefined) {
            const { ts } = this;
            const resolutions = ts.createModeAwareCache();
            const containingFile = ts.combinePaths(this.options.configFilePath ? ts.getDirectoryPath(this.options.configFilePath) : this.currentDirectory, ts.inferredTypesContainingFile);
            for (const name of this.getAutomaticTypeDirectiveNames()) {
                resolutions.set(name, undefined, ts.resolveTypeReferenceDirective(name, containingFile, this.options, this.host, undefined, undefined, undefined));
            }
            this.automaticTypeDirectiveResolutions = resolutions;
        }
        return this.automaticTypeDirectiveResolutions;
    }

    isSourceFileFromExternalLibrary(file) {
        if (file.redirectInfo !== undefined) {
            return this.isSourceFileFromExternalLibrary(file.redirectInfo.redirectTarget);
        }
        const nativeFileName = this.pathToNativeFileName.get(file.path ?? this.toPath(file.fileName));
        return nativeFileName !== undefined && (this.metadataOf(nativeFileName)?.isFromExternalLibrary ?? false);
    }

    isSourceFileDefaultLibrary(file) {
        const nativeFileName = this.pathToNativeFileName.get(file.path ?? this.toPath(file.fileName));
        return nativeFileName !== undefined && (this.metadataOf(nativeFileName)?.isDefaultLibrary ?? false);
    }

    // Module formats follow TS 6.0's rules over the TS 6.0 SourceFiles.

    // TS 6.0: src/compiler/program.ts getCompilerOptionsForFile: a source file of a referenced
    // project resolves with that project's options.
    getCompilerOptionsForFile(sourceFile) {
        return this.getRedirectFromSourceFile(sourceFile.originalFileName ?? sourceFile.fileName)?.resolvedRef.commandLine.options ?? this.options;
    }

    getModeForUsageLocation(file, usage) {
        return this.ts.getModeForUsageLocation(file, usage, this.getCompilerOptionsForFile(file));
    }

    getEmitSyntaxForUsageLocation(file, usage) {
        return getEmitSyntaxForUsageLocation(this.ts, file, usage, this.getCompilerOptionsForFile(file));
    }

    getModeForResolutionAtIndex(file, index) {
        this.collectExternalModuleReferences(file);
        return this.ts.getModeForResolutionAtIndex(file, index, this.getCompilerOptionsForFile(file));
    }

    getDefaultResolutionModeForFile(sourceFile) {
        return this.ts.getDefaultResolutionModeForFileWorker(sourceFile, this.getCompilerOptionsForFile(sourceFile));
    }

    getImpliedNodeFormatForEmit(sourceFile) {
        return this.ts.getImpliedNodeFormatForEmitWorker(sourceFile, this.getCompilerOptionsForFile(sourceFile));
    }

    getEmitModuleFormatOfFile(sourceFile) {
        return this.ts.getEmitModuleFormatOfFileWorker(sourceFile, this.getCompilerOptionsForFile(sourceFile));
    }

    shouldTransformImportCall(sourceFile) {
        return shouldTransformImportCall(this.ts, sourceFile, this.getCompilerOptionsForFile(sourceFile));
    }

    // Module and type reference resolutions, in TS 6.0's shapes with the program's file names.

    // TS 6.0 resolves every module name of every file while it builds the program; tsc-rs is asked
    // per file, when a caller needs it.
    moduleResolutionsOf(nativeFileName) {
        let resolutions = this.moduleResolutions.get(nativeFileName);
        if (resolutions === undefined) {
            this.resolveModulesOf([nativeFileName]);
            resolutions = this.moduleResolutions.get(nativeFileName);
        }
        return resolutions;
    }

    resolveModulesOf(nativeFileNames) {
        const { ts } = this;
        const requests = [];
        const entries = [];
        for (const nativeFileName of nativeFileNames) {
            const resolutions = ts.createModeAwareCache();
            this.moduleResolutions.set(nativeFileName, resolutions);
            if (nativeFileName.startsWith(bundledLibs)) {
                continue;
            }
            const sourceFile = this.sourceFileOfNative(nativeFileName);
            const options = this.getCompilerOptionsForFile(sourceFile);
            for (const literal of this.moduleNameLiterals(sourceFile)) {
                const mode = ts.getModeForUsageLocation(sourceFile, literal, options);
                entries.push({ resolutions, name: literal.text, mode, nativeFileName });
                requests.push({ file: nativeFileName, moduleName: literal.text, mode: mode ?? 0 });
            }
        }
        const results = this.requestBatch("getResolvedModule", requests);
        const retries = [];
        entries.forEach((entry, index) => {
            const result = results[index];
            if (result === undefined || result === null || result instanceof Error) {
                retries.push(entry);
                return;
            }
            entry.resolutions.set(entry.name, entry.mode, this.toResolvedModule(result));
        });
        this.retryModuleResolutions(retries);
    }

    // tsc-rs keys a resolution by the mode it used, which can differ from TS 6.0's mode.
    retryModuleResolutions(entries) {
        const { ModuleKind } = this.ts;
        const requests = [];
        const owners = [];
        for (const entry of entries) {
            for (const mode of [0, ModuleKind.ESNext, ModuleKind.CommonJS]) {
                if (mode !== (entry.mode ?? 0)) {
                    requests.push({ file: entry.nativeFileName, moduleName: entry.name, mode });
                    owners.push(entry);
                }
            }
        }
        const results = this.requestBatch("getResolvedModule", requests);
        const found = new Set();
        results.forEach((result, index) => {
            const entry = owners[index];
            if (found.has(entry) || result === undefined || result === null || result instanceof Error) {
                return;
            }
            found.add(entry);
            entry.resolutions.set(entry.name, entry.mode, this.toResolvedModule(result));
        });
        for (const entry of entries) {
            if (!found.has(entry)) {
                entry.resolutions.set(entry.name, entry.mode, { resolvedModule: undefined });
            }
        }
    }

    toResolvedModule(native) {
        return {
            resolvedModule: {
                resolvedFileName: this.jsNameOf(native.resolvedFileName),
                originalPath: native.originalPath || undefined,
                extension: native.extension,
                isExternalLibraryImport: native.isExternalLibraryImport ?? false,
                packageId: toPackageId(native.packageId),
                resolvedUsingTsExtension: native.resolvedUsingTsExtension ?? false,
            },
            failedLookupLocations: undefined,
            affectingLocations: undefined,
            resolutionDiagnostics: undefined,
        };
    }

    getResolvedModule(file, moduleName, mode) {
        const nativeFileName = this.pathToNativeFileName.get(file.path ?? this.toPath(file.fileName));
        return nativeFileName === undefined ? undefined : this.moduleResolutionsOf(nativeFileName).get(moduleName, mode);
    }

    getResolvedModuleFromModuleSpecifier(moduleSpecifier, sourceFile) {
        sourceFile ??= this.ts.getSourceFileOfNode(moduleSpecifier);
        if (sourceFile === undefined) {
            throw new Error("tsc-rs: `moduleSpecifier` must have a `SourceFile` ancestor. Use `program.getResolvedModule` instead to provide the containing file and resolution mode.");
        }
        return this.getResolvedModule(sourceFile, moduleSpecifier.text, this.getModeForUsageLocation(sourceFile, moduleSpecifier));
    }

    forEachResolvedModule(callback, file) {
        const nativeFileNames = file === undefined ? this.nativeFileNames : [this.pathToNativeFileName.get(file.path ?? this.toPath(file.fileName))].filter(name => name !== undefined);
        const missing = nativeFileNames.filter(name => !this.moduleResolutions.has(name));
        if (missing.length !== 0) {
            this.resolveModulesOf(missing);
        }
        for (const nativeFileName of nativeFileNames) {
            const filePath = this.toPath(this.toJsFileName(nativeFileName));
            this.moduleResolutions.get(nativeFileName).forEach((resolution, moduleName, mode) => callback(resolution, moduleName, mode, filePath));
        }
    }

    get resolvedModules() {
        const result = new Map();
        this.forEachResolvedModule((resolution, moduleName, mode, filePath) => {
            let resolutions = result.get(filePath);
            if (resolutions === undefined) {
                resolutions = this.ts.createModeAwareCache();
                result.set(filePath, resolutions);
            }
            resolutions.set(moduleName, mode, resolution);
        });
        return result;
    }

    typeReferenceResolutionsOf(nativeFileName) {
        let resolutions = this.typeReferenceResolutions.get(nativeFileName);
        if (resolutions !== undefined) {
            return resolutions;
        }
        const { ts } = this;
        resolutions = ts.createModeAwareCache();
        this.typeReferenceResolutions.set(nativeFileName, resolutions);
        const sourceFile = this.sourceFileOfNative(nativeFileName);
        const references = sourceFile?.typeReferenceDirectives ?? [];
        const entries = references.map(reference => ({ name: ts.toFileNameLowerCase(reference.fileName), mode: reference.resolutionMode || this.getDefaultResolutionModeForFile(sourceFile) }));
        const results = this.requestBatch("getResolvedTypeReferenceDirective", entries.map(({ name, mode }) => ({ file: nativeFileName, typeDirectiveName: name, mode: mode ?? 0 })));
        entries.forEach(({ name, mode }, index) => {
            const result = results[index];
            const resolved = result === undefined || result === null || result instanceof Error ? undefined : {
                primary: result.primary,
                resolvedFileName: this.jsNameOf(result.resolvedFileName),
                originalPath: result.originalPath || undefined,
                packageId: toPackageId(result.packageId),
                isExternalLibraryImport: result.isExternalLibraryImport ?? false,
            };
            resolutions.set(name, mode, { resolvedTypeReferenceDirective: resolved, failedLookupLocations: undefined, affectingLocations: undefined, resolutionDiagnostics: undefined });
        });
        return resolutions;
    }

    getResolvedTypeReferenceDirective(file, typeDirectiveName, mode) {
        const nativeFileName = this.pathToNativeFileName.get(file.path ?? this.toPath(file.fileName));
        return nativeFileName === undefined ? undefined : this.typeReferenceResolutionsOf(nativeFileName).get(typeDirectiveName, mode);
    }

    getResolvedTypeReferenceDirectiveFromTypeReferenceDirective(typeReference, sourceFile) {
        return this.getResolvedTypeReferenceDirective(sourceFile, typeReference.fileName, typeReference.resolutionMode || this.getDefaultResolutionModeForFile(sourceFile));
    }

    forEachResolvedTypeReferenceDirective(callback, file) {
        const nativeFileNames = file === undefined ? this.nativeFileNames : [this.pathToNativeFileName.get(file.path ?? this.toPath(file.fileName))].filter(name => name !== undefined);
        for (const nativeFileName of nativeFileNames) {
            const filePath = this.toPath(this.toJsFileName(nativeFileName));
            this.typeReferenceResolutionsOf(nativeFileName).forEach((resolution, name, mode) => callback(resolution, name, mode, filePath));
        }
    }

    get resolvedTypeReferenceDirectiveNames() {
        const result = new Map();
        this.forEachResolvedTypeReferenceDirective((resolution, name, mode, filePath) => {
            let resolutions = result.get(filePath);
            if (resolutions === undefined) {
                resolutions = this.ts.createModeAwareCache();
                result.set(filePath, resolutions);
            }
            resolutions.set(name, mode, resolution);
        });
        return result;
    }

    // TS 6.0: src/compiler/program.ts findSourceFileWorker records the package of each file found
    // through a resolution with a package id.
    get sourceFileToPackageName() {
        if (this.packageNames === undefined) {
            this.packageNames = new Map();
            this.packageMap = new Map();
            this.forEachResolvedModule(resolution => {
                const resolved = resolution.resolvedModule;
                if (resolved?.packageId === undefined) {
                    return;
                }
                const packageName = this.ts.packageIdToPackageName(resolved.packageId);
                const path = this.toPath(resolved.resolvedFileName);
                if (!this.packageNames.has(path)) {
                    this.packageNames.set(path, packageName);
                }
                // TS 6.0: src/compiler/program.ts processImportedModules
                this.packageMap.set(resolved.packageId.name, (this.packageMap.get(resolved.packageId.name) ?? false) || resolved.extension === this.ts.Extension.Dts);
            });
        }
        return this.packageNames;
    }

    getCurrentPackagesMap() {
        void this.sourceFileToPackageName;
        return this.packageMap;
    }

    typesPackageExists(packageName) {
        return this.getCurrentPackagesMap().has(this.ts.getTypesPackageName(packageName));
    }

    packageBundlesTypes(packageName) {
        return !!this.getCurrentPackagesMap().get(packageName);
    }

    // TS 6.0: redirect target path -> the file names redirected to it.
    get redirectTargetsMap() {
        if (this.redirectTargets === undefined) {
            this.redirectTargets = this.ts.createMultiMap();
            for (const { fileName, target } of this.packageRedirects().values()) {
                this.redirectTargets.add(this.toPath(this.toJsFileName(target)), fileName);
            }
        }
        return this.redirectTargets;
    }

    // TS 6.0: src/compiler/program.ts collectExternalModuleReferences, over the files that are not
    // from node_modules.
    get usesUriStyleNodeCoreModules() {
        if (this.uriStyleNodeCoreModules === undefined) {
            const { ts } = this;
            let uses;
            for (const sourceFile of this.getSourceFiles()) {
                if (sourceFile.isDeclarationFile || this.isSourceFileFromExternalLibrary(sourceFile)) {
                    continue;
                }
                for (const literal of sourceFile.imports) {
                    if (literal.text.startsWith("node:") && !ts.exclusivelyPrefixedNodeCoreModules.has(literal.text)) {
                        uses = true;
                        break;
                    }
                    if (uses === undefined && ts.unprefixedNodeCoreModules.has(literal.text)) {
                        uses = false;
                    }
                }
                if (uses) {
                    break;
                }
            }
            this.uriStyleNodeCoreModules = { uses };
        }
        return this.uriStyleNodeCoreModules.uses;
    }

    // TS 6.0: src/compiler/program.ts resolvedLibReferences
    get resolvedLibReferences() {
        if (this.libReferences === undefined) {
            this.libReferences = new Map();
            for (const nativeFileName of this.nativeFileNames) {
                if (nativeFileName.startsWith(bundledLibs)) {
                    this.libReferences.set(nativeFileName.slice(bundledLibs.length), { resolution: { resolvedModule: undefined }, actual: this.toJsFileName(nativeFileName) });
                }
            }
        }
        return this.libReferences;
    }

    getLibFileFromReference(reference) {
        const libFileName = this.ts.getLibFileNameFromLibReference(reference);
        const actual = libFileName && this.resolvedLibReferences.get(libFileName)?.actual;
        return actual === undefined ? undefined : this.getSourceFile(actual);
    }

    // TS 6.0: src/compiler/program.ts getSourceFileFromReference
    getSourceFileFromReference(referencingFile, reference) {
        const fileName = this.ts.resolveTripleslashReference(reference.fileName, referencingFile.fileName);
        for (const candidate of this.referenceCandidates(fileName, false)) {
            const sourceFile = this.getSourceFile(candidate);
            if (sourceFile !== undefined) {
                return sourceFile;
            }
        }
        return undefined;
    }

    // TS 6.0: src/compiler/program.ts getSourceFileFromReferenceWorker: the file names a reference
    // tries, in order, until one is a file. While the program is built (`processing`), an unknown
    // extension with allowNonTsExtensions is not tried with the supported ones.
    referenceCandidates(fileName, processing) {
        const { ts, options } = this;
        this.supportedExtensions ??= ts.getSupportedExtensions(options);
        if (ts.hasExtension(fileName)) {
            const canonicalFileName = this.getCanonicalFileName(fileName);
            const extensions = ts.getSupportedExtensionsWithJsonIfResolveJsonModule(options, this.supportedExtensions).flat();
            return options.allowNonTsExtensions || extensions.some(extension => ts.fileExtensionIs(canonicalFileName, extension)) ? [fileName] : [];
        }
        if (options.allowNonTsExtensions && processing) {
            return [fileName];
        }
        return [...(options.allowNonTsExtensions ? [fileName] : []), ...this.supportedExtensions[0].map(extension => fileName + extension)];
    }

    getModuleResolutionCache() {
        this.moduleResolutionCache ??= this.host.getModuleResolutionCache?.() ?? this.ts.createModuleResolutionCache(this.currentDirectory, this.getCanonicalFileName, this.options);
        return this.moduleResolutionCache;
    }

    getFilesByNameMap() {
        const result = new Map();
        for (const nativeFileName of this.nativeFileNames) {
            result.set(this.toPath(this.toJsFileName(nativeFileName)), this.sourceFileOfNative(nativeFileName));
        }
        for (const path of this.getMissingFilePaths().keys()) {
            result.set(path, false);
        }
        return result;
    }

    // TS 6.0: src/compiler/program.ts missingFileNames: root files and `/// <reference path>`
    // targets that are not in the program.
    getMissingFilePaths() {
        if (this.missingFilePaths === undefined) {
            const { ts } = this;
            const missing = new Map();
            const addMissing = fileName => {
                const path = this.toPath(fileName);
                if (!this.pathToNativeFileName.has(path)) {
                    missing.set(path, fileName);
                }
            };
            for (const name of this.rootNames) {
                addMissing(ts.normalizePath(name));
            }
            if (this.missingDefaultLib !== undefined) {
                addMissing(this.missingDefaultLib);
            }
            if (!this.options.noResolve) {
                for (const nativeFileName of this.nativeFileNames.filter(name => !name.startsWith(bundledLibs))) {
                    const sourceFile = this.sourceFileOfNative(nativeFileName);
                    for (const reference of sourceFile.referencedFiles) {
                        const fileName = ts.resolveTripleslashReference(reference.fileName, sourceFile.fileName);
                        for (const candidate of this.referenceCandidates(fileName, true)) {
                            if (this.getSourceFile(candidate) !== undefined) {
                                break;
                            }
                            addMissing(candidate);
                        }
                    }
                }
            }
            this.missingFilePaths = missing;
        }
        return this.missingFilePaths;
    }

    // TS 6.0: src/compiler/program.ts fileReasons, rebuilt from the program's files and resolutions.
    getFileIncludeReasons() {
        if (this.fileIncludeReasons !== undefined) {
            return this.fileIncludeReasons;
        }
        const { ts } = this;
        const { FileIncludeKind } = ts;
        const reasons = ts.createMultiMap();
        const add = (fileName, reason) => {
            const path = fileName === undefined ? undefined : this.toPath(fileName);
            if (path !== undefined && this.pathToNativeFileName.has(path)) {
                reasons.add(path, reason);
            }
        };
        this.rootNames.forEach((name, index) => add(ts.normalizePath(name), { kind: FileIncludeKind.RootFile, index }));
        const libs = this.options.lib?.map(lib => ts.libMap.get(lib.toLowerCase()) ?? lib);
        for (const nativeFileName of this.nativeFileNames) {
            const fileName = this.toJsFileName(nativeFileName);
            const sourceFile = this.sourceFileOfNative(nativeFileName);
            if (nativeFileName.startsWith(bundledLibs)) {
                const index = libs?.indexOf(nativeFileName.slice(bundledLibs.length));
                if (libs === undefined || index !== -1) {
                    reasons.add(this.toPath(fileName), libs === undefined ? { kind: FileIncludeKind.LibFile } : { kind: FileIncludeKind.LibFile, index });
                }
            }
            sourceFile.libReferenceDirectives.forEach((reference, index) => {
                add(this.getLibFileFromReference(reference)?.fileName, { kind: FileIncludeKind.LibReferenceDirective, file: sourceFile.path, index });
            });
            sourceFile.referencedFiles.forEach((reference, index) => {
                add(this.getSourceFileFromReference(sourceFile, reference)?.fileName, { kind: FileIncludeKind.ReferenceFile, file: sourceFile.path, index });
            });
            if (nativeFileName.startsWith(bundledLibs)) {
                continue;
            }
            sourceFile.typeReferenceDirectives.forEach((reference, index) => {
                const resolution = this.getResolvedTypeReferenceDirectiveFromTypeReferenceDirective(reference, sourceFile);
                add(resolution?.resolvedTypeReferenceDirective?.resolvedFileName, { kind: FileIncludeKind.TypeReferenceDirective, file: sourceFile.path, index });
            });
            // TS 6.0: src/compiler/program.ts processImportedModules adds files for imports only.
            sourceFile.imports.forEach((literal, index) => {
                const resolved = this.getResolvedModule(sourceFile, literal.text, this.getModeForUsageLocation(sourceFile, literal))?.resolvedModule;
                if (resolved === undefined || ts.getResolutionDiagnostic(this.options, resolved, sourceFile) || (!ts.isInJSFile(literal) && (literal.flags & ts.NodeFlags.JSDoc))) {
                    return;
                }
                add(resolved.resolvedFileName, { kind: FileIncludeKind.Import, file: sourceFile.path, index });
            });
        }
        this.getAutomaticTypeDirectiveResolutions().forEach((resolution, typeReference) => {
            const resolved = resolution.resolvedTypeReferenceDirective;
            add(resolved?.resolvedFileName, { kind: FileIncludeKind.AutomaticTypeDirectiveFile, typeReference, packageId: resolved?.packageId });
        });
        this.fileIncludeReasons = reasons;
        return reasons;
    }

    // TS 6.0: src/compiler/program.ts getCommonSourceDirectory
    getCommonSourceDirectory() {
        if (this.commonSourceDirectory === undefined) {
            const { ts } = this;
            const emittedFiles = this.getSourceFiles().filter(file => ts.sourceFileMayBeEmitted(file, this));
            this.commonSourceDirectory = ts.getCommonSourceDirectory(
                this.options,
                () => emittedFiles.filter(file => !file.isDeclarationFile).map(file => file.fileName),
                this.currentDirectory,
                this.getCanonicalFileName,
            );
        }
        return this.commonSourceDirectory;
    }

    // TS 6.0: src/compiler/program.ts isEmittedFile
    isEmittedFile(file) {
        const { ts, options } = this;
        if (options.noEmit) {
            return false;
        }
        const filePath = this.toPath(file);
        if (this.getSourceFileByPath(filePath)) {
            return false;
        }
        const isSameFile = (a, b) => ts.comparePaths(a, b, this.currentDirectory, !this.host.useCaseSensitiveFileNames()) === ts.Comparison.EqualTo;
        if (options.outFile) {
            return isSameFile(filePath, options.outFile) || isSameFile(filePath, ts.removeFileExtension(options.outFile) + ts.Extension.Dts);
        }
        if (options.declarationDir && ts.containsPath(options.declarationDir, filePath, this.currentDirectory, !this.host.useCaseSensitiveFileNames())) {
            return true;
        }
        if (options.outDir) {
            return ts.containsPath(options.outDir, filePath, this.currentDirectory, !this.host.useCaseSensitiveFileNames());
        }
        if (ts.fileExtensionIsOneOf(filePath, ts.supportedJSExtensionsFlat) || ts.isDeclarationFileName(filePath)) {
            const filePathWithoutExtension = ts.removeFileExtension(filePath);
            return !!this.getSourceFileByPath(filePathWithoutExtension + ts.Extension.Ts) || !!this.getSourceFileByPath(filePathWithoutExtension + ts.Extension.Tsx);
        }
        return false;
    }

    // Project references: TS 6.0 parses the referenced configs in JS; tsc-rs builds the program.

    getProjectReferences() {
        return this.projectReferences;
    }

    getResolvedProjectReferences() {
        if (this.resolvedProjectReferences === undefined && this.projectReferences !== undefined) {
            this.projectReferenceRedirects = new Map();
            this.sourceFileToResolvedRef = new Map();
            this.outputFileToResolvedRef = new Map();
            this.resolvedProjectReferences = this.projectReferences.map(reference => this.parseProjectReferenceConfigFile(reference));
        }
        return this.resolvedProjectReferences;
    }

    // TS 6.0: src/compiler/program.ts parseProjectReferenceConfigFile
    parseProjectReferenceConfigFile(reference) {
        const { ts, host } = this;
        const refPath = ts.resolveProjectReferencePath(reference);
        const sourceFilePath = this.toPath(refPath);
        const fromCache = this.projectReferenceRedirects.get(sourceFilePath);
        if (fromCache !== undefined) {
            return fromCache || undefined;
        }
        let commandLine;
        let sourceFile;
        if (host.getParsedCommandLine) {
            commandLine = host.getParsedCommandLine(refPath);
            sourceFile = commandLine?.options.configFile;
        }
        else {
            sourceFile = host.getSourceFile(refPath, ts.ScriptTarget.JSON);
            if (sourceFile !== undefined) {
                const basePath = ts.getNormalizedAbsolutePath(ts.getDirectoryPath(refPath), this.currentDirectory);
                commandLine = ts.parseJsonSourceFileConfigFileContent(sourceFile, ts.parseConfigHostFromCompilerHostLike(host), basePath, undefined, refPath);
            }
        }
        if (commandLine === undefined || sourceFile === undefined) {
            this.projectReferenceRedirects.set(sourceFilePath, false);
            return undefined;
        }
        sourceFile.fileName = refPath;
        sourceFile.path = sourceFilePath;
        sourceFile.resolvedPath = sourceFilePath;
        sourceFile.originalFileName = refPath;
        const resolvedRef = { commandLine, sourceFile };
        this.projectReferenceRedirects.set(sourceFilePath, resolvedRef);
        if (this.options.configFile !== sourceFile) {
            this.mapProjectReferenceFiles(resolvedRef);
        }
        if (commandLine.projectReferences) {
            resolvedRef.references = commandLine.projectReferences.map(child => this.parseProjectReferenceConfigFile(child));
        }
        return resolvedRef;
    }

    mapProjectReferenceFiles(resolvedRef) {
        const { ts } = this;
        const { commandLine } = resolvedRef;
        const ignoreCase = !this.host.useCaseSensitiveFileNames();
        let outDts;
        if (commandLine.options.outFile) {
            outDts = ts.changeExtension(commandLine.options.outFile, ts.Extension.Dts);
            this.outputFileToResolvedRef.set(this.toPath(outDts), { resolvedRef });
        }
        const getCommonSourceDirectory = ts.memoize(() => ts.getCommonSourceDirectoryOfConfig(commandLine, ignoreCase));
        for (const fileName of commandLine.fileNames) {
            let outputDts;
            if (!ts.isDeclarationFileName(fileName) && !ts.fileExtensionIs(fileName, ts.Extension.Json)) {
                if (commandLine.options.outFile) {
                    outputDts = outDts;
                }
                else {
                    outputDts = ts.getOutputDeclarationFileName(fileName, commandLine, ignoreCase, getCommonSourceDirectory);
                    this.outputFileToResolvedRef.set(this.toPath(outputDts), { resolvedRef, source: fileName });
                }
            }
            this.sourceFileToResolvedRef.set(this.toPath(fileName), { resolvedRef, outputDts });
        }
    }

    forEachResolvedProjectReference(callback) {
        return this.ts.forEachResolvedProjectReference(this.getResolvedProjectReferences(), callback);
    }

    getRedirectFromSourceFile(fileName) {
        this.getResolvedProjectReferences();
        return this.sourceFileToResolvedRef?.get(this.toPath(fileName));
    }

    getRedirectFromOutput(path) {
        this.getResolvedProjectReferences();
        return this.outputFileToResolvedRef?.get(path);
    }

    isSourceOfProjectReferenceRedirect(fileName) {
        return this.useSourceOfProjectReferenceRedirect && this.getRedirectFromSourceFile(fileName) !== undefined;
    }

    getResolvedProjectReferenceByPath(projectReferencePath) {
        this.getResolvedProjectReferences();
        return this.projectReferenceRedirects?.get(projectReferencePath) || undefined;
    }

    // Host access, as TS 6.0's program forwards it.

    fileExists(fileName) {
        return this.host.fileExists(fileName);
    }

    readFile(fileName) {
        return this.host.readFile(fileName);
    }

    directoryExists(directoryName) {
        return this.host.directoryExists === undefined ? true : this.host.directoryExists(directoryName);
    }

    realpath(path) {
        return this.host.realpath === undefined ? path : this.host.realpath(path);
    }

    getSymlinkCache() {
        this.symlinkCache ??= this.host.getSymlinkCache?.() ?? this.ts.createSymlinkCache(this.currentDirectory, this.getCanonicalFileName);
        return this.symlinkCache;
    }

    useCaseSensitiveFileNames() {
        return this.host.useCaseSensitiveFileNames();
    }

    // TS 6.0: src/compiler/program.ts writeFile
    writeFile(fileName, text, writeByteOrderMark, onError, sourceFiles, data) {
        this.host.writeFile(fileName, text, writeByteOrderMark, onError, sourceFiles, data);
    }

    getGlobalTypingsCacheLocation() {
        return this.host.getGlobalTypingsCacheLocation?.();
    }
}

// A host gives the same SourceFile objects to many programs (a language service's document registry).
const structureKeys = new WeakMap();

function structureKeyOf(ts, sourceFile) {
    let key = structureKeys.get(sourceFile);
    if (key === undefined) {
        key = structureKey(ts, sourceFile);
        structureKeys.set(sourceFile, key);
    }
    return key;
}

// What TS 6.0's tryReuseStructureFromOldProgram compares between two versions of a file.
function structureKey(ts, sourceFile) {
    const names = nodes => nodes.map(node => node.text).join("\0");
    const references = references => references.map(reference => `${reference.fileName}:${reference.resolutionMode ?? ""}:${reference.preserve ?? ""}`).join("\0");
    return [
        sourceFile.impliedNodeFormat,
        sourceFile.hasNoDefaultLib,
        sourceFile.flags & ts.NodeFlags.PermanentlySetIncrementalFlags,
        references(sourceFile.referencedFiles),
        references(sourceFile.typeReferenceDirectives),
        references(sourceFile.libReferenceDirectives),
        names(sourceFile.imports),
        names(sourceFile.moduleAugmentations),
    ].join("\n");
}

function fileChanges({ changed, created }) {
    return created.length === 0 ? { changed } : { changed, created };
}

// A release after a failed construction: the construction's own exception is the one the caller sees.
function attempt(run) {
    try {
        run();
    }
    catch {
        // Nothing to report it to.
    }
}

function removeItem(list, item) {
    const index = list.lastIndexOf(item);
    if (index !== -1) {
        list.splice(index, 1);
    }
}

// TS 6.0's PackageId leaves peerDependencies undefined when there are none.
function toPackageId(packageId) {
    if (!packageId) {
        return undefined;
    }
    const { name, subModuleName, version, peerDependencies } = packageId;
    return { name, subModuleName, version, peerDependencies: peerDependencies || undefined };
}

function diagnosticKey(diagnostic) {
    const text = typeof diagnostic.messageText === "string" ? diagnostic.messageText : diagnostic.messageText.messageText;
    return `${diagnostic.code}\0${text}`;
}

const programMethods = Object.getOwnPropertyNames(NativeProgram.prototype).filter(name => name !== "constructor" && typeof Object.getOwnPropertyDescriptor(NativeProgram.prototype, name).value === "function");

// TS 6.0: src/compiler/program.ts createSyntheticImport
function createSyntheticImport(ts, text, file) {
    const literal = ts.factory.createStringLiteral(text);
    const declaration = ts.factory.createImportDeclaration(undefined, undefined, literal);
    ts.addInternalEmitFlags(declaration, ts.InternalEmitFlags.NeverApplyImportHelper);
    ts.setParent(literal, declaration);
    ts.setParent(declaration, file);
    literal.flags &= ~ts.NodeFlags.Synthesized;
    declaration.flags &= ~ts.NodeFlags.Synthesized;
    return literal;
}

// TS 6.0: src/compiler/program.ts shouldTransformImportCallWorker
function shouldTransformImportCall(ts, sourceFile, options) {
    const moduleKind = ts.getEmitModuleKind(options);
    if ((ts.ModuleKind.Node16 <= moduleKind && moduleKind <= ts.ModuleKind.NodeNext) || moduleKind === ts.ModuleKind.Preserve) {
        return false;
    }
    return ts.getEmitModuleFormatOfFileWorker(sourceFile, options) < ts.ModuleKind.ES2015;
}

// TS 6.0: src/compiler/program.ts getEmitSyntaxForUsageLocationWorker
function getEmitSyntaxForUsageLocation(ts, file, usage, options) {
    const exprParentParent = ts.walkUpParenthesizedExpressions(usage.parent)?.parent;
    if ((exprParentParent && ts.isImportEqualsDeclaration(exprParentParent)) || ts.isRequireCall(usage.parent, false)) {
        return ts.ModuleKind.CommonJS;
    }
    if (ts.isImportCall(ts.walkUpParenthesizedExpressions(usage.parent))) {
        return shouldTransformImportCall(ts, file, options) ? ts.ModuleKind.CommonJS : ts.ModuleKind.ESNext;
    }
    const fileEmitMode = ts.getEmitModuleFormatOfFileWorker(file, options);
    if (fileEmitMode === ts.ModuleKind.CommonJS) {
        return ts.ModuleKind.CommonJS;
    }
    return ts.emitModuleKindIsNonNodeESM(fileEmitMode) || fileEmitMode === ts.ModuleKind.Preserve ? ts.ModuleKind.ESNext : undefined;
}

// The members of TS 6.0's TypeChecker (src/compiler/checker.ts createTypeChecker). Those that
// ./checker.js does not provide answer from the program or throw a "not supported" error, so that
// TS 6.0 code that needs the TS 6.0 checker (language service quick info, completions, find
// references, ...) stops with a clear message.
const typeCheckerMembers = "containsArgumentsReference createAnonymousType createArrayType createIndexInfo createPromiseType createSignature createSymbol fillMissingTypeArguments forEachExportAndPropertyOfModule getAccessibleSymbolChain getAliasedSymbol getAllPossiblePropertiesOfTypes getAmbientModules getAnyAsyncIterableType getAnyType getApparentType getAugmentedPropertiesOfType getAwaitedType getBaseConstraintOfType getBaseTypeOfLiteralType getBaseTypes getBigIntLiteralType getBigIntType getBooleanType getCandidateSignaturesForStringLiteralCompletions getConstantValue getContextualType getContextualTypeForArgumentAtIndex getContextualTypeForJsxAttribute getContextualTypeForObjectLiteralElement getDeclaredTypeOfSymbol getDefaultFromTypeParameter getDiagnostics getESSymbolType getElementTypeOfArrayType getEmitResolver getExactOptionalProperties getExpandedParameters getExportSpecifierLocalTargetSymbol getExportSymbolOfSymbol getExportsAndPropertiesOfModule getExportsOfModule getFalseType getFullyQualifiedName getGlobalDiagnostics getIdentifierCount getImmediateAliasedSymbol getIndexInfoOfType getIndexInfosAtLocation getIndexInfosOfIndexSymbol getIndexInfosOfType getIndexType getIndexTypeOfType getInstantiationCount getJsxFragmentFactory getJsxIntrinsicTagNamesAt getJsxNamespace getLocalTypeParametersOfClassOrInterfaceOrTypeAlias getMemberOverrideModifierStatus getMergedSymbol getNeverType getNodeCount getNonNullableType getNonOptionalType getNonPrimitiveType getNullType getNullableType getNumberLiteralType getNumberType getOptionalType getParameterIdentifierInfoAtPosition getParameterType getPrivateIdentifierPropertyOfType getPromiseLikeType getPromiseType getPromisedTypeOfPromise getPropertiesOfType getPropertyOfType getPropertySymbolOfDestructuringAssignment getRecursionIdentity getRelationCacheSizes getResolvedSignature getResolvedSignatureForSignatureHelp getReturnTypeOfSignature getRootSymbols getShorthandAssignmentValueSymbol getSignatureFromDeclaration getSignaturesOfType getStringLiteralType getStringType getSuggestedSymbolForNonexistentClassMember getSuggestedSymbolForNonexistentJSXAttribute getSuggestedSymbolForNonexistentModule getSuggestedSymbolForNonexistentProperty getSuggestedSymbolForNonexistentSymbol getSuggestionDiagnostics getSymbolAtLocation getSymbolCount getSymbolFlags getSymbolOfExpando getSymbolWalker getSymbolsInScope getSymbolsOfParameterPropertyDeclaration getTrueType getTypeArgumentConstraint getTypeArguments getTypeArgumentsForResolvedSignature getTypeAtLocation getTypeCount getTypeFromTypeNode getTypeOfAssignmentPattern getTypeOfPropertyOfContextualType getTypeOfPropertyOfType getTypeOfSymbol getTypeOfSymbolAtLocation getTypeOnlyAliasDeclaration getTypePredicateOfSignature getUndefinedType getUnionType getUnknownType getUnmatchedProperties getVoidType getWidenedLiteralType getWidenedType hasEffectiveRestParameter indexInfoToIndexSignatureDeclaration isArgumentsSymbol isArrayLikeType isArrayType isContextSensitive isDeclarationVisible isEmptyAnonymousObjectType isImplementationOfOverload isLibType isNullableType isOptionalParameter isPropertyAccessible isSymbolAccessible isTupleType isTypeAssignableTo isTypeInvalidDueToUnionDiscriminant isTypeParameterPossiblyReferenced isUndefinedSymbol isUnknownSymbol isValidPropertyAccess isValidPropertyAccessForCompletions requiresAddingImplicitUndefined resolveExternalModuleName resolveExternalModuleSymbol resolveName runWithCancellationToken signatureToSignatureDeclaration signatureToString symbolIsValue symbolToEntityName symbolToExpression symbolToNode symbolToParameterDeclaration symbolToString symbolToTypeParameterDeclarations tryFindAmbientModule tryGetMemberInModuleExports tryGetMemberInModuleExportsAndProperties tryGetThisTypeAt typeHasCallOrConstructSignatures typeParameterToDeclaration typePredicateToString typePredicateToTypePredicateNode typeToString typeToTypeNode writeSignature writeSymbol writeType writeTypePredicate".split(" ");

function completeChecker(program, checker) {
    const fromProgram = {
        getNodeCount: () => program.getNodeCount(),
        getIdentifierCount: () => program.getIdentifierCount(),
        getSymbolCount: () => program.getSymbolCount(),
        getTypeCount: () => program.getTypeCount(),
        getInstantiationCount: () => program.getInstantiationCount(),
        getRelationCacheSizes: () => program.getRelationCacheSizes(),
        getDiagnostics: (sourceFile, cancellationToken) => program.getSemanticDiagnostics(sourceFile, cancellationToken),
        getGlobalDiagnostics: () => program.getGlobalDiagnostics(),
        getSuggestionDiagnostics: (sourceFile, cancellationToken) => program.getSuggestionDiagnostics(sourceFile, cancellationToken),
    };
    for (const name of typeCheckerMembers) {
        if (name in checker) {
            continue;
        }
        checker[name] = fromProgram[name] ?? (() => {
            throw new Error(`tsc-rs: checker.${name} is not supported: it needs the TypeScript 6.0 checker, which does not run here`);
        });
    }
    const cancellable = new WeakMap();
    // TS 6.0: src/compiler/checker.ts runWithCancellationToken. TS 6.0's checker polls the token
    // while it checks and builds type nodes; here each checker call polls it.
    checker.runWithCancellationToken = (cancellationToken, callback) => {
        if (cancellationToken === undefined) {
            return callback(checker);
        }
        let view = cancellable.get(cancellationToken);
        if (view === undefined) {
            view = cancellableChecker(checker, cancellationToken);
            cancellable.set(cancellationToken, view);
        }
        return callback(view);
    };
    return checker;
}

function cancellableChecker(checker, cancellationToken) {
    const view = {};
    for (const [name, member] of Object.entries(checker)) {
        view[name] = typeof member !== "function" || name === "runWithCancellationToken" ? member : (...args) => {
            cancellationToken.throwIfCancellationRequested();
            return member(...args);
        };
    }
    return view;
}

// Language service features whose TS 6.0 implementation reads the TS 6.0 checker's internals
// (src/services/services.ts). Where tsc-rs's checker cannot answer them, they fail with a
// "not supported" error instead of a TypeError from deep inside TS 6.0's services.
const checkerFeatures = [
    "applyCodeActionCommand",
    "findReferences",
    "findRenameLocations",
    "getApplicableRefactors",
    "getCombinedCodeFix",
    "getCodeFixesAtPosition",
    "getCompletionEntryDetails",
    "getCompletionEntrySymbol",
    "getCompletionsAtPosition",
    "getDefinitionAndBoundSpan",
    "getDefinitionAtPosition",
    "getDocumentHighlights",
    "getEditsForFileRename",
    "getEditsForRefactor",
    "getEncodedSemanticClassifications",
    "getFileReferences",
    "getImplementationAtPosition",
    "getInlayHints",
    "getMoveToRefactoringFileSuggestions",
    "getNavigateToItems",
    "getQuickInfoAtPosition",
    "getReferencesAtPosition",
    "getRenameInfo",
    "getSemanticClassifications",
    "getSignatureHelpItems",
    "getTypeDefinitionAtPosition",
    "organizeImports",
    "prepareCallHierarchy",
    "provideCallHierarchyIncomingCalls",
    "provideCallHierarchyOutgoingCalls",
];

function wrapLanguageService(ts, service, host) {
    if (service.tsrsCheckerFeatures) {
        return service;
    }
    for (const name of checkerFeatures) {
        const feature = service[name];
        if (typeof feature !== "function") {
            continue;
        }
        service[name] = (...args) => {
            try {
                return feature(...args);
            }
            catch (error) {
                throwIfCancelled(ts, host, error);
                throw notSupportedFeature(name, error);
            }
        };
    }
    service.tsrsCheckerFeatures = true;
    return service;
}

// TS 6.0 stops a cancelled request with OperationCanceledException at the checker's next poll of
// the token, before a feature could fail on what tsc-rs's checker does not give.
function throwIfCancelled(ts, host, error) {
    if (error instanceof ts.OperationCanceledException) {
        return;
    }
    if (host?.getCancellationToken?.()?.isCancellationRequested()) {
        throw new ts.OperationCanceledException();
    }
}

function notSupportedFeature(name, error) {
    const obscure = error instanceof TypeError || (error instanceof Error && error.message.startsWith("Debug Failure."));
    if (!obscure) {
        return error;
    }
    return new Error(`tsc-rs: languageService.${name} is not supported here: it needs the TypeScript 6.0 checker (${error.message})`, { cause: error });
}

// Nodes that tsc-rs parses as another kind over the same range: the types
// of `implements` and interface `extends` clauses are TypeReferences, `A.B` in them a
// QualifiedName; a hole in an array binding pattern is a BindingElement.
let shapes;

function nativeShapes(ts) {
    if (shapes === undefined) {
        const { SyntaxKind } = ts;
        const pairs = [
            [SyntaxKind.ExpressionWithTypeArguments, SyntaxKind.TypeReference],
            [SyntaxKind.PropertyAccessExpression, SyntaxKind.QualifiedName],
            [SyntaxKind.OmittedExpression, SyntaxKind.BindingElement],
        ];
        shapes = { toNative: new Map(pairs), toJs: new Map(pairs.map(([js, native]) => [native, js])) };
    }
    return shapes;
}

// The node of `sourceFile` with this kind and range, searched from the top.
function findNode(ts, sourceFile, kind, pos, end) {
    let found;
    const visit = node => {
        if (found !== undefined || node.pos > pos || node.end < end) {
            return;
        }
        if (node.kind === kind && node.pos === pos && node.end === end) {
            found = node;
            return;
        }
        for (const jsDoc of node.jsDoc ?? ts.emptyArray) {
            visit(jsDoc);
        }
        ts.forEachChild(node, visit, nodes => nodes.forEach(visit));
    };
    visit(sourceFile);
    return found;
}

function install(ts, hooks, originals) {
    const host = require("./host.js");
    host.installServerProjectTracking(ts);
    hooks.createProgram = (...args) => createProgram(ts, ...args);
    hooks.createCompilerHostWorker = (...args) => host.createCompilerHostWorker(ts, originals.createCompilerHostWorker, ...args);
    hooks.createTypeChecker = () => {
        throw new Error("tsc-rs: the TypeScript 6.0 type checker does not run here; programs and checkers come from tsc-rs");
    };
    if (originals.createLanguageService !== undefined) {
        hooks.createLanguageService = (host, ...args) => wrapLanguageService(ts, originals.createLanguageService(host, ...args), host);
        return;
    }
    // Without that hook point, at least the language services of ts.server projects.
    const project = ts.server?.Project?.prototype;
    if (project !== undefined) {
        const getLanguageService = project.getLanguageService;
        project.getLanguageService = function getWrappedLanguageService(...args) {
            const service = getLanguageService.apply(this, args);
            return service === undefined ? service : wrapLanguageService(ts, service, this);
        };
    }
}

module.exports = { createProgram, install, NativeProgram, programOfSourceFile };
