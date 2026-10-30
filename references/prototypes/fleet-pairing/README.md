# Fleet pairing model

Runnable reference for [fleet identity](../../../docs/specs/machine-identity.md)
and [Fleet §10, Models first](../../../docs/specs/machines.md#10-build-order), before
**Fleet 2: iroh endpoint and pairing**. Three devices (`mbp`, `linux-vm`, and
`build-mac`) run in one Python process with separate keys and log replicas.
These are simulated roles; no mini, server, account, or real device key is used.

This is a throwaway implementation kept as a behavior reference. It exercises
real SPAKE2 and Ed25519, with simulated transport, clock, and app confirmation.
It is not a cryptographic proof, a production protocol, or an iroh integration.

## Run

Python 3.9 or newer, from the repository root:

```sh
cd references/prototypes/fleet-pairing
python3 -m venv .venv
.venv/bin/python -m pip install -r requirements.txt
.venv/bin/python demo.py
.venv/bin/python demo.py --json
.venv/bin/python -m unittest -v
```

Only dependency installation needs network access. The demo asserts its expected
outcomes and exits nonzero on a mismatch. `--json` produces stable names and
reasons for product comparisons; keys, codes, timestamps, and signatures do not
appear in that output. Keys and codes are random on every run and stay in memory.
The SQLite test writes only public signed records into a temporary `kanna.db`,
then removes it. No secrets are fixtures or repository files.

Files: [model.py](model.py) implements pairing and the log,
[demo.py](demo.py) demonstrates the three required outcomes and recovery, and
[test_model.py](test_model.py) is the behavioral contract for Fleet 2.

## What it showed

Verified on 2026-10-24, this Mac (Darwin arm64), Python 3.9.6:
**28 tests passed**. The demo produces:

```text
PASS directory_key_substitution: pake_confirmation_failed
PASS paired: ['build-mac', 'linux-vm', 'mbp']
PASS forged_log_signature: bad_signature
PASS delivery_orders_converged: 24
PASS mutually_revoked: ['linux-vm', 'mbp']
PASS remaining_trusted: ['build-mac']
PASS trusted_peer_readd_revoked_key: revoked
PASS stale_self_readd_endpoint: untrusted_endpoint
PASS fresh_key_recovery: ['build-mac', 'mbp-repaired']
```

| Spec requirement | Executable evidence |
| --- | --- |
| Directory cannot substitute a key | Alter either device's public key or the initial fleet anchor during PAKE: key confirmation fails, neither log changes. A forged Ed25519 log signature and an otherwise valid signature by an unknown device are also refused. |
| Code is one-time, expires, rate-limited; signing takes a gesture | Wrong code fails; reuse fails; expiry at 300 seconds fails; after three attempts the offer closes to further attempts; a successful exchange without app confirmation creates no add. |
| Bind identity and the resulting log to pairing | Confirmation proves possession of each device key; replayed or reflected confirmations fail; a modified log handoff fails its session MAC. |
| Two devices revoking each other both end up revoked | Partition A and B, sign opposite revocations with absurdly old/future timestamps, then deliver the two adds and two revokes in all **24 permutations**, to each of three replicas (**72 runs**). All converge to C as the only trusted device. |
| Revoked key never returns | Try the old name, another name, a trusted signer's re-add, the revoked signer's stale self-add, duplicate history, a stale snapshot after SQLite reload, and a certificate pointing at the revoked endpoint. None restores access. |
| Name returning with a new key is refused | A new PAKE succeeds cryptographically but admission fails with `identity_mismatch`. Concurrent conflicting adds also fail name resolution after merge. |
| Verify on sync and load | Tampered address metadata fails sync; a modified SQLite signature fails reload. Missing history remains pending and cannot confer trust. |
| Recovery | C pairs a new key for `mbp-repaired`. Both old keys remain revoked in the received history. |
| App key chains to a trusted device | A valid certificate works only for its specified endpoint; revoking the device invalidates access through its app certificate too. |

## Pairing choices

The newcomer makes a random six-digit code, a random session ID, and an offer
lasting five simulated minutes with three attempts. The inviter uses `SPAKE2_A`,
the newcomer `SPAKE2_B`. Their identity strings include both device descriptors,
the fleet anchor, the protocol domain, and the session ID. Both sides then verify
role-separated HMAC confirmations over the complete transcript and Ed25519
proofs of possession of the device keys. A separate HKDF label derives the log
handoff MAC key. Only after both confirmations and the simulated app gesture does
the inviter sign an add; the newcomer verifies the authenticated log before
installing its trust anchor. A directory provides no initial trust.

This uses the [python-spake2 API](https://github.com/warner/python-spake2), whose
identity strings bind the exchange to its participants. Explicit mutual
confirmation follows the requirement described in
[RFC 9382 §3.3–4](https://www.rfc-editor.org/rfc/rfc9382.html#section-3.3).
The package's group/profile and this model's message encoding/key schedule are
**not claimed to implement the RFC's wire format**. Fleet 2 should choose its
reviewed Rust implementation and compare behavior, not copy Python crypto glue.

The existing fleet's first device pins its own public descriptor locally. A
newcomer learns that anchor only through the authenticated handoff. Restart tests
receive an already pinned anchor; they do not trust one supplied by a database or
directory. Public log records are authenticated here, not encrypted: transport
confidentiality belongs to the product's iroh link.

## Log choices that make the result possible

Each signed entry contains a fleet ID, signer, kind, payload, parent hashes, and
an intentionally untrusted `claimed_time`. Adds cover the name, key,
capabilities, relay, and addresses. Content hashes identify immutable entries.
The model's parents include all locally accepted entries; a product could use a
causal frontier with the same meaning.

1. Verify every signature and fleet ID. Union records; never replace local state
   with a directory's snapshot. Defer records with missing ancestors.
2. Check the author's authority in the entry's signed ancestor history, including
   revocations in that history. This is “trusted at that point in the log.”
3. Keep every valid revoke as a permanent key tombstone. Current access is the
   admitted keys minus **all** known tombstones. A later or concurrent add cannot
   defeat a revoke, under any name or claimed timestamp.
4. Preserve historical name bindings. A causal attempt to replace a name's key
   fails. Concurrent conflicts remain recorded, but ambiguous names fail closed;
   no arrival order or clock chooses an identity.

An old-context add of a revoked key may remain as signed historical evidence.
It never adds that key back to the effective trusted set. Removing that evidence
or reevaluating every old entry using only today's trust would make replicas
disagree, and could invalidate the revocation that must remain effective.

## What surprised us, and the limits

**A flat signed list is insufficient.** If A revokes B first at a receiver, a
validator using only its current trusted set would reject B's concurrent revoke
of A. Reversing delivery changes the winner. Signed causal histories plus a union
of revocation tombstones preserve both revocations, without trusting timestamps.

**Causal history proves a stated context, not when someone actually signed.** A
stolen key can omit a revocation from its parents. Its attempt to restore the
*same* key still fails, and since Fleet 2 an add it signs on a branch the
revoker never saw counts for nothing: an add takes effect only through a chain
of effective adds from the anchor, and an add by a revoked key must be among the
ancestors of a revocation of that key. So the stale key can't admit a different
key either, nor can a key it admitted admit another. It can still sign
revocations on a stale branch: those only shut devices out (re-pairing fixes
that), and they have to stand, or two devices revoking each other wouldn't both
end up revoked. Fleet 2 keeps the app gesture and the key sandbox, and this is
still not complete compromise recovery.

**An offline peer cannot enforce a revocation it has not received.** The test
makes this explicit. The no-return invariant begins when a valid revocation is
available at that replica and persists across subsequent merges and reloads.
A malicious directory can still withhold updates. Mutual-revocation delivery
is simulated by the message carrier; a revoked endpoint does not regain an
application connection to deliver it. The product needs a path for authenticated
log evidence (for example through the surviving third peer) separately from
authorization to run commands.

**Recovery uses a new key and a new name.** This follows both “revoked keys never
return” and “same name, different key is refused.” Reusing the old display name
would need an explicit spec rule. Recovery with no remaining trusted device,
including starting a new fleet from a device still held, is not modeled.

Model-only limitations: the gesture is a boolean; offer state is in memory;
pairing commits both replicas in one process; there is no crash/retry protocol,
clock advancement during an exchange, persistent rate limit, parser/resource
hardening, key-file permissions, sandbox integration, or transport. None of
these shortcuts should be copied into the product. These are acknowledged
model limitations, not security claims about the product.

## What Fleet 2 should copy

Port the named scenarios and their assertions in `test_model.py`, and match
`demo.py --json`'s semantic outcomes with a product adapter (normalize random
keys to the same device names). Copy identity/session binding, explicit mutual
confirmation, the gesture before admission, authenticated anchor/log handoff,
signature verification on load and sync, causal authorization, permanent
revocation tombstones, and refusal of name/key substitution. Check both daemon
and certified app endpoint authorization after revocation.

Keep the behavioral tests as the comparison surface. The Python classes,
SQLite schema, all-ancestors representation, crypto framing, and simulated
transport are implementation conveniences, not the product interface.
