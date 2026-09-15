#!/bin/bash
set -uo pipefail
task_root=/home/jeremy/.tmp/kanna-d3ce8dec
export PATH=/home/jeremy/.local/node/bin:/home/jeremy/.local/bin:/usr/local/bin:/usr/bin:/bin
export HOME=/root
unset XDG_CONFIG_HOME
export TMPDIR="$task_root/.tmp"
export COREPACK_HOME="$task_root/.tmp/corepack"
export XDG_CACHE_HOME="$task_root/.tmp/cache"
export XDG_RUNTIME_DIR=/run/user/0
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/0/bus
test "$(id -u)" = 0 || exit 90
printf 'actual-test-identity='; id
printf 'actual-user-manager='; systemctl --user is-system-running --wait
manager_state=$(systemctl --user is-system-running || true)
case "$manager_state" in running|degraded) ;; *) echo 'Real root user manager unavailable'; exit 93;; esac
cd "$task_root/source" || exit 94
./kd test linux-installed \
  --old-artifact "$task_root/artifacts/kanna-staging_0.2.0~staging.1-1_arm64.deb" \
  --new-artifact "$task_root/artifacts/kanna-staging_0.2.0~staging.2-1_arm64.deb" \
  --channel staging
gate_status=$?
printf 'canonical-gate-exit=%s\n' "$gate_status"
printf 'remaining-test-unit-files:\n'
find /root/.config/systemd/user -maxdepth 1 -name 'kanna-installed-test-*' -print 2>/dev/null
printf 'remaining-installed-processes:\n'
ps -eo pid,ppid,args | grep '/usr/lib/kanna-staging/' | grep -v grep || true
printf 'installed-package-after:\n'
dpkg-query -W -f='${Package} ${Version}\n' kanna-staging 2>/dev/null || true
printf 'root-user-active-services:\n'
systemctl --user list-units --type=service --state=running --no-pager
echo "$gate_status" > "$task_root/arm-gate.exit"
exit "$gate_status"
