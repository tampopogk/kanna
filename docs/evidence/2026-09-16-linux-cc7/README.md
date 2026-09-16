# Prepared Linux staging.3 candidate (not published)

Canonical preparation completed 2026-09-16T04:58:44.656Z, exit 0:

```
./kd release prepare --platform linux --ref cc7bb93dcf8e8481e8e52b4d2941467d21e4f8a2 --staging-iteration 3 --out-dir .tmp/linux-C-cc7/prepared
```

- Product: cc7bb93dcf8e8481e8e52b4d2941467d21e4f8a2, tree 7c4abb7e7ab1bb87c008f144e1e82c0d790ba6b9.
- Committed VERSION 0.2.0; packages 0.2.0~staging.3-1, both architectures.
- Collection controller: 25dde2c2a8870a8c74fece0acf0527417e33124b.
- `A/`: genuine predecessor, previously published B staging.2 (source a9df2f1d).
- `B/`: newly prepared cc7 staging.3. Both actual deb/report hashes rechecked.
- Tar contains only these four debs: SHA256 `1f097380bebc6ba4d80fc3c97f71556a1fba8f8525b969f2510637084dd181c3`, 176896000 bytes.
- Retained tar: Ship `.tmp/linux-C-cc7/prepared-pair.tar`; new packages/manifest: `.tmp/linux-C-cc7/prepared/`.

These are build/audit/provenance results, **not installed, upgrade or system acceptance**. Do not copy earlier passing checks onto these hashes. The targeted merged-dev continuation PASS at cc7 establishes only its documented scope; it does not certify these packages.

Canonical MBP Linux status at 04:58:06.806Z (helper074, controllercc7) exited0 in20.22s: active staging.2, productionnull, pendingnull. Previous20s wrapper timeout was premature. B's original system failure and 13.4635/24h soak remain separate. Productcc7 descends from B and includes merged1530 and1523; main81ca48db contains cc7. No release branch/reset/abandonment is required for the proposed pinned main staging.3 path, subject fresh canonical gates.

Next: fd962919's normal controller workflow enables manifest-selected pair validation; run real Ubuntu24.04 installed/upgrade checks on these bytes, produce exact acceptance hashes, transfer retained inputs to trusted MBP074, then canonical prepared-manifest dry-run/release with source-ref and promotion-base both cc7. Existing owner staging authorization persists. New 24h soak begins only after actual verified publication. No production action, package relabeling, duplicate live/dev test or owner VM modification is authorized by this report.
