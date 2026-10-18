# kmux

A native macOS terminal multiplexer: windows of tabs, split into Ghostty
terminal, web, markdown and iOS Simulator panes, driven from the keyboard, the mouse, the `kmux` CLI
or anything that speaks its control protocol.

| Path | What |
|------|------|
| `docs/kmux-spec.md` | The spec, illustrated from the reference model. |
| `docs/presentation-layers.md` | What the core keeps (content, logical layout) and what the app keeps (windows, pixels). |
| `reference/kmux/` | The reference model: an interactive mockup (`index.html`) over a DOM-free core (`core.js`). |
| `tests/kmux-protocol/` | Protocol cases shared by the model and the app. |
| `apps/kmux/` | The app (Swift, AppKit, upstream Ghostty). |
| `crates/kmux/` | The `kmux` CLI (Rust). |
| `crates/kmux-client/` | Control-socket client, also used by kanna. |
| `crates/kmux-bench/`, `bench/` | The performance reference and its targets. |

```sh
scripts/build-kmux.sh            # target/kmux.app (builds GhosttyKit and merman first if needed; merman needs rustup)
cargo build --release            # target/release/kmux
scripts/test-kmux-protocol.sh    # shared cases: model and app
scripts/e2e-kmux.sh              # end to end: app + CLI + pixels (KMUX_E2E_IOS=0 skips the simulator)
target/release/kmux-bench        # performance: pane start, typing latency, memory (targets in bench/)
scripts/package-kmux.sh          # target/dist/kmux-*-unsigned.dmg: app + CLI, UNSIGNED, local testing only
```

Run several kmux instances at once with `kmux --instance NAME …` (each has
its own windows and socket); `kmux instances` lists them. Scripts and tests
can start kmux with `--bg` (app or CLI, or `KMUX_BG=1`) to keep it behind your other windows.

kmux saves each instance's layout and brings it back when it starts again
(terminals start anew in their directory, with their command); `--fresh` opens
a new window instead.

`kmux open md docs/kmux-spec.md` shows a markdown file natively, with Mermaid
diagrams drawn, and reloads it when it changes. ⌘= ⌘− ⌘0 or a pinch zoom it.

`kmux open --name agent --cmd claude --dock` docks a pane along the window's
edge, outside its tabs: it stays while you switch tabs (⌘1…⌘9) and closes only
with its window.

iOS panes need Xcode with an iOS simulator runtime:
`kmux open ios --app build/MyApp.app --device "iPhone 16"` (or a bundle ID
such as `com.apple.Preferences`).

## License

MIT. See [LICENSE](LICENSE).
