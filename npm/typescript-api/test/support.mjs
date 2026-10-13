// What the tests compare with and run besides this package: the unchanged typescript@6.0.3, the
// tool packages of test/consumers and nest-cli's spec fixtures. Each comes from the repository or
// from scripts/build.mjs, and each test that needs one skips with a reason when it is missing.

import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);

export const packageDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const repoDir = path.resolve(packageDir, "../..");

// lib/typescript.js of typescript@6.0.3 itself. scripts/build.mjs unpacks it into
// .cache/typescript-6.0.3; TS6_STOCK (or TSRS_TS6) names another copy.
export const stockMain = findStock();
export const stockSkip = stockMain === undefined ? "no stock typescript@6.0.3 (run scripts/build.mjs, or set TS6_STOCK)" : false;

function findStock() {
    const override = process.env.TS6_STOCK || process.env.TSRS_TS6;
    if (override) {
        return require.resolve(path.resolve(override));
    }
    const main = path.join(packageDir, ".cache/typescript-6.0.3/lib/typescript.js");
    return fs.existsSync(main) ? main : undefined;
}

// The node_modules of test/consumers, installed at the versions its package-lock.json pins with
// `npm run install:consumers` (`npm ci --ignore-scripts` in test/consumers). TSRS_CONSUMERS names
// another node_modules directory, whose versions are not checked.
const consumersProject = path.join(packageDir, "test/consumers");
export const consumersDir = process.env.TSRS_CONSUMERS ? path.resolve(process.env.TSRS_CONSUMERS) : path.join(consumersProject, "node_modules");
export const consumersSkip = consumersProblem();

function consumersProblem() {
    const hint = "run `npm run install:consumers` in npm/typescript-api, or set TSRS_CONSUMERS";
    if (!fs.existsSync(consumersDir)) {
        return `the consumer packages are not installed: no ${consumersDir} (${hint})`;
    }
    const { dependencies } = JSON.parse(fs.readFileSync(path.join(consumersProject, "package.json"), "utf8"));
    for (const [name, version] of Object.entries(dependencies)) {
        const manifest = path.join(consumersDir, name, "package.json");
        if (!fs.existsSync(manifest)) {
            return `the consumer packages are not all installed: no ${name} in ${consumersDir} (${hint})`;
        }
        const installed = JSON.parse(fs.readFileSync(manifest, "utf8")).version;
        if (!process.env.TSRS_CONSUMERS && installed !== version) {
            return `the consumer packages are out of date: ${name} ${installed} in ${consumersDir}, test/consumers pins ${version} (${hint})`;
        }
    }
    return false;
}

// nest-cli 12.0.8's spec fixtures for its compiler hooks (see the PROVENANCE.md there).
export const nestCliHookFixtures = path.join(packageDir, "test/fixtures/nest-cli-hooks");
