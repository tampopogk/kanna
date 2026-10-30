#!/usr/bin/env python3
"""Experimental app-server holder probe. Same process/thread across three phases."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import threading
import time

MARKER = 'KANNA_AUTH_PROBE_OK'


def send(stream, message):
    stream.write(json.dumps(message).encode() + b'\n')
    stream.flush()


def receive(stream, timeout=180):
    # Unbuffered pipes: selector readiness must agree with readline's buffer.
    with selectors.DefaultSelector() as selector:
        selector.register(stream, selectors.EVENT_READ)
        if not selector.select(timeout):
            raise TimeoutError()
        line = stream.readline()
        if not line:
            raise EOFError()
        return json.loads(line)


def stop(process):
    if process.stdin and not process.stdin.closed:
        process.stdin.close()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


class Rpc:
    def __init__(self, process, renew):
        self.process, self.renew = process, renew
        self.sequence = 0
        self.events = []

    def read(self):
        event = receive(self.process.stdout)
        if 'method' in event and 'id' in event:
            if event['method'] != 'account/chatgptAuthTokens/refresh':
                # No approvals/tools are allowed by this probe.
                send(self.process.stdin, {'id': event['id'], 'error': {
                    'code': -32601, 'message': 'Unsupported probe request'}})
            else:
                send(self.process.stdin, {'id': event['id'], 'result': self.renew(event['params'])})
        else:
            self.events.append(event)
        return event

    def call(self, method, params):
        self.sequence += 1
        request_id = self.sequence
        send(self.process.stdin, {'id': request_id, 'method': method, 'params': params})
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            event = self.read()
            if event.get('id') == request_id and 'method' not in event:
                if 'error' in event:
                    # Do not expose vendor errors, which may echo credentials.
                    raise RuntimeError('RPC rejected')
                return event['result']
        raise TimeoutError()


def worker():
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix='kanna-external-auth-') as tmp:
        env = {'HOME': tmp, 'CODEX_HOME': tmp, 'PATH': os.environ['PATH'], 'LANG': 'C.UTF-8'}
        version = subprocess.check_output(['codex', '--version'], env=env, text=True).strip()
        process = subprocess.Popen(['codex', '-c', 'cli_auth_credentials_store="file"',
            'app-server', '--listen', 'stdio://'], env=env, cwd=tmp, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
        state = {}

        def renew(params):
            state['refresh_requests'] += 1
            # Account IDs never appear in evidence, but must agree before supplying tokens.
            if params.get('previousAccountId') != state['account_id']:
                raise RuntimeError('Account mismatch')
            if params.get('reason') != 'unauthorized':
                raise RuntimeError('Unexpected refresh reason')
            send(sys.stdout.buffer, {'event': 'refresh'})
            response = receive(sys.stdin.buffer, 9)
            state['refresh_responses'] += 1
            return response['credentials']

        rpc = Rpc(process, renew)
        try:
            rpc.call('initialize', {'clientInfo': {'name': 'kanna_auth_probe', 'version': '0.2'},
                                    'capabilities': {'experimentalApi': True}})
            send(process.stdin, {'method': 'initialized', 'params': {}})
            thread_id = None
            while True:
                request = json.loads(sys.stdin.buffer.readline())
                if request.get('stop'):
                    break
                credentials = request['credentials']
                state.update(account_id=credentials['chatgptAccountId'],
                             refresh_requests=0, refresh_responses=0)
                login = rpc.call('account/login/start', {'type': 'chatgptAuthTokens', **credentials})
                if thread_id is None:
                    thread_id = rpc.call('thread/start', {'cwd': tmp, 'ephemeral': True,
                        'sandbox': 'read-only', 'approvalPolicy': 'never'})['thread']['id']
                rpc.events.clear()
                time.sleep(max(0, request['start_at'] - time.time()))
                started = time.time()
                turn_id = rpc.call('turn/start', {'threadId': thread_id, 'input': [
                    {'type': 'text', 'text': f'Reply with exactly {MARKER}. Do not use tools.'}]})['turn']['id']
                completed = None
                deadline = time.monotonic() + 180
                while time.monotonic() < deadline:
                    completed = next((e['params']['turn'] for e in rpc.events
                        if e.get('method') == 'turn/completed'
                        and e['params']['turn']['id'] == turn_id), None)
                    if completed:
                        break
                    rpc.read()
                marked = any(e.get('method') == 'item/completed'
                    and e['params']['item'].get('type') == 'agentMessage'
                    and e['params']['item'].get('text', '').strip() == MARKER for e in rpc.events)
                # The mode is memory-only in this version. Also check all JSON state for refresh keys.
                from worker import contains_refresh
                refresh_present = any(contains_refresh(json.loads(p.read_text()))
                    for p in Path(tmp).rglob('*.json'))
                send(sys.stdout.buffer, {'event': 'result', 'result': {
                    'version': version, 'login_ok': login.get('type') == 'chatgptAuthTokens',
                    'inference_ok': bool(completed and completed['status'] == 'completed' and marked),
                    'turn_status': 'completed' if completed and completed['status'] == 'completed' else 'failed',
                    'started_at': started, 'ended_at': time.time(),
                    'refresh_requests': state['refresh_requests'],
                    'refresh_responses': state['refresh_responses'],
                    'refresh_credential_present': refresh_present,
                    'auth_file_present': (Path(tmp) / 'auth.json').exists(),
                    'same_process_and_thread': True}})
        finally:
            stop(process)


def run(host):
    from probe import VMS, holder_token
    from keychain import entry

    def credentials(refresh=False):
        access = holder_token(refresh=refresh)
        tokens = json.loads(entry('codex-holder'))['tokens']
        return {'accessToken': access, 'chatgptAccountId': tokens['account_id']}

    report = {'timestamp': datetime.now(timezone.utc).isoformat(), 'host': host,
              'credential_mode': 'chatgptAuthTokens', 'rounds': []}
    current = credentials()
    processes = []
    try:
        for vm in VMS:
            processes.append(subprocess.Popen(['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', host,
                'sudo', '-n', 'incus', 'exec', vm, '--', 'su', '-', 'probe', '-c',
                "'python3 /opt/agent-sign-in/external.py --worker'"],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0))
        for phase in ('initial', 'holder_refreshed', 'injected_unauthorized'):
            changed = None
            if phase == 'holder_refreshed':
                newer = credentials(refresh=True)
                changed = newer['accessToken'] != current['accessToken']
                current = newer
            supplied = dict(current)
            replacement = None
            if phase == 'injected_unauthorized':
                # Preserve JWT claims; invalidate its signature, provoking a real backend 401.
                parts = supplied['accessToken'].split('.')
                parts[2] = ('A' if parts[2][0] != 'A' else 'B') + parts[2][1:]
                supplied['accessToken'] = '.'.join(parts)
            lock = threading.Lock()
            refresh_count = 0
            start_at = time.time() + 10

            def exchange(pair):
                nonlocal replacement, refresh_count
                vm, process = pair
                send(process.stdin, {'credentials': supplied, 'start_at': start_at})
                while True:
                    event = receive(process.stdout, 210)
                    if event.get('event') == 'refresh':
                        with lock:
                            # One holder refresh serves simultaneous requests from both VMs.
                            if replacement is None:
                                replacement = credentials(refresh=True)
                                refresh_count += 1
                        send(process.stdin, {'credentials': replacement})
                    elif event.get('event') == 'result':
                        return {'machine': vm, **event['result']}
                    else:
                        raise RuntimeError('Unexpected worker response')

            with ThreadPoolExecutor(max_workers=2) as pool:
                results = list(pool.map(exchange, zip(VMS, processes)))
            overlap = min(w['ended_at'] for w in results) - max(w['started_at'] for w in results)
            report['rounds'].append({'phase': phase, 'holder_access_changed': changed,
                'holder_refreshes_on_request': refresh_count,
                'replacement_access_changed': replacement is not None and replacement['accessToken'] != current['accessToken'],
                'overlap_seconds': max(0, overlap), 'workers': results})
            if replacement:
                current = replacement
            print(f'{phase}: {sum(w["inference_ok"] for w in results)}/2 successful; '
                  f'{sum(w["refresh_requests"] for w in results)} refresh requests', file=sys.stderr, flush=True)
        report['passed'] = (len(report['rounds']) == 3
            and all(r['overlap_seconds'] > 0 and all(w['login_ok'] and w['inference_ok']
                and not w['refresh_credential_present'] for w in r['workers']) for r in report['rounds'])
            and report['rounds'][1]['holder_access_changed']
            and report['rounds'][2]['holder_refreshes_on_request'] == 1
            and report['rounds'][2]['replacement_access_changed']
            and all(w['refresh_requests'] >= 1 and w['refresh_requests'] == w['refresh_responses']
                    for w in report['rounds'][2]['workers']))
        print(json.dumps(report, indent=2))
        return report['passed']
    finally:
        for process in processes:
            if process.poll() is None:
                try:
                    send(process.stdin, {'stop': True})
                except BrokenPipeError:
                    pass
            stop(process)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', action='store_true')
    parser.add_argument('--host', default='jeremyhale@172.16.0.105')
    args = parser.parse_args()
    os.umask(0o077)
    try:
        if args.worker:
            worker()
        else:
            raise SystemExit(0 if run(args.host) else 1)
    except Exception as error:
        # No tracebacks or vendor messages: either could contain credentials.
        print(json.dumps({'failed': True, 'error_kind': type(error).__name__}), file=sys.stderr)
        sys.exit(1)
