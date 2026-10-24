#!/usr/bin/env python3
"""Throwaway coordinator. Sign in in separate terminals, then run the two-VM probe."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import getpass
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import time

from keychain import entry

HERE = Path(__file__).resolve().parent
VMS = ('agent-sign-in-a', 'agent-sign-in-b')


def isolated_env(home):
    return {'HOME': str(home), 'CODEX_HOME': str(home), 'PATH': os.environ['PATH'],
            'LANG': 'en_US.UTF-8'}


def store_claude():
    # Claude's TUI runs in its own terminal. Never follow it with a getpass prompt
    # on the same screen: its redraws/terminal state can hide or swallow input.
    print('Paste the setup token from the Claude sign-in tab. Input is hidden.', flush=True)
    token = ''
    while not token:
        token = getpass.getpass('Claude setup token: ').strip()
    entry('claude', token)
    print('Claude token saved in your Mac Keychain. You can return to the task.')


def sign_in_codex():
    # Human completes the vendor flow. Never inspect the everyday login files.
    input('Press Enter to open the Codex browser sign-in. ')
    with tempfile.TemporaryDirectory(prefix='kanna-holder-login-') as tmp:
        subprocess.run(['codex', '-c', 'cli_auth_credentials_store="file"', 'login'],
                       env=isolated_env(tmp), cwd=tmp, check=True)
        auth = (Path(tmp) / 'auth.json').read_text()
        if not json.loads(auth).get('tokens', {}).get('access_token'):
            raise RuntimeError('Expected a ChatGPT subscription login')
        entry('codex-holder', auth)
    print('Codex holder login saved in your Mac Keychain; temporary login file removed.')


def holder_token(refresh=False):
    auth = entry('codex-holder')
    if refresh:
        # Exactly one holder runs this sequentially; VMs never receive this file.
        with tempfile.TemporaryDirectory(prefix='kanna-holder-refresh-') as tmp:
            path = Path(tmp) / 'auth.json'
            path.write_text(auth)
            path.chmod(0o600)
            p = subprocess.Popen(['codex', '-c', 'cli_auth_credentials_store="file"',
                'app-server', '--listen', 'stdio://'], env=isolated_env(tmp), cwd=tmp,
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
            selector = selectors.DefaultSelector()
            selector.register(p.stdout, selectors.EVENT_READ)

            def send(message):
                p.stdin.write(json.dumps(message).encode() + b'\n')
                p.stdin.flush()

            def receive(request_id):
                deadline = time.monotonic() + 60
                while time.monotonic() < deadline:
                    if not selector.select(timeout=1):
                        continue
                    line = p.stdout.readline()
                    if not line:
                        raise RuntimeError('Holder app-server exited')
                    response = json.loads(line)
                    if response.get('id') == request_id:
                        if 'error' in response:
                            raise RuntimeError('Holder refresh failed; sign in again')
                        return response
                raise RuntimeError('Holder refresh timed out')

            try:
                send({'id': 1, 'method': 'initialize', 'params': {'clientInfo': {
                    'name': 'kanna_auth_probe', 'version': '0.1'}}})
                receive(1)
                send({'method': 'initialized', 'params': {}})
                send({'id': 2, 'method': 'account/read', 'params': {'refreshToken': True}})
                receive(2)
                auth = path.read_text()
                entry('codex-holder', auth)
            finally:
                p.terminate()
                try:
                    p.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait()
                selector.close()
    return json.loads(auth)['tokens']['access_token']


def run(host):
    report = {'timestamp': datetime.now(timezone.utc).isoformat(), 'host': host,
              'machines': list(VMS), 'rounds': []}
    claude = entry('claude')
    previous_access = None
    for round_number in (1, 2):
        try:
            access = holder_token(refresh=round_number == 2)
        except Exception as e:
            report['holder_error'] = {'round': round_number, 'kind': type(e).__name__}
            break
        changed = None if previous_access is None else access != previous_access
        for provider, token in (('claude', claude), ('codex', access)):
            start_at = time.time() + 10

            def worker(vm):
                command = ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', host,
                    'sudo', '-n', 'incus', 'exec', vm, '--', 'su', '-', 'probe', '-c',
                    "'python3 /opt/agent-sign-in/worker.py'"]
                p = subprocess.run(command, input=json.dumps({'provider': provider,
                    'token': token, 'start_at': start_at}), text=True, capture_output=True, timeout=240)
                if p.returncode:
                    return {'machine': vm, 'inference_ok': False, 'error_kind': 'worker_failed'}
                return {'machine': vm, **json.loads(p.stdout)}

            with ThreadPoolExecutor(max_workers=2) as pool:
                results = list(pool.map(worker, VMS))
            overlap = (min(r.get('ended_at', 0) for r in results)
                       - max(r.get('started_at', 0) for r in results))
            report['rounds'].append({'round': round_number, 'provider': provider,
                'holder_access_changed': changed if provider == 'codex' else None,
                'overlap_seconds': max(0, overlap), 'workers': results})
        previous_access = access
    report['passed'] = len(report['rounds']) == 4 and all(r['overlap_seconds'] > 0 and all(
        w.get('inference_ok') and not w.get('refresh_credential_present') for w in r['workers'])
        for r in report['rounds'])
    report['passed'] = report['passed'] and report['rounds'][-1]['holder_access_changed'] is True
    print(json.dumps(report, indent=2))
    return report['passed']


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['store-claude', 'sign-in-codex', 'run'])
    parser.add_argument('--host', default='jeremyhale@172.16.0.105')
    args = parser.parse_args()
    os.umask(0o077)
    try:
        if args.command == 'store-claude':
            store_claude()
        elif args.command == 'sign-in-codex':
            sign_in_codex()
        else:
            raise SystemExit(0 if run(args.host) else 1)
    except RuntimeError as e:
        # These messages are authored here or in keychain.py, never CLI output.
        raise SystemExit(str(e))
    except Exception as e:
        # Do not accidentally put auth.json, JWTs or CLI output in a traceback.
        raise SystemExit(f'Probe stopped ({type(e).__name__}); no raw credential output logged.')
