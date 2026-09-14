# Android OTA implementation evidence and device handoff

Task c1516501 implements the approved plan in the stage worktree. No relay
was deployed, OTA channel published, or physical device accessed in this stage.
The owner's durable directive reserves those actions for Ship after review.

The opt-in `tools/kd/src/runtime/mobile-ota.integration.test.ts` passed with a
real Android Expo/Hermes export at staging runtime `2.2.4`. Production staging
code produced update `5fc5943b-472c-ea55-dab9-09a6e4c9a0cc`; a real relay process
served its signed manifest and all 37 declared artifacts (launch bundle plus
36 assets). Every response matched the original exported bytes and SHA-256.
This exercised the implementation working tree, not a published release.
Logs and machine-readable evidence are under `.tmp/ota-android-export.log` and
`.tmp/ota-android-integration.json`. The test stops its relay and removes its
scratch storage. It uses an ephemeral test signing key; separate config tests
verify that Expo embeds the committed certificate, channel and key metadata
into Android configuration.

The relay process integration also covers distinct iOS/Android manifests,
assets, signature tampering, signed current-update directives, cache isolation,
missing exact-platform/runtime/channel pointers and unknown-platform rejection.
The kd tests cover selected-platform export/publication/verification, full
upload reconciliation before pointer writes, failures and the real shared
staging-lineage caller with controlled git/GitHub facts.

Not established: deployed relay acceptance, production signing-secret wiring
on that relay, or the Samsung downloading/applying a published update. The
installed phone's current runtime and launch source were not inspected. Its
previous runtime must not be assumed from the implementation checkout.

Ship must follow the [Android acceptance runbook](specs/mobile-ota-updates.md#android-implementation-and-ship-acceptance):
coordinate exclusive access with task 69234f81, identify staging, fence every
adb command to serial `R5CX42N3NLK`, preserve pairing and task 0d72b7af's native
changes, and observe the exact published update applied at the installed native
runtime. Do not rebuild merely to force a runtime match. If lineage, credentials,
or runtime compatibility block that check, report the limit without claiming
on-device delivery. No production publication or desktop release is included.
