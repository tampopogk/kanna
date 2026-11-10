#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="tampopogk/kanna"
feed_repo="tampopogk/kanna-web"
release_env="$HOME/.kanna/.env.release.local"
update_key="$HOME/.kanna/secrets/kanna-update-ed25519.key"
archs=(arm64 x86_64)

usage() {
  cat >&2 <<'EOF'
usage: scripts/release.sh --staging [--major|--minor|--patch | VERSION] [--dry-run]
       scripts/release.sh --pick COMMIT... [--dry-run]
       scripts/release.sh --promote [vX.Y.Z-staging.N] [--soak DAYS | --no-soak] [--yes] [--dry-run]
       scripts/release.sh --fail vX.Y.Z-staging.N [REASON] [--dry-run]
       scripts/release.sh --status
EOF
  exit 2
}
fail() { echo "release: $*" >&2; exit 1; }
step() { echo "==> $*"; }
. "$repo_root/scripts/env.sh"

mode=""
bump=""
dry_run=false
yes=false
soak_days="$KANNA_SOAK_DAYS"
soak_given=false
words=()
set_mode() { [[ -z "$mode" ]] || usage; mode="$1"; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --staging|--promote|--fail|--status|--pick) set_mode "${1#--}" ;;
    --major|--minor|--patch) [[ -z "$bump" ]] || usage; bump="${1#--}" ;;
    --soak) [[ $# -ge 2 ]] || usage; soak_days="$2"; soak_given=true; shift ;;
    --soak=*) soak_days="${1#--soak=}"; soak_given=true ;;
    --no-soak) soak_days=0; soak_given=true ;;
    --yes) yes=true ;;
    --dry-run) dry_run=true ;;
    -h|--help) usage ;;
    -*) usage ;;
    *) words+=("$1") ;;
  esac
  shift
done
[[ -n "$mode" ]] || fail "say what to release: --staging makes a release candidate, --promote releases a soaked candidate to production (docs/specs/release-candidates.md)"
[[ "$soak_days" =~ ^[0-9]+(\.[0-9]+)?$ ]] || fail "--soak is \"$soak_days\": use a number of days, e.g. --soak 0.5"
[[ "$mode" == promote ]] || ! $soak_given || usage
[[ "$mode" == promote ]] || ! $yes || usage
[[ "$mode" == staging || -z "$bump" ]] || usage
candidate_pattern='^v[0-9]+\.[0-9]+\.[0-9]+-staging\.[0-9]+$'

main_checkout="$(kanna_main_checkout "$repo_root" || true)"
main_checkout="${main_checkout:-$repo_root}"
src="$main_checkout/.worktrees/release"
soak_state="${KANNA_SOAK_STATE:-$HOME/Library/Application Support/kanna-staging}"
soak_cli="${KANNA_SOAK_CLI:-/Applications/Kanna Staging.app/Contents/Helpers/kanna}"
now="$(date +%s)"

github() { gh "$@"; }

preflight_credentials() {
  [[ "$(git remote get-url origin)" =~ github\.com[:/]$repo(\.git)?$ ]] || fail "origin is $(git remote get-url origin), not $repo: git remote set-url origin https://github.com/$repo.git"
  local identity
  identity="$(security find-identity -v -p codesigning | sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "\(Developer ID Application: .*\)"$/\1 \2/p' | head -1)"
  [[ -n "$identity" ]] || fail "no \"Developer ID Application\" identity in the keychain: install the certificate and its private key (security find-identity -v -p codesigning)"
  sign_hash="${identity%% *}"
  sign_name="${identity#* }"
  echo "    signing as $sign_name"

  [[ -f "$release_env" ]] || fail "$release_env is missing: it names the notary credential (APPLE_KEYCHAIN_PROFILE, APPLE_KEYCHAIN_PATH); run Kanna v2's ./kd release setup-notarization"
  env_value() { sed -n "s/^$1=\"\{0,1\}\([^\"]*\)\"\{0,1\}[[:space:]]*$/\1/p" "$release_env" | tail -1; }
  local notary_profile notary_keychain
  notary_profile="$(env_value APPLE_KEYCHAIN_PROFILE)"
  notary_keychain="$(env_value APPLE_KEYCHAIN_PATH)"
  [[ -n "$notary_profile" ]] || fail "$release_env doesn't set APPLE_KEYCHAIN_PROFILE"
  [[ -n "$notary_keychain" ]] || fail "$release_env doesn't set APPLE_KEYCHAIN_PATH"
  [[ -f "$notary_keychain" ]] || fail "the keychain $notary_keychain (APPLE_KEYCHAIN_PATH) doesn't exist"
  notary=(--keychain-profile "$notary_profile" --keychain "$notary_keychain")
  xcrun notarytool history "${notary[@]}" >/dev/null 2>&1 ||
    fail "notarytool can't use profile \"$notary_profile\" in $notary_keychain: unlock that keychain, or save the credential again with xcrun notarytool store-credentials"
  echo "    notarizing with profile $notary_profile"

  [[ -f "$update_key" ]] || fail "the update key $update_key is missing: see docs/specs/auto-update.md#6-the-update-key"
  [[ "$(stat -f %Lp "$update_key")" == 600 ]] || fail "$update_key must be mode 600"
  [[ "$(swift "$repo_root/scripts/update-public-key.swift" "$update_key")" == "$KANNA_UPDATE_PUBLIC_KEY" ]] || fail "$update_key doesn't match KANNA_UPDATE_PUBLIC_KEY in scripts/env.sh"

  preflight_github
  command -v claude >/dev/null || fail "the claude CLI isn't installed: it writes the release notes"
  [[ "$(github api "repos/$feed_repo" --jq .permissions.push)" == true ]] || fail "this gh account can't push to $feed_repo, which holds the update feeds"
  [[ "$(github api "repos/$repo" --jq .full_name)" == "$repo" ]] || fail "$repo resolves to another repo"
}

preflight_github() {
  local gh_status
  gh_status="$(github auth status --active 2>&1)" || fail "gh can't reach GitHub as a signed-in account (run gh auth login if it isn't signed in):
$(grep -v -i token <<<"$gh_status")"
}

published_build() {
  local feed
  feed="$(curl -fsSL "$KANNA_UPDATE_FEEDS/$1-$2.xml" 2>/dev/null || true)"
  sed -n 's#.*<sparkle:version>\([0-9]*\)</sparkle:version>.*#\1#p' <<<"$feed" | head -1
}

toolchain() {
  local rust swift_version xcode
  rust="$(rustc --version 2>/dev/null | cut -d' ' -f1-2)"
  swift_version="$(swift --version 2>/dev/null | sed -n 's/.*Swift version \([0-9.]*\).*/Swift \1/p' | head -1)"
  xcode="$(xcodebuild -version 2>/dev/null | tr '\n' ' ' | sed -e 's/ Build version \(.*\) $/ (\1)/' -e 's/ $//')"
  echo "${rust:-rustc ?} · ${swift_version:-Swift ?} · ${xcode:-Xcode ?}"
}

production_tags() { git tag -l 'v*' | { grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true; } | sort -V; }
candidates() { git tag -l "v$1-staging.*" | { grep -E "$candidate_pattern" || true; } | sort -V; }
has_tag() { git rev-parse --verify --quiet "refs/tags/$1" >/dev/null; }
tag_commit() { git rev-parse "refs/tags/$1^{commit}"; }
tag_time() { git for-each-ref --format='%(creatordate:unix)' "refs/tags/$1"; }
tag_line() { git for-each-ref --format='%(contents)' "refs/tags/$1" | sed -n "s/^$2: //p" | head -1; }
failed_reason() { git for-each-ref --format='%(contents:subject)' "refs/tags/failed/$1"; }
newer() { [[ "$1" != "$2" && "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" == "$1" ]]; }
short() { git rev-parse --short=12 "$1"; }
day() { date -r "$1" '+%Y-%m-%d %H:%M'; }
span() {
  local seconds="$1"
  if [[ "$seconds" -ge 86400 ]]; then echo "$((seconds / 86400))d $((seconds % 86400 / 3600))h"
  elif [[ "$seconds" -ge 3600 ]]; then echo "$((seconds / 3600))h $((seconds % 3600 / 60))m"
  else echo "$((seconds / 60))m"; fi
}
soaked() { awk -v age="$1" -v days="$soak_days" 'BEGIN { exit !(age >= days * 86400) }'; }

open_version() {
  local highest
  highest="$(git tag -l 'v*-staging.*' | { grep -E "$candidate_pattern" || true; } | sed -e 's/^v//' -e 's/-staging\..*//' | sort -V | tail -1)"
  [[ -n "$highest" ]] || return 0
  has_tag "v$highest" || echo "$highest"
}

fetch_origin() {
  git fetch --quiet --tags origin || fail "can't fetch origin"
}

release_worktree() {
  if [[ -e "$src/.git" ]]; then
    [[ -z "$(git -C "$src" status --porcelain)" ]] || fail "the release worktree $src has changes or an unfinished pick: finish or discard them there (git -C \"$src\" status)"
    git -C "$src" checkout --quiet --detach "$1"
  else
    git worktree prune
    mkdir -p "$(dirname "$src")"
    git worktree add --quiet --detach "$src" "$1"
  fi
  [[ "$(git -C "$src" rev-parse HEAD)" == "$1" ]] || fail "the release worktree $src isn't at $1"
}

problems=()
check_lineage() {
  local cut each parents origin_commit
  cut="$(git merge-base "$1" origin/main)" || { problems+=("$(short "$1") shares no history with origin/main"); return 0; }
  for each in $(git rev-list "$cut..$1"); do
    parents="$(git rev-list --parents -n 1 "$each" | wc -w)"
    origin_commit="$(git log -1 --format=%b "$each" | sed -n 's/^(cherry picked from commit \([0-9a-f]\{40\}\))$/\1/p' | tail -1)"
    if [[ "$parents" -ne 2 || -z "$origin_commit" ]] || ! git merge-base --is-ancestor "$origin_commit" origin/main 2>/dev/null; then
      problems+=("$(git log -1 --format='%h "%s"' "$each") isn't a pick of a commit on origin/main: a release holds only main's commits and picks of them (release.sh --pick)")
    fi
  done
}

settle_problems() {
  [[ ${#problems[@]} -gt 0 ]] || return 0
  local p
  if $dry_run; then
    for p in "${problems[@]}"; do echo "    warning: $p (a real run refuses)"; done
  else
    for p in "${problems[@]}"; do echo "release: $p" >&2; done
    exit 1
  fi
}

staging_cli() { env -u KANNA_INSTANCE -u KANNA_TERMINAL -u KANNA_PANE -u KANNA_HOST KANNA_SOCKET="$soak_state/kanna.sock" "$soak_cli" "$@"; }

soak_gather() {
  local version="$1" out="$2" list="" me runs file name reply
  : >"$out/records.jsonl"
  : >"$out/machines.tsv"
  if [[ -x "$soak_cli" && -e "$soak_state/kanna.sock" ]]; then list="$(staging_cli machine list --json 2>/dev/null || true)"; fi
  jq -e . >/dev/null 2>&1 <<<"$list" || list=""
  me="$(jq -r '.name // empty' <<<"${list:-null}")"
  me="${me:-$(hostname -s)}"
  runs="$(jq -r '.peers[]? | select(.me == true) | .release.version // empty' <<<"${list:-null}" | head -1)"
  [[ -n "$runs" ]] || runs="$(/usr/libexec/PlistBuddy -c 'Print :KannaVersion' "${soak_cli%/Helpers/*}/Info.plist" 2>/dev/null || true)"
  for file in "$soak_state"/crashes/*.json; do
    [[ -f "$file" ]] || continue
    jq -c --arg m "$me" --arg v "$version" 'select(.build.version == $v) | {machine: $m, process, signature, reason, time}' "$file" 2>/dev/null >>"$out/records.jsonl" || true
  done
  printf '%s\t%s\t\n' "$me" "${runs:-?}" >>"$out/machines.tsv"
  if [[ -z "$list" ]]; then
    printf '%s\t-\t%s\n' "other machines" "not asked: Kanna Staging isn't installed and running on this Mac" >>"$out/machines.tsv"
    return 0
  fi
  while IFS=$'\t' read -r name runs; do
    [[ -n "$name" ]] || continue
    if reply="$(staging_cli --host "$name" crashes --all --json 2>/dev/null)" && jq -e '.crashes' >/dev/null 2>&1 <<<"$reply"; then
      jq -c --arg m "$name" --arg v "$version" '.crashes[] | select(.build.version == $v) | {machine: $m, process, signature, reason, time}' <<<"$reply" >>"$out/records.jsonl"
      printf '%s\t%s\t\n' "$name" "${runs:-?}" >>"$out/machines.tsv"
    else
      printf '%s\t%s\t%s\n' "$name" "${runs:-?}" "can't be reached: its crashes aren't counted" >>"$out/machines.tsv"
    fi
  done < <(jq -r '.peers[]? | select(.me != true and .revoked != true) | [.name, (.release.version // "")] | @tsv' <<<"$list")
}

soak_counts() {
  jq -s -r --arg m "$2" '(if $m == "" then . else map(select(.machine == $m)) end) as $r | ($r | length) as $n | ($r | map(.signature) | unique | length) as $s
    | if $n == 0 then "no crashes" else "\($n) crash\(if $n == 1 then "" else "es" end), \($s) signature\(if $s == 1 then "" else "s" end)" end' "$1/records.jsonl"
}

soak_signatures() {
  jq -s -r 'group_by(.signature) | map({signature: .[0].signature, count: length, process: .[0].process, reason: (.[0].reason // "?"), machines: (map(.machine) | unique | join(", ")), last: (map(.time) | max)})
    | sort_by(-.count, .signature)[] | "    \(.signature)  ×\(.count)  \(.process)  \(.reason[0:120])  (\(.machines); last \(.last[0:16] | sub("T"; " ")))"' "$1/records.jsonl"
}

soak_until() {
  local version="${1#v}" next
  version="${version%-staging.*}"
  next="$(candidates "$version" | awk -v t="$1" 'found { print; exit } $0 == t { found = 1 }')"
  if [[ -n "$next" ]]; then echo "$(tag_time "$next") $next"; else echo "$now"; fi
}

soak_report() {
  local tag="$1" out began ended next built current name runs note
  out="$(mktemp -d "${TMPDIR:-/tmp}/kanna-soak.XXXXXX")"
  began="$(tag_time "$tag")"
  read -r ended next <<<"$(soak_until "$tag")"
  step "Soak of $tag ($(short "$(tag_commit "$tag")"))"
  if [[ -n "$next" ]]; then echo "    the staging build for $(span $((ended - began))), since $(day "$began") (until $next)"
  else echo "    the staging build for $(span $((ended - began))), since $(day "$began") (still is)"; fi
  built="$(tag_line "$tag" Toolchain)"
  current="$(toolchain)"
  if [[ -z "$built" ]]; then echo "    its toolchain wasn't recorded; now $current"
  elif [[ "$built" == "$current" ]]; then echo "    built with $built, as now"
  else echo "    built with $built; this Mac now has $current"; fi
  soak_gather "${tag#v}" "$out"
  while IFS=$'\t' read -r name runs note; do
    if [[ -n "$note" ]]; then printf '    %-10s %s\n' "$name" "$note"
    else printf '    %-10s runs %-22s %s\n' "$name" "$runs" "$(soak_counts "$out" "$name")"; fi
  done <"$out/machines.tsv"
  soak_signatures "$out"
  rm -rf "$out"
}

notarize() {
  local file="$1" result id status attempt
  for attempt in 1 2 3; do
    result="$(xcrun notarytool submit "$file" "${notary[@]}" --wait --output-format json)" || true
    id="$(plutil -extract id raw -o - - <<<"$result" 2>/dev/null || true)"
    [[ -z "$id" && $attempt -lt 3 ]] || break
    echo "    notarytool didn't get a submission through (attempt $attempt): trying again"
  done
  status="$(plutil -extract status raw -o - - <<<"$result" 2>/dev/null || true)"
  echo "    notarization ${id:-(no id)}: ${status:-failed}"
  if [[ "$status" != Accepted ]]; then
    [[ -n "$id" ]] && xcrun notarytool log "$id" "${notary[@]}" >&2 || echo "$result" >&2
    fail "Apple didn't accept $(basename "$file")"
  fi
}

build_and_package() {
  local arch triple built built_commit stage app binary rel stripped zip stray dmg linux update_tools update_protocol macos_target signature
  : >"$verify_output"
  for arch in "${archs[@]}"; do
    step "Build $app_name $version for $arch at $(short "$commit")"
    KANNA_VERSION="$version" "$src/scripts/build.sh" "$channel" --arch "$arch"
    triple="$(kanna_arch_triple "$arch")"
    if [[ "$triple" == "$(kanna_host_triple)" ]]; then built="$src/target/Kanna.app"; else built="$src/target/$triple/Kanna.app"; fi

    stage="$dist/$arch"
    rm -rf "$stage"
    mkdir -p "$stage"
    ditto "$built" "$stage/$app_name.app"
    app="$stage/$app_name.app"
    [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" == "${version%%-*}" ]] || fail "$app doesn't have version ${version%%-*}"
    built_commit="$(/usr/libexec/PlistBuddy -c 'Print :KannaCommit' "$app/Contents/Info.plist")"
    [[ -n "$built_commit" && "$commit" == "$built_commit"* ]] || fail "$app is built from ${built_commit:-an unknown commit}, not $commit"
    for binary in "$app/Contents/MacOS/Kanna" "$app"/Contents/Helpers/*; do
      lipo -archs "$binary" | grep -qx "$arch" || fail "$binary isn't $arch only: $(lipo -archs "$binary")"
    done

    step "Strip $arch (dSYMs in ${dist#"$repo_root/"}/dSYMs/$arch)"
    for binary in "$app/Contents/MacOS/Kanna" "$app"/Contents/Helpers/*; do
      file -b "$binary" | grep -q Mach-O || continue
      rel="${binary#"$app/Contents/"}"
      mkdir -p "$dist/dSYMs/$arch/$(dirname "$rel")"
      dsymutil "$binary" -o "$dist/dSYMs/$arch/$rel.dSYM"
      stripped="$(strip -S "$binary" 2>&1)" || fail "strip -S $binary failed: $stripped"
      [[ "$(dsymutil -s "$binary" | grep -Ec '^\[ *[0-9]+\] [0-9a-f]+ [0-9a-f]+ \(N_OSO ')" == 0 ]] || fail "$binary still has a debug map after strip -S"
    done
    zip="$dist/$dmg_prefix-$version-$arch-dSYMs.zip"
    ditto -c -k --norsrc --noextattr "$dist/dSYMs/$arch" "$zip"
    symbols+=("$zip")

    step "Sign $arch"
    stray="$(find "$app/Contents" -type f ! -regex '.*/Contents/Helpers/[^/]*' ! -path "*/Contents/MacOS/Kanna" ! -path "*/Contents/Frameworks/Sparkle.framework/*" -exec sh -c 'file -b "$1" | grep -q Mach-O && echo "$1"' _ {} \;)"
    [[ -z "$stray" ]] || fail "Mach-O files other than Contents/MacOS/Kanna, Sparkle.framework and those directly in Contents/Helpers would go unsigned: $stray"
    kanna_sign_app "$src" "$app" "$sign_hash" --options runtime --timestamp || fail "signing $app failed"
    codesign --verify --strict --deep --verbose=2 "$app"

    step "DMG $arch"
    ln -s /Applications "$stage/Applications"
    dmg="$dist/$dmg_prefix-$version-$arch.dmg"
    hdiutil create -quiet -volname "$app_name" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"
    codesign --force --sign "$sign_hash" --timestamp "$dmg"

    step "Notarize $(basename "$dmg")"
    notarize "$dmg"
    xcrun stapler staple -q "$dmg"

    step "Verify $(basename "$dmg")"
    {
      echo "\$ codesign --verify --strict --verbose=2 $(basename "$dmg")"
      codesign --verify --strict --verbose=2 "$dmg" 2>&1 || true
      echo "\$ spctl -a -t open --context context:primary-signature -vv $(basename "$dmg")"
      spctl -a -t open --context context:primary-signature -vv "$dmg" 2>&1 || true
      echo "\$ xcrun stapler validate $(basename "$dmg")"
      xcrun stapler validate "$dmg" 2>&1 || true
    } | tee -a "$verify_output"
    codesign --verify --strict "$dmg" && spctl -a -t open --context context:primary-signature "$dmg" 2>/dev/null && xcrun stapler validate -q "$dmg" ||
      fail "$(basename "$dmg") didn't verify"
    dmgs+=("$dmg")
  done

  step "Linux tarball (x86_64)"
  KANNA_VERSION="$version" "$src/scripts/build-linux.sh" "$channel" --arch x86_64 --out "$dist"
  linux="$dist/$dmg_prefix-$version-linux-x86_64.tar.gz"
  [[ -f "$linux" ]] || fail "build-linux.sh didn't make $linux"
  [[ "$(tar -xzOf "$linux" kanna/release.json | jq -r .commit)" == "$commit" ]] || fail "$(basename "$linux") isn't built from $commit"
  dmgs+=("$linux")

  step "Update feeds"
  update_tools="$(dirname "$(find "$src/target/" -path '*/artifacts/sparkle/Sparkle/bin/sign_update' -print -quit)")"
  [[ -x "$update_tools/sign_update" ]] || fail "the build left no Sparkle sign_update under $src/target/"
  update_protocol="$(tar -xzOf "$linux" kanna/release.json | jq -r .protocol)"
  [[ "$update_protocol" =~ ^[0-9]+$ ]] || fail "$(basename "$linux") names no protocol"
  macos_target="$(kanna_macos_target "$src")"
  mkdir -p "$dist/feeds"
  for arch in "${archs[@]}"; do
    dmg="$dist/$dmg_prefix-$version-$arch.dmg"
    signature="$("$update_tools/sign_update" --ed-key-file "$update_key" -p "$dmg")"
    [[ -n "$signature" ]] || fail "sign_update gave no signature for $(basename "$dmg")"
    "$repo_root/scripts/write-appcast.sh" --title "$app_name" --version "$version" --build "$build_number" --minimum "$macos_target.0" --protocol "$update_protocol" \
      --url "https://github.com/$repo/releases/download/$tag/$(basename "$dmg")" --length "$(stat -f %z "$dmg")" --signature "$signature" >"$dist/feeds/$channel-$arch.xml"
    "$update_tools/sign_update" --ed-key-file "$update_key" "$dist/feeds/$channel-$arch.xml"
    "$update_tools/sign_update" --ed-key-file "$update_key" --verify "$dist/feeds/$channel-$arch.xml" || fail "$channel-$arch.xml didn't verify"
  done
}

write_notes() {
  local subjects prompt
  subjects="$(git log --merges --first-parent --pretty=format:'%s' "$1")"
  [[ -n "$subjects" ]] || subjects="Bug fixes and improvements"
  prompt="You are writing public release notes for Kanna, a macOS app for running coding agents in terminals."
  prompt+=" Given these raw git merge commit subjects from stdin, produce a concise, user-facing bulleted changelog in markdown. Rules:"
  prompt+=" Only include changes a user would care about (features, fixes, UX improvements)."
  prompt+=" Skip internal refactors, tests, CI, build, release, signing and documentation changes."
  prompt+=" Each bullet is one short sentence in plain English."
  prompt+=" Group them under ### Added and ### Fixed (omit an empty section)."
  prompt+=" If there is nothing user-facing, write only: Bug fixes and improvements."
  prompt+=" This changelog is public: say what changed, not how."
  prompt+=" Never mention branch names, internal names, file paths, scripts, commands, crates, classes, libraries or architecture."
  prompt+=" Output only the markdown."
  claude --print --model haiku "$prompt" <<<"$subjects" || fail "claude couldn't write the release notes"
}

publish_feeds() { "$repo_root/scripts/publish-appcast.sh" "$dist/feeds" "$build_number"; }

build_and_publish() {
  local arch published notes range flags assets url
  build_number="$(git rev-list --count "$commit")"
  for arch in "${archs[@]}"; do
    published="$(published_build "$channel" "$arch")"
    [[ -z "$published" || "$build_number" -gt "$published" ]] || fail "build $build_number isn't above $channel-$arch's published build $published: Sparkle would never offer it"
  done
  echo "    $app_name $version (build $build_number) from ${last_tag:-the start} to $(short "$commit")"

  release_worktree "$commit"
  dist="$repo_root/target/release-dmg/$tag"
  rm -rf "$dist"
  mkdir -p "$dist"
  dmgs=()
  symbols=()
  verify_output="$dist/verification.txt"
  build_and_package

  step "Release notes"
  if [[ -n "$last_tag" ]]; then range="$last_tag..$commit"; else range="$commit"; fi
  notes="$(write_notes "$range")"
  [[ -n "$notes" ]] || fail "claude wrote empty release notes"
  printf '%s\n' "$notes" >"$dist/notes.md"
  [[ -z "$notes_footer" ]] || printf '\n%s\n' "$notes_footer" >>"$dist/notes.md"
  echo "---"
  cat "$dist/notes.md"
  echo "---"

  if $dry_run; then
    step "Dry run: built, signed, notarized and verified; nothing tagged, pushed or published"
    for arch in ${dmgs[@]+"${dmgs[@]}"} ${symbols[@]+"${symbols[@]}"}; do echo "    $arch"; done
    echo "    feeds: $dist/feeds"
    echo "    verification: $verify_output"
    exit 0
  fi

  if [[ -n "$cut_branch" ]]; then
    step "Cut $cut_branch at $(short "$commit") on origin"
    git push --quiet origin "$commit:refs/heads/$cut_branch"
  fi
  step "Tag $tag on origin"
  if ! $tag_exists; then
    git tag -a "$tag" -m "$tag_message" "$commit"
    git push --quiet origin "refs/tags/$tag"
  fi

  step "Release $tag on $repo"
  flags=(--repo "$repo" --verify-tag --title "Kanna $tag" --notes-file "$dist/notes.md")
  [[ "$channel" != staging ]] || flags+=(--prerelease)
  github release create "$tag" ${dmgs[@]+"${dmgs[@]}"} ${symbols[@]+"${symbols[@]}"} "${flags[@]}" >/dev/null
  assets="$(github release view "$tag" --repo "$repo" --json assets --jq '.assets[].name')"
  url="$(github release view "$tag" --repo "$repo" --json url --jq .url)"
  step "Published $url"
  printf '%s\n' "$assets" | sed 's/^/    /'
  step "Publish the update feeds"
  publish_feeds
}

run_staging() {
  local wanted="" open major minor patch branch last n
  [[ ${#words[@]} -le 1 ]] || usage
  [[ ${#words[@]} -eq 0 || -z "$bump" ]] || usage
  [[ ${#words[@]} -eq 0 || "${words[0]}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "VERSION is \"${words[0]}\": use X.Y.Z, e.g. 4.0.0"
  step "Preflight"
  fetch_origin
  last_tag="$(production_tags | tail -1)"
  open="$(open_version)"
  if [[ ${#words[@]} -eq 1 ]]; then
    wanted="${words[0]}"
    [[ -z "$last_tag" ]] || newer "$wanted" "${last_tag#v}" || fail "$wanted isn't newer than the last release, $last_tag"
  elif [[ -n "$bump" || -z "$open" ]]; then
    [[ -n "$last_tag" ]] || fail "there's no v* tag yet: give the first version, e.g. scripts/release.sh --staging 4.0.0"
    IFS=. read -r major minor patch <<<"${last_tag#v}"
    case "${bump:-patch}" in
      major) wanted="$((major + 1)).0.0" ;;
      minor) wanted="$major.$((minor + 1)).0" ;;
      patch) wanted="$major.$minor.$((patch + 1))" ;;
    esac
  fi
  base="${wanted:-$open}"
  [[ -z "$open" || "$base" == "$open" ]] || newer "$base" "$open" || fail "$open is the open version, with candidates soaking: $base is below it"
  [[ -z "$open" || "$base" == "$open" ]] || echo "    $base starts a new version from main; $open stays unpromoted"

  branch="release/$base"
  cut_branch=""
  if git rev-parse --verify --quiet "refs/remotes/origin/$branch" >/dev/null; then
    commit="$(git rev-parse "refs/remotes/origin/$branch")"
    echo "    building origin/$branch at $(short "$commit")"
    check_lineage "$commit"
  else
    commit="$(git rev-parse HEAD)"
    cut_branch="$branch"
    [[ -z "$(git status --porcelain)" ]] || problems+=("the working tree has uncommitted changes, which the release wouldn't have: commit or remove them")
    git merge-base --is-ancestor "$commit" origin/main || problems+=("HEAD $(short "$commit") isn't on origin/main: merge it into main and push, then make the candidate from main")
    [[ -z "$last_tag" ]] || git merge-base --is-ancestor "$last_tag" "$commit" ||
      problems+=("$last_tag isn't behind $(short "$commit"): merge origin/release/${last_tag#v} back into main first (docs/specs/release-candidates.md#3-the-release-branch)")
    echo "    first candidate of $base: cutting $branch at $(short "$commit")"
  fi
  last="$(candidates "$base" | tail -1)"
  n="${last##*.}"
  tag_exists=false
  if [[ -n "$last" && "$(tag_commit "$last")" == "$commit" ]]; then
    tag_exists=true
    version="${last#v}"
  else
    version="$base-staging.$((${n:-0} + 1))"
  fi
  tag="v$version"
  settle_problems

  preflight_credentials
  if $tag_exists; then
    ! github release view "$tag" --repo "$repo" >/dev/null 2>&1 || fail "nothing new to soak: $branch is still at $tag's commit, $(short "$commit") (release.sh --pick brings fixes from main)"
    echo "    $tag is already on origin at $(short "$commit") without a release: carrying on with it"
  fi
  channel=staging
  app_name="Kanna Staging"
  dmg_prefix="Kanna-Staging"
  notes_footer=""
  tag_message="Kanna $tag

Branch: $branch
Toolchain: $(toolchain)"
  build_and_publish
  step "$tag is soaking: release.sh --status shows it, release.sh --promote releases it after $soak_days days"
}

run_pick() {
  local open branch tip picked parents
  [[ ${#words[@]} -ge 1 ]] || usage
  step "Preflight"
  fetch_origin
  open="$(open_version)"
  [[ -n "$open" ]] || fail "no version is open: release.sh --staging makes the first candidate"
  branch="release/$open"
  git rev-parse --verify --quiet "refs/remotes/origin/$branch" >/dev/null || fail "origin has no $branch: the next release.sh --staging cuts it from main, so there is nothing to pick onto"
  tip="$(git rev-parse "refs/remotes/origin/$branch")"
  picks=()
  for picked in "${words[@]}"; do
    picked="$(git rev-parse --verify --quiet "$picked^{commit}")" || fail "$picked isn't a commit"
    git merge-base --is-ancestor "$picked" origin/main || fail "$(short "$picked") isn't on origin/main: a fix lands on main first, then is picked"
    picks+=("$picked")
  done
  release_worktree "$tip"
  for picked in "${picks[@]}"; do
    parents="$(git rev-list --parents -n 1 "$picked" | wc -w)"
    step "Pick $(git log -1 --format='%h %s' "$picked")"
    if [[ "$parents" -gt 2 ]]; then git -C "$src" cherry-pick -x -m 1 "$picked" >/dev/null || pick_stopped "$branch"
    else git -C "$src" cherry-pick -x "$picked" >/dev/null || pick_stopped "$branch"; fi
  done
  if $dry_run; then
    git -C "$src" log --format='    %h %s' "$tip..HEAD"
    git -C "$src" checkout --quiet --detach "$tip"
    step "Dry run: the picks apply; nothing pushed"
    exit 0
  fi
  git -C "$src" push --quiet origin "HEAD:refs/heads/$branch"
  step "$branch is at $(git -C "$src" rev-parse --short=12 HEAD): release.sh --staging builds the next candidate from it"
  git -C "$src" log --format='    %h %s' "$tip..HEAD"
}

pick_stopped() {
  fail "the pick stopped in $src. Finish it there: resolve, git cherry-pick --continue, then git push origin HEAD:refs/heads/$1. Or drop it: git cherry-pick --abort"
}

named_candidate() {
  [[ "$1" =~ $candidate_pattern ]] || fail "\"$1\" isn't a candidate: use vX.Y.Z-staging.N"
  has_tag "$1" || fail "there is no candidate $1 (release.sh --status lists them)"
}

run_promote() {
  local open base age reason later ended next soaked_for answer
  [[ ${#words[@]} -le 1 ]] || usage
  step "Preflight"
  fetch_origin
  last_tag="$(production_tags | tail -1)"
  if [[ ${#words[@]} -eq 1 ]]; then
    candidate="${words[0]}"
  else
    open="$(open_version)"
    [[ -n "$open" ]] || fail "no version is open, so there is no candidate to promote: release.sh --staging makes one"
    candidate="$(candidates "$open" | tail -1)"
  fi
  named_candidate "$candidate"
  base="${candidate#v}"
  base="${base%-staging.*}"
  version="$base"
  tag="v$version"
  commit="$(tag_commit "$candidate")"
  age=$((now - $(tag_time "$candidate")))
  read -r ended next <<<"$(soak_until "$candidate")"
  soaked_for="$(span $((ended - $(tag_time "$candidate"))))"
  echo "    $candidate at $(short "$commit"), $(span "$age") old"

  tag_exists=false
  reason="$(failed_reason "$candidate")"
  [[ -z "$reason" ]] || problems+=("$candidate failed its soak ($reason): promote a later candidate")
  if has_tag "$tag"; then
    if [[ "$(tag_commit "$tag")" == "$commit" ]]; then tag_exists=true; last_tag="$(production_tags | { grep -vx "$tag" || true; } | tail -1)"
    else problems+=("$tag is already released from another commit, $(short "$(tag_commit "$tag")")"); fi
  elif [[ -n "$last_tag" ]] && ! newer "$version" "${last_tag#v}"; then
    problems+=("$version isn't newer than the last release, $last_tag")
  fi
  check_lineage "$commit"
  ! git rev-parse --verify --quiet "refs/remotes/origin/release/$base" >/dev/null || git merge-base --is-ancestor "$commit" "refs/remotes/origin/release/$base" ||
    problems+=("$candidate's commit $(short "$commit") isn't on origin/release/$base")
  soaked "$age" || problems+=("$candidate is $(span "$age") old and the soak is $soak_days days: wait, or say --soak DAYS or --no-soak")
  later="$(candidates "$base" | tail -1)"
  [[ "$later" == "$candidate" ]] || echo "    note: $later is a later candidate of $base"
  settle_problems

  preflight_credentials
  github release view "$candidate" --repo "$repo" >/dev/null 2>&1 || fail "$repo has no pre-release $candidate: only a candidate release.sh --staging published can be promoted"
  if $tag_exists; then
    ! github release view "$tag" --repo "$repo" >/dev/null 2>&1 || fail "$repo already has a release $tag"
    echo "    $tag is already on origin at $(short "$commit") without a release: carrying on with it"
  fi
  soak_report "$candidate"
  if $dry_run; then
    echo "    dry run: not asking"
  elif ! $yes; then
    [[ -t 0 ]] || fail "promoting asks first and this isn't a terminal: pass --yes to promote without the question"
    read -r -p "Promote $candidate to production as Kanna $version? [y/N] " answer
    [[ "$answer" =~ ^[Yy]([Ee][Ss])?$ ]] || fail "not promoted"
  fi

  channel=production
  app_name="Kanna"
  dmg_prefix="Kanna"
  cut_branch=""
  notes_footer="Built from the same source as release candidate $candidate ($(short "$commit")), which soaked for $soaked_for."
  tag_message="Kanna $tag

Candidate: $candidate
Soaked: $soaked_for"
  if $soak_given; then
    notes_footer+=" The soak asked of it was $soak_days days, not the usual $KANNA_SOAK_DAYS."
    tag_message+="
Soak: $soak_days days (default $KANNA_SOAK_DAYS)"
  fi
  build_and_publish
  if git rev-parse --verify --quiet "refs/remotes/origin/release/$base" >/dev/null; then
    step "Next: merge origin/release/$base back into main through a task, so the next version's candidates count from $tag"
  fi
}

run_fail() {
  local reason commit base body title today
  [[ ${#words[@]} -ge 1 ]] || usage
  candidate="${words[0]}"
  reason="${words[*]:1}"
  reason="${reason:-no reason given}"
  fetch_origin
  named_candidate "$candidate"
  commit="$(tag_commit "$candidate")"
  base="${candidate#v}"
  base="${base%-staging.*}"
  [[ -z "$(failed_reason "$candidate")" ]] || fail "$candidate already failed its soak: $(failed_reason "$candidate")"
  ! has_tag "v$base" || [[ "$(tag_commit "v$base")" != "$commit" ]] || fail "$candidate was promoted to v$base: it can't fail now"
  today="$(date '+%Y-%m-%d')"
  if $dry_run; then
    step "Dry run: would tag failed/$candidate on origin (\"$reason\") and mark the pre-release $candidate (soak failed)"
    exit 0
  fi
  preflight_github
  step "Fail $candidate"
  git tag -a "failed/$candidate" -m "$reason" "$commit"
  git push --quiet origin "refs/tags/failed/$candidate"
  if title="$(github release view "$candidate" --repo "$repo" --json name --jq .name 2>/dev/null)"; then
    body="$(github release view "$candidate" --repo "$repo" --json body --jq .body)"
    github release edit "$candidate" --repo "$repo" --title "$title (soak failed)" --notes "**Soak failed, $today:** $reason

$body" >/dev/null
    echo "    marked the pre-release $candidate"
  else
    echo "    warning: $repo has no pre-release $candidate to mark; the tag failed/$candidate records it"
  fi
  step "$candidate can't be promoted. Pick the fix onto release/$base (release.sh --pick) and make the next candidate (release.sh --staging)"
}

run_status() {
  local open branch tip candidate commit age reason state out latest since name note _
  [[ ${#words[@]} -eq 0 ]] || usage
  git fetch --quiet --tags origin 2>/dev/null || echo "    warning: can't fetch origin; showing what this checkout has"
  last_tag="$(production_tags | tail -1)"
  echo "last release: ${last_tag:-none}"
  open="$(open_version)"
  if [[ -z "$open" ]]; then
    echo "no version is open: release.sh --staging makes the first candidate of the next"
    return 0
  fi
  branch="release/$open"
  tip="$(git rev-parse --verify --quiet "refs/remotes/origin/$branch" || true)"
  echo "open version: $open (${tip:+origin/$branch at $(short "$tip")}${tip:-no release branch yet: the next candidate cuts it from main})"
  out="$(mktemp -d "${TMPDIR:-/tmp}/kanna-soak.XXXXXX")"
  latest="$(candidates "$open" | tail -1)"
  for candidate in $(candidates "$open"); do
    commit="$(tag_commit "$candidate")"
    age=$((now - $(tag_time "$candidate")))
    reason="$(failed_reason "$candidate")"
    if [[ -n "$reason" ]]; then state="failed: $reason"
    elif [[ "$candidate" != "$latest" ]]; then state="superseded"
    elif soaked "$age"; then state="soaked: release.sh --promote $candidate"
    else state="soaking, $soak_days days needed"; fi
    soak_gather "${candidate#v}" "$out"
    since=""
    [[ -z "$tip" ]] || since="$(git rev-list --count "$commit..$tip") on the branch since"
    printf '  %-22s %s  %-8s %-26s %s  %s\n' "$candidate" "$(short "$commit")" "$(span "$age")" "$(soak_counts "$out" "")" "${since:-}" "$state"
  done
  while IFS=$'\t' read -r name _ note; do [[ -z "$note" ]] || echo "  $name: $note"; done <"$out/machines.tsv"
  rm -rf "$out"
  if [[ -n "$tip" && "$(tag_commit "$latest")" != "$tip" ]]; then
    echo "on $branch since $latest (the next candidate's):"
    git log --format='  %h %s' "$(tag_commit "$latest")..$tip"
  fi
}

if [[ -n "${KANNA_RELEASE_HOOKS:-}" ]]; then . "$KANNA_RELEASE_HOOKS"; fi

cd "$repo_root"
export KANNA_RUST_PROFILE=release RUSTC_WRAPPER=
"run_$mode"
