// nest.sh make-nest-new-fixture [name...]
//
// Regenerates ../fixtures/<name> for each fixture in pins.json nestNew.fixtures (default: all):
// the @nestjs/schematics application template at the pinned @nestjs/cli and @nestjs/schematics
// versions, with --skip-git --skip-install, then every dependency pinned to the exact version npm
// resolves at pins.json npmBefore, and its package-lock.json.
//
// A fixture with "nestNew" args runs `nest new <name> <args>`. A fixture with "schematic" args runs
// the schematic the way `nest new` does (the same schematics binary, `@nestjs/schematics:application
// --name=<name> <args>`); nest-cli 12 has no flag for the module system, `nest new` asks for it in a
// terminal, so the CommonJS (jest) variant is the schematic call with that answer (--type=cjs).

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { npmEnv, pins } from "./lib/sources.mjs";
import { copyTree, fixturesDir, readJson, removePath, workDir } from "./lib/util.mjs";

const spec = pins.nestNew;
const genDir = path.join(workDir, "_gen");
const toolDir = path.join(genDir, "tool");

function run(cmd, args, cwd) {
    console.log(`$ (cd ${path.relative(process.cwd(), cwd) || "."} && ${[cmd, ...args].join(" ")})`);
    execFileSync(cmd, args, { cwd, stdio: "inherit", env: npmEnv({ ...process.env, CI: "true" }) });
}

// The cli takes @nestjs/schematics from its own dependency range; the override pins it.
function installTool() {
    removePath(toolDir);
    fs.mkdirSync(toolDir, { recursive: true });
    const pkg = {
        name: "nest-new-generator",
        private: true,
        dependencies: { "@nestjs/cli": spec["@nestjs/cli"] },
        overrides: { "@nestjs/schematics": spec["@nestjs/schematics"] },
    };
    fs.writeFileSync(path.join(toolDir, "package.json"), `${JSON.stringify(pkg, null, 2)}\n`);
    run("npm", ["install", "--ignore-scripts", `--before=${pins.npmBefore}`], toolDir);
    const installed = readJson(path.join(toolDir, "node_modules/@nestjs/schematics/package.json")).version;
    if (installed !== spec["@nestjs/schematics"]) {
        throw new Error(`@nestjs/schematics ${installed} installed, expected ${spec["@nestjs/schematics"]}`);
    }
}

function generate(name, fixture) {
    if (fixture.nestNew !== undefined) {
        run(path.join(toolDir, "node_modules/.bin/nest"), ["new", name, ...fixture.nestNew], genDir);
        return;
    }
    const schematics = path.join(toolDir, "node_modules/@angular-devkit/schematics-cli/bin/schematics.js");
    run("node", [schematics, "@nestjs/schematics:application", `--name=${name}`, ...fixture.schematic], genDir);
}

function lockedVersion(lock, name) {
    const version = lock.packages?.[`node_modules/${name}`]?.version;
    if (version === undefined) {
        throw new Error(`${name} is not in the generated package-lock.json`);
    }
    return version;
}

// Every range in package.json becomes the version the lock resolved, typescript the TS 6.0.3 pin.
function pinVersions(appDir) {
    run("npm", ["install", "--package-lock-only", "--ignore-scripts", `--before=${pins.npmBefore}`], appDir);
    const file = path.join(appDir, "package.json");
    const pkg = readJson(file);
    const lock = readJson(path.join(appDir, "package-lock.json"));
    for (const section of ["dependencies", "devDependencies"]) {
        for (const dep of Object.keys(pkg[section] ?? {})) {
            pkg[section][dep] = dep === "typescript" ? "6.0.3" : lockedVersion(lock, dep);
        }
    }
    fs.writeFileSync(file, `${JSON.stringify(pkg, null, 2)}\n`);
    run("npm", ["install", "--package-lock-only", "--ignore-scripts", `--before=${pins.npmBefore}`], appDir);
}

function main() {
    const names = process.argv.slice(2);
    for (const name of names) {
        if (spec.fixtures[name] === undefined) {
            throw new Error(`unknown fixture ${name} (known: ${Object.keys(spec.fixtures).join(", ")})`);
        }
    }
    fs.mkdirSync(genDir, { recursive: true });
    installTool();
    for (const name of names.length > 0 ? names : Object.keys(spec.fixtures)) {
        const appDir = path.join(genDir, name);
        const fixtureDir = path.join(fixturesDir, name);
        removePath(appDir);
        generate(name, spec.fixtures[name]);
        pinVersions(appDir);
        removePath(fixtureDir);
        copyTree(appDir, fixtureDir, (entry) => entry === "node_modules" || entry === ".git");
        console.log(`fixture written: ${fixtureDir}`);
    }
}

main();
