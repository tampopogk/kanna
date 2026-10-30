#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

usage() {
  cat <<'USAGE'
usage: scripts/cloud/provision-idrac.sh --idrac ADDR --hostname NAME [options]

Installs Ubuntu Server on a Dell PowerEdge through its iDRAC alone: RAID 1 on the PERC,
the ISO and an autoinstall config served from this Mac over a temporary SMB share,
one boot from virtual media, an unattended install, then waits until ssh works.
docs/specs/cloud-provisioning.md says what it does and what is left to do by hand.

  --idrac ADDR        the iDRAC's address
  --hostname NAME     the host's name; also names the Keychain item "kanna-cloud idrac NAME"
  --wifi-ssid SSID    join this Wi-Fi network; its password is read from the Keychain item
                      service "kanna-cloud wifi", account SSID
  --user NAME         the host's user (default: this Mac's user)
  --pubkey FILE       the key that user logs in with (default: ~/.ssh/id_ed25519.pub)
  --iso FILE          the Ubuntu live-server ISO (default: downloads 26.04.1 to the cache)
  --reset-raid        delete the controller's virtual disks when they aren't one RAID 1
  --reinstall         install again over a host that already answers ssh
  --yes               don't ask before erasing the disks
  --check             only show the iDRAC, storage and power state; change nothing

Every step is skipped when already done: the iDRAC password and key, the RAID volume,
the install (a host that answers ssh as NAME).
USAGE
}

idrac="" name="" ssid="" user="$(id -un)" pubkey="$HOME/.ssh/id_ed25519.pub" iso=""
reset_raid=0 reinstall=0 yes=0 check=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --idrac) idrac="$2"; shift 2 ;;
    --hostname) name="$2"; shift 2 ;;
    --wifi-ssid) ssid="$2"; shift 2 ;;
    --user) user="$2"; shift 2 ;;
    --pubkey) pubkey="$2"; shift 2 ;;
    --iso) iso="$2"; shift 2 ;;
    --reset-raid) reset_raid=1; shift ;;
    --reinstall) reinstall=1; shift ;;
    --yes) yes=1; shift ;;
    --check) check=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "provision-idrac: unknown argument $1" >&2; usage >&2; exit 2 ;;
  esac
done
[[ -n "$idrac" && -n "$name" ]] || { usage >&2; exit 2; }
[[ -f "$pubkey" ]] || { echo "provision-idrac: no public key at $pubkey" >&2; exit 1; }

ubuntu_iso_url="https://releases.ubuntu.com/26.04/ubuntu-26.04.1-live-server-amd64.iso"
ubuntu_iso_sha256="cc8a95cde20f6ced61a322420de00f10cc3c90ced545daa46cb9c1a117f1d927"
cache="$HOME/Library/Caches/kanna-cloud"
state="$HOME/Library/Application Support/kanna-cloud"
idrac_key="$HOME/.ssh/kanna-cloud-idrac"
keychain_idrac="kanna-cloud idrac $name"
keychain_wifi="kanna-cloud wifi"
work="$cache/$name"

log() { printf '%s %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die() { log "error: $*"; exit 1; }

former="kanna-fleet"
move_former() {
  if [[ -e "$1" && ! -e "$2" ]]; then
    mv "$1" "$2" && log "moved $1 to $2"
  fi
}
copy_former_keychain() {
  local password
  security find-generic-password -s "$2" -a "$3" >/dev/null 2>&1 && return 0
  password="$(security find-generic-password -s "$1" -a "$3" -w 2>/dev/null)" || return 0
  security add-generic-password -s "$2" -a "$3" -w "$password" && log "copied the Keychain item \"$1\" to \"$2\""
}
move_former "$HOME/Library/Caches/$former" "$cache"
move_former "$HOME/Library/Application Support/$former" "$state"
move_former "$HOME/.ssh/$former-idrac" "$idrac_key"
move_former "$HOME/.ssh/$former-idrac.pub" "$idrac_key.pub"
copy_former_keychain "$former idrac $name" "$keychain_idrac" root
[[ -z "$ssid" ]] || copy_former_keychain "$former wifi" "$keychain_wifi" "$ssid"

mkdir -p "$cache" "$state" "$work"
chmod 700 "$work"

ssh_opts=(-o ConnectTimeout=15 -o ServerAliveInterval=15 -o StrictHostKeyChecking=accept-new -o LogLevel=ERROR)
racadm() { ssh "${ssh_opts[@]}" -i "$idrac_key" -o IdentitiesOnly=yes -o BatchMode=yes "root@$idrac" racadm "$@" 2>&1 | tr -d '\r'; }

racadm_with_password() {
  local password="$1"; shift
  local askpass; askpass="$(mktemp "$work/askpass.XXXXXX")"
  printf '#!/bin/sh\nprintf "%%s\\n" "$KANNA_CLOUD_PW"\n' >"$askpass"
  chmod 700 "$askpass"
  KANNA_CLOUD_PW="$password" SSH_ASKPASS="$askpass" SSH_ASKPASS_REQUIRE=force \
    ssh "${ssh_opts[@]}" -o PubkeyAuthentication=no -o PreferredAuthentications=password,keyboard-interactive \
    "root@$idrac" racadm "$@" </dev/null 2>&1 | tr -d '\r' || true
  rm -f "$askpass"
}

idrac_access() {
  if [[ -f "$idrac_key" ]] && racadm getsysinfo -s | grep -q 'System Model'; then
    log "iDRAC $idrac: key login works"
    return
  fi
  [[ -f "$idrac_key" ]] || ssh-keygen -q -t rsa -b 4096 -N '' -C "kanna-cloud idrac $(id -un)@$(hostname -s)" -f "$idrac_key"
  local password
  if password="$(security find-generic-password -s "$keychain_idrac" -a root -w 2>/dev/null)"; then
    racadm_with_password "$password" getsysinfo -s | grep -q 'System Model' \
      || die "the Keychain password for \"$keychain_idrac\" doesn't log in to $idrac"
  else
    racadm_with_password calvin getsysinfo -s | grep -q 'System Model' \
      || die "$idrac takes neither key nor root/calvin; add its root password to the Keychain: security add-generic-password -s \"$keychain_idrac\" -a root -w"
    password="$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 20)"
    security add-generic-password -s "$keychain_idrac" -a root -w "$password" -j "iDRAC root at $idrac" \
      || die "couldn't store the new password in the Keychain"
    racadm_with_password calvin set iDRAC.Users.2.Password "$password" | grep -q 'modified successfully' \
      || die "couldn't set the iDRAC password (it is in the Keychain as \"$keychain_idrac\")"
    log "iDRAC $idrac: root password set and stored in the Keychain as \"$keychain_idrac\""
    sleep 5
  fi
  racadm_with_password "$password" sshpkauth -i 2 -k 1 -t "\"$(cat "$idrac_key.pub")\"" | grep -q 'successfully' \
    || die "couldn't add $idrac_key.pub to the iDRAC root user"
  racadm getsysinfo -s | grep -q 'System Model' || die "key login to $idrac still fails"
  log "iDRAC $idrac: key $idrac_key.pub added to root"
}

power_state() { racadm serveraction powerstatus | sed -n 's/.*power status: *//Ip' | tr '[:upper:]' '[:lower:]'; }

wait_power() {
  local want="$1" timeout="$2" start=$SECONDS
  until [[ "$(power_state)" == "$want" ]]; do
    (( SECONDS - start < timeout )) || return 1
    sleep 20
  done
}

show_storage() {
  racadm storage get controllers -o -p Name,FirmwareVersion
  racadm storage get pdisks -o -p State,MediaType,Size,FailurePredicted
  racadm storage get vdisks -o -p Name,Layout,Size,State
}

storage() {
  local controller pdisks vdisks
  controller="$(racadm storage get controllers | grep -m1 '^RAID\.' || true)"
  [[ -n "$controller" ]] || die "no RAID controller found"
  pdisks=($(racadm storage get pdisks | grep '^Disk\.'))
  vdisks=($(racadm storage get vdisks | grep '^Disk\.Virtual' || true))
  (( ${#pdisks[@]} == 2 )) || die "expected two drives on $controller, found ${#pdisks[@]}"
  if (( ${#vdisks[@]} == 1 )); then
    local info; info="$(racadm storage get vdisks:"${vdisks[0]}" -p Layout,State)"
    if grep -q 'Raid-1' <<<"$info" && grep -q 'Online' <<<"$info"; then
      log "storage: ${vdisks[0]} is already RAID 1 across both drives"
      return
    fi
  fi
  if (( ${#vdisks[@]} > 0 )); then
    (( reset_raid )) || die "the controller has virtual disks that aren't one RAID 1 (above); rerun with --reset-raid to delete them"
    confirm "delete every virtual disk on $controller"
    racadm storage resetconfig:"$controller"
  fi
  local list; list="$(IFS=,; echo "${pdisks[*]}")"
  racadm storage createvd:"$controller" -rl r1 -wp wb -rp ara -name "$name" -pdkey:"$list"
  racadm jobqueue create "$controller" -s TIME_NOW -r pwrcycle | tee "$work/raid-job.txt" >&2
  local job; job="$(grep -o 'JID_[0-9]*' "$work/raid-job.txt" | head -1)"
  [[ -n "$job" ]] || die "couldn't create the RAID job"
  log "storage: RAID 1 job $job runs on the next boot; waiting"
  local start=$SECONDS
  until racadm jobqueue view -i "$job" | grep -q 'Status=Completed'; do
    racadm jobqueue view -i "$job" | grep -q 'Status=Failed' && die "RAID job $job failed"
    (( SECONDS - start < 1800 )) || die "RAID job $job didn't finish in 30 minutes"
    sleep 30
  done
  log "storage: RAID 1 created"
}

confirm() {
  (( yes )) && return
  [[ -t 0 ]] || die "$1: rerun with --yes to confirm"
  local answer
  read -r -p "$1 on $idrac ($name)? type $name to go on: " answer
  [[ "$answer" == "$name" ]] || die "not confirmed"
}

ubuntu_iso() {
  if [[ -z "$iso" ]]; then
    iso="$cache/$(basename "$ubuntu_iso_url")"
    if [[ ! -f "$iso.ok" ]]; then
      log "downloading $ubuntu_iso_url"
      curl -fL -C - -o "$iso" "$ubuntu_iso_url"
      echo "$ubuntu_iso_sha256  $iso" | shasum -a 256 -c >/dev/null || { rm -f "$iso"; die "the ISO's checksum doesn't match"; }
      touch "$iso.ok"
    fi
  fi
  [[ -f "$iso" ]] || die "no ISO at $iso"
}

user_data() {
  local wifi_password=""
  if [[ -n "$ssid" ]]; then
    wifi_password="$(security find-generic-password -s "$keychain_wifi" -a "$ssid" -w 2>/dev/null)" \
      || die "no Wi-Fi password for \"$ssid\" in the Keychain; add it: security add-generic-password -s \"$keychain_wifi\" -a \"$ssid\" -w"
  fi
  KF_NAME="$name" KF_USER="$user" KF_KEY="$(cat "$pubkey")" KF_SSID="$ssid" KF_WIFI="$wifi_password" \
    KF_REPORT="$(report_script)" python3 -I - <<'PY'
import base64, json, os
env = os.environ
ethernets = {"lan": {"match": {"name": "en*"}, "dhcp4": True, "dhcp-identifier": "mac",
                     "dhcp4-overrides": {"route-metric": 100}, "optional": True}}
network = {"version": 2, "ethernets": ethernets}
if env["KF_SSID"]:
    network["wifis"] = {"@WIFI_IFACE@": {"dhcp4": True, "dhcp-identifier": "mac",
                                 "dhcp4-overrides": {"route-metric": 600}, "optional": True,
                                 "access-points": {env["KF_SSID"]: {"password": env["KF_WIFI"]}}}}
report = env["KF_REPORT"]
unit = """[Unit]
Description=Report the address on the serial console
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/local/sbin/kanna-cloud-report

[Install]
WantedBy=multi-user.target
"""
late = [
    "mkdir -p /target/usr/local/sbin",
    "echo " + base64.b64encode(report.encode()).decode() + " | base64 -d > /target/usr/local/sbin/kanna-cloud-report",
    "chmod 755 /target/usr/local/sbin/kanna-cloud-report",
    "echo " + base64.b64encode(unit.encode()).decode() + " | base64 -d > /target/etc/systemd/system/kanna-cloud-report.service",
    "curtin in-target -- systemctl enable kanna-cloud-report.service",
    "sed -i 's/^GRUB_CMDLINE_LINUX=.*/GRUB_CMDLINE_LINUX=\"console=tty0 console=ttyS0,115200n8\"/' /target/etc/default/grub",
    "curtin in-target -- update-grub",
]
config = {"autoinstall": {
    "version": 1,
    "interactive-sections": [],
    "locale": "en_US.UTF-8",
    "keyboard": {"layout": "us"},
    "timezone": "Etc/UTC",
    "refresh-installer": {"update": False},
    "source": {"id": "ubuntu-server-minimal"},
    "apt": {"fallback": "offline-install", "geoip": False},
    "early-commands": ["sh -c 'i=$(ls /sys/class/net | grep -m1 ^wl); sed -i \"s/@WIFI_IFACE@/${i:-wlan0}/\" /autoinstall.yaml'"],
    "network": network,
    "storage": {"layout": {"name": "direct"}},
    "ssh": {"install-server": True, "allow-pw": False},
    "packages": ["wpasupplicant"],
    "late-commands": late,
    "user-data": {
        "hostname": env["KF_NAME"],
        "preserve_hostname": False,
        "users": [{"name": env["KF_USER"], "groups": ["adm", "sudo"], "shell": "/bin/bash",
                   "sudo": "ALL=(ALL) NOPASSWD:ALL", "lock_passwd": True,
                   "ssh_authorized_keys": [env["KF_KEY"]]}],
        "ssh_pwauth": False,
        "disable_root": True,
    },
    "shutdown": "poweroff",
}}
print("#cloud-config")
print(json.dumps(config, indent=1))
PY
}

report_script() {
  cat <<'SH'
#!/bin/sh
for i in $(seq 1 60); do
  dev=$(ip -4 route show default | awk '{print $5; exit}')
  [ -n "$dev" ] && break
  sleep 5
done
out=/dev/ttyS0
if [ -n "$dev" ]; then
  ip=$(ip -4 -o addr show dev "$dev" scope global | awk '{split($4,a,"/"); print a[1]; exit}')
  mac=$(cat /sys/class/net/"$dev"/address)
  echo "KANNA-CLOUD-READY host=$(hostname) ip=$ip mac=$mac dev=$dev" >$out
else
  echo "KANNA-CLOUD-NONET host=$(hostname)" >$out
  ip -br link >$out
  lsusb >$out 2>&1
fi
SH
}

seed_iso() {
  local out="$work/install.iso"
  rm -rf "$work/nocloud" "$work/grub.cfg" "$out"
  mkdir -p "$work/nocloud"
  user_data >"$work/nocloud/user-data"
  printf 'instance-id: %s-%s\n' "$name" "$(date +%s)" >"$work/nocloud/meta-data"
  xorriso -osirrox on -indev "$iso" -extract /boot/grub/grub.cfg "$work/grub.cfg" >/dev/null 2>&1
  chmod u+w "$work/grub.cfg"
  sed -i '' -e 's/^set timeout=.*/set timeout=1/' \
    -e 's#linux[[:space:]]*/casper/vmlinuz.*#linux /casper/vmlinuz autoinstall ds=nocloud\\;s=/cdrom/nocloud/ console=tty0 console=ttyS0,115200n8 ---#' \
    "$work/grub.cfg"
  grep -q 'autoinstall' "$work/grub.cfg" || die "couldn't add autoinstall to the ISO's grub.cfg"
  log "building $out"
  xorriso -indev "$iso" -outdev "$out" -map "$work/grub.cfg" /boot/grub/grub.cfg \
    -map "$work/nocloud" /nocloud -boot_image any replay >"$work/xorriso.log" 2>&1 \
    || { cat "$work/xorriso.log" >&2; die "xorriso failed"; }
  rm -rf "$work/nocloud"
}

smbd_pid=""
share_start() {
  local smbd=/opt/homebrew/sbin/samba-dot-org-smbd dir="$cache/smb"
  [[ -x "$smbd" ]] || die "Samba is missing: brew install samba"
  if lsof -nP -iTCP:445 -sTCP:LISTEN >/dev/null 2>&1; then
    die "something on this Mac already listens on port 445 (File Sharing?); turn it off or stop it"
  fi
  mkdir -p "$dir"/{lock,state,cache,private,pid,log,ncalrpc}
  cat >"$dir/smb.conf" <<EOF
[global]
  server role = standalone server
  netbios name = KCLOUD
  smb ports = 445
  lock directory = $dir/lock
  state directory = $dir/state
  cache directory = $dir/cache
  private dir = $dir/private
  pid directory = $dir/pid
  ncalrpc dir = $dir/ncalrpc
  log file = $dir/log/smbd.log
  passdb backend = tdbsam:$dir/private/passdb.tdb
  map to guest = Bad User
  guest account = $(id -un)
  hosts allow = $idrac
  load printers = no
  printcap name = /dev/null
  disable spoolss = yes
[kcloud]
  path = $work
  read only = yes
  guest ok = yes
  guest only = yes
EOF
  "$smbd" -s "$dir/smb.conf" -F >/dev/null 2>&1 &
  smbd_pid=$!
  sleep 3
  kill -0 "$smbd_pid" 2>/dev/null || die "smbd didn't start; see $dir/log/smbd.log"
  log "SMB share //$mac_ip/kcloud up (pid $smbd_pid, only $idrac may connect)"
}

share_stop() {
  [[ -n "$smbd_pid" ]] || return 0
  kill "$smbd_pid" 2>/dev/null || true
  wait "$smbd_pid" 2>/dev/null || true
  smbd_pid=""
}

sol_pid=""
sol_start() {
  sol_stop
  ssh "${ssh_opts[@]}" -tt -i "$idrac_key" -o IdentitiesOnly=yes -o BatchMode=yes "root@$idrac" console com2 \
    </dev/null >>"$work/console.log" 2>&1 &
  sol_pid=$!
}

sol_stop() {
  [[ -n "$sol_pid" ]] || return 0
  kill "$sol_pid" 2>/dev/null || true
  wait "$sol_pid" 2>/dev/null || true
  sol_pid=""
}

cleanup() {
  sol_stop
  share_stop
  if [[ "${media_attached:-0}" == 1 ]]; then racadm remoteimage -d >/dev/null || true; fi
  rm -f "$work/install.iso" "$work"/askpass.*
  if [[ -n "$ssid" && -f "$work/console.log" ]]; then
    local pw; pw="$(security find-generic-password -s "$keychain_wifi" -a "$ssid" -w 2>/dev/null || true)"
    if [[ -n "$pw" ]]; then KF_PW="$pw" perl -pi -e 's/\Q$ENV{KF_PW}\E/********/g' "$work/console.log"; fi
  fi
}
trap cleanup EXIT

host_ready() {
  local addr="$1"
  [[ -n "$addr" ]] || return 1
  ssh -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -o LogLevel=ERROR \
    "$user@$addr" hostname 2>/dev/null | grep -qx "$name"
}

known_address() { sed -n 's/^ip=//p' "$state/$name.env" 2>/dev/null || true; }

install_host() {
  mac_ip="$(ipconfig getifaddr "$(route -n get "$idrac" | awk '/interface:/{print $2}')")"
  [[ -n "$mac_ip" ]] || die "couldn't find this Mac's address toward $idrac"
  ubuntu_iso
  seed_iso
  share_start

  if [[ "$(power_state)" != "off" ]]; then
    confirm "power off the server and erase its disks"
    racadm serveraction powerdown >/dev/null
    wait_power off 300 || die "the server didn't power off"
  else
    confirm "erase the server's disks"
  fi

  racadm remoteimage -d >/dev/null || true
  racadm remoteimage -c -u guest -p guest -l "//$mac_ip/kcloud/install.iso" | grep -q 'Configured' \
    || die "the iDRAC couldn't mount //$mac_ip/kcloud/install.iso; see $cache/smb/log/smbd.log"
  media_attached=1
  racadm set iDRAC.ServerBoot.FirstBootDevice VCD-DVD >/dev/null
  racadm set iDRAC.ServerBoot.BootOnce Enabled >/dev/null
  : >"$work/console.log"
  sol_start
  racadm serveraction powerup >/dev/null
  log "booting the installer; the serial console goes to $work/console.log"
  wait_power on 120 || die "the server didn't power on"

  local start=$SECONDS last=""
  while [[ "$(power_state)" != "off" ]]; do
    (( SECONDS - start < 7200 )) || die "the install didn't finish in two hours; see $work/console.log"
    kill -0 "$sol_pid" 2>/dev/null || sol_start
    local line; line="$(tr -d '\r' <"$work/console.log" | grep -aE 'subiquity|curtin|finish|install' | tail -1 | cut -c1-120 || true)"
    [[ -n "$line" && "$line" != "$last" ]] && { log "installer: $line"; last="$line"; }
    sleep 30
  done
  log "the installer powered the server off"
  racadm remoteimage -d >/dev/null || true
  media_attached=0
  share_stop
  rm -f "$work/install.iso"

  sol_start
  racadm serveraction powerup >/dev/null
  log "first boot from the disk"
  local ready="" start=$SECONDS
  until ready="$(tr -d '\r' <"$work/console.log" | grep -aoE 'KANNA-CLOUD-(READY|NONET)[^[:cntrl:]]*' | tail -1)" && [[ -n "$ready" ]]; do
    (( SECONDS - start < 1200 )) || die "the host didn't report an address in 20 minutes; see $work/console.log"
    kill -0 "$sol_pid" 2>/dev/null || sol_start
    sleep 15
  done
  sol_stop
  log "host: $ready"
  [[ "$ready" == KANNA-CLOUD-READY* ]] || die "the host has no network; its interfaces and USB devices are at the end of $work/console.log"
  local ip mac dev
  ip="$(sed -n 's/.* ip=\([^ ]*\).*/\1/p' <<<"$ready")"
  mac="$(sed -n 's/.* mac=\([^ ]*\).*/\1/p' <<<"$ready")"
  dev="$(sed -n 's/.* dev=\([^ ]*\).*/\1/p' <<<"$ready")"
  printf 'ip=%s\nmac=%s\ndev=%s\nidrac=%s\n' "$ip" "$mac" "$dev" "$idrac" >"$state/$name.env"
  ssh-keygen -R "$ip" >/dev/null 2>&1 || true
  ssh-keygen -R "$name" >/dev/null 2>&1 || true
  local start=$SECONDS
  until host_ready "$ip"; do
    (( SECONDS - start < 600 )) || die "ssh $user@$ip doesn't work yet"
    sleep 15
  done
}

idrac_access
if (( check )); then
  racadm getsysinfo -s
  racadm get iDRAC.NIC.Selection
  show_storage
  racadm serveraction powerstatus
  exit 0
fi
show_storage >&2
storage

addr="$(known_address)"
if (( ! reinstall )) && host_ready "$addr"; then
  log "$name already answers at $addr; --reinstall installs again"
else
  install_host
  addr="$(known_address)"
fi
source "$state/$name.env"
log "done: ssh $user@$ip  (MAC $mac on $dev; reserve it in the router's DHCP)"
ssh -o BatchMode=yes -o LogLevel=ERROR "$user@$ip" 'uname -a; . /etc/os-release; echo "$PRETTY_NAME"; lsblk -d -o NAME,SIZE,MODEL'
