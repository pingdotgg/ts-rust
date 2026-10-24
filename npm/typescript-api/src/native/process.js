"use strict";

// The sync API protocol with a `tsgo --api` child process, over its stdin and stdout (a named pipe
// on Windows), read and written synchronously. It follows TS7's src/api/syncChannel.ts
// (microsoft/TypeScript, Apache-2.0). tsc-rs uses it to check the in-process channel against the
// real process (TSRS_API_CHANNEL=process).

const childProcess = require("node:child_process");
const fs = require("node:fs");

const MSG_REQUEST = 1;
const MSG_CALL_RESPONSE = 2;
const MSG_CALL_ERROR = 3;
const MSG_RESPONSE = 4;
const MSG_ERROR = 5;
const MSG_CALL = 6;

const FIXARRAY3 = 0x93;
const UINT8 = 0xcc;
const BIN8 = 0xc4;
const BIN16 = 0xc5;
const BIN32 = 0xc6;

const EMPTY = Buffer.alloc(0);
const sleepCell = new Int32Array(new SharedArrayBuffer(4));
const liveChildren = new Set();

process.on("exit", () => {
    for (const child of liveChildren) {
        try {
            child.kill();
        }
        catch {
            // The child may have exited already.
        }
    }
});

class ProcessTransport {
    constructor(exe, args) {
        if (process.platform === "win32") {
            const pipePath = `\\\\.\\pipe\\tsrs-sync-${process.pid}-${Date.now()}`;
            this.child = childProcess.spawn(exe, [...args, "--pipe", pipePath], { stdio: ["ignore", "ignore", "inherit"] });
            this.readFd = this.writeFd = this.pipeFd = openNamedPipe(this.child, pipePath);
        }
        else {
            this.child = childProcess.spawn(exe, args, { stdio: ["pipe", "pipe", "inherit"] });
            const stdout = this.child.stdout;
            const stdin = this.child.stdin;
            this.readFd = stdout._handle?.fd;
            this.writeFd = stdin._handle?.fd;
            if (!(this.readFd >= 0 && this.writeFd >= 0)) {
                this.child.kill();
                throw new Error("cannot get the pipe descriptors of the tsgo --api process");
            }
            // node --test makes pipes non-blocking; reads must block.
            stdout._handle.setBlocking?.(true);
            stdin._handle.setBlocking?.(true);
            stdout.pause();
            stdout.unref();
            stdin.unref();
        }
        liveChildren.add(this.child);
        this.child.unref();
        this.readBuf = Buffer.allocUnsafe(65536);
        this.readPos = 0;
        this.readLen = 0;
        this.writeBuf = Buffer.allocUnsafe(65536);
        this.header = Buffer.allocUnsafe(4);
        this.depth = 0;
        this.closePending = false;
    }

    requestSync(method, payload, onCall) {
        return this.request(method, payload, onCall).toString("utf8");
    }

    requestBinarySync(method, payload, onCall) {
        return this.request(method, payload, onCall);
    }

    // Inside a callback, the outer request closes the channel when it returns.
    close() {
        if (this.depth > 0) {
            this.closePending = true;
            return;
        }
        liveChildren.delete(this.child);
        try {
            if (this.pipeFd !== undefined) {
                fs.closeSync(this.pipeFd);
                this.pipeFd = undefined;
            }
            this.child.stdout?.destroy();
            this.child.stdin?.destroy();
            this.child.kill();
        }
        catch {
            // The child may have exited already.
        }
        this.readFd = -1;
        this.writeFd = -1;
    }

    request(method, payload, onCall) {
        if (this.readFd < 0) {
            throw new Error("the tsc-rs API channel is closed");
        }
        this.depth++;
        try {
            return this.exchange(method, payload, onCall);
        }
        finally {
            if (--this.depth === 0 && this.closePending) {
                this.closePending = false;
                this.close();
            }
        }
    }

    exchange(method, payload, onCall) {
        const methodBuf = Buffer.from(method, "utf8");
        this.writeFrame(MSG_REQUEST, methodBuf, payload);
        let failure;
        for (;;) {
            const [type, name, data] = this.readFrame();
            if ((type === MSG_RESPONSE || type === MSG_ERROR) && !name.equals(methodBuf)) {
                throw new Error(`name mismatch for response: expected \`${method}\`, got \`${name.toString("utf8")}\``);
            }
            if (type === MSG_RESPONSE) {
                if (failure) throw failure.error;
                return data;
            }
            if (type === MSG_ERROR) {
                if (failure) throw failure.error;
                throw new Error(data.toString("utf8"));
            }
            if (type !== MSG_CALL) {
                throw new Error(`Invalid message type from the tsc-rs API session: ${type}`);
            }
            let answer;
            try {
                answer = onCall(name.toString("utf8"), data.toString("utf8"));
            }
            catch (error) {
                failure ??= { error };
                const message = String(error instanceof Error ? error.message : error).trim();
                this.writeFrame(MSG_CALL_ERROR, name, message);
                continue;
            }
            this.writeFrame(MSG_CALL_RESPONSE, name, answer);
        }
    }

    writeFrame(type, name, payload) {
        const payloadLen = typeof payload === "string" ? Buffer.byteLength(payload, "utf8") : payload.length;
        const headerLen = 2 + binHeaderSize(name.length) + name.length + binHeaderSize(payloadLen);
        let off = 0;
        this.writeBuf[off++] = FIXARRAY3;
        this.writeBuf[off++] = type;
        off = writeBinHeader(this.writeBuf, off, name.length);
        name.copy(this.writeBuf, off);
        off += name.length;
        off = writeBinHeader(this.writeBuf, off, payloadLen);
        if (headerLen + payloadLen <= this.writeBuf.length) {
            if (typeof payload === "string") {
                this.writeBuf.write(payload, off, payloadLen, "utf8");
            }
            else {
                this.writeBuf.set(payload, off);
            }
            this.writeAll(this.writeBuf, headerLen + payloadLen);
            return;
        }
        this.writeAll(this.writeBuf, headerLen);
        this.writeAll(typeof payload === "string" ? Buffer.from(payload, "utf8") : payload, payloadLen);
    }

    readFrame() {
        if (this.readByte() !== FIXARRAY3) {
            throw new Error("expected a fixed 3-element array (0x93) from the tsc-rs API session");
        }
        let type = this.readByte();
        if (type === UINT8) {
            type = this.readByte();
        }
        else if (type > 0x7f) {
            throw new Error(`expected a positive fixint or uint8 marker, received: 0x${type.toString(16)}`);
        }
        return [type, this.readBin(), this.readBin()];
    }

    readBin() {
        const marker = this.readByte();
        let size;
        if (marker === BIN8) {
            size = this.readByte();
        }
        else if (marker === BIN16) {
            this.readInto(this.header, 2);
            size = this.header.readUInt16BE(0);
        }
        else if (marker === BIN32) {
            this.readInto(this.header, 4);
            size = this.header.readUInt32BE(0);
        }
        else {
            throw new Error(`expected binary data (0xc4-0xc6), received: 0x${marker.toString(16)}`);
        }
        if (size === 0) {
            return EMPTY;
        }
        // Its own ArrayBuffer at offset 0: RemoteSourceFile views `buffer.buffer` by absolute offsets.
        const out = Buffer.allocUnsafeSlow(size);
        this.readInto(out, size);
        return out;
    }

    readByte() {
        if (this.readPos >= this.readLen) {
            this.fill();
        }
        return this.readBuf[this.readPos++];
    }

    readInto(out, length) {
        let pos = 0;
        while (pos < length) {
            const available = this.readLen - this.readPos;
            if (available > 0) {
                const n = Math.min(available, length - pos);
                this.readBuf.copy(out, pos, this.readPos, this.readPos + n);
                this.readPos += n;
                pos += n;
            }
            else if (length - pos >= this.readBuf.length) {
                pos += this.readSome(out, pos, length - pos);
            }
            else {
                this.fill();
            }
        }
    }

    fill() {
        this.readPos = 0;
        this.readLen = this.readSome(this.readBuf, 0, this.readBuf.length);
    }

    readSome(buffer, offset, length) {
        for (;;) {
            try {
                const n = fs.readSync(this.readFd, buffer, offset, length, null);
                if (n === 0) {
                    throw this.eofError();
                }
                return n;
            }
            catch (error) {
                if (!isWouldBlock(error)) throw error;
                Atomics.wait(sleepCell, 0, 0, 1);
            }
        }
    }

    writeAll(data, length) {
        let pos = 0;
        while (pos < length) {
            try {
                pos += fs.writeSync(this.writeFd, data, pos, length - pos);
            }
            catch (error) {
                if (!isWouldBlock(error)) throw error;
                Atomics.wait(sleepCell, 0, 0, 1);
            }
        }
    }

    eofError() {
        const { exitCode, signalCode } = this.child;
        const detail = signalCode ? `killed by signal ${signalCode}` : exitCode !== null ? `exited with code ${exitCode}` : "unknown reason";
        return new Error(`Unexpected EOF while reading from child process (${detail})`);
    }
}

function openNamedPipe(child, pipePath) {
    for (let i = 0; i < 500; i++) {
        try {
            return fs.openSync(pipePath, "r+");
        }
        catch {
            if (child.exitCode !== null) {
                throw new Error(`the tsgo --api process exited with code ${child.exitCode} before its pipe was ready`);
            }
            Atomics.wait(sleepCell, 0, 0, 10);
        }
    }
    child.kill();
    throw new Error("timed out connecting to the tsgo --api named pipe");
}

function isWouldBlock(error) {
    return error?.code === "EAGAIN" || error?.code === "EWOULDBLOCK";
}

function binHeaderSize(length) {
    return length < 0x100 ? 2 : length < 0x10000 ? 3 : 5;
}

function writeBinHeader(buffer, offset, length) {
    if (length < 0x100) {
        buffer[offset] = BIN8;
        buffer[offset + 1] = length;
        return offset + 2;
    }
    if (length < 0x10000) {
        buffer[offset] = BIN16;
        buffer.writeUInt16BE(length, offset + 1);
        return offset + 3;
    }
    buffer[offset] = BIN32;
    buffer.writeUInt32BE(length, offset + 1);
    return offset + 5;
}

module.exports = { ProcessTransport };
