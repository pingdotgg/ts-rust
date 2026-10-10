#!/usr/bin/env python3
"""Differential API oracle: pinned `tsgo --api` against the goport `tsgo --api`.

The script mirrors these Go files (pinned dc37b5249, /home/theo/.explore/repos/microsoft__typescript-go):

  cmd/tsgo/api.go                  flags --cwd, --callbacks, --async
  internal/api/proto.go            methods, params and response shapes
  internal/api/session.go          handles: symbol ids (global counter), type and signature ids
                                   (per checker), node handles "index.kind.path"
  internal/api/protocol_jsonrpc.go JSON-RPC with Content-Length framing (--async)
  internal/api/protocol_msgpack.go the default sync protocol: [type, method, payload] tuples
  internal/api/callbackfs.go       server-to-client FS callbacks
  internal/api/encoder/encoder.go  the binary AST (getSourceFile), used by `build` to find nodes
  internal/lsp/server.go:1725      custom/initializeAPISession (API over a Unix socket of an LSP server)

Upstream pins (UPSTREAM.json, scripts/upstream/pin.py): with GOPORT_PIN=<key> the tool runs itself
again under `pin.py exec`, so the default oracle, Go checkout and traces/ show that pin's files (traces/
is a pin cache; golden/ is keyed by the oracle anyway). The O pin dc37b5249ab6 speaks protocol 1, the
bump A pin 52168999f3dc protocol 2, the bump B pin 16c25522e123 protocol 3, the bump C pin 673a5f17d713
protocol 4, every later pin protocol 5.
Protocol 2 (Go changed the API in bump A):
  - updateSnapshot takes openProjects: [config] (tsgo#4402), not openProject;
  - the type, symbol and signature property requests (objectId) need a project (tsgo#4341:
    GetTypePropertyParams, GetSymbolPropertyParams, GetSignaturePropertyParams);
  - internal symbol names come escaped (ast.EscapeSymbolName): "__@iterator@<id>", not "\ufffd@...".
Protocol 3 (bump B, 16c25522e123) adds to protocol 2:
  - the per-file diagnostics requests take files: [file] (tsgo#4552: GetDiagnosticsParams.Files). Go ignores
    the old "file" field there and answers for the whole program;
  - getConstraintOfTypeParameter, getNonNullableType, getApparentType and getReturnTypeOfSignature take the
    type or signature as objectId (tsgo#4689: GetTypePropertyParams, GetSignaturePropertyParams), not as
    "type" or "signature". Go answers "empty type handle" to the old field.
  The other API changes up to 16c25522e123 keep the requests of `build`: new methods only (transpile*
  tsgo#4849, emit, getSymbolsInScope, ...), the language service methods keep their wire names (tsgo#4893
  moves them only in the TS client), and tsgo#4915 adds only generator comments and tags.
  `build --kind ext` (protocol 3 or later) sends the methods that B adds or changes and that the other kinds do
  not send: design in target/continuation-r97-goport/upstream/bumpB/api-battery/design.md.
Protocol 4 (every pin after 16c25522e123; bump C N = microsoft/TypeScript 673a5f17d713, study in
target/continuation-r97-goport/studies/api-oracle-N.md) changes only the snapshot requests:
  - updateSnapshot takes {snapshot: base, changes?} and clones that base; the flat fields are gone.
    createSnapshot takes the flat changes and starts from a fresh root; getCurrentLanguageServerSnapshot
    {baseSnapshot?, changes?} (LSP sessions only) takes the old path through the LS state;
  - fileChanges is changes.fileNotifications. A file change leaves a program dirty unless ensurePrograms asks
    for it, so file-change updates send ensurePrograms: true (the B behavior);
  - updateTemporarySnapshot is gone: an updateSnapshot with a layer fileSystem {kind: "layer", files: {file:
    text}} and ensurePrograms: true replaces it (the TS client's runWithTemporaryFileUpdate);
  - with a base, the answer lists only the added or replaced projects, and changes.removedProjects the removed
    ones. The run merges them into the project list of the base.
  `build` writes each protocol 3 snapshot event as exactly one protocol 4 event (TraceBuilder.snap and .temp),
  so the event keys match the B traces. A layer event has "track": false: it does not move @SNAPSHOT@.
  `check --wire 3` sends the snapshot events of protocol 4 traces in their protocol 3 form (wire3), for a
  ruling 10 rebase run of base bins that speak protocol 3. It needs a reviewer ruling before its results are
  used; the result meta and manifest record "wire": 3.
Protocol 5 (every pin after 673a5f17d713; bump D N' = microsoft/TypeScript fed0bf24149f) adds to protocol 4:
  - symbol ownership (#64518): a symbol answer has "reference" {kind (0 file, 1 snapshot), file (a source file
    descriptor) or snapshot and project, id} in place of "id"; parent, exportSymbol, a type's symbol and
    aliasSymbol and a signature's parameters and thisParameter are compact references {id, file (the owning
    file's node id)}. A request sends the symbol's reference: TraceBuilder.req moves each symbol spec from the
    answer's /id to its /reference. getParentOfSymbol, getMembersOfSymbol, getExportsOfSymbol and
    getExportSymbolOfSymbol (GetSymbolPropertyParams) take {symbol} only. A literal symbol id of an error probe
    becomes a snapshot reference with that id;
  - an encoded source file (getSourceFile, getCachedSourceFile, getConfigSourceFile) has the file's node id at
    header byte 44 (SetSourceFileID). Node ids come from a global counter, like symbol ids, so normalization
    names them by the file path and zeroes those 8 bytes;
  - 7 new methods (#64518, #64571, #64598): retainSourceFile, getCachedSourceFile, getSymbolOfDeclaration,
    getMergedSymbol, getSymbolOfNode, getSymbolOfDeclarationForChecker, getParentOfSymbolForChecker. `build
    --kind ext` sends them at the end of each ext_file trace, so the earlier event keys stay;
  - KindSourceKeyword (#63915) moves every later SyntaxKind value up by 1. `build` reads the values from the
    pin's kind_generated.go, as before.
  `check --wire 4` sends protocol 5 traces in their protocol 4 form (wire4, the inverse of symbol_refs: symbol
  specs read /id, a symbol property request is {snapshot, project, objectId}, a snapshot reference with a literal
  id is that id; and the SyntaxKind values of the params, in node handles and the signatureToSignatureDeclaration
  "kind", go back to the values of the protocol 4 pin by kind name, read from both pins' kind_generated.go) and
  answers callbacks in their protocol 4 form, for a rebase run of base bins that speak protocol 4. Then each
  event is the event of the protocol 4 pin's trace. The 7 new methods go as they are. Answers are normalized as
  protocol 5 answers, so a protocol 4 symbol answer is never `same`. As for --wire 3, it needs a reviewer ruling
  before its results are used; the result meta and manifest record "wire": 4.
`build` writes traces of the run's protocol. Normalization reads the escaped names from protocol 2 on.
Build the traces at each pin: they hold positions from the pin's encoded AST (UTF-16 offsets after the
O pin, UTF-8 at O; they differ in files with non-ASCII text). With GOPORT_PIN unset the run uses the
current pin of UPSTREAM.json.

Commands:
  build     --preset P --battery B [--kind files|proto|callbacks|lsp|xchecker|ext] [--oracle BIN] [--limit N]
  record    --battery B [--oracle BIN] [--jobs N] [--force] [--only S]
  selfcheck --battery B [--oracle BIN] [--jobs N] [--runs N] [--only S]
  check     --battery B --goport BIN --label L [--jobs N] [--only S] [--wire 3|4]
  summary   --label L [--baseline L0]
  show      --label L --battery B --trace T --event K

Files under --out-root (default target/continuation-r97-goport/tests2/api):
  traces/<battery>/<trace>.jsonl, traces/<battery>/index.json          input
  golden/<oracle-sha12>/<battery>/<trace>.golden.jsonl.gz              tsgo answers
  golden/<oracle-sha12>/<battery>/<trace>.flaky.json                   selfcheck result
  golden/<oracle-sha12>/<battery>/{record,selfcheck}-summary.json
  results/<label>/summary.json, summary.md                             counts only
  results/<label>/traces/<battery>/<trace>.json                        per-event classes
  results/<label>/responses/<battery>/<trace>.jsonl.gz                 goport answers (for `show`)

Trace format goport-api-trace/1 (JSONL). Line 1 is the header:
  format, name, battery, project {dir, tsconfig}, cwd (default project dir),
  protocol ("jsonrpc": --async, or "msgpack"), callbacks (list, empty = none),
  callbackMode ("fallback": answer null except overlay paths; "real": answer every
  callback from the real read-only FS), transport ("stdio" or "lsp"), lspOpen (files
  the LSP client opens before custom/initializeAPISession), fixture ({rel path: text},
  written under the run dir before the server starts). The callback "writeFile" (tsgo#4699)
  records {path, bytes, sha256} and writes nothing; an `emit` answer gets them as "@writes",
  sorted by path. An `emit` request is sent only with that callback, a fixture and a project
  config under the run dir (else not_run), so no run writes into a project input.
Each later line is one event, numbered from 0:
  {"kind": "request", "method": M, "params": P, "paramsFrom": F}   F optional; "track": false
                                                                     (protocol 4 layer events) keeps
                                                                     @SNAPSHOT@ and the projects
  {"kind": "overlay", "path": ABS, "content": TEXT | null}           callback FS overlay
                                                                     (null: file deleted;
                                                                     {"drop": true}: overlay removed)
Placeholders (expanded in params before paramsFrom):
  "@SNAPSHOT@"          snapshot of the latest successful updateSnapshot answer (protocol 4: of any
                        snapshot method)
  "@PROJECT@"           id of the project whose configFileName is the header tsconfig
  "@PROJECT:<rel>@"     id of the project whose config is <project dir>/<rel> (<rel> may start
                        with @RUN_DIR@ for a fixture config)
  "@PROJECT_DIR@", "@RUN_DIR@" inside strings and object keys
paramsFrom: one spec or a list of specs, applied in order. A spec takes a value from
the raw answer of an earlier event of the same server run (handles differ between
servers and between Go runs, so they are never copied from the golden):
  {"event": K, "pointer": "/id", "into": "/symbol"}      set params at pointer
  {"event": K, "pointer": "/id", "append": "/symbols"}   append to a list in params
  "pick": {"sortBy": ["name"], "index": 0} | {"index": 0} | {"where": {"field": "flags", "mask": M}}
  "when": [{"pointer": "/flags", "mask": M}, {"pointer": "/target", "nonzero": true}]
  "optional": true                                       a missing value skips only this spec
A value that cannot be found skips the request. In `check`, a request the golden
skipped is skipped; a request the golden sent but goport cannot resolve is not_run.

Normalization (per session): symbol ids are renumbered by first appearance ("s1", "s2",
...; Go symbol ids come from a global counter that concurrent checking races). Lists
from Go maps (members, exports, completion entries, changed files) are sorted first.
Type and signature ids stay raw. Symbol names that embed a symbol id ("\\ufffd@iterator@123",
"\\ufffd#123@x") get the renumbered id. Paths: project dir -> @PROJECT_DIR@, run dir -> @RUN_DIR@.
Errors keep the first message line; "handle 123" becomes "handle #".

Classes per request: same, id_only (equal when every symbol, type and signature id is
masked), diff, goport_error, oracle_error_same, oracle_error_diff, crash (goport process
died, or answered "unported Go code"), timeout, not_run, flaky_oracle, oracle_crash,
skipped (the golden skipped it; not counted). After a crash the harness restarts the
server, replays the earlier events silently and goes on (at most MAX_RESTARTS times).

Exit status: 0 done, 1 a trace failed in the harness, 2 usage error, 3 a project input
changed during the run.
"""

import argparse
import base64
import bisect
import collections
import concurrent.futures
import copy
import difflib
import gzip
import hashlib
import json
import os
import queue
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

TRACE_FORMAT = "goport-api-trace/1"
GOLDEN_FORMAT = "goport-api-golden/1"
FLAKY_FORMAT = "goport-api-flaky/1"
RESULT_FORMAT = "goport-api-result/1"
SUMMARY_FORMAT = "goport-api-summary/1"

REPO = "/home/theo/Code/sandbox/ts-rust"
PIN_TOOL = REPO + "/scripts/upstream/pin.py"
O_PIN = "dc37b5249ab6"  # the last pin with API protocol 1
A_PIN = "52168999f3dc"  # the last pin with API protocol 2
B_PIN = "16c25522e123"  # the last pin with API protocol 3
C_PIN = "673a5f17d713"  # the last pin with API protocol 4


def run_pin():
    """The upstream pin of this run: GOPORT_PIN_ACTIVE inside `pin.py exec`, else the current pin."""
    active = os.environ.get("GOPORT_PIN_ACTIVE")
    if active:
        return active
    try:
        with open(REPO + "/UPSTREAM.json", encoding="utf-8") as f:
            return json.load(f)["current"]
    except (OSError, ValueError, KeyError):
        return O_PIN


PIN = run_pin()
PROTOCOL = 1 if PIN == O_PIN else 2 if PIN == A_PIN else 3 if PIN == B_PIN else 4 if PIN == C_PIN else 5
PROTOCOL2 = PROTOCOL >= 2
DEFAULT_OUT_ROOT = REPO + "/target/continuation-r97-goport/tests2/api"
DEFAULT_ORACLE = os.path.expanduser("~/.local/bin/tsgo-oracle")
GO_REPO = "/home/theo/.explore/repos/microsoft__typescript-go"
PROJECT_INPUTS = REPO + "/target/project-inputs"

EXIT_OK, EXIT_FAILED, EXIT_USAGE, EXIT_INPUT_CHANGED = 0, 1, 2, 3
MAX_RESTARTS = 8
START_TIMEOUT = 60.0
REQUEST_TIMEOUT = {"oracle": 90.0, "goport": 180.0}
MISSING = object()

# Go: checker/types.go TypeFlags and ObjectFlags; ast/symbolflags.go SymbolFlags (pinned).
TF_TYPE_PARAMETER = 1 << 19
TF_TEMPLATE_LITERAL = 1 << 22
TF_UNION = 1 << 27
TF_INTERSECTION = 1 << 28
OF_CLASS_OR_INTERFACE = (1 << 0) | (1 << 1)
OF_REFERENCE = 1 << 2
SF_FUNCTION = 1 << 4
SF_METHOD = 1 << 13
SF_TRANSIENT = 1 << 25
SF_VALUE = 1 | 2 | 4 | 8 | 4096 | 16 | 32 | 128 | 256 | 512 | 8192 | 32768 | 65536
SF_TYPE = 32 | 64 | 128 | 256 | 8 | 2048 | 262144 | 524288
SF_NAMESPACE = 512 | 1024 | 128 | 256
SF_BLOCK_SCOPED_VARIABLE, SF_TYPE_PARAMETER, SF_ALIAS = 2, 262144, 2097152
# Go: checker/types.go TypeFormatFlagsNoTruncation | TypeFormatFlagsUseFullyQualifiedType
TYPE_FORMAT_FLAGS = 1 | 64
# Go: checker SignatureKindCall, SignatureKindConstruct
SIG_CALL, SIG_CONSTRUCT = 0, 1

PRESETS = {
    "query-core": {
        "dir": PROJECT_INPUTS + "/query/source/packages/query-core",
        "tsconfig": "tsconfig.json",
        "tsconfig2": "tsconfig.prod.json",
        "include": ["src/**/*.ts"],
        "exclude": ["src/__tests__/**"],
        "caps": {"ids": 10, "calls": 4, "typeNodes": 4, "fnExprs": 3, "shorthand": 2, "sigDecls": 2,
                 "memberCompletions": 2, "globalCompletions": 1, "refsForNode": 2, "sigUsages": 2},
    },
    "hono": {
        "dir": PROJECT_INPUTS + "/hono/source",
        "tsconfig": "tsconfig.build.json",
        "include": ["src/**/*.ts", "src/**/*.mts"],
        "exclude": ["**/*.test.ts", "**/*.test.tsx"],
        "caps": {"ids": 5, "calls": 3, "typeNodes": 3, "fnExprs": 2, "shorthand": 1, "sigDecls": 1,
                 "memberCompletions": 1, "globalCompletions": 0, "refsForNode": 1, "sigUsages": 1},
    },
    "ts-pattern": {
        "dir": PROJECT_INPUTS + "/ts-pattern/source",
        "tsconfig": "tsconfig.json",
        "include": ["src/**/*.ts"],
        "exclude": [],
        "caps": {"ids": 6, "calls": 3, "typeNodes": 3, "fnExprs": 2, "shorthand": 1, "sigDecls": 1,
                 "memberCompletions": 1, "globalCompletions": 0, "refsForNode": 1, "sigUsages": 1},
    },
    "zod": {
        "dir": PROJECT_INPUTS + "/zod/source/packages/zod",
        "tsconfig": "tsconfig.json",
        "include": ["src/**/*.ts"],
        "exclude": ["**/tests/**", "**/*.test.ts"],
        "caps": {"ids": 5, "calls": 3, "typeNodes": 3, "fnExprs": 2, "shorthand": 1, "sigDecls": 1,
                 "memberCompletions": 1, "globalCompletions": 0, "refsForNode": 1, "sigUsages": 1},
    },
    "effect": {
        "dir": PROJECT_INPUTS + "/effect/source/packages/effect",
        "tsconfig": "tsconfig.json",
        "include": ["src/**/*.ts"],
        "exclude": [],
        "caps": {"ids": 3, "calls": 2, "typeNodes": 2, "fnExprs": 1, "shorthand": 1, "sigDecls": 1,
                 "memberCompletions": 1, "globalCompletions": 0, "refsForNode": 0, "sigUsages": 0},
    },
}

# `build --kind ext` settings (api-battery/design.md section 4.1): ext_file sample count, files the samples skip,
# the emit fixture sources and the JSX files that transpile reads.
EXT_PRESETS = {
    "query-core": {"files": 8, "sampleExclude": [],
                   "fixture": ["src/subscribable.ts", "src/timeoutManager.ts", "src/focusManager.ts",
                               "src/onlineManager.ts", "src/notifyManager.ts"]},
    "hono": {"files": 12, "sampleExclude": [], "jsx": ["src/**/*.tsx"]},
    # 50 of zod's 131 roots are near-identical locale tables; the zod battery covers them.
    "zod": {"files": 10, "sampleExclude": ["src/v4/locales/**"]},
}


class UsageError(Exception):
    pass


class HarnessError(Exception):
    pass


class InputChanged(Exception):
    pass


def log(*parts):
    print(time.strftime("%H:%M:%S"), *parts, file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------
# JSON helpers (same rules as lsp_oracle.py)
# ---------------------------------------------------------------------------


def canon(v) -> str:
    return json.dumps(v, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def sha256_bytes(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def sha256_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def pointer_tokens(pointer: str):
    if pointer == "":
        return []
    if not pointer.startswith("/"):
        raise ValueError(f"bad JSON pointer {pointer!r}")
    return [t.replace("~1", "/").replace("~0", "~") for t in pointer[1:].split("/")]


def pointer_from_path(path) -> str:
    return "".join("/" + str(t).replace("~", "~0").replace("/", "~1") for t in path)


def pointer_get(v, pointer):
    tokens = pointer_tokens(pointer) if isinstance(pointer, str) else list(pointer)
    for t in tokens:
        if isinstance(v, dict):
            if t not in v:
                return MISSING
            v = v[t]
        elif isinstance(v, list):
            if isinstance(t, int):
                i = t
            elif isinstance(t, str) and t.isdigit():
                i = int(t)
            else:
                return MISSING
            if i >= len(v):
                return MISSING
            v = v[i]
        else:
            return MISSING
    return v


def pointer_set(root, pointer: str, value):
    tokens = pointer_tokens(pointer)
    if not tokens:
        return value
    cur = root
    for n, t in enumerate(tokens):
        last = n == len(tokens) - 1
        if isinstance(cur, list):
            i = int(t)
            if last:
                if i == len(cur):
                    cur.append(value)
                else:
                    cur[i] = value
                return root
            cur = cur[i]
        else:
            if last:
                cur[t] = value
                return root
            if not isinstance(cur.get(t), (dict, list)):
                cur[t] = {}
            cur = cur[t]
    return root


def first_diff_path(a, b, path=()):
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            if k not in a or k not in b:
                return path + (k,)
            sub = first_diff_path(a[k], b[k], path + (k,))
            if sub is not None:
                return sub
        return None
    if isinstance(a, list) and isinstance(b, list):
        for i in range(min(len(a), len(b))):
            sub = first_diff_path(a[i], b[i], path + (i,))
            if sub is not None:
                return sub
        return path if len(a) != len(b) else None
    return None if canon(a) == canon(b) else path


def deep_sort(x):
    if isinstance(x, dict):
        return {k: deep_sort(v) for k, v in x.items()}
    if isinstance(x, list):
        return sorted((deep_sort(e) for e in x), key=canon)
    return x


def generalize(path):
    return tuple("*" if isinstance(t, int) else t for t in path)


def pattern_string(pattern) -> str:
    return "".join("/" + ("*" if t == "*" else str(t)) for t in pattern)


def apply_multisets(v, patterns):
    """Sort every array whose generalized path (index = "*") is in `patterns`."""
    if not patterns:
        return v
    pats = {tuple(pointer_tokens(p)) for p in patterns}

    def walk(x, path):
        if isinstance(x, dict):
            return {k: walk(val, path + (k,)) for k, val in x.items()}
        if isinstance(x, list):
            items = [walk(e, path + ("*",)) for e in x]
            if path in pats:
                items.sort(key=lambda e: canon(deep_sort(e)))
            return items
        return x

    return walk(v, ())


def find_unstable(a0, b0, max_rounds=64):
    """Two oracle answers of one request: (multiset patterns, unstable pointer or None)."""
    patterns = []
    for _ in range(max_rounds):
        a, b = apply_multisets(a0, patterns), apply_multisets(b0, patterns)
        d = first_diff_path(a, b)
        if d is None:
            return patterns, None
        found = False
        for k in range(len(d), -1, -1):
            p = d[:k]
            va, vb = pointer_get(a, p), pointer_get(b, p)
            if not (isinstance(va, list) and isinstance(vb, list)) or len(va) != len(vb):
                continue
            pat = pattern_string(generalize(p))
            if pat in patterns:
                continue
            if sorted(canon(deep_sort(e)) for e in va) == sorted(canon(deep_sort(e)) for e in vb):
                patterns.append(pat)
                found = True
                break
        if not found:
            return patterns, pointer_from_path(d)
    return patterns, "(too many rounds)"


# ---------------------------------------------------------------------------
# Response shapes (Go: api/proto.go) and normalization
# ---------------------------------------------------------------------------

# Walk order matters: "name" is last, so an id embedded in a name is usually known by then.
# Protocol 5 (#64518): a symbol answer holds its id in "reference", with the owning file's descriptor; other
# answers hold compact references {id, file}. Both name the file by its node id ("node", renamed by path).
FILE_DESC = {"nodeId": "node"}
CSYM = {"id": "sym", "file": "node"} if PROTOCOL >= 5 else "sym"
SYM = {"reference": {"id": "sym", "file": FILE_DESC}, "parent": CSYM, "exportSymbol": CSYM, "name": "symname"} \
    if PROTOCOL >= 5 else {"id": "sym", "parent": "sym", "exportSymbol": "sym", "name": "symname"}
TYPE = {"id": "type", "symbol": CSYM, "aliasSymbol": CSYM, "aliasTypeArguments": ["type"], "target": "type",
        "typeParameters": ["type"], "outerTypeParameters": ["type"], "localTypeParameters": ["type"],
        "objectType": "type", "indexType": "type", "checkType": "type", "extendsType": "type", "baseType": "type",
        "substConstraint": "type", "freshType": "type", "regularType": "type",
        # protocol 4 (TypeResponse at N); B answers never have these keys
        "typeParameter": "type", "constraintType": "type", "nameType": "type", "templateType": "type",
        "thisType": "type"}
SIG = {"id": "sig", "typeParameters": ["type"], "parameters": [CSYM], "thisParameter": CSYM, "target": "sig"}
INDEX_INFO = {"keyType": TYPE, "valueType": TYPE}
PREDICATE = {"type": TYPE}
COMPLETIONS = {"entries": [{"symbol": SYM}]}
REF_SYMBOL = {"symbol": SYM}

SYM_METHODS = ["getSymbolAtPosition", "getSymbolAtLocation", "resolveName", "getParentOfSymbol",
               "getExportSymbolOfSymbol", "getSymbolOfType", "getAliasSymbolOfType", "getThisParameterOfSignature",
               "getShorthandAssignmentValueSymbol",
               # protocol 5 (#64571, #64598)
               "getSymbolOfDeclaration", "getMergedSymbol", "getSymbolOfNode", "getSymbolOfDeclarationForChecker",
               "getParentOfSymbolForChecker"]
SYM_LIST_METHODS = ["getSymbolsAtPositions", "getSymbolsAtLocations", "getMembersOfSymbol", "getExportsOfSymbol",
                    "getParametersOfSignature", "getPropertiesOfType"]
TYPE_METHODS = ["getTypeOfSymbol", "getDeclaredTypeOfSymbol", "getTypeAtLocation", "getTypeAtPosition",
                "getTargetOfType", "getFreshTypeOfType", "getRegularTypeOfType", "getObjectTypeOfType",
                "getIndexTypeOfType", "getCheckTypeOfType", "getExtendsTypeOfType", "getBaseTypeOfType",
                "getConstraintOfType", "getContextualType", "getBaseTypeOfLiteralType", "getNonNullableType",
                "getTypeFromTypeNode", "getWidenedType", "getParameterType", "getTypeOfSymbolAtLocation",
                "getReturnTypeOfSignature", "getRestTypeOfSignature", "getConstraintOfTypeParameter",
                "getAnyType", "getStringType", "getNumberType", "getBooleanType", "getVoidType", "getUndefinedType",
                "getNullType", "getNeverType", "getUnknownType", "getBigIntType", "getESSymbolType"]
TYPE_LIST_METHODS = ["getTypesOfSymbols", "getTypeAtLocations", "getTypesAtPositions", "getTypesOfType",
                     "getTypeParametersOfType", "getOuterTypeParametersOfType", "getLocalTypeParametersOfType",
                     "getAliasTypeArgumentsOfType", "getTypeParametersOfSignature", "getBaseTypes",
                     "getTypeArguments"]
INTRINSICS = ["getAnyType", "getStringType", "getNumberType", "getBooleanType", "getVoidType", "getUndefinedType",
              "getNullType", "getNeverType", "getUnknownType", "getBigIntType", "getESSymbolType"]

SHAPES = {}
SHAPES.update({m: SYM for m in SYM_METHODS})
SHAPES.update({m: [SYM] for m in SYM_LIST_METHODS})
SHAPES.update({m: TYPE for m in TYPE_METHODS})
SHAPES.update({m: [TYPE] for m in TYPE_LIST_METHODS})
SHAPES.update({"getResolvedSignature": SIG, "getTargetOfSignature": SIG, "getSignaturesOfType": [SIG],
               "getTypePredicateOfSignature": PREDICATE, "getIndexInfosOfType": [INDEX_INFO],
               "getCompletionsAtPosition": COMPLETIONS, "getReferencedSymbolsForNode": [REF_SYMBOL]})
# Methods that only `build --kind ext` sends (protocol 3).
SHAPES.update({"getSymbolOfSourceFile": SYM, "getSymbolsOfSourceFiles": [SYM], "getSymbolsInScope": [SYM],
               "getApparentPropertiesOfType": [SYM], "getApparentType": TYPE, "getDefaultFromTypeParameter": TYPE,
               "getTypeParameterAtPosition": TYPE, "getNonPrimitiveType": TYPE})
# Lists that come from Go map iteration (random order). Sorted before renumbering.
UNORDERED = {"getMembersOfSymbol": "", "getExportsOfSymbol": "", "getCompletionsAtPosition": "/entries",
             "getSymbolsInScope": ""}
# Methods that answer {snapshot, projects, changes?}: protocol 3 sends the last two, protocol 4 the first three.
SNAPSHOT_METHODS = ("createSnapshot", "updateSnapshot", "getCurrentLanguageServerSnapshot", "updateTemporarySnapshot")
# Methods whose msgpack answer is raw bytes (Go: RawBinary in session.go).
BINARY_METHODS = {"getSourceFile", "typeToTypeNode", "signatureToSignatureDeclaration", "echo", "getConfigSourceFile",
                  "getCachedSourceFile"}
# Methods that answer an encoded source file. Protocol 5 writes the file's node id at header byte 44 (Go:
# encodeSourceFileResponse, which these 3 handlers call).
SOURCE_FILE_METHODS = {"getSourceFile", "getCachedSourceFile", "getConfigSourceFile"}
SOURCE_FILE_ID = slice(44, 52)  # Go: encoder.HeaderOffsetSourceFileID, a uint64
# tsgo#4699 emit methods: files emit in parallel, so these lists come in any order.
EMIT_METHODS = {"emit", "emitToString", "getJavaScriptEmit", "getDeclarationEmit"}

# Internal symbol names that embed a symbol id (Go: checker.go:22882 "\xFE@name@<id>",
# binder.go:375 "\xFE#<class id>@#name"). Go JSON writes the \xFE byte as U+FFFD. Protocol 2 sends
# the escaped name, "__" for the \xFE byte (ast.EscapeSymbolName; a user name that starts with "__"
# gets one more "_", so it never matches).
_PREFIX = "__" if PROTOCOL2 else "."
_INTERNAL_AT = re.compile(rf"^({_PREFIX})@(.*)@(\d+)$", re.S)
_INTERNAL_PRIVATE = re.compile(rf"^({_PREFIX})#(\d+)@(.*)$", re.S)
_EMBEDDED = re.compile("\u27e8[^\u27e9]*\u27e9")
# The same internal names inside a getFullyQualifiedName answer ("A.__@iterator@123"): the id is masked.
_FQN_AT = re.compile(rf"({re.escape(_PREFIX)}@[^@]*@)\d+")
_FQN_PRIVATE = re.compile(rf"({re.escape(_PREFIX)}#)\d+@")


def walk_shape(shape, v, f, on_object=None):
    """Rebuild `v`, replacing id fields named by `shape` with f(kind, value).
    on_object(shape, obj) sees every object before its fields are replaced."""
    if v is None:
        return None
    if isinstance(shape, list):
        return [walk_shape(shape[0], e, f, on_object) for e in v] if isinstance(v, list) else v
    if not isinstance(v, dict):
        return v
    if on_object is not None:
        on_object(shape, v)
    out = dict(v)
    for key, sub in shape.items():
        if key not in out:
            continue
        val = out[key]
        if isinstance(sub, str):
            out[key] = f(sub, val)
        elif isinstance(sub, list) and isinstance(sub[0], str):
            out[key] = [f(sub[0], e) for e in val] if isinstance(val, list) else val
        else:
            out[key] = walk_shape(sub, val, f, on_object)
    return out


def mask_name(val):
    if not isinstance(val, str):
        return val
    val = _EMBEDDED.sub("\u27e8#\u27e9", val)
    val = _INTERNAL_AT.sub(lambda m: f"{m.group(1)}@{m.group(2)}@\u27e8#\u27e9", val)
    return _INTERNAL_PRIVATE.sub(lambda m: f"{m.group(1)}#\u27e8#\u27e9@{m.group(3)}", val)


def mask_name_ids(method, v):
    """Only the ids embedded in symbol names replaced."""
    shape = SHAPES.get(method)
    if shape is None:
        return v
    return walk_shape(shape, v, lambda kind, val: mask_name(val) if kind == "symname" else val)


def mask_ids(method, v):
    """Every symbol, type and signature id replaced by "#"."""
    shape = SHAPES.get(method)
    if shape is None:
        return v
    return walk_shape(shape, v, lambda kind, val: mask_name(val) if kind == "symname"
                      else ("#" if val not in (None, 0) else val))


class Normalizer:
    """Answer normalization for one server process.

    Go symbol ids come from a global counter that concurrent checking races, and goport
    numbers symbols its own way. `finalize` names each symbol by content: "<name>@<first
    declaration handle>", with "#k" for the k-th distinct symbol of that key (by first
    appearance). A symbol id seen only as a reference (never as a full symbol answer) gets
    "?n" by first appearance. Type and signature ids stay raw. Protocol 5: a source file node id
    gets the file's path from a descriptor ("#k" for the k-th distinct node of that path), or "?fn"
    by first appearance; an encoded source file has its node id bytes zeroed."""

    def __init__(self, replacements):
        self.replacements = [(a, b) for a, b in replacements if a]

    def strings(self, v):
        if isinstance(v, str):
            for a, b in self.replacements:
                if a in v:
                    v = v.replace(a, b)
            return v
        if isinstance(v, list):
            return [self.strings(e) for e in v]
        if isinstance(v, dict):
            return {k: self.strings(e) for k, e in v.items()}
        return v

    def prepare(self, method, v):
        """Paths replaced; lists from Go maps sorted by an id-free key."""
        v = self.strings(v)
        if method in UNORDERED:
            ptr = UNORDERED[method]
            lst = pointer_get(v, ptr)
            if isinstance(lst, list):
                key = (lambda e: canon(mask_ids(method, {"entries": [e]}))) if ptr else \
                      (lambda e: canon(mask_ids(method, [e])))
                lst = sorted(lst, key=key)
                v = pointer_set(v, ptr, lst) if ptr else lst
        if PROTOCOL >= 5 and method in SOURCE_FILE_METHODS and isinstance(v, dict) and isinstance(v.get("data"), str):
            data = bytearray(base64.b64decode(v["data"]))
            if len(data) >= SOURCE_FILE_ID.stop:
                data[SOURCE_FILE_ID] = bytes(SOURCE_FILE_ID.stop - SOURCE_FILE_ID.start)
                v = {**v, "data": base64.b64encode(bytes(data)).decode()}
        if method in EMIT_METHODS and isinstance(v, dict):
            for k in ("emittedFiles", "diagnostics"):
                if isinstance(v.get(k), list):
                    v[k] = sorted(v[k], key=canon)
        if method == "getFullyQualifiedName" and isinstance(v, str):
            v = _FQN_AT.sub(lambda m: m.group(1) + "⟨#⟩", v)
            v = _FQN_PRIVATE.sub(lambda m: m.group(1) + "⟨#⟩@", v)
        if method in SNAPSHOT_METHODS and isinstance(v, dict) \
                and isinstance(v.get("changes"), dict):
            ch = v["changes"]
            for proj in (ch.get("changedProjects") or {}).values():
                for k in ("changedFiles", "deletedFiles"):
                    if isinstance(proj, dict) and isinstance(proj.get(k), list):
                        proj[k] = sorted(proj[k])
            if isinstance(ch.get("removedProjects"), list):
                ch["removedProjects"] = sorted(ch["removedProjects"])
        return v

    def finalize(self, entries):
        """entries: [(method, raw answer)] of one process in order. Returns normalized answers."""
        pre = [(m, self.prepare(m, v)) for m, v in entries]
        keyed, node_paths = {}, {}

        def collect(shape, obj):
            if shape is FILE_DESC and isinstance(obj.get("nodeId"), str):
                node_paths.setdefault(obj["nodeId"], obj.get("path"))
                return
            rid = (obj.get("reference") or {}).get("id") if PROTOCOL >= 5 else obj.get("id")
            if shape is SYM and isinstance(rid, int) and rid and rid not in keyed:
                decls = obj.get("declarations") or []
                decl0 = decls[0] if decls else (obj.get("valueDeclaration") or "-")
                keyed[rid] = f"{mask_name(obj.get('name'))}@{decl0}"

        for m, v in pre:
            if SHAPES.get(m) is not None:
                walk_shape(SHAPES[m], v, lambda kind, val: val, collect)
        names, per_key, unknown = {}, collections.Counter(), [0]
        nodes, per_path, unknown_nodes = {}, collections.Counter(), [0]

        def nid(raw):
            if not isinstance(raw, str) or not raw:
                return raw
            if raw not in nodes:
                path = node_paths.get(raw)
                if path is None:
                    unknown_nodes[0] += 1
                    nodes[raw] = f"?f{unknown_nodes[0]}"
                else:
                    per_path[path] += 1
                    nodes[raw] = path if per_path[path] == 1 else f"{path}#{per_path[path]}"
            return nodes[raw]

        def cid(raw, from_name=False):
            if not isinstance(raw, int) or raw == 0:
                return raw
            if raw not in names:
                key = keyed.get(raw)
                if key is None:
                    if from_name:
                        return "?"
                    unknown[0] += 1
                    names[raw] = f"?{unknown[0]}"
                else:
                    per_key[key] += 1
                    names[raw] = key if per_key[key] == 1 else f"{key}#{per_key[key]}"
            return names[raw]

        def f(kind, val):
            if kind == "sym":
                return cid(val)
            if kind == "node":
                return nid(val)
            if kind == "symname" and isinstance(val, str):
                m = _INTERNAL_AT.match(val)
                if m:
                    return f"{m.group(1)}@{m.group(2)}@\u27e8{cid(int(m.group(3)), True)}\u27e9"
                m = _INTERNAL_PRIVATE.match(val)
                if m:
                    return f"{m.group(1)}#\u27e8{cid(int(m.group(2)), True)}\u27e9@{m.group(3)}"
            return val

        return [walk_shape(SHAPES[m], v, f) if SHAPES.get(m) is not None else v for m, v in pre]

    def error(self, err):
        err = err if isinstance(err, dict) else {"message": str(err)}
        message = self.strings(str(err.get("message", "")).split("\n", 1)[0])
        message = re.sub(r"handle (\d+)", "handle #", message)
        # Go json v2 picks "cannot" or "unable to" at random (go-json-experiment errors.go:318).
        message = message.replace("unable to unmarshal", "cannot unmarshal").replace("unable to marshal",
                                                                                     "cannot marshal")
        return {"code": err.get("code"), "message": message}


_UNPORTED_RE = re.compile(r"unported Go code: ([^\n]*)")


def unported_name(message: str):
    m = _UNPORTED_RE.search(message or "")
    return m.group(1).strip() if m else None


# ---------------------------------------------------------------------------
# Transport: JSON-RPC (Content-Length framing) and the msgpack tuple protocol
# ---------------------------------------------------------------------------


class Channel:
    """One connection to a server. A reader thread routes answers to waiters and
    answers server-to-client requests with `handler(method, params) -> result`."""

    def __init__(self, reader, writer, protocol, handler=None, name="api"):
        self.reader, self.writer, self.protocol = reader, writer, protocol
        self.handler = handler
        self.name = name
        self.waiters = {}
        self.lock = threading.Lock()
        self.write_lock = threading.Lock()
        self.closed = False
        self.close_reason = None
        self.thread = threading.Thread(target=self._read_loop, daemon=True)
        self.thread.start()

    # -- framing --
    def _read_exact(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.reader.read(n - len(buf))
            if not chunk:
                raise EOFError
            buf += chunk
        return buf

    def _read_jsonrpc(self):
        length = None
        while True:
            line = self.reader.readline()
            if not line:
                raise EOFError
            line = line.strip()
            if not line:
                if length is None:
                    continue
                break
            k, _, val = line.partition(b":")
            if k.strip().lower() == b"content-length":
                length = int(val.strip())
        return json.loads(self._read_exact(length))

    def _read_bin(self):
        t = self._read_exact(1)[0]
        if t == 0xC4:
            n = self._read_exact(1)[0]
        elif t == 0xC5:
            n = struct.unpack(">H", self._read_exact(2))[0]
        elif t == 0xC6:
            n = struct.unpack(">I", self._read_exact(4))[0]
        else:
            raise HarnessError(f"msgpack: expected bin, got 0x{t:02x}")
        return self._read_exact(n)

    def _read_msgpack(self):
        t = self._read_exact(1)[0]
        if t != 0x93:
            raise HarnessError(f"msgpack: expected 0x93, got 0x{t:02x}")
        mt = self._read_exact(1)[0]
        if mt == 0xCC:
            mt = self._read_exact(1)[0]
        method = self._read_bin().decode("utf-8", "replace")
        payload = self._read_bin()
        return {"mp": mt, "method": method, "payload": payload}

    @staticmethod
    def _bin(b: bytes) -> bytes:
        n = len(b)
        if n < 256:
            return bytes([0xC4, n]) + b
        if n < 1 << 16:
            return b"\xc5" + struct.pack(">H", n) + b
        return b"\xc6" + struct.pack(">I", n) + b

    def _write(self, data: bytes):
        with self.write_lock:
            self.writer.write(data)
            self.writer.flush()

    def _write_jsonrpc(self, msg):
        body = json.dumps(msg, ensure_ascii=False, separators=(",", ":")).encode()
        self._write(b"Content-Length: %d\r\n\r\n" % len(body) + body)

    def _write_msgpack(self, mtype, method, payload: bytes):
        self._write(b"\x93" + bytes([mtype]) + self._bin(method.encode()) + self._bin(payload))

    # -- loop --
    def _deliver(self, key, value):
        with self.lock:
            q = self.waiters.get(key)
        if q is not None:
            q.put(value)

    def _read_loop(self):
        try:
            while True:
                if self.protocol == "msgpack":
                    m = self._read_msgpack()
                    if m["mp"] == 6:  # Call: server asks the client
                        try:
                            params = json.loads(m["payload"] or b"null")
                        except ValueError:
                            params = None
                        result = self.handler(m["method"], params) if self.handler else None
                        self._write_msgpack(2, m["method"], json.dumps(result).encode())
                    elif m["mp"] in (4, 5):
                        self._deliver(m["method"], m)
                    continue
                msg = self._read_jsonrpc()
                if "method" in msg and "id" in msg:
                    result = self.handler(msg["method"], msg.get("params")) if self.handler else None
                    self._write_jsonrpc({"jsonrpc": "2.0", "id": msg["id"], "result": result})
                elif "method" in msg:
                    if self.handler:
                        self.handler("#notification:" + msg["method"], msg.get("params"))
                elif "id" in msg:
                    self._deliver(msg["id"], msg)
        except (EOFError, OSError, ValueError, HarnessError) as e:
            self.close_reason = f"{type(e).__name__}: {e}"
        finally:
            self.closed = True
            with self.lock:
                waiters = list(self.waiters.values())
            for q in waiters:
                q.put(None)

    def request(self, rid, method, params, timeout):
        """Returns ("ok", result) | ("error", err) | ("crash", reason) | ("timeout", None)."""
        key = method if self.protocol == "msgpack" else rid
        q = queue.Queue()
        with self.lock:
            self.waiters[key] = q
        try:
            if self.closed:
                return "crash", self.close_reason or "closed"
            try:
                if self.protocol == "msgpack":
                    payload = json.dumps(params, ensure_ascii=False).encode() if params is not None else b"null"
                    self._write_msgpack(1, method, payload)
                else:
                    msg = {"jsonrpc": "2.0", "id": rid, "method": method}
                    if params is not None:
                        msg["params"] = params
                    self._write_jsonrpc(msg)
            except OSError as e:
                return "crash", f"write: {e}"
            try:
                ans = q.get(timeout=timeout)
            except queue.Empty:
                return "timeout", None
            if ans is None:
                return "crash", self.close_reason or "closed"
            if self.protocol == "msgpack":
                if ans["mp"] == 5:
                    return "error", {"code": -32603, "message": ans["payload"].decode("utf-8", "replace")}
                payload = ans["payload"]
                if method in BINARY_METHODS:
                    if method == "echo":
                        return "ok", {"raw": base64.b64encode(payload).decode()}
                    if payload in (b"", b"null"):
                        return "ok", None
                    return "ok", {"data": base64.b64encode(payload).decode()}
                return "ok", json.loads(payload) if payload else None
            if "error" in ans and ans["error"] is not None:
                return "error", ans["error"]
            return "ok", ans.get("result")
        finally:
            with self.lock:
                self.waiters.pop(key, None)

    def notify(self, method, params):
        self._write_jsonrpc({"jsonrpc": "2.0", "method": method, "params": params})


# ---------------------------------------------------------------------------
# Callback FS (Go: api/callbackfs.go). Answers from the real read-only FS or an overlay.
# ---------------------------------------------------------------------------

CALLBACK_NAMES = ("readFile", "fileExists", "directoryExists", "getAccessibleEntries", "realpath")


class CallbackFS:
    def __init__(self, names, mode, protocol=PROTOCOL):
        self.names = set(names)
        self.mode = mode
        self.protocol = protocol  # 4 with --wire 4: protocol 4 answers for base bins
        self.overlay = {}  # abs path -> text | None (deleted)
        self.calls = set()
        self.writes = []  # writeFile calls since the last take_writes: {path, bytes, sha256}
        self.lock = threading.Lock()

    def take_writes(self):
        """The writeFile calls since the last call, sorted by path (Go emits files in parallel)."""
        with self.lock:
            writes, self.writes = self.writes, []
        return sorted(writes, key=lambda w: (str(w["path"]), w["sha256"]))

    def set_overlay(self, path, content):
        with self.lock:
            if isinstance(content, dict) and content.get("drop"):
                self.overlay.pop(path, None)
            else:
                self.overlay[path] = content

    def handle(self, method, arg):
        ans = self._answer(method, arg)
        return callback_kind(method, ans) if self.protocol >= 5 and not method.startswith("#notification:") else ans

    def _answer(self, method, arg):
        """The protocol 4 answer: None is "use the real FS"; readFile {"content": null} is "missing"."""
        if method == "writeFile" and method in self.names:
            # tsgo#4699 (Go: callbackfs.go WriteFile {path, data}). Recorded, never written.
            arg = arg if isinstance(arg, dict) else {}
            data = str(arg.get("data") or "").encode("utf-8")
            with self.lock:
                self.calls.add((method, str(arg.get("path") or "")))
                self.writes.append({"path": arg.get("path"), "bytes": len(data), "sha256": sha256_bytes(data)})
            return None
        if method.startswith("#notification:") or method not in CALLBACK_NAMES:
            return None
        path = arg if isinstance(arg, str) else ""
        with self.lock:
            self.calls.add((method, path))
            ov = dict(self.overlay)
        real = self.mode == "real" and not path.startswith("bundled:")
        if method == "readFile":
            if path in ov:
                return {"content": ov[path]}
            if real:
                try:
                    with open(path, "r", encoding="utf-8", newline="") as f:
                        return {"content": f.read()}
                except (OSError, UnicodeDecodeError):
                    return {"content": None}
            return None
        if method == "fileExists":
            if path in ov:
                return ov[path] is not None
            return os.path.isfile(path) if real else None
        if method == "directoryExists":
            if any(p.startswith(path.rstrip("/") + "/") and c is not None for p, c in ov.items()):
                return True
            return os.path.isdir(path) if real else None
        if method == "getAccessibleEntries":
            base = path.rstrip("/") + "/"
            extra = {p[len(base):]: c for p, c in ov.items() if p.startswith(base) and "/" not in p[len(base):]}
            if not real and not extra:
                return None
            files, dirs = [], []
            try:
                for e in os.scandir(path):
                    try:
                        if e.is_dir():
                            dirs.append(e.name)
                        elif e.is_file():
                            files.append(e.name)
                    except OSError:
                        pass
            except OSError:
                pass
            for name, c in extra.items():
                if c is None and name in files:
                    files.remove(name)
                elif c is not None and name not in files:
                    files.append(name)
            return {"files": sorted(files), "directories": sorted(dirs)}
        if method == "realpath":
            if path in ov:
                return path
            return os.path.realpath(path) if real else None
        return None

    def summary(self, norm):
        return sorted({f"{m} {norm.strings(p)}" for m, p in self.calls})


def callback_kind(method, ans):
    """Protocol 5 (#64447, Go: callbackfs.go decodeCallbackResponse): every callback answer is {kind, value?}. The
    protocol 4 answer maps to: None -> useOS (the real FS), readFile {"content": null} -> missing, writeFile ->
    noop (recorded, never written), else value."""
    if method == "writeFile":
        return {"kind": "noop"}
    if ans is None:
        return {"kind": "useOS"}
    if method == "readFile":
        return {"kind": "missing"} if ans.get("content") is None else {"kind": "value", "value": ans["content"]}
    return {"kind": "value", "value": ans}


# ---------------------------------------------------------------------------
# Servers
# ---------------------------------------------------------------------------


class Server:
    """A running server: stdio API (`--api [--async]`) or an LSP server with an API socket."""

    def __init__(self, binary, header, run_dir, cbfs, label):
        self.binary, self.header, self.run_dir, self.cbfs, self.label = binary, header, run_dir, cbfs, label
        self.proc = None
        self.api = None
        self.lsp = None
        self.sock = None
        self.lsp_records = []
        self.stderr_path = os.path.join(run_dir, f"{label}.stderr")

    def env(self):
        env = dict(os.environ)
        env["HOME"] = self.run_dir
        env["TMPDIR"] = self.run_dir
        return env

    def start(self):
        h = self.header
        cwd = h.get("cwd") or h["project"]["dir"]
        stderr = open(self.stderr_path, "ab")
        if h.get("transport", "stdio") == "lsp":
            self.proc = subprocess.Popen([self.binary, "--lsp", "--stdio"], stdin=subprocess.PIPE,
                                         stdout=subprocess.PIPE, stderr=stderr, cwd=cwd, env=self.env())
            stderr.close()
            self._start_lsp(cwd)
            return
        argv = [self.binary, "--api", "--cwd", cwd]
        if h.get("protocol", "jsonrpc") == "jsonrpc":
            argv.append("--async")
        if h.get("callbacks"):
            argv += ["--callbacks", ",".join(h["callbacks"])]
        self.proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr,
                                     cwd=cwd, env=self.env())
        stderr.close()
        self.api = Channel(self.proc.stdout, self.proc.stdin, h.get("protocol", "jsonrpc"),
                           self.cbfs.handle if self.cbfs else None)

    def _lsp_handler(self, method, params):
        if method.startswith("#notification:"):
            if method == "#notification:window/logMessage" and isinstance(params, dict):
                with open(self.stderr_path + ".lsplog", "a", encoding="utf-8") as f:
                    f.write(str(params.get("message")) + "\n")
            return None
        if method == "workspace/configuration":
            return [None for _ in (params or {}).get("items", [])]
        return None

    def _start_lsp(self, cwd):
        self.lsp = Channel(self.proc.stdout, self.proc.stdin, "jsonrpc", self._lsp_handler, name="lsp")
        root_uri = "file://" + cwd
        init = {"processId": None, "rootUri": root_uri, "capabilities": {},
                "workspaceFolders": [{"uri": root_uri, "name": os.path.basename(cwd)}]}
        st, res = self.lsp.request(1, "initialize", init, START_TIMEOUT)
        if st != "ok":
            raise HarnessError(f"lsp initialize: {st} {res}")
        self.lsp.notify("initialized", {})
        for rel in self.header.get("lspOpen", []):
            path = os.path.join(self.header["project"]["dir"], rel)
            with open(path, encoding="utf-8") as f:
                text = f.read()
            self.lsp.notify("textDocument/didOpen", {"textDocument": {
                "uri": "file://" + path, "languageId": "typescript", "version": 1, "text": text}})
        # Go creates the project session in handleInitialized (lsp/server.go:1238), and
        # initializeAPISession captures it. A request that needs the session waits for it.
        for n, rel in enumerate(self.header.get("lspOpen", [])):
            uri = "file://" + os.path.join(self.header["project"]["dir"], rel)
            st, res = self.lsp.request(100 + n, "textDocument/documentSymbol", {"textDocument": {"uri": uri}},
                                       START_TIMEOUT * 2)
            self.lsp_records.append(("textDocument/documentSymbol", st, None if st == "ok" else res))
        pipe = os.path.join(self.run_dir, f"api{self.label.lstrip('abcdefghijklmnopqrstuvwxyz')}.sock")
        st, res = self.lsp.request(2, "custom/initializeAPISession", {"pipe": pipe}, START_TIMEOUT)
        self.lsp_records.append(("custom/initializeAPISession", st, res))
        if st != "ok":
            raise HarnessError(f"custom/initializeAPISession: {st} {res}")
        deadline = time.time() + 10
        while True:
            try:
                self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                self.sock.connect(pipe)
                break
            except OSError:
                self.sock.close()
                if time.time() > deadline:
                    raise HarnessError(f"cannot connect to {pipe}")
                time.sleep(0.05)
        self.api = Channel(self.sock.makefile("rb"), self.sock.makefile("wb"), "jsonrpc", None)

    def stop(self):
        """Ends the session. Returns the exit code (None if killed)."""
        code = None
        try:
            if self.sock is not None:
                try:
                    self.sock.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                self.sock.close()
                if self.lsp is not None and not self.lsp.closed:
                    self.lsp.request(3, "shutdown", None, 10)
                    try:
                        self.lsp.notify("exit", None)
                    except OSError:
                        pass
            if self.proc is not None:
                try:
                    self.proc.stdin.close()
                except OSError:
                    pass
                try:
                    code = self.proc.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    self.proc.kill()
                    self.proc.wait()
        finally:
            if self.proc is not None and self.proc.poll() is None:
                self.proc.kill()
                self.proc.wait()
        return code

    def kill(self):
        if self.proc is not None and self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait()

    def stderr_tail(self, n=400):
        try:
            with open(self.stderr_path, "rb") as f:
                data = f.read()
            return data[-n:].decode("utf-8", "replace")
        except OSError:
            return ""


# ---------------------------------------------------------------------------
# Session run (record and check)
# ---------------------------------------------------------------------------


def load_trace(path):
    with open(path, encoding="utf-8") as f:
        lines = [json.loads(line) for line in f if line.strip()]
    if not lines or lines[0].get("format") != TRACE_FORMAT:
        raise UsageError(f"{path}: not a {TRACE_FORMAT} trace")
    return lines[0], lines[1:]


def write_fixture(run_dir, fixture):
    """Writes the header's fixture files under the run dir (never outside it)."""
    for rel, text in sorted((fixture or {}).items()):
        path = os.path.normpath(os.path.join(run_dir, rel))
        if os.path.isabs(rel) or not path.startswith(os.path.join(run_dir, "")):
            raise HarnessError(f"fixture path {rel!r} is not under the run dir")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(text)


def eval_when(src, conds):
    for c in conds or []:
        v = pointer_get(src, c.get("pointer", ""))
        if v is MISSING or v is None:
            return False
        if "mask" in c and not (isinstance(v, int) and v & c["mask"]):
            return False
        if c.get("nonzero") and v in (0, "", [], {}, False):
            return False
    return True


def resolve_spec(spec, raw_results):
    src = raw_results.get(spec["event"], MISSING)
    if src is MISSING:
        return MISSING, f"event {spec['event']} has no answer"
    if not eval_when(src, spec.get("when")):
        return MISSING, "when"
    value = pointer_get(src, spec.get("pointer", ""))
    if value is MISSING or value is None:
        return MISSING, f"no value at {spec.get('pointer', '')}"
    pick = spec.get("pick")
    if pick is not None:
        if not isinstance(value, list):
            return MISSING, "pick on a non-array"
        items = value
        if "where" in pick:
            w = pick["where"]
            items = [e for e in items if isinstance(e, dict) and isinstance(pointer_get(e, "/" + w["field"]), int)
                     and pointer_get(e, "/" + w["field"]) & w["mask"]]
        if "sortBy" in pick:
            items = sorted(items, key=lambda e: [canon(e.get(f)) if isinstance(e, dict) else "" for f in pick["sortBy"]])
        idx = pick.get("index", 0)
        if not -len(items) <= idx < len(items):
            return MISSING, f"index {idx} of {len(items)}"
        value = items[idx]
        if "then" in pick:
            value = pointer_get(value, pick["then"])
            if value is MISSING or value is None:
                return MISSING, f"no value at {pick['then']}"
    return value, None


def build_params(ev, raw_results, ctx):
    params = expand(copy.deepcopy(ev.get("params")), ctx)
    pf = ev.get("paramsFrom")
    if pf is None:
        return params, None
    specs = pf if isinstance(pf, list) else [pf]
    if params is None:
        params = {}
    for spec in specs:
        value, reason = resolve_spec(spec, raw_results)
        if value is MISSING:
            if spec.get("optional"):
                continue
            return MISSING, reason
        value = copy.deepcopy(value)
        if "append" in spec:
            lst = pointer_get(params, spec["append"])
            if not isinstance(lst, list):
                params = pointer_set(params, spec["append"], [])
                lst = pointer_get(params, spec["append"])
            lst.append(value)
        elif "into" in spec:
            params = pointer_set(params, spec["into"], value)
        else:
            params = value
    return params, None


_PROJECT_RE = re.compile(r"^@PROJECT(?::(.+))?@$")


def expand(v, ctx):
    if isinstance(v, str):
        if v == "@SNAPSHOT@":
            return ctx["snapshot"] if ctx.get("snapshot") is not None else v
        m = _PROJECT_RE.match(v)
        if m:
            rel = (m.group(1) or ctx["tsconfig"]).replace("@RUN_DIR@", ctx["run_dir"])
            want = os.path.normpath(os.path.join(ctx["project_dir"], rel))
            projects = ctx.get("projects") or []
            for p in projects:
                if p.get("configFileName") == want:
                    return p.get("id")
            return projects[0].get("id") if projects and not m.group(1) else v
        return v.replace("@PROJECT_DIR@", ctx["project_dir"]).replace("@RUN_DIR@", ctx["run_dir"])
    if isinstance(v, list):
        return [expand(e, ctx) for e in v]
    if isinstance(v, dict):
        return {expand(k, ctx): expand(e, ctx) for k, e in v.items()}  # keys: the files of a layer
    return v


def wire3(method, params):
    """(method, params) of a protocol 4 snapshot event in its protocol 3 form: the inverse of TraceBuilder.snap
    and TraceBuilder.temp. Other events come back unchanged. Used by `check --wire 3` (base bins that speak
    protocol 3) and to compare N traces with B traces."""
    if method not in SNAPSHOT_METHODS[:3]:
        return method, params
    p = params or {}
    changes = p if method == "createSnapshot" else p.get("changes") or {}
    fs = changes.get("fileSystem")
    if method == "updateSnapshot" and isinstance(fs, dict) and fs.get("kind") == "layer":
        (file, text), = fs["files"].items()
        return "updateTemporarySnapshot", {**({"snapshot": p["snapshot"]} if "snapshot" in p else {}),
                                           "file": file, "newText": text}
    b = {k: v for k, v in changes.items() if k not in ("fileNotifications", "ensurePrograms")}
    if "fileNotifications" in changes:
        b["fileChanges"] = changes["fileNotifications"]
    return "updateSnapshot", b


def wire3_event(ev):
    """The event with wire3 applied (and no "track" key); overlays and other requests unchanged."""
    if ev.get("kind") != "request" or ev["method"] not in SNAPSHOT_METHODS[:3]:
        return ev
    method, params = wire3(ev["method"], ev.get("params"))
    out = {k: v for k, v in ev.items() if k != "track"}
    out.update(method=method, params=params)
    return out


class SessionRun:
    """Runs one trace against one server. mode "record" keeps every answer; mode "check"
    follows the golden's skip decisions."""

    def __init__(self, header, events, binary, role, tmp_root, golden=None, timeout=None, keep=False, wire=None,
                 wire_kinds=None):
        self.header, self.events, self.binary, self.role = header, events, binary, role
        self.wire = wire  # 3: protocol 4 traces in their protocol 3 form; 4: protocol 5 traces in protocol 4 form
        if wire == 3:
            self.events = [wire3_event(ev) for ev in events]
        elif wire == 4:
            self.events = [wire4_event(ev, wire_kinds) for ev in events]
        self.golden = golden  # event -> golden record (check mode)
        self.timeout = timeout or REQUEST_TIMEOUT[role]
        self.tmp_root, self.keep = tmp_root, keep

    def run(self):
        h = self.header
        run_dir = tempfile.mkdtemp(prefix="api-", dir=self.tmp_root)
        pdir = h["project"]["dir"]
        self.ctx = {"project_dir": pdir, "tsconfig": h["project"]["tsconfig"], "run_dir": run_dir,
                    "snapshot": None, "projects": []}
        cbfs = CallbackFS(h.get("callbacks") or [], h.get("callbackMode", "fallback"),
                          4 if self.wire == 4 else PROTOCOL) if h.get("callbacks") else None
        norm = Normalizer([(run_dir, "@RUN_DIR@"), (pdir, "@PROJECT_DIR@")])
        records = []
        epochs = [[]]  # answers of each server process, for Normalizer.finalize
        crashed = set()
        restarts = 0
        exit_codes = []
        t_start = time.time()
        write_fixture(run_dir, h.get("fixture"))
        server = self._start(run_dir, cbfs, 0)
        try:
            if server is None:
                return self._all_failed("start failed", run_dir), {"exitCodes": exit_codes}
            self._lsp_records(server, norm, records)
            raw = {}
            k = 0
            while k < len(self.events):
                ev = self.events[k]
                if ev.get("kind") == "overlay":
                    if cbfs:
                        cbfs.set_overlay(expand(ev["path"], self.ctx), ev.get("content"))
                    records.append({"event": k, "method": "@overlay", "status": "fs"})
                    k += 1
                    continue
                method = ev["method"]
                g = (self.golden or {}).get(k)
                if g is not None and g.get("status") == "skipped":
                    records.append({"event": k, "method": method, "status": "skipped", "reason": "golden skipped"})
                    k += 1
                    continue
                params, reason = build_params(ev, raw, self.ctx)
                if params is MISSING:
                    st = "not_run" if g is not None else "skipped"
                    records.append({"event": k, "method": method, "status": st, "reason": reason})
                    k += 1
                    continue
                guard = self._emit_guard(method, params, run_dir)
                if guard:
                    log(f"{h['name']} event {k}: {guard}")
                    records.append({"event": k, "method": method, "status": "not_run", "reason": guard})
                    k += 1
                    continue
                if cbfs is not None:
                    cbfs.take_writes()
                t0 = time.time()
                status, ans = server.api.request(k + 1, method, params, self.timeout)
                ms = int((time.time() - t0) * 1000)
                writes = cbfs.take_writes() if cbfs is not None else []
                if status == "ok" and isinstance(ans, dict) and (method == "emit" or writes):
                    ans = {**ans, "@writes": writes}
                rec = {"event": k, "method": method, "status": status, "ms": ms, "sent": norm.strings(params)}
                if restarts:
                    rec["epoch"] = restarts
                if status == "ok":
                    raw[k] = ans
                    self._track(ev, params, ans)
                    rec["_ref"] = (len(epochs) - 1, len(epochs[-1]))
                    epochs[-1].append((method, ans))
                elif status == "error":
                    rec["response"] = {"error": norm.error(ans)}
                    name = unported_name(str(ans.get("message", "")) if isinstance(ans, dict) else str(ans))
                    if name:
                        rec["unported"] = name
                if status in ("crash", "timeout"):
                    rec["reason"] = ans
                    server.kill()
                    exit_codes.append(server.proc.returncode if server.proc else None)
                    rec["exitCode"] = exit_codes[-1]
                    rec["stderr"] = norm.strings(server.stderr_tail())
                    name = unported_name(rec["stderr"])
                    if name:
                        rec["unported"] = name
                    crashed.add(k)
                    records.append(rec)
                    restarts += 1
                    if restarts > MAX_RESTARTS:
                        for j in range(k + 1, len(self.events)):
                            e = self.events[j]
                            records.append({"event": j, "method": e.get("method", "@overlay"),
                                            "status": "not_run", "reason": "restart limit"})
                        server = None
                        break
                    epochs.append([])
                    server, raw = self._restart(run_dir, cbfs, restarts, k, crashed, epochs[-1])
                    if server is None:
                        for j in range(k + 1, len(self.events)):
                            e = self.events[j]
                            records.append({"event": j, "method": e.get("method", "@overlay"),
                                            "status": "not_run", "reason": "restart failed"})
                        break
                    k += 1
                    continue
                records.append(rec)
                k += 1
            if server is not None:
                exit_codes.append(server.stop())
            if cbfs is not None:
                records.append({"event": len(self.events), "method": "@callbacks", "status": "ok",
                                "response": {"result": cbfs.summary(norm)}})
            finals = [norm.finalize(e) for e in epochs]
            for rec in records:
                ref = rec.pop("_ref", None)
                if ref is not None:
                    rec["response"] = {"result": finals[ref[0]][ref[1]]}
        finally:
            if server is not None:
                server.kill()
            if not self.keep:
                shutil.rmtree(run_dir, ignore_errors=True)
        meta = {"exitCodes": exit_codes, "restarts": restarts, "seconds": round(time.time() - t_start, 1)}
        return records, meta

    def _track(self, ev, params, ans):
        """@SNAPSHOT@ and the project list from a snapshot answer. Protocol 3 (and --wire 3): each updateSnapshot
        answer replaces both. Protocol 4: a snapshot method answer, except a "track": false event. With a base
        (updateSnapshot, or getCurrentLanguageServerSnapshot with baseSnapshot) the answer has only the added
        or replaced projects: they replace the projects of the same id, and changes.removedProjects go. The
        base of every tracked update in the traces is @SNAPSHOT@, so the current list is the base's list."""
        method = ev["method"]
        if not isinstance(ans, dict) or ev.get("track") is False:
            return
        if PROTOCOL < 4 or self.wire == 3:
            if method == "updateSnapshot":
                self.ctx["snapshot"] = ans.get("snapshot")
                self.ctx["projects"] = ans.get("projects") or []
            return
        if method not in SNAPSHOT_METHODS:
            return
        projects = ans.get("projects") or []
        if method == "updateSnapshot" or (isinstance(params, dict) and params.get("baseSnapshot")):
            removed = set((ans.get("changes") or {}).get("removedProjects") or [])
            new = {p.get("id"): p for p in projects}
            projects = [new.pop(p.get("id"), p) for p in self.ctx["projects"] if p.get("id") not in removed]
            projects += new.values()
        self.ctx["snapshot"] = ans.get("snapshot")
        self.ctx["projects"] = projects

    def _emit_guard(self, method, params, run_dir):
        """None when the request may be sent. tsgo#4699 `emit` writes through the session FS (the real disk
        without the writeFile callback), so it goes only to a fixture project under the run dir, with the
        callback. Then no run writes into a project input, also when a server ignores the callback."""
        if method != "emit":
            return None
        h = self.header
        if "writeFile" not in (h.get("callbacks") or []) or not h.get("fixture"):
            return "emit guard: the trace has no writeFile callback or no fixture"
        pid = params.get("project") if isinstance(params, dict) else None
        proj = next((p for p in self.ctx["projects"] if p.get("id") == pid), None)
        cfg = str((proj or {}).get("configFileName") or "")
        if not cfg.startswith(os.path.join(run_dir, "")):
            return "emit guard: the project config is not under the run dir"
        return None

    def _start(self, run_dir, cbfs, n):
        server = Server(self.binary, self.header, run_dir, cbfs, f"{self.role}{n}")
        try:
            server.start()
        except (HarnessError, OSError) as e:
            log(f"start {self.header['name']}: {e}")
            server.kill()
            return None
        return server

    def _lsp_records(self, server, norm, records):
        for method, st, res in server.lsp_records:
            rec = {"event": -1, "method": "@lsp:" + method, "status": st}
            rec["response"] = {"result": norm.strings(res)} if st == "ok" else {"error": norm.error(res)}
            records.append(rec)

    def _restart(self, run_dir, cbfs, n, upto, crashed, epoch):
        """New server; replay events before `upto` silently (not the crashed ones). The
        replayed answers go to `epoch` (they name the symbols of the new process)."""
        self.ctx["snapshot"], self.ctx["projects"] = None, []
        del epoch[:]
        server = self._start(run_dir, cbfs, n)
        if server is None:
            return None, None
        raw = {}
        for j in range(upto):
            ev = self.events[j]
            if ev.get("kind") != "request" or j in crashed:
                continue
            g = (self.golden or {}).get(j)
            if g is not None and g.get("status") == "skipped":
                continue
            params, _ = build_params(ev, raw, self.ctx)
            if params is MISSING or self._emit_guard(ev["method"], params, run_dir):
                continue
            st, ans = server.api.request(j + 1, ev["method"], params, self.timeout)
            if st == "ok":
                raw[j] = ans
                self._track(ev, params, ans)
                epoch.append((ev["method"], ans))
            elif st in ("crash", "timeout"):
                crashed.add(j)
                server.kill()
                return self._restart(run_dir, cbfs, n + 1, upto, crashed, epoch) if n < MAX_RESTARTS else (None, None)
        return server, raw

    def _all_failed(self, reason, run_dir):
        if not self.keep:
            shutil.rmtree(run_dir, ignore_errors=True)
        return [{"event": k, "method": e.get("method", "@overlay"), "status": "crash", "reason": reason}
                for k, e in enumerate(self.events)]


# ---------------------------------------------------------------------------
# Classification
# ---------------------------------------------------------------------------


def classify(g, r, flaky_ev):
    """Returns (class, sub, pointer) for one event."""
    gs, rs = g.get("status"), r.get("status")
    if gs == "skipped":
        return "skipped", None, None
    if gs == "fs":
        return "fs", None, None
    if gs in ("crash", "timeout", "not_run"):
        return ("oracle_crash", gs, None) if rs == "ok" else ("oracle_error_same", gs, None)
    if rs in ("crash", "timeout"):
        sub = "unported:" + r["unported"] if r.get("unported") else f"exit {r.get('exitCode')}"
        return rs, sub, None
    if rs == "not_run":
        return "not_run", r.get("reason"), None
    if rs == "error" and r.get("unported"):
        return "crash", "unported:" + r["unported"], None
    g_resp, r_resp = g.get("response") or {}, r.get("response") or {}
    if gs == "error":
        if rs == "error" and canon(g_resp) == canon(r_resp):
            return "oracle_error_same", None, None
        if rs == "error" and flaky_ev and flaky_ev.get("unstable") == "error":
            return "flaky_oracle", "error", None
        sub = (r_resp.get("error") or {}).get("message") if rs == "error" else None
        return "oracle_error_diff", sub, None
    if rs == "error":
        return "goport_error", (r_resp.get("error") or {}).get("message"), None
    patterns = (flaky_ev or {}).get("multiset") or []
    a = apply_multisets(g_resp.get("result"), patterns)
    b = apply_multisets(r_resp.get("result"), patterns)
    if canon(a) == canon(b):
        return "same", None, None
    if flaky_ev and flaky_ev.get("unstable"):
        return "flaky_oracle", flaky_ev.get("unstable"), None
    method = g.get("method")
    if canon(mask_ids(method, a)) == canon(mask_ids(method, b)):
        # After a goport restart the replay skips the crashed request, so later ids can shift.
        if r.get("epoch"):
            sub = "after restart"
        elif canon(mask_name_ids(method, a)) == canon(mask_name_ids(method, b)):
            sub = "id in a symbol name"
        else:
            sub = None
        return "id_only", sub, pointer_from_path(first_diff_path(a, b) or ())
    return "diff", None, pointer_from_path(first_diff_path(a, b) or ())


# ---------------------------------------------------------------------------
# Files
# ---------------------------------------------------------------------------


def write_json(path, value):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(value, f, indent=1, sort_keys=True, ensure_ascii=False)
        f.write("\n")
    os.replace(tmp, path)


def read_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def write_gz_jsonl(path, header, records):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    with gzip.open(tmp, "wt", encoding="utf-8") as f:
        f.write(json.dumps(header, ensure_ascii=False) + "\n")
        for r in records:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    os.replace(tmp, path)


def read_gz_jsonl(path):
    with gzip.open(path, "rt", encoding="utf-8") as f:
        lines = [json.loads(line) for line in f if line.strip()]
    return lines[0], lines[1:]


def list_traces(out_root, battery, only=None):
    d = os.path.join(out_root, "traces", battery)
    if not os.path.isdir(d):
        raise UsageError(f"no traces in {d}")
    names = sorted(f[:-6] for f in os.listdir(d) if f.endswith(".jsonl"))
    if only:
        names = [n for n in names if only in n]
    return [(n, os.path.join(d, n + ".jsonl")) for n in names]


def golden_dir(out_root, oracle_sha, battery):
    return os.path.join(out_root, "golden", oracle_sha[:12], battery)


def records_by_event(records):
    return {r["event"]: r for r in records if r.get("event", -1) >= 0 and not r["method"].startswith("@callbacks")}


def special_records(records):
    return {r["method"]: r for r in records if r["method"].startswith("@lsp:") or r["method"] == "@callbacks"}


def input_fingerprint(project_dir, rels):
    h = hashlib.sha256()
    for rel in sorted(set(rels)):
        p = os.path.join(project_dir, rel)
        try:
            st = os.stat(p)
            h.update(f"{rel} {st.st_size} {st.st_mtime_ns}\n".encode())
        except OSError:
            h.update(f"{rel} missing\n".encode())
    return h.hexdigest()


def trace_inputs(header, events):
    rels = {header["project"]["tsconfig"]}
    pdir = header["project"]["dir"]
    for ev in events:
        params = ev.get("params") if isinstance(ev.get("params"), dict) else {}
        for key in ("file", "fileName"):
            f = params.get(key)
            if isinstance(f, str) and f.startswith("@PROJECT_DIR@/"):
                rels.add(f[len("@PROJECT_DIR@/"):])
            elif key == "file" and ev.get("method") == "readConfigFile" and isinstance(f, str) \
                    and not f.startswith(("/", "@", "bundled:")):
                rels.add(f)  # relative to the cwd, the project dir
    return pdir, sorted(rels)


def run_jobs(jobs, items, fn):
    results = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, jobs)) as ex:
        futs = {ex.submit(fn, it): it for it in items}
        for fut in concurrent.futures.as_completed(futs):
            results.append(fut.result())
    return results


def cleanup_tmp(tmp_root, keep):
    if keep:
        log(f"kept {tmp_root}")
    else:
        shutil.rmtree(tmp_root, ignore_errors=True)


def make_tmp_root():
    base = os.environ.get("API_ORACLE_TMP", "/tmp")
    return tempfile.mkdtemp(prefix=f"goport-api-{os.getpid()}-", dir=base)


# ---------------------------------------------------------------------------
# record / selfcheck / check
# ---------------------------------------------------------------------------


def cmd_record(args):
    oracle_sha = sha256_file(args.oracle)
    traces = list_traces(args.out_root, args.battery, args.only)
    gdir = golden_dir(args.out_root, oracle_sha, args.battery)
    tmp_root = make_tmp_root()
    counts = collections.Counter()
    failed = []

    def one(item):
        name, path = item
        tsha = sha256_file(path)
        out = os.path.join(gdir, name + ".golden.jsonl.gz")
        if not args.force and os.path.exists(out):
            try:
                gh, _ = read_gz_jsonl(out)
                if gh.get("traceSha") == tsha:
                    return name, "kept", None
            except (OSError, ValueError):
                pass
        header, events = load_trace(path)
        pdir, rels = trace_inputs(header, events)
        fp0 = input_fingerprint(pdir, rels)
        records, meta = SessionRun(header, events, args.oracle, "oracle", tmp_root,
                                   timeout=args.request_timeout, keep=args.keep_temp).run()
        if input_fingerprint(pdir, rels) != fp0:
            raise InputChanged(f"{pdir} changed while recording {name}")
        gh = {"format": GOLDEN_FORMAT, "trace": name, "battery": args.battery, "traceSha": tsha,
              "oracle": args.oracle, "oracleSha": oracle_sha, "date": time.strftime("%Y-%m-%d %H:%M:%S"),
              "meta": meta}
        write_gz_jsonl(out, gh, records)
        st = collections.Counter(r["status"] for r in records)
        log(f"record {name}: {dict(st)} {meta.get('seconds')}s")
        return name, "recorded", st

    try:
        for name, what, st in run_jobs(args.jobs, traces, one):
            counts[what] += 1
            if st:
                for k, v in st.items():
                    counts["status:" + k] += v
                if st.get("crash") or st.get("timeout"):
                    failed.append(name)
    finally:
        cleanup_tmp(tmp_root, args.keep_temp)
    summary = {"battery": args.battery, "oracle": args.oracle, "oracleSha": oracle_sha, "traces": len(traces),
               "counts": dict(counts), "oracleCrashTraces": sorted(failed), "date": time.strftime("%Y-%m-%d %H:%M:%S")}
    write_json(os.path.join(gdir, "record-summary.json"), summary)
    print(json.dumps(summary, indent=1, sort_keys=True))
    return EXIT_OK


def cmd_selfcheck(args):
    oracle_sha = sha256_file(args.oracle)
    traces = list_traces(args.out_root, args.battery, args.only)
    gdir = golden_dir(args.out_root, oracle_sha, args.battery)
    tmp_root = make_tmp_root()
    totals = collections.Counter()

    def one(item):
        name, path = item
        gpath = os.path.join(gdir, name + ".golden.jsonl.gz")
        if not os.path.exists(gpath):
            return name, None
        header, events = load_trace(path)
        _, grecs = read_gz_jsonl(gpath)
        gmap = records_by_event(grecs)
        gspecial = special_records(grecs)
        flaky = {}
        for run in range(args.runs):
            recs, _ = SessionRun(header, events, args.oracle, "oracle", tmp_root, golden=gmap,
                                 timeout=args.request_timeout).run()
            rmap = records_by_event(recs)
            rsp = special_records(recs)
            pairs = [(k, g, rmap.get(k)) for k, g in gmap.items()]
            pairs += [(m, g, rsp.get(m)) for m, g in gspecial.items()]
            for k, g, r in pairs:
                if r is None or g.get("status") in ("skipped", "fs"):
                    continue
                key = str(k)
                entry = flaky.get(key, {})
                if g.get("status") != r.get("status"):
                    entry["unstable"] = "status"
                elif g.get("status") == "ok":
                    a, b = g["response"].get("result"), r["response"].get("result")
                    pats, ptr = find_unstable(a, b)
                    if pats:
                        entry["multiset"] = sorted(set(entry.get("multiset", [])) | set(pats))
                    if ptr is not None:
                        entry["unstable"] = ptr
                elif canon(g.get("response")) != canon(r.get("response")):
                    entry["unstable"] = "error"
                if entry:
                    entry["method"] = g.get("method")
                    flaky[key] = entry
        write_json(os.path.join(gdir, name + ".flaky.json"),
                   {"format": FLAKY_FORMAT, "trace": name, "runs": args.runs, "events": flaky})
        log(f"selfcheck {name}: {len(flaky)} unstable events")
        return name, flaky

    try:
        per = {}
        for name, flaky in run_jobs(args.jobs, traces, one):
            if flaky is None:
                continue
            per[name] = len(flaky)
            for e in flaky.values():
                totals["unstable" if e.get("unstable") else "multiset"] += 1
                totals["method:" + str(e.get("method"))] += 1
    finally:
        shutil.rmtree(tmp_root, ignore_errors=True)
    summary = {"battery": args.battery, "runs": args.runs, "totals": dict(totals),
               "tracesWithUnstable": {k: v for k, v in sorted(per.items()) if v}}
    write_json(os.path.join(gdir, "selfcheck-summary.json"), summary)
    print(json.dumps(summary, indent=1, sort_keys=True))
    return EXIT_OK


def cmd_check(args):
    if args.wire and PROTOCOL != args.wire + 1:
        raise UsageError(f"--wire {args.wire} needs protocol {args.wire + 1} traces (pin {PIN} has protocol "
                         f"{PROTOCOL})")
    oracle_sha = args.oracle_sha or sha256_file(args.oracle)
    traces = list_traces(args.out_root, args.battery, args.only)
    gdir = golden_dir(args.out_root, oracle_sha, args.battery)
    rdir = os.path.join(args.out_root, "results", args.label)
    tmp_root = make_tmp_root()
    goport_sha = sha256_file(args.goport)
    wire_kinds = wire4_kinds() if args.wire == 4 else None

    def one(item):
        name, path = item
        gpath = os.path.join(gdir, name + ".golden.jsonl.gz")
        if not os.path.exists(gpath):
            return name, None
        header, events = load_trace(path)
        gh, grecs = read_gz_jsonl(gpath)
        if gh.get("traceSha") != sha256_file(path):
            log(f"check {name}: golden is for another trace version; record again")
            return name, None
        fpath = os.path.join(gdir, name + ".flaky.json")
        flaky = read_json(fpath)["events"] if os.path.exists(fpath) else {}
        gmap = records_by_event(grecs)
        pdir, rels = trace_inputs(header, events)
        fp0 = input_fingerprint(pdir, rels)
        recs, meta = SessionRun(header, events, args.goport, "goport", tmp_root, golden=gmap,
                                timeout=args.request_timeout, keep=args.keep_temp, wire=args.wire,
                                wire_kinds=wire_kinds).run()
        if args.wire:
            meta["wire"] = args.wire
        if input_fingerprint(pdir, rels) != fp0:
            raise InputChanged(f"{pdir} changed while checking {name}")
        rmap = records_by_event(recs)
        out = []
        earlier = False  # an earlier request differed: later ids can shift (types made or not made)
        for k in sorted(gmap):
            g, r = gmap[k], rmap.get(k, {"status": "not_run", "reason": "no record"})
            cls, sub, ptr = classify(g, r, flaky.get(str(k)))
            if cls == "id_only" and sub is None and earlier:
                sub = "after an earlier difference"
            if cls not in ("same", "skipped", "fs", "oracle_error_same", "id_only", "flaky_oracle"):
                earlier = True
            out.append({"event": k, "method": g.get("method"), "class": cls, "sub": sub, "pointer": ptr,
                        "ms": [g.get("ms"), r.get("ms")]})
        gsp, rsp = special_records(grecs), special_records(recs)
        for m, g in gsp.items():
            r = rsp.get(m, {"status": "not_run", "reason": "no record"})
            cls, sub, ptr = classify(g, r, flaky.get(m))
            out.append({"event": m, "method": m, "class": cls, "sub": sub, "pointer": ptr})
        write_json(os.path.join(rdir, "traces", args.battery, name + ".json"),
                   {"format": RESULT_FORMAT, "trace": name, "battery": args.battery, "meta": meta, "events": out})
        write_gz_jsonl(os.path.join(rdir, "responses", args.battery, name + ".jsonl.gz"),
                       {"trace": name, "goport": args.goport, "goportSha": goport_sha}, recs)
        c = collections.Counter(e["class"] for e in out)
        log(f"check {name}: {dict(c)} {meta.get('seconds')}s")
        return name, c

    try:
        run_jobs(args.jobs, traces, one)
    finally:
        cleanup_tmp(tmp_root, args.keep_temp)
    manifest_path = os.path.join(rdir, "manifest.json")
    manifest = read_json(manifest_path) if os.path.exists(manifest_path) else {"batteries": {}}
    manifest["batteries"][args.battery] = {"goport": args.goport, "goportSha": goport_sha, "oracleSha": oracle_sha,
                                           "traces": len(traces), "date": time.strftime("%Y-%m-%d %H:%M:%S"),
                                           "script": os.path.abspath(__file__),
                                           "scriptSha": sha256_file(os.path.abspath(__file__))}
    if args.wire:
        manifest["batteries"][args.battery]["wire"] = args.wire
    write_json(manifest_path, manifest)
    s = build_summary(args.out_root, args.label, None)
    print(summary_markdown(s))
    return EXIT_OK


# ---------------------------------------------------------------------------
# summary / show
# ---------------------------------------------------------------------------

COUNTED = ["same", "id_only", "diff", "goport_error", "oracle_error_same", "oracle_error_diff", "crash", "timeout",
           "not_run", "flaky_oracle", "oracle_crash"]


def load_label(out_root, label):
    rdir = os.path.join(out_root, "results", label, "traces")
    res = {}
    if not os.path.isdir(rdir):
        raise UsageError(f"no results for label {label}")
    for battery in sorted(os.listdir(rdir)):
        for f in sorted(os.listdir(os.path.join(rdir, battery))):
            if f.endswith(".json"):
                res[(battery, f[:-5])] = read_json(os.path.join(rdir, battery, f))
    return res


def build_summary(out_root, label, baseline):
    res = load_label(out_root, label)
    total = collections.Counter()
    per_battery = collections.defaultdict(collections.Counter)
    per_method = collections.defaultdict(collections.Counter)
    subs = collections.Counter()
    examples = {}
    first_id_only = collections.Counter()
    ms = collections.defaultdict(lambda: [0, 0, 0])  # method -> [oracle ms, goport ms, requests]
    skipped = 0
    for (battery, trace), r in res.items():
        seen_id_only = False
        # Symbols are named over the whole session, so a later goport error (a symbol never
        # answered in full) can also change earlier answers. Such id_only is not a root.
        other = any(e["class"] not in ("same", "skipped", "fs", "oracle_error_same", "id_only", "flaky_oracle")
                    for e in r["events"])
        for e in r["events"]:
            if e["class"] == "id_only" and not e.get("sub") and other:
                e = dict(e, sub="trace has another difference")
            cls = e["class"]
            if cls in ("skipped", "fs"):
                skipped += cls == "skipped"
                continue
            total[cls] += 1
            per_battery[battery][cls] += 1
            t = e.get("ms") or [None, None]
            if isinstance(t[0], int) and isinstance(t[1], int):
                ms[e["method"]][0] += t[0]
                ms[e["method"]][1] += t[1]
                ms[e["method"]][2] += 1
            per_method[e["method"]][cls] += 1
            if cls not in ("same", "oracle_error_same"):
                key = f"{cls} {e['method']}" + (f" | {e['sub']}" if e.get("sub") else "") + \
                      (f" @ {e['pointer']}" if e.get("pointer") is not None and cls != "id_only" else "")
                subs[key] += 1
                examples.setdefault(key, f"{battery}/{trace}#{e['event']}")
            if cls == "id_only" and not seen_id_only and not e.get("sub"):
                seen_id_only = True
                first_id_only[e["method"]] += 1
    s = {"format": SUMMARY_FORMAT, "label": label, "traces": len(res), "total": dict(total), "skipped": skipped,
         "batteries": {b: dict(c) for b, c in sorted(per_battery.items())},
         "methods": {m: dict(c) for m, c in sorted(per_method.items())},
         "groups": [{"key": k, "count": n, "example": examples[k]} for k, n in subs.most_common()],
         "firstIdOnlyMethod": dict(first_id_only),
         "ms": {m: {"oracle": v[0], "goport": v[1], "requests": v[2]} for m, v in sorted(ms.items())}}
    if baseline:
        base = load_label(out_root, baseline)
        cmp = collections.Counter()
        for key, r in res.items():
            b = {str(e["event"]): e["class"] for e in base.get(key, {}).get("events", [])}
            for e in r["events"]:
                old = b.get(str(e["event"]))
                if e["class"] in ("skipped", "fs"):
                    continue
                if old is None:
                    cmp["absent"] += 1
                elif old == "same" and e["class"] == "same":
                    cmp["retained"] += 1
                elif old != "same" and e["class"] == "same":
                    cmp["new_same"] += 1
                elif old == "same":
                    cmp["lost"] += 1
                elif e["class"] == "not_run":
                    cmp["unrun"] += 1
        s["baseline"] = {"label": baseline, **dict(cmp)}
    rdir = os.path.join(out_root, "results", label)
    write_json(os.path.join(rdir, "summary.json"), s)
    with open(os.path.join(rdir, "summary.md"), "w", encoding="utf-8") as f:
        f.write(summary_markdown(s))
    return s


def summary_markdown(s):
    lines = [f"# API oracle: {s['label']}", "", f"Traces: {s['traces']}. Skipped requests (golden skipped): "
             f"{s['skipped']}.", "", "| Class | Requests |", "|---|---:|"]
    for c in COUNTED:
        if s["total"].get(c):
            lines.append(f"| {c} | {s['total'][c]} |")
    lines += ["", "| Battery | " + " | ".join(COUNTED) + " |", "|---|" + "---:|" * len(COUNTED)]
    for b, c in s["batteries"].items():
        lines.append(f"| {b} | " + " | ".join(str(c.get(k, 0)) for k in COUNTED) + " |")
    lines += ["", "## Groups (not same)", "", "| Count | Class, method, detail | Example |", "|---:|---|---|"]
    for g in s["groups"][:80]:
        key = g["key"].replace("|", "\\|")
        if len(key) > 160:
            key = key[:157] + "..."
        lines.append(f"| {g['count']} | {key} | {g['example']} |")
    if s.get("firstIdOnlyMethod"):
        lines += ["", "First root id_only request (no other difference in the trace), by method: " +
                  ", ".join(f"{m} {n}" for m, n in sorted(s["firstIdOnlyMethod"].items(), key=lambda x: -x[1]))]
    slow = [(m, v) for m, v in (s.get("ms") or {}).items() if v["goport"] >= 200 and v["goport"] > 2 * max(1, v["oracle"])]
    if slow:
        lines += ["", "Request time, oracle and goport runs at different times (rough; not a perf measurement):", "",
                  "| Method | Requests | Oracle ms | Goport ms |", "|---|---:|---:|---:|"]
        for m, v in sorted(slow, key=lambda x: -x[1]["goport"])[:15]:
            lines.append(f"| {m} | {v['requests']} | {v['oracle']} | {v['goport']} |")
    if s.get("baseline"):
        b = s["baseline"]
        lines += ["", f"Against {b['label']}: " + ", ".join(f"{k} {v}" for k, v in b.items() if k != "label")]
    return "\n".join(lines) + "\n"


def cmd_summary(args):
    s = build_summary(args.out_root, args.label, args.baseline)
    print(summary_markdown(s))
    return EXIT_OK


def first_byte_diff(a_b64, b_b64):
    a, b = base64.b64decode(a_b64), base64.b64decode(b_b64)
    n = next((i for i in range(min(len(a), len(b))) if a[i] != b[i]), min(len(a), len(b)))
    info = f"first differing byte {n} (lengths {len(a)} and {len(b)})"
    if len(a) >= 44:
        nodes = struct.unpack_from("<I", a, 40)[0]
        if n >= nodes:
            idx, off = divmod(n - nodes, 28)
            field = ["kind", "pos", "end", "next", "parent", "data", "flags"][off // 4]
            info += f"; node {idx} field {field}: oracle {a[nodes + idx * 28:nodes + idx * 28 + 28].hex()} " \
                    f"goport {b[nodes + idx * 28:nodes + idx * 28 + 28].hex()}"
        else:
            info += "; before the nodes section"
    return info


def cmd_show(args):
    rdir = os.path.join(args.out_root, "results", args.label)
    manifest = read_json(os.path.join(rdir, "manifest.json"))
    oracle_sha = manifest["batteries"][args.battery]["oracleSha"]
    _, grecs = read_gz_jsonl(os.path.join(golden_dir(args.out_root, oracle_sha, args.battery),
                                          args.trace + ".golden.jsonl.gz"))
    _, rrecs = read_gz_jsonl(os.path.join(rdir, "responses", args.battery, args.trace + ".jsonl.gz"))
    key = int(args.event) if str(args.event).lstrip("-").isdigit() else args.event
    g = records_by_event(grecs).get(key) or special_records(grecs).get(key)
    r = records_by_event(rrecs).get(key) or special_records(rrecs).get(key)
    if g is None:
        raise UsageError(f"no event {args.event}")
    print(f"method: {g.get('method')}\noracle sent: {canon(g.get('sent'))[:400]}")
    print(f"goport sent: {canon((r or {}).get('sent'))[:400]}")
    ga = (g.get("response") or {}).get("result")
    ra = ((r or {}).get("response") or {}).get("result")
    if isinstance(ga, dict) and isinstance(ra, dict) and "data" in ga and "data" in ra and ga != ra:
        print(first_byte_diff(ga["data"], ra["data"]))
        return EXIT_OK
    def short(v):
        if isinstance(v, str) and len(v) > 300:
            return v[:300] + f"...({len(v)} chars)"
        if isinstance(v, list):
            return [short(e) for e in v]
        if isinstance(v, dict):
            return {k: short(e) for k, e in v.items()}
        return v

    a = json.dumps(short({k: g.get(k) for k in ("status", "response", "reason")}), indent=1, sort_keys=True,
                   ensure_ascii=False).splitlines()
    b = json.dumps(short({k: (r or {}).get(k) for k in ("status", "response", "reason", "stderr", "unported")}),
                   indent=1, sort_keys=True, ensure_ascii=False).splitlines()
    diff = list(difflib.unified_diff(a, b, "oracle", "goport", lineterm="", n=3))
    print("\n".join(diff[:args.max_lines]) if diff else "(equal)")
    return EXIT_OK


# ---------------------------------------------------------------------------
# build: sample nodes from Go's encoded AST and write traces
# ---------------------------------------------------------------------------


def go_kinds(repo=None):
    """Go ast.Kind values (ast/kind_generated.go, iota order) of the Go checkout repo (default GO_REPO)."""
    src = open(os.path.join(repo or GO_REPO, "internal/ast/kind_generated.go"), encoding="utf-8").read()
    m = re.search(r"const \(\n(.*?)\n\)", src, re.S)
    kinds, val = {}, -1
    for line in m.group(1).split("\n"):
        line = line.split("//")[0].strip()
        if not line:
            continue
        name = line.split()[0]
        if "=" in line:
            rhs = line.split("=", 1)[1].strip()
            if "iota" in rhs:
                val = 0
                kinds[name] = val
            else:
                kinds[name] = kinds.get(rhs.split()[0])
            continue
        val += 1
        kinds[name] = val
    return kinds


class EncodedFile:
    """Go api/encoder format: header offsets, string table, 28-byte node records."""

    def __init__(self, data: bytes):
        self.data = data
        (self.str_offsets, self.str_data, self.ext_data, self.struct_data,
         self.nodes_off) = struct.unpack_from("<5I", data, 24)
        self.count = (len(data) - self.nodes_off) // 28

    def node(self, i):
        return struct.unpack_from("<7I", self.data, self.nodes_off + i * 28)

    def string(self, idx):
        start, end = struct.unpack_from("<2I", self.data, self.str_offsets + idx * 4)
        return self.data[self.str_data + start:self.str_data + end].decode("utf-8", "replace")


def spread(items, cap):
    if cap is None or len(items) <= cap:
        return list(items)
    if cap <= 0:
        return []
    return [items[i * len(items) // cap] for i in range(cap)]


def by_kind(nodes, kinds, cap):
    """Up to `cap` nodes: one of each kind (rarest kinds first), then a second of each, ..."""
    groups = collections.defaultdict(list)
    for i in nodes:
        groups[kinds[i]].append(i)
    order = sorted(groups, key=lambda k: (len(groups[k]), k))
    out = []
    for rnd in range(max((len(g) for g in groups.values()), default=0)):
        for k in order:
            g = groups[k]
            if rnd < len(g) and len(out) < cap:
                out.append(g[(len(g) // 2 + rnd) % len(g)])
    return sorted(out)


def utf16_len(s):
    return len(s.encode("utf-16-le")) // 2


def glob_regex(pattern):
    """tsconfig-style glob: "**/" matches zero or more directories, "*" and "?" stay in one segment."""
    out, i = "", 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out += "(?:.*/)?"
            i += 3
        elif pattern.startswith("**", i):
            out += ".*"
            i += 2
        elif pattern[i] == "*":
            out += "[^/]*"
            i += 1
        elif pattern[i] == "?":
            out += "[^/]"
            i += 1
        else:
            out += re.escape(pattern[i])
            i += 1
    return re.compile(out + r"\Z")


def glob_files(pdir, include, exclude):
    inc = [glob_regex(p) for p in include]
    exc = [glob_regex(p) for p in exclude]
    out = []
    for root, dirs, files in os.walk(pdir):
        dirs[:] = sorted(d for d in dirs if d != "node_modules" and not d.startswith("."))
        for f in sorted(files):
            rel = os.path.relpath(os.path.join(root, f), pdir)
            if any(r.match(rel) for r in inc) and not any(r.match(rel) for r in exc):
                out.append(rel)
    return sorted(out)


# Methods that take their type or signature as "objectId" from protocol 3 on (tsgo#4689). The builders write
# "type" or "signature"; TraceBuilder.req moves the value to "objectId" in a protocol 3 run.
OBJECT_ID_METHODS = {"getConstraintOfTypeParameter", "getNonNullableType", "getApparentType", "getReturnTypeOfSignature"}
# Go: proto.go GetSymbolPropertyParams. Protocol 2 to 4: {snapshot, project, objectId}; protocol 5: {symbol}.
SYMBOL_PROPERTY_METHODS = {"getParentOfSymbol", "getMembersOfSymbol", "getExportsOfSymbol", "getExportSymbolOfSymbol"}


def symbol_refs(method, params, pf):
    """Protocol 5 (#64518): (params, paramsFrom) that send symbols as SymbolReference objects. A spec that takes
    a symbol (into ".../symbol", append "/symbols", or into "/objectId" of a symbol property method) reads the
    answer's /reference in place of its /id (pick: the "then" pointer). A symbol property request is {symbol}
    only. A literal symbol id (the error probes) becomes a snapshot reference with that id."""
    if method in SYMBOL_PROPERTY_METHODS and isinstance(params, dict):
        params = {k: v for k, v in params.items() if k not in ("snapshot", "project")}
    if isinstance(params, dict):
        params = dict(params)
        if isinstance(params.get("symbol"), int):
            params["symbol"] = snapshot_symbol(params["symbol"])
        if isinstance(params.get("actions"), list):
            params["actions"] = [{**a, "symbol": snapshot_symbol(a["symbol"])}
                                 if isinstance(a, dict) and isinstance(a.get("symbol"), int) else a
                                 for a in params["actions"]]
    specs = pf if isinstance(pf, list) else [] if pf is None else [pf]
    out = []
    for spec in specs:
        into = spec.get("into", "")
        if into.endswith("/symbol") or spec.get("append") == "/symbols" or \
                (into == "/objectId" and method in SYMBOL_PROPERTY_METHODS):
            spec = dict(spec, into="/symbol") if into == "/objectId" else dict(spec)
            if "pick" in spec:
                spec["pick"] = {**spec["pick"], "then": to_reference(spec["pick"]["then"])}
            else:
                spec["pointer"] = to_reference(spec["pointer"])
        out.append(spec)
    return params, out if isinstance(pf, list) else out[0] if out else None


def to_reference(pointer):
    if not pointer.endswith("/id"):
        raise ValueError(f"protocol 5: a symbol spec reads a symbol answer's /id, not {pointer!r}")
    return pointer[:-len("/id")] + "/reference"


def snapshot_symbol(symbol_id):
    """A snapshot-owned SymbolReference (Go: SymbolOwnerKindSnapshot) with a literal id, for the error probes."""
    return {"kind": 1, "snapshot": "@SNAPSHOT@", "project": "@PROJECT@", "id": symbol_id}


def literal_symbol(v):
    """The literal id of a snapshot_symbol() reference, else v."""
    return v["id"] if isinstance(v, dict) and v == snapshot_symbol(v.get("id")) and isinstance(v["id"], int) else v


def from_reference(pointer):
    if not pointer.endswith("/reference"):
        raise ValueError(f"wire 4: a symbol spec reads a symbol answer's /reference, not {pointer!r}")
    return pointer[:-len("/reference")] + "/id"


def wire4_kinds():
    """{SyntaxKind value at this pin: value at C_PIN} by kind name, for `check --wire 4` (#63915 adds
    KindSourceKeyword). The C_PIN values come from the goCheckout of its UPSTREAM.json record."""
    with open(REPO + "/UPSTREAM.json", encoding="utf-8") as f:
        old_repo = json.load(f)["pins"][C_PIN]["goCheckout"]
    new, old = go_kinds(), go_kinds(old_repo)
    names = {v: k for k, v in new.items() if not k.startswith(("KindFirst", "KindLast")) and v is not None}
    return {v: old[k] for v, k in names.items() if old.get(k) is not None}


_HANDLE = re.compile(r"^(\d+)\.(\d+)\.(/.*)$")


def old_kinds(v, kinds, method, key=None):
    """v with the SyntaxKind values of node handles "<index>.<kind>.<path>" (and the signatureToSignatureDeclaration
    "kind") mapped by kinds."""
    if isinstance(v, str):
        m = _HANDLE.match(v)
        return f"{m.group(1)}.{kinds[int(m.group(2))]}.{m.group(3)}" if m and int(m.group(2)) in kinds else v
    if isinstance(v, bool):
        return v
    if isinstance(v, int) and key == "kind" and method == "signatureToSignatureDeclaration":
        return kinds.get(v, v)
    if isinstance(v, list):
        return [old_kinds(e, kinds, method) for e in v]
    if isinstance(v, dict):
        return {k: old_kinds(e, kinds, method, k) for k, e in v.items()}
    return v


def wire4(method, params, pf, kinds=None):
    """(params, paramsFrom) of a protocol 5 request in its protocol 4 form: the inverse of symbol_refs. Used by
    `check --wire 4` (base bins that speak protocol 4). A symbol spec reads the answer's /id, a symbol property
    request is {snapshot, project, objectId} again, and a snapshot reference with a literal id is that id. With
    kinds (wire4_kinds()), the SyntaxKind values of the params are the protocol 4 pin's."""
    if kinds and params is not None:
        params = old_kinds(params, kinds, method)
    if isinstance(params, dict):
        params = dict(params)
        if "symbol" in params:
            params["symbol"] = literal_symbol(params["symbol"])
        if isinstance(params.get("actions"), list):
            params["actions"] = [{**a, "symbol": literal_symbol(a["symbol"])} if isinstance(a, dict) and "symbol" in a
                                 else a for a in params["actions"]]
        if method in SYMBOL_PROPERTY_METHODS:
            params = {"snapshot": "@SNAPSHOT@", "project": "@PROJECT@", **params}
    specs = pf if isinstance(pf, list) else [] if pf is None else [pf]
    out = []
    for spec in specs:
        into = spec.get("into", "")
        if into.endswith("/symbol") or spec.get("append") == "/symbols":
            prop = into == "/symbol" and method in SYMBOL_PROPERTY_METHODS
            spec = dict(spec, into="/objectId") if prop else dict(spec)
            if "pick" in spec:
                spec["pick"] = {**spec["pick"], "then": from_reference(spec["pick"]["then"])}
            else:
                spec["pointer"] = from_reference(spec["pointer"])
        out.append(spec)
    return params, out if isinstance(pf, list) else out[0] if out else None


def wire4_event(ev, kinds=None):
    """The event with wire4 applied; overlays come back unchanged."""
    if ev.get("kind") != "request":
        return ev
    params, pf = wire4(ev["method"], ev.get("params"), ev.get("paramsFrom"), kinds)
    out = dict(ev)
    for k, v in (("params", params), ("paramsFrom", pf)):
        if v is None:
            out.pop(k, None)
        else:
            out[k] = v
    return out


class TraceBuilder:
    """Collects events. Helpers add the request chains of api/proto.go."""

    def __init__(self):
        self.events = []
        self.snaps = 0  # snapshot events so far (snap)

    def req(self, method, params=None, pf=None):
        if PROTOCOL >= 3 and method in OBJECT_ID_METHODS and isinstance(pf, dict) \
                and pf.get("into") in ("/type", "/signature"):
            pf = {**pf, "into": "/objectId"}
        if PROTOCOL >= 5:
            params, pf = symbol_refs(method, params, pf)
        ev = {"kind": "request", "method": method}
        if params is not None:
            ev["params"] = params
        if pf is not None:
            ev["paramsFrom"] = pf
        self.events.append(ev)
        return len(self.events) - 1

    def snap(self, b, lsp=False):
        """One snapshot event from its protocol 3 params `b` ({}, {openProjects} or {fileChanges}). Protocol 4
        (study api-oracle-N.md section 2): the first stdio event is createSnapshot, a later one updateSnapshot
        {snapshot: @SNAPSHOT@, changes?}; an LSP session uses getCurrentLanguageServerSnapshot {baseSnapshot?
        (not on the first), changes?}. fileChanges become fileNotifications with ensurePrograms: true."""
        self.snaps += 1
        if PROTOCOL < 4:
            return self.req("updateSnapshot", b)
        changes = {k: v for k, v in b.items() if k != "fileChanges"}
        if "fileChanges" in b:
            changes.update(fileNotifications=b["fileChanges"], ensurePrograms=True)
        if lsp:
            base = {"baseSnapshot": "@SNAPSHOT@"} if self.snaps > 1 else {}
            return self.req("getCurrentLanguageServerSnapshot", {**base, **({"changes": changes} if changes else {})})
        if self.snaps == 1:
            return self.req("createSnapshot", changes)
        return self.req("updateSnapshot", {"snapshot": "@SNAPSHOT@", **({"changes": changes} if changes else {})})

    def temp(self, b, pf):
        """updateTemporarySnapshot {file, newText[, snapshot]} with paramsFrom pf. Protocol 4: updateSnapshot with
        a layer that holds the file, ensurePrograms: true and "track": false (the base comes from snapshot or pf)."""
        if PROTOCOL < 4:
            return self.req("updateTemporarySnapshot", b, pf)
        changes = {"fileSystem": {"kind": "layer", "files": {b["file"]: b["newText"]}}, "ensurePrograms": True}
        ev = self.req("updateSnapshot", {**({"snapshot": b["snapshot"]} if "snapshot" in b else {}),
                                         "changes": changes}, pf)
        self.events[ev]["track"] = False
        return ev

    @staticmethod
    def ck(**kw):
        return {"snapshot": "@SNAPSHOT@", "project": kw.pop("project", "@PROJECT@"), **kw}

    @staticmethod
    def diag(file):
        """Params of a per-file diagnostics request. Protocol 3 sends a file list (tsgo#4552)."""
        return TraceBuilder.ck(files=[file]) if PROTOCOL >= 3 else TraceBuilder.ck(file=file)

    @staticmethod
    def obj(project="@PROJECT@"):
        """Params of an objectId request (a type, symbol or signature property). Protocol 2 needs the project."""
        return {"snapshot": "@SNAPSHOT@", "project": project} if PROTOCOL2 else {"snapshot": "@SNAPSHOT@"}

    def type_chain(self, ev, ptr, loc=None, depth=0, project="@PROJECT@", other_type=None, string_type=None):
        """Requests about the type at `ptr` in the answer of event `ev`. The field getters
        (Go: proto.go:86-101) run at every depth, gated like the TS client (api.ts: only
        when the handle field is set). Depth 0 adds checker queries and one more level."""
        tid = lambda into, **kw: {"event": ev, "pointer": ptr + "/id", "into": into, **kw}  # noqa: E731
        ck = lambda **kw: self.ck(project=project, **kw)  # noqa: E731
        self.req("typeToString", ck(), tid("/type"))
        oid = lambda field: {"event": ev, "pointer": ptr + "/id", "into": "/objectId",  # noqa: E731
                             "when": [{"pointer": ptr + "/" + field, "nonzero": True}]}
        getters = [("symbol", "getSymbolOfType"), ("aliasSymbol", "getAliasSymbolOfType"),
                   ("aliasTypeArguments", "getAliasTypeArgumentsOfType"), ("target", "getTargetOfType"),
                   ("freshType", "getFreshTypeOfType"), ("regularType", "getRegularTypeOfType"),
                   ("typeParameters", "getTypeParametersOfType"),
                   ("outerTypeParameters", "getOuterTypeParametersOfType"),
                   ("localTypeParameters", "getLocalTypeParametersOfType"), ("objectType", "getObjectTypeOfType"),
                   ("indexType", "getIndexTypeOfType"), ("checkType", "getCheckTypeOfType"),
                   ("extendsType", "getExtendsTypeOfType"), ("baseType", "getBaseTypeOfType"),
                   ("substConstraint", "getConstraintOfType")]
        for field, m in getters:
            g = self.req(m, self.obj(project), oid(field))
            if m not in ("getSymbolOfType", "getAliasSymbolOfType") and "TypeArguments" not in m \
                    and "TypeParameters" not in m:
                self.req("typeToString", ck(), {"event": g, "pointer": "/id", "into": "/type"})
        types = self.req("getTypesOfType", self.obj(project),
                         {"event": ev, "pointer": ptr + "/id", "into": "/objectId",
                          "when": [{"pointer": ptr + "/flags", "mask": TF_UNION | TF_INTERSECTION | TF_TEMPLATE_LITERAL}]})
        self.req("typeToString", ck(), {"event": types, "pointer": "/0/id", "into": "/type"})
        self.req("getConstraintOfTypeParameter", ck(),
                 tid("/type", when=[{"pointer": ptr + "/flags", "mask": TF_TYPE_PARAMETER}]))
        if depth > 0:
            return
        extra = {"location": loc} if loc else {}
        self.req("typeToString", ck(flags=TYPE_FORMAT_FLAGS, **extra), tid("/type"))
        tn = self.req("typeToTypeNode", ck(**extra), tid("/type"))
        self.req("printNode", {}, {"event": tn, "pointer": "/data", "into": "/data"})
        props = self.req("getPropertiesOfType", ck(), tid("/type"))
        for i in range(2):
            t = self.req("getTypeOfSymbol", ck(), {"event": props, "pointer": f"/{i}/id", "into": "/symbol"})
            self.req("typeToString", ck(), {"event": t, "pointer": "/id", "into": "/type"})
            if loc:
                self.req("getTypeOfSymbolAtLocation", ck(location=loc),
                         {"event": props, "pointer": f"/{i}/id", "into": "/symbol"})
        call = self.req("getSignaturesOfType", ck(kind=SIG_CALL), tid("/type"))
        self.sig_chain(call, "/0", project=project)
        construct = self.req("getSignaturesOfType", ck(kind=SIG_CONSTRUCT), tid("/type"))
        self.sig_chain(construct, "/0", project=project, short=True)
        ii = self.req("getIndexInfosOfType", ck(), tid("/type"))
        self.req("typeToString", ck(), {"event": ii, "pointer": "/0/keyType/id", "into": "/type"})
        self.req("typeToString", ck(), {"event": ii, "pointer": "/0/valueType/id", "into": "/type"})
        for m in ("getBaseTypeOfLiteralType", "getNonNullableType", "getWidenedType"):
            t = self.req(m, ck(), tid("/type"))
            self.req("typeToString", ck(), {"event": t, "pointer": "/id", "into": "/type"})
        self.req("isArrayLikeType", ck(), tid("/type"))
        bt = self.req("getBaseTypes", ck(), tid("/type", when=[{"pointer": ptr + "/objectFlags", "mask": OF_CLASS_OR_INTERFACE}]))
        self.type_chain(bt, "/0", depth=1, project=project)
        ta = self.req("getTypeArguments", ck(), tid("/type", when=[{"pointer": ptr + "/objectFlags", "mask": OF_REFERENCE}]))
        self.type_chain(ta, "/0", depth=1, project=project)
        self.type_chain(types, "/0", depth=1, project=project)
        for target in (string_type, other_type):
            if target is not None:
                self.req("isTypeAssignableTo", ck(),
                         [tid("/source"), {"event": target[0], "pointer": target[1] + "/id", "into": "/target"}])

    def sig_chain(self, ev, ptr, project="@PROJECT@", short=False):
        ck = lambda **kw: self.ck(project=project, **kw)  # noqa: E731
        gid = lambda into, **kw: {"event": ev, "pointer": ptr + "/id", "into": into, **kw}  # noqa: E731
        rt = self.req("getReturnTypeOfSignature", ck(), gid("/signature"))
        self.req("typeToString", ck(), {"event": rt, "pointer": "/id", "into": "/type"})
        if short:
            self.req("getParametersOfSignature", self.obj(project), gid("/objectId"))
            return
        self.req("getRestTypeOfSignature", ck(), gid("/signature"))
        self.req("getTypePredicateOfSignature", ck(), gid("/signature"))
        self.req("getParametersOfSignature", self.obj(project), gid("/objectId"))
        for field, m in (("typeParameters", "getTypeParametersOfSignature"),
                         ("thisParameter", "getThisParameterOfSignature"), ("target", "getTargetOfSignature")):
            self.req(m, self.obj(project), gid("/objectId", when=[{"pointer": ptr + "/" + field, "nonzero": True}]))
        decl = self.req("signatureToSignatureDeclaration", ck(kind=GO_KIND_CALL_SIGNATURE), gid("/signature"))
        self.req("printNode", {}, {"event": decl, "pointer": "/data", "into": "/data"})
        self.req("getParameterType", ck(index=0),
                 gid("/signature", when=[{"pointer": ptr + "/parameters", "nonzero": True}]))

    def symbol_chain(self, ev, ptr, loc, file, pos, with_usages):
        ck = self.ck
        sid = lambda into, **kw: {"event": ev, "pointer": ptr + "/id", "into": into, **kw}  # noqa: E731
        for m in ("getTypeOfSymbol", "getDeclaredTypeOfSymbol"):
            t = self.req(m, ck(), sid("/symbol"))
            self.req("typeToString", ck(), {"event": t, "pointer": "/id", "into": "/type"})
        for field, m in (("parent", "getParentOfSymbol"), ("exportSymbol", "getExportSymbolOfSymbol")):
            self.req(m, self.obj(), sid("/objectId", when=[{"pointer": ptr + "/" + field, "nonzero": True}]))
        self.req("getMembersOfSymbol", self.obj(), sid("/objectId"))
        self.req("getExportsOfSymbol", self.obj(), sid("/objectId"))
        name = {"event": ev, "pointer": ptr + "/name", "into": "/name"}
        if loc:
            self.req("resolveName", ck(location=loc, meaning=SF_VALUE | SF_TYPE | SF_NAMESPACE), name)
        rn = self.req("resolveName", ck(file=file, position=pos, meaning=SF_VALUE, excludeGlobals=True), name)
        # A local symbol of an exported declaration has exportSymbol set.
        for field, m in (("exportSymbol", "getExportSymbolOfSymbol"), ("parent", "getParentOfSymbol")):
            self.req(m, self.obj(), {"event": rn, "pointer": "/id", "into": "/objectId",
                                     "when": [{"pointer": "/" + field, "nonzero": True}]})
        self.req("getReferencesToSymbolInFile", ck(file=file), sid("/symbol"))
        if loc:
            self.req("getTypeOfSymbolAtLocation", ck(location=loc), sid("/symbol"))
        if with_usages:
            self.req("getSignatureUsages", ck(),
                     {"event": ev, "pointer": ptr + "/valueDeclaration", "into": "/signatureDecl",
                      "when": [{"pointer": ptr + "/flags", "mask": SF_FUNCTION | SF_METHOD}]})


GO_KIND_CALL_SIGNATURE = 180  # checked against kind_generated.go in `build`


def oracle_session(oracle, pdir, tsconfig, files, tmp_root, project_out=None):
    """Go answers used to choose samples: project roots and encoded ASTs. project_out (a dict)
    gets the ProjectResponse of the project."""
    run_dir = tempfile.mkdtemp(prefix="build-", dir=tmp_root)
    header = {"name": "build", "project": {"dir": pdir, "tsconfig": tsconfig}}
    srv = Server(oracle, header, run_dir, None, "build")
    srv.start()
    try:
        st, _ = srv.api.request(1, "initialize", None, START_TIMEOUT)
        method = "createSnapshot" if PROTOCOL >= 4 else "updateSnapshot"
        st, snap = srv.api.request(2, method, open_params(os.path.join(pdir, tsconfig)), START_TIMEOUT * 4)
        if st != "ok":
            raise HarnessError(f"{method}: {snap}")
        want = os.path.join(pdir, tsconfig)
        proj = next((p for p in snap["projects"] if p.get("configFileName") == want), snap["projects"][0])
        if project_out is not None:
            project_out.update(proj)
        roots = set(proj["rootFiles"])
        encoded = {}
        for n, rel in enumerate(files):
            st, ans = srv.api.request(3 + n, "getSourceFile", {"snapshot": snap["snapshot"], "project": proj["id"],
                                                               "file": os.path.join(pdir, rel)}, START_TIMEOUT)
            if st == "ok" and ans:
                encoded[rel] = base64.b64decode(ans["data"])
        return roots, encoded
    finally:
        srv.stop()
        shutil.rmtree(run_dir, ignore_errors=True)


class FileSamples:
    def __init__(self, enc: EncodedFile, rel, kinds, caps):
        K = kinds
        ids, members, calls, type_nodes, fn_exprs, shorthand, sig_decls = [], [], [], [], [], [], []
        type_kinds = {K[k] for k in ("KindTypeReference", "KindUnionType", "KindIntersectionType", "KindFunctionType",
                                     "KindTypeLiteral", "KindArrayType", "KindTupleType", "KindIndexedAccessType",
                                     "KindMappedType", "KindConditionalType", "KindTypeQuery", "KindTypeOperator",
                                     "KindLiteralType", "KindTemplateLiteralType")}
        records = [enc.node(i) for i in range(enc.count)]
        for i, (kind, pos, end, _next, parent, data, _flags) in enumerate(records):
            if i == 0 or kind == 0xFFFFFFFF:
                continue
            pkind = records[parent][0] if parent else None
            if pkind == 0xFFFFFFFF:
                gp = records[parent][4]
                pkind_real = records[gp][0] if gp else None
            else:
                pkind_real = pkind
            if kind == K["KindIdentifier"] and (data >> 30) == 1:
                text = enc.string(data & 0x00FFFFFF)
                start = end - utf16_len(text)
                if start < 0 or pkind_real in (K.get("KindJSDoc"), K.get("KindJSDocTypeExpression")):
                    continue
                ids.append((i, start))
                if pkind == K["KindPropertyAccessExpression"] and i != parent + 1:
                    # the name of a PropertyAccessExpression: not its first child (the expression)
                    members.append((i, start))
            elif kind in (K["KindCallExpression"], K["KindNewExpression"]):
                calls.append(i)
            elif kind in type_kinds:
                type_nodes.append(i)
            elif kind in (K["KindArrowFunction"], K["KindFunctionExpression"], K["KindObjectLiteralExpression"]):
                fn_exprs.append(i)
            elif kind == K["KindShorthandPropertyAssignment"]:
                shorthand.append(i)
            elif kind in (K["KindFunctionDeclaration"], K["KindMethodDeclaration"]):
                sig_decls.append(i)
        self.kinds = {i: rec[0] for i, rec in enumerate(records)}
        self.ids = spread(ids, caps["ids"])
        self.members = spread([m for m in members if m not in self.ids], caps["memberCompletions"])
        self.calls = spread(calls, caps["calls"])
        self.type_nodes = by_kind(type_nodes, self.kinds, caps["typeNodes"])
        self.fn_exprs = spread(fn_exprs, caps["fnExprs"])
        self.shorthand = spread(shorthand, caps["shorthand"])
        self.sig_decls = spread(sig_decls, caps["sigDecls"])
        self.global_positions = spread([p for _, p in ids], caps["globalCompletions"])

    def handle(self, i, path):
        return f"{i}.{self.kinds[i]}.{path}"


def trace_header(name, battery, preset, **extra):
    h = {"format": TRACE_FORMAT, "name": name, "battery": battery,
         "project": {"dir": preset["dir"], "tsconfig": preset["tsconfig"]}, "protocol": "jsonrpc",
         "callbacks": [], "callbackMode": "fallback", "transport": "stdio"}
    h.update(extra)
    return h


def open_params(config):
    """Protocol 3 updateSnapshot params (protocol 4: createSnapshot params) that open one project."""
    return {"openProjects": [config]} if PROTOCOL2 else {"openProject": config}


def open_session(tb, preset, extra_projects=()):
    tb.req("initialize")
    snap = tb.snap(open_params("@PROJECT_DIR@/" + preset["tsconfig"]))
    for rel in extra_projects:
        snap = tb.snap(open_params("@PROJECT_DIR@/" + rel))
    return snap


def file_trace(preset, rel, s: FileSamples, caps, apath):
    """Whole-file requests, then chains for each sample."""
    tb = TraceBuilder()
    open_session(tb, preset)
    file = "@PROJECT_DIR@/" + rel
    ck = tb.ck
    tb.req("getDefaultProjectForFile", {"snapshot": "@SNAPSHOT@", "file": file})
    tb.req("getSourceFile", ck(file=file))
    for m in ("getSyntacticDiagnostics", "getSemanticDiagnostics", "getSuggestionDiagnostics",
              "getDeclarationDiagnostics"):
        tb.req(m, tb.diag(file))
    string_type = (tb.req("getStringType", ck()), "")
    h = lambda i: s.handle(i, apath)  # noqa: E731
    prev_type = None
    sym_events = []
    for n, (i, pos) in enumerate(s.ids):
        a = tb.req("getSymbolAtPosition", ck(file=file, position=pos))
        b = tb.req("getTypeAtPosition", ck(file=file, position=pos))
        tb.req("getSymbolAtLocation", ck(location=h(i)))
        d = tb.req("getTypeAtLocation", ck(location=h(i)))
        tb.req("typeToString", ck(), {"event": d, "pointer": "/id", "into": "/type"})
        tb.req("getContextualType", ck(location=h(i)))
        tb.symbol_chain(a, "", h(i), file, pos, with_usages=n < caps["sigUsages"])
        tb.type_chain(b, "", loc=h(i), other_type=prev_type, string_type=string_type)
        if n < caps["refsForNode"]:
            tb.req("getReferencedSymbolsForNode", ck(node=h(i), position=pos))
        prev_type = (b, "")
        sym_events.append(a)
    if s.ids:
        tb.req("getSymbolsAtPositions", ck(file=file, positions=[p for _, p in s.ids]))
        tb.req("getTypesAtPositions", ck(file=file, positions=[p for _, p in s.ids]))
        tb.req("getSymbolsAtLocations", ck(locations=[h(i) for i, _ in s.ids]))
        tb.req("getTypeAtLocations", ck(locations=[h(i) for i, _ in s.ids]))
        tb.req("getTypesOfSymbols", ck(symbols=[]),
               [{"event": a, "pointer": "/id", "append": "/symbols", "optional": True} for a in sym_events])
    for i in s.calls:
        g = tb.req("getResolvedSignature", ck(location=h(i)))
        tb.sig_chain(g, "")
    for i in s.type_nodes:
        t = tb.req("getTypeFromTypeNode", ck(location=h(i)))
        tb.type_chain(t, "", depth=1)
        tn = tb.req("typeToTypeNode", ck(location=h(i)), {"event": t, "pointer": "/id", "into": "/type"})
        tb.req("printNode", {}, {"event": tn, "pointer": "/data", "into": "/data"})
    for i in s.fn_exprs:
        c = tb.req("getContextualType", ck(location=h(i)))
        tb.type_chain(c, "", depth=1)
        tb.req("isContextSensitive", ck(location=h(i)))
        t = tb.req("getTypeAtLocation", ck(location=h(i)))
        tb.type_chain(t, "", depth=1)
    for i in s.shorthand:
        tb.req("getShorthandAssignmentValueSymbol", ck(location=h(i)))
    for i in s.sig_decls:
        u = tb.req("getSignatureUsages", ck(signatureDecl=h(i)))
        g = tb.req("getResolvedSignature", ck(), {"event": u, "pointer": "/0/call", "into": "/location"})
        tb.sig_chain(g, "")
    for i, pos in s.members:
        tb.req("getCompletionsAtPosition", ck(file=file, position=pos, triggerCharacter=".", includeSymbol=True))
    for pos in s.global_positions:
        tb.req("getCompletionsAtPosition", ck(file=file, position=pos))
    return tb.events


def misc_trace(preset, files):
    """Once per project: intrinsics, config, echo, snapshots, errors, whole-program diagnostics."""
    tb = TraceBuilder()
    snap = open_session(tb, preset)
    ck = tb.ck
    first = "@PROJECT_DIR@/" + files[0]
    tb.req("getDefaultProjectForFile", {"snapshot": "@SNAPSHOT@", "file": first})
    tb.req("getDefaultProjectForFile", {"snapshot": "@SNAPSHOT@", "file": {"uri": "file://" + preset["dir"] + "/" + files[0]}})
    tb.req("parseConfigFile", {"file": "@PROJECT_DIR@/" + preset["tsconfig"]})
    tb.req("parseConfigFile", {"file": preset["tsconfig"]})
    tb.req("getConfigFileParsingDiagnostics", ck())
    for m in INTRINSICS:
        t = tb.req(m, ck())
        tb.req("typeToString", ck(), {"event": t, "pointer": "/id", "into": "/type"})
        tb.req("getPropertiesOfType", ck(), {"event": t, "pointer": "/id", "into": "/type"})
    for payload in ({"a": [1, "x", None, True]}, "text é中\U0001F600", 12.5, [], None):
        tb.req("echo", payload)
    tb.req("ping")
    for name, meaning in (("Promise", SF_TYPE), ("Array", SF_VALUE), ("console", SF_VALUE), ("Map", SF_TYPE)):
        r = tb.req("resolveName", ck(name=name, meaning=meaning))
        tb.symbol_chain(r, "", None, first, 0, with_usages=False)
        t = tb.req("getDeclaredTypeOfSymbol", ck(), {"event": r, "pointer": "/id", "into": "/symbol"})
        tb.type_chain(t, "", depth=0)
    tb.req("getSourceFile", ck(file="@PROJECT_DIR@/does/not/exist.ts"))
    tb.req("getSourceFile", ck(file="bundled:///libs/lib.es5.d.ts"))
    for m in ("getSyntacticDiagnostics", "getSemanticDiagnostics", "getSuggestionDiagnostics",
              "getDeclarationDiagnostics"):
        tb.req(m, ck())
    tb.req("getCompletionsAtPosition", ck(file=first, position=0))
    # errors
    tb.req("getSymbolAtPosition", {"snapshot": 999, "project": "@PROJECT@", "file": first, "position": 0})
    tb.req("getSymbolAtPosition", ck(project="/no/such/tsconfig.json", file=first, position=0))
    tb.req("getSymbolAtPosition", ck(file="@PROJECT_DIR@/does/not/exist.ts", position=0))
    tb.req("getTypeOfSymbol", ck(symbol=0))
    tb.req("getTypeOfSymbol", ck(symbol=987654321))
    tb.req("typeToString", ck(type=4000000000))
    tb.req("getSignaturesOfType", ck(type=0, kind=0))
    tb.req("getSymbolAtLocation", ck(location="bad-handle"))
    tb.req("getSymbolAtLocation", ck(location="999999.79." + preset["dir"] + "/" + files[0]))
    tb.req("getSymbolAtPosition", ck(file=first, position="x"))
    tb.req("noSuchMethod", {"x": 1})
    tb.req("printNode", {"data": "not base64!"})
    tb.req("release", {"snapshot": 0})
    # snapshots
    s2 = tb.snap({})
    s3 = tb.snap({"fileChanges": {"changed": [first]}})
    s4 = tb.snap({"fileChanges": {"invalidateAll": True}})
    tb.req("getSymbolAtPosition", ck(file=first, position=0))
    s5 = tb.snap(open_params("@PROJECT_DIR@/" + preset["tsconfig"]))
    for ev in (snap, s2, s3, s4, s5):
        tb.req("release", {}, {"event": ev, "pointer": "/snapshot", "into": "/snapshot"})
    tb.req("release", {}, {"event": snap, "pointer": "/snapshot", "into": "/snapshot"})
    tb.req("getSymbolAtPosition", {"project": "@PROJECT@", "file": first, "position": 0},
           {"event": snap, "pointer": "/snapshot", "into": "/snapshot"})
    return tb.events


def write_trace(out_dir, header, events):
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, header["name"] + ".jsonl")
    with open(path, "w", encoding="utf-8") as f:
        f.write(json.dumps(header, ensure_ascii=False) + "\n")
        for ev in events:
            f.write(json.dumps(ev, ensure_ascii=False) + "\n")
    return path


def slug(rel):
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", rel)


def cmd_build(args):
    global GO_KIND_CALL_SIGNATURE
    if args.preset not in PRESETS:
        raise UsageError(f"unknown preset {args.preset}")
    preset = dict(PRESETS[args.preset])
    if args.kind == "ext" and (PROTOCOL < 3 or args.preset not in EXT_PRESETS):
        raise UsageError(f"--kind ext needs protocol 3 (pin {PIN} has {PROTOCOL}) and a preset in "
                         f"{', '.join(EXT_PRESETS)}")
    kinds = go_kinds()
    GO_KIND_CALL_SIGNATURE = kinds["KindCallSignature"]
    caps = dict(preset["caps"])
    for kv in args.cap or []:
        k, _, v = kv.partition("=")
        caps[k] = int(v)
    files = glob_files(preset["dir"], preset["include"], preset["exclude"])
    tmp_root = make_tmp_root()
    project = {}
    try:
        roots, encoded = oracle_session(args.oracle, preset["dir"], preset["tsconfig"], files, tmp_root, project)
    finally:
        shutil.rmtree(tmp_root, ignore_errors=True)
    files = [f for f in files if os.path.join(preset["dir"], f) in roots and f in encoded]
    if args.limit:
        files = files[:args.limit]
    if args.sample:
        files = spread(files, args.sample)
    out_dir = os.path.join(args.out_root, "traces", args.battery)
    if os.path.isdir(out_dir) and os.listdir(out_dir) and not args.force:
        raise UsageError(f"{out_dir} exists; use --force to replace its traces")
    os.makedirs(out_dir, exist_ok=True)
    for f in os.listdir(out_dir):
        if f.endswith(".jsonl"):
            os.remove(os.path.join(out_dir, f))
    written = []
    samples = {rel: FileSamples(EncodedFile(encoded[rel]), rel, kinds, caps) for rel in files}
    apath = lambda rel: os.path.join(preset["dir"], rel)  # noqa: E731
    kind = args.kind
    if kind in ("files", "proto", "callbacks"):
        chosen = files if kind == "files" else files[:2]
        extra = {}
        if kind == "proto":
            extra = {"protocol": "msgpack"}
        if kind == "callbacks":
            extra = {"callbacks": list(CALLBACK_NAMES), "callbackMode": "real"}
        for rel in chosen:
            ev = file_trace(preset, rel, samples[rel], caps, apath(rel))
            written.append(write_trace(out_dir, trace_header(slug(rel), args.battery, preset, file=rel, **extra), ev))
        ev = misc_trace(preset, files)
        written.append(write_trace(out_dir, trace_header("_misc", args.battery, preset, **extra), ev))
        if kind == "callbacks":
            written.append(write_trace(out_dir, *changes_trace(preset, args.battery, files)))
    elif kind == "lsp":
        written.append(write_trace(out_dir, *lsp_trace(preset, args.battery, files, samples)))
    elif kind == "xchecker":
        for header, ev in xchecker_traces(preset, args.battery, files, samples, apath):
            written.append(write_trace(out_dir, header, ev))
    elif kind == "ext":
        ext = ExtPlan(args.preset, preset, files, encoded, kinds, caps, project, samples)
        for header, ev in ext_traces(ext, args.battery):
            check_emit_guard(header, ev)
            written.append(write_trace(out_dir, header, ev))
    else:
        raise UsageError(f"unknown kind {kind}")
    index = {"preset": args.preset, "kind": kind, "caps": caps, "files": files, "traces": len(written),
             "oracle": args.oracle, "oracleSha": sha256_file(args.oracle),
             "requests": sum(sum(1 for _ in open(p)) - 1 for p in written)}
    if kind == "ext":
        index["ext"] = ext.summary()
    write_json(os.path.join(out_dir, "index.json"), index)
    print(json.dumps({k: v for k, v in index.items() if k != "files"}, indent=1))
    return EXIT_OK


def changes_trace(preset, battery, files):
    """Callback overlay: change, create and delete files, then updateSnapshot fileChanges."""
    tb = TraceBuilder()
    rel = files[0]
    path = "@PROJECT_DIR@/" + rel
    new_rel = os.path.join(os.path.dirname(rel), "__api_oracle_new.ts")
    new_path = "@PROJECT_DIR@/" + new_rel
    with open(os.path.join(preset["dir"], rel), encoding="utf-8") as f:
        text = f.read()
    open_session(tb, preset)
    ck = tb.ck
    pos = utf16_len(text) + 14

    def probe(file, position):
        tb.req("getSourceFile", ck(file=file))
        tb.req("getSemanticDiagnostics", tb.diag(file))
        t = tb.req("getTypeAtPosition", ck(file=file, position=position))
        tb.req("typeToString", ck(), {"event": t, "pointer": "/id", "into": "/type"})

    probe(path, pos)
    tb.events.append({"kind": "overlay", "path": path,
                      "content": text + "\nexport const __apiOracleProbe: number = \"x\" as any as 5;\n"})
    tb.snap({"fileChanges": {"changed": [path]}})
    probe(path, pos)
    tb.events.append({"kind": "overlay", "path": new_path,
                      "content": "export const created = [1, 2] as const;\nexport type C = typeof created;\n"})
    tb.snap({"fileChanges": {"created": [new_path]}})
    tb.req("getDefaultProjectForFile", {"snapshot": "@SNAPSHOT@", "file": new_path})
    probe(new_path, 14)
    tb.events.append({"kind": "overlay", "path": new_path, "content": None})
    tb.snap({"fileChanges": {"deleted": [new_path]}})
    tb.req("getSourceFile", ck(file=new_path))
    tb.events.append({"kind": "overlay", "path": path, "content": {"drop": True}})
    tb.snap({"fileChanges": {"changed": [path]}})
    probe(path, pos)
    tb.snap({"fileChanges": {"invalidateAll": True}})
    probe(path, 0)
    header = trace_header("_changes", battery, preset, callbacks=["readFile", "fileExists", "directoryExists",
                                                                   "getAccessibleEntries"], callbackMode="fallback")
    return header, tb.events


def lsp_trace(preset, battery, files, samples):
    rel = files[0]
    s = samples[rel]
    tb = TraceBuilder()
    tb.req("initialize")
    tb.snap({}, lsp=True)  # adopt the LSP state (Go answers a nil-pointer panic here at B)
    tb.snap(open_params("@PROJECT_DIR@/" + preset["tsconfig"]), lsp=True)
    file = "@PROJECT_DIR@/" + rel
    ck = tb.ck
    tb.req("getDefaultProjectForFile", {"snapshot": "@SNAPSHOT@", "file": file})
    tb.req("getSourceFile", ck(file=file))
    tb.req("getSemanticDiagnostics", tb.diag(file))
    for i, pos in s.ids[:4]:
        a = tb.req("getSymbolAtPosition", ck(file=file, position=pos))
        tb.req("getTypeOfSymbol", ck(), {"event": a, "pointer": "/id", "into": "/symbol"})
        b = tb.req("getTypeAtPosition", ck(file=file, position=pos))
        tb.type_chain(b, "", depth=1)
    tb.snap({}, lsp=True)
    tb.req("getSymbolAtPosition", ck(file=file, position=s.ids[0][1] if s.ids else 0))
    tb.req("release", {"snapshot": "@SNAPSHOT@"})
    header = trace_header("_lsp", battery, preset, transport="lsp", lspOpen=[rel])
    return header, tb.events


def xchecker_traces(preset, battery, files, samples, apath):
    """Handles of one checker passed to requests of another (Rust: api/session_p1.rs:379-421)."""
    out = []
    # 1. Two projects over the same files: types, signatures and transient symbols of project 1
    #    sent with project 2.
    if preset.get("tsconfig2"):
        p2 = "@PROJECT:" + preset["tsconfig2"] + "@"
        tb = TraceBuilder()
        open_session(tb, preset, extra_projects=[preset["tsconfig2"]])
        str2 = tb.req("getStringType", tb.ck(project=p2))
        for rel in files[:3]:
            file = "@PROJECT_DIR@/" + rel
            s = samples[rel]
            for i, pos in s.ids[:4]:
                t = tb.req("getTypeAtPosition", tb.ck(file=file, position=pos))
                tb.req("typeToString", tb.ck(project=p2), {"event": t, "pointer": "/id", "into": "/type"})
                tb.req("getPropertiesOfType", tb.ck(project=p2), {"event": t, "pointer": "/id", "into": "/type"})
                tb.req("isTypeAssignableTo", tb.ck(project=p2),
                       [{"event": t, "pointer": "/id", "into": "/source"},
                        {"event": str2, "pointer": "/id", "into": "/target"}])
                sg = tb.req("getSignaturesOfType", tb.ck(kind=0), {"event": t, "pointer": "/id", "into": "/type"})
                rt = tb.req("getReturnTypeOfSignature", tb.ck(project=p2),
                            {"event": sg, "pointer": "/0/id", "into": "/signature"})
                tb.req("typeToString", tb.ck(project=p2), {"event": rt, "pointer": "/id", "into": "/type"})
                props = tb.req("getPropertiesOfType", tb.ck(), {"event": t, "pointer": "/id", "into": "/type"})
                pt = tb.req("getTypeOfSymbol", tb.ck(project=p2),
                            {"event": props, "pointer": "", "into": "/symbol",
                             "pick": {"where": {"field": "flags", "mask": SF_TRANSIENT}, "index": 0, "then": "/id"}})
                tb.req("typeToString", tb.ck(project=p2), {"event": pt, "pointer": "/id", "into": "/type"})
        out.append((trace_header("x_two_projects", battery, preset), tb.events))
    # 2. Symbols from language service checkers (completions, find references) used with the API checker.
    tb = TraceBuilder()
    open_session(tb, preset)
    for rel in files[:4]:
        file = "@PROJECT_DIR@/" + rel
        s = samples[rel]
        for i, pos in (s.members or s.ids)[:3]:
            c = tb.req("getCompletionsAtPosition", tb.ck(file=file, position=pos, triggerCharacter=".", includeSymbol=True))
            for mask in (SF_TRANSIENT, SF_VALUE):
                sym = {"event": c, "pointer": "/entries", "into": "/symbol",
                       "pick": {"where": {"field": "symbol/flags", "mask": mask}, "sortBy": ["name"], "index": 0,
                                "then": "/symbol/id"}}
                t = tb.req("getTypeOfSymbol", tb.ck(), sym)
                tb.req("typeToString", tb.ck(), {"event": t, "pointer": "/id", "into": "/type"})
                tb.req("getTypeOfSymbolAtLocation", tb.ck(location=s.handle(i, apath(rel))), sym)
        for i, pos in s.ids[:2]:
            r = tb.req("getReferencedSymbolsForNode", tb.ck(node=s.handle(i, apath(rel)), position=pos))
            t = tb.req("getTypeOfSymbol", tb.ck(), {"event": r, "pointer": "/0/symbol/id", "into": "/symbol"})
            tb.req("typeToString", tb.ck(), {"event": t, "pointer": "/id", "into": "/type"})
    out.append((trace_header("x_ls_symbols", battery, preset), tb.events))
    # 3. File A handles in requests about file B (one API checker per project in Go and Rust).
    if len(files) >= 2:
        tb = TraceBuilder()
        open_session(tb, preset)
        a_rel, b_rel = files[0], files[len(files) // 2]
        sa, sb = samples[a_rel], samples[b_rel]
        for (ia, pa), (ib, pb) in zip(sa.ids[:4], sb.ids[:4]):
            ta = tb.req("getTypeAtPosition", tb.ck(file="@PROJECT_DIR@/" + a_rel, position=pa))
            tb2 = tb.req("getTypeAtPosition", tb.ck(file="@PROJECT_DIR@/" + b_rel, position=pb))
            tb.req("typeToString", tb.ck(location=sb.handle(ib, apath(b_rel))),
                   {"event": ta, "pointer": "/id", "into": "/type"})
            tb.req("isTypeAssignableTo", tb.ck(), [{"event": ta, "pointer": "/id", "into": "/source"},
                                                   {"event": tb2, "pointer": "/id", "into": "/target"}])
            sa_ = tb.req("getSymbolAtPosition", tb.ck(file="@PROJECT_DIR@/" + a_rel, position=pa))
            tb.req("getTypeOfSymbolAtLocation", tb.ck(location=sb.handle(ib, apath(b_rel))),
                   {"event": sa_, "pointer": "/id", "into": "/symbol"})
            tb.req("getReferencesToSymbolInFile", tb.ck(file="@PROJECT_DIR@/" + b_rel),
                   {"event": sa_, "pointer": "/id", "into": "/symbol"})
        out.append((trace_header("x_file_a_b", battery, preset), tb.events))
    return out


# ---------------------------------------------------------------------------
# build --kind ext: the API methods that pin 16c25522e123 adds or changes and that the other kinds do not
# send (target/continuation-r97-goport/upstream/bumpB/api-battery/design.md, section numbers below).
# Wave 1 and wave 2 methods are in separate traces, so a missing wave 2 handler cannot turn wave 1 events
# into not_run. `check --only` keys: ext_file, ext_misc, ext_config, adder, temp, emit, transpile.
# ---------------------------------------------------------------------------

REL_IMPORT = re.compile(r"""(?:from|import)\s*['"](\.{1,2}/[^'"]+)['"]""")
TEMP_SUFFIX = "\nexport const __apiOracleTemp: number = \"x\";\n"


def u16_offsets(text):
    """UTF-16 offset of each code point index of `text`, plus the end."""
    out, n = [], 0
    for ch in text:
        out.append(n)
        n += 2 if ord(ch) > 0xFFFF else 1
    out.append(n)
    return out


def line_start_u16(text, offs, pos16):
    """UTF-16 offset of the start of the line that holds UTF-16 offset pos16."""
    ci = bisect.bisect_left(offs, pos16)
    return offs[text.rfind("\n", 0, ci) + 1]


def resolve_rel_import(rel, spec, roots):
    base = re.sub(r"\.(m|c)?js$", "", os.path.normpath(os.path.join(os.path.dirname(rel), spec)))
    return next((c for c in (base, base + ".ts", base + ".mts", base + "/index.ts") if c in roots), None)


def first_statement_end(enc, decl, K):
    """UTF-16 end of the first statement of the body block of node `decl`, or None (no body, empty body)."""
    block = stmts = None
    for j in range(decl + 1, enc.count):
        kind, _pos, end, _next, parent, _data, _flags = enc.node(j)
        if block is None:
            if parent == decl and kind == K["KindBlock"]:
                block = j
        elif stmts is None:
            if parent == block and kind == 0xFFFFFFFF:
                stmts = j
        elif parent == stmts:
            return end
    return None


class ExtPlan:
    """The files of the ext traces (section 4.1), chosen from the root file list of `build`."""

    def __init__(self, name, preset, files, encoded, kinds, caps, project, samples=None):
        cfg = EXT_PRESETS[name]
        pdir = preset["dir"]
        self.preset, self.kinds, self.files = preset, kinds, files
        self.enc = {f: EncodedFile(encoded[f]) for f in files}
        self.samples = samples or {f: FileSamples(self.enc[f], f, kinds, caps) for f in files}
        self.text = {}
        for f in files:
            with open(os.path.join(pdir, f), encoding="utf-8", newline="") as fh:
                self.text[f] = fh.read()
        roots = set(files)
        imports = {f: sorted({r for r in (resolve_rel_import(f, sp, roots) for sp in REL_IMPORT.findall(self.text[f]))
                              if r}) for f in files}
        importers = {f: sorted(g for g in files if f in imports[g]) for f in files}
        excl = [glob_regex(p) for p in cfg["sampleExclude"]]
        ts_files = [f for f in files if not f.endswith(".d.ts") and not any(r.match(f) for r in excl)]
        # The bigger half by encoded node count (ties by name), in name order: enough constructs per file.
        by_nodes = sorted(ts_files, key=lambda f: (-self.enc[f].count, f))
        big = sorted(by_nodes[:max(cfg["files"], len(by_nodes) // 2)])
        self.ext_files = spread(big, cfg["files"])
        # Import adder: target A, B1 = A's first imported root, B2 = the first big file that A does not import.
        self.adder = []
        for a in spread([f for f in big if imports[f]], 3):
            b2 = next((g for g in big if g != a and g not in imports[a] and not g.endswith("index.ts")), None)
            if b2:
                self.adder.append((a, imports[a][0], b2))
        # Temporary snapshots: small files that import a root and that a root imports, with their first importer.
        small = [f for f in ts_files if len(self.text[f].encode()) <= 25000 and imports[f] and importers[f]]
        self.temp = [(t, importers[t][0]) for t in spread(small, 2)]
        self.emit_files = spread(big, 8)
        self.variants = spread(big, 5)
        self.jsx = spread(glob_files(pdir, cfg["jsx"], []), 5) if cfg.get("jsx") else []
        self.fixture = cfg.get("fixture") or []
        # The project's own compilerOptions (the bundler-plugin use of transpile), project dir as @PROJECT_DIR@.
        self.options = Normalizer([(pdir, "@PROJECT_DIR@")]).strings(project.get("compilerOptions") or {})

    def summary(self):
        return {"extFiles": self.ext_files, "adder": self.adder, "temp": self.temp, "emitFiles": self.emit_files,
                "variants": self.variants, "jsx": self.jsx, "fixture": self.fixture}

    def handle(self, rel, i):
        return self.samples[rel].handle(i, os.path.join(self.preset["dir"], rel))


def at(ev, pointer, into, **kw):
    return {"event": ev, "pointer": pointer, "into": into, **kw}


def pdir_file(rel):
    return "@PROJECT_DIR@/" + rel


def ext_file_trace(ext, rel):
    """Section 4.2 ext_file: wave 1 methods about one file (#4791, #4700, #4893, #4689, #4897, #4556, #3515)."""
    K, s, enc, text = ext.kinds, ext.samples[rel], ext.enc[rel], ext.text[rel]
    tb = TraceBuilder()
    open_session(tb, ext.preset)
    ck, obj, file = tb.ck, tb.obj, pdir_file(rel)

    def about_symbol(spec):
        for m in ("getFullyQualifiedName", "getJsDocTags", "getDocumentationComment"):
            tb.req(m, ck(), spec)

    mod = tb.req("getSymbolOfSourceFile", ck(file=file))
    tb.req("getFullyQualifiedName", ck(), at(mod, "/id", "/symbol"))
    exports = tb.req("getExportsOfSymbol", obj(), at(mod, "/id", "/objectId"))
    for k in range(3):
        about_symbol(at(exports, "", "/symbol", pick={"sortBy": ["name"], "index": k, "then": "/id"}))
    for i, pos in s.ids[:4]:
        a = tb.req("getSymbolAtPosition", ck(file=file, position=pos))
        about_symbol(at(a, "/id", "/symbol"))
        t = tb.req("getTypeAtPosition", ck(file=file, position=pos))
        ap = tb.req("getApparentType", obj(), at(t, "/id", "/objectId"))
        tb.req("typeToString", ck(), at(ap, "/id", "/type"))
        tb.req("getApparentPropertiesOfType", obj(), at(t, "/id", "/objectId"))
    if s.ids:
        i, pos = s.ids[0]
        for meaning in (SF_TYPE_PARAMETER, SF_ALIAS, SF_BLOCK_SCOPED_VARIABLE):
            tb.req("getSymbolsInScope", ck(file=file, position=pos, meaning=meaning))
        tb.req("getSymbolsInScope", ck(location=ext.handle(rel, i), meaning=SF_BLOCK_SCOPED_VARIABLE | SF_FUNCTION))
    # Type parameter handles come from TypeParameter nodes, gated on the type flag (design decision 8).
    is_tp = [{"pointer": "/flags", "mask": TF_TYPE_PARAMETER}]
    tp_nodes = [j for j in range(1, enc.count) if enc.node(j)[0] == K["KindTypeParameter"]]
    for j in spread(tp_nodes, 3):
        t = tb.req("getTypeAtLocation", ck(location=ext.handle(rel, j)))
        d = tb.req("getDefaultFromTypeParameter", obj(), at(t, "/id", "/objectId", when=is_tp))
        tb.req("typeToString", ck(), at(d, "/id", "/type"))
        tb.req("getConstraintOfTypeParameter", obj(), at(t, "/id", "/objectId", when=is_tp))
    for i in s.calls[:2]:
        g = tb.req("getResolvedSignature", ck(location=ext.handle(rel, i)))
        for index in (0, 1):
            tp = tb.req("getTypeParameterAtPosition", ck(index=index), at(g, "/id", "/signature"))
            tb.req("typeToString", ck(), at(tp, "/id", "/type"))
    offs = u16_offsets(text)
    for i in s.sig_decls[:2]:
        t = tb.req("getTypeAtLocation", ck(location=ext.handle(rel, i)))
        sg = tb.req("getSignaturesOfType", ck(kind=SIG_CALL), at(t, "/id", "/type"))
        tps = tb.req("getTypeParametersOfSignature", obj(),
                     at(sg, "/0/id", "/objectId", when=[{"pointer": "/0/typeParameters", "nonzero": True}]))
        tb.req("getDefaultFromTypeParameter", obj(), at(tps, "/0/id", "/objectId"))
        p_close = line_start_u16(text, offs, enc.node(i)[2] - 1)  # the line of the closing "}"
        body_end = first_statement_end(enc, i, K)
        d1 = tb.req("signatureToSignatureDeclaration", ck(kind=K["KindMethodSignature"]), at(sg, "/0/id", "/signature"))
        tb.req("formatNodeForInsertion", ck(file=file, position=p_close), at(d1, "/data", "/data"))
        tb.req("formatNodeForInsertion", ck(file=file, position=0), at(d1, "/data", "/data"))
        d2 = tb.req("signatureToSignatureDeclaration", ck(kind=K["KindFunctionDeclaration"]),
                    at(sg, "/0/id", "/signature"))
        tb.req("formatNodeForInsertion", ck(file=file, position=offs[-1]), at(d2, "/data", "/data"))
        tn = tb.req("typeToTypeNode", ck(), at(t, "/id", "/type"))
        if body_end is not None:
            tb.req("formatNodeForInsertion", ck(file=file, position=line_start_u16(text, offs, body_end - 1)),
                   at(tn, "/data", "/data"))
    if PROTOCOL >= 5:
        ext_symbol_owner_events(tb, ext, rel, mod)
    return tb.events


# Declaration node kinds (Go ast.IsDeclaration) for getSymbolOfDeclaration samples.
DECLARATION_KINDS = ("KindFunctionDeclaration", "KindClassDeclaration", "KindInterfaceDeclaration",
                     "KindTypeAliasDeclaration", "KindEnumDeclaration", "KindVariableDeclaration",
                     "KindMethodDeclaration", "KindPropertyDeclaration", "KindParameter")


def ext_symbol_owner_events(tb, ext, rel, mod):
    """Protocol 5: the 7 methods of #64518, #64571 and #64598, after the other ext_file events, so their keys
    stay. The file descriptor is the /reference/file of the module symbol (event `mod`; a script file has none,
    so those requests skip). A declaration index is a node index of this file's encoded AST."""
    K, s, enc = ext.kinds, ext.samples[rel], ext.enc[rel]
    ck, file = tb.ck, pdir_file(rel)
    desc = at(mod, "/reference/file", "/file")
    r = tb.req("retainSourceFile", {}, desc)
    tb.req("getCachedSourceFile", {}, desc)
    tb.req("releaseSourceFile", {}, at(r, "/lease", "/lease"))
    kinds = {K[k] for k in DECLARATION_KINDS}
    for j in spread([j for j in range(1, enc.count) if enc.node(j)[0] in kinds], 3):
        tb.req("getSymbolOfDeclaration", {"index": j}, desc)
        tb.req("getSymbolOfDeclarationForChecker", ck(location=ext.handle(rel, j)))
        tb.req("getSymbolOfNode", ck(location=ext.handle(rel, j)))
    for i, pos in s.ids[:3]:
        a = tb.req("getSymbolAtPosition", ck(file=file, position=pos))
        m = tb.req("getMergedSymbol", ck(), at(a, "/id", "/symbol"))
        tb.req("getParentOfSymbolForChecker", ck(), at(m, "/id", "/symbol"))
        tb.req("getSymbolOfNode", ck(location=ext.handle(rel, i)))
    # errors: the source file node, an index past the end, a descriptor of no cached file
    tb.req("getSymbolOfDeclaration", {"index": 0}, desc)
    tb.req("getSymbolOfDeclaration", {"index": enc.count + 10}, desc)
    tb.req("getCachedSourceFile", {"file": {"fileName": file, "path": file, "contentHash": "0" * 32,
                                            "parseOptionsKey": "0", "scriptKind": 3, "nodeId": "1"}})


def ext_misc_trace(ext):
    """Section 4.2 ext_misc: once per project (#4533, #4569, #4791, #4897 globals, error probes)."""
    tb = TraceBuilder()
    open_session(tb, ext.preset)
    ck, obj = tb.ck, tb.obj
    n = tb.req("getNonPrimitiveType", ck())
    tb.req("typeToString", ck(), at(n, "/id", "/type"))
    tb.req("getPropertiesOfType", ck(), at(n, "/id", "/type"))
    tb.req("getWellKnownSignatures", ck())
    tb.req("getSymbolsOfSourceFiles", ck(files=[pdir_file(f) for f in ext.ext_files]))
    tb.req("getSymbolsOfSourceFiles", ck(files=[]))
    tb.req("getSymbolsOfSourceFiles", ck(files=[pdir_file(ext.files[0]), pdir_file("does/not/exist.ts")]))
    tb.req("getSymbolOfSourceFile", ck(file="bundled:///libs/lib.es5.d.ts"))
    first = ext.ext_files[0]
    pos = ext.samples[first].ids[0][1] if ext.samples[first].ids else 0
    for meaning in (SF_VALUE, SF_TYPE):  # all globals: 0.4 to 0.7 MB each (design decision 11)
        tb.req("getSymbolsInScope", ck(file=pdir_file(first), position=pos, meaning=meaning))
    # errors
    tb.req("getSymbolsInScope", ck(meaning=SF_VALUE))
    tb.req("getSymbolsInScope", ck(location="bad-handle", meaning=SF_VALUE))
    call = next(((f, ext.samples[f].calls[0]) for f in ext.ext_files if ext.samples[f].calls), None)
    if call:
        g = tb.req("getResolvedSignature", ck(location=ext.handle(*call)))
        tb.req("getTypeParameterAtPosition", ck(index=-1), at(g, "/id", "/signature"))
    tb.req("getFullyQualifiedName", ck(symbol=987654321))
    tb.req("getApparentType", {**obj(), "objectId": 4000000000})
    return tb.events


def ext_config_trace(ext):
    """Section 4.2 ext_config: config files of the project (#4724, #4888, #4627)."""
    preset = ext.preset
    tb = TraceBuilder()
    open_session(tb, preset)
    ck = tb.ck
    conf = pdir_file(preset["tsconfig"])
    names = tb.req("getConfigFileNames", ck())
    for ptr in ("/0", "/1"):
        tb.req("getConfigSourceFile", ck(), at(names, ptr, "/file"))
    tb.req("getConfigSourceFile", ck(file=pdir_file(ext.files[0])))
    tb.req("getConfigSourceFile", ck(file=pdir_file("does/not/exist.json")))
    r = tb.req("readConfigFile", {"file": conf})
    tb.req("readConfigFile", {"file": preset["tsconfig"]})
    tb.req("readConfigFile", {"file": {"uri": "file://" + os.path.join(preset["dir"], preset["tsconfig"])}})
    x = tb.req("readConfigFile", {}, at(names, "/1", "/file"))
    tb.req("readConfigFile", {"file": "package.json"})
    tb.req("readConfigFile", {"file": "missing.json"})
    pj = "parseJsonConfigFileContent"
    tb.req(pj, {"configFileName": conf}, at(r, "/config", "/json"))
    tb.req(pj, {"configDirectory": "@PROJECT_DIR@"}, at(r, "/config", "/json"))
    tb.req(pj, {}, [at(x, "/config", "/json"), at(names, "/1", "/configFileName")])
    tb.req(pj, {"json": {}, "configDirectory": "@PROJECT_DIR@", "configFileName": conf})
    tb.req(pj, {"json": {}})
    tb.req(pj, {"json": {"files": [None], "include": [None], "exclude": [None]}, "configDirectory": "@PROJECT_DIR@"})
    first = ext.files[0]
    for argv in (["-p", conf], ["-p", conf, "--noEmit", "--declaration", "--outDir", "out"],
                 [first, "--target", "es2022", "--module", "nodenext", "--strict"],
                 ["--lib", "es2022,dom", "--types", "node", first], ["--notAnOption"], ["--target", "es3000"], []):
        tb.req("parseCommandLine", {"commandLine": argv})
    tb.req("parseConfigFile", {}, at(names, "/1", "/file"))
    return tb.events


def ext_adder_trace(ext):
    """Section 4.2 ext_adder: getImportAdderEdits (#3881, wave 2)."""
    tb = TraceBuilder()
    open_session(tb, ext.preset)
    ck, obj = tb.ck, tb.obj

    def pick(ev, index, slot, mask=None):
        p = {"sortBy": ["name"], "index": index, "then": "/id"}
        if mask:
            p["where"] = {"field": "flags", "mask": mask}
        return at(ev, "", f"/actions/{slot}/symbol", pick=p)

    def adder(a, specs, **kw):
        tb.req("getImportAdderEdits", ck(file=pdir_file(a), actions=[{"kind": "importSymbol", **kw} for _ in specs]),
               specs)

    for a, b1, b2 in ext.adder:
        m = tb.req("getSymbolsOfSourceFiles", ck(files=[pdir_file(b1), pdir_file(b2)]))
        e1 = tb.req("getExportsOfSymbol", obj(), at(m, "/0/id", "/objectId"))
        e2 = tb.req("getExportsOfSymbol", obj(), at(m, "/1/id", "/objectId"))
        adder(a, [pick(e2, 0, 0)])  # a new import
        adder(a, [pick(e2, 0, 0), pick(e2, 1, 1)])  # two names, one import
        adder(a, [pick(e1, 0, 0)])  # a module that A imports
        adder(a, [pick(e1, 0, 0), pick(e2, 0, 1)])  # two modules
        adder(a, [pick(e2, 0, 0, SF_TYPE)], isValidTypeOnlyUseSite=False)
    a = pdir_file(ext.adder[0][0])
    for actions in ([{"kind": "unknown"}], [{"kind": "importSymbol", "symbol": 0}],
                    [{"kind": "importSymbol", "symbol": 987654321}]):
        tb.req("getImportAdderEdits", ck(file=a, actions=actions))
    tb.req("getImportAdderEdits", ck(file=pdir_file("does/not/exist.ts"),
                                     actions=[{"kind": "importSymbol", "symbol": 987654321}]))
    return tb.events


def ext_temp_trace(ext):
    """Section 4.2 ext_temp: updateTemporarySnapshot (#4642, wave 2). Overlays only; no file is written."""
    tb = TraceBuilder()
    s1 = open_session(tb, ext.preset)
    snap = lambda ev: at(ev, "/snapshot", "/snapshot")  # noqa: E731
    pr = lambda **kw: {"project": "@PROJECT@", **kw}  # noqa: E731
    first_x = None
    for t, imp in ext.temp:
        text, file = ext.text[t], pdir_file(t)
        tb.req("getSemanticDiagnostics", pr(files=[file]), snap(s1))
        tb.snap({})  # a newer snapshot stays the latest
        x = tb.temp({"file": file, "newText": text + TEMP_SUFFIX}, snap(s1))
        first_x = first_x if first_x is not None else x
        tb.req("getSemanticDiagnostics", pr(files=[file]), snap(x))
        tb.req("getSyntacticDiagnostics", pr(files=[file]), snap(x))
        ty = tb.req("getTypeAtPosition", pr(file=file, position=utf16_len(text) + len("\nexport const ")), snap(x))
        tb.req("typeToString", pr(), [snap(x), at(ty, "/id", "/type")])
        tb.req("getSourceFile", pr(file=file), snap(x))
        tb.req("getSemanticDiagnostics", pr(files=[pdir_file(imp)]), snap(x))
        y = tb.temp({"file": file, "newText": text}, snap(x))  # temporary on temporary
        tb.req("getSemanticDiagnostics", pr(files=[file]), snap(y))
        tb.req("release", {}, snap(y))
        tb.req("release", {}, snap(x))
        tb.req("getSemanticDiagnostics", pr(files=[file]), snap(s1))
        tb.req("getSourceFile", pr(file=file), snap(s1))
        z_file = pdir_file(os.path.join(os.path.dirname(t), "__api_oracle_temp.ts"))  # not on disk
        z_text = f"import * as m from \"./{os.path.splitext(os.path.basename(t))[0]}\";\nexport const probe = m;\n"
        z = tb.temp({"file": z_file, "newText": z_text}, snap(s1))
        tb.req("getDefaultProjectForFile", {"file": z_file}, snap(z))
        tb.req("getSemanticDiagnostics", pr(files=[z_file]), snap(z))
        tb.req("getTypeAtPosition", pr(file=z_file, position=z_text.index("probe")), snap(z))
        tb.req("release", {}, snap(z))
    t0 = pdir_file(ext.temp[0][0])
    tb.temp({"file": pdir_file("notes.txt"), "newText": "x"}, snap(s1))
    tb.temp({"file": t0, "newText": "x"}, snap(first_x))  # released
    tb.temp({"snapshot": 999, "file": t0, "newText": "x"}, None)
    tb.snap({})
    tb.req("getSemanticDiagnostics", tb.diag(t0))
    return tb.events


def ext_emit_trace(ext):
    """Section 4.2 ext_emit: emit to memory (#4699, wave 2). These handlers never write."""
    tb = TraceBuilder()
    open_session(tb, ext.preset)
    ck = tb.ck
    tb.req("emitToString", ck())
    for emit_only in (1, 2, 3):
        tb.req("emitToString", ck(emitOnly=emit_only))
    files = [pdir_file(f) for f in ext.emit_files]
    for f in files:
        tb.req("getJavaScriptEmit", ck(files=[f]))
        tb.req("getDeclarationEmit", ck(files=[f]))
    tb.req("getJavaScriptEmit", ck(files=files[:5]))
    tb.req("getDeclarationEmit", ck(files=files[:5]))
    tb.req("getJavaScriptEmit", ck(files=[]))
    tb.req("getDeclarationEmit", ck(files=[{"uri": "file://" + os.path.join(ext.preset["dir"], ext.emit_files[0])}]))
    tb.req("getJavaScriptEmit", ck(files=[pdir_file("does/not/exist.ts")]))
    tb.req("getJavaScriptEmit", ck())
    return tb.events


def ext_emit_fixture_trace(ext):
    """Section 4.2 ext_emit_fixture: `emit` (#4699) on a fixture under the run dir, with the writeFile
    callback (design decision 4). Returns (fixture, events)."""
    names = [os.path.basename(rel) for rel in ext.fixture]
    fx = {}
    for rel in ext.fixture:
        with open(os.path.join(ext.preset["dir"], rel), encoding="utf-8", newline="") as f:
            fx["emitfx/" + os.path.basename(rel)] = f.read()
    config = {"compilerOptions": {"target": "es2020", "module": "esnext", "moduleResolution": "bundler",
                                  "lib": ["es2022", "dom"], "strict": True, "declaration": True, "declarationMap": True,
                                  "sourceMap": True, "outDir": "out", "types": []}, "files": names}
    fx["emitfx/tsconfig.json"] = json.dumps(config, indent=2) + "\n"
    fx["emitfx/tsconfig.noemit.json"] = json.dumps({"extends": "./tsconfig.json", "compilerOptions": {"noEmit": True}},
                                                   indent=2) + "\n"
    fx["emitfx/tsconfig.errors.json"] = json.dumps({"extends": "./tsconfig.json",
                                                    "compilerOptions": {"noEmitOnError": True},
                                                    "files": names + ["bad.ts"]}, indent=2) + "\n"
    fx["emitfx/bad.ts"] = "export const n: number = \"x\";\n"
    fx["emitfx/bad.json"] = "{\n  \"compilerOptions\": {\n    \"strict\" true\n  }\n}\n"
    cfg = lambda name: "@RUN_DIR@/emitfx/" + name  # noqa: E731
    ck = lambda name, **kw: {"snapshot": "@SNAPSHOT@", "project": "@PROJECT:" + cfg(name) + "@", **kw}  # noqa: E731
    tb = TraceBuilder()
    tb.req("initialize")
    tb.snap(open_params(cfg("tsconfig.json")))
    tb.req("emit", ck("tsconfig.json"))
    for emit_only in (1, 2, 3):
        tb.req("emit", ck("tsconfig.json", emitOnly=emit_only))
    tb.req("emitToString", ck("tsconfig.json"))
    tb.snap(open_params(cfg("tsconfig.noemit.json")))
    tb.req("emit", ck("tsconfig.noemit.json"))  # skipped: noEmit
    tb.req("getJavaScriptEmit", ck("tsconfig.noemit.json", files=[cfg(names[0])]))  # forced
    tb.snap(open_params(cfg("tsconfig.errors.json")))
    tb.req("emit", ck("tsconfig.errors.json"))  # skipped: noEmitOnError and TS2322
    tb.req("emitToString", ck("tsconfig.errors.json"))
    tb.req("readConfigFile", {"file": cfg("bad.json")})
    return fx, tb.events


TRANSPILE_MODULE_SETS = [
    ({"module": 1, "target": 9, "sourceMap": True, "esModuleInterop": True}, True),
    ({"module": 199, "target": 99, "verbatimModuleSyntax": True}, True),
    ({"module": 99, "target": 9, "inlineSourceMap": True, "inlineSources": True, "removeComments": True}, True),
    ({"module": 99, "target": 2, "experimentalDecorators": True}, True),
    ({}, False)]
TRANSPILE_DECLARATION_SETS = [({"declarationMap": True}, True), ({"stripInternal": True, "target": 99}, True)]
TRANSPILE_SYNTAX_ERROR = "const x: = 1;\nexport {}"


def ext_transpile_trace(ext, declaration):
    """Section 4.2 ext_transpile_module and ext_transpile_declaration (#4849, wave 2). No snapshot.
    Go panics on a .d.ts input ("Output generation failed"); the event stays (design decision 10)."""
    tb = TraceBuilder()
    tb.req("initialize")
    m_file = "transpileDeclarationFromFile" if declaration else "transpileModuleFromFile"
    m_text = "transpileDeclaration" if declaration else "transpileModule"
    def from_file(f, options, report=True):
        tb.req(m_file, {"fileName": pdir_file(f), "options": {"compilerOptions": options, "reportDiagnostics": report}})

    for f in ext.files:
        from_file(f, ext.options)
    for f in ext.variants:
        for options, report in TRANSPILE_DECLARATION_SETS if declaration else TRANSPILE_MODULE_SETS:
            from_file(f, options, report)
    inline = {"compilerOptions": {"module": 99, "target": 9}, "reportDiagnostics": True}
    for f in ext.variants[:2]:
        tb.req(m_text, {"input": ext.text[f], "options": {**inline, "fileName": f}})
    if declaration:
        tb.req(m_text, {"input": TRANSPILE_SYNTAX_ERROR, "options": {"reportDiagnostics": True}})
        tb.req(m_text, {"input": "export function f(a) { return a; }\n", "options": {"reportDiagnostics": True}})
    else:
        tb.req(m_text, {"input": ext.text[ext.variants[0]], "options": inline})  # Go default name module.ts
        tb.req(m_text, {"input": TRANSPILE_SYNTAX_ERROR, "options": {"reportDiagnostics": True}})
        tb.req(m_text, {"input": TRANSPILE_SYNTAX_ERROR, "options": {"reportDiagnostics": False}})
        tb.req(m_text, {"input": "", "options": {}})
        tb.req(m_text, {"input": "import x = require(\"y\");\nexport = x;\n",
                        "options": {"compilerOptions": {"module": 1}}})
        for f in ext.jsx:
            from_file(f, {"jsx": 4, "jsxImportSource": "hono/jsx", "module": 99, "target": 9})
            from_file(f, {"jsx": 1})
        if ext.jsx:
            tb.req(m_text, {"input": "export const el = <div id=\"a\">{1}</div>;\n",
                            "options": {"compilerOptions": {"jsx": 4}, "reportDiagnostics": True}})  # module.tsx
    tb.req(m_file, {"fileName": pdir_file("does/not/exist.ts"), "options": {}})
    return tb.events


def ext_traces(ext, battery):
    """(header, events) of every ext trace (section 3)."""
    h = lambda name, **kw: trace_header(name, battery, ext.preset, **kw)  # noqa: E731
    for rel in ext.ext_files:
        yield h("ext_file_" + slug(rel), file=rel), ext_file_trace(ext, rel)
    yield h("ext_misc"), ext_misc_trace(ext)
    yield h("ext_config"), ext_config_trace(ext)
    if ext.adder:
        yield h("ext_adder"), ext_adder_trace(ext)
    if ext.temp:
        yield h("ext_temp"), ext_temp_trace(ext)
    yield h("ext_emit"), ext_emit_trace(ext)
    if ext.fixture:
        fx, events = ext_emit_fixture_trace(ext)
        yield h("ext_emit_fixture", callbacks=["writeFile"], fixture=fx), events
    yield h("ext_transpile_module"), ext_transpile_trace(ext, False)
    yield h("ext_transpile_declaration"), ext_transpile_trace(ext, True)


def check_emit_guard(header, events):
    """The build side of SessionRun._emit_guard: a trace sends `emit` only to a fixture project."""
    for k, ev in enumerate(events):
        if ev.get("method") != "emit":
            continue
        project = (ev.get("params") or {}).get("project")
        if "writeFile" not in (header.get("callbacks") or []) or not header.get("fixture") or ev.get("paramsFrom") \
                or not isinstance(project, str) or not project.startswith("@PROJECT:@RUN_DIR@/"):
            raise HarnessError(f"{header['name']} event {k}: emit outside a fixture project")


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    sub = ap.add_subparsers(dest="cmd", required=True)

    def common(p, oracle=True):
        p.add_argument("--out-root", default=DEFAULT_OUT_ROOT)
        if oracle:
            p.add_argument("--oracle", default=DEFAULT_ORACLE)
        p.add_argument("--jobs", type=int, default=1)
        p.add_argument("--only")
        p.add_argument("--request-timeout", type=float)
        p.add_argument("--keep-temp", action="store_true")

    p = sub.add_parser("build", help="write traces for a project (runs the oracle once to find nodes)")
    p.add_argument("--out-root", default=DEFAULT_OUT_ROOT)
    p.add_argument("--oracle", default=DEFAULT_ORACLE)
    p.add_argument("--preset", required=True)
    p.add_argument("--battery", required=True)
    p.add_argument("--kind", default="files", choices=["files", "proto", "callbacks", "lsp", "xchecker", "ext"])
    p.add_argument("--limit", type=int, help="first N files")
    p.add_argument("--sample", type=int, help="N files spread over the list")
    p.add_argument("--cap", action="append", help="override a sample cap, e.g. ids=4")
    p.add_argument("--force", action="store_true")
    p = sub.add_parser("record", help="run tsgo once per trace and write goldens")
    common(p)
    p.add_argument("--battery", required=True)
    p.add_argument("--force", action="store_true")
    p = sub.add_parser("selfcheck", help="run tsgo again and write .flaky.json")
    common(p)
    p.add_argument("--battery", required=True)
    p.add_argument("--runs", type=int, default=1)
    p = sub.add_parser("check", help="run goport and classify each request")
    common(p)
    p.add_argument("--battery", required=True)
    p.add_argument("--goport", required=True)
    p.add_argument("--label", required=True)
    p.add_argument("--oracle-sha", help="golden set (default: hash of --oracle)")
    p.add_argument("--wire", type=int, choices=[3, 4], help="3: send the snapshot events of protocol 4 traces in "
                   "their protocol 3 form; 4: send protocol 5 traces in their protocol 4 form (a ruling 10 rebase "
                   "run of base bins; needs a reviewer ruling)")
    p = sub.add_parser("summary")
    p.add_argument("--out-root", default=DEFAULT_OUT_ROOT)
    p.add_argument("--label", required=True)
    p.add_argument("--baseline")
    p = sub.add_parser("show")
    p.add_argument("--out-root", default=DEFAULT_OUT_ROOT)
    p.add_argument("--label", required=True)
    p.add_argument("--battery", required=True)
    p.add_argument("--trace", required=True)
    p.add_argument("--event", required=True)
    p.add_argument("--max-lines", type=int, default=120)
    args = ap.parse_args(argv)
    fn = {"build": cmd_build, "record": cmd_record, "selfcheck": cmd_selfcheck, "check": cmd_check,
          "summary": cmd_summary, "show": cmd_show}[args.cmd]
    try:
        return fn(args)
    except UsageError as e:
        print(f"api_oracle: {e}", file=sys.stderr)
        return EXIT_USAGE
    except InputChanged as e:
        print(f"api_oracle: {e}", file=sys.stderr)
        return EXIT_INPUT_CHANGED
    except HarnessError as e:
        print(f"api_oracle: {e}", file=sys.stderr)
        return EXIT_FAILED


if __name__ == "__main__":
    if os.environ.get("GOPORT_PIN") and not os.environ.get("GOPORT_PIN_ACTIVE"):
        os.execvp(sys.executable, [sys.executable, PIN_TOOL, "exec", "--", sys.executable,
                                   os.path.abspath(__file__), *sys.argv[1:]])
    sys.exit(main())
