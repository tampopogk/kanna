/** Fixed existing-staging-host operation. Input arrives over pinned SSH stdin;
 * no arbitrary shell fragments, credentials, or relay environment are returned. */
export const linuxArchiveSetupHost = String.raw`
import os, sys, json, hashlib, stat, subprocess, pathlib, pwd, fcntl
P=pathlib.Path
base=P('/opt/kanna-relay')
owned=P('/opt/kanna-apt-setup')
archive=P('/srv/kanna-apt/archive')
def fail(message): raise RuntimeError(message)
def run(args, data=None):
    r=subprocess.run(args,input=data,text=True,capture_output=True,cwd=base)
    if r.returncode: fail('Setup command refused: '+args[0]+' '+args[1]+' (output suppressed)')
    return r.stdout.strip()
def regular(path):
    st=path.lstat()
    if not stat.S_ISREG(st.st_mode): fail('Expected regular file: '+str(path))
    return path.read_bytes()
def digest(data): return hashlib.sha256(data).hexdigest()
def container(service):
    ids=run(['docker','compose','ps','-q',service]).splitlines()
    if len(ids)!=1: fail('Expected one running '+service+' container')
    v=json.loads(run(['docker','inspect',ids[0]]))[0]
    if not v['State']['Running']: fail(service+' is not running')
    return {'id':v['Id'],'image':v['Image'],'startedAt':v['State']['StartedAt']}
def connections(relay):
    program="fetch('http://127.0.0.1:8080/health').then(r=>{if(!r.ok)throw Error();return r.json()}).then(j=>{if(!Number.isSafeInteger(j.connections)||j.connections<0)throw Error();console.log(j.connections)}).catch(()=>process.exit(1))"
    count=run(['docker','exec',relay['id'],'node','-e',program])
    if not count.isdigit(): fail('Cannot verify live relay connection count')
    return int(count)
def snapshot():
    for path in [P('/opt'),base,P('/srv')]:
        if not stat.S_ISDIR(path.lstat().st_mode): fail('Non-directory setup ancestor')
    try: account=pwd.getpwnam('kanna-apt')
    except KeyError: account=None
    if account and not (owned/'identity.json').is_file(): fail('Existing unowned kanna-apt account')
    if not account and not (owned/'identity.json').exists() and (P('/srv/kanna-apt').exists() or P('/var/lib/kanna-apt').exists()): fail('Existing unowned archive/home path')
    files={name:digest(regular(base/name)) for name in ['docker-compose.yml','Caddyfile','.env']}
    return {'files':files,'relay':container('relay'),'caddy':container('caddy'),
      'managed': (owned/'identity.json').exists(), 'accountUid':account.pw_uid if account else None}
def render(compose,caddy):
    mount='      - /srv/kanna-apt/archive:/srv/kanna-apt:ro'
    marker='      - ./Caddyfile:/etc/caddy/Caddyfile:ro'
    block='\n# BEGIN kanna apt staging (kd owned)\napt.kanna.build {\n    root * /srv/kanna-apt\n    file_server {\n        hide .*\n    }\n    @metadata path /dists/* /linux/state.json\n    header @metadata Cache-Control "no-cache"\n}\n# END kanna apt staging\n'
    if mount not in compose:
        if compose.count(marker)!=1: fail('Unsupported compose Caddy mount layout')
        # Confirm the insertion belongs to caddy, not another service.
        prefix=compose.split(marker)[0]
        if prefix.rsplit('\n  ',1)[-1].split(':',1)[0]!='caddy':
            # Nested indentation also matches; use the last top-level service.
            import re
            services=re.findall(r'^  ([a-zA-Z0-9_-]+):$',prefix,re.M)
            if not services or services[-1]!='caddy': fail('Caddy mount ownership mismatch')
        compose=compose.replace(marker,marker+'\n'+mount)
    if 'apt.kanna.build' in caddy:
        if caddy.count(block)!=1: fail('Existing unowned apt Caddy vhost')
    else: caddy+=block
    return compose,caddy
request=json.load(sys.stdin)
if os.getuid()!=0: fail('Setup requires the authenticated administrator sudo path')
if request['mode']=='inspect':
    result=snapshot()
    result['relayConnections']=connections(result['relay'])
    result['freeBytes']=os.statvfs('/srv').f_bavail*os.statvfs('/srv').f_frsize
    # Validate supported layout during read-only inspection.
    before=(regular(base/'docker-compose.yml').decode(),regular(base/'Caddyfile').decode())
    result['proxyChange']=render(*before)!=before
    print(json.dumps(result)); sys.exit(0)
if request['mode']!='apply': fail('Unknown setup operation')
# Root-only setup lock is separate from the archive publisher lock.
if owned.exists():
    st=owned.lstat()
    if not stat.S_ISDIR(st.st_mode) or st.st_uid!=0 or st.st_mode&0o022: fail('Unsafe setup directory')
else: owned.mkdir(mode=0o700)
fd=os.open(owned/'setup.lock',os.O_CREAT|os.O_RDWR|os.O_NOFOLLOW,0o600)
fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
observed=snapshot()
expected=request['expected'].copy(); expected.pop('freeBytes',None); expected.pop('relayConnections',None); expected.pop('proxyChange',None)
if observed!=expected: fail('Host configuration changed since plan; inspect and plan again')
if os.statvfs('/srv').f_bavail*os.statvfs('/srv').f_frsize < 1073741824: fail('Archive requires at least 1GiB available before setup')
public=request['publisherPublicKey'].strip()
import re
if not re.fullmatch(r'ssh-ed25519 [A-Za-z0-9+/=]+(?: [^\r\n]*)?',public): fail('Invalid dedicated publisher key')
helper=request['helper']
receipt={'publisherPublicKey':public,'helperSha256':digest(helper.encode()),'rendererSha256':digest(request['renderer'].encode()),'aptPublicKeySha256':digest(request['aptPublicKey'].encode())}
if (owned/'identity.json').exists() and json.loads(regular(owned/'identity.json'))!=receipt: fail('Existing setup identity differs; no implicit key rotation')
oldCompose=regular(base/'docker-compose.yml'); oldCaddy=regular(base/'Caddyfile')
compose,caddy=render(oldCompose.decode(),oldCaddy.decode())
changed=compose.encode()!=oldCompose or caddy.encode()!=oldCaddy
if changed and (not request.get('proxyMaintenance') or connections(observed['relay'])!=0): fail('Caddy mount change requires explicit proxy maintenance and zero live relay connections')
# Recreating with an updated local floating tag would silently upgrade Caddy.
resolved=json.loads(run(['docker','compose','config','--format','json']))
image=resolved['services']['caddy']['image']
if run(['docker','image','inspect',image,'--format','{{.Id}}'])!=observed['caddy']['image']: fail('Configured Caddy image no longer matches running image; no implicit upgrade')
# Validate with the existing Caddy image before replacing any running config.
run(['docker','exec','-i',observed['caddy']['id'],'caddy','validate','--config','/dev/stdin','--adapter','caddyfile'],caddy)
# Preserve rollback material exclusively, never overwrite an earlier backup.
for name,data in [('docker-compose.yml',oldCompose),('Caddyfile',oldCaddy)]:
    backup=owned/(name+'.before')
    if not backup.exists():
        with backup.open('xb') as f: f.write(data)
        backup.chmod(0o600)
if not (owned/'identity.json').exists():
    with (owned/'identity.json').open('x') as f:
        json.dump(receipt,f,sort_keys=True); f.flush(); os.fsync(f.fileno())
    (owned/'identity.json').chmod(0o600)
if observed['accountUid'] is None:
    run(['useradd','--system','--user-group','--home-dir','/var/lib/kanna-apt','--create-home','--shell','/bin/sh','kanna-apt'])
account=pwd.getpwnam('kanna-apt')
if account.pw_dir!='/var/lib/kanna-apt' or account.pw_uid==0 or os.getgrouplist('kanna-apt',account.pw_gid)!=[account.pw_gid]: fail('Publisher has unexpected identity/groups')
for path,uid,gid,mode in [(P('/srv/kanna-apt'),0,0,0o755),(archive,account.pw_uid,account.pw_gid,0o755),(P(account.pw_dir)/'.ssh',0,0,0o755)]:
    if not path.exists(): path.mkdir(mode=mode)
    st=path.lstat()
    if not stat.S_ISDIR(st.st_mode): fail('Unsafe setup directory')
    if path==archive and any(path.iterdir()) and not observed['managed']: fail('Archive contains unowned state')
    os.chown(path,uid,gid); path.chmod(mode)
# Home and authorized_keys are root-owned: the publisher cannot replace its
# forced command. No shell, forwarding, metadata token or other VM files are
# reachable through this credential; it speaks only the existing storage RPC.
os.chown(account.pw_dir,0,0); os.chmod(account.pw_dir,0o755)
def write(path,data,mode=0o644):
    if path.exists() and not stat.S_ISREG(path.lstat().st_mode): fail('Unsafe setup file')
    temp=path.with_name(path.name+'.setup-new')
    with temp.open('xb') as f: f.write(data); f.flush(); os.fsync(f.fileno())
    temp.chmod(mode); os.replace(temp,path)
write(owned/'storage-helper.py',helper.encode())
write(owned/'render-config.py',request['renderer'].encode())
# Parent must be traversable for the forced helper; backups/receipt stay private.
owned.chmod(0o755)
command='/usr/bin/python3 -u /opt/kanna-apt-setup/storage-helper.py /srv/kanna-apt/archive'
write(P(account.pw_dir)/'.ssh/authorized_keys',('restrict,command="'+command+'" '+public+'\n').encode())
keys=archive/'keys'
if not keys.exists(): keys.mkdir(mode=0o755)
if not stat.S_ISDIR(keys.lstat().st_mode): fail('Unsafe public key directory')
key=keys/'kanna-archive.asc'
if key.exists() and regular(key)!=request['aptPublicKey'].encode(): fail('Public apt key rotation is not setup')
if not key.exists(): write(key,request['aptPublicKey'].encode())
changed=compose.encode()!=oldCompose or caddy.encode()!=oldCaddy
try:
    if changed:
        if connections(observed['relay'])!=0: fail('Relay connections arrived before Caddy maintenance; no proxy change')
        if regular(base/'docker-compose.yml')!=oldCompose or regular(base/'Caddyfile')!=oldCaddy or digest(regular(base/'.env'))!=observed['files']['.env']: fail('Relay config changed before replacement')
        write(base/'docker-compose.yml',compose.encode()); write(base/'Caddyfile',caddy.encode())
        run(['docker','compose','config','--quiet'])
        run(['docker','compose','up','-d','--no-deps','--no-build','--pull','never','--force-recreate','caddy'])
    if digest(regular(base/'.env'))!=observed['files']['.env']: fail('Relay environment changed during setup')
    if container('relay')!=observed['relay']: fail('Relay identity moved during apt setup')
except Exception:
    if changed and regular(base/'docker-compose.yml')==compose.encode() and regular(base/'Caddyfile')==caddy.encode():
        write(base/'docker-compose.yml',oldCompose); write(base/'Caddyfile',oldCaddy)
        run(['docker','compose','up','-d','--no-deps','--no-build','--pull','never','--force-recreate','caddy'])
    raise
write(owned/'receipt.json',json.dumps(receipt,sort_keys=True).encode(),0o600)
print(json.dumps({'configured':True,'relay':container('relay'),'caddy':container('caddy'),'receipt':receipt}))
`;

/** Used only by an explicitly authorized future staging relay deploy after it
 * uploads the reviewed base templates. Root-owned setup receipt is the opt-in. */
export const linuxArchiveConfigRenderer = linuxArchiveSetupHost.split('request=json.load(sys.stdin)')[0] + String.raw`
if os.getuid()!=0: fail('Archive config preservation requires administrator')
if not (owned/'receipt.json').exists(): fail('Archive setup receipt missing')
for path in [owned,owned/'receipt.json']:
    st=path.lstat()
    if st.st_uid!=0 or st.st_mode&0o022: fail('Unsafe archive setup owner')
compose,caddy=render(regular(base/'docker-compose.yml').decode(),regular(base/'Caddyfile').decode())
# Called within the normal reviewed relay deployment, before compose pull/up.
(base/'docker-compose.yml').write_text(compose)
(base/'Caddyfile').write_text(caddy)
`;
