"""No accounts/network: check isolation and evidence redaction with fake CLIs."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
SENTINEL = 'SYNTHETIC_SECRET_MUST_NOT_APPEAR_IN_EVIDENCE'
FAKE = '''#!{python}
import json, os, pathlib, sys
provider = pathlib.Path(sys.argv[0]).name
if '--version' in sys.argv:
    print(provider + ' synthetic-test')
    sys.exit(0)
assert 'ANTHROPIC_API_KEY' not in os.environ
assert 'OPENAI_API_KEY' not in os.environ
if provider == 'codex':
    assert 'CLAUDE_CODE_OAUTH_TOKEN' not in os.environ
    if 'login' in sys.argv:
        token = sys.stdin.read()
        if token.startswith('reject:'):
            print('agent identity JWT rejected ' + token, file=sys.stderr)
            sys.exit(1)
        pathlib.Path(os.environ['CODEX_HOME'], 'auth.json').write_text(json.dumps(
            {{'tokens': {{'access_token': token, 'refresh_token': ''}}, 'last_refresh': 'today'}}))
        sys.exit(0)
    print(json.dumps({{'type': 'item.completed', 'item': {{'type': 'agent_message', 'text': 'KANNA_AUTH_PROBE_OK'}}}}))
    print(json.dumps({{'type': 'turn.completed'}}))
else:
    assert os.environ['CLAUDE_CODE_OAUTH_TOKEN'] == '{sentinel}'
    print(json.dumps({{'is_error': False, 'result': 'KANNA_AUTH_PROBE_OK', 'untrusted': '{sentinel}'}}))
print('{sentinel}', file=sys.stderr)
'''


with tempfile.TemporaryDirectory(prefix='kanna-auth-selftest-') as tmp:
    for provider in ('claude', 'codex'):
        binary = Path(tmp) / provider
        binary.write_text(FAKE.format(python=sys.executable, sentinel=SENTINEL))
        binary.chmod(0o755)
    env = dict(os.environ, PATH=tmp + os.pathsep + os.environ['PATH'],
               ANTHROPIC_API_KEY='must-be-removed', OPENAI_API_KEY='must-be-removed',
               CLAUDE_CODE_OAUTH_TOKEN='must-be-replaced')
    for provider, token, expected in (('claude', SENTINEL, True),
                                     ('codex', SENTINEL, True),
                                     ('codex', 'reject:' + SENTINEL, False)):
        p = subprocess.run([sys.executable, str(HERE / 'worker.py')],
            input=json.dumps({'provider': provider, 'token': token, 'start_at': time.time()}),
            env=env, text=True, capture_output=True, check=True)
        assert SENTINEL not in p.stdout + p.stderr
        report = json.loads(p.stdout)
        assert bool(report.get('inference_ok')) == expected, report
        assert report['refresh_credential_present'] is False
        if not expected:
            assert report['login_error_kind'] == 'agent_identity_jwt'
print('PASS: synthetic CLI isolation, inference parsing, rejection and evidence redaction')
