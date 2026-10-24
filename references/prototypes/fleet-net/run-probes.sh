#!/usr/bin/env bash
# Run from this folder. Keys and result destination must be explicit.
set -euo pipefail
if [ "$#" -ne 5 ]; then
  echo 'usage: run-probes.sh lan|relay|away PEER_ID PRIVATE_KEY_PATH RELAY_URL OUTPUT_DIR' >&2
  exit 2
fi
scenario=$1
peer=$2
key=$3
relay=$4
out=$5
mkdir -p "$out"
case "$scenario" in
  lan) endpoint=(--mdns); expected=direct ;;
  relay) endpoint=(--relay "$relay" --relay-only); expected=relay ;;
  away) endpoint=(--relay "$relay"); expected=direct ;;
  *) echo "unknown scenario: $scenario" >&2; exit 2 ;;
esac
for run in 1 2 3; do
  ./target/release/fleet-net --key "$key" "${endpoint[@]}" probe \
    --peer "$peer" --expect "$expected" --samples 100 --mib 16 \
    --label "$scenario-$run" --output "$out/$scenario-$run.json" > "$out/$scenario-$run.log" 2>&1
done
python3 summarize.py "$out/$scenario-1.json" "$out/$scenario-2.json" "$out/$scenario-3.json"
