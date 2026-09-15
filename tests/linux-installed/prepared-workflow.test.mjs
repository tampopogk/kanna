import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';

const path = fileURLToPath(new URL('../../.github/workflows/linux-release-check.yml', import.meta.url));
const workflow = parse(readFileSync(path, 'utf8'));
const fetch = workflow.jobs['prepared-fetch'];
const upgrade = workflow.jobs['prepared-upgrade'];
const manualOnly = "${{ github.event_name == 'workflow_dispatch' && inputs.prepared_pair_asset_id != '' }}";
assert.deepEqual(workflow.permissions, { contents: 'read' });
assert.equal(fetch.if, manualOnly);
assert.deepEqual(fetch.permissions, { contents: 'write' });
assert.deepEqual(fetch.steps.map(step => step.uses ?? 'run'), [
  'actions/checkout@v4', 'run', 'actions/upload-artifact@v4',
]);
assert.equal(fetch.steps[0].with['persist-credentials'], false);
const command = fetch.steps[1];
assert.equal(command.env.GH_TOKEN, '${{ github.token }}');
assert.match(command.run, /python3 tests\/linux-installed\/fetch-prepared-pair.py/);
assert.match(command.run, /sha256sum --check --strict/);
assert.match(command.run, /python3 tests\/linux-installed\/verify-prepared-pair.py/);
assert.doesNotMatch(command.run, /\b(sudo|apt|pnpm|npm|pip|git|curl|gh|kd)\b/);
assert.equal(fetch.steps[2].with.path, '.tmp/prepared-upgrade/pair.tar');
assert.equal(upgrade.if, manualOnly);
assert.equal(upgrade.needs, 'prepared-fetch');
assert.deepEqual(upgrade.permissions, { contents: 'read' });
assert.equal(upgrade.steps[0].with['persist-credentials'], false);
assert.doesNotMatch(JSON.stringify(upgrade), /GH_TOKEN|github\.token|fetch-prepared-pair/);
const download = upgrade.steps.find(step => step.uses === 'actions/download-artifact@v4');
assert.equal(download.with.name, fetch.steps[2].with.name);
const checks = upgrade.steps.findIndex(step => step.name === 'Recheck exact tar and four manifests before installation');
assert.match(upgrade.steps[checks].run, /sha256sum --check --strict/);
assert.match(upgrade.steps[checks].run, /verify-prepared-pair.py/);
assert(checks < upgrade.steps.findIndex(step => step.run?.includes('sudo apt-get')));
assert.deepEqual(upgrade.strategy.matrix.include, [
  { architecture: 'x86_64', runner: 'ubuntu-24.04' },
  { architecture: 'arm64', runner: 'ubuntu-24.04-arm' },
]);
for (const [name, job] of Object.entries(workflow.jobs)) {
  if (name !== 'prepared-fetch') assert.notEqual(job.permissions?.contents, 'write');
}
console.log('Prepared workflow privilege, manual-dispatch and artifact boundary checks passed');
