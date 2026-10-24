// node:test reporter: one JSON line per test (file, full name, kind, status, ms, failure message).

import path from "node:path";

export default async function* reporter(source) {
    const stacks = new Map();
    const base = process.cwd();
    for await (const { type, data } of source) {
        if (type === "test:start") {
            const stack = stacks.get(data.file) ?? [];
            stack.length = data.nesting;
            stack.push(data.name);
            stacks.set(data.file, stack);
            continue;
        }
        if (type !== "test:pass" && type !== "test:fail") {
            continue;
        }
        const names = [...(stacks.get(data.file) ?? []).slice(0, data.nesting), data.name];
        let status = type === "test:pass" ? "pass" : "fail";
        if (data.skip !== undefined) status = "skip";
        if (data.todo !== undefined) status = "todo";
        const error = data.details?.error;
        const cause = error?.cause ?? error;
        yield JSON.stringify({
            file: data.file ? path.relative(base, data.file) : null,
            name: names.join(" > "),
            kind: data.details?.type ?? "test",
            status,
            ms: Math.round(data.details?.duration_ms ?? 0),
            message: cause ? String(cause.message ?? cause).slice(0, 4000) : undefined,
        }) + "\n";
    }
}
