#!/usr/bin/env bash
set -uo pipefail

scripts="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
dir="$(mktemp -d "${TMPDIR:-/tmp}/kanna-release.XXXXXX")" && dir="$(cd "$dir" && pwd -P)" || exit 1
trap 'rm -rf "$dir"' EXIT
failed=0
fail() { echo "FAIL: $*"; failed=1; }

origin="$dir/origin.git"
work="$dir/repo"
git init -q --bare -b main "$origin" || exit 1
git init -q -b main "$work" || exit 1
g() { git -C "$work" "$@"; }
g config user.email release@test
g config user.name release-test
g remote add origin "$origin"
mkdir -p "$work/scripts"
cp "$scripts/release.sh" "$scripts/env.sh" "$work/scripts/"
printf '.worktrees/\ntarget/\n' >"$work/.gitignore"
commit() { echo "$1" >"$work/$1.txt" && g add -A && g commit -q -m "$2" && g rev-parse HEAD; }
commit a "first" >/dev/null
g tag -a v4.1.0 -m "Kanna v4.1.0"
b="$(commit b "second")"
g push -q origin main refs/tags/v4.1.0

mkdir -p "$dir/gh" "$dir/state/crashes"
: >"$dir/state/kanna.sock"
cat >"$dir/hooks.sh" <<'EOF'
preflight_credentials() { :; }
preflight_github() { :; }
published_build() { :; }
toolchain() { echo "${FAKE_TOOLCHAIN:-rustc 1.0 · Swift 6.0 · Xcode 26.0}"; }
write_notes() { echo "notes for $1"; }
publish_feeds() { echo "$channel $build_number" >>"$T/feeds.log"; }
build_and_package() {
  echo "$channel $version $commit $(git -C "$src" rev-parse HEAD) $(cd "$src" && ls *.txt | tr '\n' ' ')" >>"$T/builds.log"
  echo dmg >"$dist/$dmg_prefix-$version.dmg"
  dmgs+=("$dist/$dmg_prefix-$version.dmg")
}
github() {
  echo "$*" >>"$T/gh.log"
  [[ "$1" == release ]] || return 0
  local action="$2" tag="$3" title="" notes="" want=""
  shift 3
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --title) title="$2"; shift ;;
      --notes) notes="$2"; shift ;;
      --notes-file) notes="$(cat "$2")"; shift ;;
      --json) want="$2"; shift ;;
      --prerelease) echo pre >"$T/gh/$tag.pre" ;;
    esac
    shift
  done
  case "$action" in
    create|edit) printf '%s\n' "$title" >"$T/gh/$tag.title"; printf '%s\n' "$notes" >"$T/gh/$tag.body" ;;
    view)
      [[ -f "$T/gh/$tag.title" ]] || return 1
      case "$want" in name) cat "$T/gh/$tag.title" ;; body) cat "$T/gh/$tag.body" ;; assets|url) echo "$tag" ;; esac ;;
  esac
}
EOF
cat >"$dir/kanna" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  "machine list --json") echo '{"name":"mbp","peers":[{"name":"mbp","me":true,"release":{"version":"4.1.1-staging.2"}},{"name":"studio","release":{"version":"4.1.1-staging.2"}},{"name":"mini","release":{"version":"4.1.0"}},{"name":"old","revoked":true}]}' ;;
  "--host studio crashes --all --json") cat "$T/studio.json" ;;
  *) echo "unreachable" >&2; exit 3 ;;
esac
EOF
chmod +x "$dir/kanna"
record() { printf '{"process":"%s","signature":"%s","reason":"%s","time":"%s","build":{"channel":"staging","version":"%s"}}' "$1" "$2" "$3" "$4" "$5"; }
record app aaaaaaaaaaaaaaaa SIGABRT 2026-11-09T00:12:41.453Z 4.1.1-staging.2 >"$dir/state/crashes/1-app.json"
record app aaaaaaaaaaaaaaaa SIGABRT 2026-11-09T20:07:14.228Z 4.1.1-staging.2 >"$dir/state/crashes/2-app.json"
record daemon bbbbbbbbbbbbbbbb "panic: index out of bounds" 2026-11-09T10:00:00.000Z 4.1.1-staging.2 >"$dir/state/crashes/3-daemon.json"
record app cccccccccccccccc SIGSEGV 2026-11-08T10:00:00.000Z 4.1.1-staging.1 >"$dir/state/crashes/4-app.json"
echo "not json" >"$dir/state/crashes/5-app.json"
echo "{\"crashes\":[$(record app aaaaaaaaaaaaaaaa SIGABRT 2026-11-10T01:00:00.000Z 4.1.1-staging.2),$(record app dddddddddddddddd SIGBUS 2026-11-01T01:00:00.000Z 4.1.0-staging.9)]}" >"$dir/studio.json"

release() {
  (cd "$work" && T="$dir" KANNA_RELEASE_HOOKS="$dir/hooks.sh" KANNA_SOAK_STATE="$dir/state" KANNA_SOAK_CLI="$dir/kanna" KANNA_SOAK_DAYS=3 scripts/release.sh "$@") >"$dir/said" 2>&1 </dev/null
}
said() { grep -qF -- "$1" "$dir/said"; }
show() { sed 's/^/    | /' "$dir/said"; }
on_origin() { git -C "$origin" rev-parse --verify --quiet "$1^{commit}"; }
ago() { date -r $(($(date +%s) - $1)) '+%Y-%m-%dT%H:%M:%S'; }
builds() { wc -l <"$dir/builds.log" 2>/dev/null | tr -d ' ' || echo 0; }

release && fail "no mode: ran"
said "--staging makes a release candidate" || { fail "no mode: doesn't say what to do"; show; }

release --staging --dry-run || { fail "first candidate, dry run: exit $?"; show; }
said "first candidate of 4.1.1: cutting release/4.1.1" || fail "first candidate, dry run: doesn't say it cuts the branch"
grep -q "^staging 4.1.1-staging.1 $b $b " "$dir/builds.log" || fail "first candidate, dry run: didn't build HEAD: $(cat "$dir/builds.log")"
[[ -z "$(on_origin refs/heads/release/4.1.1)$(on_origin refs/tags/v4.1.1-staging.1)" ]] || fail "first candidate, dry run: pushed something"
[[ ! -s "$dir/feeds.log" ]] || fail "first candidate, dry run: published feeds"

GIT_COMMITTER_DATE="$(ago 345600)" release --staging || { fail "first candidate: exit $?"; show; }
[[ "$(on_origin refs/heads/release/4.1.1)" == "$b" ]] || fail "first candidate: origin's release/4.1.1 isn't at its commit"
[[ "$(on_origin refs/tags/v4.1.1-staging.1)" == "$b" ]] || fail "first candidate: its tag isn't on origin at its commit"
g for-each-ref --format='%(contents)' refs/tags/v4.1.1-staging.1 | grep -qx "Branch: release/4.1.1" || fail "first candidate: the tag doesn't name its branch"
g for-each-ref --format='%(contents)' refs/tags/v4.1.1-staging.1 | grep -q "^Toolchain: rustc 1.0" || fail "first candidate: the tag doesn't record the toolchain"
[[ -f "$dir/gh/v4.1.1-staging.1.pre" ]] || fail "first candidate: not a pre-release"
grep -q "^staging " "$dir/feeds.log" || fail "first candidate: no staging feeds"

c="$(commit c "unrelated")"
f="$(commit f "the fix")"
g push -q origin main
release --staging && fail "branch unchanged: made a candidate"
said "nothing new to soak" || { fail "branch unchanged: wrong refusal"; show; }
release --staging --minor --dry-run || { fail "a new version above the open one: exit $?"; show; }
said "4.2.0 starts a new version from main; 4.1.1 stays unpromoted" || fail "a new version above the open one: doesn't say so"
release --staging 4.1.0 && fail "a version below the last release: ran"

g checkout -q -b side && off="$(commit side "not on main")" && g checkout -q main
release --pick "$off" && fail "pick of a commit that isn't on main: ran"
said "isn't on origin/main" || { fail "pick of a commit that isn't on main: wrong refusal"; show; }
release --pick "$f" --dry-run || { fail "pick, dry run: exit $?"; show; }
[[ "$(on_origin refs/heads/release/4.1.1)" == "$b" ]] || fail "pick, dry run: pushed"
release --pick "$f" || { fail "pick: exit $?"; show; }
tip="$(on_origin refs/heads/release/4.1.1)"
[[ "$(git -C "$origin" log --format=%s -1 "$tip")" == "the fix" && "$(git -C "$origin" rev-parse "$tip^")" == "$b" ]] || fail "pick: release/4.1.1 isn't the candidate plus the fix"
git -C "$origin" log --format=%b -1 "$tip" | grep -q "cherry picked from commit $f" || fail "pick: no cherry-pick line"

before="$(builds)"
GIT_COMMITTER_DATE="$(ago 86400)" release --staging || { fail "second candidate: exit $?"; show; }
[[ "$(on_origin refs/tags/v4.1.1-staging.2)" == "$tip" ]] || fail "second candidate: not tagged at the branch's tip"
tail -1 "$dir/builds.log" | grep -q "^staging 4.1.1-staging.2 $tip $tip a.txt b.txt f.txt $" || fail "second candidate: built $(tail -1 "$dir/builds.log"), not the branch (main's c.txt left out)"
[[ "$(g rev-parse HEAD)" == "$f" && "$(g rev-parse --abbrev-ref HEAD)" == main ]] || fail "second candidate: moved the checkout off main"

release --status || { fail "status: exit $?"; show; }
said "open version: 4.1.1" || fail "status: no open version"
grep -q "v4.1.1-staging.1 .* 4d 0h .*1 crash, 1 signature .*1 on the branch since .*superseded" "$dir/said" || { fail "status: staging.1's row"; show; }
grep -q "v4.1.1-staging.2 .* 1d 0h .*4 crashes, 2 signatures .*0 on the branch since .*soaking, 3 days needed" "$dir/said" || { fail "status: staging.2's row"; show; }
said "mini: can't be reached" || fail "status: doesn't say mini can't be reached"

before="$(builds)"
release --promote --yes && fail "promote of a 1-day-old candidate: ran"
said "v4.1.1-staging.2 is 1d 0h old and the soak is 3 days" || { fail "promote of a 1-day-old candidate: wrong refusal"; show; }
[[ "$(builds)" == "$before" ]] || fail "promote of a 1-day-old candidate: built"

release --promote --soak 0 && fail "promote without a terminal or --yes: ran"
said "pass --yes" || { fail "promote without a terminal: wrong refusal"; show; }
said "aaaaaaaaaaaaaaaa  ×3  app  SIGABRT  (mbp, studio; last 2026-11-10 01:00)" || { fail "soak report: signature a isn't grouped across machines"; show; }
said "bbbbbbbbbbbbbbbb  ×1  daemon  panic: index out of bounds  (mbp; last 2026-11-09 10:00)" || fail "soak report: signature b"
said "cccccccccccccccc" && fail "soak report: counts another candidate's crash"
said "dddddddddddddddd" && fail "soak report: counts another version's crash from studio"
grep -q "mbp .*runs 4.1.1-staging.2 .*3 crashes, 2 signatures" "$dir/said" || fail "soak report: mbp's line"
grep -q "studio .*runs 4.1.1-staging.2 .*1 crash, 1 signature" "$dir/said" || fail "soak report: studio's line"
said "the staging build for 1d 0h" || fail "soak report: how long it was the staging build"
said "as now" || fail "soak report: toolchain unchanged"
[[ "$(builds)" == "$before" ]] || fail "promote without a terminal: built"
FAKE_TOOLCHAIN="rustc 2.0 · Swift 6.0 · Xcode 26.0" release --promote --soak 0 --dry-run || { fail "promote, dry run: exit $?"; show; }
said "this Mac now has rustc 2.0" || fail "soak report: doesn't say the toolchain changed"
[[ -z "$(on_origin refs/tags/v4.1.1)" ]] || fail "promote, dry run: tagged"
release --promote --dry-run || { fail "promote of a young candidate, dry run: exit $?"; show; }
said "warning: v4.1.1-staging.2 is 1d 0h old" || fail "promote of a young candidate, dry run: no warning"

release --fail v4.1.1-staging.1 --dry-run the Desk loses focus || { fail "fail, dry run: exit $?"; show; }
[[ -z "$(on_origin refs/tags/failed/v4.1.1-staging.1)" ]] || fail "fail, dry run: tagged"
release --fail v4.1.1-staging.1 the Desk loses focus || { fail "fail: exit $?"; show; }
[[ "$(on_origin refs/tags/failed/v4.1.1-staging.1)" == "$b" ]] || fail "fail: no failed/ tag on origin"
grep -q "(soak failed)" "$dir/gh/v4.1.1-staging.1.title" || fail "fail: the pre-release's title"
grep -q "Soak failed, .*the Desk loses focus" "$dir/gh/v4.1.1-staging.1.body" && grep -q "notes for" "$dir/gh/v4.1.1-staging.1.body" || fail "fail: the pre-release's notes"
release --fail v4.1.1-staging.1 again && fail "fail twice: ran"
before="$(builds)"
release --promote v4.1.1-staging.1 --no-soak --yes && fail "promote of a failed candidate: ran"
said "failed its soak (the Desk loses focus)" || { fail "promote of a failed candidate: wrong refusal"; show; }
[[ "$(builds)" == "$before" ]] || fail "promote of a failed candidate: built"
release --status && said "failed: the Desk loses focus" || { fail "status: doesn't show the failed candidate"; show; }

release --promote --soak 0 --yes || { fail "promote: exit $?"; show; }
[[ "$(on_origin refs/tags/v4.1.1)" == "$tip" ]] || fail "promote: v4.1.1 isn't on the candidate's commit"
tail -1 "$dir/builds.log" | grep -q "^production 4.1.1 $tip $tip a.txt b.txt f.txt $" || fail "promote: built $(tail -1 "$dir/builds.log"), not the candidate's commit"
[[ "$tip" != "$(g rev-parse origin/main)" ]] || fail "promote: main hadn't moved, so the test proves nothing"
grep -q "Built from the same source as release candidate v4.1.1-staging.2" "$dir/gh/v4.1.1.body" || fail "promote: the notes don't name the candidate"
grep -q "The soak asked of it was 0 days, not the usual 3" "$dir/gh/v4.1.1.body" || fail "promote: the notes don't record the override"
g for-each-ref --format='%(contents)' refs/tags/v4.1.1 | grep -qx "Candidate: v4.1.1-staging.2" || fail "promote: the tag doesn't name the candidate"
g for-each-ref --format='%(contents)' refs/tags/v4.1.1 | grep -qx "Soak: 0 days (default 3)" || fail "promote: the tag doesn't record the override"
[[ ! -f "$dir/gh/v4.1.1.pre" ]] || fail "promote: a pre-release"
tail -1 "$dir/feeds.log" | grep -q "^production " || fail "promote: no production feeds"
release --fail v4.1.1-staging.2 late && fail "fail of a promoted candidate: ran"
release --promote --yes && fail "promote with no open version: ran"
release --status && said "no version is open" || { fail "status after the promote"; show; }

release --staging && fail "next version before the merge back: ran"
said "v4.1.1 isn't behind" || { fail "next version before the merge back: wrong refusal"; show; }
g merge -q --no-ff -m "Merge branch 'release-back'" origin/release/4.1.1 && g push -q origin main || fail "merge back"
release --staging || { fail "next version: exit $?"; show; }
[[ "$(on_origin refs/tags/v4.1.2-staging.1)" == "$(g rev-parse HEAD)" && "$(on_origin refs/heads/release/4.1.2)" == "$(g rev-parse HEAD)" ]] || fail "next version: 4.1.2's first candidate and branch aren't at main"

g tag -a v4.1.2-staging.2 -m "by hand" "$off" && g push -q origin refs/tags/v4.1.2-staging.2
release --promote v4.1.2-staging.2 --no-soak --yes && fail "promote of a hand-made tag off main: ran"
said "isn't a pick of a commit on origin/main" || { fail "promote of a hand-made tag off main: wrong refusal"; show; }
git -C "$origin" update-ref -d refs/tags/v4.1.2-staging.2 && g tag -d v4.1.2-staging.2 >/dev/null
g checkout -q --detach origin/release/4.1.2 && commit direct "pushed straight to the branch" >/dev/null && g push -q origin HEAD:refs/heads/release/4.1.2 && g checkout -q main
before="$(builds)"
release --staging && fail "a release branch with a commit that isn't a pick: ran"
said "\"pushed straight to the branch\" isn't a pick of a commit on origin/main" || { fail "a release branch with a commit that isn't a pick: wrong refusal"; show; }
[[ "$(builds)" == "$before" ]] || fail "a release branch with a commit that isn't a pick: built"

[[ $failed -eq 0 ]] && echo "release: all passed"
exit $failed
