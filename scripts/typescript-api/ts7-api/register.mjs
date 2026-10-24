// Loaded with NODE_OPTIONS=--import=<this file>: every Node process of the TS7 API test run (the
// test runner, each test file, the children of sync/shutdown.test.ts) resolves TS7's
// src/api/syncChannel.ts to channel-shim.mjs, whose SyncRpcChannel is the tsc-rs channel
// (npm/typescript-api/src/native/channel.js). No test or source file changes.

import { register } from "node:module";

register(new URL("./hooks.mjs", import.meta.url));
