# Fleet network model

A standalone Rust transport probe for [Fleet §3, §5 and §10](../../../docs/specs/fleet.md), before Fleet 2, 3 and 8. This is a reference experiment, not product code.

**Status: in progress.** LAN discovery/direct traffic is measured; the remaining runs and conclusions are recorded below as they complete. A real off-LAN hole-punching run is required before this model releases its dependent slices. A same-LAN connection bootstrapped through a relay does not satisfy that requirement.

## Versions and stability

Checked October 23, 2026:

- **iroh / iroh-relay 1.4.1**, pinned in Cargo and on the relay. [Latest release](https://github.com/n0-computer/iroh/releases/tag/v1.4.1) includes a rustls-webpki dependency fix. The preceding [1.4.0 release](https://github.com/n0-computer/iroh/releases/tag/v1.4.0) fixed LAN address refresh without a relay, reduced relay cache startup memory, bounded pending path queues and updated vulnerable TLS dependencies.
- **iroh-mdns-address-lookup 0.6.1**, pinned separately. It is still a 0.x helper even though its transport dependency is stable. [Crate documentation](https://docs.rs/crate/iroh-mdns-address-lookup/0.6.1).
- Iroh 1.0 was released June 15, 2026. Upstream explicitly promises API and wire stability across 1.x minor releases, including communication between different languages. This is a compatibility commitment, not evidence that every NAT or network will work. [Upstream stability statement](https://www.iroh.computer/blog/v1).
- This probe uses stable endpoint, stream and path-observation APIs. It does not enable `unstable-custom-transports` or `unstable-net-report`.
- The earlier attempt's standalone manifest and lockfile were reused and updated; its `main.rs` was only `Hello, world!`.

## What the probe does

Two endpoints negotiate **`kanna/1`**. Every bidirectional stream starts with a JSON header line naming its kind. The first stream is `control`, with an illustrative `app.hello` and JSON request/reply echoes. Five separate bulk stream kinds follow: `file`, `port`, `sync`, `job`, `transfer`. Each is tested in both directions, on that same connection.

The bulk handlers only exercise byte transport; they do not implement files, TCP forwarding, job scheduling, git sync, pairing or certificates. The hello reply is fixed at model protocol 1. Those product behaviours belong to their slices and other models. The endpoint authenticates the peer key through iroh; the model server does not implement a paired-peer log.

Configuration is deliberately explicit:

- `presets::Minimal`, with `RelayMode::Disabled` unless `--relay` supplies the one private relay. No n0 public relay, DNS discovery or pkarr publisher is installed.
- `--mdns` adds local discovery. A LAN probe receives **only the server's public endpoint ID**, not its IP/port.
- Automatic UPnP/PCP/NAT-PMP port mapping is excluded from Cargo features. Away tests therefore cannot silently depend on router port forwarding created by the probe.
- `--relay-only` removes IP transports on the probe. This proves a deterministic relay fallback when UDP is unavailable; it is not a test of an established connection migrating after UDP is interrupted.
- Bulk send streams have priority -1 on both ends; control keeps the default priority. Streams share congestion control, bandwidth and CPU, so this does not promise low echo delay under saturation.
- The selected application-data path is checked before/after each idle echo and during every bulk transfer. Snapshots contain addresses, byte counters, RTT, packet loss and selection; the report also records path events, including startup transitions. A configured relay URL alone never counts as proof of relaying.

The model handles trusted test data. Resource limits, unknown-peer admission and hostile headers are product work, not hardened here. The deployed relay restricts registration to the two model public keys.

## Build and run

From this directory, with Rust/Cargo installed (tested with Rust 1.99.0 on the Mac and 1.99.1 on Linux):

```sh
cargo build --release --locked
python3 smoke.py
cargo clippy --locked -- -D warnings
```

Each process logs its endpoint ID to stderr. `--key PATH` creates a new private key with mode 0600 or reuses it. Keep it **outside the repository**, one key per concurrently running endpoint. Omitting it generates an ephemeral key. Do not use the same key for a live server and probe.

### LAN, no external service

On the receiving machine:

```sh
./target/release/fleet-net --key /tmp/fleet-net-server.key --mdns serve
```

On the other machine, substitute the endpoint ID printed by the server:

```sh
./target/release/fleet-net --key /tmp/fleet-net-client.key --mdns probe \
  --peer SERVER_ENDPOINT_ID --expect direct --samples 100 --mib 16 \
  --label lan --output /tmp/fleet-net-lan.json
python3 summarize.py /tmp/fleet-net-lan.json
```

### Private relay and away

Restart the server with its same key and the private relay:

```sh
./target/release/fleet-net --key /tmp/fleet-net-server.key \
  --relay https://relay.kanna.build:8443 serve
```

For a repeat on the current test rig, the server's ID is
`bedd173017ae1ee4c19b8501c2a6819c6c4c385a708c0da7ef99fe7f94aca248`.
The Mac probe key is `/tmp/fleet-net-probe.key`; the R720 key is `/tmp/fleet-net-linux.key`. These paths are temporary local state, not committed artifacts. Rebooting/clearing `/tmp` may require regenerating keys and updating the relay allowlist.

For **forced relay**, keep both machines on the LAN but disable the probe's IP transports:

```sh
./target/release/fleet-net --key /tmp/fleet-net-probe.key \
  --relay https://relay.kanna.build:8443 --relay-only probe \
  --peer bedd173017ae1ee4c19b8501c2a6819c6c4c385a708c0da7ef99fe7f94aca248 \
  --expect relay --samples 100 --mib 16 --label relay --output /tmp/fleet-net-relay.json
```

For **hole punching from away**, put the Mac on a phone hotspot or independent internet connection, disable its LAN/VPN routes, then run the same command **without `--relay-only` and with `--expect direct`**. Do not add `--mdns` or a peer IP address. The relay is the rendezvous point; successful direct-path selection and the remote public address establish the observation. Record the carrier/access network and routing context alongside the JSON. If it remains relayed after 30 seconds, the probe fails and saves the path evidence; report that outcome instead of calling it a successful hole punch. A second topology may be needed.

Three repeated runs, with logs kept outside the repository:

```sh
./run-probes.sh away \
  bedd173017ae1ee4c19b8501c2a6819c6c4c385a708c0da7ef99fe7f94aca248 \
  /tmp/fleet-net-probe.key https://relay.kanna.build:8443 /tmp/fleet-net-away
```

`lan` and `relay` are the other scenarios. Supply the same five arguments; the relay argument is ignored in LAN mode. Copy successful JSON reports into `results/` with a note about the topology. `summarize.py` checks that all ten bulk transfers completed, data rates agree with byte counts and elapsed time, control echoes overlapped bulk transfers, and the selected path matches the requested class. Raw path events remain available to inspect transitions between snapshots.

## Relay deployment

Existing GCP project `kanna-build` (number `402613185450`), instance `kanna-relay-vm`, `us-central1-a`, e2-micro, Debian 12. Reserved static IP `34.133.233.111`, hostname `relay.kanna.build`. Access:

```sh
gcloud compute ssh kanna-relay-vm --project=kanna-build --zone=us-central1-a
```

The existing Docker/Caddy service still owns ports 80/443. The model adds:

- `/usr/local/bin/iroh-relay-fleet-net`, upstream **1.4.1** Linux x86_64 binary.
- `/etc/fleet-net-relay.toml`, from [relay/relay.toml](relay/relay.toml).
- `fleet-net-relay.service`, from [relay/fleet-net-relay.service](relay/fleet-net-relay.service), enabled at boot.
- GCP ingress rule `fleet-net-iroh`, scoped to the `kanna-relay` tag: TCP 8443 and UDP 7842. Metrics stay on loopback port 9091; the HTTP captive-portal service stays on loopback 3340.

The unit uses systemd `LoadCredential` to read Caddy's existing certificate/key **on the VM**, without exporting keys. It runs as a dynamic unprivileged user. Credentials are copied at service start: restart this model service after Caddy renews its certificate. This is a prototype deployment, not an automated certificate-renewal integration.

Download and verify the release archive before installing:

```sh
curl -fL -o /tmp/iroh-relay.tar.gz \
  https://github.com/n0-computer/iroh/releases/download/v1.4.1/iroh-relay-v1.4.1-x86_64-unknown-linux-gnu.tar.gz
# Expected SHA-256 from the GitHub release asset metadata:
# eeceef0f06f04ad2644644cd80dd5733e64da46daf8a9f86d87a5ad5c13b06e4
sha256sum /tmp/iroh-relay.tar.gz
```

Extract it to a temporary directory, install the executable and the two configuration files at the paths above, then `sudo systemctl daemon-reload && sudo systemctl enable --now fleet-net-relay`. The certificate paths in the example unit are specific to this VM. For another server use its certificate paths and a matching hostname, and update the public endpoint allowlist in `relay.toml`. No certificate, private key, bearer token or cloud credential belongs in this folder.

To stop the model relay: `sudo systemctl disable --now fleet-net-relay`; remove its GCP rule with `gcloud compute firewall-rules delete fleet-net-iroh --project=kanna-build`. Neither action changes the existing Kanna relay.

## Measurement method and rig

Every run makes one fresh connection, opens control first, waits for the requested selected path (up to 30 seconds), discards ten warm-up echoes, then takes 100 idle echoes with 10 ms between them. It subsequently transfers **16 MiB per kind per direction** (160 MiB total payload per run), verifies every byte, and continues sequential echoes during each bulk transfer. Only one bulk stream is active at a time. Each echo is allowed to finish even when its bulk transfer has just completed, so the last loaded sample may straddle completion.

Echo is an application JSON round trip, **not one-way latency or rendered terminal latency**. It includes serialization, framing, event-loop scheduling and transport. Upload throughput ends after the server verifies the bytes and replies; download ends after all bytes and FIN arrive. Stream/header setup and verification are included. Numbers are payload Mbps (decimal 1,000,000 bits/s); sizes are MiB (1,048,576 bytes). Percentiles use nearest rank. Table upload/download rates aggregate the five transfers as total bytes / total time; loaded p95 is the **worst per-transfer p95**, not a pooled percentile.

Rig: Apple M5 MacBook Pro, 16 GB, macOS 26.2 (25C56), arm64, address `172.16.0.104`; R720 host, Ubuntu 26.04.1, kernel 7.0.0-30, x86_64, address `172.16.0.105`. The R720 route is the USB Wi-Fi interface `wlxe0d768301a7a`; its four Ethernet interfaces are down. These are **shared-LAN measurements over Wi-Fi, not wired line-rate claims**. The relay is in Iowa (`us-central1`), so relay latency includes the path to that region. Both measured endpoints remain these two machines; the relay is not the benchmark receiver.

## Results and slice contract

Run `python3 summarize.py` to regenerate the table from the committed raw JSON. Results and outstanding topology checks follow below.

Fleet 2 should copy the explicit minimal endpoint setup, local discovery, private relay map and header-first stream dispatch. Fleet 3 should derive its path indicator from the **selected path**, not relay configuration or the mere presence of an open relay path. Fleet 8 should reuse the same sizes, warm-up, echo framing, priorities, payload verification and idle/loaded scenarios, then add the real PTY and rendering work to expose its incremental delay. Compare on the same rig/topology with a model run alongside the product; environmental jitter is not a product regression. Preserve raw samples and throughput in both directions.

Observed surprises so far: the test LAN is wireless; separate streams and control priority still share congestion and can have large loaded echo tails. Stable iroh 1.x and the separately versioned mDNS helper must be tracked independently. The VM already had an unrelated relay service, so this deployment uses separate ports rather than replacing it.
