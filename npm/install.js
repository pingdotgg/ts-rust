// Postinstall of the goport `typescript`, `tsc-rs` and `@tsc-rs/typescript` packages: lib/install.js.
// Go's package has no install script; this is the only file the port adds to Go's layout.
//
// The bin (bin/tsc, or bin/tsc-rs: the one entry of package.json `bin`) is Go's JS launcher
// (packages/typescript/bin/tsc): it starts Node, finds the platform package and execs its native
// tsc. The Node start costs about 22 ms on every run. On POSIX this script rewrites the bin as a sh
// and JS polyglot:
//
//   #!/bin/sh
//   ":" //; p=$0; ...; t="${p%/*}/../../@typescript/typescript-linux-x64/lib/tsc"; ...; exec "$t" "$@"
//   import "../lib/tsc.js";
//
// sh (the shebang, npm's .bin symlink, pnpm's shim) runs line 2, where `":" //` is the no-op `:`.
// It finds the bin's real dir from $0 and execs the native tsc by its path relative to that dir,
// so a moved project still works. Node (`node bin/tsc`, tools that start a bin with
// process.execPath) reads line 2 as a string and a comment and runs line 3, Go's launcher, as
// with Go. The exec path keeps the real path of the native tsc in the platform package, where it
// reads the lib files (Go's noembed build).
//
// The relative path is fixed at install. pnpm keeps the result of a postinstall in its store (the
// side-effects cache) and gives it to a later install of the same package in another layout
// (hoisted, isolated or the global virtual store), where the path can name no file. Then sh does
// not find an executable there and runs this file with Node (`exec node "$p"`), which runs Go's
// launcher, as with Go.
//
// To find the real dir, sh first tries npm's node_modules/.bin/tsc link, with no fork: when $0 is
// a symlink and npm's link target from its dir (../typescript/bin/tsc, under the install name: also
// for an alias, and ../@tsc-rs/typescript/bin/tsc for a scoped name) is the same file (test -ef)
// and not a symlink, that path is the bin. Else it follows $0 one link at a time, as pnpm's shim
// does (old macOS has no `readlink -f`). A readlink is a fork and an exec, about 1 ms on the
// .bin path, which runs for every npx and package.json script.
//
// Every failure keeps the JS launcher, which still works. It stays on Windows (npm's cmd
// shim runs bin/tsc with Node), under Yarn (Yarn runs bins with Node), outside a
// node_modules dir (do not rewrite a source checkout), and when the native tsc does not run
// here or reports another version than this package.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import getExePath from "#getExePath";

const pkgDir = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const pkg = JSON.parse(fs.readFileSync(path.join(pkgDir, "package.json"), "utf8"));
const binPath = path.join(pkgDir, Object.values(pkg.bin)[0]);

function keepLauncherReason() {
    if (process.platform === "win32") return "Windows";
    if (/\byarn\//.test(process.env.npm_config_user_agent ?? "")) return "Yarn";
    if (!pkgDir.split(path.sep).includes("node_modules")) return "not installed in node_modules";
    return undefined;
}

function useNativeBin() {
    if (keepLauncherReason()) return;
    const exe = getExePath();
    const out = execFileSync(exe, ["--version"], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
    // The port's packages record the version their tsc reports as tscVersion (npm/pack.mjs).
    const want = `Version ${pkg.tscVersion ?? pkg.version}`;
    if (out !== want) {
        throw new Error(`${exe} reports "${out}", not "${want}"`);
    }
    const target = path.relative(fs.realpathSync(path.dirname(binPath)), fs.realpathSync(exe));
    // npm links node_modules/.bin/<bin> to this path. The .bin dir is in the node_modules dir of
    // the package, above its scope dir when the install name is scoped.
    const parts = pkgDir.split(path.sep);
    const nodeModules = parts.slice(0, parts.lastIndexOf("node_modules") + 1).join(path.sep);
    const npmLink = path.relative(path.join(nodeModules, ".bin"), binPath);
    // The paths go in sh "..." strings on a JS line comment.
    for (const p of [target, npmLink]) {
        if (/["$`\\\n\r\u2028\u2029]/.test(p)) {
            throw new Error(`cannot quote the path ${JSON.stringify(p)}`);
        }
    }
    // p becomes a path of the bin that is not a symlink. A relative link target is relative to the
    // link's dir.
    const resolve = [
        `p=$0; g="\${p%/*}/${npmLink}"`,
        'if [ -L "$p" ] && [ ! -L "$g" ] && [ "$p" -ef "$g" ]; then p=$g',
        "else case $p in */*) ;; *) p=./$p ;; esac",
        'while [ -L "$p" ]; do t=$(readlink "$p"); case $t in /*) p=$t ;; *) p=${p%/*}/$t ;; esac; done; fi',
    ].join("; ");
    // When the native tsc is not at the path, the bin runs Go's launcher with Node (see above).
    const run = `t="\${p%/*}/${target}"; [ -x "$t" ] || exec node "$p" "$@"; exec "$t" "$@"`;
    const polyglot = `#!/bin/sh\n":" //; ${resolve}; ${run}\nimport "../lib/tsc.js";\n`;
    const tmp = `${binPath}.${process.pid}.tmp`;
    fs.rmSync(tmp, { force: true });
    try {
        fs.writeFileSync(tmp, polyglot);
        fs.chmodSync(tmp, 0o755);
        fs.renameSync(tmp, binPath);
    }
    catch (e) {
        // A failed write, chmod or rename leaves no temp file; the bin is not changed.
        try {
            fs.rmSync(tmp, { force: true });
        }
        catch {}
        throw e;
    }
}

try {
    useNativeBin();
}
catch (e) {
    console.warn(`${pkg.name}: ${path.relative(pkgDir, binPath)} keeps the Node launcher: ${e instanceof Error ? e.message : e}`);
}
