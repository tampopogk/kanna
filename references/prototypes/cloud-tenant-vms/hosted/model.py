import copy
import hashlib
import json
from dataclasses import dataclass

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

DOMAIN = "kanna/cloud-tenant-vms-hosted-model/v1"


class Refused(Exception):
    pass


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def verify(key, signature, message):
    try:
        Ed25519PublicKey.from_public_bytes(bytes.fromhex(key)).verify(bytes.fromhex(signature), message)
    except (InvalidSignature, ValueError, TypeError) as exc:
        raise Refused("bad_signature") from exc


class Device:
    def __init__(self, name):
        self.name = name
        self._secret = Ed25519PrivateKey.generate()
        self.key = self._secret.public_key().public_bytes(
            serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()
        self.log = None

    def descriptor(self, hosted=False):
        return dict(name=self.name, key=self.key, hosted=hosted)

    def bootstrap(self):
        self.log = PeerLog(self.descriptor())

    def entry(self, kind, payload, parents=None):
        body = dict(domain=DOMAIN, fleet=self.log.fleet, kind=kind, signer=self.key,
                    parents=sorted(self.log.events if parents is None else parents), payload=payload)
        return dict(body=body, signature=self._secret.sign(canonical(body)).hex())

    def add(self, other, hosted=False):
        entry = self.entry("add", other.descriptor(hosted))
        self.log.merge([entry])
        return entry

    def revoke(self, key):
        entry = self.entry("revoke", dict(key=key))
        self.log.merge([entry])
        return entry

    def join(self, log):
        self.log = copy.deepcopy(log)


@dataclass
class View:
    names: dict
    revoked: set
    hosted: set

    @property
    def trusted(self):
        return {next(iter(keys)) for keys in self.names.values()
                if len(keys) == 1 and next(iter(keys)) not in self.revoked}

    def admits(self, key):
        if key in self.revoked:
            raise Refused("revoked")
        if key not in self.trusted:
            raise Refused("unpaired")
        return key


class PeerLog:
    def __init__(self, anchor):
        self.anchor = copy.deepcopy(anchor)
        self.fleet = digest(anchor)
        self.events = {}
        self.pending = {}

    def view(self, ids=None):
        ids = list(self.events if ids is None else ids)
        bodies = {i: self.events[i]["body"] for i in ids}
        hosted = {b["payload"]["key"] for b in bodies.values() if b["kind"] == "add" and b["payload"].get("hosted")}
        revoked, seen = set(), {}
        for event_id, body in bodies.items():
            if body["kind"] != "revoke":
                continue
            target = body["payload"]["key"]
            if body["signer"] in hosted and target != body["signer"]:
                continue
            revoked.add(target)
            seen.setdefault(target, set()).update(self._ancestors(body["parents"]))
        names = {self.anchor["name"]: {self.anchor["key"]}}
        signers = {self.anchor["key"]}
        adds = [i for i, b in bodies.items() if b["kind"] == "add"]
        while True:
            left = []
            for event_id in adds:
                body = bodies[event_id]
                signer, payload = body["signer"], body["payload"]
                if signer in hosted:
                    continue
                if signer not in signers or (signer in revoked and event_id not in seen.get(signer, set())):
                    left.append(event_id)
                    continue
                signers.add(payload["key"])
                names.setdefault(payload["name"], set()).add(payload["key"])
            if len(left) == len(adds):
                break
            adds = left
        return View(names, revoked, hosted)

    def _ancestors(self, parents):
        found, todo = set(), list(parents)
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
            if body["signer"] in past.hosted:
                raise Refused("hosted_signer")
            if payload["key"] in past.revoked:
                raise Refused("revoked")
            if payload["name"] in past.names and past.names[payload["name"]] != {payload["key"]}:
                raise Refused("identity_mismatch")
            if payload["key"] in past.hosted and not payload.get("hosted"):
                raise Refused("hosted_downgrade")
        elif body["kind"] == "revoke":
            if body["signer"] in past.hosted and payload["key"] != body["signer"]:
                raise Refused("hosted_signer")
        else:
            raise Refused("unknown_entry")

    def merge(self, entries):
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
            ready = sorted(i for i, e in stage.pending.items() if all(p in stage.events for p in e["body"]["parents"]))
            if not ready:
                break
            for event_id in ready:
                entry = stage.pending.pop(event_id)
                stage._authorize(entry)
                stage.events[event_id] = entry
        self.events, self.pending = stage.events, stage.pending

    def force(self, entries):
        for entry in copy.deepcopy(entries):
            self.events[digest(entry)] = entry

    def export(self):
        return copy.deepcopy(list(self.events.values()) + list(self.pending.values()))
