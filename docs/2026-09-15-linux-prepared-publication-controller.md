# Retained Linux preparation publication controller

Ship task d3ce8dec resumed after the reported provider usage-limit stop.
The engineering correction is implemented; this run encountered no provider
refusal. The hosting-only attention badge was cleared. Ship remains open.

## Change

Linux ship accepts a retained preparation manifest plus matching explicit
40-hex product source/promotion-base selections and staging iteration. It
verifies both original deb/report streams using the isolated product commit's
VERSION and runtime policy, without Bazel or the deleted preparation cache.
The clean controller identity is reported separately. Candidate provenance
records the immutable commit selection; the selected remote main or matching
Linux series branch must still contain it. Legacy candidates keep exact-tip
behavior. Retries compare the complete base, source, artifacts and acceptance.

Promotion isolates and rebuilds production identity from the stored commit.
Exact staging evidence, full Linux soak, lineage, branch containment, channel
ownership and production-floor checks remain. Staging debs cannot become
production debs, and pinned promotion cannot skip its production build.
No release branch/tag pin is created by preparation.

## Validation

- `pnpm --dir tools/kd exec vitest run tests/linux-release-prepare.test.ts tests/linux-release-commands.test.ts tests/linux-release-lifecycle.test.ts tests/release-tasks.test.ts`: 56 passes.
- `pnpm --dir tools/kd typecheck`: pass.
- Disposable integration uses real Git snapshots, collector/Debian verifier,
  POSIX publication and disposable apt signing. Executables are synthetic;
  GitHub and public readback are intercepted. It covers prepare → retained
  ship after controller/main advance; altered source/tree/iteration/deb/report,
  duplicate architecture, traversal, symlink, dirty-controller and acceptance
  refusals; unresolved ancestry; soak refusal; and a synthetic production
  rebuild of the pinned source.
- Existing lifecycle tests preserve exact-tip, Linux ownership, lineage,
  signature/readback recovery, immutable receipt and 24h behavior.

These are controller tests, not new native builds or public publication.
Product B stays a9df2f1d46fb08bcf53200b738e0fdc3c127b636, VERSION 0.2.0,
staging iteration 2. The four real A/B artifacts and original failed upgrade
plus corrected ARM/floor results are unchanged.

## Remaining delivery work

The focused PR goes to the existing Merge Master policy path. After merge,
use the retained-B rehearsal in [publication next step](2026-09-15-linux-publication-next-step.md)
once approved archive/key configuration exists. No actual configuration,
key, host, DNS, Caddy, release pin, publication, promotion or soak changed.
Authenticated cross-machine acceptance stays parked with 641dbb6f.
Website links stay disabled until verified public artifacts exist.
