// Writes the linux-x64 npm package set in Go's layout at the pin: `typescript` (the JS
// launcher and the JS API) and `@typescript/typescript-linux-x64` (the native tsc and the
// lib files). scripts/goport/npm-pack.sh runs it; see there for the usage.
//
// It follows Herebyfile.mjs `buildNativePreviewPackages` of the Go checkout with the
// release profile "typescript" (publishAsTypescript) for the current platform only, as
// Go's local build does. With --native-bin it adds one thing to the main package: the
// postinstall npm/install.js (as lib/install.js), which rewrites bin/tsc on POSIX as a sh and JS
// polyglot: sh runs the native tsc without Node, and Node runs Go's launcher (see there).
//
// PORT: not in Go. Two more names write the port's own package sets from the same input. Both
// have the same platform packages `@tsc-rs/<os>-<arch>`, one per --exe (byte for byte: the release
// publishes them once), and the port's metadata, LICENSE and NOTICE.txt. The default name
// `typescript` is Go's set.
//   `--name tsc-rs`: the main package `tsc-rs` (bin `tsc-rs`, npm/getExePath.js as
//     lib/getExePath.js, npm/tsc-rs-readme.md). It does not clash with a `typescript` install.
//   `--name @tsc-rs/typescript`: the drop-in for `typescript` ("typescript":
//     "npm:@tsc-rs/typescript@<v>"), npm/typescript-readme.md. It is Go's main package (bin `tsc`,
//     Go's lib/getExePath.js unchanged) with two changes. Its optionalDependencies alias the
//     platform packages under Go's names, "@typescript/typescript-<os>-<arch>":
//     "npm:@tsc-rs/<os>-<arch>@<v>", so Go's getExePath.js and the VS Code extension's
//     resolveTsdkPathToExe find the tsc-rs tsc under any install name. Its lib/version.cjs exports
//     the TypeScript version (see versionCjs below). Only at layout "typescript".
// Both have their own --package-version. The tsc keeps reporting the TypeScript version
// (--version): the compiler matches `typesVersions` against it, so a tsc that reported 0.1.0 would
// pick the typings that packages keep for TypeScript 5.6 and older. The main package records it as
// `tscVersion`, for the postinstall check.
//
// usage: node npm/pack.mjs --layout <typescript|typescript-go> --go-dir <dir> --exe <tsc>...
//          --libs <dir> --dist <dir> --version <v> --git-head <sha> --out <dir> [--native-bin]
//          [--name <typescript|tsc-rs|@tsc-rs/typescript>] [--package-version <v>]
//
// --version is the version the tsc reports. --package-version is the npm version (default
// --version).
//
// --exe is <os>-<arch>=<tsc> (Node's process.platform and process.arch, for example
// darwin-arm64=<path>), or a plain <tsc> for linux-x64. Repeat it for more platforms. It is not
// run here, so a cross-built tsc works.
//
// --layout is the pin layout (scripts/upstream/pin.py). "typescript" (microsoft/TypeScript, pin N
// on): --go-dir is <repo>/tsc, the input is <repo>/packages/typescript (it already has bin/tsc and
// lib/tsc.js), and LICENSE.txt and NOTICE.txt are at <repo>. "typescript-go": the input is
// <go-dir>/_packages/native-preview (bin/tsgo, lib/tsgo.js), with LICENSE and NOTICE.txt at <go-dir>.
//
// Output: one dir per package in --out: `typescript` and `typescript-<os>-<arch>` (Go's set), or
// `tsc-rs` or `tsc-rs-typescript`, and `tsc-rs-<os>-<arch>`.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const { values: args } = parseArgs({
    options: {
        layout: { type: "string" },
        "go-dir": { type: "string" },
        exe: { type: "string", multiple: true },
        name: { type: "string", default: "typescript" },
        "package-version": { type: "string" },
        libs: { type: "string" },
        dist: { type: "string" },
        version: { type: "string" },
        "git-head": { type: "string" },
        out: { type: "string" },
        "native-bin": { type: "boolean", default: false },
    },
    strict: true,
});
for (const name of ["layout", "go-dir", "exe", "libs", "dist", "version", "git-head", "out"]) {
    if (!args[name]) throw new Error(`missing --${name}`);
}
const { layout, "go-dir": goDir, libs, dist, version, "git-head": gitHead, out, name } = args;
if (layout !== "typescript" && layout !== "typescript-go") throw new Error(`unknown --layout ${layout}`);
// The port's sets, by main package name: the bin and the README.
const portSets = {
    "tsc-rs": { binName: "tsc-rs", readme: "tsc-rs-readme.md" },
    "@tsc-rs/typescript": { binName: "tsc", readme: "typescript-readme.md" },
};
const asTypescript = name === "typescript";
const portSet = portSets[name];
if (!asTypescript && !portSet) throw new Error(`unknown --name ${name}`);
// The drop-in keeps Go's main package: Go's name for the platform packages (aliased) and Go's
// getExePath.js.
const dropIn = name === "@tsc-rs/typescript";
const binName = asTypescript ? "tsc" : portSet.binName;
const packageVersion = args["package-version"] ?? version;
const atN = layout === "typescript";
if (dropIn && !atN) throw new Error(`--name ${name} needs --layout typescript`);
const root = atN ? path.dirname(goDir) : goDir;
const inputDir = atN ? path.join(root, "packages", "typescript") : path.join(goDir, "_packages", "native-preview");
const tsLicenseFile = path.join(root, atN ? "LICENSE.txt" : "LICENSE");
const tsNoticeFile = path.join(root, "NOTICE.txt");
// The port's output dirs are named as npm pack names the tarball: @scope/name is scope-name.
const dirName = packageName => packageName.replace(/^@/, "").replace("/", "-");

// Go: Herebyfile.mjs getPlatforms. PORT: the platforms are the --exe values, not only the current
// platform as in Go's local build.
const platforms = args.exe.map(value => {
    const match = value.match(/^([a-z0-9]+)-([a-z0-9]+)=(.+)$/);
    const [nodeOs, nodeArch, exe] = match ? match.slice(1) : ["linux", "x64", value];
    const goName = `@typescript/typescript-${nodeOs}-${nodeArch}`;
    return {
        nodeOs,
        nodeArch,
        exe,
        packageName: asTypescript ? goName : `@tsc-rs/${nodeOs}-${nodeArch}`,
        // The name the main package depends on: Go's name in Go's set and the drop-in.
        dependencyName: asTypescript || dropIn ? goName : `@tsc-rs/${nodeOs}-${nodeArch}`,
    };
});

// Go: Herebyfile.mjs getPublishTag (publishAsTypescript). Go's nativePreviewReleaseVersion is
// undefined at both layouts, so a version with no dev, beta or rc part is refused.
// PORT: the port's sets have no such rule; `npm publish --tag` picks their tag.
function publishTag() {
    const match = version.match(/-(dev|beta|rc)(?:[.-]|$)/);
    if (match?.[1]) return match[1] === "dev" ? "next" : match[1];
    throw new Error(`Refusing to publish 'typescript' with the latest tag from non-release version ${version}.`);
}

// Go: Herebyfile.mjs stripSourceConditions and stripConditionsFromValue
function stripConditions(value) {
    if (value == null || typeof value !== "object") return value;
    delete value["@typescript/source"];
    for (const key of Object.keys(value)) value[key] = stripConditions(value[key]);
    const keys = Object.keys(value);
    return keys.length === 1 && keys[0] === "default" ? value.default : value;
}

const writeJson = (file, value) => fs.writeFileSync(file, JSON.stringify(value, undefined, 4));

// Go: Herebyfile.mjs buildNativePreviewPackages, inputPackageJson with publishAsTypescript.
const input = JSON.parse(fs.readFileSync(path.join(inputDir, "package.json"), "utf8"));
input.version = packageVersion;
delete input.private;
input.files = [...new Set([...(input.files ?? []), "NOTICE.txt"])];
input.bin = { [binName]: `./bin/${binName}` };
if (asTypescript) {
    input.description = "TypeScript is a language for application scale JavaScript development";
    input.homepage = "https://www.typescriptlang.org/";
    input.keywords = ["TypeScript", "Microsoft", "compiler", "language", "javascript"];
    input.bugs = { url: "https://github.com/microsoft/TypeScript/issues" };
    input.repository = { type: "git", url: "https://github.com/microsoft/TypeScript.git" };
}
else {
    // PORT: the port's own packages. The port is MIT. The JS launcher, the JS API and the lib files
    // are TypeScript's, unchanged (Apache-2.0). See portNotice below. The platform packages copy
    // these fields, so both port sets give the same platform packages.
    input.license = "MIT AND Apache-2.0";
    input.author = "Theo Browne";
    input.description = "A Rust port of the TypeScript 7 compiler";
    input.keywords = ["typescript", "tsc", "compiler", "rust"];
    // npm trusted publishing (the release workflow) needs repository.url to name the repo that
    // publishes. The platform packages copy it.
    input.homepage = "https://github.com/pingdotgg/ts-rust";
    input.bugs = { url: "https://github.com/pingdotgg/ts-rust/issues" };
    input.repository = { type: "git", url: "git+https://github.com/pingdotgg/ts-rust.git" };
}
delete input.scripts;
delete input.devDependencies;
for (const field of ["exports", "imports"]) input[field] = stripConditions(input[field]);
input.gitHead = gitHead;
input.publishConfig = asTypescript ? { access: "public", tag: publishTag() } : { access: "public" };

fs.rmSync(out, { recursive: true, force: true });

// The main package (`typescript`, `tsc-rs` or `@tsc-rs/typescript`).
const mainDir = path.join(out, dirName(name));
const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.dirname(here);

// PORT: the port's sets ship the port's MIT LICENSE, and a NOTICE.txt with the licenses and
// notices of the code it ports or copies (the repo's NOTICE.md and licenses/). Go's set keeps
// TypeScript's.
function portNotice() {
    const part = (title, file) => [`${"=".repeat(78)}\n${title}\n${"=".repeat(78)}\n`, fs.readFileSync(file, "utf8")];
    return [
        fs.readFileSync(path.join(repoRoot, "NOTICE.md"), "utf8"),
        ...part("TypeScript license", tsLicenseFile),
        ...part("TypeScript third-party notices", tsNoticeFile),
        ...part("Go license", path.join(repoRoot, "licenses", "Go-LICENSE.txt")),
        ...part("Unicode license", path.join(repoRoot, "licenses", "Unicode-LICENSE.txt")),
        ...part("memfast license", path.join(repoRoot, "licenses", "memfast-LICENSE.txt")),
    ].join("\n");
}
function writeLicense(dir) {
    if (asTypescript) {
        fs.copyFileSync(tsLicenseFile, path.join(dir, "LICENSE"));
        fs.copyFileSync(tsNoticeFile, path.join(dir, "NOTICE.txt"));
    }
    else {
        fs.copyFileSync(path.join(repoRoot, "LICENSE"), path.join(dir, "LICENSE"));
        fs.writeFileSync(path.join(dir, "NOTICE.txt"), portNotice());
    }
}
const mainPackage = {
    ...input,
    name,
    // PORT: the drop-in's own description. Its platform packages keep the port's.
    ...(dropIn ? { description: "A Rust port of the TypeScript 7 compiler, as a drop-in for the typescript package" } : {}),
    ...(asTypescript ? {} : { tscVersion: version }),
    // PORT: the drop-in aliases the port's platform packages under Go's names.
    optionalDependencies: Object.fromEntries(
        platforms.map(p => [p.dependencyName, dropIn ? `npm:${p.packageName}@${packageVersion}` : packageVersion]),
    ),
};
if (atN) {
    // Go copies the whole input but node_modules and dist; its filter sees the path from the repo.
    fs.cpSync(inputDir, mainDir, {
        recursive: true,
        filter: src => {
            const p = path.posix.join("packages/typescript", path.relative(inputDir, src).split(path.sep).join("/"));
            return !p.endsWith("/node_modules") && !p.includes("/dist");
        },
    });
}
else {
    for (const entry of ["bin", "lib", "vendor"]) {
        fs.cpSync(path.join(inputDir, entry), path.join(mainDir, entry), { recursive: true });
    }
    fs.rmSync(path.join(mainDir, "bin", "tsgo"));
    fs.renameSync(path.join(mainDir, "lib", "tsgo.js"), path.join(mainDir, "lib", "tsc.js"));
}
fs.cpSync(dist, path.join(mainDir, "dist"), { recursive: true });
fs.rmSync(path.join(mainDir, "bin"), { recursive: true, force: true });
fs.mkdirSync(path.join(mainDir, "bin"));
fs.writeFileSync(path.join(mainDir, "bin", binName), '#!/usr/bin/env node\nimport "../lib/tsc.js";\n');
fs.chmodSync(path.join(mainDir, "bin", binName), 0o755);
if (asTypescript) {
    fs.copyFileSync(path.join(inputDir, "typescript-package-readme.md"), path.join(mainDir, "README.md"));
}
else {
    fs.copyFileSync(path.join(here, portSet.readme), path.join(mainDir, "README.md"));
}
if (name === "tsc-rs") {
    fs.copyFileSync(path.join(here, "getExePath.js"), path.join(mainDir, "lib", "getExePath.js"));
}
if (dropIn) {
    fs.writeFileSync(path.join(mainDir, "lib", "version.cjs"), versionCjs(fs.readFileSync(path.join(mainDir, "lib", "version.cjs"), "utf8")));
}
if (args["native-bin"]) {
    // PORT: not in Go. The native bin on POSIX (npm/install.js).
    mainPackage.scripts = { postinstall: "node lib/install.js" };
    fs.copyFileSync(path.join(here, "install.js"), path.join(mainDir, "lib", "install.js"));
}
writeJson(path.join(mainDir, "package.json"), mainPackage);
writeLicense(mainDir);

// PORT: the drop-in's lib/version.cjs. Go's exports package.json `version`, which is the TypeScript
// version in Go's package. The drop-in's npm version is the port's own (0.8.0), and tools compare
// require("typescript").version with TypeScript versions, so it exports `tscVersion`, the version
// the tsc reports. Go's versionMajorMinor stays; it must be the major.minor of that version.
function versionCjs(go) {
    // Go's file: the version line, `exports.version = version;` and versionMajorMinor (CRLF in
    // some checkouts).
    const lines = go.split(/\r?\n/);
    const majorMinor = lines[2]?.match(/^exports\.versionMajorMinor = "(\d+\.\d+)";$/)?.[1];
    if (lines[0] !== 'const { version } = require("../package.json");' || lines[1] !== "exports.version = version;" || !majorMinor) {
        throw new Error(`unexpected lib/version.cjs at the pin:\n${go}`);
    }
    if (!version.startsWith(`${majorMinor}.`)) throw new Error(`the tsc reports ${version}, not ${majorMinor}.x (lib/version.cjs)`);
    return [
        "// PORT: tsc-rs. package.json `version` is the tsc-rs npm version; `tscVersion` is the TypeScript",
        "// version that the tsc reports (npm/pack.mjs versionCjs).",
        'const { tscVersion: version } = require("../package.json");',
        ...lines.slice(1),
    ].join("\n");
}

// The platform packages: the lib files and the native tsc in lib/.
for (const { nodeOs, nodeArch, exe, packageName } of platforms) {
    const platformDir = path.join(out, asTypescript ? `typescript-${nodeOs}-${nodeArch}` : dirName(packageName));
    const platformPackage = {
        ...input,
        bin: undefined,
        files: ["lib", "NOTICE.txt"],
        imports: undefined,
        dependencies: undefined,
        name: packageName,
        os: [nodeOs],
        cpu: [nodeArch],
        exports: { "./package.json": "./package.json" },
    };
    fs.cpSync(libs, path.join(platformDir, "lib"), { recursive: true });
    // getExePath.js looks for lib/tsc.exe on Windows.
    const exeName = nodeOs === "win32" ? "tsc.exe" : "tsc";
    fs.copyFileSync(exe, path.join(platformDir, "lib", exeName));
    fs.chmodSync(path.join(platformDir, "lib", exeName), 0o755);
    writeJson(path.join(platformDir, "package.json"), platformPackage);
    writeLicense(platformDir);
    // PORT: the port's platform packages serve both port sets.
    const mains = asTypescript ? ["typescript"] : Object.keys(portSets);
    fs.writeFileSync(
        path.join(platformDir, "README.md"),
        [
            `# \`${packageName}\``,
            "",
            `This package provides ${nodeOs}-${nodeArch} support for ${mains.map(m => `[${m}](https://www.npmjs.com/package/${m})`).join(" and ")}.`,
        ].join("\n") + "\n",
    );
}
