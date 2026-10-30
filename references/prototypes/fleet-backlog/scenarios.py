"""Deterministic acceptance scenarios and replayable adversarial schedules."""
import argparse
from copy import deepcopy
from dataclasses import asdict
import json

from model import Daemon, Network


def fleet(seed=0):
    return Network([Daemon("creator", (), slots=0),
                    Daemon("linux-a", ("linux",)),
                    Daemon("linux-b", ("linux",)),
                    Daemon("build-mac", ("macos", "xcode", "display")),
                    Daemon("scheduler", (), slots=0)], seed)


def started(net, task):
    return [d.name for d in net.nodes.values()
            if any(t == task for t, _, _ in d.disk.starts)]


def race(reverse=False):
    net = fleet()
    owner = net.nodes["creator"]
    task = owner.create(needs=("linux",))
    net.gossip()
    runners = [net.nodes["linux-a"], net.nodes["linux-b"]]
    if reverse:
        runners.reverse()
    for d in runners:
        assert d.ask() == task
    assert len(net.queue) == 2  # both asks exist before owner handles either
    net.drain()
    for d in runners:
        d.run_ready()
    assert started(net, task) == [runners[0].name]
    assert [e["event"] for e in net.trace[:3]] == ["ask", "ask", "grant"]
    assert any(e["event"] == "refused" and e["owner"] == runners[0].name
               for e in net.trace)
    net.gossip()
    net.assert_converged()
    return dict(winner=runners[0].name, starts=1, trace=net.trace)


def offline_owner():
    net = fleet()
    owner = net.nodes["creator"]
    task = owner.create(needs=("linux",))
    net.gossip()
    owner.online = False
    for _ in range(3):
        net.now += 86_400_000  # arbitrary elapsed time never elects an owner
        for d in net.nodes.values():
            d.ask()
        net.drain()
    assert started(net, task) == []
    # A scheduler is not an offline-owner recovery mechanism either.
    net.centralize("scheduler")
    net.drain()
    assert net.nodes["scheduler"].disk.authority.get(task) is None
    assert owner.disk.authority[task].phase == "ready"
    owner = net.restart("creator")
    net.centralize("scheduler")
    net.drain()
    net.nodes["scheduler"].assign([net.nodes["linux-b"].advert()])
    net.drain()
    net.nodes["linux-b"].run_ready()
    assert started(net, task) == ["linux-b"]
    net.gossip()
    net.assert_converged()
    return dict(waited_days=3, starts_after_owner_returned=1, trace=net.trace)


def lost_grant():
    net = fleet()
    owner = net.nodes["creator"]
    task = owner.create(needs=("linux",))
    net.gossip()
    net.nodes["linux-a"].ask()
    net.deliver()  # source durably relinquishes; grant is still queued
    grant = deepcopy(net.queue[0])
    net.partition(["creator"], ["linux-a", "linux-b", "build-mac", "scheduler"])
    net.drain()  # grant lost
    assert owner.disk.authority[task].phase == "sent"
    assert not owner.start(task)
    net.nodes["linux-b"].ask()
    net.drain()
    assert started(net, task) == []
    owner = net.restart("creator")
    net.heal()
    owner.retry()
    net.deliver()  # receive grant
    # Lose ACK, restart receiver, replay exact grant twice.
    net.queue.clear()
    receiver = net.restart("linux-a")
    net.queue.extend([deepcopy(grant), deepcopy(grant)])
    net.drain(shuffled=True)
    receiver.run_ready()
    receiver = net.restart("linux-a")
    net.queue.append(grant)
    net.drain()
    receiver.run_ready()
    assert started(net, task) == ["linux-a"]
    assert len(receiver.disk.starts) == 1
    assert owner.disk.outbox == {}
    net.gossip()
    net.assert_converged()
    return dict(lost_grant_and_ack=True, restarts=3, starts=1, trace=net.trace)


def scheduler_takeover():
    net = fleet()
    origin = net.nodes["creator"]
    tasks = [origin.create(needs=("linux",), preferred=("linux-b",)),
             net.nodes["build-mac"].create(needs=("xcode",), preferred=("build-mac",)),
             net.nodes["linux-a"].create(needs=("linux",))]
    net.gossip()
    assert net.nodes["linux-b"].ask() == tasks[0]
    net.drain()
    net.nodes["linux-b"].run_ready()
    # A request already in flight when central policy is enabled is refused.
    net.nodes["build-mac"].ask()
    net.centralize("scheduler")
    net.drain(shuffled=True)
    assert started(net, tasks[0]) == ["linux-b"]  # running task never moved
    scheduler = net.nodes["scheduler"]
    assert scheduler.disk.authority[tasks[1]].phase == "ready"
    assert scheduler.disk.authority[tasks[2]].phase == "ready"
    assert scheduler.capabilities == frozenset()  # owning != executing
    origin.online = False  # no project home required to place the rest
    assert all(d.ask() is None for d in net.nodes.values())
    scheduler.assign([net.nodes[n].advert() for n in
                      ("linux-a", "linux-b", "build-mac")])
    net.drain()
    for d in net.nodes.values():
        d.run_ready()
    assert started(net, tasks[1]) == ["build-mac"]
    assert started(net, tasks[2]) == ["linux-a"]
    net.check()
    origin.online = True
    net.gossip()
    net.assert_converged()
    return dict(owners=[net.nodes["linux-a"].disk.replica.record(t)["owner"]
                        for t in tasks], creator_needed=False, trace=net.trace)


def partitioned_edits():
    net = fleet()
    a, b = net.nodes["linux-a"], net.nodes["linux-b"]
    a.offset, b.offset = 5000, -500
    first = a.create(name="first")
    net.gossip()
    second = b.create(name="causally later, slow wall clock")
    assert a.disk.replica.record(first)["created"] < b.disk.replica.record(second)["created"]
    net.partition(["linux-a", "build-mac"], ["linux-b", "creator", "scheduler"])
    a.edit(first, name="A", needs=("linux",))
    b.edit(first, name="B", first_message="independent edit survives")
    before = b.stamp()
    b.offset -= 100_000
    assert b.stamp() > before
    net.gossip()
    assert a.disk.replica.record(first)["name"] != b.disk.replica.record(first)["name"]
    net.heal()
    net.gossip()
    net.assert_converged()
    record = a.disk.replica.record(first)
    assert record["needs"] == ("linux",)
    assert record["first_message"] == "independent edit survives"
    winner = max((op for op in a.disk.replica.entries.values()
                  if op.task == first and op.field == "name"), key=lambda op: op.stamp)
    assert record["name"] == winner.value
    assert all(d.waiting().index(first) < d.waiting().index(second)
               for d in net.nodes.values())
    snapshot = {task: dict(record, created=asdict(record["created"]))
                for task, record in a.disk.replica.records().items()}
    return dict(field_winner=winner.value, causal_order=True,
                rollback_monotonic=True, converged=True,
                records=snapshot, order=a.waiting())


def guards():
    net = fleet()
    o = net.nodes["creator"]
    claimed = o.create(needs=("linux",), claimed_by="mbp")
    dep = o.create(needs=("linux",), preferred=("linux-a",))
    blocked = o.create(needs=("linux",), after=(dep,), preferred=("linux-b",))
    mac = o.create(needs=("xcode",), preferred=("build-mac",))
    net.gossip()
    assert net.nodes["linux-b"].ask() is None
    assert net.nodes["linux-a"].ask() == dep
    assert net.nodes["build-mac"].ask() == mac
    net.drain()
    for d in net.nodes.values():
        d.run_ready()
    assert started(net, claimed) == started(net, blocked) == []
    assert net.nodes["linux-a"].ask() is None  # full
    net.nodes["linux-a"].finish(dep)
    net.gossip()
    assert net.nodes["linux-b"].ask() == blocked
    net.drain()
    net.nodes["linux-b"].run_ready()
    assert started(net, blocked) == ["linux-b"]
    return dict(claims_skipped=True, dependencies_wait=True,
                capabilities_and_preferences=True, slots_respected=True)


def id_collision():
    net = fleet()
    a, b = net.nodes["linux-a"], net.nodes["linux-b"]
    task = a.create("task-12345678")
    try:
        a.create(task)
        raise AssertionError("known collision accepted")
    except ValueError:
        pass
    # Neither disconnected creator knows the other birth yet.
    net.partition(["linux-a"], ["linux-b", "creator", "scheduler", "build-mac"])
    b.create(task)
    net.heal()
    net.gossip()
    net.assert_converged()
    assert all(d.disk.replica.record(task) is None for d in net.nodes.values())
    assert not a.start(task) and not b.start(task)
    return dict(known_collision_refused=True, unseen_collision_quarantined=True)


def naive_lww_counterexample():
    # Negative control: disconnected replicas can BOTH start before LWW merges.
    replicas = [{"owner": "creator"}, {"owner": "creator"}]
    starts = []
    for replica, runner in zip(replicas, ["linux-a", "linux-b"]):
        replica["owner"] = runner
        starts.append(runner)
    converged_owner = max(r["owner"] for r in replicas)
    assert len(starts) == 2
    return dict(starts=starts, converged_owner=converged_owner,
                conclusion="Convergence cannot undo a double start")


def random_schedule(nodes=8, seed=0, steps=300):
    assert nodes >= 3
    net = Network([Daemon("n%02d" % i, ("linux",), slots=2,
                          offset=(i % 3 - 1) * 700) for i in range(nodes)], seed)
    names = list(net.nodes)
    tasks = []
    for i in range(nodes * 2):
        d = net.nodes[names[i % nodes]]
        tasks.append(d.create(needs=("linux",)))
    net.gossip()
    for step in range(steps):
        name = net.rng.choice(names)
        d = net.nodes[name]
        action = net.rng.randrange(10)
        if action == 0:
            group = net.rng.sample(names, net.rng.randrange(1, nodes))
            net.partition(group, [n for n in names if n not in group])
        elif action == 1:
            net.heal()
        elif action == 2:
            d.online = False
        elif action == 3:
            net.restart(name)
        elif action == 4 and d.online:
            d.ask()
        elif action == 5 and net.queue:
            # A delayed duplicate can arrive after handoff, start or restart.
            net.queue.append(deepcopy(net.rng.choice(net.queue)))
        elif action == 6 and net.queue:
            net.deliver(net.rng.randrange(len(net.queue)))
        elif action == 7:
            d.retry()
        elif action == 8 and d.online:
            d.run_ready()
            running = [t for t, a in d.disk.authority.items() if a.phase == "running"]
            if running:
                d.finish(net.rng.choice(running))
        elif action == 9:
            net.gossip()
        net.now += net.rng.randrange(20)
        net.check()
    # Once faults stop, repeated exchange and retries must drain the backlog.
    net.heal()
    for name in names:
        net.restart(name)
    for _ in range(len(tasks) + 5):
        for d in net.nodes.values():
            d.retry()
        net.drain(shuffled=True)
        net.gossip()
        for d in net.nodes.values():
            d.run_ready()
            for t, a in d.disk.authority.items():
                if a.phase == "running":
                    d.finish(t)
            d.ask()
        net.drain(shuffled=True)
        if all(not d.disk.outbox and
               all(r and r["state"] == "done" for r in d.disk.replica.records().values())
               for d in net.nodes.values()):
            break
    net.gossip()
    net.assert_converged()
    assert all(len(started(net, t)) == 1 for t in tasks)
    assert all(net.nodes[names[0]].disk.replica.record(t)["state"] == "done" for t in tasks)
    return dict(seed=seed, nodes=nodes, fault_steps=steps, tasks=len(tasks),
                starts=len(tasks), converged=True)


SCENARIOS = {"two_runners": race, "reverse_race": lambda: race(True),
             "offline_owner": offline_owner, "lost_grant": lost_grant,
             "scheduler_takeover": scheduler_takeover,
             "partitioned_edits": partitioned_edits, "placement_guards": guards,
             "id_collision": id_collision, "unsafe_lww_control": naive_lww_counterexample}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nodes", type=int, default=8)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--steps", type=int, default=300)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if args.nodes < 3 or args.steps < 0:
        parser.error("--nodes must be >= 3; --steps must be >= 0")
    results = {name: scenario() for name, scenario in SCENARIOS.items()}
    results["fault_schedule"] = random_schedule(args.nodes, args.seed, args.steps)
    if args.json:
        print(json.dumps(results, indent=2, sort_keys=True))
    else:
        for name, result in results.items():
            summary = {k: v for k, v in result.items() if k not in ("trace", "records")}
            print("PASS", name, json.dumps(summary, sort_keys=True))
            for event in result.get("trace", []):
                if event["event"] in ("ask", "grant", "refused", "start", "scheduler", "restart"):
                    print("  " + json.dumps(event, sort_keys=True))


if __name__ == "__main__":
    main()
