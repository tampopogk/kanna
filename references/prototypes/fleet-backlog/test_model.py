import unittest
from copy import deepcopy
import json
from pathlib import Path

from model import Clock, Daemon, Network, Stamp
from scenarios import SCENARIOS, fleet, random_schedule, started


class Acceptance(unittest.TestCase):
    def test_captured_replay(self):
        expected = json.loads(Path(__file__).with_name("expected.json").read_text())
        actual = {name: scenario() for name, scenario in SCENARIOS.items()}
        actual["fault_schedule"] = random_schedule()
        self.assertEqual(json.loads(json.dumps(actual)), expected)

    def test_scenarios(self):
        for name, scenario in SCENARIOS.items():
            with self.subTest(scenario=name):
                scenario()

    def test_hlc_receive_ties_and_rollback(self):
        a, b = Clock("a"), Clock("b")
        first = a.tick(1000)
        self.assertLess(first, b.tick(1, first))
        self.assertLess(first, a.tick(-100))
        self.assertEqual(Stamp(1000, 2, "a"), a.tick(0, Stamp(1000, 1, "b")))
        self.assertEqual(Stamp(2000, 0, "a"), a.tick(2000))
        # Concurrent equal physical/logical timestamps use stable device ID.
        self.assertLess(Stamp(1, 0, "a"), Stamp(1, 0, "b"))

    def test_local_start_races_remote_ask(self):
        for local_first in (False, True):
            net = Network([Daemon("a", ("linux",)), Daemon("b", ("linux",))])
            task = net.nodes["a"].create(needs=("linux",))
            net.gossip()
            net.nodes["b"].ask()
            if local_first:
                net.nodes["a"].start(task)
            net.drain()
            for d in net.nodes.values():
                d.run_ready()
            self.assertEqual(started(net, task), ["a" if local_first else "b"])

    def test_display_owner_is_not_authority(self):
        net = fleet()
        task = net.nodes["creator"].create(needs=("linux",))
        net.gossip()
        net.nodes["linux-a"].ask()
        net.deliver()  # grant exists, but hasn't reached receiver
        grant = net.queue.pop()
        net.gossip()  # owner display arrives first
        runner = net.nodes["linux-a"]
        self.assertEqual(runner.disk.replica.record(task)["owner"], "linux-a")
        self.assertFalse(runner.start(task))
        net.queue.append(grant)
        net.drain()
        self.assertTrue(runner.start(task))

    def test_returning_ownership_rejects_old_grants(self):
        net = fleet()
        a, b = net.nodes["linux-a"], net.nodes["linux-b"]
        task = a.create(needs=("linux",))
        net.gossip()
        self.assertTrue(a.grant(task, b.name, b.advert()))
        old = deepcopy(net.queue[0])
        net.drain()
        self.assertTrue(b.grant(task, a.name, a.advert()))
        net.drain()
        net.queue.append(old)  # epoch 1 at b, after it relinquished epoch 1
        net.drain()
        self.assertFalse(b.start(task))
        self.assertTrue(a.start(task))
        net.gossip()
        net.assert_converged()

    def test_two_owners_offer_one_slot(self):
        net = fleet()
        a, b, r = [net.nodes[n] for n in ("creator", "scheduler", "linux-a")]
        first = a.create(needs=("linux",))
        second = b.create(needs=("linux",))
        # Two owners see the same free-slot advertisement.
        self.assertTrue(a.grant(first, r.name, r.advert()))
        self.assertTrue(b.grant(second, r.name, r.advert()))
        net.drain()
        r.run_ready()
        self.assertEqual(r.free(), 0)
        self.assertEqual(len(r.disk.starts), 1)
        running = next(t for t, auth in r.disk.authority.items() if auth.phase == "running")
        r.finish(running)
        r.run_ready()
        self.assertEqual(len(r.disk.starts), 2)

    def test_owner_rechecks_stale_ask(self):
        for field, value in (("claimed_by", "mbp"), ("needs", ("xcode",)),
                             ("preferred", ("build-mac",)), ("after", ("missing",))):
            with self.subTest(field=field):
                net = fleet()
                o = net.nodes["creator"]
                task = o.create(needs=("linux",))
                net.gossip()
                net.nodes["linux-a"].ask()
                o.edit(task, **{field: value})
                net.drain()
                self.assertEqual(o.disk.authority[task].phase, "ready")

    def test_clock_persisted_across_restart(self):
        net = fleet()
        a = net.nodes["linux-a"]
        task = a.create()
        before = a.stamp()
        a = net.restart(a.name)
        a.offset = -10_000
        self.assertGreater(a.stamp(), before)
        a.edit(task, name="after restart")

    def test_received_grant_survives_source_going_away(self):
        net = fleet()
        o, r = net.nodes["creator"], net.nodes["linux-a"]
        task = o.create(needs=("linux",))
        net.gossip()
        r.ask()
        net.deliver()  # ask
        net.deliver()  # grant
        o.online = False
        net.drain()  # ACK lost
        self.assertTrue(r.start(task))
        o = net.restart("creator")
        o.retry()
        net.drain()
        r.run_ready()
        self.assertEqual(started(net, task), [r.name])

    def test_local_creation_does_not_skip_older_remote_work(self):
        net = fleet()
        old = net.nodes["creator"].create(needs=("linux",))
        net.gossip()
        r = net.nodes["linux-a"]
        new = r.create(needs=("linux",))
        r.run_ready()
        self.assertEqual(started(net, new), [])
        self.assertEqual(r.ask(), old)
        net.drain()
        r.run_ready()
        self.assertEqual(started(net, old), [r.name])

    def test_adversarial_schedules(self):
        for nodes in (3, 8, 16):
            for seed in range(20):
                with self.subTest(nodes=nodes, seed=seed):
                    random_schedule(nodes=nodes, seed=seed, steps=200)


if __name__ == "__main__":
    unittest.main()
