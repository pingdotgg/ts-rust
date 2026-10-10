import { createHostServiceChannel } from "../classic-host.js";

export function fixture() {
    const text = new Map([["/a.ts", 'import { value } from "./b"; export const result: number = value;'], ["/b.ts", "export const value = 1;"]]);
    const versions = new Map([["/a.ts", "1"], ["/b.ts", "1"]]);
    const reads = new Map();
    const retained = [];
    let roots = ["/a.ts"];
    let settings = { strict: true };
    let failSnapshot = false;
    let projectVersion = "1";
    const host = {
        getCompilationSettings: () => settings,
        getProjectVersion: () => projectVersion,
        getScriptFileNames: () => roots,
        getCurrentDirectory: () => "/",
        getDefaultLibFileName: () => "/lib.d.ts",
        getScriptVersion: path => versions.get(path) ?? "0",
        getScriptSnapshot(path) {
            if (failSnapshot) throw new Error("snapshot failure");
            reads.set(path, (reads.get(path) ?? 0) + 1);
            const source = text.get(path);
            if (source === undefined) return undefined;
            const snapshot = { getLength: () => source.length, getText: (start, end) => source.slice(start, end), getChangeRange: () => undefined };
            retained.push(snapshot);
            return snapshot;
        },
        readFile: path => text.get(path),
        fileExists: path => text.has(path),
    };
    return { host, text, versions, reads, retained,
        setRoots: value => roots = value, setSettings: value => settings = value,
        failSnapshot: value => failSnapshot = value,
        setProjectVersion: value => projectVersion = value,
    };
}

export function argumentsForSettings(settings, names) {
    return ["--noLib", "--strict", String(settings.strict)].concat(names);
}

function check(condition, message) {
    if (!condition) throw new Error(message);
}

export function verifyClassicRuntime(module) {
    const project = fixture();
    const service = createHostServiceChannel(module, project.host, argumentsForSettings);
    const secondProject = fixture();
    secondProject.text.set("/b.ts", 'export const value = "other";');
    const second = createHostServiceChannel(module, secondProject.host, argumentsForSettings);
    const diagnostics = target => target.request("textDocument/diagnostic", { textDocument: { uri: "file:///a.ts" } });
    try {
        check(!service.then, "creation must return synchronously");
        const initial = diagnostics(service);
        check(!initial.then && Array.isArray(initial.items), "request must return synchronously");
        check(!initial.items.some(item => item.code === 2322), "initial snapshot must typecheck");
        const reads = project.reads.get("/b.ts");
        check(reads > 0, "an imported file must read the supplied snapshot");
        diagnostics(service);
        check(project.reads.get("/b.ts") === reads, "unchanged versions must reuse snapshots");
        check(diagnostics(second).items.some(item => item.code === 2322), "separate hosts must retain their own text");
        project.text.set("/b.ts", 'export const value = "edited";');
        project.versions.set("/b.ts", "2");
        check(diagnostics(service).items.some(item => item.code === 2322), "an imported edit must reach the checker");
        project.text.set("/a.ts", "export const result: number = undefined;");
        project.versions.set("/a.ts", "2");
        check(diagnostics(service).items.some(item => item.code === 2322), "strict settings must apply");
        project.setSettings({ strict: false });
        check(!diagnostics(service).items.some(item => item.code === 2322), "settings changes must reach the checker");
        project.setRoots(["/b.ts"]);
        let removed = false;
        try { diagnostics(service); }
        catch (error) { removed = error.code === -32602; }
        check(removed, "a removed root must leave the program");
        check(service.dispose() === undefined && service.dispose() === undefined, "disposal must be synchronous and repeatable");
        return { synchronous: true, snapshots: true, versions: true, options: true, roots: true, isolation: true, disposal: true };
    } finally {
        service.dispose();
        second.dispose();
    }
}
