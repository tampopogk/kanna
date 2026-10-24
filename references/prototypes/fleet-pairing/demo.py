"""Print reproducible semantic outcomes; random public/private keys stay unprinted."""

import argparse
import itertools
import json

from model import Device, Offer, PeerLog, Refused, pair


def refusal(action):
    try:
        action()
    except Refused as exc:
        return str(exc)
    raise AssertionError("attack unexpectedly succeeded")


def run():
    a, b, c = (Device(name) for name in ("mbp", "linux-vm", "build-mac"))
    a.bootstrap()
    offer = Offer(b)
    forged = Device("directory")

    def substitute(slot, value):
        if slot == "hello_b":
            value["device"]["key"] = forged.key
        return value

    result = {"directory_key_substitution": refusal(lambda:
        pair(a, offer, offer.code, gesture=True, relay=substitute))}
    assert b.log is None and not a.log.events
    pair(a, offer, offer.code, gesture=True)
    offer = Offer(c)
    pair(a, offer, offer.code, gesture=True)
    b.log.sync(a.log.export())
    result["paired"] = sorted(d.name for d in (a, b, c) if d.key in c.log.view().trusted)

    forged_entry = a.entry("add", forged.descriptor())
    forged_entry["signature"] = forged.sign(b"forged")
    result["forged_log_signature"] = refusal(lambda: c.log.merge([forged_entry]))
    past = list(a.log.events)
    a.revoke(b.key, claimed_time=10**30)
    b.revoke(a.key, claimed_time=-(10**30))
    entries = {**a.log.events, **b.log.events}
    checked = 0
    for order in itertools.permutations(entries.values()):
        log = PeerLog(c.log.anchor)
        for entry in order:
            log.merge([entry])
        assert log.view().revoked == {a.key, b.key}
        assert log.view().trusted == {c.key}
        checked += 1
    result["delivery_orders_converged"] = checked
    c.log.sync(a.log.export())
    c.log.sync(b.log.export())
    result["mutually_revoked"] = sorted(d.name for d in (a, b, c) if d.key in c.log.view().revoked)
    result["remaining_trusted"] = [c.name]

    result["trusted_peer_readd_revoked_key"] = refusal(lambda:
        c.log.merge([c.entry("add", dict(b.descriptor(), name="new-name"))]))
    stale = b.entry("add", dict(b.descriptor(), name="another-name"), parents=past)
    c.log.merge([stale])
    result["stale_self_readd_endpoint"] = refusal(lambda: c.log.accept_endpoint(b.key))
    replacement = Device("mbp-repaired")
    offer = Offer(replacement)
    pair(c, offer, offer.code, gesture=True)
    assert replacement.log.view().revoked == {a.key, b.key}
    assert replacement.log.view().trusted == {c.key, replacement.key}
    result["fresh_key_recovery"] = [c.name, replacement.name]
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="stable behavioral comparison output")
    args = parser.parse_args()
    outcomes = run()
    expected = {
        "directory_key_substitution": "pake_confirmation_failed",
        "paired": ["build-mac", "linux-vm", "mbp"],
        "forged_log_signature": "bad_signature",
        "delivery_orders_converged": 24,
        "mutually_revoked": ["linux-vm", "mbp"],
        "remaining_trusted": ["build-mac"],
        "trusted_peer_readd_revoked_key": "revoked",
        "stale_self_readd_endpoint": "untrusted_endpoint",
        "fresh_key_recovery": ["build-mac", "mbp-repaired"],
    }
    assert outcomes == expected, (outcomes, expected)
    if args.json:
        print(json.dumps(outcomes, indent=2, sort_keys=True))
    else:
        for case, outcome in outcomes.items():
            print(f"PASS {case}: {outcome}")
