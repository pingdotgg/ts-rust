"use strict";

// Custom transformers ({ before, after, afterDeclarations }) on top of tsc-rs emit. They run on
// TS 6.0 trees with TS 6.0's own machinery (transformNodes, the factory, the printer); tsc-rs only
// ever sees text:
// - before: a file whose tree changed is printed back to source text (statements the
//   transformers left alone keep their original text) and tsc-rs emits that text;
// - after and afterDeclarations: each emitted text is parsed by TS 6.0, transformed and, when the
//   tree changed, printed back.
// Source maps are composed, so they still point at the original source.
// Module specifiers that transformers only replaced (the nest-cli tsconfig-paths hook) are written
// into the output text instead: tsc-rs emits the file from its original text, as TS 6.0 names
// import variables after the original specifier.

const sourceMappingUrl = "//# sourceMappingURL=";
const inlineSourceMap = "data:application/json;base64,";
const byteOrderMark = "\uFEFF";
const base64Digits = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const keptStatementMarker = "__tsrs_kept_statement_";

// Runs the custom transformers over the outputs of `files` ({ sourceFile, nativeFileName,
// outputs: [{ fileName, text }] }), replacing outputs in place. `reemit(texts)` emits the JS of
// changed files from their new source texts (Map nativeFileName -> text) and returns
// Map nativeFileName -> outputs. `directory` is the absolute directory that relative output and
// source names are relative to.
function transformOutputs(ts, { options, emitHost, files, customTransformers, reemit, cancellationToken, directory }) {
    const before = customTransformers?.before ?? [];
    const after = customTransformers?.after ?? [];
    const afterDeclarations = customTransformers?.afterDeclarations ?? [];
    const context = { options, emitHost, directory: directory ?? emitHost.getCurrentDirectory() };
    if (before.length !== 0) {
        transformSources(ts, { ...context, files, factories: before, reemit, cancellationToken });
    }
    if (after.length === 0 && afterDeclarations.length === 0) {
        return;
    }
    for (const file of files) {
        cancellationToken?.throwIfCancellationRequested();
        for (const output of [...file.outputs]) {
            if (after.length !== 0 && isScriptOutput(ts, output.fileName)) {
                const diagnostics = transformEmittedText(ts, { ...context, file, output, factories: after, kind: "script" });
                file.scriptDiagnostics = [...file.scriptDiagnostics ?? [], ...diagnostics];
            }
            else if (afterDeclarations.length !== 0 && isDeclarationOutput(ts, output.fileName)) {
                const diagnostics = transformEmittedText(ts, { ...context, file, output, factories: afterDeclarations, kind: "declaration" });
                file.declarationDiagnostics = [...file.declarationDiagnostics ?? [], ...diagnostics];
            }
        }
    }
}

function transformSources(ts, { options, emitHost, directory, files, factories, reemit, cancellationToken }) {
    const changed = [];
    for (const file of files) {
        // TS 6.0 runs the script transformers on every file it writes JS (or JSON) for.
        if (!file.outputs.some(output => isScriptOutput(ts, output.fileName) || ts.fileExtensionIs(output.fileName, ts.Extension.Json))) {
            continue;
        }
        cancellationToken?.throwIfCancellationRequested();
        const originalScript = file.outputs.find(output => isScriptOutput(ts, output.fileName))?.text;
        const change = transformSource(ts, { options, emitHost, sourceFile: file.sourceFile, factories, originalScript });
        file.scriptDiagnostics = change.diagnostics;
        if (change.splice !== undefined) {
            changed.push({ file, change });
        }
        else if (change.specifiers !== undefined) {
            rewriteScriptSpecifiers(ts, file.outputs, change.specifiers);
        }
    }
    if (changed.length === 0) {
        return;
    }
    const outputs = reemit(new Map(changed.map(({ file, change }) => [file.nativeFileName, change.splice.text])));
    for (const { file, change } of changed) {
        const reemitted = outputs.get(file.nativeFileName) ?? [];
        if (!reemitted.some(output => isScriptOutput(ts, output.fileName))) {
            throw new Error(`tsc-rs: internal error: no JavaScript output for the transformed ${file.sourceFile.fileName}`);
        }
        file.outputs = file.outputs.filter(output => !isScriptOutput(ts, output.fileName) && !isScriptMapOutput(ts, output.fileName));
        for (const output of reemitted) {
            if (isScriptMapOutput(ts, output.fileName)) {
                continue;
            }
            const mapOutput = reemitted.find(candidate => candidate.fileName === `${output.fileName}.map`);
            if (change.splice.keepAlive !== undefined) {
                removeKeepAlive(ts, output, mapOutput, change.splice.keepAlive);
            }
            const sources = new ExtraSources(ts, { options, emitHost, directory, sourceFile: file.sourceFile, outputFileName: output.fileName, recorded: change.splice.sources });
            rewriteSourceMap(ts, output, mapOutput, json => composeWithSplice(json, change.splice, file.sourceFile.text, sources));
            file.outputs.push(output);
            if (mapOutput !== undefined) {
                file.outputs.push(mapOutput);
            }
            if (change.specifiers !== undefined) {
                rewriteScriptSpecifiers(ts, [output, ...mapOutput === undefined ? [] : [mapOutput]], change.specifiers);
            }
            if (change.helpers.length !== 0) {
                insertHelpers(ts, { options, emitHost, directory, file, output, helpers: change.helpers });
            }
        }
    }
}

// TS 6.0: src/compiler/emitter.ts emitJsFileOrBundle runs `before` ahead of its own transformers;
// here they run alone, and a file whose tree changed becomes new source text for tsc-rs.
// `originalScript` is tsc-rs's JS output of the unchanged file.
function transformSource(ts, { options, emitHost, sourceFile, factories, originalScript }) {
    const state = { helpers: undefined, printerHooks: false, context: undefined };
    const captureHelpers = context => node => {
        state.helpers = context.readEmitHelpers();
        return node;
    };
    const originalStatements = parsedStatements(sourceFile);
    const transform = ts.transformNodes(
        unsupportedResolver(),
        emitHost,
        ts.factory,
        options,
        [sourceFile],
        [...factories.map(factory => wrapScriptFactory(ts, factory, state)), captureHelpers],
        /*allowDtsFiles*/ false,
    );
    try {
        const result = transform.transformed[0];
        const helpers = [...state.helpers ?? [], ...unscopedSourceFileHelpers(ts, sourceFile, result)];
        if (result === sourceFile && !state.printerHooks && helpers.length === 0 && annotatedNodes(sourceFile).length === 0 && !mutatedInPlace(ts, sourceFile, originalStatements)) {
            return { diagnostics: transform.diagnostics, helpers };
        }
        if (ts.isJsonSourceFile(sourceFile)) {
            throw new Error(`tsc-rs: a custom 'before' transformer that changes a JSON source file is not supported (${sourceFile.fileName})`);
        }
        const printAll = state.printerHooks || sourceFileHasOwnEmitNode(sourceFile, result);
        const specifiers = printAll ? undefined : findSpecifierChanges(ts, { file: sourceFile, originalStatements, result, dirty: dirtyStatements(sourceFile), options });
        if (specifiers !== undefined && helpers.length === 0 && keepsOriginalText(ts, sourceFile, originalStatements, result, specifiers)) {
            return { diagnostics: transform.diagnostics, helpers: [], specifiers: specifiers.edits };
        }
        const splice = printSpliced(ts, {
            file: sourceFile,
            originalStatements,
            result,
            transform,
            printerOptions: { ...scriptPrinterOptions(ts, options), removeComments: false, noEmitHelpers: true },
            printAll,
            keptAs: specifiers?.statements,
            keepAlive: { retained: retainedImportNames(ts, sourceFile, originalStatements, originalScript) },
        });
        return { diagnostics: transform.diagnostics, helpers: helpers.filter(helper => !helper.scoped), splice, specifiers: specifiers?.edits };
    }
    finally {
        transform.dispose();
    }
}

// Whether the transformed file is the original one with only module specifiers replaced, so that
// tsc-rs's emit of the original text plus the new specifiers is the output.
function keepsOriginalText(ts, sourceFile, originalStatements, result, specifiers) {
    const statements = result.statements;
    return annotatedNodes(sourceFile).length === 0
        && !mutatedInPlace(ts, sourceFile, originalStatements)
        && !ts.positionIsSynthesized(statements.pos)
        && statements.length === originalStatements.length
        && statements.every((statement, index) => statement === originalStatements[index] || specifiers.statements.get(statement) === originalStatements[index]);
}

// Top-level statements of `result` that are statements of `file` with only module specifier
// literals replaced: { statements: Map transformed -> original, edits: Map original specifier
// text -> { text, mapped }, pairs }. `inPlace` (the after transformers) edits the parsed text at
// the specifiers themselves; otherwise tsc-rs emits the original specifiers and every one of them
// in the output gets its new text, so a specifier text that also stays unchanged somewhere, or
// that the emit makes itself, is left to the printer.
function findSpecifierChanges(ts, { file, originalStatements, result, dirty, options, inPlace = false }) {
    if (!inPlace && options.rewriteRelativeImportExtensions) {
        return undefined;
    }
    const originals = new Set(originalStatements);
    const candidates = [];
    for (const statement of result.statements) {
        if (originals.has(statement)) {
            continue;
        }
        const original = originalIn(statement, originals);
        if (original === undefined || dirty.has(original) || !isPristine(ts, original)) {
            continue;
        }
        const pairs = [];
        if (onlySpecifiersChanged(ts, statement, original, pairs) && pairs.length !== 0 && printsAsOriginal(ts, file, statement, original, pairs)) {
            candidates.push({ statement, original, pairs });
        }
    }
    if (candidates.length === 0) {
        return undefined;
    }
    const printer = ts.createPrinter();
    for (const { pairs } of candidates) {
        for (const pair of pairs) {
            // TS 6.0 maps a synthesized specifier only when it got a text range.
            pair.edit = { text: printer.printNode(ts.EmitHint.Unspecified, pair.literal, file), mapped: ts.getSourceMapRange(pair.literal).pos >= 0 };
        }
    }
    const excluded = inPlace ? new Set() : ambiguousSpecifiers(ts, result, candidates);
    const statements = new Map();
    const edits = new Map();
    const pairs = [];
    for (const candidate of candidates) {
        if (candidate.pairs.some(pair => excluded.has(pair.originalLiteral.text))) {
            continue;
        }
        statements.set(candidate.statement, candidate.original);
        for (const pair of candidate.pairs) {
            edits.set(pair.originalLiteral.text, pair.edit);
            pairs.push(pair);
        }
    }
    return statements.size === 0 ? undefined : { statements, edits, pairs };
}

// Whether `node` is the parse tree node `original` (or an update of it) with nothing changed but
// module specifier literals, which go to `pairs`.
function onlySpecifiersChanged(ts, node, original, pairs) {
    if (node === original) {
        return true;
    }
    if (node.kind !== original.kind || !madeFrom(node, original) || node.emitNode !== undefined) {
        return false;
    }
    const slot = specifierSlotOf(ts, node);
    const originalSlot = specifierSlotOf(ts, original);
    const same = (child, originalChild) => {
        if (child === originalChild) {
            return true;
        }
        if (child === slot && originalChild === originalSlot) {
            return addSpecifierPair(ts, slot, originalSlot, pairs);
        }
        return onlySpecifiersChanged(ts, child, originalChild, pairs);
    };
    const children = childEntries(ts, node);
    const originalChildren = childEntries(ts, original);
    if (children.length !== originalChildren.length) {
        return false;
    }
    for (let index = 0; index < children.length; index++) {
        const child = children[index];
        const originalChild = originalChildren[index];
        if (child === originalChild) {
            continue;
        }
        if (!Array.isArray(child) || !Array.isArray(originalChild)) {
            if (Array.isArray(child) || Array.isArray(originalChild) || !same(child, originalChild)) {
                return false;
            }
            continue;
        }
        if (child.length !== originalChild.length || !child.every((element, at) => same(element, originalChild[at]))) {
            return false;
        }
    }
    return true;
}

function addSpecifierPair(ts, slot, originalSlot, pairs) {
    const literal = slotLiteral(ts, slot);
    const originalLiteral = slotLiteral(ts, originalSlot);
    if (literal === undefined || originalLiteral === undefined || slot.kind !== originalSlot.kind) {
        return false;
    }
    // A parse tree node or one with emit data prints differently from the new literal TS 6.0 writes.
    if (!(literal.flags & ts.NodeFlags.Synthesized) || literal.emitNode !== undefined || (slot !== literal && slot.emitNode !== undefined)) {
        return false;
    }
    pairs.push({ slot, originalSlot, literal, originalLiteral });
    return true;
}

// The node of `originals` that `node` was made from (transformers may clear the Synthesized flag of
// an update, so this follows `original` as TS 6.0's getNodeForGeneratedName does).
function originalIn(node, originals) {
    let current = node;
    while (current !== undefined && !originals.has(current)) {
        current = current.original;
    }
    return current;
}

function madeFrom(node, original) {
    for (let current = node; current !== undefined; current = current.original) {
        if (current === original) {
            return true;
        }
    }
    return false;
}

function childEntries(ts, node) {
    const entries = [];
    ts.forEachChild(node, child => {
        entries.push(child);
    }, children => {
        entries.push(children);
    });
    return entries;
}

// What the module specifier of an import or export declaration, an external module reference, an
// import type or an import()/require() call is.
function specifierSlotOf(ts, node) {
    switch (node.kind) {
        case ts.SyntaxKind.ImportDeclaration:
        case ts.SyntaxKind.ExportDeclaration:
            return node.moduleSpecifier;
        case ts.SyntaxKind.ExternalModuleReference:
            return node.expression;
        case ts.SyntaxKind.ImportType:
            return node.argument;
        case ts.SyntaxKind.CallExpression:
            return node.arguments.length !== 0 && isImportOrRequire(ts, node.expression) ? node.arguments[0] : undefined;
        default:
            return undefined;
    }
}

function isImportOrRequire(ts, expression) {
    return expression.kind === ts.SyntaxKind.ImportKeyword || (ts.isIdentifier(expression) && expression.escapedText === "require");
}

function slotLiteral(ts, slot) {
    const literal = slot !== undefined && ts.isLiteralTypeNode(slot) ? slot.literal : slot;
    return literal !== undefined && ts.isStringLiteralLike(literal) ? literal : undefined;
}

// The printer's text of the statement with its new specifiers swapped back is the original's.
function printsAsOriginal(ts, file, statement, original, pairs) {
    const originals = new Map(pairs.map(pair => [pair.slot, pair.originalSlot]));
    const swapped = ts.createPrinter({}, { substituteNode: (_hint, node) => originals.get(node) ?? node });
    return swapped.printNode(ts.EmitHint.Unspecified, statement, file) === ts.createPrinter().printNode(ts.EmitHint.Unspecified, original, file);
}

// Specifier texts that cannot be told apart in the output: ones that also stay unchanged
// elsewhere in the file, get two different new texts, or that the emit writes itself (helpers,
// the JSX runtime).
function ambiguousSpecifiers(ts, result, candidates) {
    const edits = new Map();
    const pairLiterals = new Set();
    const excluded = new Set();
    for (const { pairs } of candidates) {
        for (const pair of pairs) {
            const text = pair.originalLiteral.text;
            const known = edits.get(text);
            if (generatedSpecifier.test(text) || (known !== undefined && (known.text !== pair.edit.text || known.mapped !== pair.edit.mapped))) {
                excluded.add(text);
            }
            edits.set(text, pair.edit);
            pairLiterals.add(pair.literal);
        }
    }
    const visit = node => {
        const literal = slotLiteral(ts, specifierSlotOf(ts, node));
        if (literal !== undefined && !pairLiterals.has(literal) && edits.has(literal.text)) {
            excluded.add(literal.text);
        }
        ts.forEachChild(node, visit);
    };
    ts.forEachChild(result, visit);
    return excluded;
}

const generatedSpecifier = /^tslib$|\/jsx(-dev)?-runtime$/;

// Writes the new specifiers (`edits`: original text -> { text, mapped }) into JS that tsc-rs
// emitted from the original specifiers, and moves the maps.
function rewriteScriptSpecifiers(ts, outputs, edits) {
    for (const output of outputs) {
        if (!isScriptOutput(ts, output.fileName)) {
            continue;
        }
        const mapOutput = outputs.find(candidate => candidate.fileName === `${output.fileName}.map`);
        const { body } = splitSourceMappingUrl(output.text.startsWith(byteOrderMark) ? output.text.slice(1) : output.text);
        const scriptKind = ts.fileExtensionIs(output.fileName, ts.Extension.Jsx) ? ts.ScriptKind.JSX : ts.ScriptKind.JS;
        const parsed = ts.createSourceFile(output.fileName, body, ts.ScriptTarget.Latest, /*setParentNodes*/ false, scriptKind);
        const changes = [];
        const visit = node => {
            const literal = slotLiteral(ts, specifierSlotOf(ts, node));
            if (literal !== undefined && edits.has(literal.text)) {
                changes.push(specifierEdit(ts, parsed, literal, edits.get(literal.text)));
            }
            ts.forEachChild(node, visit);
        };
        ts.forEachChild(parsed, visit);
        applyTextEdits(ts, output, mapOutput, changes);
    }
}

function specifierEdit(ts, parsed, literal, edit) {
    return { start: literal.getStart(parsed), end: literal.end, text: edit.text, mapped: edit.mapped };
}

// Replaces ranges ({ start, end, text, mapped }) of an output's text and moves its map: mappings
// after a range on its line shift; the ones at its ends stay only when `mapped`.
function applyTextEdits(ts, output, mapOutput, changes) {
    if (changes.length === 0) {
        return;
    }
    const hasByteOrderMark = output.text.startsWith(byteOrderMark);
    const { body, url } = splitSourceMappingUrl(hasByteOrderMark ? output.text.slice(1) : output.text);
    const lineStarts = ts.computeLineStarts(body);
    const byLine = new Map();
    let text = "";
    let last = 0;
    for (const change of [...changes].sort((a, b) => a.start - b.start)) {
        const start = ts.computeLineAndCharacterOfPosition(lineStarts, change.start);
        const end = ts.computeLineAndCharacterOfPosition(lineStarts, change.end);
        // A literal with a line continuation keeps its text.
        if (start.line !== end.line || change.start < last) {
            continue;
        }
        text += body.slice(last, change.start) + change.text;
        last = change.end;
        const edits = byLine.get(start.line) ?? [];
        edits.push({ start: start.character, end: end.character, delta: change.text.length - (change.end - change.start), mapped: change.mapped });
        byLine.set(start.line, edits);
    }
    text += body.slice(last);
    const move = json => ({ ...json, mappings: encodeMappings(shiftColumns(decodeMappings(ts, json.mappings), byLine)) });
    const prefix = hasByteOrderMark ? byteOrderMark : "";
    if (url?.startsWith(inlineSourceMap)) {
        output.text = prefix + text + sourceMappingUrl + toInlineSourceMap(move(fromInlineSourceMap(url)));
        return;
    }
    if (url !== undefined && mapOutput !== undefined) {
        mapOutput.text = JSON.stringify(move(JSON.parse(mapOutput.text)));
    }
    output.text = prefix + text + (url === undefined ? "" : sourceMappingUrl + url);
}

function shiftColumns(lines, byLine) {
    for (const [line, edits] of byLine) {
        lines[line] = lines[line]?.flatMap(segment => {
            let shift = 0;
            for (const edit of edits) {
                if (segment[0] < edit.start || (segment[0] === edit.start && edit.mapped)) {
                    break;
                }
                if (segment[0] > edit.end || (segment[0] === edit.end && edit.mapped)) {
                    shift += edit.delta;
                    continue;
                }
                return [];
            }
            return [[segment[0] + shift, ...segment.slice(1)]];
        });
    }
    return lines;
}

// Parses an emitted JS or declaration text, runs `factories` on it, and when the tree changed,
// prints it back into `output` (and its map).
function transformEmittedText(ts, { options, emitHost, directory, file, output, factories, kind }) {
    const hasByteOrderMark = output.text.startsWith(byteOrderMark);
    const { body, url } = splitSourceMappingUrl(hasByteOrderMark ? output.text.slice(1) : output.text);
    const scriptKind = kind === "declaration" ? ts.ScriptKind.TS : ts.fileExtensionIs(output.fileName, ts.Extension.Jsx) ? ts.ScriptKind.JSX : ts.ScriptKind.JS;
    const parsed = ts.createSourceFile(
        file.sourceFile.fileName,
        body,
        {
            languageVersion: ts.getEmitScriptTarget(options),
            impliedNodeFormat: file.sourceFile.impliedNodeFormat,
            setExternalModuleIndicator: ts.getSetExternalModuleIndicator(options),
        },
        /*setParentNodes*/ true,
        scriptKind,
    );
    if (kind === "declaration") {
        // TS 6.0: src/compiler/transformers/declarations.ts transformRoot hands afterDeclarations a declaration file.
        parsed.isDeclarationFile = true;
    }
    const state = { helpers: undefined, printerHooks: false, context: undefined };
    const wrap = kind === "declaration" ? wrapDeclarationFactory : wrapScriptFactory;
    const originalStatements = [...parsed.statements];
    const transform = ts.transformNodes(unsupportedResolver(), emitHost, ts.factory, options, [parsed], factories.map(factory => wrap(ts, factory, state)), /*allowDtsFiles*/ true);
    try {
        const result = transform.transformed[0];
        if (result === parsed && !state.printerHooks && annotatedNodes(parsed).length === 0 && !mutatedInPlace(ts, parsed, originalStatements)) {
            return transform.diagnostics;
        }
        const printAll = state.printerHooks || sourceFileHasOwnEmitNode(parsed, result);
        const extraHeader = kind === "declaration" ? addedReferenceDirectives(ts, parsed, result, ts.getNewLineCharacter(options)) : "";
        const specifiers = printAll || extraHeader !== "" ? undefined : findSpecifierChanges(ts, { file: parsed, originalStatements, result, dirty: dirtyStatements(parsed), options, inPlace: true });
        if (specifiers !== undefined && keepsOriginalText(ts, parsed, originalStatements, result, specifiers)) {
            const mapOutput = file.outputs.find(candidate => candidate.fileName === `${output.fileName}.map`);
            applyTextEdits(ts, output, mapOutput, specifiers.pairs.map(pair => specifierEdit(ts, parsed, pair.originalLiteral, pair.edit)));
            return transform.diagnostics;
        }
        const splice = printSpliced(ts, {
            file: parsed,
            originalStatements,
            result,
            transform,
            printerOptions: kind === "declaration" ? declarationPrinterOptions(options) : scriptPrinterOptions(ts, options),
            printAll,
            extraHeader,
        });
        const sources = new ExtraSources(ts, { options, emitHost, directory, sourceFile: file.sourceFile, outputFileName: output.fileName, recorded: splice.sources });
        replaceOutputText(ts, file, output, splice, { hasByteOrderMark, url, sources });
        return transform.diagnostics;
    }
    finally {
        transform.dispose();
    }
}

// Helpers that `before` transformers requested go into the JS output after its prologue, as
// TS 6.0's printer writes them; helpers tsc-rs already wrote are not repeated.
function insertHelpers(ts, { options, emitHost, directory, file, output, helpers }) {
    const missing = helpers.filter(helper => helper.importName === undefined || !output.text.includes(`var ${helper.importName} = `));
    if (missing.length === 0) {
        return;
    }
    const addHelpers = () => sourceFile => {
        ts.addEmitHelpers(sourceFile, missing);
        return sourceFile;
    };
    transformEmittedText(ts, { options, emitHost, directory, file, output, factories: [addHelpers], kind: "script" });
}

function replaceOutputText(ts, file, output, splice, { hasByteOrderMark, url, sources }) {
    let text = splice.text;
    if (url !== undefined) {
        if (!/[\r\n]$/.test(text)) {
            text += "\n";
        }
        const mapOutput = file.outputs.find(candidate => candidate.fileName === `${output.fileName}.map`);
        const remap = json => sources.addTo({ ...json, mappings: encodeMappings(splice.remap(decodeMappings(ts, json.mappings), sources, json.sources.length)) });
        if (url.startsWith(inlineSourceMap)) {
            text += sourceMappingUrl + toInlineSourceMap(remap(fromInlineSourceMap(url)));
        }
        else {
            if (mapOutput !== undefined) {
                mapOutput.text = JSON.stringify(remap(JSON.parse(mapOutput.text)));
            }
            text += sourceMappingUrl + url;
        }
    }
    output.text = (hasByteOrderMark ? byteOrderMark : "") + text;
}

// Updates the source map of `output` (its own .map output or the inline one) with `update(json)`.
function rewriteSourceMap(ts, output, mapOutput, update) {
    const hasByteOrderMark = output.text.startsWith(byteOrderMark);
    const { body, url } = splitSourceMappingUrl(hasByteOrderMark ? output.text.slice(1) : output.text);
    if (url?.startsWith(inlineSourceMap)) {
        output.text = (hasByteOrderMark ? byteOrderMark : "") + body + sourceMappingUrl + toInlineSourceMap(update(fromInlineSourceMap(url)));
        return;
    }
    if (mapOutput !== undefined) {
        mapOutput.text = JSON.stringify(update(JSON.parse(mapOutput.text)));
    }
}

// The source map of a sourceMappingURL that holds the map itself, or undefined.
function fromInlineSourceMap(url) {
    if (!url.startsWith(inlineSourceMap)) {
        return undefined;
    }
    return JSON.parse(Buffer.from(url.slice(inlineSourceMap.length), "base64").toString("utf8"));
}

// TS 6.0: src/compiler/emitter.ts getSourceMappingURL (base64 of the UTF-8 text)
function toInlineSourceMap(json) {
    return inlineSourceMap + Buffer.from(JSON.stringify(json), "utf8").toString("base64");
}

// The map that tsc-rs made for the printed source, moved onto the original source. Text that only
// the printer made (synthesized nodes) maps nowhere, as in TS 6.0.
function composeWithSplice(json, splice, originalText, sources) {
    const lines = splice.decode(json.mappings).map(segments => segments?.flatMap(segment => {
        if (segment.length === 1) {
            return [];
        }
        const original = splice.originalPosition(segment[2], segment[3]);
        if (original === undefined) {
            return [];
        }
        const source = original.source === 0 ? segment[1] : sources.index(original.source, json.sources.length);
        return [[segment[0], source, original.line, original.character, segment[4]]];
    }));
    const composed = { ...json, mappings: encodeMappings(lines) };
    if (Array.isArray(json.sourcesContent)) {
        composed.sourcesContent = json.sourcesContent.map((content, index) => index === 0 ? originalText : content);
    }
    return sources.addTo(composed);
}

// Sources other than the transformed file that transformers mapped synthesized nodes to
// (setSourceMapRange with a `source`), named as TS 6.0 names them in a map.
class ExtraSources {
    constructor(ts, { options, emitHost, directory, sourceFile, outputFileName, recorded }) {
        this.ts = ts;
        this.options = options;
        this.emitHost = emitHost;
        this.directory = directory;
        this.sourceFile = sourceFile;
        this.outputFileName = outputFileName;
        this.recorded = recorded ?? { names: [], contents: [] };
        this.used = [];
    }

    // The index in the composed map (after its `count` own sources) of source `recordedIndex` of
    // the printer's recording.
    index(recordedIndex, count) {
        let at = this.used.indexOf(recordedIndex);
        if (at < 0) {
            at = this.used.push(recordedIndex) - 1;
        }
        return count + at;
    }

    addTo(json) {
        if (this.used.length === 0) {
            return json;
        }
        const sources = [...json.sources, ...this.used.map(index => this.sourceName(this.recorded.names[index]))];
        const result = { ...json, sources };
        if (Array.isArray(json.sourcesContent)) {
            result.sourcesContent = [...json.sourcesContent, ...this.used.map(index => this.recorded.contents[index] ?? null)];
        }
        return result;
    }

    // TS 6.0: src/compiler/sourcemap.ts addSource with the directory of
    // src/compiler/emitter.ts getSourceMapDirectory
    sourceName(fileName) {
        const { ts, options, emitHost, directory } = this;
        let mapDirectory;
        if (options.sourceRoot) {
            mapDirectory = emitHost.getCommonSourceDirectory();
        }
        else if (options.mapRoot) {
            mapDirectory = ts.getDirectoryPath(ts.getSourceFilePathInNewDir(this.sourceFile.fileName, emitHost, ts.normalizeSlashes(options.mapRoot)));
            if (ts.getRootLength(mapDirectory) === 0) {
                mapDirectory = ts.combinePaths(emitHost.getCommonSourceDirectory(), mapDirectory);
            }
        }
        else {
            mapDirectory = ts.getDirectoryPath(ts.normalizePath(this.outputFileName));
        }
        return ts.getRelativePathToDirectoryOrUrl(mapDirectory, fileName, directory, emitHost.getCanonicalFileName, /*isAbsolutePathAnUrl*/ true);
    }
}

// TS 6.0: src/compiler/transformer.ts wrapScriptTransformerFactory
function wrapScriptFactory(ts, factory, state) {
    return context => {
        watchContext(context, state);
        const transformer = factory(context);
        if (typeof transformer !== "function") {
            return node => ts.isBundle(node) ? transformer.transformBundle(node) : transformer.transformSourceFile(node);
        }
        return ts.chainBundle(context, transformer);
    };
}

// TS 6.0: src/compiler/transformer.ts wrapDeclarationTransformerFactory
function wrapDeclarationFactory(ts, factory, state) {
    return context => {
        watchContext(context, state);
        const transformer = factory(context);
        if (typeof transformer !== "function") {
            return node => ts.isBundle(node) ? transformer.transformBundle(node) : transformer.transformSourceFile(node);
        }
        return transformer;
    };
}

// Substitutions and emit notifications act when the printer runs, so a file that asks for them
// is printed whole.
function watchContext(context, state) {
    if (state.context === context) {
        return;
    }
    state.context = context;
    const { enableSubstitution, enableEmitNotification } = context;
    context.enableSubstitution = kind => {
        state.printerHooks = true;
        enableSubstitution(kind);
    };
    context.enableEmitNotification = kind => {
        state.printerHooks = true;
        enableEmitNotification(kind);
    };
}

function unsupportedResolver() {
    return new Proxy({}, {
        get(_, name) {
            if (typeof name === "symbol") {
                return undefined;
            }
            return () => {
                throw new Error(`tsc-rs: EmitResolver.${name} is not supported: custom transformers run without the TypeScript 6.0 checker`);
            };
        },
    });
}

// Parse tree nodes that got an emit node (emit flags, synthetic comments, source map ranges,
// helpers): TS 6.0 records them on the source file so it can clean them up after the emit.
function annotatedNodes(sourceFile) {
    return sourceFile.emitNode?.annotatedNodes ?? [];
}

// Emit flags, comments or source map ranges on the source file itself change how all of it
// prints (helpers are handled apart).
function sourceFileHasOwnEmitNode(sourceFile, result) {
    const hasEmitData = emitNode => emitNode !== undefined
        && Object.keys(emitNode).some(key => key !== "annotatedNodes" && key !== "helpers" && emitNode[key] !== undefined);
    return hasEmitData(sourceFile.emitNode) || (result !== sourceFile && hasEmitData(result.emitNode));
}

function unscopedSourceFileHelpers(ts, sourceFile, result) {
    const helpers = new Set([...ts.getEmitHelpers(sourceFile) ?? [], ...ts.getEmitHelpers(result) ?? []]);
    return [...helpers].filter(helper => !helper.scoped);
}

// TS 6.0: src/compiler/emitter.ts:850 emitJsFileOrBundle printerOptions
function scriptPrinterOptions(ts, options) {
    return {
        removeComments: options.removeComments,
        newLine: options.newLine,
        noEmitHelpers: options.noEmitHelpers,
        module: ts.getEmitModuleKind(options),
        moduleResolution: ts.getEmitModuleResolutionKind(options),
        target: ts.getEmitScriptTarget(options),
        sourceMap: options.sourceMap,
        inlineSourceMap: options.inlineSourceMap,
        inlineSources: options.inlineSources,
        extendedDiagnostics: options.extendedDiagnostics,
    };
}

// TS 6.0: src/compiler/emitter.ts:925 emitDeclarationFileOrBundle printerOptions
function declarationPrinterOptions(options) {
    return {
        removeComments: options.removeComments,
        newLine: options.newLine,
        noEmitHelpers: true,
        module: options.module,
        moduleResolution: options.moduleResolution,
        target: options.target,
        sourceMap: options.declarationMap,
        inlineSourceMap: options.inlineSourceMap,
        extendedDiagnostics: options.extendedDiagnostics,
        onlyPrintJsDocStyle: true,
        omitBraceSourceMapPositions: true,
    };
}

// Reference directives an afterDeclarations transformer added with updateSourceFile.
function addedReferenceDirectives(ts, parsed, result, newLine) {
    if (result === parsed) {
        return "";
    }
    const lines = [];
    const add = (kind, before, after) => {
        const known = new Set((before ?? []).map(reference => reference.fileName));
        for (const reference of after ?? []) {
            if (!known.has(reference.fileName)) {
                const preserve = reference.preserve ? `preserve="true" ` : "";
                lines.push(`/// <reference ${kind}="${reference.fileName}" ${preserve}/>`);
            }
        }
    };
    add("path", parsed.referencedFiles, result.referencedFiles);
    add("types", parsed.typeReferenceDirectives, result.typeReferenceDirectives);
    add("lib", parsed.libReferenceDirectives, result.libReferenceDirectives);
    return lines.map(line => line + newLine).join("");
}

// Prints `result`, the transformed `file`, back to text. Top-level statements that are still the
// original nodes (and that no transformer annotated) keep their original text, so only the
// changed statements go through the printer; the file header (shebang, copyright comments,
// reference directives) and the end of the file keep their text too. All changed statements print
// in one pass, as TS 6.0 prints a file, so generated names do not collide.
// `keptAs` maps transformed statements whose text stays the original statement's (only their
// module specifiers changed) to that statement. `keepAlive` ({ retained }, see
// keptImportNames) is for text that tsc-rs compiles again.
function printSpliced(ts, { file, originalStatements, result, transform, printerOptions, printAll, keptAs, extraHeader = "", keepAlive }) {
    const text = file.text;
    // TS 6.0: src/compiler/emitter.ts emitBodyWithDetachedComments prints the detached comments and
    // the comments at the end of the file only when the statement list kept its text range; a
    // transformer that builds a new list makes the file header the first statement's comments.
    const statementsKeepRange = !ts.positionIsSynthesized(result.statements.pos);
    const headerEnd = fileHeaderEnd(ts, file, originalStatements, statementsKeepRange);
    const dirty = printAll ? undefined : dirtyStatements(file);
    if (!statementsKeepRange && originalStatements.length !== 0) {
        dirty?.add(originalStatements[0]);
    }
    const originalIndex = new Map(originalStatements.map((statement, index) => [statement, index]));
    const used = new Set();
    const marker = `${keptStatementMarker}${Math.random().toString(36).slice(2)}_`;
    const kept = [];
    const prologueCount = countPrologue(ts, result.statements);
    const statements = result.statements.map((statement, index) => {
        const source = keptAs?.get(statement) ?? statement;
        const original = originalIndex.get(source);
        if (dirty === undefined || original === undefined || index < prologueCount || dirty.has(source) || used.has(source) || !isPristine(ts, source)) {
            return statement;
        }
        used.add(source);
        kept.push({ ...keptRange(ts, file, source, original, headerEnd), statement: source, at: index });
        return ts.factory.createExpressionStatement(ts.factory.createIdentifier(`${marker}${kept.length - 1}`));
    });
    const keepAliveMarker = keepAlive === undefined ? undefined : `${marker}keep_alive`;
    const keepAliveNames = keepAlive === undefined ? [] : result.statements.flatMap(statement => keptImportNames(ts, statement, keepAlive.retained));
    if (keepAliveNames.length !== 0) {
        const { factory } = ts;
        const list = factory.createArrayLiteralExpression([factory.createStringLiteral(keepAliveMarker), ...keepAliveNames]);
        statements.push(factory.createExpressionStatement(factory.createVoidExpression(list)));
    }

    const synthetic = ts.factory.updateSourceFile(result, statements);
    synthetic.text = blank(text, 0, headerEnd);
    synthetic.isDeclarationFile = false;
    ts.setEmitFlags(synthetic, ts.EmitFlags.NoLeadingComments | ts.EmitFlags.NoTrailingComments);

    const recorder = createMappingRecorder();
    const printer = ts.createPrinter(printerOptions, {
        onEmitNode: transform.emitNodeWithNotification,
        isEmitNotificationEnabled: transform.isEmitNotificationEnabled,
        substituteNode: transform.substituteNode,
    });
    const writer = ts.createTextWriter(ts.getNewLineCharacter(printerOptions));
    const unmark = markSynthesizedNodes(ts, synthetic.statements);
    try {
        printer.writeFile(synthetic, writer, recorder);
    }
    finally {
        unmark();
    }
    const printed = writer.getText().replace(/[\r\n]+$/, "");

    const builder = new SpliceBuilder(ts, text, printed, recorder);
    builder.original(0, headerEnd);
    builder.inserted(extraHeader);
    let cursor = 0;
    for (let index = 0; index < kept.length; index++) {
        const name = `${marker}${index};`;
        const at = printed.indexOf(name, cursor);
        if (at < 0) {
            throw new Error("tsc-rs: internal error: a kept statement was not printed");
        }
        builder.printed(cursor, at);
        const { start, end, statement } = kept[index];
        // Pasted before other code, a statement that ASI ended needs its `;`.
        if (kept[index].at < statements.length - 1 && endsWithoutSemicolon(ts, text, statement)) {
            builder.original(start, statement.end);
            builder.inserted(";", statement.end);
            builder.original(statement.end, end);
        }
        else {
            builder.original(start, end);
        }
        cursor = at + name.length;
    }
    builder.printed(cursor, printed.length);
    if (statementsKeepRange) {
        builder.original(fileEndStart(ts, file, originalStatements, headerEnd), text.length);
    }
    else {
        builder.inserted(ts.getNewLineCharacter(printerOptions));
    }
    return builder.finish(keepAliveNames.length === 0 ? undefined : keepAliveMarker);
}

// Gives every node without a source map position one in a stand-in source while the printer
// runs, so that the recording shows where synthesized nodes start and end (TS 6.0 maps nothing
// there).
function markSynthesizedNodes(ts, statements) {
    const marked = [];
    const visit = node => {
        if (ts.getSourceMapRange(node).pos < 0) {
            const existing = node.emitNode;
            const emitNode = existing ?? (node.emitNode = {});
            marked.push({ node, emitNode, created: existing === undefined, range: emitNode.sourceMapRange });
            emitNode.sourceMapRange = synthesizedRange;
        }
        ts.forEachChild(node, visit);
    };
    statements.forEach(visit);
    return () => {
        for (const { node, emitNode, created, range } of marked) {
            if (created) {
                node.emitNode = undefined;
            }
            else {
                emitNode.sourceMapRange = range;
            }
        }
    };
}

const synthesizedSourceName = "\0tsc-rs synthesized";
const synthesizedRange = { pos: 0, end: 0, source: { fileName: synthesizedSourceName, text: "", skipTrivia: pos => pos } };

// Whether the source text of a statement ends where ASI ended it, not at a `;` or at the `}` of a
// block or declaration body.
function endsWithoutSemicolon(ts, text, statement) {
    if (text[statement.end - 1] === ";") {
        return false;
    }
    let last = statement;
    for (;;) {
        if (ts.isIfStatement(last)) {
            last = last.elseStatement ?? last.thenStatement;
        }
        else if (ts.isIterationStatement(last, /*lookInLabeledStatements*/ false) && !ts.isDoStatement(last)) {
            last = last.statement;
        }
        else if (ts.isLabeledStatement(last) || ts.isWithStatement(last)) {
            last = last.statement;
        }
        else {
            break;
        }
    }
    switch (last.kind) {
        case ts.SyntaxKind.Block:
        case ts.SyntaxKind.ClassDeclaration:
        case ts.SyntaxKind.InterfaceDeclaration:
        case ts.SyntaxKind.EnumDeclaration:
        case ts.SyntaxKind.TryStatement:
        case ts.SyntaxKind.SwitchStatement:
        case ts.SyntaxKind.EmptyStatement:
            return false;
        case ts.SyntaxKind.FunctionDeclaration:
        case ts.SyntaxKind.ModuleDeclaration:
            return last.body === undefined;
        default:
            return true;
    }
}

// The local names of a printed import statement that tsc-rs must not elide from the text it
// compiles again: a statement at the end of that text uses them, and its output is removed. TS 6.0
// decides elision before custom transformers run (src/compiler/checker.ts emit resolver
// isReferencedAliasDeclaration), so it keeps an import whose last use a transformer removed
// (`retained`: retainedImportNames), and never elides one that a transformer created (no parse
// tree node).
function keptImportNames(ts, statement, retained) {
    const names = [];
    const add = (node, name) => {
        if (name === undefined) {
            return;
        }
        const original = ts.getParseTreeNode(node);
        if (original === undefined || (original.name !== undefined && retained.has(ts.idText(original.name)))) {
            names.push(name);
        }
    };
    if (ts.isImportEqualsDeclaration(statement)) {
        const exported = statement.modifiers?.some(modifier => modifier.kind === ts.SyntaxKind.ExportKeyword);
        if (!statement.isTypeOnly && !exported && ts.isExternalModuleReference(statement.moduleReference)) {
            add(statement, statement.name);
        }
        return names;
    }
    const clause = ts.isImportDeclaration(statement) ? statement.importClause : undefined;
    if (clause === undefined || clause.isTypeOnly) {
        return names;
    }
    add(clause, clause.name);
    const bindings = clause.namedBindings;
    if (bindings !== undefined && ts.isNamespaceImport(bindings)) {
        add(bindings, bindings.name);
    }
    else if (bindings !== undefined) {
        for (const element of bindings.elements) {
            if (!element.isTypeOnly) {
                add(element, element.name);
            }
        }
    }
    return names;
}

// The local names of the import bindings of the file that tsc-rs's emit of its unchanged text
// (`script`) kept, as TS 6.0 does for the bindings the original program references. An import
// declaration in the output keeps its local names. Under CommonJS, the variable that holds the
// require of a namespace import or of `import x = require()` keeps the local name, and a default or
// named import is read as `<variable>.<imported name>`.
function retainedImportNames(ts, sourceFile, originalStatements, script) {
    const names = new Set();
    if (script === undefined || originalStatements.every(statement => importSpecifierText(ts, statement) === undefined)) {
        return names;
    }
    const { body } = splitSourceMappingUrl(script.startsWith(byteOrderMark) ? script.slice(1) : script);
    const output = ts.createSourceFile(sourceFile.fileName, body, ts.ScriptTarget.Latest, /*setParentNodes*/ false, ts.ScriptKind.JS);
    const required = new Map();
    for (const statement of output.statements) {
        if (ts.isImportDeclaration(statement) || ts.isImportEqualsDeclaration(statement)) {
            for (const name of importBindingNames(ts, statement)) {
                names.add(name);
            }
        }
        else if (ts.isVariableStatement(statement)) {
            for (const declaration of statement.declarationList.declarations) {
                const specifier = requiredSpecifier(ts, declaration.initializer);
                if (specifier !== undefined && ts.isIdentifier(declaration.name)) {
                    required.set(ts.idText(declaration.name), specifier);
                }
            }
        }
    }
    if (required.size === 0) {
        return names;
    }
    const reads = new Set();
    const visit = node => {
        const member = ts.isPropertyAccessExpression(node) ? ts.idText(node.name)
            : ts.isElementAccessExpression(node) && ts.isStringLiteralLike(node.argumentExpression) ? node.argumentExpression.text
            : undefined;
        if (member !== undefined && ts.isIdentifier(node.expression) && required.has(ts.idText(node.expression))) {
            reads.add(`${required.get(ts.idText(node.expression))}\0${member}`);
        }
        ts.forEachChild(node, visit);
    };
    ts.forEachChild(output, visit);
    for (const statement of originalStatements) {
        const specifier = importSpecifierText(ts, statement);
        if (specifier === undefined) {
            continue;
        }
        if (ts.isImportEqualsDeclaration(statement)) {
            if (required.get(ts.idText(statement.name)) === specifier) {
                names.add(ts.idText(statement.name));
            }
            continue;
        }
        const clause = statement.importClause;
        if (clause?.name !== undefined && reads.has(`${specifier}\0default`)) {
            names.add(ts.idText(clause.name));
        }
        const bindings = clause?.namedBindings;
        if (bindings !== undefined && ts.isNamespaceImport(bindings)) {
            if (required.get(ts.idText(bindings.name)) === specifier) {
                names.add(ts.idText(bindings.name));
            }
        }
        else if (bindings !== undefined) {
            for (const element of bindings.elements) {
                if (reads.has(`${specifier}\0${ts.moduleExportNameTextUnescaped(element.propertyName ?? element.name)}`)) {
                    names.add(ts.idText(element.name));
                }
            }
        }
    }
    return names;
}

function importBindingNames(ts, statement) {
    if (ts.isImportEqualsDeclaration(statement)) {
        return [ts.idText(statement.name)];
    }
    const clause = statement.importClause;
    const bindings = clause?.namedBindings;
    const names = clause?.name === undefined ? [] : [ts.idText(clause.name)];
    if (bindings !== undefined && ts.isNamespaceImport(bindings)) {
        names.push(ts.idText(bindings.name));
    }
    else if (bindings !== undefined) {
        names.push(...bindings.elements.map(element => ts.idText(element.name)));
    }
    return names;
}

// The module specifier of an import declaration or of `import x = require("...")`.
function importSpecifierText(ts, statement) {
    const specifier = ts.isImportDeclaration(statement) ? statement.moduleSpecifier
        : ts.isImportEqualsDeclaration(statement) && ts.isExternalModuleReference(statement.moduleReference) ? statement.moduleReference.expression
        : undefined;
    return specifier !== undefined && ts.isStringLiteralLike(specifier) ? specifier.text : undefined;
}

// The module of a `require("...")` call, also inside the helpers that wrap it (__importDefault,
// __importStar, tslib's).
function requiredSpecifier(ts, expression) {
    while (expression !== undefined && ts.isCallExpression(expression)) {
        if (ts.isIdentifier(expression.expression) && ts.idText(expression.expression) === "require") {
            const [argument] = expression.arguments;
            return argument !== undefined && ts.isStringLiteralLike(argument) ? argument.text : undefined;
        }
        expression = expression.arguments[0];
    }
    return undefined;
}

// Removes the line of the statement that kept imports alive (see keptImportNames) from tsc-rs's
// output and its map.
function removeKeepAlive(ts, output, mapOutput, marker) {
    const hasByteOrderMark = output.text.startsWith(byteOrderMark);
    const { body, url } = splitSourceMappingUrl(hasByteOrderMark ? output.text.slice(1) : output.text);
    const at = body.indexOf(`"${marker}"`);
    if (at < 0) {
        return;
    }
    const lineStart = body.lastIndexOf("\n", at) + 1;
    const lineBreak = body.indexOf("\n", at);
    const lineEnd = lineBreak < 0 ? body.length : lineBreak + 1;
    if (!/^\s*void \[$/.test(body.slice(lineStart, at))) {
        throw new Error("tsc-rs: internal error: unexpected output for a kept import");
    }
    const line = body.slice(0, lineStart).split("\n").length - 1;
    const newBody = body.slice(0, lineStart) + body.slice(lineEnd);
    output.text = (hasByteOrderMark ? byteOrderMark : "") + newBody + (url === undefined ? "" : sourceMappingUrl + url);
    rewriteSourceMap(ts, output, mapOutput, json => {
        const lines = decodeMappings(ts, json.mappings);
        lines.splice(line, 1);
        return { ...json, mappings: encodeMappings(lines) };
    });
}

// The top-level statements of each source file in the order of its text (what tsc-rs compiles),
// as first seen: a transformer that reorders or edits the array in place leaves it so for every
// later emit of the same SourceFile, which TS 6.0 prints as it is.
const parsedStatementsOf = new WeakMap();

function parsedStatements(sourceFile) {
    let statements = parsedStatementsOf.get(sourceFile);
    if (statements === undefined) {
        statements = [...sourceFile.statements];
        if (statements.every(statement => statement.pos >= 0)) {
            statements.sort((a, b) => a.pos - b.pos);
        }
        parsedStatementsOf.set(sourceFile, statements);
    }
    return statements;
}

// Some transformers reorder or edit node arrays of the parse tree in place (ts-jest's hoist-jest
// sorts statements); TS 6.0 then prints the edited tree, so such a file counts as changed.
function mutatedInPlace(ts, sourceFile, originalStatements) {
    const statements = sourceFile.statements;
    if (statements.length !== originalStatements.length || statements.some((statement, index) => statement !== originalStatements[index])) {
        return true;
    }
    return !statements.every(statement => isPristine(ts, statement));
}

// A parse tree node whose node arrays still hold parse tree nodes in source order.
function isPristine(ts, node) {
    let pristine = node.pos >= 0;
    const visitNode = child => {
        if (child.pos < 0) {
            pristine = false;
        }
        return pristine ? ts.forEachChild(child, visitNode, visitNodes) : true;
    };
    const visitNodes = nodes => {
        let end = nodes.pos;
        for (const child of nodes) {
            if (child.pos < end || visitNode(child)) {
                pristine = false;
                return true;
            }
            end = child.end;
        }
        return undefined;
    };
    if (pristine) {
        ts.forEachChild(node, visitNode, visitNodes);
    }
    return pristine;
}

function countPrologue(ts, statements) {
    let count = 0;
    while (count < statements.length && ts.isPrologueDirective(statements[count])) {
        count++;
    }
    return count;
}

// Top-level statements under a parse tree node that a transformer annotated.
function dirtyStatements(file) {
    const dirty = new Set();
    for (const node of annotatedNodes(file)) {
        let statement = node;
        while (statement.parent !== undefined && statement.parent !== file) {
            statement = statement.parent;
        }
        if (statement.parent === file) {
            dirty.add(statement);
        }
    }
    return dirty;
}

// The end of the text that stays at the top of the file: the shebang, the detached comments
// (TS 6.0: src/compiler/utilities.ts emitDetachedComments) and reference directives.
function fileHeaderEnd(ts, file, originalStatements, withDetachedComments) {
    const text = file.text;
    const shebang = ts.getShebang(text);
    let end = shebang === undefined ? 0 : shebang.length;
    if (originalStatements.length === 0) {
        return end === 0 ? 0 : skipWhitespace(text, end);
    }
    const comments = ts.getLeadingCommentRanges(text, 0) ?? [];
    const lineStarts = ts.getLineStarts(file);
    const lineOf = position => ts.computeLineOfPosition(lineStarts, position);
    let last;
    for (const comment of comments) {
        if (last !== undefined && lineOf(comment.pos) >= lineOf(last.end) + 2) {
            break;
        }
        last = comment;
    }
    if (withDetachedComments && last !== undefined && lineOf(ts.skipTrivia(text, 0)) >= lineOf(last.end) + 2) {
        end = Math.max(end, last.end);
    }
    for (const comment of comments) {
        if (text.startsWith("///", comment.pos)) {
            end = Math.max(end, comment.end);
        }
    }
    return end === 0 ? 0 : skipWhitespace(text, end);
}

// The original text of a kept statement: its leading comments (not the trailing comments of the
// statement before it, which are on the line where that one ends) through its own trailing
// comments.
function keptRange(ts, file, statement, index, headerEnd) {
    const text = file.text;
    let start = statement.pos;
    if (index === 0) {
        start = Math.max(start, headerEnd);
    }
    else {
        const previousTrailing = ts.getTrailingCommentRanges(text, statement.pos);
        if (previousTrailing !== undefined && previousTrailing.length !== 0) {
            start = previousTrailing.at(-1).end;
        }
    }
    return { start: skipWhitespace(text, start), end: trailingCommentsEnd(ts, text, statement.end) };
}

function fileEndStart(ts, file, originalStatements, headerEnd) {
    const last = originalStatements.at(-1);
    return last === undefined ? headerEnd : trailingCommentsEnd(ts, file.text, last.end);
}

function trailingCommentsEnd(ts, text, position) {
    const trailing = ts.getTrailingCommentRanges(text, position);
    return trailing === undefined || trailing.length === 0 ? position : trailing.at(-1).end;
}

function skipWhitespace(text, position) {
    while (position < text.length && /\s/.test(text[position])) {
        position++;
    }
    return position;
}

// `text` with the characters in [start, end) turned into spaces, line breaks kept, so positions
// and lines stay the same.
function blank(text, start, end) {
    if (end <= start) {
        return text;
    }
    return text.slice(0, start) + text.slice(start, end).replace(/[^\r\n\u2028\u2029]/g, " ") + text.slice(end);
}

// Builds the spliced text out of pieces of the original text and of the printed text, and maps
// positions of the result back.
class SpliceBuilder {
    constructor(ts, originalText, printedText, recorder) {
        this.ts = ts;
        this.originalText = originalText;
        this.printedText = printedText;
        this.recorder = recorder;
        this.text = "";
        this.pieces = [];
    }

    original(start, end) {
        if (end > start) {
            this.pieces.push({ kind: "original", start: this.text.length, length: end - start, source: start });
            this.text += this.originalText.slice(start, end);
        }
    }

    printed(start, end) {
        if (end > start) {
            this.pieces.push({ kind: "printed", start: this.text.length, length: end - start, source: start });
            this.text += this.printedText.slice(start, end);
        }
    }

    // Text that is in neither source. With `end`, a position of the original text, the end of the
    // text maps there (a `;` that ends a statement maps as the end of the statement).
    inserted(text, end) {
        if (end !== undefined && text.length !== 0) {
            this.pieces.push({ kind: "inserted", start: this.text.length, length: text.length, source: end });
        }
        this.text += text;
    }

    // `keepAlive` is the marker of the statement that keeps imports (see keptImportNames), if the
    // text has one.
    finish(keepAlive) {
        return new SplicedText(this.ts, this.text, this.pieces, this.originalText, this.printedText, this.recorder, keepAlive);
    }
}

class SplicedText {
    constructor(ts, text, pieces, originalText, printedText, recorder, keepAlive) {
        this.ts = ts;
        this.text = text;
        this.pieces = pieces;
        this.keepAlive = keepAlive;
        this.originalText = originalText;
        this.printedText = printedText;
        this.lineStarts = ts.computeLineStarts(text);
        this.originalLineStarts = ts.computeLineStarts(originalText);
        this.printedLineStarts = ts.computeLineStarts(printedText);
        this.printedMappings = recorder.lines;
        // Sources of the printer's mappings: 0 is the file itself, others come from transformers,
        // except the stand-in for synthesized nodes.
        this.sources = { names: recorder.sources, contents: recorder.contents };
        this.synthesizedSource = recorder.sources.indexOf(synthesizedSourceName);
    }

    decode(mappings) {
        return decodeMappings(this.ts, mappings);
    }

    // The index of the last piece that starts at or before `position`.
    pieceIndex(position) {
        let low = 0;
        let high = this.pieces.length - 1;
        let found = -1;
        while (low <= high) {
            const middle = (low + high) >> 1;
            if (this.pieces[middle].start <= position) {
                found = middle;
                low = middle + 1;
            }
            else {
                high = middle - 1;
            }
        }
        return found;
    }

    // The position in the original text of a line and character of this text. A position where a
    // piece ends (the end of a node) maps through that piece when the next one does not map it.
    originalPosition(line, character) {
        if (line >= this.lineStarts.length) {
            return undefined;
        }
        const position = this.lineStarts[line] + character;
        const index = this.pieceIndex(position);
        for (const candidate of [this.pieces[index], this.pieces[index - 1]]) {
            if (candidate === undefined || position > candidate.start + candidate.length) {
                continue;
            }
            const original = this.originalPositionIn(candidate, position);
            if (original !== undefined) {
                return original;
            }
        }
        return undefined;
    }

    originalPositionIn(piece, position) {
        if (piece.kind === "inserted") {
            return position === piece.start + piece.length ? { source: 0, ...lineAndCharacter(this.ts, this.originalLineStarts, piece.source) } : undefined;
        }
        if (piece.kind === "original") {
            return { source: 0, ...lineAndCharacter(this.ts, this.originalLineStarts, piece.source + position - piece.start) };
        }
        // From the printer's last mapping on the line, only through text copied from the original
        // (the character at the position too, unless the line ends there): text the printer made up
        // (synthesized nodes) maps nowhere, as in TS 6.0.
        const printedPosition = piece.source + position - piece.start;
        const printed = lineAndCharacter(this.ts, this.printedLineStarts, printedPosition);
        const segment = findPrintedSegment(this.printedMappings, printed.line, printed.character, this.synthesizedSource);
        if (segment === undefined || segment[1] === this.synthesizedSource) {
            return undefined;
        }
        const distance = printed.character - segment[0];
        if (segment[1] !== 0) {
            return distance === 0 ? { source: segment[1], line: segment[2], character: segment[3] } : undefined;
        }
        const source = this.originalLineStarts[segment[2]] + segment[3];
        if (distance !== 0 && this.printedText.substr(printedPosition - distance, distance) !== this.originalText.substr(source, distance)) {
            return undefined;
        }
        return { source: 0, line: segment[2], character: segment[3] + distance };
    }

    // `mappings` (decoded, from the original text to some source) moved onto this text; mappings
    // the printer made to other sources go to `sources` (ExtraSources).
    remap(mappings, sources, sourceCount) {
        const result = [];
        const add = (position, segment) => {
            const { line, character } = lineAndCharacter(this.ts, this.lineStarts, position);
            (result[line] ??= []).push([character, ...segment.slice(1)]);
        };
        for (const piece of this.pieces) {
            if (piece.kind === "inserted") {
                continue;
            }
            const isOriginal = piece.kind === "original";
            const lineStarts = isOriginal ? this.originalLineStarts : this.printedLineStarts;
            const sourceMappings = isOriginal ? mappings : this.printedMappings;
            const end = piece.source + piece.length;
            const firstLine = this.ts.computeLineOfPosition(lineStarts, piece.source);
            const lastLine = this.ts.computeLineOfPosition(lineStarts, end);
            for (let line = firstLine; line <= lastLine; line++) {
                for (const segment of sourceMappings[line] ?? []) {
                    const position = lineStarts[line] + segment[0];
                    if (position < piece.source || position >= end || segment.length === 1) {
                        continue;
                    }
                    if (isOriginal) {
                        add(piece.start + position - piece.source, segment);
                        continue;
                    }
                    if (segment[1] === this.synthesizedSource) {
                        continue;
                    }
                    if (segment[1] !== 0) {
                        add(piece.start + position - piece.source, [segment[0], sources.index(segment[1], sourceCount), segment[2], segment[3]]);
                        continue;
                    }
                    const source = findSegment(mappings, segment[2], segment[3]);
                    if (source !== undefined) {
                        add(piece.start + position - piece.source, source);
                    }
                }
            }
        }
        for (const segments of result) {
            segments?.sort((a, b) => a[0] - b[0]);
        }
        return result;
    }
}

function lineAndCharacter(ts, lineStarts, position) {
    return ts.computeLineAndCharacterOfPosition(lineStarts, position);
}

// The printer's segment for a position of the printed text: at the position itself, the last one
// of a node that is not synthesized, if any (a synthesized child starts where its parent does);
// before it, the last one.
function findPrintedSegment(mappings, line, character, synthesizedSource) {
    let found;
    for (const segment of mappings[line] ?? []) {
        if (segment[0] > character) {
            break;
        }
        if (segment[0] === character && found?.[0] === character && segment[1] === synthesizedSource && found[1] !== synthesizedSource) {
            continue;
        }
        found = segment;
    }
    return found;
}

// The last mapped segment of `line` at or before `character`.
function findSegment(mappings, line, character) {
    const segments = mappings[line];
    if (segments === undefined) {
        return undefined;
    }
    let found;
    for (const segment of segments) {
        if (segment[0] > character) {
            break;
        }
        if (segment.length > 1) {
            found = segment;
        }
    }
    return found;
}

// A source map generator for the printer that keeps its mappings by generated line:
// [column, sourceIndex, sourceLine, sourceCharacter]. Source 0 is the printed file; others are
// sources that transformers gave synthesized nodes.
function createMappingRecorder() {
    const lines = [];
    const sources = [];
    const contents = [];
    return {
        lines,
        sources,
        contents,
        // TS 6.0: src/compiler/sourcemap.ts addSource (one index per file name)
        addSource(fileName) {
            const index = sources.indexOf(fileName);
            return index < 0 ? sources.push(fileName) - 1 : index;
        },
        setSourceContent(index, content) {
            contents[index] = content;
        },
        addMapping(generatedLine, generatedCharacter, sourceIndex, sourceLine, sourceCharacter) {
            if (sourceIndex !== undefined) {
                (lines[generatedLine] ??= []).push([generatedCharacter, sourceIndex, sourceLine, sourceCharacter]);
            }
        },
    };
}

function decodeMappings(ts, mappings) {
    const lines = [];
    const decoder = ts.decodeMappings(mappings);
    for (const mapping of decoder) {
        const segment = mapping.sourceIndex === undefined
            ? [mapping.generatedCharacter]
            : [mapping.generatedCharacter, mapping.sourceIndex, mapping.sourceLine, mapping.sourceCharacter, mapping.nameIndex];
        (lines[mapping.generatedLine] ??= []).push(segment);
    }
    if (decoder.error !== undefined) {
        throw new Error(`tsc-rs: cannot read a source map: ${decoder.error}`);
    }
    return lines;
}

function encodeMappings(lines) {
    let text = "";
    let sourceIndex = 0;
    let sourceLine = 0;
    let sourceCharacter = 0;
    let nameIndex = 0;
    for (let line = 0; line < lines.length; line++) {
        if (line > 0) {
            text += ";";
        }
        let column = 0;
        let previous;
        for (const segment of lines[line] ?? []) {
            if (previous !== undefined && sameSegment(previous, segment)) {
                continue;
            }
            if (previous !== undefined) {
                text += ",";
            }
            previous = segment;
            text += vlq(segment[0] - column);
            column = segment[0];
            if (segment.length === 1) {
                continue;
            }
            text += vlq(segment[1] - sourceIndex) + vlq(segment[2] - sourceLine) + vlq(segment[3] - sourceCharacter);
            [sourceIndex, sourceLine, sourceCharacter] = [segment[1], segment[2], segment[3]];
            if (segment[4] !== undefined) {
                text += vlq(segment[4] - nameIndex);
                nameIndex = segment[4];
            }
        }
    }
    return text;
}

function sameSegment(a, b) {
    return a.length === b.length && a.every((value, index) => value === b[index]);
}

function vlq(value) {
    let rest = value < 0 ? (-value << 1) + 1 : value << 1;
    let text = "";
    do {
        let digit = rest & 31;
        rest >>>= 5;
        if (rest > 0) {
            digit |= 32;
        }
        text += base64Digits[digit];
    } while (rest > 0);
    return text;
}

// TS 6.0 ends an output that has a map with "//# sourceMappingURL=<url>" and no line break.
function splitSourceMappingUrl(text) {
    const at = text.lastIndexOf(sourceMappingUrl);
    if (at < 0 || (at > 0 && text[at - 1] !== "\n") || /[\r\n]/.test(text.slice(at))) {
        return { body: text, url: undefined };
    }
    return { body: text.slice(0, at), url: text.slice(at + sourceMappingUrl.length) };
}

function isScriptMapOutput(ts, fileName) {
    return ts.fileExtensionIs(fileName, ".map") && isScriptOutput(ts, fileName.slice(0, -".map".length));
}

function isScriptOutput(ts, fileName) {
    return ts.fileExtensionIsOneOf(fileName, [ts.Extension.Js, ts.Extension.Jsx, ts.Extension.Mjs, ts.Extension.Cjs]);
}

function isDeclarationOutput(ts, fileName) {
    return ts.isDeclarationFileName(fileName);
}

module.exports = {
    transformOutputs,
    splitSourceMappingUrl,
    fromInlineSourceMap,
    isScriptOutput,
    isDeclarationOutput,
};
