#!/usr/bin/env bash
set -uo pipefail

usage() {
  cat <<'USAGE'
The check a pipeline's stages run (.kanna/pipelines): the build, the Rust tests
and the Swift tests of this worktree. Exit 0 only when all three pass.

usage: scripts/check.sh [--expect-fail] [--base COMMIT]
  --expect-fail  for a bug's failing test: exit 0 only when the build passes,
                 at least one test fails, and every failing test is one the
                 branch added or changed since COMMIT
  --base COMMIT  where the branch began (default: $KANNA_BASE, else its merge
                 base with main)

Runs scripts/build.sh (cargo build and the Swift app, so target/Kanna.app is
there for scripts/e2e-kanna.sh), scripts/merge-test.sh (cargo test, every
crate, no fail-fast), then swift test. A failed build skips the tests.

Prints one line per step, then what failed first: a `FAILED STEP: NAME` line
per failing test or build error, each one's own output below them, and the
path of each step's whole output (.kanna/tmp/check/STEP.log). The last line is
`score: N` for a stage's guard: 0 when all pass, minus the number failing
otherwise, -1000 for a build that fails.
USAGE
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
expect_fail=0 base="${KANNA_BASE:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --expect-fail) expect_fail=1 ;;
    --base) [[ $# -ge 2 ]] || { usage >&2; exit 2; }; base="$2"; shift ;;
    --base=*) base="${1#--base=}" ;;
    -h|--help) usage; exit 0 ;;
    *) echo "check: unknown argument $1 (see --help)" >&2; exit 2 ;;
  esac
  shift
done

out="$repo_root/.kanna/tmp/check"
mkdir -p "$out" || exit 2
rm -f "$out"/*.log "$out"/*.exit

took() {
  local s=$(($(date +%s) - $1))
  if ((s >= 60)); then echo "$((s / 60))m $((s % 60))s"; else echo "${s}s"; fi
}
step() {
  local name="$1" began code; shift
  began="$(date +%s)"
  echo "==> $name: ${*#"$repo_root/"}"
  "$@" >"$out/$name.log" 2>&1 </dev/null
  code=$?
  echo "$code" >"$out/$name.exit"
  if ((code == 0)); then echo "    $name passed in $(took "$began")"; else echo "    $name exited $code after $(took "$began")"; fi
  return "$code"
}

if step build "$repo_root/scripts/build.sh" dev; then
  step cargo-test "$repo_root/scripts/merge-test.sh"
  step swift-test swift test --package-path "$repo_root/apps/kanna" --scratch-path "$repo_root/target/kanna-build"
fi

if [[ "$expect_fail" == 1 && -z "$base" ]]; then
  base="$(git -C "$repo_root" merge-base HEAD main 2>/dev/null)"
fi

CHECK_OUT="$out" CHECK_ROOT="$repo_root" CHECK_EXPECT_FAIL="$expect_fail" CHECK_BASE="$base" exec python3 - <<'REPORT'
import os, re, subprocess, sys

out, root = os.environ["CHECK_OUT"], os.environ["CHECK_ROOT"]
expect_fail, base = os.environ["CHECK_EXPECT_FAIL"] == "1", os.environ["CHECK_BASE"]
ESCAPES = re.compile(r"\x1b(\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(\x07|\x1b\\)|[@-Z\\-_])")
DETAIL_LINES, OUTPUT_LINES = 12, 70


def log(step):
    try:
        with open(f"{out}/{step}.log", errors="replace") as f:
            return ESCAPES.sub("", f.read()).replace("\r", "").splitlines()
    except OSError:
        return None


def code(step):
    try:
        with open(f"{out}/{step}.exit") as f:
            return int(f.read().strip())
    except (OSError, ValueError):
        return None


def compile_errors(lines):
    found = []
    for i, line in enumerate(lines):
        rust = re.match(r"error(\[E\d+\])?: (?!test failed|could not compile|\d+ targets? failed|aborting|process didn't exit)", line)
        swift = re.match(r"(/\S+:\d+:\d+): error: (.*)", line)
        if swift:
            found.append((f"{os.path.relpath(swift.group(1).split(':')[0], root)}:{swift.group(1).split(':')[1]}: {swift.group(2)}", [], None))
        elif rust:
            where = next((l.strip()[4:] for l in lines[i + 1:i + 4] if l.strip().startswith("--> ")), "")
            found.append((f"{where}: {line}" if where else line, lines[i + 1:i + 1 + DETAIL_LINES], None))
    seen, unique = set(), []
    for item in found:
        if item[0] not in seen:
            seen.add(item[0])
            unique.append(item)
    return unique


def cargo_failures(lines):
    target, found, blocks = "", [], {}
    for i, line in enumerate(lines):
        running = re.match(r"\s+(Running|Doc-tests) (.*)", line)
        if running:
            crate = re.search(r"/deps/([A-Za-z0-9_]+)-[0-9a-f]+\)", running.group(2))
            target = crate.group(1) if crate else running.group(2).split(" ")[0]
        failed = re.match(r"test (.+) \.\.\. FAILED$", line)
        if failed:
            found.append((target, failed.group(1)))
        block = re.match(r"---- (.+) stdout ----$", line)
        if block:
            body = []
            for l in lines[i + 1:]:
                if l.startswith("---- ") or l == "failures:":
                    break
                body.append(l)
            blocks[block.group(1)] = [l for l in body if l.strip() and "RUST_BACKTRACE" not in l]
    return [(f"{target} {name}".strip(), blocks.get(name, []), name.split(" ")[0].split("::")[-1]) for target, name in found]


def swift_failures(lines):
    found, issues = [], {}
    for line in lines:
        issue = re.match(r"[✘✗] Test (.+?) recorded an issue at (.*)", line)
        if issue:
            issues.setdefault(issue.group(1), []).append(issue.group(2))
        failed = re.match(r"[✘✗] Test (?!run with \d+ tests? )(.+?) failed after ", line)
        if failed:
            found.append(failed.group(1))
        case = re.match(r"Test Case '-\[(\S+) (\S+)\]' failed", line)
        if case:
            name = f"{case.group(1)} {case.group(2)}"
            found.append(name)
            issues[name] = [l for l in lines if f"-[{name}]" in l and ": error: " in l]
    keys = lambda name: name.strip('"') if name.startswith('"') else name.split(" ")[-1].split("(")[0]
    return [(name, issues.get(name, []), keys(name)) for name in dict.fromkeys(found)]


failures = []
steps = [("build", "build", compile_errors), ("cargo-test", "cargo test", lambda l: cargo_failures(l) or compile_errors(l)), ("swift-test", "swift test", lambda l: swift_failures(l) or compile_errors(l))]
broken = code("build") != 0
for step, label, parse in steps:
    lines, exit_code = log(step), code(step)
    if lines is None or exit_code in (0, None):
        continue
    found = parse(lines)
    if not found:
        found = [(f"exited {exit_code} and named nothing", [l for l in lines if l.strip()][-DETAIL_LINES:], None)]
    failures += [(label, name, detail, key) for name, detail, key in found]

logs = ", ".join(f"{os.path.relpath(out, root)}/{step}.log" for step, _, _ in steps if log(step) is not None)


def report(headline, shown, passed, score):
    print(headline)
    for label, name, _, _ in shown:
        print(f"FAILED {label}: {name}")
    room = OUTPUT_LINES - len(shown)
    for label, name, detail, _ in shown:
        if not detail or room <= 2:
            continue
        print(f"\n{label}: {name}")
        for line in detail[:min(DETAIL_LINES, room - 2)]:
            print(f"    {line.rstrip()}")
        room -= 2 + min(len(detail), DETAIL_LINES)
    print(f"whole output: {logs}")
    if score is not None:
        print(f"score: {score}")
    sys.exit(0 if passed else 1)


if not expect_fail:
    if not failures:
        report("check: the build, the Rust tests and the Swift tests pass", [], True, 0)
    if broken:
        report(f"check: the build doesn't pass ({len(failures)} to fix); no test ran", failures, False, -1000)
    by = ", ".join(f"{label} {sum(1 for f in failures if f[0] == label)}" for label in dict.fromkeys(f[0] for f in failures))
    report(f"check: {len(failures)} failing ({by})", failures, False, -len(failures))

if broken:
    report("check: --expect-fail wants a test that fails, and the build doesn't pass; no test ran", failures, False, None)
if not failures:
    print("FAILED check --expect-fail: every test passes, so none shows the bug")
    report("check: add a test that fails because of the bug", [], False, None)
added = subprocess.run(["git", "-C", root, "diff", "--unified=0", base, "HEAD"], capture_output=True, text=True, errors="replace")
if not base or added.returncode != 0:
    print(f"FAILED check --expect-fail: can't tell what the branch added since '{base}': {added.stderr.strip()}")
    sys.exit(1)
lines = [l[1:] for l in added.stdout.splitlines() if l.startswith("+") and not l.startswith("+++")]
new = lambda key: key is not None and any(re.search(rf"\b(fn|func)\s+{re.escape(key)}\b", l) or f'"{key}"' in l for l in lines)
old = [f for f in failures if not new(f[3])]
if old:
    report(f"check: --expect-fail wants only the branch's own tests to fail; {len(old)} failing aren't ones it added or changed since {base[:8]}", old, False, None)
print(f"check: {len(failures)} of the branch's tests fail as --expect-fail wants, and the rest pass:")
for label, name, _, _ in failures:
    print(f"    {label}: {name}")
print(f"whole output: {logs}")
REPORT
