#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="tampopogk/kanna"
release_env="$HOME/.kanna/.env.release.local"
archs=(arm64 x86_64)

usage() { echo "usage: scripts/release.sh [--major|--minor|--patch | VERSION] [--staging] [--dry-run]" >&2; exit 2; }
fail() { echo "release: $*" >&2; exit 1; }
step() { echo "==> $*"; }

bump=""
explicit=""
staging=false
dry_run=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --major|--minor|--patch) [[ -z "$bump$explicit" ]] || usage; bump="${1#--}" ;;
    --staging) staging=true ;;
    --dry-run) dry_run=true ;;
    -h|--help) usage ;;
    [0-9]*) [[ -z "$bump$explicit" ]] || usage; explicit="$1" ;;
    *) usage ;;
  esac
  shift
done
[[ -z "$explicit" || "$explicit" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "VERSION is \"$explicit\": use X.Y.Z, e.g. 4.0.0"

step "Preflight"
cd "$repo_root"
[[ "$(git remote get-url origin)" =~ github\.com[:/]$repo(\.git)?$ ]] || fail "origin is $(git remote get-url origin), not $repo: git remote set-url origin https://github.com/$repo.git"
git fetch --quiet --tags origin main || fail "can't fetch origin ($repo)"
problems=()
[[ -z "$(git status --porcelain)" ]] || problems+=("the working tree has uncommitted changes: commit or remove them")
git merge-base --is-ancestor HEAD origin/main || problems+=("HEAD $(git rev-parse --short HEAD) isn't on origin/main: merge it into main and push, then release from main")
if [[ ${#problems[@]} -gt 0 ]]; then
  if $dry_run; then
    for p in "${problems[@]}"; do echo "    warning: $p (allowed in a dry run)"; done
  else
    for p in "${problems[@]}"; do echo "release: $p" >&2; done
    exit 1
  fi
fi

identity="$(security find-identity -v -p codesigning | sed -n 's/^ *[0-9]*) \([0-9A-F]\{40\}\) "\(Developer ID Application: .*\)"$/\1 \2/p' | head -1)"
[[ -n "$identity" ]] || fail "no \"Developer ID Application\" identity in the keychain: install the certificate and its private key (security find-identity -v -p codesigning)"
sign_hash="${identity%% *}"
sign_name="${identity#* }"
echo "    signing as $sign_name"

[[ -f "$release_env" ]] || fail "$release_env is missing: it names the notary credential (APPLE_KEYCHAIN_PROFILE, APPLE_KEYCHAIN_PATH); run Kanna v2's ./kd release setup-notarization"
env_value() { sed -n "s/^$1=\"\{0,1\}\([^\"]*\)\"\{0,1\}[[:space:]]*$/\1/p" "$release_env" | tail -1; }
notary_profile="$(env_value APPLE_KEYCHAIN_PROFILE)"
notary_keychain="$(env_value APPLE_KEYCHAIN_PATH)"
[[ -n "$notary_profile" ]] || fail "$release_env doesn't set APPLE_KEYCHAIN_PROFILE"
[[ -n "$notary_keychain" ]] || fail "$release_env doesn't set APPLE_KEYCHAIN_PATH"
[[ -f "$notary_keychain" ]] || fail "the keychain $notary_keychain (APPLE_KEYCHAIN_PATH) doesn't exist"
notary=(--keychain-profile "$notary_profile" --keychain "$notary_keychain")
xcrun notarytool history "${notary[@]}" >/dev/null 2>&1 ||
  fail "notarytool can't use profile \"$notary_profile\" in $notary_keychain: unlock that keychain, or save the credential again with xcrun notarytool store-credentials"
echo "    notarizing with profile $notary_profile"

gh_status="$(gh auth status --active 2>&1)" || fail "gh can't reach GitHub as a signed-in account (run gh auth login if it isn't signed in):
$(grep -v -i token <<<"$gh_status")"
command -v claude >/dev/null || fail "the claude CLI isn't installed: it writes the release notes"

release_tags() { git tag -l 'v*' | { grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true; } | sort -V; }
last_tag="$(release_tags | tail -1)"
if [[ -n "$explicit" ]]; then
  version="$explicit"
  [[ -z "$last_tag" || "$(printf '%s\n%s\n' "${last_tag#v}" "$version" | sort -V | tail -1)" == "$version" && "${last_tag#v}" != "$version" ]] ||
    fail "$version isn't newer than the last release, $last_tag"
else
  [[ -n "$last_tag" ]] || fail "there's no v* tag yet: give the first version, e.g. scripts/release.sh 4.0.0"
  IFS=. read -r major minor patch <<<"${last_tag#v}"
  case "${bump:-patch}" in
    major) version="$((major + 1)).0.0" ;;
    minor) version="$major.$((minor + 1)).0" ;;
    patch) version="$major.$minor.$((patch + 1))" ;;
  esac
fi
remote_tags="$(git ls-remote --tags origin 'refs/tags/v*' | sed 's#.*refs/tags/##')"
if $staging; then
  channel=staging
  n="$( { printf '%s\n' "$remote_tags"; git tag -l "v$version-staging.*"; } | sed -n "s/^v${version//./\\.}-staging\.\([0-9]*\)$/\1/p" | sort -n | tail -1)"
  version="$version-staging.$((${n:-0} + 1))"
  app_name="Kanna Staging"
  dmg_prefix="Kanna-Staging"
else
  channel=production
  app_name="Kanna"
  dmg_prefix="Kanna"
fi
tag="v$version"
head="$(git rev-parse HEAD)"
if grep -qx "$tag" <<<"$remote_tags"; then
  [[ "$(git rev-parse "$tag^{commit}" 2>/dev/null)" == "$head" ]] || fail "$tag is already on origin at another commit"
  echo "    $tag is already on origin at HEAD: carrying on with its release"
fi
[[ "$(gh api "repos/$repo" --jq .full_name)" == "$repo" ]] || fail "$repo resolves to another repo"
gh release view "$tag" --repo "$repo" >/dev/null 2>&1 && fail "$repo already has a release $tag"
build_number="$(git rev-list --count HEAD)"
echo "    $app_name $version (build $build_number) from ${last_tag:-the start} to $(git rev-parse --short HEAD)"

dist="$repo_root/target/release-dmg/$tag"
rm -rf "$dist"
mkdir -p "$dist"

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

verify_output="$dist/verification.txt"
: >"$verify_output"
dmgs=()
symbols=()
for arch in "${archs[@]}"; do
  step "Build $app_name $version for $arch"
  KANNA_VERSION="$version" "$repo_root/scripts/build.sh" "$channel" --arch "$arch"
  triple="$(. "$repo_root/scripts/env.sh" && kanna_arch_triple "$arch")"
  if [[ "$triple" == "$(. "$repo_root/scripts/env.sh" && kanna_host_triple)" ]]; then built="$repo_root/target/Kanna.app"; else built="$repo_root/target/$triple/Kanna.app"; fi

  stage="$dist/$arch"
  rm -rf "$stage"
  mkdir -p "$stage"
  ditto "$built" "$stage/$app_name.app"
  app="$stage/$app_name.app"
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" == "${version%%-*}" ]] || fail "$app doesn't have version ${version%%-*}"
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
    [[ "$(dsymutil -s "$binary" | grep -c N_OSO)" == 0 ]] || fail "$binary still has a debug map after strip -S"
  done
  zip="$dist/$dmg_prefix-$version-$arch-dSYMs.zip"
  ditto -c -k --norsrc --noextattr "$dist/dSYMs/$arch" "$zip"
  symbols+=("$zip")

  step "Sign $arch"
  while IFS= read -r -d '' helper; do
    file -b "$helper" | grep -q Mach-O || continue
    codesign --force --sign "$sign_hash" --options runtime --timestamp "$helper"
  done < <(find "$app/Contents/Helpers" -type f -print0)
  stray="$(find "$app/Contents" -type f ! -path "*/Contents/Helpers/*" ! -path "*/Contents/MacOS/Kanna" -exec sh -c 'file -b "$1" | grep -q Mach-O && echo "$1"' _ {} \;)"
  [[ -z "$stray" ]] || fail "Mach-O files outside Contents/Helpers would go unsigned: $stray"
  codesign --force --sign "$sign_hash" --options runtime --timestamp --entitlements "$repo_root/apps/kanna/Resources/Kanna.entitlements" "$app"
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

step "Release notes"
if [[ -n "$last_tag" ]]; then range="$last_tag..HEAD"; else range="HEAD"; fi
subjects="$(git log --merges --first-parent --pretty=format:'%s' "$range")"
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
notes="$(claude --print --model haiku "$prompt" <<<"$subjects")" || fail "claude couldn't write the release notes"
[[ -n "$notes" ]] || fail "claude wrote empty release notes"
printf '%s\n' "$notes" >"$dist/notes.md"
echo "---"
printf '%s\n' "$notes"
echo "---"

for dmg in "${dmgs[@]}" "${symbols[@]}"; do [[ "$(basename "$dmg")" != latest.json ]] || fail "refusing to publish an asset named latest.json"; done

if $dry_run; then
  step "Dry run: built, signed, notarized and verified; nothing tagged, pushed or published"
  for dmg in "${dmgs[@]}" "${symbols[@]}"; do echo "    $dmg"; done
  echo "    verification: $verify_output"
  exit 0
fi

step "Tag $tag on origin"
if ! grep -qx "$tag" <<<"$remote_tags"; then
  git tag -a "$tag" -m "Kanna $tag" "$head"
  git push --quiet origin "refs/tags/$tag"
fi

step "Release $tag on $repo"
flags=(--repo "$repo" --verify-tag --title "Kanna $tag" --notes-file "$dist/notes.md")
$staging && flags+=(--prerelease)
gh release create "$tag" "${dmgs[@]}" "${symbols[@]}" "${flags[@]}" >/dev/null
assets="$(gh release view "$tag" --repo "$repo" --json assets --jq '.assets[].name')"
! grep -qx latest.json <<<"$assets" || fail "$tag has an asset named latest.json: delete it now, Kanna v2 installs read it"
url="$(gh release view "$tag" --repo "$repo" --json url --jq .url)"
step "Published $url"
printf '%s\n' "$assets" | sed 's/^/    /'
