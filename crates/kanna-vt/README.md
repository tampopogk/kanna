# kanna-vt

A terminal's screen, kept by **libghostty-vt** (Ghostty's terminal-emulation C library). The daemon keeps one per terminal ([architecture](../../docs/specs/architecture.md), [plan](../../docs/plans/mux-build.md) M1): it feeds it the pty's output, answers `kanna read` from it, and sends it to Kanna.app as a GHOSTSNP snapshot, which the app loads with `ghostty_surface_load_snapshot`.

## API

```rust
use kanna_vt::{Lines, Terminal};

let mut t = Terminal::new(80, 24, 10_000)?;   // cols, rows, scrollback lines
t.write(b"\x1b[1mhello\x1b[0m\r\n");           // program output, in any pieces
let replies = t.take_replies();               // answers to queries (CPR, DSR, ...) for the pty
t.set_cell_pixels(9, 18); t.resize(120, 40)?;

t.text(Lines::Screen);                        // `kanna read`: the visible screen
t.text(Lines::Last(200));                     // `kanna read --lines 200`: counting scrollback
t.size(); t.cursor(); t.title(); t.pwd(); t.alternate_screen(); t.cursor_visible();
t.progress(); t.mode(Mode::SYNC_OUTPUT);      // OSC 9;4 progress; a frame still being drawn

let snap: Vec<u8> = t.snapshot()?;            // GHOSTSNP, for the app or for disk
let back = Terminal::from_snapshot(&snap)?;   // same grid, size, scrollback, modes, title
```

| Item | What it does |
|---|---|
| `Terminal::new(cols, rows, scrollback_lines)` | A blank terminal, its scrollback limited in lines only. Sizes below 1 become 1. Unfinished-sequence tracking is on from the first byte, so a snapshot can be taken at any point. |
| `write(&[u8])` | Feeds the program's output. Any bytes are safe: libghostty-vt treats output as untrusted. |
| `take_replies()` | The bytes the terminal wrote back for the program: cursor position, device status, mode and color reports, device attributes (Ghostty's own: DA1 `CSI ? 62;22;52 c`, DA2 `CSI > 1;10;0 c`), size reports (XTWINOPS 14/16/18, and mode 2048's in-band reports, also sent on a resize) and XTVERSION (`libghostty`). No default colors are set, so OSC 10/11/12 queries get no reply and OSC 4 reports libghostty-vt's default palette, not the app's theme. Capped at 1 MiB between drains. |
| `resize(cols, rows)` | The primary screen reflows; the alternate screen doesn't. |
| `set_cell_pixels(width, height)` | A cell's size in pixels, for size reports. Unset, they report 0 pixels. |
| `text(Lines::Screen)` / `text(Lines::Last(n))` | Plain text, one line per row, trailing blanks and blank bottom rows trimmed, as the daemon's M0 `read` returns it. Wrapped lines are the rows they fill. In the alternate screen, `Last(n)` sees only that screen, which has no scrollback. |
| `snapshot()` / `from_snapshot(&[u8])` | Encodes or decodes GHOSTSNP. A snapshot carries its grid: whoever loads it gets that size. Malformed, truncated or corrupt snapshots are refused, never a crash (each record is CRC32C-checked). |
| `set_scrollback_lines(n)` / `scrollback_lines()` | The scrollback limit in lines. libghostty-vt prunes whole pages, so it keeps about `n` lines, give or take a page. |
| `set_scrollback_bytes(Some(n))` / `scrollback_bytes()` | A limit in bytes as well; whichever is reached first applies. `Terminal::new` sets none (libghostty-vt's own default, 10 kB, would keep only about 800 lines). |
| `progress()` | The last OSC 9;4 progress report: `None`, `Set(percent)`, `Error`, `Indeterminate` or `Paused`. Agent status reads it. Not kept in a snapshot. |
| `mode(Mode::SYNC_OUTPUT)` | Whether the program is inside a synchronized update (mode 2026): its frame isn't finished, so agent status waits. |
| `GHOSTTY_COMMIT` | The Ghostty commit the library was built from. |
| `MAX_UNFINISHED_SEQUENCE_BYTES` | 1 MiB, the same as the app's `ghostty_surface_load_snapshot` accepts. |

`Terminal` is `Send`, not `Sync`.

## Building and pinning

```sh
scripts/build-ghostty-vt.sh                           # this machine: target/ghostty-vt/aarch64-apple-darwin
scripts/build-ghostty-vt.sh x86_64-unknown-linux-gnu  # Linux, cross-built with Zig
cargo test -p kanna-vt
```

- The script builds libghostty-vt from `kanna_ghostty_source` in `scripts/env.sh` (a clone of the Ghostty fork at `KANNA_GHOSTTY_COMMIT`), with the Zig from `kanna_zig`. It is the same source and commit that `scripts/build-ghosttykit.sh` builds the app's GhosttyKit from. It runs `zig build -Demit-lib-vt=true -Demit-xcframework=false -Doptimize=ReleaseSafe`, adding `-Dtarget=x86_64-linux-gnu -Dcpu=baseline` for Linux.
- It installs `include/`, `lib/libghostty-vt.a` and `ghostty.rev` (the commit) into `target/ghostty-vt/<rust target>/`.
- `build.rs` links that library statically. It refuses to build if `ghostty.rev` isn't the commit `scripts/env.sh` pins, so the daemon's snapshots always come from the app's Ghostty. The snapshot format makes no compatibility promise between commits. To move Ghostty, change the pin in `env.sh` and rebuild both.
- If the library is missing for this machine, the build stops and says which script to run. For another target, `cargo check` still works (it only warns), since checking never links.
- `KANNA_GHOSTTY_VT_DIR=<prefix>` links another build instead. A commit mismatch is then only a warning.
- `GHOSTTY_SOURCE_DIR` builds another Ghostty tree, as it does for GhosttyKit.

**Linux:** `cargo check -p kanna-vt --target x86_64-unknown-linux-gnu` passes with or without the Linux library. With it built, the test binaries also link, using `zig cc -target x86_64-linux-gnu` as the linker. They haven't been run on Linux yet.

## The bindings

`src/ffi.rs` binds by hand only the functions, structs and constants that are used. `csrc/abi.c` is compiled against the real headers on every host build:

- it assigns each function to a pointer of the signature `ffi.rs` declares, so a mismatch fails the compile;
- it exports every size, offset and constant value, which the `abi` test compares with the Rust definitions.

## Tests

`tests/terminal.rs` covers text after writes (including bytes split mid-sequence and mid-UTF-8), wrapping, the alternate screen, resize and reflow, `Last(n)` with scrollback, the scrollback limit, title and pwd, replies, and snapshot round trips (including the alternate screen and an unfinished sequence). Two fuzz tests cover hostile input:

- random output weighted to escape sequences, plus a list of nasty sequences, with resizes, reads and snapshot reloads;
- truncated, bit-flipped and random snapshots.

## Known problems (Ghostty at the pinned commit)

- **No snapshot inside an overlong sequence.** While the output is inside an unfinished sequence longer than `MAX_UNFINISHED_SEQUENCE_BYTES` (an unterminated OSC or APC past 1 MiB), `snapshot()` fails with an invalid value. It works again once the sequence ends. The daemon must handle `snapshot()` errors, for example by retrying after more output.

## Snapshot compatibility with the app

Both libraries come from one commit and share one codec (`src/terminal/snapshot`). The app's `ghostty_surface_load_snapshot` (`src/apprt/embedded.zig`) decodes with `snapshot.decodeExact` and a 1 MiB continuation limit, the same limit this crate uses. Since step 1.2 the daemon attaches the app with `snapshot()`. Scenario `survives_relaunch` compares the app's `debug.text` after a relaunch with `kanna read`, and the daemon's `app_link` tests load its snapshots into a replica `Terminal` and check that output after the snapshot neither misses nor repeats anything. Not yet checked end to end: the app's grid against `size()`, scrollback and colors in the app, and a truncated snapshot refused by the app.

kmux's `spikes/ghostty-external-io/prove.sh` has done the same with snapshots from upstream libghostty-vt.
