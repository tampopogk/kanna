#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$repo_root/scripts/env.sh"
case "${1:-}" in
  "") ;;
  -h|--help)
    cat <<'USAGE'
usage: scripts/dev-signing-identity.sh

Makes "Kanna Dev Signing", a self-signed code-signing certificate and key in the login keychain,
once, for a Mac without an Apple Development certificate of the team in
~/.kanna/developer/signing.json. scripts/build-kanna-app.sh
signs dev builds with it then, so macOS privacy grants survive rebuilds; Keychain grants don't.
USAGE
    exit 0 ;;
  *) echo "usage: scripts/dev-signing-identity.sh" >&2; exit 2 ;;
esac
if team="$(kanna_signing_team)" && [[ -n "$team" ]] && identity="$(kanna_team_identity "Apple Development" "$team")" && [[ -n "$identity" ]]; then
  echo "Not needed: dev builds are signed with Apple Development ($identity, team $team)"
  exit 0
fi
if identity="$(kanna_selfsigned_identity)" && [[ -n "$identity" ]]; then
  echo "Kanna Dev Signing is already in the keychain ($identity)"
  exit 0
fi

dir="$(mktemp -d "${TMPDIR:-/tmp}/kanna-signing.XXXXXX")"
trap 'rm -rf "$dir"' EXIT
cat >"$dir/req.cnf" <<'CNF'
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext
[dn]
CN = Kanna Dev Signing
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
CNF
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 7300 -config "$dir/req.cnf" -keyout "$dir/key.pem" -out "$dir/cert.pem" 2>/dev/null
password="$(/usr/bin/openssl rand -hex 16)"
/usr/bin/openssl pkcs12 -export -inkey "$dir/key.pem" -in "$dir/cert.pem" -name "Kanna Dev Signing" -passout "pass:$password" -out "$dir/identity.p12"
keychain="$(security default-keychain -d user | tr -d ' "')"
security import "$dir/identity.p12" -k "$keychain" -f pkcs12 -P "$password" -T /usr/bin/codesign
identity="$(kanna_selfsigned_identity)"
[[ -n "$identity" ]] || { echo "dev-signing-identity: imported into $keychain, but codesign can't find it" >&2; exit 1; }
echo "Made Kanna Dev Signing ($identity) in $keychain: scripts/build.sh signs dev builds with it from now on"
