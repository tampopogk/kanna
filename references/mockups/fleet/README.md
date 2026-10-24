# Fleet Desk model

The interactive model is **[../fleet.html](../fleet.html)**. Its [spec and screenshots](../../../docs/specs/fleet-app.md) are linked from [Fleet §6](../../../docs/specs/fleet.md#6-one-app-several-machines). This is a standalone reference model before Fleet 3, not product code. Ready for user review; approval is still pending.

## Run

Open `references/mockups/fleet.html` directly in a browser. It is one HTML file with inline CSS and JavaScript, no dependencies, network, sign-in or build step. In Kanna, from the repo root:

```sh
kanna markup references/mockups/fleet.html
```

For a plain tab without comments:

```sh
kanna open web "file://$PWD/references/mockups/fleet.html" --tab
```

The README is in this adjacent directory so `fleet.html` stays at the exact location named by Fleet 3.

## Try it

1. Hover or keyboard-focus **Review build output**: its tooltip says **tests-1 · Relayed**. Pull it and hover its agent header or Shelf item for the same details. Select the fixed Fleet tab and hover or focus a path indicator for its connection explanation; then return to the task’s document tab. Shelf and Fleet stay first in the tab strip and cannot be closed; switching them keeps the current agent and task in place.
2. Pull **Restore task tabs**. It is offline: the last view stays, but input is disabled. Switch to **All on LAN** above the app window: input returns without losing its place or changing its owner.
3. Use **Internet direct** and inspect Fleet. Direct internet is distinct from LAN and relay; task tooltips update with the path. No connection state adds a persistent badge to a task card.
4. Use **Remote offline**. Inspect the planner, retained claims, and the backlog task waiting for its offline owner. Open Recipe Box to use a local task.
5. File → New Desk / Open Recent opens **projects**. Open Kanna repeatedly: its existing Desk comes forward. Open Recipe Box and return: each keeps its own selection.
6. Type different text in two task agents. The model only appends local fixture text. The three Kanna tasks deliberately share `term-41` on different peers; returning to each must show only its own text.

**Reset** restores the mixed-path scene. Light / dark follows the original Desk mockup's palette. The controls above the app are simulation controls, never product machine/path pickers.

## Repeatable captures

Append a fragment to the file URL, then reload to initialize that scene:

| Fragment | Scene |
|---|---|
| `#picture` | Project Desk; capture at 1360 × 850 |
| `#picture,fleet` | Fleet page; capture at 1360 × 920 |
| `#picture,relay-task` | Task on the relay |
| `#picture,offline-task` | Offline task's last view |
| `#picture,lan` | All peers directly reachable on LAN |
| `#picture,internet,fleet` | Direct internet paths |
| `#picture,offline` | Remote planner and machines offline |
| Add `,light` | The same scene in light mode |

Screenshots live in `docs/specs/fleet-{desk,page,relay,offline,tooltip,path-tooltip}.png`. For the task tooltip capture, use `#picture` and hover the **Review build output** card. For the path tooltip, use `#picture,fleet` and hover the **Relayed** indicator. Refresh them when changing the model. Keep this model as the reference after the product lands.

## What it showed

- A single project Desk can hold claims across peers without choosing a machine. Pulling and parking are presentation changes. Claim-device metadata stays internal. Machine and connection details live in tooltips on task cards, Shelf items, agent headers and build summaries; hover or keyboard focus reveals them, Escape dismisses them. Fleet shows the full machine inventory.
- Connection state and task state need separate treatment. An offline task keeps its claim; its last state must be labeled stale, not silently presented as current or changed to stopped.
- The same task can have a Linux agent and a Mac build job. The job stays in the task’s UI tests pane, with its machine in a tooltip; Fleet lists machines and paths, without a Work column.
- A direct-path recovery updates the connection tooltip automatically, without disturbing the task or its position.
- Bare content IDs are insufficient: three peers can all have `term-41`.

**Checked:** Chrome rendering at the capture sizes, light mode and 900px width; project reopening; fixed Shelf/Fleet tabs, task preservation and keyboard tab navigation; three claims; direct / relay / offline tooltips; disabled offline input; reconnect preserving selection and order; pane-input isolation; no persistent host or path labels in task views; hover/focus tooltip contents and dismissal; Fleet path details; no JavaScript errors. Screenshots were inspected visually. Product tests should copy the comparison cases in the spec and compare their rendering with these scenes.

**Simulation limits:** `mbp` is this Mac; `dev-1`, `tests-1`, and `tools-1` are illustrative Incus peers on r720-1, not provisioned VMs. All work, paths, ages and capabilities are fixtures. No SSH, Cloud or relay probe was needed. This model does not validate transport, persistence, repo-identity matching, actual jobs or background delivery; those belong to their slices and other Fleet models. Shelf membership is fixed; backlog mutation is outside this UI model. Project windows are represented by retained sections in one page, with only the front one visible.
