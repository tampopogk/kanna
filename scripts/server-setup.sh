#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
usage: scripts/server-setup.sh [options]

Sets up Kanna on a Linux server (Ubuntu) for the user who runs it: packages, the agent
CLIs, the daemon, keeper and CLI installed in /opt/kanna/<commit> with /opt/kanna/current
linked to it (from a release tarball, or built from this checkout with Rust), and a
systemd user unit that runs the daemon at boot (linger). docs/specs/fleet.md §9 says
what it sets up and why. Safe to run again: each step is skipped when already done, and
an update restarts the daemon, whose terminals the keeper keeps. Later updates come with
kanna fleet update from another machine (docs/specs/fleet-update.md).

  --agents LIST    agent CLIs to install with npm, comma-separated: claude, codex
                   (default: claude,codex; none installs none)
  --no-packages    don't install packages with apt
  --tarball FILE   install this Linux tarball (scripts/build-linux.sh, or a release's)
                   instead of building from this checkout
  --no-start       install the unit but don't start the daemon
USAGE
}

agents="claude,codex" packages=1 start=1 tarball=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --agents) agents="$2"; shift 2 ;;
    --no-packages) packages=0; shift ;;
    --no-start) start=0; shift ;;
    --tarball) tarball="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

fail() { echo "server-setup: $*" >&2; exit 1; }
step() { echo "==> $*"; }

[[ "$(uname -s)" == Linux ]] || fail "this is for Linux servers; on a Mac, Kanna.app runs the daemon"
[[ "$(id -u)" != 0 ]] || fail "run it as the user Kanna runs as, not root: it uses sudo where it needs to"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
user="$(id -un)"
prefix=/opt/kanna
state="${XDG_DATA_HOME:-$HOME/.local/share}/kanna-prod"
unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
bin_dir="$HOME/.local/bin"
user_path="$PATH"
export PATH="$bin_dir:$HOME/.cargo/bin:$PATH"

if [[ "$packages" == 1 ]]; then
  step "Packages"
  wanted=(git openssh-client build-essential pkg-config curl ca-certificates xz-utils sqlite3 jq bubblewrap socat nodejs npm)
  missing=()
  for p in "${wanted[@]}"; do dpkg-query -W -f '${Status}' "$p" 2>/dev/null | grep -q "install ok installed" || missing+=("$p"); done
  if [[ ${#missing[@]} -gt 0 ]]; then
    sudo apt-get update -q
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends "${missing[@]}"
  fi
fi

if [[ -z "$tarball" ]] && ! command -v cargo >/dev/null 2>&1; then
  step "Rust"
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path
fi

if [[ "$agents" != none ]]; then
  step "Agent CLIs: $agents"
  IFS=, read -r -a list <<< "$agents"
  for agent in "${list[@]}"; do
    case "$agent" in
      claude) package=@anthropic-ai/claude-code ;;
      codex) package=@openai/codex ;;
      *) fail "unknown agent $agent: use claude or codex" ;;
    esac
    command -v "$agent" >/dev/null 2>&1 || npm install -g -q --prefix "$HOME/.local" "$package"
  done
fi

if [[ ! -w "$prefix" ]]; then
  sudo install -d -o "$user" -g "$(id -gn)" -m 755 "$prefix"
fi
staged="$prefix/.setup.new"
rm -rf "$staged"
mkdir -p "$staged/kanna-skills"
if [[ -n "$tarball" ]]; then
  step "Unpack $(basename "$tarball")"
  tar -xzf "$tarball" -C "$staged" --strip-components=1 --no-same-owner
  commit="$(jq -r .commit "$staged/release.json")"
  [[ "$commit" =~ ^[0-9a-f]{40}(-dirty)?$ ]] || fail "$tarball's release.json names no commit"
  [[ "$(jq -r .os "$staged/release.json")-$(jq -r .arch "$staged/release.json")" == "linux-$(uname -m)" ]] || fail "$tarball is for $(jq -r '.os + " " + .arch' "$staged/release.json"), and this is linux $(uname -m)"
else
  step "Build"
  commit="$(git -C "$repo_root" rev-parse HEAD)"
  [[ -z "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]] || commit="$commit-dirty"
  "$repo_root/scripts/build-ghostty-vt.sh"
  KANNA_CHANNEL=production cargo build --release --manifest-path "$repo_root/Cargo.toml" -p kanna -p kanna-daemon -p kanna-keeper
  built="$repo_root/target/release"
  cp "$built/kanna" "$built/kanna-daemon" "$built/kanna-keeper" "$staged/"
  for skill in kanna-tasks kanna-planning kanna-new-project kanna-markup kanna-explain kanna-quiz; do cp -RL "$repo_root/.agents/kanna-skills/skills/$skill" "$staged/kanna-skills/"; done
  version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo_root/Cargo.toml" | head -1)"
  protocols="$(sed -n 's/^pub const PROTOCOLS: \[u32; 2\] = \[\([0-9]*\), \([0-9]*\)\];/[\1, \2]/p' "$repo_root/crates/kanna-build/src/lib.rs")"
  echo "{\"commit\": \"$commit\", \"version\": \"$version\", \"channel\": \"production\", \"os\": \"linux\", \"arch\": \"$(uname -m)\", \"protocols\": $protocols}" > "$staged/release.json"
fi

step "Install $prefix/$commit"
dest="$prefix/$commit"
rm -rf "$dest.new"
mv "$staged" "$dest.new"
if diff -r -q "$dest.new" "$dest" >/dev/null 2>&1; then
  rm -rf "$dest.new"
else
  rm -rf "$dest"
  mv "$dest.new" "$dest"
fi
was="$(readlink "$prefix/current" 2>/dev/null || true)"
if [[ -n "$was" && "$was" != "$commit" ]]; then
  ln -sfn "$was" "$prefix/previous.new"
  mv -T "$prefix/previous.new" "$prefix/previous"
fi
previous="$(readlink "$prefix/previous" 2>/dev/null || true)"
ln -sfn "$commit" "$prefix/current.new"
mv -T "$prefix/current.new" "$prefix/current"
mkdir -p "$bin_dir"
ln -sfn "$prefix/current/kanna" "$bin_dir/kanna"

step "Daemon unit"
mkdir -p "$unit_dir" "$state"
chmod 700 "$state"
unit="$unit_dir/kanna-daemon.service"
cat > "$unit.new" <<UNIT
[Unit]
Description=Kanna daemon
After=network-online.target

[Service]
Environment=KANNA_SOCKET=$state/kanna.sock
Environment=KANNA_KEEPER=$prefix/current/kanna-keeper
Environment=KANNA_AGENT_SKILLS=$prefix/current/kanna-skills
Environment=PATH=%h/.local/bin:%h/.cargo/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=$prefix/current/kanna-daemon
Restart=always
KillMode=process

[Install]
WantedBy=default.target
UNIT
changed=1
cmp -s "$unit.new" "$unit" && changed=0
mv "$unit.new" "$unit"

if [[ "$(loginctl show-user "$user" -P Linger 2>/dev/null)" != yes ]]; then
  sudo loginctl enable-linger "$user"
fi
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
for _ in $(seq 50); do [[ -S "$XDG_RUNTIME_DIR/systemd/private" ]] && break; sleep 0.1; done
[[ "$changed" == 1 ]] && systemctl --user daemon-reload
systemctl --user enable -q kanna-daemon.service
if [[ "$start" == 1 ]]; then
  running="$(systemctl --user show kanna-daemon.service -P MainPID)"
  if [[ "$changed" == 1 || "$running" == 0 ]] || ! [[ "/proc/$running/exe" -ef "$prefix/current/kanna-daemon" ]]; then
    systemctl --user restart kanna-daemon.service
  fi
  kanna() { KANNA_SOCKET="$state/kanna.sock" "$prefix/current/kanna" "$@"; }
  for _ in $(seq 100); do kanna agents status >/dev/null 2>&1 && break; sleep 0.1; done
  status="$(kanna agents status 2>&1)" && systemctl --user -q is-active kanna-daemon.service ||
    fail "the daemon didn't start: ${status:-no reply}; journalctl --user -u kanna-daemon"
  if [[ "$agents" != none && "$status" == "not set up yet"* ]]; then
    step "Kanna's skills for the agents"
    kanna agents setup
  fi
fi

for old in "$prefix"/*; do
  name="$(basename "$old")"
  [[ "$name" == current || "$name" == previous || "$name" == "$commit" || "$name" == "$previous" ]] && continue
  [[ -d "$old" && ! -L "$old" ]] && rm -rf "$old"
done

step "Done: Kanna $commit"
case ":$user_path:" in *":$bin_dir:"*) ;; *) echo "    add $bin_dir to PATH for kanna" ;; esac
echo "    kanna task list    # the daemon: systemctl --user status kanna-daemon"
