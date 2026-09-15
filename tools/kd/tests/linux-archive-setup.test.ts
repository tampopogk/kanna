import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { afterAll, afterEach, expect, it, vi } from 'vitest';
const custody = vi.hoisted(() => ({ home: '' }));
vi.mock('node:os', async original => ({ ...await original<typeof import('node:os')>(), homedir: () => custody.home }));
vi.mock('node:dns/promises', () => ({ resolveNs: vi.fn(async () => ['ns51.domaincontrol.com','ns52.domaincontrol.com']), resolve4: vi.fn(async () => ['34.133.43.193']), resolve6: vi.fn(async () => []), resolveCname: vi.fn(async () => []) }));
vi.mock('../src/runtime/linux-apt-storage', async original => ({
 ...await original<typeof import('../src/runtime/linux-apt-storage')>(),
 linuxArchiveStorage: vi.fn(() => ({ withExclusivePublication: async (fn: () => Promise<void>) => fn(), read: async () => null })),
}));
import { setupLinuxArchive, setupPlan, linuxArchiveSetupInputSchema } from '../src/runtime/linux-archive-setup';
import { linuxArchiveSetupHost, linuxArchiveConfigRenderer, linuxRelaySocketProbe } from '../src/runtime/linux-archive-setup-host';
import { linuxAptStorageWorker } from '../src/runtime/linux-apt-storage';
import { parseCliArgs } from '../src/cli';
import type { CommandRunner } from '../src/runtime/process';
const repo=resolve(import.meta.dirname,'../../..');
mkdirSync(join(repo,'.tmp'),{recursive:true});
const root=mkdtempSync(join(repo,'.tmp/linux-setup-tests-'));
afterAll(()=>rmSync(root,{recursive:true,force:true}));
afterEach(()=>{vi.restoreAllMocks();vi.unstubAllGlobals();});
function fixture() {
 const directory=mkdtempSync(join(root,'fixture-'));
 const identity=join(directory,'admin');writeFileSync(identity,'TEST ONLY',{mode:0o600});
 const pin=join(directory,'host.pub');writeFileSync(pin,'ssh-ed25519 AAAATESTONLY');
 const calls:{command:string;args:string[];stdin?:string}[]=[];
 let address='34.133.43.193'; let changed=false; let authFailure=false;
 const runner:CommandRunner={run:async(command,args,options)=>{
  calls.push({command,args,stdin:options?.stdin});
  if(command==='gcloud') {
   if(authFailure)return {exitCode:1,stdout:'',stderr:'SECRET must not escape'};
   if(args[0]==='auth')return {exitCode:0,stdout:JSON.stringify([{account:'fixture@example.invalid'}]),stderr:''};
   if(args.includes('get-guest-attributes'))return {exitCode:0,stdout:JSON.stringify({queryValue:{items:[{namespace:'hostkeys',key:'ssh-ed25519',value:'AAAATESTONLY'}]}}),stderr:''};
   expect(args).toContain('describe');
   return {exitCode:0,stdout:JSON.stringify({id:'6655221359129467471',name:'kanna-relay-staging',status:'RUNNING',zone:'zones/us-central1-a',networkInterfaces:[{accessConfigs:[{natIP:address}]}]}),stderr:''};
  }
  if(command==='/usr/bin/ssh'){
   expect(args).toContain('StrictHostKeyChecking=yes');expect(args).toContain('IdentityAgent=none');
   expect(JSON.parse(options!.stdin!).mode).toBe('inspect');
   return {exitCode:0,stdout:JSON.stringify({files:{compose:changed?'changed':'same'},relay:{id:'relay123'},caddy:{id:'caddy123'},managed:false,accountUid:null,proxyChange:true,relayTraffic:{commit:"abcdef123",pairedUsers:0,openSockets:0,liveRows:0},freeBytes:2**31}),stderr:''};
  }
  throw Error('Unexpected mutation/tool '+command);
 }};
 return {context:{repoRoot:directory,env:{},runner},input:{mode:'plan' as const,staging:true as const,proxyMaintenance:false,adminUser:'jeremy',adminIdentity:identity,hostKeyFile:pin},calls,move:()=>{changed=true;},wrong:()=>{address='34.133.233.111';},fail:()=>{authFailure=true;}};
}
it('plans authenticated fixed staging scope without any remote mutation or key generation',async()=>{
 const f=fixture(); const plan=await setupLinuxArchive(f.context,f.input);if (!("sha256" in plan)) throw Error("Expected plan");
 expect(plan).toMatchObject({target:{project:'kanna-staging',vm:'kanna-relay-staging',domain:'apt.kanna.build'},dnsAction:{type:'A',values:['34.133.43.193']}});
 expect(f.calls.filter(c=>c.command==='gcloud').every(c=>!c.args.some(a=>['create','update','enable','ssh','scp'].includes(a)))).toBe(true);
 expect(f.calls.some(c=>c.command.includes('keygen'))).toBe(false);
 expect(setupPlan({a:1}).sha256).toBe(setupPlan({a:1}).sha256);
 expect(setupPlan({a:2}).sha256).not.toBe(setupPlan({a:1}).sha256);
});
it('uses authenticated public guest hostkeys, never ssh-keyscan/metadata writes',async()=>{
 const f=fixture(); const plan=await setupLinuxArchive(f.context,{...f.input,hostKeyFile:undefined});
 expect(plan).toHaveProperty('snapshot.pin','ssh-ed25519 AAAATESTONLY');
 expect(f.calls.some(c=>c.args.includes('get-guest-attributes'))).toBe(true);
});
it('refuses wrong target and credential failures before SSH and does not echo errors/secrets',async()=>{
 const f=fixture();f.wrong();await expect(setupLinuxArchive(f.context,f.input)).rejects.toThrow(/identity/);
 expect(f.calls.some(c=>c.command.includes('ssh'))).toBe(false);
 f.fail();await expect(setupLinuxArchive(f.context,f.input)).rejects.toThrow(/output suppressed/);
});
it('refuses changed plan before any apply/key action',async()=>{
 const f=fixture();const plan=await setupLinuxArchive(f.context,f.input);if (!("sha256" in plan)) throw Error("Expected plan");
 const path=join(f.context.repoRoot,'plan.json');writeFileSync(path,JSON.stringify(plan));f.move();
 await expect(setupLinuxArchive(f.context,{...f.input,mode:'apply',plan:path,confirm:plan.sha256,proxyMaintenance:true})).rejects.toThrow(/changed/);
 expect(f.calls.filter(c=>c.command==='/usr/bin/ssh').every(c=>JSON.parse(c.stdin!).mode==='inspect')).toBe(true);
});
it('rejects production, duplicate/unrecognized selectors and malformed apply input',()=>{
 expect(()=>linuxArchiveSetupInputSchema.parse({mode:'apply',staging:false,production:true})).toThrow();
 expect(()=>parseCliArgs(['release','setup-linux','--production'])).toThrow();
 expect(()=>parseCliArgs(['release','setup-linux','--mode','plan','--mode','apply'])).toThrow();
 expect(parseCliArgs(['release','setup-linux','--staging','--mode','inspect','--admin-user','jeremy','--admin-identity','/owned/key']).taskId).toBe('release.setup-linux');
});
it('executes host rendering in a disposable Python fixture: scoped/idempotent and refuses unrelated vhost/layout',()=>{
 const prefix=linuxArchiveSetupHost.split('request=json.load(sys.stdin)')[0];
 const program=prefix+`\ncompose=${JSON.stringify(readFileSync(join(repo,'services/relay/deploy/docker-compose.yml'),'utf8'))}\ncaddy=${JSON.stringify(readFileSync(join(repo,'services/relay/deploy/Caddyfile'),'utf8'))}\na,b=render(compose,caddy)\nassert render(a,b)==(a,b)\nassert a.split('  caddy:')[0]==compose.split('  caddy:')[0]\nassert b.startswith(caddy)\nassert a.count('/srv/kanna-apt/archive:/srv/kanna-apt:ro')==1\nfor x,y in [(compose,caddy+'\\napt.kanna.build {}'),(compose.replace('./Caddyfile','./unknown'),caddy)]:\n try: render(x,y)\n except RuntimeError: pass\n else: raise AssertionError('unsafe render accepted')\nprint('scope PASS')\n`;
 const result=execFileSync('/usr/bin/python3',['-c',program],{encoding:'utf8',cwd:root});expect(result).toContain('scope PASS');
});
it('pins forced helper-only access and never restarts/rebuilds relay in apply',()=>{
 expect(linuxArchiveSetupHost).toContain('restrict,command=');
 expect(linuxArchiveSetupHost).toContain("'--no-deps','--no-build','--pull','never','--force-recreate','caddy'");
 expect(linuxArchiveSetupHost).not.toMatch(/compose','(?:pull|build)'/);
 expect(linuxArchiveSetupHost).toContain("container('relay')!=observed['relay']");
 expect(linuxArchiveSetupHost).toContain("'Caddyfile','.env'");
 expect(linuxAptStorageWorker).toContain('O_NOFOLLOW');
});
it('executes scoped apply/retry in a disposable host filesystem without running system commands',()=>{
 const dir=mkdtempSync(join(root,'host-'));
 mkdirSync(join(dir,'opt/kanna-relay'),{recursive:true});mkdirSync(join(dir,'srv'));mkdirSync(join(dir,'var/lib'),{recursive:true});
 writeFileSync(join(dir,'opt/kanna-relay/docker-compose.yml'),readFileSync(join(repo,'services/relay/deploy/docker-compose.yml')));
 writeFileSync(join(dir,'opt/kanna-relay/Caddyfile'),readFileSync(join(repo,'services/relay/deploy/Caddyfile')));
 writeFileSync(join(dir,'opt/kanna-relay/.env'),'SECRET=fixture-never-returned\n');
 const shim=String.raw`
import os, sys, json, pathlib, subprocess, pwd, types
fixture=pathlib.Path(os.environ['SETUP_FIXTURE'])
commands=fixture/'commands.jsonl'
os.getuid=lambda:0
os.chown=lambda *args:None
os.getgrouplist=lambda *args:[555]
def account(name):
 if not (fixture/'account').exists(): raise KeyError(name)
 return types.SimpleNamespace(pw_uid=555,pw_gid=555,pw_dir=str(fixture/'var/lib/kanna-apt'))
pwd.getpwnam=account
real_lstat=pathlib.Path.lstat
def lstat(self):
 st=real_lstat(self)
 # Actual temp files remain the invoking user's; only ownership observation is
 # simulated. No chown/useradd/docker/sudo command can run in this fixture.
 return types.SimpleNamespace(st_mode=st.st_mode,st_uid=0)
pathlib.Path.lstat=lstat
def command(args,**kw):
 with commands.open('a') as log: log.write(json.dumps(args)+'\n')
 out=''
 if args[:3]==['docker','compose','ps']: out=args[-1]+'-id'
 elif args[:2]==['docker','inspect']:
  ident=args[-1]; out=json.dumps([{'Id':ident,'Image':ident+'-image','State':{'Running':True,'StartedAt':'same-start'}}])
 elif args[0]=='useradd':
  (fixture/'account').write_text('created'); (fixture/'var/lib/kanna-apt').mkdir()
 elif args[:2]==['docker','exec']:
  if 'node' in args:
   arriving=fixture/'arriving'
   busy=(fixture/'busy').exists() or (arriving.exists() and int(arriving.read_text())<=0)
   if arriving.exists(): arriving.write_text(str(int(arriving.read_text())-1))
   out=json.dumps({'commit':'abcdef123','pairedUsers':0,'openSockets':7 if busy else 0,'liveRows':7 if busy else 0})
 elif args==['docker','compose','config','--quiet']: pass
 elif args==['docker','compose','config','--format','json']: out=json.dumps({'services':{'caddy':{'image':'caddy:fixture'}}})
 elif args[:3]==['docker','image','inspect']: out='caddy-id-image'
 elif args==['docker','compose','up','-d','--no-deps','--no-build','--pull','never','--force-recreate','caddy']: pass
 else: raise AssertionError('Unexpected system mutation: '+str(args))
 return types.SimpleNamespace(returncode=0,stdout=out,stderr='')
subprocess.run=command
`;
 // Replace literal path strings without executing a shell or changing product
 // host defaults. This mapping is confined to the fixture's Python program.
 const source=linuxArchiveSetupHost.replaceAll('/opt/kanna-relay',join(dir,'opt/kanna-relay')).replaceAll('/opt/kanna-apt-setup',join(dir,'opt/kanna-apt-setup')).replaceAll("P('/opt')",`P(${JSON.stringify(join(dir,'opt'))})`).replaceAll('/srv/kanna-apt',join(dir,'srv/kanna-apt')).replaceAll("'/srv'",JSON.stringify(join(dir,'srv'))).replaceAll('/var/lib/kanna-apt',join(dir,'var/lib/kanna-apt'));
 const invoke=(payload:unknown)=>JSON.parse(execFileSync('/usr/bin/python3',['-c',shim+'\n'+source],{input:JSON.stringify(payload),encoding:'utf8',stdio:['pipe','pipe','pipe'],env:{...process.env,SETUP_FIXTURE:dir}}));
 const first=invoke({mode:'inspect'});expect(JSON.stringify(first)).not.toContain('fixture-never-returned');
 const payload={mode:'apply',expected:first,publisherPublicKey:'ssh-ed25519 AAAATESTONLY fixture',aptPublicKey:'TEST PUBLIC APT KEY',helper:linuxAptStorageWorker,renderer:linuxArchiveConfigRenderer,proxyMaintenance:true};
 expect(()=>invoke({...payload,proxyMaintenance:false})).toThrow();
 writeFileSync(join(dir,'busy'),'busy');expect(()=>invoke(payload)).toThrow();rmSync(join(dir,'busy'));
 writeFileSync(join(dir,'arriving'),'1');
 expect(()=>invoke(payload)).toThrow();
 expect(readFileSync(join(dir,'opt/kanna-relay/Caddyfile'),'utf8')).not.toContain('apt.kanna.build');
 const callsBeforeRetry=readFileSync(join(dir,'commands.jsonl'),'utf8').trim().split('\n').map(l=>JSON.parse(l));
 expect(callsBeforeRetry.filter(a=>a.includes('up'))).toHaveLength(0);
 rmSync(join(dir,'arriving'));
 const retryExpected=invoke({mode:'inspect'});
 writeFileSync(join(dir,'arriving'),'2');
 expect(()=>invoke({...payload,expected:retryExpected})).toThrow();
 expect(readFileSync(join(dir,'opt/kanna-relay/Caddyfile'),'utf8')).not.toContain('apt.kanna.build');
 expect(readFileSync(join(dir,'commands.jsonl'),'utf8').trim().split('\n').map(l=>JSON.parse(l)).filter(a=>a.includes('up'))).toHaveLength(0);
 rmSync(join(dir,'arriving'));
 const applied=invoke({...payload,expected:invoke({mode:'inspect'})});expect(applied.configured).toBe(true);
 const second=invoke({mode:'inspect'});expect(second.managed).toBe(true);
 const retry=invoke({...payload,expected:second});expect(retry.relay).toEqual(applied.relay);
 expect(()=>invoke({...payload,expected:second,publisherPublicKey:'ssh-ed25519 AAAADIFFERENT'})).toThrow();
 writeFileSync(join(dir,'opt/kanna-relay/Caddyfile'),readFileSync(join(dir,'opt/kanna-relay/Caddyfile'),'utf8')+'\n# concurrent change');
 expect(()=>invoke({...payload,expected:second})).toThrow();
 const calls=readFileSync(join(dir,'commands.jsonl'),'utf8').trim().split('\n').map(l=>JSON.parse(l));
 expect(calls.filter(a=>a[0]==='useradd')).toHaveLength(1);
 expect(calls.filter(a=>a.includes('up'))).toHaveLength(1);
 const auth=readFileSync(join(dir,'var/lib/kanna-apt/.ssh/authorized_keys'),'utf8');expect(auth).toContain('restrict,command=');
 expect(readFileSync(join(dir,'opt/kanna-relay/.env'),'utf8')).toBe('SECRET=fixture-never-returned\n');
 // A later normal staging relay deploy uploads fresh base templates. The
 // installed renderer restores only the managed mount/vhost, without commands.
 writeFileSync(join(dir,'opt/kanna-relay/docker-compose.yml'),readFileSync(join(repo,'services/relay/deploy/docker-compose.yml')));
 writeFileSync(join(dir,'opt/kanna-relay/Caddyfile'),readFileSync(join(repo,'services/relay/deploy/Caddyfile')));
 const renderer=linuxArchiveConfigRenderer.replaceAll('/opt/kanna-relay',join(dir,'opt/kanna-relay')).replaceAll('/opt/kanna-apt-setup',join(dir,'opt/kanna-apt-setup')).replaceAll('/srv/kanna-apt',join(dir,'srv/kanna-apt'));
 execFileSync('/usr/bin/python3',['-c',shim+'\n'+renderer],{encoding:'utf8',env:{...process.env,SETUP_FIXTURE:dir}});
 expect(readFileSync(join(dir,'opt/kanna-relay/Caddyfile'),'utf8')).toContain('apt.kanna.build');
 expect(readFileSync(join(dir,'opt/kanna-relay/docker-compose.yml'),'utf8')).toContain(join(dir,'srv/kanna-apt/archive')+':'+join(dir,'srv/kanna-apt')+':ro');

});

it('applies orchestration with disposable protected keys and merges only Linux selectors after HTTPS readback',async()=>{
 const { nodeCommandRunner }=await import('../src/runtime/process');
 const f=fixture();custody.home=join(f.context.repoRoot,'custody');mkdirSync(custody.home,{mode:0o700});mkdirSync(join(custody.home,'.kanna'),{mode:0o700});
 const envPath=join(custody.home,'.kanna/.env.release.local');writeFileSync(envPath,'# desktop-owned\nAPPLE_KEYCHAIN_PROFILE="keep-this"\n',{mode:0o600});
 let transferred: Record<string,string>|undefined;
 const base=f.context.runner;
 const runner:CommandRunner={run:async(command,args,options)=>{
  if(command==='/usr/sbin/system_profiler')return {exitCode:0,stdout:JSON.stringify({SPHardwareDataType:[{machine_name:'MacBook Pro'}]}),stderr:''};
  if(command==='/usr/bin/ssh-keygen')return nodeCommandRunner.run(command,args,options);
  if(command==='/usr/bin/ssh' && JSON.parse(options!.stdin!).mode==='apply'){
   transferred=JSON.parse(options!.stdin!);return {exitCode:0,stdout:JSON.stringify({configured:true}),stderr:''};
  }
  return base.run(command,args,options);
 }};
 const context={...f.context,runner};const plan=await setupLinuxArchive(context,f.input);if (!("sha256" in plan)) throw Error("Expected plan");const path=join(f.context.repoRoot,'plan.json');writeFileSync(path,JSON.stringify(plan));
 vi.stubGlobal('fetch',async(url:string)=>{expect(url).toBe('https://apt.kanna.build/keys/kanna-archive.asc');return new Response(transferred!.aptPublicKey);});
 const result=await setupLinuxArchive(context,{...f.input,mode:'apply',plan:path,confirm:plan.sha256,proxyMaintenance:true});
 expect(result).toMatchObject({configured:true,published:false});
 expect(Object.keys(transferred!)).toEqual(['mode','expected','publisherPublicKey','aptPublicKey','helper','renderer','proxyMaintenance']);
 expect(JSON.stringify(transferred)).not.toContain('PRIVATE KEY');
 const updated=readFileSync(envPath,'utf8');expect(updated).toContain('APPLE_KEYCHAIN_PROFILE="keep-this"');expect(updated).toContain('KANNA_LINUX_SSH_HOST="34.133.43.193"');
 expect(JSON.stringify(result)).not.toContain('PRIVATE KEY');
 for(const name of ['private.asc','passphrase','publisher_identity','known_hosts']) expect(statSync(join(custody.home,'.kanna/linux-apt',name)).mode & 0o077).toBe(0);
 const fingerprint=(result as {fingerprint:string}).fingerprint;
 const retry=await setupLinuxArchive(context,{...f.input,mode:'apply',plan:path,confirm:plan.sha256,proxyMaintenance:true});expect(retry).toHaveProperty('fingerprint',fingerprint);
 writeFileSync(envPath,readFileSync(envPath,'utf8').replace('KANNA_LINUX_SSH_HOST="34.133.43.193"','KANNA_LINUX_SSH_HOST="different.invalid"'));
 await expect(setupLinuxArchive(context,{...f.input,mode:'apply',plan:path,confirm:plan.sha256,proxyMaintenance:true})).rejects.toThrow(/no implicit archive repointing/);
 expect(readFileSync(envPath,'utf8')).toContain('different.invalid');
},30000);

it('probes actual authenticated sockets, refusing absent/mismatched stats without leaking credentials or client rows',()=>{
 const valid={status:'ok',commit:'abcdef123',connections:0,bytes:{connections:{open:1}},liveConnections:[{secret:'PRIVATE CLIENT ROW'}]};
 const invoke=(stats:unknown,token='disposable-token-not-a-real-credential',status=200)=>{
  const script=`global.fetch=async(url,options)=>{if(url!=='http://127.0.0.1:8080/stats'||options.redirect!=='error'||options.headers.Authorization!=='Bearer '+process.env.KANNA_RELAY_STATS_TOKEN)throw Error();return {ok:${status===200},json:async()=>(${JSON.stringify(stats)})}};\n`+linuxRelaySocketProbe;
  return execFileSync(process.execPath,['-e',script],{encoding:'utf8',stdio:['pipe','pipe','pipe'],env:{...process.env,KANNA_RELAY_STATS_TOKEN:token,KANNA_RELAY_COMMIT:'abcdef123'}});
 };
 expect(JSON.parse(invoke(valid))).toEqual({commit:'abcdef123',pairedUsers:0,openSockets:1,liveRows:1});
 expect(invoke(valid)).not.toContain('PRIVATE CLIENT ROW');
 expect(JSON.parse(invoke({...valid,bytes:{connections:{open:0}},liveConnections:[]}))).toHaveProperty('openSockets',0);
 for(const bad of [{...valid,liveConnections:undefined},{...valid,liveConnections:[]},{...valid,commit:'unknown'},{...valid,commit:'abcdef124'},{...valid,bytes:{}},{...valid,bytes:{connections:{open:-1}}}]) expect(()=>invoke(bad)).toThrow();
 for(const [token,status] of [['',200],['disposable-token-not-a-real-credential',403]] as const) {
  try { invoke(valid,token,status);throw Error('unexpected success'); }
  catch(error) { const result=error as {stderr?:Buffer};expect(String(result.stderr)).toContain('Cannot verify authenticated relay socket stats');expect(String(result.stderr)).not.toContain('disposable-token'); }
 }
});
