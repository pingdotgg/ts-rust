/** Counts real host reads while forwarding every operation to the original host. */
export async function countFileReads(run) {
    const original = WebAssembly.instantiate;
    const reads = new Map();
    WebAssembly.instantiate = async (module, imports) => {
        let instance;
        const fs = imports.ts_host.fs;
        imports.ts_host.fs = (op, ptr, length) => {
            if (op === 0) {
                const path = new TextDecoder().decode(new Uint8Array(instance.exports.memory.buffer, ptr, length));
                reads.set(path, (reads.get(path) ?? 0) + 1);
            }
            return fs(op, ptr, length);
        };
        instance = await original(module, imports);
        return instance;
    };
    try { return await run(reads); }
    finally { WebAssembly.instantiate = original; }
}
