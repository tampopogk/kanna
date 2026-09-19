# Prepared Linux staging.4 candidate (not published)

Canonical preparation completed 2026-09-19T12:22:40.150Z, exit 0, on Jeremy's
Mac Studio (`desktop-aa43ab36-e634-4ae9-b629-e8c8a91f7bff`):

```
./kd release prepare --platform linux --ref 40d25b76cf05433ec81f23b7ebc7f3f04214bf55 --staging-iteration 4
```

- Product: 40d25b76cf05433ec81f23b7ebc7f3f04214bf55, tree e0aa6bd4a0d740a7c63a0b8a094c1de9eb47f14f.
- Committed VERSION 0.2.0; packages 0.2.0~staging.4-1, both architectures, 0 audit findings each.
- `A/`: genuine predecessor, the currently published staging.3 (source cc7bb93d).
  Its manifest and reports are copied verbatim from `docs/evidence/2026-09-16-linux-cc7/B/`.
  Its deb bytes were re-downloaded from `https://apt.kanna.build` and their actual
  SHA-256 rechecked against that committed manifest.
- `B/`: newly prepared staging.4. Actual deb/report hashes rechecked on disk and
  re-verified through ship's own `readLinuxPrepared` canonical verification
  (real ar/control/data inspection, all eight executable hashes, and the product
  commit's runtime policy).
- Pair tar contains only these four debs, no directory prefix.

These are build/audit/provenance results, **not installed, upgrade or system
acceptance**. Do not copy earlier passing checks onto these hashes.

## Why this directory exists

The `prepared_pair_asset_id` path is the only implemented transport that puts
locally prepared release bytes on a real Ubuntu 24.04 host. The CI build lane
cannot stand in for it, for three independent reasons measured at this commit:

1. `kd build linux-package` defaults `stagingIteration` to 1
   (`tools/kd/src/tasks/registry.ts`), and `debianVersion()` bakes the iteration
   into the Debian `Version`, so a CI package is always `~staging.1-1`.
2. CI reports carry empty `buildRevision`/`buildTree`, which `verifyLinuxArtifact`
   rejects outright — a CI package can never be a release artifact at any iteration.
3. Cross-host reproducibility does not hold and is not claimed
   (`docs/dev/linux-release.md`): against run 35442188329 at this same commit,
   0/8 arm64 and 4/8 x86_64 installed executables were byte-identical.

The arm64 UTM VM was unavailable as an acceptance host: it is running the owner's
live installed Kanna Staging desktop. It was not modified.
