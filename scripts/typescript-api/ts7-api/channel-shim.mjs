// TS7's SyncRpcChannel (src/api/syncChannel.ts: constructor(exe, args, collectTiming)) as the tsc-rs
// channel. With TSRS_API_CHANNEL=process the channel runs `tsgo --api` as a child process, with
// TS7's exe (built/local/tsc) unless TSRS_TSGO is set. TSRS_SHIM_LOG gets one line per channel.

import fs from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { createChannel } = require("../../../npm/typescript-api/src/native/channel.js");
const kind = process.env.TSRS_API_CHANNEL === "process" ? "process" : "native";

export class SyncRpcChannel {
    constructor(exe, args, collectTiming = false) {
        if (process.env.TSRS_SHIM_LOG) {
            fs.appendFileSync(process.env.TSRS_SHIM_LOG, `${kind} ${process.pid}\n`);
        }
        return createChannel(args, { collectTiming, tsgo: exe });
    }
}
