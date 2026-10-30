import itertools
import unittest

from model import Device, Refused


def fleet():
    mbp, box, vm, stranger, laptop = (Device(n) for n in ("mbp", "linux-box", "cloud-vm", "stranger", "laptop"))
    mbp.bootstrap()
    mbp.add(box)
    mbp.add(vm, hosted=True)
    box.join(mbp.log)
    vm.join(mbp.log)
    return mbp, box, vm, stranger, laptop


class HostedKey(unittest.TestCase):
    def setUp(self):
        self.mbp, self.box, self.vm, self.stranger, self.laptop = fleet()

    def refused(self, reason, log, entries):
        with self.assertRaises(Refused) as ctx:
            log.merge(entries)
        self.assertEqual(str(ctx.exception), reason)

    def test_hosted_peer_is_trusted_as_itself(self):
        view = self.mbp.log.view()
        self.assertEqual(view.admits(self.vm.key), self.vm.key)
        self.assertIn(self.vm.key, view.hosted)

    def test_hosted_add_refused(self):
        forged = self.vm.entry("add", self.stranger.descriptor())
        for log in (self.mbp.log, self.box.log, self.vm.log):
            self.refused("hosted_signer", log, [forged])

    def test_hosted_add_never_counts_even_if_stored(self):
        forged = self.vm.entry("add", self.stranger.descriptor())
        self.mbp.log.force([forged])
        with self.assertRaises(Refused) as ctx:
            self.mbp.log.view().admits(self.stranger.key)
        self.assertEqual(str(ctx.exception), "unpaired")

    def test_hosted_add_chain_never_counts(self):
        first = self.vm.entry("add", self.stranger.descriptor())
        self.mbp.log.force([first])
        self.stranger.join(self.mbp.log)
        second = self.stranger.entry("add", self.laptop.descriptor())
        self.refused("untrusted_signer", self.mbp.log, [second])
        self.mbp.log.force([second])
        self.assertNotIn(self.laptop.key, self.mbp.log.view().trusted)

    def test_hosted_revoke_of_another_refused(self):
        for target in (self.mbp.key, self.box.key):
            forged = self.vm.entry("revoke", dict(key=target))
            self.refused("hosted_signer", self.mbp.log, [forged])
            self.mbp.log.force([forged])
            self.assertIn(target, self.mbp.log.view().trusted)

    def test_hosted_self_revoke_counts(self):
        own = self.vm.revoke(self.vm.key)
        self.mbp.log.merge([own])
        self.box.log.merge([own])
        for log in (self.mbp.log, self.box.log, self.vm.log):
            with self.assertRaises(Refused) as ctx:
                log.view().admits(self.vm.key)
            self.assertEqual(str(ctx.exception), "revoked")

    def test_hosted_cannot_shed_hosted(self):
        self.refused("hosted_signer", self.mbp.log, [self.vm.entry("add", self.vm.descriptor(hosted=False))])
        self.refused("hosted_downgrade", self.mbp.log, [self.box.entry("add", self.vm.descriptor(hosted=False))])
        self.mbp.log.force([self.box.entry("add", self.vm.descriptor(hosted=False))])
        self.assertIn(self.vm.key, self.mbp.log.view().hosted)
        self.mbp.log.force([self.vm.entry("add", self.stranger.descriptor())])
        self.assertNotIn(self.stranger.key, self.mbp.log.view().trusted)

    def test_hosted_new_name_or_rename_does_not_escape(self):
        renamed = dict(self.vm.descriptor(hosted=False), name="mbp-2")
        self.refused("hosted_downgrade", self.mbp.log, [self.mbp.entry("add", renamed)])

    def test_user_device_still_admits_and_revokes(self):
        self.box.log.merge(self.mbp.log.export())
        self.box.add(self.laptop)
        self.mbp.log.merge(self.box.log.export())
        self.assertIn(self.laptop.key, self.mbp.log.view().trusted)
        self.mbp.revoke(self.vm.key)
        self.assertIn(self.vm.key, self.mbp.log.view().revoked)

    def test_revoked_user_device_stays_revoked_despite_hosted(self):
        self.mbp.revoke(self.box.key)
        readd = self.vm.entry("add", self.box.descriptor())
        self.refused("hosted_signer", self.mbp.log, [readd])
        self.mbp.log.force([readd])
        self.assertIn(self.box.key, self.mbp.log.view().revoked)

    def test_orders_converge(self):
        base = self.mbp.log.export()
        valid = [self.mbp.entry("add", self.laptop.descriptor()), self.vm.entry("revoke", dict(key=self.vm.key))]
        forged = [self.vm.entry("add", self.stranger.descriptor()), self.vm.entry("revoke", dict(key=self.mbp.key)),
                  self.vm.entry("revoke", dict(key=self.box.key))]
        outcomes = set()
        for order in itertools.permutations(valid + forged):
            replica = Device("replica")
            replica.join(self.box.log)
            replica.log.events = {}
            replica.log.merge(base)
            for entry in order:
                try:
                    replica.log.merge([entry])
                except Refused:
                    replica.log.force([entry])
            view = replica.log.view()
            outcomes.add((frozenset(view.trusted), frozenset(view.revoked)))
        self.assertEqual(len(outcomes), 1)
        trusted, revoked = outcomes.pop()
        self.assertEqual(trusted, {self.mbp.key, self.box.key, self.laptop.key})
        self.assertEqual(revoked, {self.vm.key})


if __name__ == "__main__":
    unittest.main()
