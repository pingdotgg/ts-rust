"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { useOSAnswer } = require("./session.js");

// Compiler hosts made by ts.createCompilerHost over ts.sys, with the methods they were made with.
// tsc-rs reads the disk itself for those (it gives the same answers, faster). A host that gets its
// own files or its own module resolver afterwards answers like any other host.
const defaultHosts = new WeakMap();
const hostMethods = ["getSourceFile", "getSourceFileByPath", "readFile", "fileExists", "directoryExists", "getDirectories", "realpath", "resolveModuleNames", "resolveModuleNameLiterals"];
const bundledPrefix = "bundled:///";
const bundledLibs = "bundled:///libs/";
const bundledLibTexts = new Map();

// The ts.server projects whose updateGraph is running, innermost last, and the module resolver
// their class has (TS 6.0's resolution cache).
const serverProjects = [];
let serverProjectResolver;

function createCompilerHostWorker(ts, original, options, setParentNodes, system = ts.sys) {
    const host = original(options, setParentNodes, system);
    if (system === ts.sys) {
        defaultHosts.set(host, hostMethods.map(name => host[name]));
    }
    return host;
}

function isDefaultHost(host) {
    const methods = defaultHosts.get(host);
    return methods !== undefined && hostMethods.every((name, index) => host[name] === methods[index]);
}

// ts.server projects create their programs inside updateGraph. Their module resolution is TS 6.0's
// resolution cache, which tsc-rs's own resolution replaces (see usesHostModuleResolution).
function installServerProjectTracking(ts) {
    const project = ts.server?.Project?.prototype;
    if (project === undefined || project.updateGraph.tsrsTracked) {
        return;
    }
    const updateGraph = project.updateGraph;
    serverProjectResolver = project.resolveModuleNameLiterals;
    project.updateGraph = function updateGraphTracked(...args) {
        serverProjects.push(this);
        try {
            return updateGraph.apply(this, args);
        }
        finally {
            serverProjects.pop();
        }
    };
    project.updateGraph.tsrsTracked = true;
}

// Whether the host's own module resolver answers tsc-rs's module resolution. TS 6.0's resolution
// cache (ts.server projects, watch programs) only memoizes TS 6.0's standard resolution, so those
// programs resolve with tsc-rs; a custom resolver (ts-loader, webpack aliases, a language service
// plugin's) is called.
function usesHostModuleResolution(host) {
    if (host.resolveModuleNames !== undefined) {
        return true;
    }
    if (host.resolveModuleNameLiterals === undefined) {
        return false;
    }
    // TS 6.0: src/services/services.ts calls the project's resolveModuleNameLiterals, which a plugin
    // may replace (on the project or on the class) to resolve names of its own.
    if (serverProjects.length !== 0) {
        return serverProjects.at(-1).resolveModuleNameLiterals !== serverProjectResolver;
    }
    // TS 6.0: src/compiler/watchPublic.ts createWatchProgram binds the resolution cache to a ResolutionCacheHost.
    return typeof host.watchDirectoryOfFailedLookupLocation !== "function";
}

// The text tsc-rs compiles for a bundled lib file (lib/ holds the same files), or undefined.
function bundledLibText(nativeFileName) {
    const name = nativeFileName.slice(bundledLibs.length);
    if (!bundledLibTexts.has(name)) {
        const fileName = path.join(__dirname, "..", "lib", name);
        bundledLibTexts.set(name, fs.existsSync(fileName) ? fs.readFileSync(fileName, "utf8") : undefined);
    }
    return bundledLibTexts.get(name);
}

// The file system that tsc-rs sees while it builds `program`: undefined means the operating system.
function fileSystemFor(ts, program) {
    const { host } = program;
    if (program.usesDefaultHost) {
        return undefined;
    }
    // tsc-rs asks with absolute names; the host gets the names TS 6.0 would use.
    const name = path => program.jsNameOf(path);
    // What tsc-rs found missing, so that a later program can tell it which of those appeared.
    const checked = (path, exists, isDirectory) => {
        if (!exists) {
            program.noteMissing(path, isDirectory);
        }
        return exists;
    };
    const read = (path, text) => {
        if (typeof text !== "string") {
            program.noteMissing(path, false);
        }
        return text;
    };
    return {
        readFile(path) {
            if (path.startsWith(bundledPrefix)) {
                return useOSAnswer;
            }
            if (host.getSourceFile !== undefined && (isSourceFileName(ts, path) || program.rootNativeToJs.has(path))) {
                return read(path, program.adoptHostSourceFile(path)?.text);
            }
            return read(path, host.readFile(name(path)));
        },
        fileExists(path) {
            if (path.startsWith(bundledPrefix)) {
                return useOSAnswer;
            }
            // TS 6.0 asks the host for root files with getSourceFile only, not fileExists.
            if (program.rootNativeToJs.has(path) && host.getSourceFile !== undefined) {
                return checked(path, program.adoptHostSourceFile(path) !== undefined, false);
            }
            return checked(path, !!host.fileExists(name(path)), false);
        },
        directoryExists(path) {
            if (path.startsWith(bundledPrefix)) {
                return useOSAnswer;
            }
            // TS 6.0: src/compiler/moduleNameResolver.ts directoryProbablyExists
            return host.directoryExists === undefined ? true : checked(path, !!host.directoryExists(name(path)), true);
        },
        getAccessibleEntries(path) {
            if (path.startsWith(bundledPrefix) || host.getDirectories === undefined) {
                return useOSAnswer;
            }
            const files = host.readDirectory === undefined
                ? []
                : host.readDirectory(name(path), undefined, undefined, undefined, 1).map(fileName => ts.getBaseFileName(fileName));
            return { files, directories: host.getDirectories(name(path)) };
        },
        realpath(path) {
            if (path.startsWith(bundledPrefix)) {
                return useOSAnswer;
            }
            if (host.realpath === undefined) {
                return path;
            }
            return ts.getNormalizedAbsolutePath(host.realpath(name(path)), program.nativeDirectory);
        },
        stat(path) {
            if (path.startsWith(bundledPrefix)) {
                return useOSAnswer;
            }
            if (host.fileExists(name(path)) || (program.rootNativeToJs.has(path) && program.adoptHostSourceFile(path) !== undefined)) {
                return { mode: 0o100644, size: 0, mtime: new Date(0) };
            }
            if (host.directoryExists?.(name(path))) {
                return { mode: 0o40755, size: 0, mtime: new Date(0) };
            }
            program.noteMissing(path, false);
            return undefined;
        },
    };
}

function isSourceFileName(ts, path) {
    return ts.getBaseFileName(path) !== "package.json"
        && ts.fileExtensionIsOneOf(path, ts.supportedTSExtensionsFlat.concat(ts.supportedJSExtensionsFlat, [".json"]));
}

// The host's resolveModuleNames / resolveModuleNameLiterals, called the way TS 6.0's program calls
// them (once per file, with all its module names), answer tsc-rs's module resolution through a
// native module resolver callback. tsc-rs asks per directory, so a file of that directory that
// imports the name answers; the files tsc-rs did not ask about are resolved at the end.
class HostModuleResolver {
    constructor(program) {
        this.program = program;
        // Directory path -> native names of the host files in it.
        this.filesByDirectory = new Map();
        // tsc-rs resolves while it builds the program's snapshot. The session keeps the callback, which
        // must not keep the program alive after that (a WeakRef would until the end of the job).
        const target = { resolver: this };
        this.target = target;
        this.callbackName = program.session.registerCallback("resolveModuleName", params => target.resolver?.resolveForNative(params) ?? null);
        try {
            this.id = program.session.request("createModuleResolver", { compilerOptions: program.nativeOptions, resolveModuleNameCallback: this.callbackName });
        }
        catch (error) {
            program.session.unregisterCallback(this.callbackName);
            throw error;
        }
    }

    addFile(nativeFileName, fileName) {
        const directory = this.program.toPath(this.program.ts.getDirectoryPath(fileName));
        let files = this.filesByDirectory.get(directory);
        if (files === undefined) {
            files = [];
            this.filesByDirectory.set(directory, files);
        }
        files.push(nativeFileName);
    }

    resolveForNative({ moduleName, containingDirectory, resolutionMode }) {
        const { program } = this;
        const { ModuleKind } = program.ts;
        const mode = resolutionMode === 0 ? undefined : resolutionMode;
        let otherMode;
        for (const nativeFileName of this.filesByDirectory.get(program.toPath(containingDirectory)) ?? []) {
            const sourceFile = program.hostSourceFiles.get(nativeFileName);
            if (sourceFile === undefined || !program.moduleNameLiterals(sourceFile).some(literal => literal.text === moduleName)) {
                continue;
            }
            const resolutions = this.resolveFile(nativeFileName);
            if (resolutions.has(moduleName, mode)) {
                return this.toNative(resolutions.get(moduleName, mode));
            }
            otherMode ??= [undefined, ModuleKind.CommonJS, ModuleKind.ESNext].map(other => resolutions.get(moduleName, other)).find(found => found !== undefined);
        }
        if (otherMode !== undefined) {
            return this.toNative(otherMode);
        }
        return this.toNative(this.resolveOutsideFiles(moduleName, containingDirectory, mode));
    }

    // TS 6.0: src/compiler/program.ts resolveModuleNamesWorker
    resolveFile(nativeFileName) {
        const { program } = this;
        const { ts, options } = program;
        const existing = program.moduleResolutions.get(nativeFileName);
        if (existing !== undefined) {
            return existing;
        }
        const sourceFile = program.hostSourceFiles.get(nativeFileName);
        const resolutions = ts.createModeAwareCache();
        program.moduleResolutions.set(nativeFileName, resolutions);
        const literals = program.moduleNameLiterals(sourceFile);
        if (literals.length === 0) {
            return resolutions;
        }
        const containingFile = ts.getNormalizedAbsolutePath(sourceFile.originalFileName, program.currentDirectory);
        // TS 6.0: src/compiler/program.ts resolveNamesReusingOldState resolves a name that the file
        // itself declares as an ambient module to that module, without asking the host.
        const ownAmbient = name => sourceFile.ambientModuleNames.includes(name);
        const unknown = literals.filter(literal => !ownAmbient(literal.text));
        const results = unknown.length === 0 ? [] : this.callHost(unknown, containingFile, sourceFile);
        let next = 0;
        for (const literal of literals) {
            const resolution = ownAmbient(literal.text) ? emptyResolution : results[next++] ?? emptyResolution;
            resolutions.set(literal.text, ts.getModeForUsageLocation(sourceFile, literal, options), resolution);
        }
        return resolutions;
    }

    // TS 6.0: src/compiler/program.ts createProgram, actualResolveModuleNamesWorker
    callHost(literals, containingFile, containingSourceFile) {
        const { ts, host, options } = this.program;
        if (host.resolveModuleNameLiterals !== undefined) {
            return host.resolveModuleNameLiterals(literals, containingFile, undefined, options, containingSourceFile, undefined);
        }
        return host.resolveModuleNames(literals.map(literal => literal.text), containingFile, undefined, undefined, options, containingSourceFile).map(resolved => {
            if (resolved === undefined) {
                return emptyResolution;
            }
            return resolved.extension === undefined
                ? { resolvedModule: { ...resolved, extension: ts.extensionFromPath(resolved.resolvedFileName) } }
                : { resolvedModule: resolved };
        });
    }

    // A module name that no program file of the directory imports (a helper import of tsc-rs).
    resolveOutsideFiles(moduleName, containingDirectory, mode) {
        const { ts, options } = this.program;
        const containingFile = ts.combinePaths(containingDirectory, "__tsrs_resolution__.ts");
        const sourceFile = ts.createSourceFile(containingFile, "", ts.getEmitScriptTarget(options));
        sourceFile.impliedNodeFormat = mode;
        const literal = ts.factory.createStringLiteral(moduleName);
        const declaration = ts.factory.createImportDeclaration(undefined, undefined, literal);
        ts.setParent(literal, declaration);
        ts.setParent(declaration, sourceFile);
        return this.callHost([literal], containingFile, sourceFile)[0];
    }

    toNative(resolution) {
        const resolved = resolution?.resolvedModule;
        if (resolved === undefined) {
            return null;
        }
        const { ts, nativeDirectory } = this.program;
        return {
            resolvedFileName: ts.getNormalizedAbsolutePath(resolved.resolvedFileName, nativeDirectory),
            originalPath: resolved.originalPath === undefined ? undefined : ts.getNormalizedAbsolutePath(resolved.originalPath, nativeDirectory),
            packageId: resolved.packageId,
        };
    }

    // Resolves the files tsc-rs did not ask about, so that the host sees every file once.
    // tsc-rs resolves again when a later request rebuilds the program (the emit of files that custom
    // transformers changed): the host answers those resolutions too while `run` runs.
    whileResolving(run) {
        const { program } = this;
        for (const [nativeFileName, sourceFile] of program.hostSourceFiles) {
            if (sourceFile !== undefined) {
                this.addFile(nativeFileName, program.toJsFileName(nativeFileName));
            }
        }
        this.target.resolver = this;
        try {
            return run();
        }
        finally {
            this.filesByDirectory.clear();
            this.target.resolver = undefined;
        }
    }

    finish(nativeFileNames) {
        for (const nativeFileName of nativeFileNames) {
            if (this.program.hostSourceFiles.has(nativeFileName) && !this.program.moduleResolutions.has(nativeFileName)) {
                this.resolveFile(nativeFileName);
            }
        }
        this.filesByDirectory.clear();
        this.target.resolver = undefined;
    }

    // What releasing the resolver needs, without the program.
    nativeHandle() {
        return { id: this.id, callbackName: this.callbackName };
    }

    release() {
        const { session } = this.program;
        session.unregisterCallback(this.callbackName);
        session.request("releaseModuleResolver", { resolver: this.id });
    }
}

const emptyResolution = Object.freeze({ resolvedModule: undefined });

module.exports = {
    bundledLibText,
    createCompilerHostWorker,
    fileSystemFor,
    HostModuleResolver,
    installServerProjectTracking,
    isDefaultHost,
    usesHostModuleResolution,
};
