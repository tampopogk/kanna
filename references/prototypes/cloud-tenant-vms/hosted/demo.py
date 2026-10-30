import sys

from model import Device, Refused


def outcome(fn):
    try:
        fn()
        return "counted"
    except Refused as e:
        return str(e)


def main():
    mbp, box, vm, stranger = (Device(n) for n in ("mbp", "linux-box", "cloud-vm", "stranger"))
    mbp.bootstrap()
    mbp.add(box)
    mbp.add(vm, hosted=True)
    vm.join(mbp.log)
    box.join(mbp.log)
    log = mbp.log
    rows = [
        ("hosted_adds_stranger", outcome(lambda: log.merge([vm.entry("add", stranger.descriptor())])), "hosted_signer"),
        ("hosted_revokes_mbp", outcome(lambda: log.merge([vm.entry("revoke", dict(key=mbp.key))])), "hosted_signer"),
        ("hosted_readds_self_unhosted", outcome(lambda: log.merge([vm.entry("add", vm.descriptor(hosted=False))])), "hosted_signer"),
        ("user_downgrades_hosted", outcome(lambda: log.merge([box.entry("add", vm.descriptor(hosted=False))])), "hosted_downgrade"),
    ]
    log.force([vm.entry("add", stranger.descriptor()), vm.entry("revoke", dict(key=mbp.key))])
    rows.append(("stored_forgeries_ignored", outcome(lambda: log.view().admits(stranger.key)), "unpaired"))
    rows.append(("mbp_still_trusted", outcome(lambda: log.view().admits(mbp.key)), "counted"))
    rows.append(("hosted_revokes_self", outcome(lambda: log.merge([vm.entry("revoke", dict(key=vm.key))])), "counted"))
    rows.append(("hosted_now_refused", outcome(lambda: log.view().admits(vm.key)), "revoked"))
    ok = True
    for name, got, want in rows:
        ok &= got == want
        print(f"{'PASS' if got == want else 'FAIL'} {name}: {got}")
    sys.exit(0 if ok else 1)


main()
