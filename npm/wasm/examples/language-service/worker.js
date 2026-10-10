import { createLanguageService, tsc } from "../../browser.js";
import { verifyLanguageService } from "../../test/language-service-sequence.mjs";

self.onmessage = async () => {
    try {
        const compiler = await tsc(["--noEmit", "/a.ts"], {
            files: { "/a.ts": 'const value: number = "wrong";' }, diagnostics: "json",
        });
        if (compiler.diagnostics[0]?.code !== 2322) throw new Error("compiler control failed");
        const dom = await tsc(["--noEmit", "--lib", "es2022,dom", "/dom.ts"], {
            files: { "/dom.ts": 'const element: HTMLElement | null = document.querySelector("div");' },
        });
        if (dom.exitCode !== 0) throw new Error("DOM library control failed");
        self.postMessage({ ok: true, results: await verifyLanguageService(createLanguageService) });
    } catch (error) {
        self.postMessage({ ok: false, error: String(error), stderr: error.stderr });
    }
};
