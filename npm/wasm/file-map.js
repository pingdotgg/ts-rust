/** Snapshots a structural readonly map or a record of project files. */
export function fileEntries(files = {}) {
    const entries = typeof files?.[Symbol.iterator] === "function"
        ? Array.from(files, ([path, text]) => [path, text])
        : Object.entries(files);
    for (const [path, text] of entries) {
        if (typeof path !== "string" || !path.startsWith("/") || typeof text !== "string") {
            throw new TypeError("files must map absolute paths to text");
        }
    }
    return entries;
}
