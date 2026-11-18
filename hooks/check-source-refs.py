#!/usr/bin/env python3
"""Verify FreeSWITCH source line references against the pinned commit.

Every `file.c:NNN` reference in a doc comment or markdown file indexes a
specific FreeSWITCH tree. This checks each one against the commit pinned in
hooks/source-refs.yaml by comparing a content hash of the referenced line or
range, so bumping the pin produces a diff naming exactly the references whose
target text moved.

Local only -- it needs a FreeSWITCH clone at $FREESWITCH_SOURCE containing the
pinned commit, which CI does not have. It also refuses to pass without a
sofia-sip clone at $SOFIA_SIP_SOURCE holding the commit pinned under
sofia_sip, because freeswitch-c-oracle compiles sofia's sdp_parse from that
commit's blobs and would otherwise skip its SDP properties. sofia-sip carries no
line references, so the block is only carried through --update.

The checkout is almost never parked on the pin, so reading a cited range out of
the working tree yields the wrong text and a bogus "this reference is stale"
conclusion. --show reads it from the pinned blob instead.

    hooks/check-source-refs.py                                  verify
    hooks/check-source-refs.py --update                         regenerate the index
    hooks/check-source-refs.py --show switch_core_media.c:5805  print the pinned text
"""

import argparse
import hashlib
import os
import re
import subprocess
import sys
from pathlib import Path

import yaml

INDEX = "hooks/source-refs.yaml"
HASH_LEN = 12

# Only comments carry references; a C file:line inside a Rust string literal is
# test data (a synthetic FreeSWITCH log line), not a citation.
RUST_COMMENT = re.compile(r"^\s*(///|//!|//)")

REF = re.compile(
    r"`?(?P<file>[A-Za-z0-9_]+\.[ch]):(?P<start>\d+)(?:-(?P<end>\d+))?"
    r"(?P<more>(?:,\s*\d+(?:-\d+)?)+)?"
)
# A bare `:NNN` continues the last file named earlier in the same document.
CONT = re.compile(r"`:(?P<start>\d+)(?:-(?P<end>\d+))?`")
COMMIT = re.compile(r"\b[0-9a-f]{40}\b")


def fail(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


class FsTree:
    """Reads blobs out of the FreeSWITCH repo at one pinned commit."""

    def __init__(self, repo: Path, commit: str):
        self.repo = repo
        self.commit = commit
        self._lines: dict[str, list[str]] = {}
        self._paths: dict[str, str] | None = None

    def _git(self, *args: str) -> str:
        return subprocess.run(
            ["git", "--git-dir", str(self.repo / ".git"), *args],
            check=True,
            capture_output=True,
            text=True,
        ).stdout

    def path_for(self, basename: str) -> str:
        if self._paths is None:
            self._paths = {}
            candidates: dict[str, list[str]] = {}
            for p in self._git("ls-tree", "-r", "--name-only", self.commit).splitlines():
                candidates.setdefault(p.rsplit("/", 1)[-1], []).append(p)
            for name, paths in candidates.items():
                # tests/unit/ shadows several core basenames
                real = [p for p in paths if not p.startswith("tests/")]
                if len(real) == 1:
                    self._paths[name] = real[0]
                elif real:
                    self._paths[name] = "\0".join(real)
        found = self._paths.get(basename)
        if found is None:
            fail(
                f"{basename} is not in FreeSWITCH {self.commit[:10]}. Only the "
                "FreeSWITCH tree is indexed -- sofia-sip and other unvendored "
                "dependencies are cited by symbol name, never by line."
            )
        if "\0" in found:
            fail(f"{basename} is ambiguous: {found.replace(chr(0), ', ')}")
        return found

    def lines(self, path: str) -> list[str]:
        if path not in self._lines:
            self._lines[path] = self._git("show", f"{self.commit}:{path}").split("\n")
        return self._lines[path]

    def digest(self, path: str, start: int, end: int) -> str | None:
        """None when the range runs past EOF -- the file shrank under the pin."""
        lines = self.lines(path)
        if end > len(lines):
            return None
        body = "\n".join(lines[start - 1 : end])
        return hashlib.sha256(body.encode()).hexdigest()[:HASH_LEN]


def scanned_files(repo: Path) -> list[Path]:
    out = subprocess.run(
        ["git", "ls-files", "-z", "*.md", "*.rs"],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [repo / p for p in out.split("\0") if p]


def extract(repo: Path) -> tuple[dict[tuple[str, int, int], set[str]], set[str]]:
    """Map (basename, start, end) -> citing repo files, plus every pinned commit."""
    refs: dict[tuple[str, int, int], set[str]] = {}
    commits: set[str] = set()
    for path in scanned_files(repo):
        rel = path.relative_to(repo).as_posix()
        is_rust = path.suffix == ".rs"
        last_file: str | None = None
        for lineno, line in enumerate(path.read_text().splitlines(), 1):
            commits.update(COMMIT.findall(line))
            if is_rust and not RUST_COMMENT.match(line):
                continue
            for m in _line_refs(line, last_file):
                if isinstance(m, str):
                    last_file = m
                    continue
                refs.setdefault(m, set()).add(f"{rel}:{lineno}")
    return refs, commits


def _line_refs(line: str, last_file: str | None):
    """Yield refs in source order, and the basename each time one is named."""
    events: list[tuple[int, object]] = []
    for m in REF.finditer(line):
        events.append((m.start(), ("file", m)))
    for m in CONT.finditer(line):
        events.append((m.start(), ("cont", m)))
    for _, (kind, m) in sorted(events, key=lambda e: e[0]):
        if kind == "file":
            name = m.group("file")
            yield name
            last_file = name
            start = int(m.group("start"))
            end = int(m.group("end") or start)
            yield (name, start, end)
            for extra in re.findall(r"(\d+)(?:-(\d+))?", m.group("more") or ""):
                s = int(extra[0])
                yield (name, s, int(extra[1] or s))
        elif last_file:
            start = int(m.group("start"))
            yield (last_file, start, int(m.group("end") or start))


def show(tree: FsTree, specs: list[str], tag: str) -> int:
    """Print the pinned text a reference resolves to, so it can be verified."""
    for spec in specs:
        m = REF.fullmatch(spec.strip("`"))
        if not m:
            fail(f"{spec!r} is not a file.c:NNN or file.c:NNN-MMM reference")
        start = int(m.group("start"))
        end = int(m.group("end") or start)
        path = tree.path_for(m.group("file"))
        lines = tree.lines(path)
        if end > len(lines):
            fail(f"{path}:{start}-{end} runs past EOF at {tree.commit[:10]}")
        span = f"{start}" if start == end else f"{start}-{end}"
        print(f"=== {path}:{span} @ {tag}")
        for n in range(start, end + 1):
            print(f"{n:>6}  {lines[n - 1]}")
    return 0


def ref_key(basename: str, start: int, end: int, tree: FsTree) -> str:
    path = tree.path_for(basename)
    return f"{path}:{start}" if start == end else f"{path}:{start}-{end}"


def load_index(repo: Path) -> tuple[str, str, str | None, dict[str, str]]:
    path = repo / INDEX
    if not path.exists():
        fail(
            f"{INDEX} is missing; seed it with --update --commit SHA --tag TAG --block-parse REVISION"
        )
    doc = yaml.safe_load(path.read_text())
    entries = {}
    for entry in doc["refs"]:
        ref, digest = entry.rsplit(" ", 1)
        entries[ref] = digest
    pin = doc["freeswitch"]
    return pin["commit"], pin["tag"], pin.get("block_parse"), entries


def load_trees(repo: Path) -> list[dict[str, str]]:
    """The other trees freeswitch-c-oracle compiles, which --update carries over verbatim."""
    path = repo / INDEX
    if not path.exists():
        return []
    return yaml.safe_load(path.read_text()).get("trees") or []


def load_sofia(repo: Path) -> dict[str, str] | None:
    """The sofia-sip pin freeswitch-c-oracle compiles, which --update carries over verbatim."""
    path = repo / INDEX
    if not path.exists():
        return None
    return yaml.safe_load(path.read_text()).get("sofia_sip")


def write_index(
    repo: Path,
    commit: str,
    tag: str,
    block_parse: str | None,
    trees: list[dict[str, str]],
    sofia: dict[str, str] | None,
    entries: dict[str, str],
) -> None:
    def order(ref: str) -> tuple[str, int]:
        path, lines = ref.rsplit(":", 1)
        return path, int(lines.split("-")[0])

    unrevised = [tree["name"] for tree in trees if not tree.get("block_parse")]
    if not block_parse:
        unrevised.insert(0, "pin")
    if unrevised:
        fail(f"no block_parse for tree {', '.join(unrevised)} in {INDEX}")
    tree_lines = "".join(
        f"- name: {tree['name']}\n  commit: {tree['commit']}\n"
        f"  block_parse: {tree['block_parse']}\n"
        + (f"  fetch: {tree['fetch']}\n" if tree.get("fetch") else "")
        for tree in trees
    )
    sofia_lines = (
        f"sofia_sip:\n  commit: {sofia['commit']}\n  tag: {sofia['tag']}\n"
        f"  fetch: {sofia['fetch']}\n"
        if sofia
        else ""
    )
    body = "\n".join(f"- {ref} {entries[ref]}" for ref in sorted(entries, key=order))
    # sofia_sip follows freeswitch: CI's fetch step takes the first commit: line as the pin.
    (repo / INDEX).write_text(
        "# Generated by hooks/check-source-refs.py --update. Do not edit by hand,\n"
        "# except to bump a commit below and then regenerate.\n"
        f"freeswitch:\n  commit: {commit}\n  tag: {tag}\n  block_parse: {block_parse}\n"
        + sofia_lines
        + (f"trees:\n{tree_lines}" if trees else "")
        + f"refs:\n{body}\n"
    )


def require_clone(var: str, project: str, commit: str) -> Path:
    """The clone the environment variable names, refusing one that lacks the pinned commit."""
    source = os.environ.get(var)
    if not source:
        fail(f"{var} is unset; export the path of a {project} clone containing the pinned commit")
    repo = Path(source)
    if not (repo / ".git").exists():
        fail(f"{var}={source} is not a git repository")
    have = subprocess.run(
        ["git", "--git-dir", str(repo / ".git"), "cat-file", "-e", f"{commit}^{{commit}}"],
        capture_output=True,
    )
    if have.returncode != 0:
        fail(f"{var}={source} has no commit {commit[:10]}; run: git fetch --tags")
    return repo


def open_tree(commit: str) -> FsTree:
    return FsTree(require_clone("FREESWITCH_SOURCE", "FreeSWITCH", commit), commit)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--update", action="store_true", help="regenerate the index")
    ap.add_argument("--quiet", action="store_true", help="print only on failure")
    ap.add_argument("--commit", help="pin to seed a missing index with")
    ap.add_argument("--tag", help="tag naming --commit")
    ap.add_argument("--block-parse", help="BlockParse revision --commit runs, in its string form")
    ap.add_argument(
        "--list",
        action="store_true",
        help="print every citation and the reference it resolves to, for review",
    )
    ap.add_argument(
        "--show",
        nargs="+",
        metavar="REF",
        help="print the pinned text of file.c:NNN[-MMM]; the working tree is not the pin",
    )
    args = ap.parse_args()

    # The script's physical path lands in the main checkout even when the hook
    # runs for a worktree. Inherit the hook's cwd: git sets it to the worktree
    # top, and with GIT_DIR set rev-parse would otherwise echo any cwd override.
    repo = Path(
        subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    )
    if args.commit and not (repo / INDEX).exists():
        commit, tag, block_parse, indexed = args.commit, args.tag, args.block_parse, {}
    else:
        commit, tag, block_parse, indexed = load_index(repo)
    tree = open_tree(commit)
    if args.show:
        return show(tree, args.show, tag)

    sofia = load_sofia(repo)
    if sofia:
        require_clone("SOFIA_SIP_SOURCE", "sofia-sip", sofia["commit"])
    elif (repo / INDEX).exists():
        fail(f"{INDEX} has no sofia_sip pin; add commit, tag and fetch under sofia_sip")

    refs, commits = extract(repo)
    seen = {ref_key(*k, tree): v for k, v in refs.items()}

    if args.list:
        for ref, cites in sorted(seen.items(), key=lambda kv: sorted(kv[1])):
            for cite in sorted(cites):
                print(f"{cite}\t{ref}")
        return 0

    digests = {ref: tree.digest(*_split(ref)) for ref in seen}
    truncated = sorted(ref for ref, d in digests.items() if d is None)

    if args.update:
        if truncated:
            for ref in truncated:
                print(f"  {ref} runs past EOF at {commit[:10]}", file=sys.stderr)
            print(
                "❌ Re-verify those references against the new pin before indexing.",
                file=sys.stderr,
            )
            return 1
        write_index(repo, commit, tag, block_parse, load_trees(repo), sofia, digests)
        print(f"SourceRefs {len(digests)} refs written to {INDEX} @ {commit[:10]}")
        return 0

    problems: list[str] = []
    for ref in truncated:
        problems.append(
            f"{ref} runs past EOF at {commit[:10]} "
            f"(cited by {', '.join(sorted(seen[ref]))})"
        )
    for other in sorted(commits - {commit}):
        problems.append(f"pin {other[:10]} disagrees with the index ({commit[:10]})")
    for ref in sorted(set(indexed) - set(digests)):
        problems.append(f"{ref} is indexed but no longer cited")
    for ref in sorted(set(digests) - set(indexed)):
        problems.append(f"{ref} is cited by {', '.join(sorted(seen[ref]))} but not indexed")
    for ref in sorted(set(digests) & set(indexed)):
        if digests[ref] != indexed[ref]:
            problems.append(
                f"{ref} changed at {commit[:10]}: indexed {indexed[ref]}, "
                f"now {digests[ref]} (cited by {', '.join(sorted(seen[ref]))})"
            )

    if problems:
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        print(
            f"❌ {len(problems)} source reference problem(s). "
            "Re-verify each against the pinned tree, then run --update.",
            file=sys.stderr,
        )
        return 1

    if not args.quiet:
        files = {c.rsplit(":", 1)[0] for cites in seen.values() for c in cites}
        print(f"SourceRefs {len(digests)} refs / {len(files)} files @ {tag} ok")
    return 0


def _split(ref: str) -> tuple[str, int, int]:
    path, lines = ref.rsplit(":", 1)
    start, _, end = lines.partition("-")
    return path, int(start), int(end or start)


if __name__ == "__main__":
    sys.exit(main())
