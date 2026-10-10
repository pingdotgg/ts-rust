import { memoryFileSystem } from "./core.js";
import { createServiceChannel } from "./service-channel.js";

const encoder = new TextEncoder();

/** Private host bridge; the classic facade supplies compiler-option translation. */
export function createHostServiceChannel(module, host, argumentsForSettings) {
    let roots = new Set();
    let snapshots = new Map();
    const files = new Map();
    const memory = memoryFileSystem(files);
    let argsKey;
    let projectVersion;
    let disposed = false;
    let synchronizing = false;
    let channel;

    function capture(path, previous) {
        const version = host.getScriptVersion(path);
        if (previous && previous.version === version) return previous;
        const snapshot = host.getScriptSnapshot(path);
        const text = snapshot === undefined ? host.readFile(path) : snapshot.getText(0, snapshot.getLength());
        return { version, snapshot, text };
    }

    function readFile(path) {
        let record = snapshots.get(path);
        if (!record) {
            record = capture(path);
            snapshots.set(path, record);
            if (record.text !== undefined) files.set(path, record.text);
        }
        return record.text;
    }

    const fs = {
        ...memory,
        readFile,
        stat(path) {
            const known = memory.stat(path);
            if (known?.isDirectory) return known;
            if (known) return { ...known, size: encoder.encode(files.get(path)).length };
            if (host.directoryExists?.(path)) return { isDirectory: true, size: 0 };
            if (!host.fileExists(path)) return undefined;
            const text = readFile(path);
            return text === undefined ? undefined : { isDirectory: false, size: encoder.encode(text).length };
        },
        realpath: host.realpath?.bind(host),
        readDirectory(path) {
            const known = memory.readDirectory(path);
            const files = host.readDirectory?.(path);
            const directories = host.getDirectories?.(path);
            if (!known && !files && !directories && !host.directoryExists?.(path)) return undefined;
            const entries = new Map((known ?? []).map(entry => [entry.name, entry]));
            const prefix = path.endsWith("/") ? path : `${path}/`;
            for (const file of files ?? []) {
                if (!file.startsWith(prefix)) continue;
                const relative = file.slice(prefix.length);
                const slash = relative.indexOf("/");
                const name = slash < 0 ? relative : relative.slice(0, slash);
                entries.set(name, { name, kind: slash < 0 ? "file" : "directory" });
            }
            for (const directory of directories ?? []) {
                const name = directory.startsWith(prefix) ? directory.slice(prefix.length) : directory;
                entries.set(name, { name, kind: "directory" });
            }
            return Array.from(entries.values());
        },
    };

    function synchronize() {
        if (synchronizing) throw new Error("language service cannot reenter host synchronization");
        synchronizing = true;
        try { return collectChanges(); }
        finally { synchronizing = false; }
    }

    function collectChanges() {
        const names = host.getScriptFileNames();
        const args = argumentsForSettings(host.getCompilationSettings(), names);
        const nextKey = JSON.stringify(args);
        const nextProjectVersion = host.getProjectVersion?.();
        const projectChanged = nextProjectVersion !== projectVersion;
        const nextRoots = new Set(names);
        const nextSnapshots = new Map();
        let changed = nextKey !== argsKey || projectChanged;
        for (const path of snapshots.keys()) {
            if (roots.has(path) && !nextRoots.has(path)) continue;
            const previous = snapshots.get(path);
            const record = capture(path, projectChanged ? undefined : previous);
            nextSnapshots.set(path, record);
            if (record !== previous) changed = true;
        }
        for (const path of names) {
            if (nextSnapshots.has(path)) continue;
            nextSnapshots.set(path, capture(path));
            changed = true;
        }
        if (!changed) return args;
        channel?.call({ action: "configure", args });
        roots = nextRoots;
        snapshots = nextSnapshots;
        argsKey = nextKey;
        projectVersion = nextProjectVersion;
        files.clear();
        for (const [path, record] of snapshots) {
            if (record.text !== undefined) files.set(path, record.text);
        }
        return args;
    }

    const args = synchronize();
    channel = createServiceChannel(module, {
        args, fs, cwd: host.getCurrentDirectory(), caseInsensitive: host.useCaseSensitiveFileNames?.() === false,
    });
    return {
        request(method, params) {
            if (disposed) throw new Error("language service is disposed");
            synchronize();
            return channel.call({ action: "request", method, params });
        },
        dispose() {
            if (synchronizing) throw new Error("language service cannot dispose during host synchronization");
            channel.dispose();
            disposed = true;
            snapshots.clear();
            files.clear();
        },
    };
}
