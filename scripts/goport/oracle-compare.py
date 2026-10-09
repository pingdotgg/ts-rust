#!/usr/bin/env python3
"""Compares two LSP or API oracle results request by request: the base (last accepted revision) and a candidate.

usage: scripts/goport/oracle-compare.py <base results dir>... <new results dir> [--answers FILE@SHA256]...
                                        [--parity] [--known-diff KEY]... [--identity] [--kind lsp|api] [--out FILE]

A results dir is <out-root>/results/<label> of `scripts/goport/lsp_oracle.py check` or
`scripts/goport/api_oracle.py check`. Each traces/<battery>/**/<trace>.json there lists the requests of
one trace with their class. A request is keyed by battery, trace and event (LSP "i", API "event").
A base request is protected when its class is "same" or "oracle_error_same" (goport gives the oracle's
answer, or the oracle's error). Per request:
- retained: protected in the base and in the new run
- recovered: not protected in the base, protected in the new run
- lost: protected in the base, another class in the new run (not an unrun class)
- unrun: protected in the base, not_run, skipped_method or skipped in the new run
- absent: protected in the base, and the new run has no such request or trace
- newRequests: only in the new run
Both sides must use the same goldens (one oracle and pin). At a pin bump, Go's answers change: the base is
then the base revision measured again at the new pin (batch.oracleRebase, several base dirs below), or the
reviewer checks each loss against Go at the new pin.

Several base dirs (batch.oracleRebase, reviewer ruling 10): runs of the base revision's bins at the new pin.
Every run counts: a base request is protected when it is protected in any base run. Its base class is that
protected class, else flaky_oracle when a base run has it so, else its class in the first base run that has
it. The output also has bases, the head of each base dir in order (base stays the first).

Answer sets (--answers FILE@SHA256, repeatable): the recorded Go answers of flaky requests at one pin, as the
batch records them in oracleAnswers. A set is a file FILE.json.gz of format goport-oracle-answers/1: {format,
kind (lsp or api), pin, oracleSha256, goldenSha12, requests: {"<battery>/<trace>#<event>": {method, multiset
(patterns), answers: [{sha256, answer, sources}], ...}}}. Each answer's sha256 is the sha256 of its canon()
text. Each source is a Go golden of the request's trace at the set's oracle
(.../golden/<goldenSha12>/<battery>/<trace>.golden.jsonl.gz), and no source is listed twice for a request.
The file must have the sha256 of its name. The tool refuses a set with another sha256, another format or kind,
or a bad answer, source or header, and a request in two sets of one oracle.
goport's answer is its record in responses/<battery>/<trace>.jsonl.gz of the new results dir, normalized as
lsp_oracle.py and api_oracle.py do: the result after apply_multisets() with the patterns of the answer set, or
{status, response} for an error or a missing result, as canon() text. The oracle of a trace is its
oracleSha256 (LSP trace result) or manifest.json batteries.<battery>.oracleSha (API).
A base request whose class is flaky_oracle and whose key is in a set is protected in a weaker form. When the
new run's trace uses the oracle of a set that has the key (the same pin), the new request must have the set's
method and goport's answer must be one of the set's answers (retainedByAnswers), else it is lost (or unrun or
absent). With a set of another oracle only (a later pin bump) it is protected like a same request
(notApplied in the set's counts): retained, or lost, unrun or absent. With --answers, each battery and the
total also count retainedByAnswers, the output has kind and answers [{path, sha256, kind, pin, oracleSha256,
requests, applied, notApplied}], and a lostFirst row of such a request names its set (answers, carried,
answersWhy).
Masked entries (bump C reviewer ruling 1 item 3, ruling 2 item 1): a set whose header has the "mask" of its kind
(API "type-ids", LSP "auto-import-modules"; MASKS) may have entries with that "mask" and exactly one answer: the
Go answer after the mask, the same for every Go run at the set's pin. goport's answer is masked the same way
(after apply_multisets) before it is compared. A masked entry covers only its own key; every other key is
compared unmasked.
- API "type-ids": mask_type_ids (the API tool loaded at the set's pin) replaces only type and signature ids; the
  symbol field, symbol names, all flags and every other field stay exact (api_oracle.mask_ids, which also masks
  symbols and defines the id_only class, is not used).
- LSP "auto-import-modules" (bump D): a textDocument/completion entry only. The header also has "maskModules"
  {<auto-import name>: [<module>...]}: the names whose auto-import module Go varies between runs at the pin, each
  with the modules that Go gives in some but not all runs of one request (masked-answers.py). mask_auto_import_modules
  takes each auto-import item of such a name and one of its modules out of the ordered items and puts it, with "*"
  for its module specifier, once into the sorted list maskedItems (Go gives AlgorithmTypes once from the hono index
  in some runs, and twice, from middleware/jwt and utils/jwt/jwa, in others). An item of another name or of another
  module stays in place and exact, as does every other field.
The header also has "maskTool" {<path>: sha256} of the files whose code the mask runs when the set was made:
scripts/goport/oracle-compare.py and the kind's tool (api_oracle.py: SHAPES, walk_shape; lsp_oracle.py: canon).
When a sha256 differs from this checkout's file, every source golden of every masked entry is masked again with
this checkout's tools, and each must give the entry's answer, or the set is refused. Retention
through a masked entry counts as retainedByMaskedAnswers, not retainedByAnswers, and each set in the output has
maskedRequests, mask and maskTool {same, rechecked (source goldens masked again)}. These fields appear only
when a set has a masked entry, so other outputs stay the same.

Parity (--parity, ruling 10 condition 3): the new run itself must match Go at its pin. LSP: no request of
class diff, goport_error, oracle_error_diff, timeout or crash (lsp_oracle.py DIVERGENT_CLASSES) and no crash
exit (summary.json crashExits). API: no goport_error, crash or timeout, and each diff, id_only or
oracle_error_diff request is a known diff (--known-diff "<battery>/<trace>#<event>", repeatable; API only).
A request in an answer set of the oracle of its trace must have the set's method and goport's answer must be
one of the set's answers, whatever its class; it is then allowed. The output has parity {classes, crashExits,
answerRequests, allowedByAnswers, knownDiffs, knownDiffsUsed, knownDiffRows, bad, badFirst [{battery, trace,
event, method, class, why}]}. An unused known diff and an answer set request that the new run lacks are bad too.
knownDiffRows (bump C ruling 1 item 4) has one row per known diff, in the order given: {key, class, method,
pointer, sub, goldenError}, the new run's class, first diff pointer and sub of the request, and for an
oracle_error class the error message of the Go golden (<out-root>/golden/<oracle sha256 prefix>/<battery>/
<trace>.golden.jsonl.gz, where the results dir is <out-root>/results/<label>). Each row is also printed on
stderr, so the reviewer sees the class and pointer of every known diff at each run.

Identity (--identity): each head (base, bases, new) also has resultsSha256 (the sha256 of the lines
"<sha256>  ./<path>" of every regular file in the dir, sorted by path: the output of
`cd DIR && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum`), goportSha256 (the tsgo
sha256 values that the run records: LSP summary.json goport[].sha256, API manifest.json
batteries.*.goportSha) and oracleSha256 (the oracle sha256 values: LSP summary.json oracleSha256 and
each trace's, API manifest.json batteries.*.oracleSha), each sorted.

Wire (bump C reviewer ruling 1 item 1): `api_oracle.py check --wire 3` is only for the base re-measure of a
revision that speaks an older API protocol. The new run is refused (exit 2) when its manifest.json has a battery
with a "wire" key or a trace result has "wire" in its meta. Base runs may have it.

Prints one JSON object {base, new, protectedClasses, total, batteries, lostFirst[, bases, kind, answers,
parity]}, and writes it to --out when given. Without --answers, --parity and --identity, and with one base
dir, the output is the same as before these options. Exit 0: no loss. Exit 1: a protected base request is
lost, unrun or absent, or a parity problem. Exit 2: bad input or a refused answer set.
"""
import argparse, collections, gzip, hashlib, importlib.util, json, os, posixpath, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
# The main checkout (api_oracle.REPO): answer set sources are relative to it, also for a worktree's tools.
REPO = '/home/theo/Code/sandbox/ts-rust'
PROTECTED = ('same', 'oracle_error_same')
UNRUN = ('not_run', 'skipped_method', 'skipped')
FIELDS = ('retained', 'recovered', 'lost', 'unrun', 'absent', 'newRequests')
ANSWER_FIELDS = ('retainedByAnswers',)
KINDS = {'goport-lsp-summary/1': 'lsp', 'goport-lsp-result/1': 'lsp', 'goport-api-summary/1': 'api', 'goport-api-result/1': 'api'}
COMMIT = re.compile(r'^[0-9a-f]{7,40}$')
SHA256 = re.compile(r'^[0-9a-f]{64}$')
ANSWERS_FORMAT = 'goport-oracle-answers/1'
ANSWERED = ('ok', 'error')  # response statuses that hold an answer
ORACLE_TOOLS = {'lsp': 'lsp_oracle', 'api': 'api_oracle'}
# Parity: the classes that are always a problem, and (API) the classes that need a known diff.
PARITY_BAD = {'lsp': ('diff', 'goport_error', 'oracle_error_diff', 'timeout', 'crash'), 'api': ('goport_error', 'crash', 'timeout')}
PARITY_DIFF = {'lsp': (), 'api': ('diff', 'id_only', 'oracle_error_diff')}
# The mask kind of masked answer set entries per set kind (see the docstring, mask_type_ids() and
# mask_auto_import_modules()). MASK is the API kind (masked-answers.py).
MASKS = {'api': 'type-ids', 'lsp': 'auto-import-modules'}
MASK = MASKS['api']
# The method of every LSP masked entry.
LSP_MASK_METHOD = 'textDocument/completion'
# The files whose code a mask runs, per set kind, keyed as the answer set header's maskTool names them.
MASK_TOOL_FILES = {kind: {f'scripts/goport/{ORACLE_TOOLS[kind]}.py': os.path.join(HERE, ORACLE_TOOLS[kind] + '.py'),
                          'scripts/goport/oracle-compare.py': os.path.abspath(__file__)} for kind in MASKS}
_tools = {}
_pinned_tools = {}


def fail(msg):
    print(f'oracle-compare.py: {msg}', file=sys.stderr)
    sys.exit(2)


def read_json(path):
    try:
        return json.load(open(path))
    except (OSError, ValueError) as e:
        fail(f'cannot read {path}: {e}')


def tool(kind):
    """lsp_oracle.py or api_oracle.py of this checkout: their canon() and apply_multisets() normalize an answer."""
    if kind not in _tools:
        spec = importlib.util.spec_from_file_location(ORACLE_TOOLS[kind], os.path.join(HERE, ORACLE_TOOLS[kind] + '.py'))
        _tools[kind] = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(_tools[kind])
    return _tools[kind]


def tool_at(kind, pin):
    """lsp_oracle.py or api_oracle.py of this checkout, loaded as at a Go pin (GOPORT_PIN_ACTIVE, which sets the
    API protocol and with it the internal symbol name form that mask_ids masks)."""
    if (kind, pin) not in _pinned_tools:
        spec = importlib.util.spec_from_file_location(f'{ORACLE_TOOLS[kind]}_{pin}', os.path.join(HERE, ORACLE_TOOLS[kind] + '.py'))
        mod = importlib.util.module_from_spec(spec)
        saved = os.environ.get('GOPORT_PIN_ACTIVE')
        os.environ['GOPORT_PIN_ACTIVE'] = pin
        try:
            spec.loader.exec_module(mod)
        finally:
            if saved is None:
                del os.environ['GOPORT_PIN_ACTIVE']
            else:
                os.environ['GOPORT_PIN_ACTIVE'] = saved
        _pinned_tools[(kind, pin)] = mod
    return _pinned_tools[(kind, pin)]


def mask_type_ids(api, method, v):
    """An API answer v of method with every type and signature id replaced by "#" (the "type-ids" mask; bump C
    reviewer ruling 2 item 1), by the API tool api (tool_at(), loaded at the answer set's pin). Symbol references,
    symbol names (also the ids inside them), flags and every other field are kept. masked-answers.py builds masked
    sets with this function."""
    shape = api.SHAPES.get(method)
    if shape is None:
        return v
    return api.walk_shape(shape, v, lambda kind, val: '#' if kind in ('type', 'sig') and val not in (None, 0) else val)


def auto_import_module(item):
    """(name, module) of an LSP completion item with data.autoImport {name, moduleSpecifier}, else None. A relative
    module specifier (".", "..", "./..." or "../...") is resolved against the directory of data.fileName, so one
    module has one form in every file (for example "@PROJECT_ROOT@/src/middleware/language"); any other specifier
    (a package) stays as it is."""
    data = item.get('data') if isinstance(item, dict) else None
    auto = data.get('autoImport') if isinstance(data, dict) else None
    if not isinstance(auto, dict) or not isinstance(auto.get('name'), str) or not isinstance(auto.get('moduleSpecifier'), str):
        return None
    spec = auto['moduleSpecifier']
    if (spec in ('.', '..') or spec.startswith(('./', '../'))) and isinstance(data.get('fileName'), str):
        spec = posixpath.normpath(posixpath.join(posixpath.dirname(data['fileName']), spec))
    return auto['name'], spec


def mask_auto_import_modules(v, modules):
    """An LSP textDocument/completion answer v after the "auto-import-modules" mask (bump D). modules is the set
    header's maskModules {name: [module...]}. Each item with data.autoImport whose name modules lists and whose
    module (auto_import_module()) is one of that name's modules leaves items. Its copy with "*" for
    data.autoImport.moduleSpecifier, and for data.source and labelDetails.description when they equal the
    specifier, goes into maskedItems: each distinct copy once, sorted by canon() text. Go sorts the completion
    items by module specifier, so the item moves when Go gives the other module, and when two of a name's modules
    give one symbol, Go lists it once or twice. Every other item stays in its place and exact."""
    if not isinstance(v, dict) or not isinstance(v.get('items'), list):
        return v
    canon, items, masked = tool('lsp').canon, [], set()
    for item in v['items']:
        found = auto_import_module(item)
        if not found or found[1] not in modules.get(found[0], ()):
            items.append(item)
            continue
        copy = json.loads(json.dumps(item))
        spec = copy['data']['autoImport']['moduleSpecifier']
        copy['data']['autoImport']['moduleSpecifier'] = '*'
        if copy['data'].get('source') == spec:
            copy['data']['source'] = '*'
        details = copy.get('labelDetails')
        if isinstance(details, dict) and details.get('description') == spec:
            details['description'] = '*'
        masked.add(canon(copy))
    return {**v, 'items': items, 'maskedItems': [json.loads(text) for text in sorted(masked)]}


def masker(s, entry):
    """The mask function (answer -> masked answer) of a masked entry of the answer set s, or None."""
    if not entry['mask']:
        return None
    if s['kind'] == 'api':
        api = tool_at('api', s['pin'])
        return lambda v: mask_type_ids(api, entry['method'], v)
    return lambda v: mask_auto_import_modules(v, s['maskModules'])


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


def mask_tool(kind='api'):
    """{path: sha256} of the files whose code the mask of a set kind runs (the maskTool of a masked set header)."""
    return {name: sha256_file(path) for name, path in MASK_TOOL_FILES[kind].items()}


def keytext(key):
    return f'{key[0]}/{key[1]}#{key[2]}'


def parse_key(text):
    """(battery, trace, event) of "<battery>/<trace>#<event>" (battery up to the first "/", event after the last "#"),
    or None."""
    battery, _, rest = text.partition('/')
    trace, _, event = rest.rpartition('#')
    return (battery, trace, event) if battery and trace and event else None


def load(results_dir):
    """{(battery, trace, event): (class, method)} of one results dir, its head, and its trace info
    (kinds of the trace formats, oracle sha256 values the traces name, per trace its oracle, and per request
    with a diff pointer or sub its (pointer, sub))."""
    tdir = os.path.join(results_dir, 'traces')
    if not os.path.isdir(tdir):
        fail(f'no traces/ in {results_dir}')
    requests, traces, info = {}, 0, {'kinds': set(), 'oracles': set(), 'traces': {}, 'wire': set(), 'detail': {}}
    for d, _, files in sorted(os.walk(tdir)):
        for f in sorted(files):
            if not f.endswith('.json'):
                continue
            t = read_json(os.path.join(d, f))
            traces += 1
            info['kinds'].add(KINDS.get(t.get('format')))
            if t.get('oracleSha256'):
                info['oracles'].add(t['oracleSha256'])
            info['traces'][(t['battery'], t['trace'])] = t.get('oracleSha256')
            if 'wire' in (t.get('meta') or {}):
                info['wire'].add(f"{t['battery']}/{t['trace']}")
            for e in t.get('events') or []:
                event = e.get('i', e.get('event'))
                key = (t['battery'], t['trace'], str(event))
                requests[key] = (e['class'], e.get('method'))
                if e.get('pointer') is not None or e.get('sub') is not None:
                    info['detail'][key] = (e.get('pointer'), e.get('sub'))
    if not traces:
        fail(f'no trace results in {tdir}')
    return requests, {'dir': os.path.abspath(results_dir), 'label': os.path.basename(os.path.normpath(results_dir)),
                      'traces': traces, 'requests': len(requests)}, info


def side_file(results_dir, name):
    path = os.path.join(results_dir, name)
    return read_json(path) if os.path.isfile(path) else {}


def kind_of(results_dir, info):
    """lsp or api from summary.json's format, else from the trace formats; None when unknown."""
    kinds = {KINDS.get(side_file(results_dir, 'summary.json').get('format'))} | info['kinds']
    kinds.discard(None)
    if len(kinds) > 1:
        fail(f'{results_dir} has LSP and API results')
    return kinds.pop() if kinds else None


def goldens(results_dir, kind, info):
    """The oracle sha256 values (golden sets) that a results dir names."""
    if kind == 'lsp':
        return set(side_file(results_dir, 'summary.json').get('oracleSha256') or []) | info['oracles']
    batteries = side_file(results_dir, 'manifest.json').get('batteries') or {}
    return {b.get('oracleSha') for b in batteries.values() if isinstance(b, dict) and b.get('oracleSha')}


def identity(results_dir, kind, info):
    """--identity: resultsSha256, goportSha256 and oracleSha256 of a results dir (see the docstring)."""
    files = []
    for d, _, names in os.walk(results_dir):
        for name in names:
            path = os.path.join(d, name)
            if os.path.islink(path) or not os.path.isfile(path):
                continue
            files.append((os.path.relpath(path, results_dir).encode(), sha256_file(path)))
    listing = b''.join(f'{digest}  ./'.encode() + path + b'\n' for path, digest in sorted(files))
    if kind == 'lsp':
        goport = [g.get('sha256') for g in side_file(results_dir, 'summary.json').get('goport') or [] if isinstance(g, dict)]
    else:
        goport = [b.get('goportSha') for b in (side_file(results_dir, 'manifest.json').get('batteries') or {}).values()
                  if isinstance(b, dict)]
    return {'resultsSha256': hashlib.sha256(listing).hexdigest(), 'goportSha256': sorted({g for g in goport if g}),
            'oracleSha256': sorted(goldens(results_dir, kind, info))}


class Run:
    """The new results dir with its kind: the oracle of each trace, and on demand goport's answers."""

    def __init__(self, results_dir, kind, info):
        self.dir, self.kind, self.info = results_dir, kind, info
        self.goldens = goldens(results_dir, kind, info)
        self.batteries = side_file(results_dir, 'manifest.json').get('batteries') or {} if kind == 'api' else {}
        self.response_cache = {}

    def oracle(self, battery, trace):
        """The oracle sha256 of the golden that the run used for a trace (the run's only golden for a trace it
        does not have), or None."""
        if self.kind == 'lsp':
            found = self.info['traces'].get((battery, trace))
        else:
            found = (self.batteries.get(battery) or {}).get('oracleSha')
        return found or (next(iter(self.goldens)) if len(self.goldens) == 1 else None)

    def answer(self, key, patterns, mask=None):
        """goport's answer to a request in this run as canon() text (normalized as the oracle tools do), or None
        when goport gave no answer (no record, or a status other than ok and error). mask (masker() of a masked
        answer set entry): an ok result is masked with it."""
        battery, trace, event = key
        path = os.path.join(self.dir, 'responses', battery, trace + '.jsonl.gz')
        if path not in self.response_cache:
            records = {}
            if os.path.isfile(path):
                with gzip.open(path, 'rt', encoding='utf-8') as f:
                    lines = [json.loads(line) for line in f if line.strip()]
                if self.kind == 'lsp':
                    records = {str(r['i']): r for r in lines if 'i' in r}
                else:
                    api = tool('api')
                    records = {str(k): r for k, r in api.records_by_event(lines[1:]).items()} | api.special_records(lines[1:])
            self.response_cache[path] = records
        rec = self.response_cache[path].get(event)
        if rec is None or rec.get('status') not in ANSWERED:
            return None
        t, resp = tool(self.kind), rec.get('response') or {}
        if rec['status'] == 'error' or 'result' not in resp:
            return t.canon({'status': rec['status'], 'response': resp})
        v = t.apply_multisets(resp['result'], patterns)
        return t.canon(mask(v) if mask else v)


def load_answers(ref, cache):
    """The answer set that ref (FILE@SHA256) names: {path, sha256, kind, pin, oracleSha256, requests}, each request
    {method, multiset, texts (canon() text of each answer)}. Raises ValueError when the set is refused."""
    path, _, want = ref.strip().rpartition('@')
    if not path or not SHA256.match(want):
        raise ValueError(f'answer set {ref!r} is not FILE@SHA256')
    if (path, want) in cache:
        return cache[(path, want)]
    try:
        data = open(path, 'rb').read()
    except OSError as e:
        raise ValueError(f'cannot read the answer set {path}: {e}')
    if hashlib.sha256(data).hexdigest() != want:
        raise ValueError(f'answer set {path} does not have the sha256 {want}')
    try:
        doc = json.loads(gzip.decompress(data))
    except (OSError, ValueError) as e:
        raise ValueError(f'answer set {path} is not gzip JSON: {e}')
    kind, oracle = doc.get('kind'), doc.get('oracleSha256')
    if (doc.get('format') != ANSWERS_FORMAT or kind not in ORACLE_TOOLS or not isinstance(doc.get('pin'), str)
            or not COMMIT.match(doc['pin']) or not isinstance(oracle, str) or not SHA256.match(oracle)
            or doc.get('goldenSha12') != oracle[:12] or not isinstance(doc.get('requests'), dict)):
        raise ValueError(f'answer set {path} lacks the {ANSWERS_FORMAT} header (kind, pin, oracleSha256, goldenSha12, requests)')
    canon, requests = tool(kind).canon, {}
    set_mask, modules = doc.get('mask'), doc.get('maskModules')
    if any(isinstance(e, dict) and e.get('mask') is not None for e in doc['requests'].values()) or set_mask is not None:
        tools, files = doc.get('maskTool'), MASK_TOOL_FILES[kind]
        if (set_mask != MASKS[kind] or not isinstance(tools, dict) or sorted(tools) != sorted(files)
                or not all(isinstance(v, str) and SHA256.match(v) for v in tools.values())):
            raise ValueError(f'answer set {path} has masked entries: an {kind.upper()} set needs the header mask '
                             f'{MASKS[kind]!r} and maskTool {{{", ".join(sorted(files))}: sha256}}')
        if kind == 'lsp' and not (isinstance(modules, dict) and modules and all(
                isinstance(ms, list) and ms and len(set(ms)) == len(ms) and all(isinstance(m, str) and m for m in ms)
                for ms in modules.values())):
            raise ValueError(f'answer set {path} has masked entries: an LSP set needs maskModules {{name: [module...]}}, '
                             f'each name with distinct modules')
    for k, entry in doc['requests'].items():
        key = parse_key(k)
        answers = entry.get('answers') if isinstance(entry, dict) else None
        multiset = entry.get('multiset') or [] if isinstance(entry, dict) else None
        if not key or not answers or not isinstance(answers, list) or not isinstance(multiset, list):
            raise ValueError(f'answer set {path}: request {k} needs battery/trace#event, answers and multiset')
        mask = entry.get('mask')
        if mask is not None and (mask != set_mask or len(answers) != 1):
            raise ValueError(f'answer set {path}: request {k} has mask {mask!r}; a masked entry has the mask of the set '
                             f'header ({set_mask!r}) and one answer')
        if mask is not None and kind == 'lsp' and entry.get('method') != LSP_MASK_METHOD:
            raise ValueError(f'answer set {path}: request {k} is masked; an LSP masked entry is a {LSP_MASK_METHOD} request')
        texts, sources = [], set()
        for a in answers:
            text = canon(a.get('answer')) if isinstance(a, dict) else None
            if text is None or hashlib.sha256(text.encode('utf-8')).hexdigest() != a.get('sha256') or text in texts:
                raise ValueError(f'answer set {path}: {k} has an answer whose sha256 is not that of its canon() text, or a repeated answer')
            src = a.get('sources')
            end = f'/golden/{doc["goldenSha12"]}/{key[0]}/{key[1]}.golden.jsonl.gz'
            if not isinstance(src, list) or not src or not all(isinstance(s, str) and s.endswith(end) and s not in sources for s in src):
                raise ValueError(f'answer set {path}: {k} has an answer without sources, a repeated source, or a source that is not '
                                 f'a Go golden ...{end}')
            sources.update(src)
            texts.append(text)
        requests[key] = {'method': entry.get('method'), 'multiset': multiset, 'texts': texts, 'mask': mask,
                         'sources': sorted(sources)}
    found = {'path': path, 'sha256': want, 'kind': kind, 'pin': doc['pin'], 'oracleSha256': oracle, 'requests': requests}
    if set_mask is not None:
        if kind == 'lsp':
            found['maskModules'] = modules
        same = doc['maskTool'] == mask_tool(kind)
        found.update(mask=set_mask, maskTool={'same': same, 'rechecked': 0 if same else recheck_masked(path, found)})
    cache[(path, want)] = found
    return found


def recheck_masked(path, s):
    """The mask code changed since the set was made: masks every source golden of every masked entry of the set s
    again with this checkout's tools. Reads each golden once. Returns the number of (request, source golden) pairs;
    raises ValueError when a golden is missing or does not give the entry's answer."""
    t = tool_at('api', s['pin']) if s['kind'] == 'api' else tool('lsp')
    by_source = collections.defaultdict(list)
    for key, entry in sorted(s['requests'].items()):
        if entry['mask']:
            for src in entry['sources']:
                by_source[src].append((key, entry))
    count = 0
    for src, entries in sorted(by_source.items()):
        full = src if os.path.isabs(src) else os.path.join(REPO, src)
        try:
            with gzip.open(full, 'rt', encoding='utf-8') as f:
                lines = [json.loads(line) for line in f if line.strip()]
        except (OSError, ValueError) as e:
            raise ValueError(f'answer set {path}: the mask tool changed and the source golden {src} cannot be read: {e}')
        records = ({str(k): r for k, r in t.records_by_event(lines[1:]).items()} if s['kind'] == 'api'
                   else {str(r['i']): r for r in lines[1:] if 'i' in r})
        del lines
        for key, entry in entries:
            rec = records.get(key[2]) or {}
            resp = rec.get('response') or {}
            text = (t.canon(masker(s, entry)(t.apply_multisets(resp['result'], entry['multiset'])))
                    if rec.get('status') == 'ok' and 'result' in resp else None)
            if text != entry['texts'][0]:
                raise ValueError(f'answer set {path}: the mask tool changed, and {keytext(key)} in the source golden {src} does '
                                 f'not give the masked answer with this checkout\'s mask')
            count += 1
    return count


def answer_why(new_run, key, method, s):
    """Why goport's answer to a request fails its answer set s (the set's method and one of its answers), or None."""
    entry = s['requests'][key]
    if method != entry['method']:
        return f'the new request has the method {method}, the answer set {s["path"]} {entry["method"]}'
    got = new_run.answer(key, entry['multiset'], masker(s, entry))
    if got is None:
        return 'goport has no answer'
    if got not in entry['texts']:
        return f'goport\'s answer (sha256 {hashlib.sha256(got.encode()).hexdigest()[:12]}) is not in the answer set {s["path"]}'
    return None


def golden_error(run, key):
    """The error message of the Go golden record of an API request in the new run (see knownDiffRows), or None."""
    battery, trace, event = key
    oracle = run.oracle(battery, trace)
    path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(run.dir))), 'golden', (oracle or '')[:12], battery,
                        trace + '.golden.jsonl.gz')
    if not oracle or not os.path.isfile(path):
        return None
    with gzip.open(path, 'rt', encoding='utf-8') as f:
        lines = [json.loads(line) for line in f if line.strip()]
    rec = tool('api').records_by_event(lines[1:]).get(int(event) if event.isdigit() else event) or {}
    error = (rec.get('response') or {}).get('error')
    return error.get('message') if isinstance(error, dict) else None


def main():
    p = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    p.add_argument('dirs', nargs='+', metavar='DIR', help='the base results dirs, then the new results dir')
    p.add_argument('--answers', action='append', default=[], metavar='FILE@SHA256',
                   help='answer set (batch.oracleAnswers of the base batch and of the batch); repeatable')
    p.add_argument('--parity', action='store_true', help='check that the new run matches Go at its pin (batch.oracleRebase)')
    p.add_argument('--known-diff', action='append', default=[], metavar='KEY',
                   help='with --parity: an API diff key "<battery>/<trace>#<event>" (batch.oracleRebase.api.knownDiffs); repeatable')
    p.add_argument('--identity', action='store_true', help='add resultsSha256, goportSha256 and oracleSha256 to each head')
    p.add_argument('--kind', choices=('lsp', 'api'), help='results kind (default: from the results dirs)')
    p.add_argument('--out')
    a = p.parse_args()
    if len(a.dirs) < 2:
        p.error('give a base results dir and the new results dir')
    if len({os.path.realpath(d) for d in a.dirs}) != len(a.dirs):
        fail('a results dir is given twice')
    if a.known_diff and not a.parity:
        fail('--known-diff needs --parity')
    runs = [load(d) for d in a.dirs[:-1]]
    new, nhead, ninfo = load(a.dirs[-1])
    wired = sorted(f'manifest battery {b}' for b, rec in (side_file(a.dirs[-1], 'manifest.json').get('batteries') or {}).items()
                   if isinstance(rec, dict) and 'wire' in rec) + sorted(ninfo['wire'])
    if wired:
        fail(f'the new run {a.dirs[-1]} was made with --wire ({", ".join(wired[:5])}); only a base re-measure may use it')
    # The union of the base runs: a request is protected when a base run has it protected (see the docstring).
    base = dict(runs[0][0])
    for requests, _, _ in runs[1:]:
        for key, (cls, method) in requests.items():
            old = base.get(key)
            if old is None or (old[0] not in PROTECTED and (cls in PROTECTED or (cls == 'flaky_oracle' and old[0] != 'flaky_oracle'))):
                base[key] = (cls, method)
    heads = [r[1] for r in runs]
    kind = None
    if a.answers or a.parity or a.identity:
        found = {kind_of(d, info) for d, (_, _, info) in zip(a.dirs, runs + [(new, nhead, ninfo)])} - {None}
        if len(found) > 1 or (a.kind and found and found != {a.kind}):
            fail(f'the results dirs are {"/".join(sorted(found))} results{", not " + a.kind if a.kind else ""}')
        kind = a.kind or (found.pop() if found else fail('cannot tell LSP from API results; pass --kind'))
    if a.identity:
        for d, (_, head, info) in zip(a.dirs, runs + [(new, nhead, ninfo)]):
            head.update(identity(d, kind, info))
    new_run = Run(a.dirs[-1], kind, ninfo) if kind else None

    # The answer sets: key -> {oracle: set}. Each is of the results kind, and a key is in one set per oracle.
    answer_sets, holder, cache = [], {}, {}
    for ref in a.answers:
        try:
            s = load_answers(ref, cache)
        except ValueError as e:
            fail(str(e))
        if s['kind'] != kind:
            fail(f'answer set {s["path"]} is of kind {s["kind"]}, the results are {kind}')
        if any(x is s for x in answer_sets):
            continue
        for key in s['requests']:
            by = holder.setdefault(key, {})
            if s['oracleSha256'] in by:
                fail(f'{keytext(key)} is in the answer sets {by[s["oracleSha256"]]["path"]} and {s["path"]} of one oracle')
            by[s['oracleSha256']] = s
        answer_sets.append(s)
    stats = {id(s): collections.Counter() for s in answer_sets}

    def set_at(key):
        """The answer set that holds a key at the oracle of the new run's trace, or None."""
        by = holder.get(key)
        return by.get(new_run.oracle(key[0], key[1])) if by else None

    masked = any(e['mask'] for s in answer_sets for e in s['requests'].values())
    fields = FIELDS + (ANSWER_FIELDS if a.answers else ()) + (('retainedByMaskedAnswers',) if masked else ())
    total, per = collections.Counter(), collections.defaultdict(collections.Counter)
    lost = []
    for key, (cls, method) in base.items():
        now = new.get(key)
        s = set_at(key) if cls == 'flaky_oracle' else None
        carried = holder.get(key) if cls == 'flaky_oracle' and not s else None
        why = None
        if s:
            # A flake request at its set's pin: the new request keeps the set's method and goport's answer stays in the set.
            stats[id(s)]['applied'] += 1
            if now is None:
                field = 'absent'
            elif now[0] in UNRUN:
                field = 'unrun'
            else:
                why = answer_why(new_run, key, now[1], s)
                field = 'lost' if why else 'retainedByMaskedAnswers' if s['requests'][key]['mask'] else 'retainedByAnswers'
        elif carried:
            # A set of another pin only: protected like a same request (a later pin bump).
            for c in carried.values():
                stats[id(c)]['notApplied'] += 1
            field = 'absent' if now is None else 'retained' if now[0] in PROTECTED else 'unrun' if now[0] in UNRUN else 'lost'
        elif cls not in PROTECTED:
            field = 'recovered' if now and now[0] in PROTECTED else None
        elif now is None:
            field = 'absent'
        elif now[0] in PROTECTED:
            field = 'retained'
        else:
            field = 'unrun' if now[0] in UNRUN else 'lost'
        if field:
            total[field] += 1
            per[key[0]][field] += 1
        if field in ('lost', 'unrun', 'absent'):
            row = {'battery': key[0], 'trace': key[1], 'event': key[2], 'method': method, 'base': cls,
                   'new': now[0] if now else 'absent'}
            if s or carried:
                row['answers'] = s['path'] if s else sorted(c['path'] for c in carried.values())[0]
                if carried:
                    row['carried'] = True
                if why:
                    row['answersWhy'] = why
            lost.append(row)
    for key in new.keys() - base.keys():
        total['newRequests'] += 1
        per[key[0]]['newRequests'] += 1
    out = {'base': heads[0], 'new': nhead, 'protectedClasses': list(PROTECTED),
           'total': {k: total[k] for k in fields},
           'batteries': {b: {k: c[k] for k in fields} for b, c in sorted(per.items())},
           'lostFirst': lost[:50]}
    if len(heads) > 1:
        out['bases'] = heads
    if a.answers:
        out.update(kind=kind, answers=[{**{k: s[k] for k in ('path', 'sha256', 'kind', 'pin', 'oracleSha256')},
                                        'requests': len(s['requests']), 'applied': stats[id(s)]['applied'],
                                        'notApplied': stats[id(s)]['notApplied'],
                                        **({'maskedRequests': sum(bool(e['mask']) for e in s['requests'].values()),
                                            **{k: s[k] for k in ('mask', 'maskTool') if k in s}}
                                           if masked else {})} for s in answer_sets])
    bad = []
    if a.parity:
        if a.known_diff and kind != 'api':
            fail('--known-diff is for API results only (LSP parity has no known diffs)')
        known = {}
        for text in a.known_diff:
            key = parse_key(text)
            if not key or key in known:
                fail(f'--known-diff {text!r} is not "<battery>/<trace>#<event>", or it is given twice')
            known[key] = False
        classes, checked, allowed = collections.Counter(), 0, 0
        row = lambda key, method, cls, why: {'battery': key[0], 'trace': key[1], 'event': key[2], 'method': method, 'class': cls, 'why': why}
        for key, (cls, method) in new.items():
            classes[cls] += 1
            s = set_at(key)
            if s:
                checked += 1
                why = answer_why(new_run, key, method, s)
                if why:
                    bad.append(row(key, method, cls, why))
                elif cls in PARITY_BAD[kind] + PARITY_DIFF[kind]:
                    allowed += 1
            elif cls in PARITY_BAD[kind]:
                bad.append(row(key, method, cls, f'{cls} in the new run'))
            elif cls in PARITY_DIFF[kind]:
                if key in known:
                    known[key] = True
                else:
                    bad.append(row(key, method, cls, f'{cls} in the new run, not a known diff'))
        for key in holder.keys() - new.keys():
            s = set_at(key)
            if s:
                bad.append(row(key, s['requests'][key]['method'], 'absent', f'in the answer set {s["path"]}, not in the new run'))
        for key, used in known.items():
            if not used:
                cls, method = new.get(key) or ('absent', None)
                bad.append(row(key, method, cls, 'known diff is not a diff in the new run'))
        known_rows = []
        for key in known:
            cls, method = new.get(key) or ('absent', None)
            pointer, sub = ninfo['detail'].get(key, (None, None))
            error = golden_error(new_run, key) if cls.startswith('oracle_error') else None
            known_rows.append({'key': keytext(key), 'class': cls, 'method': method, 'pointer': pointer, 'sub': sub,
                               'goldenError': error})
            print(f'oracle-compare.py: known diff {keytext(key)}: {cls} {method} pointer {pointer}'
                  + (f' sub {sub}' if sub is not None else '') + (f'; Go golden error: {error}' if error else ''), file=sys.stderr)
        exits = 0
        if kind == 'lsp':
            exits = sum((b or {}).get('crashExits') or 0 for b in (side_file(a.dirs[-1], 'summary.json').get('batteries') or {}).values())
            if exits:
                bad.append(row(('*', '*', '*'), None, 'crash', f'{exits} crash exits (summary.json)'))
        out.update(kind=kind, parity={'classes': dict(sorted(classes.items())), 'crashExits': exits, 'answerRequests': checked,
                                      'allowedByAnswers': allowed, 'knownDiffs': len(known), 'knownDiffsUsed': sum(known.values()),
                                      'knownDiffRows': known_rows, 'bad': len(bad), 'badFirst': bad[:50]})
    text = json.dumps(out, indent=1)
    if a.out:
        with open(a.out, 'w') as f:
            f.write(text + '\n')
    print(text)
    sys.exit(1 if lost or bad else 0)


if __name__ == '__main__':
    main()
