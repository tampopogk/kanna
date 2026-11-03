#!/usr/bin/env bash
set -uo pipefail

usage() {
  cat <<'USAGE'
Line and branch coverage of a checkout's Rust and Swift tests, as one LCOV file:
the Stats tab's coverage (docs/specs/code-quality-metrics.md). The daemon runs it
as stats.coverage in .kanna/config.json, in its own worktree at a commit of main.

usage: KANNA_COVERAGE=FILE scripts/coverage.sh [--e2e [SCENARIO …]]
  (run in the checkout to measure; writes FILE)
  --e2e   also run the e2e scenarios (all, or those named) against
          kanna, kanna-daemon and kanna-keeper built with coverage

Rust: cargo llvm-cov over the workspace, with branches (-Z coverage-options=branch,
allowed on stable by RUSTC_BOOTSTRAP=1). Swift: swift test --enable-code-coverage,
exported with Xcode's llvm-cov. Failing tests still count what they ran.
USAGE
}

e2e=0 scenarios=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --e2e) e2e=1 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "coverage: unknown argument $1 (see --help)" >&2; exit 2 ;;
    *) scenarios+=("$1") ;;
  esac
  shift
done
out="${KANNA_COVERAGE:?say where the LCOV goes: KANNA_COVERAGE=FILE scripts/coverage.sh}"
tree="$(pwd -P)"
[[ -f "$tree/scripts/env.sh" ]] || { echo "coverage: run it in a checkout of this repo" >&2; exit 2; }
. "$tree/scripts/env.sh"
kanna_targets "$tree" || exit 1
work="$tree/target/coverage"
rm -rf "$work"
mkdir -p "$work"

host="$(rustc -vV | sed -n 's/^host: //p')"
tools="$(rustc --print sysroot)/lib/rustlib/$host/bin"
if [[ ! -x "$tools/llvm-cov" ]]; then
  echo "==> rustup component add llvm-tools-preview"
  rustup component add llvm-tools-preview || exit 1
fi

echo "==> Rust: cargo llvm-cov --workspace --branch"
RUSTC_BOOTSTRAP=1 cargo llvm-cov --workspace --branch --no-fail-fast --ignore-run-fail --lcov --output-path "$work/rust.lcov" || exit 1

echo "==> Swift: scripts/build.sh, then swift test --enable-code-coverage"
"$tree/scripts/build.sh" || exit 1
swift=(swift test --package-path "$tree/apps/kanna" --scratch-path "$tree/target/kanna-build")
"${swift[@]}" --enable-code-coverage || echo "==> Some Swift tests failed: what they ran still counts"
profdata="$(dirname "$("${swift[@]}" --show-codecov-path)")/default.profdata"
bundle="$(ls -d "$tree"/target/kanna-build/debug/*.xctest 2>/dev/null | head -1)"
if [[ -f "$profdata" && -n "$bundle" ]]; then
  xcrun llvm-cov export -format=lcov -instr-profile "$profdata" "$bundle/Contents/MacOS/$(basename "$bundle" .xctest)" -ignore-filename-regex='(/\.build/|/checkouts/|/Tests/)' >"$work/swift.lcov" || exit 1
else
  echo "==> No Swift coverage: $profdata or the test bundle is missing" >&2
fi

if [[ "$e2e" == 1 ]]; then
  echo "==> e2e: kanna, kanna-daemon and kanna-keeper built with coverage"
  e2e_target="$work/e2e-build"
  RUSTFLAGS="-C instrument-coverage" cargo build -p kanna -p kanna-daemon -p kanna-keeper --target-dir "$e2e_target" || exit 1
  app="$work/Kanna.app"
  cp -R "$tree/target/Kanna.app" "$app"
  for helper in kanna kanna-daemon kanna-keeper; do
    cp "$e2e_target/debug/$helper" "$app/Contents/Helpers/$helper"
    codesign --force --sign - "$app/Contents/Helpers/$helper" >/dev/null 2>&1
  done
  mkdir -p "$work/e2e"
  KANNA_E2E_APP="$app" LLVM_PROFILE_FILE="$work/e2e/%p-%m%c.profraw" "$tree/scripts/e2e-kanna.sh" ${scenarios[@]+"${scenarios[@]}"} || echo "==> Some e2e scenarios failed: what they ran still counts"
  if compgen -G "$work/e2e/*.profraw" >/dev/null; then
    "$tools/llvm-profdata" merge -sparse "$work"/e2e/*.profraw -o "$work/e2e.profdata" || exit 1
    "$tools/llvm-cov" export -format=lcov -instr-profile "$work/e2e.profdata" "$e2e_target/debug/kanna-daemon" -object "$e2e_target/debug/kanna" -object "$e2e_target/debug/kanna-keeper" -ignore-filename-regex='(/\.cargo/|/rustc/|/tests/)' >"$work/e2e.lcov" || exit 1
  else
    echo "==> The e2e scenarios wrote no profiles" >&2
  fi
fi

cat "$work"/*.lcov >"$out"
echo "==> $(grep -c '^SF:' "$out") files in $out"
