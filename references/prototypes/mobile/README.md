# Kanna on the phone: SwiftUI prototype

Second of three steps: [mockup](../../mockups/mobile.html), this prototype, then the spec and the real app. It's for feeling the navigation, the chat and the quiz flow on a phone. Everything is fixtures and timers in `KannaMobile/Store.swift`: no daemon, no networking, no real pairing, no camera, and nothing shared with the real app.

## Run it

Needs Xcode 26 and an iOS 26 Simulator runtime. From the repo root:

```sh
references/prototypes/mobile/run.sh
```

That boots an iPhone 17 Pro, builds, installs, opens Simulator and launches. For another device, `KANNA_PROTO_DEVICE="iPhone Air" references/prototypes/mobile/run.sh`.

Or open `references/prototypes/mobile/KannaMobile.xcodeproj` in Xcode, pick an iPhone Simulator, and press ⌘R. XcodeGen isn't needed to run it; `project.yml` only regenerates the project when files are added (`xcodegen generate`). Signing is off, so a real iPhone needs a team set in Xcode and `CODE_SIGNING_ALLOWED` removed.

## What to try

**Prototype controls** are at the bottom of the repo switcher: tap the repo's name on any tab. On the pairing screens, press and hold anywhere. They switch every choice below and jump to any mockup screen.

- **Pairing:** the first launch. Scan the code, wait for studio to confirm, and pick a repo.
- **Chat:** the Mac's keyboard types in the Simulator. Try these:
  - `what landed overnight`
  - `explain codex`, then tap the code card, tap a line, and ask about it
  - `is anything on the shelf out of date`, then tap a choice, and watch the Shelf change
  - `quiz me`
  - `start native diff viewer`
- **No planner:** switch to kmux and send anything.
- **Busy planner:** in the controls, use *Busy with something else*, then send a message.
- **Shelf:** open *Native diff viewer* and Pick it for mbp, which is full. Open *Review build output* and answer it. Open *Check tab restoration* and follow what it waits for.
- **Machines:** open studio and move the planner to dev-1. The chip turns to Relayed.
- **Stats:** press and hold a chart, then drag. Haptics need a real iPhone.
- **Quiz:** play a round and miss one. Tap the code on the miss, close the round half way and come back to it, and finish it. The miss returns, reworded, in the next round. Tap a tile, then *Quiz me on this*.
- **Connection:** *Walk out of the house* goes LAN, Relayed, Direct · internet with nothing else moving. Offline, write a message, then come back and it sends. Tap the chip for what the path means.

| Switch | Options | Why |
|---|---|---|
| Quiz score (D10) | Streak (default), Points, Both, Neither | The real choice the mockup left. Neither is added because D10's case is that the map is already the score |
| Wrap by default (D9) | On (default), off | Off, code cards scroll sideways |
| Quiz agent is slow (D12) | Off (default), on | Shows the quiet *Writing your next round* button |
| Several Macs, mbp is closed (D4) | | One Mac skips the sheet. A closed Mac still takes the pick |
| Camera refused, Forget this phone | | The pairing paths |
| Appearance, Timing | Fast, As it would be, Slow | Light and dark, and how long the planner takes |

## The decisions, for the spec task

No spec was changed here. *Confirmed* means the mockup's recommendation is built and holds together in the flows. Fixtures can't test anything on the Mac's side, so D2, D4, D5, D8, D11, D12 and D13 still need proving in the real daemon. How it feels on a phone is the user's call. Nobody has tried this on a real iPhone yet.

| | Decision | Result |
|---|---|---|
| D1 | One planner machine per repo | Confirmed. **Added:** a page for another machine offers *Run the planner here*, beside the mockup's button on the planner's own page |
| D2 | A started planner runs windowless (changes the planning-agent spec) | Confirmed as a flow: the note says where, and the message waits, then goes |
| D3 | No interrupting the planner | Confirmed. The wait is real here: try it with Timing on Slow |
| D4 | Desk claims move into the daemon (changes how Desks persist) | Confirmed as a flow. **Added:** a picked task says *shows when it next opens* on a closed Mac |
| D5 | The daemon keeps a task's last words (new) | Confirmed: the *needs you* screen is only useful with them |
| D6 | Pick, message and read only | **Changed:** Pick shows only for Shelf tasks. The mockup's *running* and *needs you* screens offer it on tasks already on a Desk, which can't be picked. **New question:** a message to a task that hasn't started says *Waits until it starts*, which no spec covers |
| D7 | Queue offline for a day, then ask again | Confirmed. **Added:** the asking is *A day old, not sent* with Discard and Keep waiting. Messages to tasks queue too, and *No network* is its own banner |
| D8 | Subsystems and mastery | Unchanged and still open. The map reads well, but the numbers are invented. Shades here are at 40, 60 and 80% |
| D9 | Wrap by default, with a toggle | Confirmed. Lines break at spaces first, and inside a word only when they must |
| D10 | Streak or points | **Your call:** switchable, on the recommended streak. Try all four |
| D11 | When a streak breaks | Part built: one round a day adds a day. Mac rounds and skipped days can't be simulated |
| D12 | Rounds written ahead | Confirmed. **Added:** *Another round* reads *Writing…* until the next one is in. The lag is visible: a miss returns one round later, as D12 expected |
| D13 | A phone in the peer log, and the Mac shows the code | Confirmed as a flow: nothing to type, and a refused camera sends you to Settings. The protocol change is untested |

## Where it differs from the mockup

Screens are in `screenshots/`, mockup on the left of each pair, app on the right: `dark-1.png` to `dark-4.png`, `light-1.png` to `light-4.png`. Reproduce them with `scripts/shots.sh OUT/shots`, then `scripts/compare.sh OUT`.

- **iOS 26's own controls:** the tab bar floats, and the back button is a chevron without *Shelf*. These stay native, so swiping back and the keyboard behave as a real app's would. The real app gets the same unless it opts out.
- **Icons:** SF Symbols, not the mockup's drawn set.
- **Header:** set solid, since iOS 26's translucent bar lets text show through under the repo name.
- **The chip's card:** custom, as drawn. The native popover covered the chip.
- **Mockup slips, fixed here:** mbp's Desk says 3 of 3 but lists one task, so all three show. *Schedule history page* is asked about in Chat but missing from the Shelf, so the Shelf is 7, not 6. A machine's page lists its Desk's tasks as well as those running on it. kmux has a hollow dot, because its planner isn't running.
- **Fixtures:** line numbers follow the mockup, and the real code has moved on. The answers to the round's questions are checked against the code and specs.

## For the real app

- The keyboard hides the tab bar, so after sending a message you can't leave Chat until it's gone. Here a tap anywhere on the conversation dismisses it, as well as dragging it down. The tap is tested; try the drag by hand.
- Hanging indents use a `UILabel` with a paragraph style (`CodeText` in `Code.swift`). Whether SwiftUI's `Text` can do them wasn't tried.
- Press-and-drag on a chart works inside a `ScrollView` with a long press sequenced before a drag (`Tile` in `Stats.swift`), and scrolling still works.

## Checks

`KannaMobileUITests/FlowTests.swift` plays 16 flows by tapping:

```sh
cd references/prototypes/mobile
xcodebuild -project KannaMobile.xcodeproj -scheme KannaMobile -destination 'platform=iOS Simulator,name=iPhone 17 Pro' -derivedDataPath build/dd test
```
