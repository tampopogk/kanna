# Fleet backlog model

Reference for [Fleet 10](../../../docs/specs/machines.md#10-build-order), implementing
[tasks §1–2](../../../docs/specs/machine-tasks.md). Python 3.9+; standard library only.
Run on this Mac or Linux; all machines and partitions are simulated. No SSH,
cloud access, accounts, agent processes or secrets are needed.

```sh
# From the repository root:
python3 references/prototypes/fleet-backlog/scenarios.py
python3 -m unittest discover -s references/prototypes/fleet-backlog -v

# N daemons; reproduce a fault schedule by seed:
python3 references/prototypes/fleet-backlog/scenarios.py --nodes 16 --seed 17 --steps 500

# Machine-readable oracle, including the required scenario traces:
python3 references/prototypes/fleet-backlog/scenarios.py --json > /tmp/fleet-backlog.json
diff -u references/prototypes/fleet-backlog/expected.json /tmp/fleet-backlog.json
```

Use normal Python, without `-O`: assertions are the model's oracle. The fixed
scenarios use five daemons to make the roles readable. `--nodes` sets the separate
fault simulation's fleet size (minimum three); there is no fixed fleet limit in
the model. `expected.json` is the default command's captured result, not a hand
written success report.

The full suite exchanges whole replicas and can take several minutes. Use the
default scenario command for a quick demonstration.

## Store decision: iroh-docs, with an explicit HLC projection

Choose **iroh-docs** for Fleet 10. It provides replicas with entries identified by
namespace, author and key; entries reference content by hash and size. This fits
Kanna's existing iroh peer transport. The content is separate from the replica,
so consumers must wait for the complete payload before applying it.
[iroh-docs 0.101.0 API](https://docs.rs/iroh-docs/0.101.0/iroh_docs/).

Use one fleet namespace, a device-bound author per daemon, and **immutable field
operations**, keyed by `task-id / birth / field / HLC`. The payload carries the
field value and full HLC. Resolve each field across authors by the largest
`(physical milliseconds, logical counter, device ID)`. Do not overwrite one
shared document key and assume the library's timestamp implements our order.
The Python `Replica` models the union of complete immutable entries and this
projection; it does **not** import iroh-docs or verify its sync implementation.
This is a choice of storage contract, not an integration or performance result.

Why the other two do not simplify this particular record:

| Candidate | Fit for this model |
| --- | --- |
| Automerge | Its map conflicts resolve by operation counter and actor ID, retaining competing values. We still need our HLC projection; these flat fields do not need collaborative text/list machinery. [Conflict rules](https://automerge.org/docs/reference/documents/conflicts/). |
| Loro | Its map uses Lamport timestamp LWW, not our physical/logical HLC order. It would also need a projection, plus transport integration. [Map rules](https://loro.dev/docs/tutorial/map). |
| iroh-docs | Immutable keys let the application define HLC ordering explicitly, using the fleet's chosen transport ecosystem. The tradeoff is an operation log and a projection we own. |

Compaction, real blob availability, signing/author-to-device binding, iroh
version compatibility and storage performance remain product integration work.
No claim here depends on an undocumented native iroh-docs conflict rule.

## What the simulation contains

`model.py` gives each daemon a separate replica, persisted HLC, authority journal,
outbox, capabilities, slots and placement configuration. The network queues,
duplicates, reorders or drops packets and partitions arbitrary groups. Offline
nodes keep their state. A restart replaces the daemon object from a deep copy of
its simulated disk. No daemon reads another daemon's journal: `Network.check()`
is an omniscient **test assertion**, never an arbitration service.

The task record contains name, first message, agent, `after`, `needs`, state,
owner, immutable creation HLC, placement preferences and app claim. Ordinary
fields merge independently. Lifecycle methods emit state and owner updates;
editing their displayed fields does not confer authority.

The HLC advances on local writes and message receipt, persists over restart and
stays monotonic through wall-clock rollback. Creation HLC never changes when a
task is renamed. Runners sort eligible waiting records by `(creation HLC, task
ID)` and ask the oldest they know. HLC preserves causality and deterministic
ties; it cannot reconstruct the actual creation order of disconnected machines
with skewed clocks. An unseen older task cannot win a local placement decision.

### Permission to start is separate from the replicated owner field

An owner serializes local starts and incoming asks. Its journal progresses:

```text
ready(epoch e) -- local execution --> running --> done
ready(epoch e) -- grant -----------> sent(epoch e), durable outbox(epoch e+1)
                                           |
                                  authenticated grant delivered
                                           v
                                receiver: ready(epoch e+1)
```

The creation daemon begins with epoch zero. A remote grant consumes its old
permission **before sending** the new permission. The receiver persists receipt
and its epoch before acknowledging. Source or destination restart, lost ACK,
duplicate grant, or ownership returning to an earlier owner cannot resurrect an
old permission. A receiver can start without waiting for the source to receive
the ACK. A lost grant may leave nobody able to run until the source retries;
it never creates two owners. Record replication alone cannot authorize a start.

The model treats each journal transition as atomic. Fleet 10 should commit the
retired authority, exact grant/outbox and pending record update in one local
transaction, then send/publish them. Retrying a committed grant must reuse its
epoch and recipient. A record adapter need not share that transaction: publish
its immutable operation from the durable outbox. Do not regenerate a competing
grant after a crash. Persist a start intent and reconcile it with the keeper;
the Python model's atomic simulated spawn does not prove real process exactly
once behavior.

Only previously unstarted tasks execute here. Task transfer, turn-boundary
resume, bundle/transcript handling and the real spawn/keeper crash window belong
to Fleet 11 and the product. The single-start invariant applies to each birth's
initial execution; it does not prohibit later explicit resumes in the product.

### Placement is replaceable policy

In pull mode, free capable workers ask. `needs` must be a subset of capabilities;
preferences are a hard allowlist that narrows eligible workers, with no timeout
fallback. App claims skip the task, dependencies must be known Done, and slots
are checked again at the receiver. Two owners can offer work against the same
slot advertisement; the receiver holds the extra task waiting without exceeding
its slots. Even a creation owner asks instead of skipping an older remote task.

Central mode is an explicit configuration change, not a timeout election:
existing owners grant waiting tasks to a scheduler, which uses the **same grant
method** to assign eligible workers. The scheduler need not have execution
capabilities. Running tasks stay put. Offline owners still wait. Pull requests
already in flight are refused by an owner that has adopted central mode.
Configuration propagation is modeled as an operator action on online nodes;
reapply/sweep when an offline node returns. Mixed policies during propagation
can change placement, but cannot bypass the one-owner rule. New waiting tasks
also need that sweep. No global instantaneous scheduler election is claimed.

The scheduler demo creates tasks on three different machines, then takes the
first creator offline while other machines run the remaining work. There is no
repo home or permanently privileged daemon. `build-mac` means this Mac; a mini
is neither assumed nor required.

## What it showed

Verified on this Mac with Python 3.9.6 on 2026-10-24: all 12 tests passed,
including the 60 fault schedules and the captured replay comparison (177 seconds).

The captured default run in `expected.json` includes:

| Scenario | Assertion / observation |
| --- | --- |
| Two runners ask before either response | One grant, one refusal, one start. Reversing delivery order reverses the winner. |
| Owner disappears before grant | Three simulated days pass with no start. Even a scheduler cannot take it. After the owner returns, it grants to the scheduler, which grants to a worker. |
| Grant and ACK lost | Source restart retries the committed grant; receiver restart and duplicate delivery still produce one start. |
| Scheduler takes over | Already running work stays on Linux; waiting tasks pass through the scheduler and land on the preferred build Mac / capable Linux worker. |
| Partitioned edits | Same-field writes resolve by full HLC; different-field changes survive; all replicas converge after healing. Slow and backward wall clocks preserve observed causality. |
| Placement guards | Claims, dependencies, capabilities, hard preferences and slot limits are enforced against the deciding daemon's known state. |
| ID collisions | A known 32-bit ID is refused on creation. Unseen concurrent collisions remain distinct births and are quarantined once discovered. |
| Deliberately unsafe LWW control | Both runners start before records converge to a single displayed owner. Eventual agreement alone is insufficient. |

The test suite additionally exercises local-start versus remote-ask ordering,
owner display arriving before a grant, an old grant replayed after ownership
returns, two owners offering one slot, stale asks, persisted clocks, and a source
disappearing after grant delivery. It runs 60 seeded schedules: 3, 8 and 16 nodes,
20 seeds each, 200 fault actions each. The oracle checks exclusive authority and
at most one initial start after every delivery/action; once faults stop, retry
and sync must finish every task exactly once and converge every replica.

This is bounded simulation, not a proof over every possible history. It assumes
authenticated honest peers, non-rollbackable local durable state and serialized
owner transitions. Signatures, authorization of imported lifecycle writes and
hostile-message validation are acknowledged product responsibilities, not model
security hardening. Full history and full replica exchange deliberately trade
efficiency for inspectable behavior.

## What Fleet 10 should copy

1. The immutable operation/field projection and full HLC ordering, including
   persisted clocks, device tie-breaks and immutable creation order.
2. The separation of replicated display state from durable start permission.
   Serialize asks and local starts; consume permission before sending; persist
   receipt, epochs, start intent and the exact retryable outbox.
3. The pull/central policy boundary. Both use identical ownership grants;
   capabilities, preferences, app claims, dependencies and slots are checked
   where decisions occur. No home machine, lease expiry or offline takeover.
4. The scenario actions and invariants in `scenarios.py` / `test_model.py`.
   Feed equivalent actions to real daemons; compare normalized grants, refusals,
   starts and converged records with `expected.json`. Grant packet ordering may
   differ; one winner and never two starts may not. Run both race orders.
5. Product integration checks the simulation cannot replace: durable crash
   injection at transaction/send/receive/spawn boundaries and real iroh-docs
   synchronization with delayed payloads. Passing this model is not evidence
   those integrations already work.

## What surprised me

The replicated `owner` field is not a lock even when every replica ultimately
agrees. The smallest safe protocol instead permits a temporary gap with **zero**
machines allowed to start. Losing availability at that point is intentional.

A scheduler takeover is ordinary ownership transfer, not a new coordination
primitive. It inherits the same inability to rescue an offline owner's work.

The spec's 32-bit collision check is necessarily local during a partition.
The model keeps an immutable birth identity behind the visible ID so it never
merges two unrelated grants. It quarantines detected ambiguity; if both births
already ran before discovery, that cannot be undone. Fleet 10 must keep the
local refusal and explicitly surface this rare conflict, rather than promise
global uniqueness without coordination.

Similarly, preferences, claims and dependency edits are eventually visible.
An owner rechecks an ask against what it knows, but cannot honor an edit still
hidden behind a partition. Stricter instantaneous placement or claim semantics
would require an additional owner-mediated protocol; this model does not imply
one from ordinary LWW fields.
