"""Executable specification, not a daemon or an iroh-docs implementation.

Each daemon has its own replica, clock, durable journal and placement policy.
Only Network delivers messages; no decision reads another daemon's journal.
Python 3.9+, standard library only.
"""
from copy import deepcopy
from dataclasses import dataclass, field
import random
from typing import Any, Dict, Optional, Tuple


@dataclass(frozen=True, order=True)
class Stamp:
    physical: int
    logical: int
    node: str

    def __deepcopy__(self, memo):
        return self  # immutable, safe to share across simulated packet copies


@dataclass
class Clock:
    node: str
    physical: int = 0
    logical: int = 0

    def tick(self, wall: int, remote: Optional[Stamp] = None) -> Stamp:
        rp, rl = (remote.physical, remote.logical) if remote else (0, 0)
        p = max(wall, self.physical, rp)
        if p == self.physical == rp:
            logical = max(self.logical, rl) + 1
        elif p == self.physical:
            logical = self.logical + 1
        elif p == rp:
            logical = rl + 1
        else:
            logical = 0
        self.physical, self.logical = p, logical
        return Stamp(p, logical, self.node)


@dataclass(frozen=True)
class Write:
    task: str
    birth: Stamp
    field: str
    stamp: Stamp
    value: Any

    def __deepcopy__(self, memo):
        # Avoid recursively rebuilding immutable identifiers for every gossip
        # packet, while still isolating potentially mutable field payloads.
        return Write(self.task, self.birth, self.field, self.stamp,
                     deepcopy(self.value, memo))


@dataclass
class Replica:
    # iroh-docs adapter contract: immutable keys, complete payloads, set union.
    # Physical key: task/birth/field/stamp; author = stamp.node.
    entries: Dict[Tuple, Write] = field(default_factory=dict)

    def merge(self, writes):
        for op in writes:
            key = (op.task, op.birth, op.field, op.stamp)
            if key in self.entries:
                assert self.entries[key] == op, "immutable key reused"
            self.entries[key] = op

    def births(self, task):
        return {op.birth for op in self.entries.values() if op.task == task}

    def record(self, task):
        births = self.births(task)
        if not births:
            return None
        if len(births) != 1:
            return None  # disconnected ID collision: quarantine, never conflate
        fields = {}
        for op in self.entries.values():
            if op.task == task and (op.field not in fields or
                                   op.stamp > fields[op.field].stamp):
                fields[op.field] = op
        return {name: op.value for name, op in fields.items()}

    def records(self):
        return {task: self.record(task) for task in
                sorted({op.task for op in self.entries.values()})}


@dataclass
class Authority:
    birth: Stamp
    epoch: int
    phase: str = "ready"  # ready -> running -> done, or ready -> sent


@dataclass
class Packet:
    source: str
    target: str
    kind: str
    body: Any


@dataclass
class Disk:
    clock: Clock
    replica: Replica = field(default_factory=Replica)
    authority: Dict[str, Authority] = field(default_factory=dict)
    outbox: Dict[Tuple, Packet] = field(default_factory=dict)
    starts: set = field(default_factory=set)
    scheduler: Optional[str] = None


class Daemon:
    def __init__(self, name, capabilities, slots=1, offset=0, disk=None):
        self.name, self.capabilities = name, frozenset(capabilities)
        self.slots, self.offset = slots, offset
        self.disk = disk or Disk(Clock(name))
        self.online = True
        self.net = None

    def stamp(self, remote=None):
        return self.disk.clock.tick(self.net.now + self.offset, remote)

    def ingest(self, writes):
        writes = list(writes)
        if writes:
            self.stamp(max(op.stamp for op in writes))
            self.disk.replica.merge(writes)

    def write(self, task, field_name, value):
        birth, = self.disk.replica.births(task)
        op = Write(task, birth, field_name, self.stamp(), deepcopy(value))
        self.disk.replica.merge([op])

    def edit(self, task, **fields):
        # Lifecycle fields are projections of protocol decisions, not edits.
        assert not ({"owner", "state", "created"} & fields.keys())
        for name, value in fields.items():
            self.write(task, name, value)

    def create(self, task=None, **fields):
        task = task or "task-%08x" % self.net.rng.getrandbits(32)
        if self.disk.replica.births(task):
            raise ValueError("task ID already in this replica")
        birth = self.stamp()
        values = dict(name=task, first_message="Do the task", agent="codex",
                      after=(), needs=(), preferred=(), claimed_by=None,
                      state="waiting", owner=self.name, created=birth)
        values.update(fields)
        assert values["owner"] == self.name and values["state"] == "waiting"
        self.disk.replica.merge([Write(task, birth, k, birth, deepcopy(v))
                                 for k, v in values.items()])
        self.disk.authority[task] = Authority(birth, 0)
        return task

    def free(self):
        return self.slots - sum(a.phase == "running"
                                for a in self.disk.authority.values())

    def eligible(self, record, worker):
        # Owner checks its own record, advertised capabilities and slots.
        # Advertisements may be stale; receiver checks again before starting.
        return (record and record["state"] == "waiting"
                and not record["claimed_by"]
                and set(record["needs"]) <= set(worker["capabilities"])
                and (not record["preferred"] or
                     worker["name"] in record["preferred"])
                and worker["free"] > 0
                and all((self.disk.replica.record(dep) or {}).get("state") == "done"
                        for dep in record["after"]))

    def advert(self):
        return dict(name=self.name, capabilities=tuple(self.capabilities),
                    free=self.free())

    def waiting(self):
        records = self.disk.replica.records()
        return sorted((t for t, r in records.items()
                       if r and r["state"] == "waiting"),
                      key=lambda t: (records[t]["created"], t))

    def ask(self):
        if not self.online or self.disk.scheduler is not None or self.free() <= 0:
            return None
        for task in self.waiting():
            record = self.disk.replica.record(task)
            if self.eligible(record, self.advert()):
                self.net.send(self.name, record["owner"], "ask",
                              (task, record["created"], self.advert()))
                self.net.log("ask", task=task, source=self.name, target=record["owner"])
                return task
        return None

    def grant(self, task, target, worker=None, adopt=False):
        """One serialized durable transition before any packet can escape."""
        if not self.online:
            return False
        authority = self.disk.authority.get(task)
        record = self.disk.replica.record(task)
        if not authority or authority.phase != "ready" or not record:
            return False
        if adopt:
            if target != self.disk.scheduler or record["state"] != "waiting":
                return False
        elif not worker or not self.eligible(record, worker):
            return False
        if target == self.name:
            return self.start(task)
        # Atomic model transaction: retire authority, publish owner, persist
        # exact grant in outbox. Product must commit all before sending.
        authority.phase = "sent"
        self.write(task, "owner", target)
        epoch = authority.epoch + 1
        packet = Packet(self.name, target, "grant", dict(
            task=task, birth=authority.birth, epoch=epoch,
            writes=list(self.disk.replica.entries.values())))
        self.disk.outbox[(task, authority.birth, epoch)] = packet
        self.net.queue.append(deepcopy(packet))
        self.net.log("grant", task=task, source=self.name, target=target, epoch=epoch)
        return True

    def start(self, task):
        a = self.disk.authority.get(task)
        if not self.online or not a or a.phase != "ready":
            return False
        if not self.eligible(self.disk.replica.record(task), self.advert()):
            return False
        key = (task, a.birth, a.epoch)
        assert key not in self.disk.starts
        # Start intent persists before a simulated spawn. A real keeper must
        # reconcile this intent after a crash, never blindly spawn again.
        self.disk.starts.add(key)
        a.phase = "running"
        self.write(task, "state", "running")
        self.net.log("start", task=task, node=self.name, epoch=a.epoch)
        self.net.check()
        return True

    def finish(self, task):
        a = self.disk.authority[task]
        assert self.online and a.phase == "running"
        a.phase = "done"
        self.write(task, "state", "done")

    def run_ready(self):
        if not self.online:
            return
        for task in self.waiting():
            if self.disk.scheduler == self.name:
                continue  # scheduler assigns, it doesn't implicitly execute
            a = self.disk.authority.get(task)
            if a and a.epoch > 0:
                self.start(task)  # accept an assignment; original owners ask too

    def assign(self, advertisements):
        assert self.disk.scheduler == self.name
        for task in self.waiting():
            for worker in advertisements:
                if worker["name"] != self.name and self.eligible(
                        self.disk.replica.record(task), worker):
                    if self.grant(task, worker["name"], worker):
                        worker["free"] -= 1
                        break

    def retry(self):
        if self.online:
            self.net.queue.extend(deepcopy(list(self.disk.outbox.values())))

    def receive(self, packet):
        if packet.kind == "sync":
            self.ingest(packet.body)
        elif packet.kind == "ask":
            task, birth, worker = packet.body
            a = self.disk.authority.get(task)
            allowed = self.disk.scheduler is None
            if (allowed and worker["name"] == packet.source and a and
                    a.birth == birth and self.grant(task, packet.source, worker)):
                return
            record = self.disk.replica.record(task)
            self.net.log("refused", task=task, node=self.name, asker=packet.source,
                         owner=record["owner"] if record else None)
        elif packet.kind == "grant":
            b = packet.body
            self.ingest(b["writes"])
            old = self.disk.authority.get(b["task"])
            # Authenticated, non-Byzantine senders; only grant() emits grants.
            # High-water mark survives returning ownership and daemon restarts.
            if old is None or (old.birth == b["birth"] and old.epoch < b["epoch"]):
                self.disk.authority[b["task"]] = Authority(b["birth"], b["epoch"])
            self.net.send(self.name, packet.source, "ack",
                          (b["task"], b["birth"], b["epoch"]))
        elif packet.kind == "ack":
            saved = self.disk.outbox.get(packet.body)
            if saved and saved.target == packet.source:
                del self.disk.outbox[packet.body]
        else:
            raise ValueError(packet.kind)


class Network:
    def __init__(self, daemons, seed=0):
        self.nodes = {d.name: d for d in daemons}
        assert len(self.nodes) == len(daemons)
        for d in daemons:
            d.net = self
        self.now = 1000
        self.rng = random.Random(seed)
        self.queue = []
        self.blocked = set()
        self.trace = []

    def log(self, event, **fields):
        self.trace.append(dict(event=event, **fields))

    def send(self, source, target, kind, body):
        self.queue.append(Packet(source, target, kind, deepcopy(body)))

    def connected(self, source, target):
        return (self.nodes[source].online and self.nodes[target].online
                and (source, target) not in self.blocked)

    def partition(self, *groups):
        group = {name: i for i, names in enumerate(groups) for name in names}
        assert set(group) == set(self.nodes)
        self.blocked = {(a, b) for a in group for b in group if group[a] != group[b]}

    def heal(self):
        self.blocked.clear()

    def deliver(self, index=0):
        packet = self.queue.pop(index)
        if self.connected(packet.source, packet.target):
            self.nodes[packet.target].receive(packet)
        else:
            self.log("drop", kind=packet.kind, source=packet.source, target=packet.target)
        self.check()
        return packet

    def drain(self, shuffled=False):
        while self.queue:
            self.deliver(self.rng.randrange(len(self.queue)) if shuffled else 0)

    def gossip(self):
        for source, d in self.nodes.items():
            for target in self.nodes:
                if source != target and self.connected(source, target):
                    self.send(source, target, "sync", list(d.disk.replica.entries.values()))
        self.drain(shuffled=True)

    def restart(self, name):
        old = self.nodes[name]
        # Simulated durable storage survives; all volatile object state is new.
        new = Daemon(name, old.capabilities, old.slots, old.offset, deepcopy(old.disk))
        new.net = self
        self.nodes[name] = new
        self.log("restart", node=name)
        return new

    def centralize(self, scheduler):
        # Explicit operator policy change, not timeout election. Offline nodes
        # receive configuration when they reconnect. No authority is fabricated.
        for d in self.nodes.values():
            if d.online:
                d.disk.scheduler = scheduler
                for task in d.waiting():
                    if d.name != scheduler:
                        d.grant(task, scheduler, adopt=True)
        self.log("scheduler", node=scheduler)

    def check(self):
        # Omniscient oracle ONLY for assertions; never consulted by a daemon.
        entitled, starts = {}, {}
        for d in self.nodes.values():
            assert d.free() >= 0
            for task, a in d.disk.authority.items():
                key = (task, a.birth)
                if a.phase in ("ready", "running"):
                    assert key not in entitled, ("two owners", key, entitled, d.name)
                    entitled[key] = d.name
            for task, birth, epoch in d.disk.starts:
                key = (task, birth)
                assert key not in starts, ("double start", key, starts, d.name, epoch)
                starts[key] = d.name

    def assert_converged(self):
        replicas = [d.disk.replica for d in self.nodes.values()]
        assert all(r.entries == replicas[0].entries for r in replicas)
        assert all(r.records() == replicas[0].records() for r in replicas)
