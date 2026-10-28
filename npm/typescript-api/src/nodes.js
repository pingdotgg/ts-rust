"use strict";

// The binary syntax tree that tsc-rs sends for a source file (protocol 9, the layout of
// crates/ts_goport/src/api/encoder/encoder.rs). Only what the JS side needs is read: the file
// text, the file's path key and each node's kind and UTF-16 range, so that a TS 6.0 node parsed
// in JS maps to the tsc-rs node handle "<index>.<kind>.<path>" and back.

const headerStringTableOffsets = 24;
const headerStringTable = 28;
const headerExtendedData = 32;
const headerNodes = 40;
const nodeSize = 28;
const nodeListKind = 0xFFFFFFFF;
const extendedDataMask = 0x00FFFFFF;
const sourceFileText = 0;
const sourceFileFileName = 4;
const sourceFilePath = 8;

const utf8 = new TextDecoder("utf-8");

class NativeSourceFile {
    constructor(bytes) {
        this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        this.bytes = bytes;
        this.stringOffsets = this.view.getUint32(headerStringTableOffsets, true);
        this.strings = this.view.getUint32(headerStringTable, true);
        this.extendedData = this.view.getUint32(headerExtendedData, true);
        this.nodes = this.view.getUint32(headerNodes, true);
        this.nodeCount = (bytes.byteLength - this.nodes) / nodeSize;
        this.indexByRange = undefined;
        const sourceFileData = this.extendedData + (this.field(1, 20) & extendedDataMask);
        this.text = this.string(this.view.getUint32(sourceFileData + sourceFileText, true));
        this.fileName = this.string(this.view.getUint32(sourceFileData + sourceFileFileName, true));
        this.path = this.string(this.view.getUint32(sourceFileData + sourceFilePath, true));
    }

    field(index, offset) {
        return this.view.getUint32(this.nodes + index * nodeSize + offset, true);
    }

    kind(index) {
        return this.field(index, 0);
    }

    pos(index) {
        return this.field(index, 4);
    }

    end(index) {
        return this.field(index, 8);
    }

    string(index) {
        const start = this.view.getUint32(this.stringOffsets + index * 4, true);
        const end = this.view.getUint32(this.stringOffsets + (index + 1) * 4, true);
        return decodeWtf8(this.bytes.subarray(this.strings + start, this.strings + end));
    }

    // The first node, in tree order, with this kind and range.
    indexOf(kind, pos, end) {
        if (this.indexByRange === undefined) {
            this.indexByRange = new Map();
            for (let index = 1; index < this.nodeCount; index++) {
                const nodeKind = this.kind(index);
                if (nodeKind === nodeListKind) {
                    continue;
                }
                const key = rangeKey(nodeKind, this.pos(index), this.end(index));
                if (!this.indexByRange.has(key)) {
                    this.indexByRange.set(key, index);
                }
            }
        }
        return this.indexByRange.get(rangeKey(kind, pos, end));
    }

    handle(index) {
        return `${index}.${this.kind(index)}.${this.path}`;
    }
}

function rangeKey(kind, pos, end) {
    return `${kind}:${pos}:${end}`;
}

// Strings are WTF-8: UTF-8 that can also hold lone UTF-16 surrogates (0xED 0xA0-0xBF 0x80-0xBF).
function decodeWtf8(bytes) {
    if (!bytes.includes(0xED)) {
        return utf8.decode(bytes);
    }
    let text = "";
    let start = 0;
    for (let index = 0; index + 2 < bytes.length; index++) {
        if (bytes[index] !== 0xED || bytes[index + 1] < 0xA0 || bytes[index + 1] > 0xBF) {
            continue;
        }
        text += utf8.decode(bytes.subarray(start, index));
        text += String.fromCharCode(0xD000 | ((bytes[index + 1] & 0x3F) << 6) | (bytes[index + 2] & 0x3F));
        index += 2;
        start = index + 1;
    }
    return text + utf8.decode(bytes.subarray(start));
}

function parseHandle(handle) {
    const firstDot = handle.indexOf(".");
    const secondDot = handle.indexOf(".", firstDot + 1);
    return {
        index: Number(handle.slice(0, firstDot)),
        kind: Number(handle.slice(firstDot + 1, secondDot)),
        path: handle.slice(secondDot + 1),
    };
}

module.exports = { NativeSourceFile, parseHandle };
