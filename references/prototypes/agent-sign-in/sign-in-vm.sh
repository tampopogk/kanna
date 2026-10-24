#!/usr/bin/env bash
# The user runs this in a dedicated terminal for each VM. No holder file is copied.
set -euo pipefail
vm=${1:?usage: bash sign-in-vm.sh agent-sign-in-a|agent-sign-in-b [ssh-host]}
host=${2:-jeremyhale@172.16.0.105}
case "$vm" in agent-sign-in-a|agent-sign-in-b) ;; *) exit 2 ;; esac
read -r -p "Press Enter to sign in to Codex on $vm. "
ssh -tt "$host" sudo -n incus exec "$vm" -- su - probe -c \
  "'umask 077; mkdir -p /home/probe/.local/share/agent-sign-in/codex; export CODEX_HOME=/home/probe/.local/share/agent-sign-in/codex; codex -c cli_auth_credentials_store=\"file\" login --device-auth'"
