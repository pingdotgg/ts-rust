"use strict";

// Codes whose tsc-rs text dropped words that TS 6.0 users see: the
// TS 6.0 text is used. Neither message has arguments.
const TS6_TEXT_CODES = new Set([5074, 5090]);

const messagesByCode = new WeakMap();

// A tsc-rs DiagnosticResponse (crates/ts_goport/src/api/proto.rs) as a TS 6.0 Diagnostic.
// `getSourceFile(nativeFileName)` gives the JS SourceFile of a program file, or undefined; a
// diagnostic without a file has no start or length, like a TS 6.0 global diagnostic.
// TS 6.0: src/compiler/utilities.ts createFileDiagnostic
function toTsDiagnostic(ts, response, getSourceFile) {
    const diagnostic = toRelatedInformation(ts, response, getSourceFile);
    if (response.reportsUnnecessary) {
        diagnostic.reportsUnnecessary = true;
    }
    if (response.reportsDeprecated) {
        diagnostic.reportsDeprecated = true;
    }
    if (response.source && response.source !== "TS") {
        diagnostic.source = response.source;
    }
    if (response.relatedInformation?.length) {
        diagnostic.relatedInformation = response.relatedInformation.map(related => toRelatedInformation(ts, related, getSourceFile));
    }
    return diagnostic;
}

function toTsDiagnostics(ts, responses, getSourceFile) {
    return (responses ?? []).map(response => toTsDiagnostic(ts, response, getSourceFile));
}

function toRelatedInformation(ts, response, getSourceFile) {
    const file = response.fileName ? getSourceFile(response.fileName) : undefined;
    const located = file !== undefined && response.pos >= 0;
    return {
        file,
        start: located ? response.pos : undefined,
        length: located ? Math.max(response.end - response.pos, 0) : undefined,
        messageText: toMessageText(ts, response),
        category: response.category,
        code: response.code,
    };
}

// A response with a chain is the head of a TS 6.0 DiagnosticMessageChain; its `messageChain`
// entries are the chain's `next`.
function toMessageText(ts, response) {
    if (!response.messageChain?.length) {
        return getText(ts, response);
    }
    return toMessageChain(ts, response);
}

// TS 6.0: src/compiler/utilities.ts chainDiagnosticMessages
function toMessageChain(ts, response) {
    return {
        messageText: getText(ts, response),
        category: response.category,
        code: response.code,
        next: response.messageChain?.length ? response.messageChain.map(next => toMessageChain(ts, next)) : undefined,
    };
}

function getText(ts, response) {
    if (TS6_TEXT_CODES.has(response.code)) {
        return getTs6Message(ts, response.code)?.message ?? response.text;
    }
    return response.text;
}

function getTs6Message(ts, code) {
    let messages = messagesByCode.get(ts);
    if (messages === undefined) {
        messages = new Map();
        for (const message of Object.values(ts.Diagnostics)) {
            if (!messages.has(message.code)) {
                messages.set(message.code, message);
            }
        }
        messagesByCode.set(ts, messages);
    }
    return messages.get(code);
}

module.exports = {
    toTsDiagnostic,
    toTsDiagnostics,
    getTs6Message,
};
