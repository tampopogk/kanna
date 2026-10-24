"""Offline checks for the external-token bridge: isolation, renewal, redaction."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from external import receive, send, stop

HERE = Path(__file__).resolve().parent
SECRET = 'SYNTHETIC_TOKEN_DO_NOT_LOG'
FAKE = r'''#!PYTHON
import json, os, pathlib, sys
if '--version' in sys.argv:
    print('codex synthetic-test')
    sys.exit()
assert not any(k in os.environ for k in ['OPENAI_API_KEY', 'ANTHROPIC_API_KEY', 'CLAUDE_CODE_OAUTH_TOKEN'])
assert not pathlib.Path(os.environ['CODEX_HOME'], 'auth.json').exists()
def emit(v):
    print(json.dumps(v), flush=True)
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get('method')
    if method == 'initialized':
        continue
    if method == 'initialize':
        assert msg['params']['capabilities']['experimentalApi']
        result = {}
    elif method == 'account/login/start':
        assert set(msg['params']) == {'type', 'accessToken', 'chatgptAccountId'}
        token = msg['params']['accessToken']
        print(token, file=sys.stderr, flush=True)
        if token.startswith('reject'):
            emit({'id': msg['id'], 'error': {'message': token}})
            continue
        result = {'type': 'chatgptAuthTokens', 'untrusted': token}
    elif method == 'thread/start':
        result = {'thread': {'id': 'test-thread'}}
    elif method == 'turn/start':
        emit({'id': 99, 'method': 'account/chatgptAuthTokens/refresh',
              'params': {'reason': 'unauthorized', 'previousAccountId': 'test-account'}})
        renewal = json.loads(sys.stdin.readline())
        assert renewal['id'] == 99
        assert renewal['result']['accessToken'] == 'replacement'
        emit({'id': msg['id'], 'result': {'turn': {'id': 'test-turn'}}})
        emit({'method': 'item/completed', 'params': {'item': {
            'type': 'agentMessage', 'text': 'KANNA_AUTH_PROBE_OK', 'untrusted': token}}})
        emit({'method': 'turn/completed', 'params': {
            'turn': {'id': 'test-turn', 'status': 'completed'}, 'untrusted': token}})
        continue
    else:
        raise RuntimeError('Unexpected request')
    emit({'id': msg['id'], 'result': result})
'''

with tempfile.TemporaryDirectory() as tmp:
    binary = Path(tmp) / 'codex'
    binary.write_text(FAKE.replace('PYTHON', sys.executable))
    binary.chmod(0o755)
    env = dict(os.environ, PATH=tmp + os.pathsep + os.environ['PATH'],
               OPENAI_API_KEY=SECRET, ANTHROPIC_API_KEY=SECRET, CLAUDE_CODE_OAUTH_TOKEN=SECRET)
    for reject in (False, True):
        p = subprocess.Popen([sys.executable, str(HERE / 'external.py'), '--worker'], env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0)
        try:
            send(p.stdin, {'credentials': {'accessToken': ('reject:' if reject else '') + SECRET,
                        'chatgptAccountId': 'test-account'}, 'start_at': 0})
            if reject:
                p.wait(timeout=10)
                output = p.stdout.read() + p.stderr.read()
                assert p.returncode != 0
                assert SECRET.encode() not in output
            else:
                assert receive(p.stdout, 10) == {'event': 'refresh'}
                send(p.stdin, {'credentials': {'accessToken': 'replacement', 'chatgptAccountId': 'test-account'}})
                report = receive(p.stdout, 10)
                assert SECRET not in json.dumps(report)
                result = report['result']
                assert result['inference_ok'] and result['refresh_requests'] == result['refresh_responses'] == 1
                assert not result['auth_file_present'] and not result['refresh_credential_present']
                send(p.stdin, {'stop': True})
                p.wait(timeout=10)
                assert p.returncode == 0
                assert SECRET.encode() not in p.stderr.read()
        finally:
            stop(p)
print('PASS: external-token isolation, renewal callback, success parsing, error redaction')
