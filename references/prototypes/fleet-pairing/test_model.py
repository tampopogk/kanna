"""Behavioral contract: port these scenarios to Fleet 2's integration tests."""

import copy
import itertools
import json
import sqlite3
import tempfile
import unittest
from pathlib import Path

from model import Device, Offer, PeerLog, Refused, canonical, fleet, pair


class PairingTests(unittest.TestCase):
    def setUp(self):
        self.a, self.b = Device("mbp"), Device("linux-vm")
        self.a.bootstrap()
        self.offer = Offer(self.b)

    def refused_pair(self, reason, **kwargs):
        before = self.a.log.export()
        with self.assertRaisesRegex(Refused, "^" + reason + "$"):
            pair(self.a, self.offer, kwargs.pop("code", self.offer.code),
                 gesture=kwargs.pop("gesture", True), **kwargs)
        self.assertEqual(before, self.a.log.export())
        self.assertIsNone(self.b.log)

    def test_real_pake_pairs_three_devices(self):
        a, b, c = fleet()
        for device in (a, b, c):
            self.assertEqual(device.log.view().trusted, {a.key, b.key, c.key})
            self.assertEqual(device.log.export(), a.log.export())

    def test_wrong_code_does_not_add_either_side(self):
        wrong = b"000000" if self.offer.code != b"000000" else b"000001"
        self.refused_pair("pake_confirmation_failed", code=wrong)

    def test_directory_cannot_substitute_either_key(self):
        forged = Device("directory").key
        for label in ("hello_a", "hello_b"):
            with self.subTest(direction=label):
                def substitute(slot, value):
                    if slot == label:
                        value["device"]["key"] = forged
                    return value
                self.refused_pair("pake_confirmation_failed", relay=substitute)

    def test_directory_cannot_substitute_bootstrap_anchor(self):
        def substitute(slot, value):
            if slot == "hello_a":
                value["anchor"] = Device("directory").descriptor()
            return value
        self.refused_pair("pake_confirmation_failed", relay=substitute)

    def test_confirmation_proves_device_key_possession(self):
        impostor = Device("impostor")
        def substitute(slot, value):
            if slot == "confirm_b":
                # Even a valid PAKE MAC cannot rescue a forged Ed25519 proof.
                value["signature"] = impostor.sign(b"unrelated transcript")
            return value
        self.refused_pair("bad_signature", relay=substitute)

    def test_replayed_confirmation_fails_in_fresh_session(self):
        captured = {}
        def capture(slot, value):
            captured[slot] = copy.deepcopy(value)
            return value
        self.refused_pair("gesture_required", gesture=False, relay=capture)
        self.offer = Offer(self.b)
        self.refused_pair("pake_confirmation_failed", relay=lambda slot, value:
                          captured[slot] if slot == "confirm_b" else value)

    def test_reflected_confirmation_fails(self):
        captured = {}
        def reflect(slot, value):
            if slot == "confirm_b":
                captured[slot] = value
            if slot == "confirm_a":
                return captured["confirm_b"]
            return value
        self.refused_pair("pake_confirmation_failed", relay=reflect)

    def test_log_handoff_is_authenticated(self):
        def substitute(slot, value):
            if slot == "handoff":
                value["payload"]["entries"] = []
            return value
        self.refused_pair("handoff_tampered", relay=substitute)

    def test_app_gesture_required(self):
        self.refused_pair("gesture_required", gesture=False)
        self.assertTrue(self.offer.used)

    def test_one_time_code(self):
        pair(self.a, self.offer, self.offer.code, gesture=True)
        with self.assertRaisesRegex(Refused, "^code_used$"):
            pair(self.a, self.offer, self.offer.code, gesture=True)

    def test_expiry_at_five_minutes(self):
        self.refused_pair("code_expired", now=300)

    def test_online_attempt_budget(self):
        wrong = b"000000" if self.offer.code != b"000000" else b"000001"
        for _ in range(3):
            self.refused_pair("pake_confirmation_failed", code=wrong)
        self.refused_pair("rate_limited")


class LogTests(unittest.TestCase):
    def setUp(self):
        self.a, self.b, self.c = fleet()
        self.devices = (self.a, self.b, self.c)

    def test_forged_signature_and_unsigned_peer_refused(self):
        attacker = Device("directory")
        entry = self.a.entry("add", attacker.descriptor())
        entry["signature"] = attacker.sign(canonical(entry["body"]))
        before = self.c.log.export()
        with self.assertRaisesRegex(Refused, "^bad_signature$"):
            self.c.log.merge([entry])
        self.assertEqual(before, self.c.log.export())
        with self.assertRaisesRegex(Refused, "^untrusted_endpoint$"):
            self.c.log.accept_endpoint(attacker.key)

    def test_valid_signature_from_untrusted_signer_refused(self):
        attacker = Device("directory")
        attacker.log = copy.deepcopy(self.a.log)
        for kind, payload in (("add", attacker.descriptor()), ("revoke", {"key": self.a.key})):
            with self.subTest(kind=kind), self.assertRaisesRegex(Refused, "^untrusted_signer$"):
                self.c.log.merge([attacker.entry(kind, payload)])

    def test_metadata_tampering_refused_on_sync(self):
        entry = copy.deepcopy(next(iter(self.a.log.events.values())))
        entry["body"]["payload"]["addrs"] = ["attacker.invalid:1234"]
        with self.assertRaisesRegex(Refused, "^bad_signature$"):
            self.c.log.merge([entry])

    def test_identity_mismatch_is_not_a_key_rotation(self):
        replacement = Device(self.b.name)
        offer = Offer(replacement)
        with self.assertRaisesRegex(Refused, "^identity_mismatch$"):
            pair(self.a, offer, offer.code, gesture=True)
        self.assertIsNone(replacement.log)
        with self.assertRaisesRegex(Refused, "^identity_mismatch$"):
            self.c.log.view().resolve(self.b.name, replacement.key)

    def test_cross_fleet_replay_refused(self):
        other = Device("other-root")
        other.bootstrap()
        with self.assertRaisesRegex(Refused, "^wrong_fleet$"):
            other.log.merge(list(self.a.log.events.values()))

    def test_mutual_revocation_all_24_delivery_orders_and_replay(self):
        # Both sign from the same trusted past, partitioned from each other.
        self.a.revoke(self.b.key, claimed_time=10**30)
        self.b.revoke(self.a.key, claimed_time=-(10**30))
        entries = {**self.a.log.events, **self.b.log.events}
        self.assertEqual(len(entries), 4)
        for order in itertools.permutations(entries.values()):
            for observer in self.devices:
                # A/B already know their own revoke; C knows neither. Preserve
                # those different local histories while permuting delivery.
                log = copy.deepcopy(observer.log)
                seen_revoked = set(log.view().revoked)
                for entry in order:
                    log.merge([entry])
                    self.assertTrue(seen_revoked <= log.view().revoked)
                    seen_revoked |= log.view().revoked
                    self.assertFalse(seen_revoked & log.view().trusted)
                log.merge(list(entries.values()) * 2)
                self.assertFalse(log.pending)
                self.assertEqual(log.view().revoked, {self.a.key, self.b.key})
                self.assertEqual(log.view().trusted, {self.c.key})

    def test_revoked_author_cannot_sign_after_known_revocation(self):
        self.c.revoke(self.a.key)
        self.a.log.sync(self.c.log.export())
        with self.assertRaisesRegex(Refused, "^untrusted_signer$"):
            self.a.revoke(self.c.key)

    def test_revoked_key_never_returns_by_name_replay_or_stale_branch(self):
        past = list(self.a.log.events)
        self.c.revoke(self.b.key)
        for device in (self.a, self.b):
            device.log.sync(self.c.log.export())
        for signer in (self.b, self.c):
            for name in (self.b.name, "renamed-stolen-device"):
                payload = dict(self.b.descriptor(), name=name)
                with self.subTest(signer=signer.name, name=name):
                    # Full current context: rejected outright.
                    expected = "untrusted_signer" if signer is self.b else "revoked"
                    with self.assertRaisesRegex(Refused, "^" + expected + "$"):
                        self.c.log.merge([signer.entry("add", payload)])
                    # A compromised signer can pretend it never saw the revoke.
                    # Its old-context signature is historical evidence only:
                    # permanent tombstones still prevent endpoint authorization.
                    stale = signer.entry("add", payload, parents=past, claimed_time=10**30)
                    self.c.log.merge([stale])
                    self.assertNotIn(self.b.key, self.c.log.view().trusted)
        for device in (self.a, self.b):
            device.log.sync(self.c.log.export())
            self.assertNotIn(self.b.key, device.log.view().trusted)
            with self.assertRaisesRegex(Refused, "^untrusted_endpoint$"):
                device.log.accept_endpoint(self.b.key)

    def test_offline_revocation_takes_effect_when_delivered(self):
        self.a.revoke(self.b.key)
        self.assertIn(self.b.key, self.c.log.view().trusted)
        self.c.log.sync(self.a.log.export())
        self.assertNotIn(self.b.key, self.c.log.view().trusted)

    def test_missing_history_never_authorizes(self):
        revoke = self.b.revoke(self.a.key)
        log = PeerLog(self.a.log.anchor)
        log.merge([revoke])
        self.assertEqual(len(log.pending), 1)
        self.assertFalse(log.view().revoked)
        log.sync(self.b.log.export())
        self.assertFalse(log.pending)
        self.assertEqual(log.view().revoked, {self.a.key})

    def test_certificate_is_bound_to_endpoint_and_revoked_device(self):
        app = Device("app")
        cert = self.b.certificate(app.key)
        self.c.log.accept_endpoint(app.key, cert)
        with self.assertRaisesRegex(Refused, "^untrusted_endpoint$"):
            self.c.log.accept_endpoint(Device("impostor").key, cert)
        self.c.revoke(self.b.key)
        with self.assertRaisesRegex(Refused, "^untrusted_endpoint$"):
            self.c.log.accept_endpoint(app.key, cert)

    def test_certificate_cannot_launder_a_revoked_endpoint_key(self):
        self.c.revoke(self.b.key)
        cert = self.a.certificate(self.b.key)
        with self.assertRaisesRegex(Refused, "^untrusted_endpoint$"):
            self.c.log.accept_endpoint(self.b.key, cert)

    def test_surviving_third_device_pairs_a_fresh_key(self):
        self.a.revoke(self.b.key)
        self.b.revoke(self.a.key)
        for device in (self.a, self.b):
            self.c.log.sync(device.log.export())
        replacement = Device("mbp-repaired")
        offer = Offer(replacement)
        pair(self.c, offer, offer.code, gesture=True)
        self.assertEqual(replacement.log.view().trusted, {self.c.key, replacement.key})
        self.assertEqual(replacement.log.view().revoked, {self.a.key, self.b.key})

    def test_concurrent_name_collision_fails_closed_in_both_orders(self):
        x, y = Device("duplicate"), Device("duplicate")
        left = self.a.entry("add", x.descriptor())
        right = self.b.entry("add", y.descriptor())
        for order in ((left, right), (right, left)):
            log = copy.deepcopy(self.c.log)
            for entry in order:
                log.merge([entry])
            for key in (x.key, y.key):
                self.assertNotIn(key, log.view().trusted)
                with self.assertRaisesRegex(Refused, "^identity_mismatch$"):
                    log.view().resolve("duplicate", key)

    def test_sqlite_reload_preserves_tombstones_and_verifies_signatures(self):
        self.c.revoke(self.b.key)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "kanna.db"
            self.c.log.save(path)
            loaded = PeerLog.load(path, self.c.log.anchor)
            self.assertEqual(loaded.view(), self.c.log.view())
            loaded.sync(self.a.log.export())  # Stale directory snapshot is a union, not replacement.
            self.assertNotIn(self.b.key, loaded.view().trusted)
            with sqlite3.connect(path) as db:
                row_id, wire = db.execute("SELECT id, wire FROM peer_log LIMIT 1").fetchone()
                forged = json.loads(wire)
                forged["signature"] = "00" * 64
                db.execute("UPDATE peer_log SET wire = ? WHERE id = ?",
                           (json.dumps(forged), row_id))
            with self.assertRaisesRegex(Refused, "^bad_signature$"):
                PeerLog.load(path, self.c.log.anchor)


if __name__ == "__main__":
    unittest.main(verbosity=2)
