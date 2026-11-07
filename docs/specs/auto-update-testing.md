# Auto Update Testing

**Status:** built · **Tests:** [auto update](auto-update.md)

`tools/auto-update/` runs a real update, from one Developer ID build to the next, by hand: it needs the signing identity, opens real windows, and takes minutes. Sparkle relaunches through Launch Services, so the environment doesn't cross; the builds are therefore this worktree's dev channel, whose folder is its own and never the user's, with a throwaway key and a feed on `127.0.0.1` baked in by `KANNA_UPDATE_FEED` and `KANNA_UPDATE_PUBLIC_KEY`.

```sh
tools/auto-update/build.sh DIR                  # builds A and B, and B's DMG
tools/auto-update/build-next-protocol.sh DIR    # builds C, of the next protocol
tools/auto-update/run.sh DIR ask_first bad_updates real_update machine_left_behind
```

| Phase | Proves |
|---|---|
| `ask_first` | SIGKILL and SIGTERM at Found, after Later and mid-download each leave A in place with no installer running |
| `bad_updates` | a truncated DMG, a DMG signed by another key and a feed signed by another key are refused, and A stays |
| `real_update` | a claude agent, a Desk task, a Shelf task, a shell and a running program on A; yes in the toast; on B the same pids under the same keeper, now of B's build, the screens, a longest pause of 1 s in the running program, claude carrying on its conversation, `exit 7` reporting 7 |
| `machine_left_behind` | A paired with a daemon of its protocol; the toast names it; on C it shows `other_protocol`, its program runs on, and updating it brings it back with the program still running |

`swift test --filter KannaUpdatesTests` covers the states and the warning line. `release.sh --dry-run` signs the real feeds and checks them with `sign_update --verify`.
