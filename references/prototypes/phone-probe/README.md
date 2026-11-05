# Phone probe

Runs the first three checks of [What depends on a real iPhone](../../../docs/specs/mobile-app.md#5-what-depends-on-a-real-iphone) on a real phone, with the real `kanna-ios-ffi`. `server/` is an iroh echo server on UDP 7479 that shows a QR and logs what the phone reports. `PhoneProbe/` is the app. `PhoneProbeUITests/` drives it with no hands, the camera included if it faces the code.

## Run it

```sh
scripts/build-ios-ffi.sh
cd references/prototypes/phone-probe
(cd server && cargo build --release && target/release/phone-probe-server .) &
xcodegen generate
xcodebuild -project PhoneProbe.xcodeproj -scheme PhoneProbe -destination 'id=DEVICE' -allowProvisioningUpdates test
```

The tests take the target as `TEST_RUNNER_PROBE_TARGET`, JSON with `key`, `addrs` and `instance` (and `daemonKey`, `daemonInstance`), except `testScan`, which scans. A `dns-sd -R PhoneProbePlain _kanna._udp local. 7479` beside the server gives the plain service to compare.

## Results

November 6, 2026: iPhone 15 Pro, iOS 27.0.1, on the same LAN as a MacBook Pro.

| Check | Result |
|---|---|
| iroh and `kanna-net` on `aarch64-apple-ios` | **Pass.** Binds in 30–34 ms; dials direct in 20–58 ms; RTT 8–23 ms |
| Reaching the LAN by a known address | **Pass.** No multicast entitlement, once Local Network is allowed |
| `NWBrowser` reading iroh's records | **Fail.** `.bonjour` lists the names; `.bonjourWithTXTRecord` doesn't. A listed record never resolves: `NWConnection` stays `preparing` for 30 s |
| A plain Bonjour service | **Pass.** Browsed, resolved and dialed with no other address: 24 ms |
| Back from the background | **Pass.** 57, 78 and 67 ms from `active` to an echo, after 9, 9 and 89 s away, against a 1 s target |
| Scanning | **Pass** with the real camera, on a terminal's QR |

- **The instance name** is the key in lowercase base32 with no padding, not z-base-32.
- **On the Mac,** `dns-sd -L` printed nothing for iroh's record in 8 s, and resolved the plain service in a second. So this isn't iOS: nobody's Bonjour resolves iroh's records. Why was not diagnosed.
- **Not covered:** a phone locked while away (the 89 s was on the Home Screen), a refused Local Network prompt, any other network.
- **Unexplained:** this server's own iroh record never appeared in a browse, though the daemons' do. The results above for iroh's records are from this Mac's daemon.

## What had to change to build

- `vendor/netdev` and `vendor/netwatch`: neither compiles for iOS; see `NOTICE`. Upstream `netwatch` 0.20 has the same gap.
- `fast-apple-datapath` is macOS only. It reaches the private `sendmsg_x` through `dlsym`, which an App Store build shouldn't carry, and a chat has no need of it.
