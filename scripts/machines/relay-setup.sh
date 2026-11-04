#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: sudo scripts/machines/relay-setup.sh --cert FILE --key FILE [--https-port 8444] [--quic-port 7843] [--new-token]

Installs your own iroh-relay on a Linux server with a stable address, as the kanna-relay
systemd unit: iroh-relay 1.4.1 (checked by SHA-256), admitting only clients that present
its shared token (a 256-bit secret, made on the first run or with --new-token), TLS from the
certificate and key you name (re-read whenever the certificate changes), run as a dynamic
unprivileged user with no capabilities. It prints the token file's path; each of your
machines then runs, with a copy of that file:

  kanna machine relay https://HOST:HTTPS-PORT --token-file FILE

Open TCP HTTPS-PORT and UDP QUIC-PORT to the internet; nothing else.
EOF
  exit 2
}

cert="" key="" https=8444 quic=7843 fresh=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --cert) cert="$2"; shift ;;
    --key) key="$2"; shift ;;
    --https-port) https="$2"; shift ;;
    --quic-port) quic="$2"; shift ;;
    --new-token) fresh=1 ;;
    *) usage ;;
  esac
  shift
done
[[ -n "$cert" && -n "$key" && $EUID -eq 0 ]] || usage
[[ -r "$cert" && -r "$key" ]] || { echo "can't read $cert or $key" >&2; exit 1; }

version=1.4.1
sha256=eeceef0f06f04ad2644644cd80dd5733e64da46daf8a9f86d87a5ad5c13b06e4
[[ "$(uname -m)" == x86_64 ]] || { echo "this script installs the x86_64 build; get iroh-relay $version for $(uname -m) from https://github.com/n0-computer/iroh/releases" >&2; exit 1; }
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl -fsSL -o "$work/relay.tar.gz" "https://github.com/n0-computer/iroh/releases/download/v$version/iroh-relay-v$version-x86_64-unknown-linux-gnu.tar.gz"
echo "$sha256  $work/relay.tar.gz" | sha256sum -c --quiet
tar -xzf "$work/relay.tar.gz" -C "$work"
install -m 0755 -o root -g root "$(find "$work" -type f -name iroh-relay | head -1)" /usr/local/bin/kanna-iroh-relay

install -d -m 0700 -o root -g root /etc/kanna-relay
token=/etc/kanna-relay/token
if [[ -n "$fresh" || ! -s "$token" ]]; then
  (umask 077 && head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$token")
fi
(umask 077 && cat >/etc/kanna-relay/relay.toml <<EOF
enable_relay = true
http_bind_addr = "127.0.0.1:0"
enable_quic_addr_discovery = true
enable_metrics = false
access.shared_token = ["$(cat "$token")"]

[tls]
https_bind_addr = "0.0.0.0:$https"
quic_bind_addr = "0.0.0.0:$quic"
cert_mode = "Manual"
manual_cert_path = "/run/credentials/kanna-relay.service/tls.crt"
manual_key_path = "/run/credentials/kanna-relay.service/tls.key"
EOF
)

cat >/etc/systemd/system/kanna-relay.service <<EOF
[Unit]
Description=Your machines' iroh relay (Kanna)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/kanna-iroh-relay --config-path \${CREDENTIALS_DIRECTORY}/relay.toml
LoadCredential=relay.toml:/etc/kanna-relay/relay.toml
LoadCredential=tls.crt:$cert
LoadCredential=tls.key:$key
DynamicUser=yes
CapabilityBoundingSet=
AmbientCapabilities=
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
ProtectProc=invisible
ProcSubset=pid
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources
UMask=0077
MemoryMax=256M
TasksMax=256
LimitNOFILE=65536
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

cat >/etc/systemd/system/kanna-relay-cert.path <<EOF
[Unit]
Description=Restart kanna-relay when its certificate is renewed

[Path]
PathChanged=$cert
Unit=kanna-relay-cert.service

[Install]
WantedBy=multi-user.target
EOF

cat >/etc/systemd/system/kanna-relay-cert.service <<'EOF'
[Unit]
Description=Restart kanna-relay for its renewed certificate

[Service]
Type=oneshot
ExecStart=/usr/bin/systemctl try-restart kanna-relay.service
EOF

systemctl daemon-reload
systemctl enable --now kanna-relay-cert.path
systemctl enable kanna-relay.service
systemctl restart kanna-relay.service
sleep 1
systemctl is-active --quiet kanna-relay.service || { journalctl -u kanna-relay --no-pager -n 30 >&2; exit 1; }
echo "kanna-relay is running on TCP $https and UDP $quic; its token is in $token"
