"use strict";

const { getSession } = require("./session.js");
const { toNativeOptions, toTsOptionDiagnostics } = require("./options.js");
const { transformOutputs } = require("./transformers.js");

// TS 6.0: src/services/transpile.ts optionsRedundantWithVerbatimModuleSyntax
const optionsRedundantWithVerbatimModuleSyntax = new Set(["isolatedModules"]);
const transpileDirectory = "/";
const byteOrderMark = "\uFEFF";

// TS 6.0: src/services/transpile.ts transpileWorker (transpileModule, transpileDeclaration and
// transpile all come here). tsc-rs transpiles the text (its transpileModule and
// transpileDeclaration requests); custom transformers run as in a program emit.
function transpileWorker(ts, input, transpileOptions, declaration) {
    const diagnostics = [];
    const options = transpileOptions.compilerOptions ? ts.fixupCompilerOptions(transpileOptions.compilerOptions, diagnostics) : {};
    const defaultOptions = ts.getDefaultCompilerOptions();
    for (const key in defaultOptions) {
        if (ts.hasProperty(defaultOptions, key) && options[key] === undefined) {
            options[key] = defaultOptions[key];
        }
    }
    for (const option of ts.transpileOptionValueCompilerOptions) {
        if (options.verbatimModuleSyntax && optionsRedundantWithVerbatimModuleSyntax.has(option.name)) {
            continue;
        }
        options[option.name] = option.transpileOptionValue;
    }
    options.suppressOutputPathCheck = true;
    options.allowNonTsExtensions = true;
    if (declaration) {
        options.declaration = true;
        options.emitDeclarationOnly = true;
        options.isolatedDeclarations = true;
    }
    else {
        options.declaration = false;
        options.declarationMap = false;
    }
    options.noLib = !declaration;

    const inputFileName = transpileOptions.fileName || (transpileOptions.compilerOptions && transpileOptions.compilerOptions.jsx ? "module.tsx" : "module.ts");
    let sourceFile;
    const getSourceFile = () => sourceFile ??= createInputSourceFile(ts, input, inputFileName, options, transpileOptions);
    // tsc-rs roots the input file at "/" (TS 7: internal/transpile/transpile.go inputDirectory);
    // relative path options go there too, where TS 6.0 keeps both relative to "".
    const native = toNativeOptions(ts, options, { currentDirectory: transpileDirectory });
    const request = text => getSession(ts).request(declaration ? "transpileDeclaration" : "transpileModule", {
        input: text,
        options: { compilerOptions: native.options, fileName: inputFileName, reportDiagnostics: !!transpileOptions.reportDiagnostics },
    });
    const response = request(input);
    // TS 6.0's order: syntax errors, option errors, then what the emit reported.
    const reported = toTsOptionDiagnostics(ts, options, response.diagnostics ?? [], () => getSourceFile());
    diagnostics.push(...reported.filter(diagnostic => diagnostic.file !== undefined));
    if (transpileOptions.reportDiagnostics) {
        diagnostics.push(...native.diagnostics);
    }
    diagnostics.push(...reported.filter(diagnostic => diagnostic.file === undefined));

    const outputFileName = getOutputFileName(ts, inputFileName, options, declaration);
    const outputs = [{ fileName: outputFileName, text: response.outputText }];
    if (response.sourceMapText) {
        outputs.push({ fileName: `${outputFileName}.map`, text: response.sourceMapText });
    }
    const transformers = transformersFor(ts, transpileOptions, options, declaration);
    if (transformers !== undefined) {
        const file = { sourceFile: getSourceFile(), nativeFileName: inputFileName, outputs };
        transformOutputs(ts, {
            options,
            emitHost: createTranspileEmitHost(ts, options, getSourceFile()),
            files: [file],
            customTransformers: transformers,
            directory: transpileDirectory,
            reemit: texts => {
                const reemitted = request(texts.get(inputFileName));
                const list = [{ fileName: outputFileName, text: reemitted.outputText }];
                if (reemitted.sourceMapText) {
                    list.push({ fileName: `${outputFileName}.map`, text: reemitted.sourceMapText });
                }
                return new Map([[inputFileName, list]]);
            },
        });
        for (const scriptDiagnostic of [...file.scriptDiagnostics ?? [], ...file.declarationDiagnostics ?? []]) {
            diagnostics.push(scriptDiagnostic);
        }
        outputs.splice(0, outputs.length, ...file.outputs);
    }
    const output = outputs.find(candidate => !candidate.fileName.endsWith(".map"));
    const map = outputs.find(candidate => candidate.fileName.endsWith(".map"));
    if (declaration && transpileOptions.moduleName) {
        addAmdModuleDirective(ts, output, map, transpileOptions.moduleName, options);
    }
    // TS 6.0's transpile writeFile drops the byte order mark that emitBOM asks the host to write.
    const outputText = output.text.startsWith(byteOrderMark) ? output.text.slice(1) : output.text;
    return { outputText, diagnostics, sourceMapText: map?.text };
}

// TS 6.0: src/compiler/emitter.ts emitTripleSlashDirectives writes the module name of a
// declaration file first; tsc-rs has no module name.
function addAmdModuleDirective(ts, output, map, moduleName, options) {
    output.text = `/// <amd-module name="${moduleName}" />${ts.getNewLineCharacter(options)}${output.text}`;
    if (map !== undefined) {
        const json = JSON.parse(map.text);
        map.text = JSON.stringify({ ...json, mappings: `;${json.mappings}` });
    }
}

// TS 6.0: src/services/transpile.ts transpileWorker creates the program's only source file so.
function createInputSourceFile(ts, input, inputFileName, options, transpileOptions) {
    const getCanonicalFileName = fileName => fileName;
    const host = {
        useCaseSensitiveFileNames: () => false,
        getCurrentDirectory: () => "",
        fileExists: fileName => fileName === inputFileName,
        readFile: () => "",
        directoryExists: () => true,
        getDirectories: () => [],
    };
    const sourceFile = ts.createSourceFile(
        inputFileName,
        input,
        {
            languageVersion: ts.getEmitScriptTarget(options),
            impliedNodeFormat: ts.getImpliedNodeFormatForFile(ts.toPath(inputFileName, "", getCanonicalFileName), /*packageJsonInfoCache*/ undefined, host, options),
            setExternalModuleIndicator: ts.getSetExternalModuleIndicator(options),
            jsDocParsingMode: transpileOptions.jsDocParsingMode ?? ts.JSDocParsingMode.ParseAll,
        },
        // TS 6.0's binder sets the parents before its emit runs the transformers.
        /*setParentNodes*/ true,
    );
    if (transpileOptions.moduleName) {
        sourceFile.moduleName = transpileOptions.moduleName;
    }
    if (transpileOptions.renamedDependencies) {
        sourceFile.renamedDependencies = new Map(Object.entries(transpileOptions.renamedDependencies));
    }
    return sourceFile;
}

// The custom transformers of the emit, plus the module renames of `renamedDependencies`, which
// TS 6.0's CommonJS module transform applies (src/compiler/factory/utilities.ts
// tryRenameExternalModule) and tsc-rs's transpile has no option for.
function transformersFor(ts, transpileOptions, options, declaration) {
    const custom = transpileOptions.transformers ?? {};
    const before = declaration ? [] : [...custom.before ?? []];
    const after = declaration ? [] : [...custom.after ?? []];
    const afterDeclarations = [...custom.afterDeclarations ?? []];
    const renames = transpileOptions.renamedDependencies;
    if (!declaration && renames !== undefined && Object.keys(renames).length !== 0) {
        after.unshift(renameDependencies(ts, new Map(Object.entries(renames))));
    }
    if (before.length === 0 && after.length === 0 && afterDeclarations.length === 0) {
        return undefined;
    }
    return { before, after, afterDeclarations };
}

// TS 6.0 renames the module of each import and export as its CommonJS module transform writes the
// require call, and keeps the names it generated from the original module name; here the require
// calls of module-level code in tsc-rs's output are renamed.
function renameDependencies(ts, renames) {
    return context => sourceFile => {
        const { factory } = context;
        const visit = node => {
            if (ts.isFunctionLike(node) || ts.isClassLike(node)) {
                return node;
            }
            if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === "require"
                && node.arguments.length === 1 && ts.isStringLiteral(node.arguments[0]) && renames.has(node.arguments[0].text)) {
                return factory.updateCallExpression(node, node.expression, node.typeArguments, [factory.createStringLiteral(renames.get(node.arguments[0].text))]);
            }
            return ts.visitEachChild(node, visit, context);
        };
        return ts.visitEachChild(sourceFile, visit, context);
    };
}

// The output name only tells the transformers which kind of text they get.
function getOutputFileName(ts, inputFileName, options, declaration) {
    if (declaration) {
        return ts.getDeclarationEmitOutputFilePathWorker(inputFileName, options, /*host*/ { getCurrentDirectory: () => "", getCommonSourceDirectory: () => "", getCanonicalFileName: fileName => fileName });
    }
    return ts.changeExtension(inputFileName, ts.getOutputExtension(inputFileName, options));
}

// TS 6.0: src/services/transpile.ts transpileWorker's program, as an emit host for
// transformation contexts.
function createTranspileEmitHost(ts, options, sourceFile) {
    return {
        getCanonicalFileName: fileName => fileName,
        getCommonSourceDirectory: () => "",
        getCompilerOptions: () => options,
        getCurrentDirectory: () => "",
        getSourceFile: fileName => fileName === sourceFile.fileName ? sourceFile : undefined,
        getSourceFileByPath: path => path === sourceFile.path ? sourceFile : undefined,
        getSourceFiles: () => [sourceFile],
        isSourceFileFromExternalLibrary: () => false,
        getRedirectFromSourceFile: () => undefined,
        isSourceOfProjectReferenceRedirect: () => false,
        writeFile: () => undefined,
        isEmitBlocked: () => false,
        getEmitModuleFormatOfFile: file => ts.getEmitModuleFormatOfFileWorker(file, options),
        getDefaultResolutionModeForFile: file => ts.getDefaultResolutionModeForFileWorker(file, options),
        getModeForResolutionAtIndex: () => undefined,
        readFile: () => "",
        fileExists: fileName => fileName === sourceFile.fileName,
        useCaseSensitiveFileNames: () => false,
        getBuildInfo: () => undefined,
        redirectTargetsMap: new Map(),
        getFileIncludeReasons: () => new Map(),
    };
}

function install(ts, hooks) {
    hooks.transpileWorker = (input, transpileOptions, declaration) => transpileWorker(ts, input, transpileOptions, declaration);
}

module.exports = { install, transpileWorker };
