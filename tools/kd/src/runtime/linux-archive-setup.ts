/** Narrow setup for the approved staging relay. No VM/IAM, relay image, OTA,
 * production, or general-purpose remote provisioning surface. */
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, existsSync, rmSync, lstatSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { homedir, hostname } from 'node:os';
import { resolve4, resolve6, resolveNs, resolveCname } from 'node:dns/promises';
import { randomBytes } from 'node:crypto';
import * as openpgp from 'openpgp';
import { z } from 'zod';
import { resolveKdEnvironment } from './environment';
import { sha256 } from './linux-release-artifacts';
import { readLinuxKeyFile, linuxReleaseConfig } from './linux-release-config';
import { createAptPublicationSigner } from './linux-apt-signature';
import { linuxAptStorageWorker, pinnedLinuxSshCommand, linuxArchiveStorage } from './linux-apt-storage';
import { writeMachineLinuxSelectors } from './release-env';
import { linuxArchiveSetupHost, linuxArchiveConfigRenderer } from './linux-archive-setup-host';
import type { CommandRunner } from './process';

export const linuxArchiveSetupInputSchema = z.object({
  mode: z.enum(['inspect', 'plan', 'apply']), staging: z.literal(true),
  adminUser: z.string().regex(/^[a-z_][a-z0-9_-]*$/i),
  adminIdentity: z.string().startsWith('/'),
  // Optional pre-existing, independently authenticated host-key evidence.
  // Never filled by ssh-keyscan or a trust-on-first-use connection.
  hostKeyFile: z.string().startsWith('/').optional(),
  plan: z.string().optional(), confirm: z.string().regex(/^[a-f0-9]{64}$/).optional(),
  out: z.string().optional(), proxyMaintenance: z.boolean().default(false),
  disconnectRelay: z.boolean().default(false),
}).strict().refine(v => !v.disconnectRelay || v.proxyMaintenance, { message: '--disconnect-relay requires --proxy-maintenance' });
export type SetupInput = z.infer<typeof linuxArchiveSetupInputSchema>;
interface Context { repoRoot: string; env: NodeJS.ProcessEnv; runner: CommandRunner }
const environment = resolveKdEnvironment('staging');
const target = { project: environment.firebaseProjectId, vm: environment.gceVmName!, zone: 'us-central1-a', instanceId: '6655221359129467471', domain: 'apt.kanna.build', address: '34.133.43.193', archive: '/srv/kanna-apt/archive' };
const quote = (s: string) => "'" + s.replace(/'/g, "'\"'\"'") + "'";
export const dnsSetupAction = { provider: 'existing authoritative DNS account (DomainControl delegation)', name: 'apt.kanna.build.', type: 'A', ttl: 300, values: [target.address], conflicts: 'Refuse other A values, AAAA or CNAME; do not replace unrelated records.' };
async function command(c: Context, executable: string, args: string[], stdin?: string) {
  const r = await c.runner.run(executable, args, { cwd: c.repoRoot, env: c.env, stdin });
  if (r.exitCode) {
    let diagnostic = 'command output suppressed';
    if (executable === 'gcloud') {
      if (/Reauthentication failed|reauthentication is needed|invalid_grant/i.test(r.stderr)) diagnostic = 'Reauthentication failed; complete normal gcloud auth login on this release host (no automatic retry)';
      else if (/Guest Attribute|hostkeys\//i.test(r.stderr) && /404|not found/i.test(r.stderr)) diagnostic = 'Authenticated hostkeys/ guest attribute not found; use independently authenticated existing host-key evidence, without enabling metadata';
      else if (/SERVICE_DISABLED/.test(r.stderr)) diagnostic = 'SERVICE_DISABLED; setup does not enable cloud APIs';
      else if (/PERMISSION_DENIED|permission/i.test(r.stderr)) diagnostic = 'Authenticated account lacks permission for this read-only inventory';
    } else if (executable === '/usr/bin/ssh') {
      if (/Host key verification failed|REMOTE HOST IDENTIFICATION HAS CHANGED/.test(r.stderr)) diagnostic = 'Pinned SSH host key verification failed; no trust-on-first-use or rotation';
      else if (/Permission denied/.test(r.stderr)) diagnostic = 'Existing administrator SSH identity refused; no metadata/key mutation';
      else if (/password is required/.test(r.stderr)) diagnostic = 'Administrator sudo -n requires interaction; setup does not change sudo/auth policy';
      else {
        const safe = r.stderr.match(/RuntimeError: ([A-Za-z0-9 /:;._()-]+)\s*$/);
        if (safe) diagnostic = safe[1];
      }
    }
    throw new Error(`Linux archive setup ${executable} ${args[0]} failed (exit ${r.exitCode}): ${diagnostic}.`);
  }
  return r.stdout.trim();
}
async function cloud(c: Context, args: string[]) { return JSON.parse(await command(c, 'gcloud', [...args, '--project', target.project, '--format=json', '--quiet'])); }
function keyLine(value: string): string {
  const match = /^(ssh-ed25519|ssh-rsa|ecdsa-sha2-nistp256) ([A-Za-z0-9+/=]+)$/.exec(value.trim());
  if (!match) throw new Error('Expected one independently authenticated SSH public host key, with no hostname/comment.');
  return match[0];
}
async function withPin<T>(c: Context, pin: string, use: (path: string) => Promise<T>) {
  mkdirSync(join(c.repoRoot, '.tmp'), { recursive: true });
  const dir = mkdtempSync(join(c.repoRoot, '.tmp/linux-setup-'));
  const path = join(dir, 'known_hosts');
  writeFileSync(path, `${target.address} ${pin}\n`, { mode: 0o600 });
  try { return await use(path); } finally { rmSync(dir, { recursive: true, force: true }); }
}
async function host(c: Context, input: SetupInput, pin: string, payload: unknown) {
  readLinuxKeyFile(input.adminIdentity, true);
  return withPin(c, pin, async path => {
    const [executable, args] = pinnedLinuxSshCommand({ host: target.address, user: input.adminUser, port: 22, identityPath: input.adminIdentity, knownHostsPath: path }, ['sudo', '-n', '/usr/bin/python3', '-c', linuxArchiveSetupHost].map(quote).join(' '));
    return JSON.parse(await command(c, executable, args, JSON.stringify(payload)));
  });
}
async function dns() {
  const optional = async (query: () => Promise<string[]>) => {
    try { return (await query()).sort(); } catch (e) {
      if (['ENODATA', 'ENOTFOUND'].includes((e as NodeJS.ErrnoException).code ?? '')) return [];
      throw e;
    }
  };
  return { ns: (await resolveNs('kanna.build')).sort(), cname: await optional(() => resolveCname(target.domain)), a: await optional(() => resolve4(target.domain)), aaaa: await optional(() => resolve6(target.domain)) };
}
export function setupPlan(snapshot: Record<string, unknown>) {
  const plan = { schemaVersion: 1, kind: 'linux-staging-archive-setup', target, snapshot,
    changes: [...(snapshot.maintenance === 'disconnect-staging-proxy' ? ['briefly stop only staging Caddy; verify socket drain; always restore proxy admission'] : []), 'dedicated MBP apt and publisher keys', 'forced storage-helper-only kanna-apt account', 'dedicated POSIX archive and public apt key', 'Caddy read-only mount/vhost; recreate only caddy, preserve relay ID/image/start', 'merge only KANNA_LINUX_* machine selectors after HTTPS key readback'], dnsAction: dnsSetupAction };
  return { ...plan, sha256: sha256(JSON.stringify(plan)) };
}
async function inspect(c: Context, input: SetupInput) {
  const accounts = JSON.parse(await command(c, 'gcloud', ['auth', 'list', '--filter=status:ACTIVE', '--format=json', '--quiet']));
  if (accounts.length !== 1 || typeof accounts[0].account !== 'string') throw new Error('Select exactly one authenticated gcloud account on this host.');
  const vm = await cloud(c, ['compute', 'instances', 'describe', target.vm, '--zone', target.zone]);
  if (String(vm.id) !== target.instanceId || vm.name !== target.vm || vm.status !== 'RUNNING' || !vm.zone?.endsWith('/' + target.zone) || vm.networkInterfaces?.[0]?.accessConfigs?.[0]?.natIP !== target.address) throw new Error('Authenticated staging VM identity/IP differs from approved setup target.');
  let pin: string;
  if (input.hostKeyFile) pin = keyLine(readLinuxKeyFile(input.hostKeyFile));
  else {
    const attributes = await cloud(c, ['compute', 'instances', 'get-guest-attributes', target.vm, '--zone', target.zone, '--query-path', 'hostkeys/']);
    const keys = attributes.queryValue?.items?.filter((v: { namespace: string; key: string }) => v.namespace === 'hostkeys' && v.key === 'ssh-ed25519');
    if (keys?.length !== 1) throw new Error('No authenticated guest host key. Supply --host-key-file from an independently authenticated console/admin observation; do not enable metadata or use ssh-keyscan.');
    pin = keyLine(`${keys[0].key} ${keys[0].value}`);
  }
  const hostState = await host(c, input, pin, { mode: 'inspect' });
  // Capacity is a minimum gate, not a changing identity in the plan digest.
  if (hostState.freeBytes < 1073741824) throw new Error('Staging VM has less than 1GiB free for archive setup.');
  delete hostState.freeBytes;
  return { account: accounts[0].account, machine: hostname(), vmId: String(vm.id), pin, adminUser: input.adminUser, adminIdentity: input.adminIdentity, hostState, maintenance: input.disconnectRelay ? 'disconnect-staging-proxy' : 'already-idle', dns: await dns() };
}
function ownedDirectory(path: string) {
  if (!existsSync(path)) mkdirSync(path, { mode: 0o700 });
  const st = lstatSync(path);
  if (!st.isDirectory() || st.uid !== process.getuid?.() || (st.mode & 0o077)) throw new Error('Unsafe MBP key custody directory.');
}
async function keys(c: Context, pin: string) {
  const home = homedir();
  ownedDirectory(join(home, '.kanna'));
  const dir = join(home, '.kanna/linux-apt');
  if (!existsSync(dir)) {
    // Exclusive directory reservation; interruption leaves an explicit partial
    // setup to inspect, never a silently regenerated identity.
    mkdirSync(dir, { mode: 0o700 });
    const passphrase = randomBytes(32).toString('base64');
    const key = await openpgp.generateKey({ type: 'rsa', rsaBits: 3072, subkeys: [], format: 'armored', passphrase, userIDs: [{ name: 'Kanna apt archive', email: 'ops@kanna.build' }], config: { v6Keys: false, preferredHashAlgorithm: openpgp.enums.hash.sha512 } });
    for (const [name, bytes] of [['private.asc', key.privateKey], ['public.asc', key.publicKey], ['passphrase', passphrase], ['known_hosts', `${target.address} ${pin}\n`]]) writeFileSync(join(dir, name), bytes, { flag: 'wx', mode: 0o600, flush: true });
    await command(c, '/usr/bin/ssh-keygen', ['-t', 'ed25519', '-N', '', '-C', 'kanna-apt-publisher', '-f', join(dir, 'publisher_identity')]);
    writeFileSync(join(dir, 'complete.json'), JSON.stringify({ schemaVersion: 1, pin }), { flag: 'wx', mode: 0o600 });
  }
  ownedDirectory(dir);
  if (!existsSync(join(dir, 'complete.json'))) throw new Error('Partial/existing Linux key directory needs custody assessment; setup will not regenerate or overwrite keys.');
  if (readLinuxKeyFile(join(dir, 'known_hosts'), true) !== `${target.address} ${pin}\n`) throw new Error('Existing host pin differs; no implicit rotation.');
  const publicKey = readLinuxKeyFile(join(dir, 'public.asc'), true);
  const fingerprint = (await openpgp.readKey({ armoredKey: publicKey })).getFingerprint();
  await createAptPublicationSigner({ publicKey, fingerprint, privateKey: readLinuxKeyFile(join(dir, 'private.asc'), true), passphrase: readLinuxKeyFile(join(dir, 'passphrase'), true), now: () => new Date() });
  readLinuxKeyFile(join(dir, 'publisher_identity'), true);
  return { dir, publicKey, fingerprint, publisherPublicKey: readFileSync(join(dir, 'publisher_identity.pub'), 'utf8').trim() };
}
export async function setupLinuxArchive(c: Context, raw: SetupInput) {
  const input = linuxArchiveSetupInputSchema.parse(raw);
  const snapshot = await inspect(c, input);
  const plan = setupPlan(snapshot);
  if (input.mode !== 'apply') {
    if (input.out) writeFileSync(resolve(c.repoRoot, input.out), JSON.stringify(plan, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
    return plan;
  }
  if (!input.plan || !input.confirm) throw new Error('Apply requires --plan and its exact --confirm SHA256.');
  const expected = JSON.parse(readFileSync(resolve(c.repoRoot, input.plan), 'utf8'));
  if (input.confirm !== plan.sha256 || JSON.stringify(expected) !== JSON.stringify(plan)) throw new Error('Setup plan/host/account/DNS changed; inspect and plan again.');
  const hardware = JSON.parse(await command(c, '/usr/sbin/system_profiler', ['SPHardwareDataType', '-json']));
  if (hardware.SPHardwareDataType?.[0]?.machine_name !== 'MacBook Pro') throw new Error('Actual archive setup/signing custody requires the trusted MacBook Pro.');
  const answers = snapshot.dns;
  if (answers.a.length !== 1 || answers.a[0] !== target.address || answers.aaaa.length || answers.cname.length) throw new Error(`DNS action needed in the existing authoritative account: A apt.kanna.build = ${target.address}, TTL 300; no conflicting A/AAAA. No Cloud DNS API or zone will be enabled.`);
  if (snapshot.hostState.proxyChange && (!input.proxyMaintenance || (!input.disconnectRelay && (snapshot.hostState.relayTraffic?.openSockets !== 0 || snapshot.hostState.relayTraffic?.liveRows !== 0 || snapshot.hostState.relayTraffic?.pairedUsers !== 0)))) throw new Error('Apply requires a coordinated --proxy-maintenance window with zero live relay connections; no application disconnects or drain are performed.');
  const custody = await keys(c, snapshot.pin);
  const applied = await host(c, input, snapshot.pin, { mode: 'apply', expected: snapshot.hostState, publisherPublicKey: custody.publisherPublicKey, aptPublicKey: custody.publicKey, helper: linuxAptStorageWorker, renderer: linuxArchiveConfigRenderer, proxyMaintenance: input.proxyMaintenance, disconnectRelay: input.disconnectRelay });
  const relayHealthResponse = await fetch('https://relay-staging.kanna.build/health', { redirect: 'error', signal: AbortSignal.timeout(30000) });
  const relayHealth = relayHealthResponse.ok ? await relayHealthResponse.json() as {status?: string; commit?: string} : null;
  if (relayHealth?.status !== 'ok' || relayHealth.commit !== snapshot.hostState.relayTraffic.commit) throw new Error('Staging relay HTTPS health/source readback failed after setup; inspect retained state.');
  const publicKeyUrl = `https://${target.domain}/keys/kanna-archive.asc`;
  const response = await fetch(publicKeyUrl, { redirect: 'error', signal: AbortSignal.timeout(30000) });
  if (!response.ok || sha256(await response.text()) !== sha256(custody.publicKey)) throw new Error('Public HTTPS apt key readback failed; retain setup and retry from a fresh plan. No release selectors installed.');
  const assignments: [string, string][] = Object.entries({
    KANNA_LINUX_ARCHIVE_BACKEND: 'ssh', KANNA_LINUX_ARCHIVE_ROOT: target.archive, KANNA_LINUX_ARCHIVE_BASE_URL: `https://${target.domain}`, KANNA_LINUX_ARCHIVE_VALID_HOURS: '168',
    KANNA_LINUX_SSH_HOST: target.address, KANNA_LINUX_SSH_USER: 'kanna-apt', KANNA_LINUX_SSH_PORT: '22', KANNA_LINUX_SSH_KNOWN_HOSTS_PATH: join(custody.dir, 'known_hosts'), KANNA_LINUX_SSH_IDENTITY_PATH: join(custody.dir, 'publisher_identity'),
    KANNA_LINUX_APT_PUBLIC_KEY_PATH: join(custody.dir, 'public.asc'), KANNA_LINUX_APT_FINGERPRINT: custody.fingerprint, KANNA_LINUX_APT_PRIVATE_KEY_PATH: join(custody.dir, 'private.asc'), KANNA_LINUX_APT_PASSPHRASE_PATH: join(custody.dir, 'passphrase'),
  });
  const config = linuxReleaseConfig(Object.fromEntries(assignments));
  const storage = linuxArchiveStorage(config);
  await storage.withExclusivePublication(async () => { await storage.read('linux/state.json'); });
  const configPath = writeMachineLinuxSelectors(homedir(), assignments);
  return { configured: true, published: false, target, publicKeyUrl, fingerprint: custody.fingerprint, configPath, relayHealth: { status: relayHealth.status, commit: relayHealth.commit }, applied };
}
