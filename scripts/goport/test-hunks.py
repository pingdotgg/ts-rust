#!/usr/bin/env python3
"""test-hunks.py: the test code that a branch changes, for an expectation review.

usage: test-hunks.py [--base REV] [--head REV] [--all] [CHECKOUT]
       test-hunks.py help

Compares the .rs files under crates/ of CHECKOUT (default: the current directory) between the merge base of
BASE (default origin/main) and HEAD (default HEAD), and HEAD. For each file with a test change it prints:

  new tests        test fns (#[test]) that HEAD adds
  removed tests    test fns that the base has and HEAD does not
  changed tests    test fns of both whose text differs, with their removed and added line counts
  changed helpers  other test code (a mock, a helper fn) that HEAD changes, by enclosing item
  removed lines    each removed or changed line of base test code, with its enclosing fn (as int95's tool did)

Test code is a file under a tests/ dir, a file that a `#[cfg(test)] mod NAME;` declares, and the item after a
#[cfg(test)], #[cfg(all(test, ...))] or #[test] attribute (a mod, a fn, an impl, a use), to its closing brace
or semicolon. A test fn is a #[test] fn or the fn of a test macro call (`child_test! { fn name() {...} }`). Comments, strings and char literals do not count toward the braces. Production lines of a hunk
are counted per file and not listed; with --all, files with only production changes are listed too.
A test fn is keyed by its name and its place among fns of that name in the file.
Exit 0; 2 on bad arguments.
"""
import re
import subprocess
import sys

ATTR = re.compile(r"#\[\s*(?:cfg\s*\(\s*(?:test\s*\)|all\s*\((?:[^]]*?[(,]\s*)?test\s*[,)])|(?:[\w:]+::)?test\s*\])")
TEST_FN_ATTR = re.compile(r"#\[\s*(?:[\w:]+::)?test\s*\]")
FN = re.compile(r"\bfn\s+(\w+)")
ITEM_NAME = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|unsafe|extern)\s+)*"
    r"(fn|mod|impl|struct|enum|trait|const|static|type|macro_rules!)\b\s*([\w<>:, &']*)"
)
# Lines that only hold structure (blank, an attribute, a mod header, a closing brace): not counted as helpers.
STRUCTURE = re.compile(r"^\s*(?:#\[.*\]|(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{|\}[;,)]*)?\s*$")
# A macro call that declares a test (Rust tests/go_baselines/project_lsp/mod.rs child_test).
TEST_MACRO = re.compile(r"(?m)^\s*\w*test!\s*\{")
# A #[cfg(test)] mod with a body: its items are the top test items.
MOD_ITEM = re.compile(r"(?:#\[[^\]]*\]\s*)+(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")
MOD_DECL = re.compile(
    r"#\[\s*cfg\s*\(\s*(?:test\s*\)|all\s*\([^]]*\btest\b[^]]*)\]\s*"
    r"(?:#\[\s*path\s*=\s*\"([^\"]+)\"\s*\]\s*)?(?:#\[[^]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;"
)


def git(cwd, *args):
    return subprocess.run(["git", "-C", cwd, *args], capture_output=True, text=True, check=True).stdout


def show(cwd, rev, path):
    try:
        return git(cwd, "show", f"{rev}:{path}")
    except subprocess.CalledProcessError:
        return None


def mask(text):
    """text with comments and the contents of string and char literals as spaces; newlines stay."""
    out = []
    i, n = 0, len(text)
    ident = lambda c: c.isalnum() or c == "_"
    while i < n:
        c = text[i]
        if c == "/" and text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
        elif c == "/" and text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append(re.sub(r"[^\n]", " ", text[i:j]))
            i = j
        elif c in "rb" and (i == 0 or not ident(text[i - 1])) and re.match(r"(?:br|r)#*\"", text[i:]):
            m = re.match(r"(?:br|r)(#*)\"", text[i:])
            close = '"' + m.group(1)
            j = text.find(close, i + m.end())
            j = n if j < 0 else j + len(close)
            out.append('"' + re.sub(r"[^\n]", " ", text[i + 1 : j - 1]) + '"')
            i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            j = min(j + 1, n)
            out.append('"' + re.sub(r"[^\n]", " ", text[i + 1 : j - 1]) + '"')
            i = j
        elif c == "'":
            # A char literal ('x', '\n', '\u{1F}'); else a lifetime or a label.
            m = re.match(r"'(?:\\(?:u\{[0-9a-fA-F]*\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'", text[i:])
            if m:
                out.append("'" + " " * (m.end() - 2) + "'")
                i += m.end()
            else:
                out.append(c)
                i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)


def item_end(code, start):
    """Offset after the item that starts at `start` (its attributes first): at the `;` or `}` that closes it."""
    i, n, depth = start, len(code), 0
    while i < n:
        c = code[i]
        if c == "#" and depth == 0 and code.startswith("[", i + 1):
            # An attribute: skip its brackets.
            d, j = 0, i + 1
            while j < n:
                d += code[j] == "["
                d -= code[j] == "]"
                j += 1
                if d == 0:
                    break
            i = j
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0 and c == "}":
                return i + 1
        elif c == ";" and depth == 0:
            return i + 1
        i += 1
    return n


class Rust:
    """The test regions and the test fns of one file."""

    def __init__(self, text, whole_file_is_test):
        self.lines = text.split("\n")
        code = mask(text)
        starts = [0]
        for k, ch in enumerate(code):
            if ch == "\n":
                starts.append(k + 1)
        line_of = lambda off: next_line(starts, off)
        code_lines = code.split("\n")
        # The brace depth at the start of each line.
        depth, d = [], 0
        for line in code_lines:
            depth.append(d)
            d += line.count("{") - line.count("}")
        self.test = [whole_file_is_test] * len(self.lines)
        # The depth of the top items of the test code of each line: 0 in a test file, one more than a
        # #[cfg(test)] mod, the item itself for another #[cfg(test)] item.
        base = [0 if whole_file_is_test else None] * len(self.lines)
        self.fns = {}  # key -> (first line, last line), 0-based
        seen = {}
        for m in re.finditer(r"#\[", code):
            attr = ATTR.match(code, m.start())
            if not attr:
                continue
            end = item_end(code, m.start())
            a, b = line_of(m.start()), line_of(end - 1)
            is_mod = MOD_ITEM.match(code, m.start(), end) is not None
            for k in range(a, b + 1):
                self.test[k] = True
                if base[k] is None:
                    base[k] = depth[a] + (1 if is_mod else 0)
            if TEST_FN_ATTR.match(code, m.start()):
                fn = FN.search(code, m.start(), end)
                if fn:
                    # Comment lines just above the attribute belong to the test.
                    while a > 0 and code_lines[a - 1].strip() == "" and self.lines[a - 1].strip().startswith("//"):
                        a -= 1
                    name = fn.group(1)
                    seen[name] = seen.get(name, 0) + 1
                    key = name if seen[name] == 1 else f"{name}#{seen[name]}"
                    self.fns.setdefault(key, (a, b))
        # A test macro (`child_test! { fn name() { ... } }`, `os_child_test!`) declares one test fn.
        for m in TEST_MACRO.finditer(code):
            end = item_end(code, m.end() - 1)
            fn = FN.search(code, m.end(), end)
            if fn:
                a, b = line_of(m.start()), line_of(end - 1)
                for k in range(a, b + 1):
                    self.test[k] = True
                    if base[k] is None:
                        base[k] = depth[a]
                name = fn.group(1)
                seen[name] = seen.get(name, 0) + 1
                key = name if seen[name] == 1 else f"{name}#{seen[name]}"
                self.fns.setdefault(key, (a, b))
        # The innermost item of each line: the last item keyword at or above it, for removed lines.
        self.label, cur = [], None
        for line in code_lines:
            m = ITEM_NAME.match(line)
            if m:
                cur = f"{m.group(1)} {m.group(2).strip()}".strip()
            self.label.append(cur)
        # The top test item of each test line: the last item keyword at its base depth, for helpers.
        self.top, last = [], {}
        for k, line in enumerate(code_lines):
            m = ITEM_NAME.match(line)
            if m:
                last[depth[k]] = f"{m.group(1)} {m.group(2).strip()}".strip()
                for deeper in [x for x in last if x > depth[k]]:
                    del last[deeper]
            if base[k] is None:
                self.top.append(None)
            else:
                # Before the first top item: the enclosing #[cfg(test)] mod, or the file top.
                self.top.append(last.get(base[k]) or last.get(base[k] - 1) or "(file top)")

    def fn_text(self, key):
        a, b = self.fns[key]
        return "\n".join(self.lines[a : b + 1])

    def fn_at(self, k):
        return next((key for key, (a, b) in self.fns.items() if a <= k <= b), None)


def next_line(starts, off):
    lo, hi = 0, len(starts) - 1
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if starts[mid] <= off:
            lo = mid
        else:
            hi = mid - 1
    return lo


def test_module_files(cwd, rev):
    """Paths that a `#[cfg(test)] mod NAME;` declares at REV (path attributes too)."""
    out = set()
    hits = git(cwd, "grep", "-l", "-E", r"cfg\((all\(.*)?test", rev, "--", "crates/*.rs")
    for hit in hits.split():
        path = hit.split(":", 1)[1]
        text = show(cwd, rev, path) or ""
        directory = path.rsplit("/", 1)[0]
        stem = path.rsplit("/", 1)[1][:-3]
        # A mod.rs, lib.rs, main.rs or *_root.rs declares children in its own dir; other files in dir/stem.
        own = stem in ("mod", "lib", "main") or stem.endswith("_root")
        for m in MOD_DECL.finditer(mask(text)):
            # mask() blanks the path string: read it from the text at the same offsets.
            raw = MOD_DECL.match(text, m.start())
            if raw and raw.group(1):
                out.add(f"{directory}/{raw.group(1)}")
                continue
            base = directory if own else f"{directory}/{stem}"
            out.add(f"{base}/{m.group(2)}.rs")
            out.add(f"{base}/{m.group(2)}/mod.rs")
            # A flat layout (scanner_util.rs declares scanner_test next to it).
            out.add(f"{directory}/{m.group(2)}.rs")
    return out


def main(argv):
    if argv[:1] in (["help"], ["-h"], ["--help"]):
        print(__doc__.strip())
        return 0
    base, head, show_all, cwd = "origin/main", "HEAD", False, "."
    args = list(argv)
    while args:
        a = args.pop(0)
        if a in ("--base", "--head") and args:
            if a == "--base":
                base = args.pop(0)
            else:
                head = args.pop(0)
        elif a == "--all":
            show_all = True
        elif a.startswith("-"):
            print(f"test-hunks.py: unknown option {a} (see test-hunks.py help)", file=sys.stderr)
            return 2
        else:
            cwd = a
    merge_base = git(cwd, "merge-base", base, head).strip()
    files = [
        f
        for f in git(cwd, "diff", "--name-only", merge_base, head, "--", "crates").split()
        if f.endswith(".rs")
    ]
    test_mods = test_module_files(cwd, head) | test_module_files(cwd, merge_base)
    print(f"test-hunks: {merge_base[:12]} (merge base of {base}) .. {head}, {len(files)} changed .rs files")
    for f in files:
        whole = "/tests/" in f or f in test_mods
        old_text, new_text = show(cwd, merge_base, f), show(cwd, head, f)
        old = Rust(old_text or "", whole)
        new = Rust(new_text or "", whole)
        removed, added = [], []
        for line in git(cwd, "diff", "-U0", merge_base, head, "--", f).split("\n"):
            m = re.match(r"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@", line)
            if m:
                o, oc = int(m.group(1)), int(m.group(2) or 1)
                s, sc = int(m.group(3)), int(m.group(4) or 1)
                removed += range(o - 1, o - 1 + oc) if oc else []
                added += range(s - 1, s - 1 + sc) if sc else []
        test_removed = [k for k in removed if k < len(old.test) and old.test[k]]
        test_added = [k for k in added if k < len(new.test) and new.test[k]]
        prod = (len(removed) - len(test_removed), len(added) - len(test_added))
        new_tests = [k for k in new.fns if k not in old.fns]
        gone_tests = [k for k in old.fns if k not in new.fns]
        changed = []
        for k in old.fns:
            if k in new.fns and old.fn_text(k) != new.fn_text(k):
                (oa, ob), (na, nb) = old.fns[k], new.fns[k]
                r = sum(oa <= x <= ob for x in removed)
                a = sum(na <= x <= nb for x in added)
                changed.append(f"{k} (-{r} +{a})")
        helpers = {}
        for k in test_removed:
            if old.fn_at(k) is None and not STRUCTURE.match(old.lines[k]):
                helpers.setdefault(old.top[k], [0, 0])[0] += 1
        for k in test_added:
            if new.fn_at(k) is None and not STRUCTURE.match(new.lines[k]):
                helpers.setdefault(new.top[k], [0, 0])[1] += 1
        if not (test_removed or test_added or new_tests or gone_tests) and not show_all:
            continue
        kind = "test file" if whole else "src file"
        print(f"== {f} ({kind}): test lines -{len(test_removed)} +{len(test_added)}, production lines -{prod[0]} +{prod[1]}")
        if new_tests:
            print(f"  new tests ({len(new_tests)}): {', '.join(new_tests)}")
        if gone_tests:
            print(f"  removed tests ({len(gone_tests)}): {', '.join(gone_tests)}")
        if changed:
            print(f"  changed tests ({len(changed)}): {', '.join(changed)}")
        if helpers:
            print("  changed helpers: " + ", ".join(f"{h or '?'} (-{r} +{a})" for h, (r, a) in helpers.items()))
        for k in test_removed:
            print(f"  -{k + 1} [{old.fn_at(k) or old.label[k]}] {old.lines[k].strip()[:150]}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
