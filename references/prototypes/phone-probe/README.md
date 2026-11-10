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

The tests take the target as `TEST_RUNNER_PROBE_TARGET`, JSON with `key`, `addrs` and `instance` (and `daemonKey`, `daemonInstance`), except `testScan`, which scans. The server publishes `_kanna-phone._udp` through the system's Bonjour, as a daemon does on macOS.

## Results

November 6, 2026: iPhone 15 Pro, iOS 27.0.1, on the same LAN as a MacBook Pro.

| Check | Result |
|---|---|
| iroh and `kanna-net` on `aarch64-apple-ios` | **Pass.** Binds in 30–34 ms; dials direct in 20–58 ms; RTT 8–23 ms |
| Reaching the LAN by a known address | **Pass.** No multicast entitlement, once Local Network is allowed |
| `NWBrowser` reading iroh's records | **Fail.** `.bonjour` lists the names; `.bonjourWithTXTRecord` doesn't. A listed record never resolves: `NWConnection` stays `preparing` for 30 s |
| A plain Bonjour service | **Pass** through the system's `DNSServiceRegister`, from `dns-sd -R` or from Rust. Browsed, resolved and dialed with no other address: 22–24 ms |
| The same service from `mdns-sd` on macOS | **Fail.** Not seen by the phone, nor by the same Mac's `dns-sd` |
| Back from the background | **Pass.** 57, 78 and 67 ms from `active` to an echo, after 9, 9 and 89 s away, against a 1 s target |
| Scanning | **Pass** with the real camera, on a terminal's QR |

- **The instance name** is the key in lowercase base32 with no padding, not z-base-32.
- **On the Mac,** `dns-sd -L` printed nothing for iroh's record in 8 s, and resolved the plain service in a second. So this isn't iOS: nobody's Bonjour resolves iroh's records. Why was not diagnosed.
- **Not covered:** a phone locked while away (the 89 s was on the Home Screen), a refused Local Network prompt, any other network.
- **Unexplained:** this server's own iroh record never appeared in a browse, though the daemons' do. The results above for iroh's records are from this Mac's daemon.

## Cellular

November 7, 2026: the real app, not the probe, on the same iPhone with Wi-Fi off and cellular on, no VPN, against a MacBook Pro with a public address and `relay.kanna.build`. One run of `testAway`.

| Check | Result |
|---|---|
| Launch to a link | 5,966 ms, `Direct · internet`, RTT 100 ms |
| Question to answer on the phone | **Pass.** The app got the answer in 69 s, most of it a new planner's first start and its trust prompt |
| Back after 5, 15 and 45 s on the Home Screen | 12,126, 12,779 and 12,024 ms, each `Relayed` |
| The chip after that, watched for 90 s | `Relayed` to `Direct · internet` at 18 s, back to `Relayed` at 84 s |

- **Why coming back took 12 s,** and why the chip went back to `Relayed`: [timed and fixed](#timed-and-fixed), below.
- **Not covered:** a carrier NAT with no way through, Wi-Fi coming back while the app is open, a phone locked while away.

### Timed and fixed

Later on November 7: the same phone on 4G with one bar and Wi-Fi off, through `https://relay.kanna.build:8444`. `DeviceTests/testAway`, with the app stopped by `SIGSTOP` for most of each absence, since XCUITest keeps it from being suspended.

| Back after | Before | After |
|---|---|---|
| 5 s | 12,126 ms | 508 ms |
| 15 s | 12,779 ms | 501 ms |
| 45 s | 12,024 ms | 2,677 ms |

These are one run each on a link that varies widely. The numbers from before came from an earlier run, on the same phone and with the app not frozen. Over the session a link with the app kept came back in 171–1,078 ms, and a redial after 45 s took 2.7–12.6 s. A cold launch took 5,966 ms before; the launches here took 7–28 s, but each came seconds after Wi-Fi was switched off, so they aren't comparable.

Where a return went, on the same link, before the fix:

| Step | Cost |
|---|---|
| `LiveBackend.close` waiting on the endpoint | None: done in 0.2–2.3 s, before the return began |
| Bonjour | None when the dial lands: it's cancelled then. It runs 8 s with a failed dial, alongside it |
| Looking up the saved "good address", a relay URL after a landing on Relayed | Up to 3 s, in the path of every such return; 2,530 ms measured |
| First dial: TCP 0.4 s, TLS 1.3 s, relay connected 3.3 s, QUIC through the relay 4.8 s | 4.8–9 s. A relay round trip is 1.3–2.9 s just after waking and settles near 400 ms |
| Stream header and `hello` | 2.2 s |
| A failed dial: a new endpoint for the next machine, draining the old one | 3 s, and the relay connection thrown away |

What the signal costs and what the code costs: this was one bar, in a place with little coverage, and both columns above are on it, so neither says what a return takes on normal coverage.

- **The signal:** the relay's round trip, 1.3–2.9 s just after waking, settling near 400 ms. On Wi-Fi to the same relay it's 165 ms, and the same dial, from nothing to a path through the relay, took 0.9 s against 4.8–9 s here. That dial is about ten round trips, so most of its time scales with the link. Normal coverage wasn't measured.
- **The code:** rebuilding those connections on every return at all, the lookup of a relay URL (up to 3 s), and a failed dial's new endpoint (3 s, and the relay connection lost). None of these depends on the signal. What's left after the fix is one `hello`, one round trip of the kept link, which is why it comes to about 0.5 s here.
- **The 12 s itself** can't be divided exactly: it was timed before the steps were, in one run. The steps above come from later runs with the same build.

- **iroh's DNS client on cellular** took up to 7.5 s or timed out over 9 s, in the same seconds that `getaddrinfo` answered in 2–3 ms. It did read the right nameservers.
- **The chip going Direct, then Relayed, then Direct** is iroh losing a quiet direct path: at 2.2 s RTT the Mac abandoned it (`RemoteAbandoned`), and a hole punch brought it back 36 s later. The chip was right, and the link stayed up.
- **Not covered:** a phone locked while away, where `Instant` stops; an absence of more than a minute; another carrier.

## What had to change to build

- `vendor/netdev` and `vendor/netwatch`: neither compiles for iOS; see `NOTICE`. Upstream `netwatch` 0.20 has the same gap.
- `fast-apple-datapath` is macOS only. It reaches the private `sendmsg_x` through `dlsym`, which an App Store build shouldn't carry, and a chat has no need of it.
