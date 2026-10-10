#!/usr/bin/env python3
"""Writes a masked API or LSP answer set for oracle-compare.py --answers (bump C reviewer ruling 1 item 3, ruling 2
item 1; LSP: bump D).

usage: masked-answers.py --keys TSV --pin PIN --golden DIR... --out FILE.json.gz [--kind api|lsp] [--note TEXT]

API (the default):

--keys: lines "api TAB <battery>/<trace>#<event> ..." (for example upstream/bumpC/rebaseN/answers-excluded.tsv; #
comments). --golden: the Go golden roots of the pin's oracle, each .../golden/<oracle sha256 prefix> (the pin's
golden and every re-record). For each key, every golden root gives the Go answer of that event, normalized as the
tools do (apply_multisets with the key's multiset patterns from the flaky file of the first golden root), then
masked with oracle-compare.py mask_type_ids() (the "type-ids" mask) and the API tool loaded at the pin. All runs
must give one masked answer, or the key fails and nothing is written. The entry is {method, multiset, mask,
answers: [{answer, sha256, sources}]} with every Go golden as a source (relative to the repo root, sorted). The
header is goport-oracle-answers/1 with kind api, pin, oracleSha256 (every golden's oracleSha), goldenSha12, mask,
maskTool (oracle-compare.py mask_tool()) and note. Prints per key the method, the run count and the masked
sha256, then the file sha256.

LSP (--kind lsp, the "auto-import-modules" mask of oracle-compare.py): --keys lines "lsp TAB <battery>/<trace>#<i>",
each a textDocument/completion request. The first --golden root is the pin's LSP golden; it must have an ok answer
for every key. A later root (a re-record, maybe of some traces only) is a source of each key whose trace it has.
The answers are normalized as lsp_oracle.py does (apply_multisets with the patterns of the first root's flaky
file). The tool reads one golden at a time, in two passes:
1. maskModules, from every textDocument/completion request of the keys' batteries in all roots: an auto-import
   (name, module) pair (oracle-compare.py auto_import_module()) varies when some but not all Go runs of one request
   give it. maskModules lists each name of a varying pair with the modules of its varying pairs. A module of the
   name that never varies is not listed, so its items stay exact.
2. Each key's Go answers, masked with mask_auto_import_modules(), must give one masked answer, or nothing is
   written.
The header has mask "auto-import-modules", maskTool (oracle-compare.py mask_tool("lsp")) and maskModules. Prints
maskModules, per key the run count and the masked sha256, then the file sha256.
"""
import argparse, collections, glob, gzip, hashlib, importlib.util, json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location('oracle_compare', os.path.join(HERE, 'oracle-compare.py'))
OC = importlib.util.module_from_spec(spec)
spec.loader.exec_module(OC)


def main():
    p = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    p.add_argument('--keys', required=True)
    p.add_argument('--pin', required=True)
    p.add_argument('--golden', nargs='+', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--kind', choices=('api', 'lsp'), default='api')
    p.add_argument('--note', default='')
    a = p.parse_args()
    if a.kind == 'lsp':
        return main_lsp(a)
    api = OC.tool_at('api', a.pin)
    keys = [line.split('\t')[1].strip() for line in open(a.keys, encoding='utf-8')
            if line.strip() and not line.startswith('#') and line.split('\t')[0] == 'api']
    if not keys or len(keys) != len(set(keys)) or not all(OC.parse_key(k) for k in keys):
        sys.exit(f'masked-answers.py: {a.keys} needs distinct api keys <battery>/<trace>#<event>')
    roots = [os.path.realpath(g) for g in a.golden]
    sha12 = {os.path.basename(r) for r in roots}
    if len(sha12) != 1 or len(roots) != len(set(roots)):
        sys.exit(f'masked-answers.py: the golden roots must be distinct dirs of one oracle, not {sorted(sha12)}')
    sha12, oracles, out, bad = sha12.pop(), set(), {}, []
    for key in keys:
        battery, trace, event = OC.parse_key(key)
        flaky_file = os.path.join(roots[0], battery, trace + '.flaky.json')
        flaky = (json.load(open(flaky_file)).get('events') or {}).get(event) or {} if os.path.isfile(flaky_file) else {}
        patterns, masked, method = flaky.get('multiset') or [], {}, None
        for root in roots:
            path = os.path.join(root, battery, trace + '.golden.jsonl.gz')
            if not os.path.isfile(path):
                bad.append(f'{key}: no golden {path}')
                continue
            with gzip.open(path, 'rt', encoding='utf-8') as f:
                lines = [json.loads(line) for line in f if line.strip()]
            oracles.add(lines[0].get('oracleSha'))
            rec = {str(k): r for k, r in api.records_by_event(lines[1:]).items()}.get(event) or {}
            if rec.get('status') != 'ok' or 'result' not in (rec.get('response') or {}):
                bad.append(f'{key}: {path} has no ok answer')
                continue
            method = method or rec.get('method')
            v = OC.mask_type_ids(api, method, api.apply_multisets(rec['response']['result'], patterns))
            text = api.canon(v)
            masked.setdefault(hashlib.sha256(text.encode('utf-8')).hexdigest(), (v, []))[1].append(os.path.relpath(path, api.REPO))
        if len(masked) != 1:
            bad.append(f'{key}: {len(masked)} masked answers in {len(roots)} Go runs')
            continue
        (sha, (answer, sources)), = masked.items()
        out[key] = {'method': method, 'multiset': patterns, 'mask': OC.MASK,
                    'answers': [{'answer': answer, 'sha256': sha, 'sources': sorted(sources)}]}
        print(f'{key}\t{method}\t{len(sources)} Go runs\tmasked {sha[:16]}')
    if len(oracles) != 1 or not str(next(iter(oracles))).startswith(sha12):
        bad.append(f'the goldens name the oracles {sorted(map(str, oracles))}, not one oracle {sha12}...')
    if bad:
        sys.exit('\n'.join(bad))
    doc = {'format': OC.ANSWERS_FORMAT, 'kind': 'api', 'pin': a.pin, 'oracleSha256': oracles.pop(), 'goldenSha12': sha12,
           'mask': OC.MASK, 'maskTool': OC.mask_tool(), 'note': a.note, 'requests': dict(sorted(out.items()))}
    data = gzip.compress(json.dumps(doc, sort_keys=True).encode('utf-8'), mtime=0)
    with open(a.out, 'wb') as f:
        f.write(data)
    print(f'{len(out)} masked keys, mask {OC.MASK}; {a.out} sha256 {hashlib.sha256(data).hexdigest()}')


def completion_results(path):
    """(header, {i: result}) of an LSP golden: the result of each ok textDocument/completion record."""
    out = {}
    with gzip.open(path, 'rt', encoding='utf-8') as f:
        header = json.loads(f.readline())
        for line in f:
            if not line.strip():
                continue
            r = json.loads(line)
            resp = r.get('response') or {}
            if 'i' in r and r.get('method') == OC.LSP_MASK_METHOD and r.get('status') == 'ok' and 'result' in resp:
                out[str(r['i'])] = resp['result']
    return header, out


def main_lsp(a):
    """--kind lsp. Reads one golden at a time, in two passes: maskModules from every completion request of the keys'
    batteries, then the masked Go answers of the keys."""
    lsp = OC.tool('lsp')
    keys = [line.split('\t')[1].strip() for line in open(a.keys, encoding='utf-8')
            if line.strip() and not line.startswith('#') and line.split('\t')[0] == 'lsp']
    if not keys or len(keys) != len(set(keys)) or not all(OC.parse_key(k) for k in keys):
        sys.exit(f'masked-answers.py: {a.keys} needs distinct lsp keys <battery>/<trace>#<i>')
    roots = [os.path.realpath(g) for g in a.golden]
    sha12 = {os.path.basename(r) for r in roots}
    if len(sha12) != 1 or len(roots) != len(set(roots)):
        sys.exit(f'masked-answers.py: the golden roots must be distinct dirs of one oracle, not {sorted(sha12)}')
    sha12, oracles, bad = sha12.pop(), set(), []

    def goldens(battery, trace):
        """(path, header, {i: result}) of each root that has the trace, the pin golden first."""
        for root in roots:
            path = os.path.join(root, battery, trace + '.golden.jsonl.gz')
            if os.path.isfile(path):
                header, results = completion_results(path)
                oracles.add((header.get('oracle') or {}).get('sha256'))
                yield path, header, results

    def patterns(battery, trace):
        """{i: multiset patterns} of the trace's flaky file in the pin golden."""
        path = os.path.join(roots[0], battery, trace + '.flaky.json')
        events = (json.load(open(path)).get('events') or {}) if os.path.isfile(path) else {}
        return {i: (e or {}).get('multiset') or [] for i, e in events.items()}

    # Pass 1, maskModules: the (name, module) pairs that some but not all Go runs of one request give.
    varying = collections.defaultdict(set)
    for battery in sorted({OC.parse_key(k)[0] for k in keys}):
        first = os.path.join(roots[0], battery)
        for path in sorted(glob.glob(os.path.join(first, '**', '*.golden.jsonl.gz'), recursive=True)):
            trace = os.path.relpath(path, first)[:-len('.golden.jsonl.gz')]
            union, common = {}, {}
            for _, _, results in goldens(battery, trace):
                for i, v in results.items():
                    pairs = {p for p in map(OC.auto_import_module, v.get('items') or []) if p} if isinstance(v, dict) else set()
                    union[i] = union.get(i, set()) | pairs
                    common[i] = common[i] & pairs if i in common else pairs
            for i in union:
                for name, module in union[i] - common[i]:
                    varying[name].add(module)
    modules = {name: sorted(varying[name]) for name in sorted(varying)}
    for name, ms in modules.items():
        print(f'maskModules\t{name}\t{" ".join(ms)}')
    # Pass 2: each key's Go answers after the mask must give one masked answer.
    by_trace = collections.defaultdict(list)
    for key in keys:
        battery, trace, i = OC.parse_key(key)
        by_trace[(battery, trace)].append((key, i))
    out = {}
    for (battery, trace), items in sorted(by_trace.items()):
        found = {key: {} for key, _ in items}  # key -> {masked sha256: [canon text, sources]}
        multisets, in_pin = patterns(battery, trace), set()
        for path, _, results in goldens(battery, trace):
            if path.startswith(roots[0] + os.sep):
                in_pin = set(results)
            for key, i in items:
                if i not in results:
                    continue
                text = lsp.canon(OC.mask_auto_import_modules(lsp.apply_multisets(results[i], multisets.get(i, [])), modules))
                sha = hashlib.sha256(text.encode('utf-8')).hexdigest()
                found[key].setdefault(sha, [text, []])[1].append(os.path.relpath(path, OC.REPO))
        for key, i in items:
            if i not in in_pin:
                bad.append(f'{key}: the pin golden {roots[0]} has no ok {OC.LSP_MASK_METHOD} answer')
                continue
            if len(found[key]) != 1:
                bad.append(f'{key}: {len(found[key])} masked answers in {sum(len(v[1]) for v in found[key].values())} Go runs')
                continue
            (sha, (text, sources)), = found[key].items()
            answer = json.loads(text)
            out[key] = {'method': OC.LSP_MASK_METHOD, 'multiset': multisets.get(i, []), 'mask': OC.MASKS['lsp'],
                        'answers': [{'answer': answer, 'sha256': sha, 'sources': sorted(sources)}]}
            print(f'{key}\t{len(sources)} Go runs\tmasked {sha[:16]}\tmaskedItems {len(answer.get("maskedItems") or [])}')
    if len(oracles) != 1 or not str(next(iter(oracles))).startswith(sha12):
        bad.append(f'the goldens name the oracles {sorted(map(str, oracles))}, not one oracle {sha12}...')
    if bad:
        sys.exit('\n'.join(bad))
    doc = {'format': OC.ANSWERS_FORMAT, 'kind': 'lsp', 'pin': a.pin, 'oracleSha256': oracles.pop(), 'goldenSha12': sha12,
           'mask': OC.MASKS['lsp'], 'maskTool': OC.mask_tool('lsp'), 'maskModules': modules, 'note': a.note,
           'requests': dict(sorted(out.items()))}
    data = gzip.compress(json.dumps(doc, sort_keys=True).encode('utf-8'), mtime=0)
    with open(a.out, 'wb') as f:
        f.write(data)
    print(f'{len(out)} masked keys, mask {OC.MASKS["lsp"]}, {len(modules)} names; {a.out} sha256 {hashlib.sha256(data).hexdigest()}')


if __name__ == '__main__':
    main()
