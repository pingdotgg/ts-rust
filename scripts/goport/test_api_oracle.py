#!/usr/bin/env python3
"""Unit tests for api_oracle.py protocol 3 (pin B), protocol 4 (pin N), protocol 5 (pin N') and --wire. No server
runs.

  python3 scripts/goport/test_api_oracle.py
"""
import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))


def load(pin):
    """api_oracle.py imported as a fresh module with GOPORT_PIN_ACTIVE=pin (PROTOCOL is fixed at import)."""
    old = os.environ.get("GOPORT_PIN_ACTIVE")
    os.environ["GOPORT_PIN_ACTIVE"] = pin
    try:
        spec = importlib.util.spec_from_file_location(f"api_oracle_{pin}", os.path.join(HERE, "api_oracle.py"))
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod
    finally:
        if old is None:
            del os.environ["GOPORT_PIN_ACTIVE"]
        else:
            os.environ["GOPORT_PIN_ACTIVE"] = old


B, N, N2 = load("16c25522e123"), load("673a5f17d713"), load("fed0bf24149f")
OPEN = {"openProjects": ["@PROJECT_DIR@/tsconfig.json"]}
CHANGE = {"fileChanges": {"changed": ["@PROJECT_DIR@/a.ts"]}}
TEMP = {"file": "@PROJECT_DIR@/a.ts", "newText": "x"}
PF = {"event": 1, "pointer": "/snapshot", "into": "/snapshot"}


def events(mod):
    """The snapshot events of every builder form: stdio, LSP and temporary."""
    tb, lsp = mod.TraceBuilder(), mod.TraceBuilder()
    for b in (OPEN, {}, CHANGE, OPEN):
        tb.snap(b)
    tb.temp(TEMP, PF)
    tb.temp({"snapshot": 999, **TEMP}, None)
    for b in ({}, OPEN, {}):
        lsp.snap(b, lsp=True)
    return tb.events + lsp.events


def run(mod, wire=None):
    r = mod.SessionRun({"project": {"dir": "/p", "tsconfig": "tsconfig.json"}}, [], "tsgo", "oracle", "/tmp",
                       wire=wire)
    r.ctx = {"snapshot": None, "projects": []}
    return r


def proj(pid, n=0):
    return {"id": pid, "configFileName": pid, "n": n}


class Protocol(unittest.TestCase):
    def test_protocols(self):
        self.assertEqual((B.PROTOCOL, N.PROTOCOL, N2.PROTOCOL), (3, 4, 5))

    def test_b_events_unchanged(self):
        ev = events(B)
        self.assertEqual([e["method"] for e in ev], ["updateSnapshot"] * 4 + ["updateTemporarySnapshot"] * 2
                         + ["updateSnapshot"] * 3)
        self.assertEqual([e["params"] for e in ev], [OPEN, {}, CHANGE, OPEN, TEMP, {"snapshot": 999, **TEMP},
                                                     {}, OPEN, {}])
        self.assertEqual(ev[4]["paramsFrom"], PF)
        self.assertFalse(any("track" in e for e in ev))

    def test_n_events(self):
        ev = events(N)
        s, fn = "@SNAPSHOT@", {"fileNotifications": CHANGE["fileChanges"], "ensurePrograms": True}
        layer = {"fileSystem": {"kind": "layer", "files": {TEMP["file"]: "x"}}, "ensurePrograms": True}
        self.assertEqual([(e["method"], e["params"]) for e in ev], [
            ("createSnapshot", OPEN), ("updateSnapshot", {"snapshot": s}),
            ("updateSnapshot", {"snapshot": s, "changes": fn}), ("updateSnapshot", {"snapshot": s, "changes": OPEN}),
            ("updateSnapshot", {"changes": layer}), ("updateSnapshot", {"snapshot": 999, "changes": layer}),
            ("getCurrentLanguageServerSnapshot", {}),
            ("getCurrentLanguageServerSnapshot", {"baseSnapshot": s, "changes": OPEN}),
            ("getCurrentLanguageServerSnapshot", {"baseSnapshot": s})])
        self.assertEqual([e.get("track") for e in ev][4:6], [False, False])
        self.assertEqual(ev[4]["paramsFrom"], PF)

    def test_wire3_inverts(self):
        self.assertEqual([N.wire3_event(e) for e in events(N)], events(B))
        self.assertEqual(N.wire3("getSemanticDiagnostics", {"x": 1}), ("getSemanticDiagnostics", {"x": 1}))

    def test_expand_keys(self):
        ctx = {"snapshot": 7, "project_dir": "/p", "run_dir": "/r", "tsconfig": "tsconfig.json", "projects": []}
        got = N.expand({"snapshot": "@SNAPSHOT@", "files": {"@PROJECT_DIR@/a.ts": "@RUN_DIR@"}}, ctx)
        self.assertEqual(got, {"snapshot": 7, "files": {"/p/a.ts": "/r"}})

    def test_track(self):
        a, b, c = proj("a"), proj("b"), proj("c")
        r = run(N)
        r._track({"method": "createSnapshot"}, OPEN, {"snapshot": 1, "projects": [a, b]})
        r._track({"method": "updateSnapshot"}, {}, {"snapshot": 2, "projects": [proj("a", 1), c],
                                                    "changes": {"removedProjects": ["b"]}})
        self.assertEqual(r.ctx, {"snapshot": 2, "projects": [proj("a", 1), c]})
        r._track({"method": "updateSnapshot", "track": False}, {}, {"snapshot": 3, "projects": []})
        r._track({"method": "getCurrentLanguageServerSnapshot"}, {}, {"snapshot": 4, "projects": [b]})
        self.assertEqual(r.ctx, {"snapshot": 4, "projects": [b]})
        r._track({"method": "getCurrentLanguageServerSnapshot"}, {"baseSnapshot": 4}, {"snapshot": 5, "projects": []})
        self.assertEqual(r.ctx, {"snapshot": 5, "projects": [b]})
        for mod, wire in ((B, None), (N, 3)):  # protocol 3: each updateSnapshot answer replaces, nothing else
            r = run(mod, wire)
            r._track({"method": "updateSnapshot"}, {}, {"snapshot": 1, "projects": [a, b]})
            r._track({"method": "updateTemporarySnapshot"}, {}, {"snapshot": 2, "projects": [c]})
            r._track({"method": "updateSnapshot"}, {}, {"snapshot": 3, "projects": [c]})
            self.assertEqual(r.ctx, {"snapshot": 3, "projects": [c]})

    def test_wire_events(self):
        self.assertEqual(run(N, 3).events, [])
        r = N.SessionRun({}, events(N), "tsgo", "oracle", "/tmp", wire=3)
        self.assertEqual(r.events, events(B))

    def test_prepare_sorts_create(self):
        ans = {"changes": {"changedProjects": {"p": {"changedFiles": ["b", "a"]}}, "removedProjects": ["y", "x"]}}
        got = N.Normalizer([]).prepare("createSnapshot", ans)["changes"]
        self.assertEqual(got, {"changedProjects": {"p": {"changedFiles": ["a", "b"]}}, "removedProjects": ["x", "y"]})


def symbol_events(mod):
    """Symbol requests of every spec form: into /symbol, symbol property (/objectId), append, pick, literal ids."""
    tb = mod.TraceBuilder()
    tb.req("getTypeOfSymbol", tb.ck(), {"event": 0, "pointer": "/id", "into": "/symbol"})
    tb.req("getParentOfSymbol", tb.obj(), {"event": 0, "pointer": "/id", "into": "/objectId",
                                           "when": [{"pointer": "/parent", "nonzero": True}]})
    tb.req("getTypesOfSymbols", tb.ck(symbols=[]), [{"event": 0, "pointer": "/id", "append": "/symbols"}])
    tb.req("getTypeOfSymbol", tb.ck(), {"event": 1, "pointer": "/entries", "into": "/symbol",
                                        "pick": {"index": 0, "then": "/symbol/id"}})
    tb.req("getSymbolOfType", tb.obj(), {"event": 2, "pointer": "/id", "into": "/objectId"})
    tb.req("getTypeOfSymbol", tb.ck(symbol=987654321))
    tb.req("getImportAdderEdits", tb.ck(actions=[{"kind": "importSymbol", "symbol": 0}, {"kind": "unknown"}]))
    return tb.events


class Protocol5(unittest.TestCase):
    def test_n_symbol_events_unchanged(self):
        ev = symbol_events(N)
        self.assertEqual(ev[1]["params"], {"snapshot": "@SNAPSHOT@", "project": "@PROJECT@"})
        self.assertEqual([e.get("paramsFrom") for e in ev], [e.get("paramsFrom") for e in symbol_events(B)])
        self.assertEqual(ev[0]["paramsFrom"]["pointer"], "/id")
        self.assertEqual(ev[5]["params"]["symbol"], 987654321)

    def test_symbol_references(self):
        ev = symbol_events(N2)
        self.assertEqual(ev[0]["paramsFrom"], {"event": 0, "pointer": "/reference", "into": "/symbol"})
        self.assertEqual(ev[1]["params"], {})
        self.assertEqual(ev[1]["paramsFrom"], {"event": 0, "pointer": "/reference", "into": "/symbol",
                                               "when": [{"pointer": "/parent", "nonzero": True}]})
        self.assertEqual(ev[2]["paramsFrom"], [{"event": 0, "pointer": "/reference", "append": "/symbols"}])
        self.assertEqual(ev[3]["paramsFrom"]["pick"]["then"], "/symbol/reference")
        # a type property request keeps its objectId and params
        self.assertEqual(ev[4]["paramsFrom"], {"event": 2, "pointer": "/id", "into": "/objectId"})
        self.assertEqual(ev[4]["params"], {"snapshot": "@SNAPSHOT@", "project": "@PROJECT@"})
        ref = lambda n: {"kind": 1, "snapshot": "@SNAPSHOT@", "project": "@PROJECT@", "id": n}  # noqa: E731
        self.assertEqual(ev[5]["params"]["symbol"], ref(987654321))
        self.assertEqual(ev[6]["params"]["actions"], [{"kind": "importSymbol", "symbol": ref(0)}, {"kind": "unknown"}])
        with self.assertRaises(ValueError):
            N2.TraceBuilder().req("getTypeOfSymbol", {}, {"event": 0, "pointer": "/name", "into": "/symbol"})

    def test_normalize_owners(self):
        desc = lambda node, path: {"fileName": path, "path": path, "contentHash": "h", "parseOptionsKey": "0",  # noqa: E731
                                   "scriptKind": 3, "nodeId": node}
        sym = lambda sid, node, path, parent=None: {  # noqa: E731
            "reference": {"kind": 0, "file": desc(node, path), "id": sid}, "name": f"n{sid}", "flags": 1,
            "declarations": [f"1.80.{path}"], **({"parent": parent} if parent else {})}
        entries = [("getSymbolAtPosition", sym(901, "77", "/p/a.ts", parent={"id": 902, "file": "77"})),
                   ("getSymbolAtPosition", sym(903, "78", "/p/a.ts")),  # the same path, a new AST
                   ("getTypeAtPosition", {"id": 5, "symbol": {"id": 901, "file": "77"},
                                          "aliasSymbol": {"id": 904, "file": "99"}})]
        a, b, c = N2.Normalizer([("/p", "@P@")]).finalize(entries)
        self.assertEqual(a["reference"], {"kind": 0, "file": desc("@P@/a.ts", "@P@/a.ts"), "id": "n901@1.80.@P@/a.ts"})
        self.assertEqual(a["parent"], {"id": "?1", "file": "@P@/a.ts"})
        self.assertEqual(b["reference"]["file"]["nodeId"], "@P@/a.ts#2")
        self.assertEqual(c["symbol"], {"id": "n901@1.80.@P@/a.ts", "file": "@P@/a.ts"})
        self.assertEqual(c["aliasSymbol"], {"id": "?2", "file": "?f1"})
        masked = N2.mask_ids("getTypeAtPosition", entries[2][1])
        self.assertEqual(masked["aliasSymbol"], {"id": "#", "file": "#"})

    def test_source_file_node_id_zeroed(self):
        data = bytes(range(64))
        for method in ("getSourceFile", "getCachedSourceFile", "getConfigSourceFile"):
            out = N2.Normalizer([]).prepare(method, {"data": N2.base64.b64encode(data).decode()})
            got = N2.base64.b64decode(out["data"])
            self.assertEqual((got[:44], got[44:52], got[52:]), (data[:44], bytes(8), data[52:]), method)
            same = N.Normalizer([]).prepare(method, {"data": N.base64.b64encode(data).decode()})
            self.assertEqual(N.base64.b64decode(same["data"]), data, method)
        node = N2.Normalizer([]).prepare("typeToTypeNode", {"data": N2.base64.b64encode(data).decode()})
        self.assertEqual(N2.base64.b64decode(node["data"]), data)
        self.assertIsNone(N2.Normalizer([]).prepare("getConfigSourceFile", None))

    def test_callback_kinds(self):
        fs = N2.CallbackFS(["readFile", "fileExists", "writeFile"], "fallback")
        fs.set_overlay("/p/a.ts", "x")
        fs.set_overlay("/p/gone.ts", None)
        self.assertEqual(fs.handle("readFile", "/p/a.ts"), {"kind": "value", "value": "x"})
        self.assertEqual(fs.handle("readFile", "/p/gone.ts"), {"kind": "missing"})
        self.assertEqual(fs.handle("readFile", "/p/other.ts"), {"kind": "useOS"})
        self.assertEqual(fs.handle("fileExists", "/p/gone.ts"), {"kind": "value", "value": False})
        self.assertEqual(fs.handle("writeFile", {"path": "/p/o.js", "data": "y"}), {"kind": "noop"})
        self.assertIsNone(fs.handle("#notification:x", None))
        old = N.CallbackFS(["readFile"], "fallback")
        old.set_overlay("/p/a.ts", "x")
        self.assertEqual((old.handle("readFile", "/p/a.ts"), old.handle("readFile", "/p/b.ts")), ({"content": "x"}, None))


class Wire4(unittest.TestCase):
    def test_wire4_inverts_symbol_refs(self):
        self.assertEqual([N2.wire4_event(e) for e in symbol_events(N2)], symbol_events(N))
        self.assertEqual([N2.wire4_event(e) for e in events(N2)], events(N))
        overlay = {"kind": "overlay", "path": "/p/a.ts", "content": "x"}
        self.assertIs(N2.wire4_event(overlay), overlay)
        new = {"kind": "request", "method": "getCachedSourceFile", "params": {"file": {"nodeId": "1"}}}
        self.assertEqual(N2.wire4_event(new), new)
        with self.assertRaises(ValueError):
            N2.wire4("getTypeOfSymbol", {}, {"event": 0, "pointer": "/id", "into": "/symbol"})

    def test_wire4_keeps_other_symbols(self):
        # only an exact snapshot reference with an int id is a literal id
        for v in ("@X@", {"kind": 0, "id": 5}, {"kind": 1, "snapshot": 3, "project": "@PROJECT@", "id": 5}):
            self.assertEqual(N2.wire4("getTypeOfSymbol", {"symbol": v}, None)[0], {"symbol": v})

    def test_wire4_kinds(self):
        kinds = {181: 180, 168: 167}  # N' value -> N value (#63915)
        ev = {"kind": "request", "method": "signatureToSignatureDeclaration", "params": {"kind": 181, "flags": 181,
              "location": "7.168./p/a.ts", "x": ["7.999./p/a.ts", "a.181./p", True]}}
        self.assertEqual(N2.wire4_event(ev, kinds)["params"], {"kind": 180, "flags": 181, "location": "7.167./p/a.ts",
                                                               "x": ["7.999./p/a.ts", "a.181./p", True]})
        other = {"kind": "request", "method": "getTypeAtLocation", "params": {"kind": 181, "location": "1.181./p"}}
        self.assertEqual(N2.wire4_event(other, kinds)["params"], {"kind": 181, "location": "1.180./p"})
        self.assertEqual(N2.wire4_event(ev)["params"], ev["params"])

    def test_wire4_run(self):
        r = N2.SessionRun({}, symbol_events(N2), "tsgo", "goport", "/tmp", wire=4, wire_kinds={})
        self.assertEqual(r.events, symbol_events(N))
        self.assertEqual(N2.SessionRun({}, symbol_events(N2), "tsgo", "goport", "/tmp").events, symbol_events(N2))

    def test_wire4_callbacks(self):
        fs = N2.CallbackFS(["readFile", "writeFile"], "fallback", 4)
        fs.set_overlay("/p/a.ts", "x")
        self.assertEqual((fs.handle("readFile", "/p/a.ts"), fs.handle("readFile", "/p/b.ts")), ({"content": "x"}, None))
        self.assertIsNone(fs.handle("writeFile", {"path": "/p/o.js", "data": "y"}))

    def test_wire_needs_its_protocol(self):
        import types
        for mod, wire in ((N, 4), (N2, 3), (B, 3), (B, 4)):
            with self.assertRaises(mod.UsageError):
                mod.cmd_check(types.SimpleNamespace(wire=wire))


if __name__ == "__main__":
    unittest.main()
