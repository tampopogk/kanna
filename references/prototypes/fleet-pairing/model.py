"""Executable behavior model for fleet-identity.md; not a product protocol.

Real SPAKE2 and Ed25519, simulated transport/clock/app gesture. Private keys and
pairing codes exist only in memory. See README for the causal-log tradeoff.
"""

import copy
import hashlib
import hmac
import json
import secrets
import sqlite3
from dataclasses import dataclass

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey, Ed25519PublicKey,
)
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from spake2 import SPAKE2_A, SPAKE2_B


DOMAIN = "kanna/fleet-pairing-model/v1"


class Refused(Exception):
    """Stable reason strings are the behavioral comparison surface."""


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=True).encode()


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def verify(key, signature, message):
    try:
        Ed25519PublicKey.from_public_bytes(bytes.fromhex(key)).verify(
            bytes.fromhex(signature), message)
    except (InvalidSignature, ValueError, TypeError) as exc:
        raise Refused("bad_signature") from exc


class Device:
    def __init__(self, name):
        self.name = name
        self._secret = Ed25519PrivateKey.generate()
        self.key = self._secret.public_key().public_bytes(
            serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()
        self.log = None

    def descriptor(self):
        return dict(name=self.name, key=self.key, capabilities=["simulated"],
                    relay=None, addrs=[])

    def sign(self, message):
        return self._secret.sign(message).hex()

    def bootstrap(self):
        # A local first device is the trust anchor, never a directory response.
        self.log = PeerLog(self.descriptor())

    def entry(self, kind, payload, *, parents=None, claimed_time=0):
        """Raw signing primitive also used to simulate a compromised device."""
        body = dict(domain=DOMAIN, fleet=self.log.fleet, kind=kind,
                    signer=self.key, parents=sorted(self.log.events if parents is None
                                                   else parents),
                    payload=payload, claimed_time=claimed_time)
        return dict(body=body, signature=self.sign(canonical(body)))

    def revoke(self, key, claimed_time=0):
        entry = self.entry("revoke", dict(key=key), claimed_time=claimed_time)
        self.log.merge([entry])
        return entry

    def certificate(self, app_key):
        body = dict(domain=DOMAIN + "/app", key=app_key, device=self.key)
        return dict(body=body, signature=self.sign(canonical(body)))


@dataclass
class View:
    names: dict
    revoked: set

    @property
    def trusted(self):
        # Concurrent conflicting names fail closed, without a timestamp winner.
        return {next(iter(keys)) for keys in self.names.values() if len(keys) == 1
                and next(iter(keys)) not in self.revoked}

    def resolve(self, name, key):
        if name not in self.names:
            raise Refused("unpaired")
        if self.names[name] != {key}:
            raise Refused("identity_mismatch")
        if key in self.revoked:
            raise Refused("revoked")
        return key


class PeerLog:
    """Union of signed causal entries, with grow-only key tombstones.

    Tiny-model simplification: parents lists *all* known accepted event hashes,
    rather than just the frontier. Arrival order and claimed time confer no trust.
    Pending entries cannot authorize anything until their full history arrives.
    """

    def __init__(self, anchor):
        self.anchor = copy.deepcopy(anchor)
        self.fleet = digest(anchor)
        self.events = {}
        self.pending = {}

    def view(self, ids=None):
        names = {self.anchor["name"]: {self.anchor["key"]}}
        revoked = set()
        for event_id in self.events if ids is None else ids:
            body = self.events[event_id]["body"]
            payload = body["payload"]
            if body["kind"] == "revoke":
                revoked.add(payload["key"])
            else:
                names.setdefault(payload["name"], set()).add(payload["key"])
        return View(names, revoked)

    def _ancestors(self, parents):
        found = set()
        todo = list(parents)
        while todo:
            event_id = todo.pop()
            if event_id not in found:
                found.add(event_id)
                todo.extend(self.events[event_id]["body"]["parents"])
        return found

    def _authorize(self, entry):
        body = entry["body"]
        past = self.view(self._ancestors(body["parents"]))
        if body["signer"] not in past.trusted:
            raise Refused("untrusted_signer")
        payload = body["payload"]
        if body["kind"] == "add":
            if payload["key"] in past.revoked:
                raise Refused("revoked")
            if payload["name"] in past.names and past.names[payload["name"]] != {payload["key"]}:
                raise Refused("identity_mismatch")
        elif body["kind"] != "revoke":
            raise Refused("unknown_entry")

    def merge(self, entries):
        # Atomic batch: invalid input never partially changes the receiver.
        stage = copy.deepcopy(self)
        for entry in copy.deepcopy(entries):
            body = entry["body"]
            verify(body["signer"], entry["signature"], canonical(body))
            if body["domain"] != DOMAIN or body["fleet"] != self.fleet:
                raise Refused("wrong_fleet")
            event_id = digest(entry)
            if event_id not in stage.events:
                stage.pending[event_id] = entry
        while True:
            ready = sorted(event_id for event_id, entry in stage.pending.items()
                           if all(p in stage.events for p in entry["body"]["parents"]))
            if not ready:
                break
            for event_id in ready:
                entry = stage.pending.pop(event_id)
                stage._authorize(entry)
                stage.events[event_id] = entry
        self.events, self.pending = stage.events, stage.pending

    def export(self):
        return copy.deepcopy(dict(anchor=self.anchor,
                                  entries=list(self.events.values()) + list(self.pending.values())))

    def sync(self, envelope):
        if envelope["anchor"] != self.anchor:
            raise Refused("wrong_fleet")
        self.merge(envelope["entries"])

    def accept_endpoint(self, endpoint_key, certificate=None):
        if endpoint_key in self.view().revoked:
            raise Refused("untrusted_endpoint")
        if endpoint_key in self.view().trusted:
            return
        if certificate is None:
            raise Refused("untrusted_endpoint")
        body = certificate["body"]
        verify(body["device"], certificate["signature"], canonical(body))
        if (body["domain"] != DOMAIN + "/app" or body["key"] != endpoint_key
                or body["device"] not in self.view().trusted):
            raise Refused("untrusted_endpoint")

    def save(self, path):
        # Only public signed records are stored. Anchor is locally pinned by caller.
        with sqlite3.connect(path) as db:
            db.execute("CREATE TABLE IF NOT EXISTS peer_log (id TEXT PRIMARY KEY, wire TEXT)")
            for entry in self.export()["entries"]:
                db.execute("INSERT OR IGNORE INTO peer_log VALUES (?, ?)",
                           (digest(entry), canonical(entry).decode()))

    @classmethod
    def load(cls, path, pinned_anchor):
        log = cls(pinned_anchor)
        with sqlite3.connect(path) as db:
            entries = [json.loads(row[0]) for row in db.execute("SELECT wire FROM peer_log")]
        log.merge(entries)  # No trusted flag survives a restart: verify again.
        return log


class Offer:
    def __init__(self, device, now=0):
        self.device = device
        self.code = f"{secrets.randbelow(1_000_000):06d}".encode()
        self.session = secrets.token_hex(16)
        self.expires = now + 300
        self.attempts = 0
        self.used = False

    def begin(self, now):
        if self.used:
            raise Refused("code_used")
        if now >= self.expires:
            raise Refused("code_expired")
        if self.attempts >= 3:
            raise Refused("rate_limited")
        self.attempts += 1


def derive(shared, role):
    return HKDF(algorithm=hashes.SHA256(), length=32, salt=None,
                info=(DOMAIN + "/" + role).encode()).derive(shared)


def tag(shared, role, message):
    return hmac.new(derive(shared, role), message, hashlib.sha256).hexdigest()


class Party:
    """One side of SPAKE2, with identity binding and explicit key confirmation."""

    def __init__(self, role, device, code, hello_a, hello_b):
        self.role, self.device = role, device
        self.hello_a, self.hello_b = hello_a, hello_b
        factory = SPAKE2_A if role == "A" else SPAKE2_B
        self.pake = factory(code, idA=canonical(hello_a), idB=canonical(hello_b))
        self.outbound = self.pake.start()
        self.confirmed = False

    def finish(self, incoming):
        try:
            self.shared = self.pake.finish(incoming)
        except Exception as exc:
            raise Refused("bad_pake_message") from exc
        a, b = (self.outbound, incoming) if self.role == "A" else (incoming, self.outbound)
        self.transcript = canonical(dict(domain=DOMAIN, a=self.hello_a, b=self.hello_b,
                                         message_a=a.hex(), message_b=b.hex()))
        return dict(mac=tag(self.shared, "confirm-" + self.role, self.transcript),
                    signature=self.device.sign(self.transcript))

    def confirm(self, proof):
        other = "B" if self.role == "A" else "A"
        if not hmac.compare_digest(proof["mac"], tag(self.shared, "confirm-" + other, self.transcript)):
            raise Refused("pake_confirmation_failed")
        hello = self.hello_b if other == "B" else self.hello_a
        verify(hello["device"]["key"], proof["signature"], self.transcript)
        self.confirmed = True


def pair(inviter, offer, code, *, gesture, now=0, relay=None):
    """Run two isolated PAKE parties through an adversarial public-message relay.

    relay(label, value) can alter wire messages, but receives neither code nor
    private/session keys. The gesture is a simulated app decision, not a real UI.
    """
    if relay is None:
        relay = lambda label, value: value
    offer.begin(now)
    newcomer = offer.device
    if newcomer.log is not None:
        raise Refused("already_paired")
    hello_a = dict(domain=DOMAIN, device=inviter.descriptor(), anchor=inviter.log.anchor)
    hello_b = dict(domain=DOMAIN, device=newcomer.descriptor(), session=offer.session)
    a = Party("A", inviter, code, hello_a, relay("hello_b", copy.deepcopy(hello_b)))
    b = Party("B", newcomer, offer.code, relay("hello_a", copy.deepcopy(hello_a)), hello_b)
    proof_a = a.finish(relay("pake_b", b.outbound))
    proof_b = b.finish(relay("pake_a", a.outbound))
    a.confirm(relay("confirm_b", proof_b))
    b.confirm(relay("confirm_a", proof_a))
    offer.used = True
    if not gesture:
        raise Refused("gesture_required")
    # No add is signed before both confirmations and the app gesture.
    entry = inviter.entry("add", a.hello_b["device"])
    staged = copy.deepcopy(inviter.log)
    staged.merge([entry])
    payload = staged.export()
    wire = relay("handoff", dict(payload=payload,
                                 mac=tag(a.shared, "handoff", canonical(payload))))
    if not hmac.compare_digest(wire["mac"], tag(b.shared, "handoff", canonical(wire["payload"]))):
        raise Refused("handoff_tampered")
    if wire["payload"]["anchor"] != b.hello_a["anchor"]:
        raise Refused("wrong_fleet")
    received = PeerLog(b.hello_a["anchor"])
    received.sync(wire["payload"])
    if inviter.key not in received.view().trusted or newcomer.key not in received.view().trusted:
        raise Refused("untrusted_endpoint")
    inviter.log, newcomer.log = staged, received
    return entry


def fleet():
    """Three simulated devices; no machine access or real fleet credentials."""
    a, b, c = (Device(name) for name in ("mbp", "linux-vm", "build-mac"))
    a.bootstrap()
    for device in (b, c):
        offer = Offer(device)
        pair(a, offer, offer.code, gesture=True)
    b.log.sync(a.log.export())
    return a, b, c
