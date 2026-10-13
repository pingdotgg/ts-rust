// Builds the tsc-rs Node-API addon (crates/ts_napi) and copies it to
// native/ts_napi.<platform>-<arch>.node, where src/native/addon.js loads it.
//
// usage: node scripts/build-native.mjs [--no-build] [--profile <name>]
//        node scripts/build-native.mjs help
// --no-build copies the library that is already built. --profile picks a Cargo profile (default
// release). On a Linux host with a systemd user session the build runs through
// scripts/run-cargo-capped.sh (TS_CARGO_JOBS defaults to 6 here); on CI runners (CI is set), which
// have systemd-run but no user session, and elsewhere it runs cargo directly. CARGO_TARGET_DIR is
// honored when it is set.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const packageDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = path.resolve(packageDir, "../..");
const nativeDir = path.join(packageDir, "native");

function usage() {
    const lines = fs.readFileSync(fileURLToPath(import.meta.url), "utf8").split("\n");
    const header = lines.slice(0, lines.findIndex(line => !line.startsWith("//")));
    return header.map(line => line.replace(/^\/\/ ?/, "")).join("\n");
}

function parseArgs(argv) {
    const options = { build: true, profile: "release" };
    for (let i = 0; i < argv.length; i++) {
        const arg = argv[i];
        if (arg === "help" || arg === "--help" || arg === "-h") {
            console.log(usage());
            process.exit(0);
        }
        else if (arg === "--no-build") {
            options.build = false;
        }
        else if (arg === "--profile") {
            options.profile = argv[++i];
        }
        else {
            throw new Error(`unknown argument: ${arg}\n\n${usage()}`);
        }
    }
    return options;
}

function build(profile) {
    const profileArgs = profile === "release" ? ["--release"] : ["--profile", profile];
    const cargoArgs = ["build", ...profileArgs, "-p", "ts_napi"];
    const env = { ...process.env, TS_CARGO_JOBS: process.env.TS_CARGO_JOBS ?? "6" };
    const capped = process.platform === "linux" && !process.env.CI && hasUserSession();
    const result = capped
        ? spawnSync("bash", [path.join(repoDir, "scripts", "run-cargo-capped.sh"), ...cargoArgs], { cwd: repoDir, env, stdio: "inherit" })
        : spawnSync("cargo", cargoArgs, { cwd: repoDir, env, stdio: "inherit" });
    if (result.status !== 0) {
        throw new Error(`cargo ${cargoArgs.join(" ")} failed (${result.error?.message ?? `exit ${result.status}`})`);
    }
}

// scripts/run-cargo-capped.sh runs the build in a `systemd-run --user --scope`.
function hasUserSession() {
    return spawnSync("systemd-run", ["--user", "--scope", "--quiet", "--collect", "true"], { stdio: "ignore" }).status === 0;
}

function libraryName() {
    switch (process.platform) {
        case "win32":
            return "ts_napi.dll";
        case "darwin":
            return "libts_napi.dylib";
        default:
            return "libts_napi.so";
    }
}

// A rename keeps the file that running processes have mapped intact; a copy over it would not.
function install(profile) {
    const targetDir = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(repoDir, "target");
    const library = path.join(targetDir, profile === "dev" ? "debug" : profile, libraryName());
    if (!fs.existsSync(library)) {
        throw new Error(`${library} is missing: build crates/ts_napi first`);
    }
    fs.mkdirSync(nativeDir, { recursive: true });
    const destination = path.join(nativeDir, `ts_napi.${process.platform}-${process.arch}.node`);
    const temporary = `${destination}.${process.pid}.tmp`;
    fs.copyFileSync(library, temporary);
    fs.renameSync(temporary, destination);
    return destination;
}

const options = parseArgs(process.argv.slice(2));
if (options.build) {
    build(options.profile);
}
console.log(`wrote ${path.relative(process.cwd(), install(options.profile))}`);
