"""Receives ONE provider's credential on stdin; emits only allowlisted evidence."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def contains_refresh(value):
    if isinstance(value, dict):
        return any((k in ('refresh_token', 'refreshToken') and bool(v)) or contains_refresh(v)
                   for k, v in value.items())
    if isinstance(value, list):
        return any(map(contains_refresh, value))
    return False


def run(request):
    provider = request['provider']
    if provider not in ('claude', 'codex'):
        raise ValueError('unsupported provider')
    token = request.get('token')
    local_login = provider == 'codex' and request.get('credential_mode') == 'per_machine_login'
    with tempfile.TemporaryDirectory(prefix='kanna-auth-probe-') as tmp:
        home = Path(tmp)
        # An allowlist prevents inherited API keys, profiles, hooks and user config
        # from accidentally making this credential probe pass.
        env = {'HOME': tmp, 'PATH': os.environ['PATH'], 'LANG': 'C.UTF-8',
               'CODEX_HOME': str(home / 'codex'),
               'CLAUDE_CONFIG_DIR': str(home / 'claude')}
        Path(env['CODEX_HOME']).mkdir(mode=0o700)
        Path(env['CLAUDE_CONFIG_DIR']).mkdir(mode=0o700)
        if local_login:
            # An independent vendor login made ON THIS VM. Never copied from the holder.
            env['CODEX_HOME'] = '/home/probe/.local/share/agent-sign-in/codex'
        version = subprocess.check_output([provider, '--version'], env=env, text=True).strip()
        result = {'provider': provider, 'version': version, 'login_ok': True, 'inference_ok': False,
                  'refresh_credential_present': False,
                  'credential_mode': 'per_machine_login' if local_login else 'shared_token'}
        if provider == 'codex' and not local_login:
            login = subprocess.run(['codex', '-c', 'cli_auth_credentials_store="file"',
                'login', '--with-access-token'], input=token, env=env, cwd=tmp,
                text=True, capture_output=True, timeout=60)
            result['login_ok'] = login.returncode == 0
            # Never forward arbitrary CLI output: a future version might echo input.
            result['login_error_kind'] = ('agent_identity_jwt' if 'agent identity' in login.stderr.lower()
                                         else 'other' if login.returncode else None)
            auth = Path(env['CODEX_HOME']) / 'auth.json'
            if auth.exists():
                result['refresh_credential_present'] = contains_refresh(json.loads(auth.read_text()))
            if not result['login_ok'] or result['refresh_credential_present']:
                return result
        elif local_login:
            auth = Path(env['CODEX_HOME']) / 'auth.json'
            result['login_ok'] = auth.exists()
            if not auth.exists():
                result['login_error_kind'] = 'missing_local_login'
                return result
            result['refresh_credential_present'] = contains_refresh(json.loads(auth.read_text()))
        else:
            env['CLAUDE_CODE_OAUTH_TOKEN'] = token

        # All workers receive the same future wall-clock start (VM clocks must agree).
        time.sleep(max(0, request['start_at'] - time.time()))
        marker = 'KANNA_AUTH_PROBE_OK'
        prompt = f'Reply with exactly {marker}. Do not use tools.'
        if provider == 'claude':
            args = ['claude', '-p', prompt, '--output-format', 'json',
                    '--tools', '', '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}',
                    '--no-session-persistence']
        else:
            args = ['codex', 'exec', '--skip-git-repo-check', '--ephemeral',
                    '--sandbox', 'read-only', '--json', prompt]
        result['started_at'] = time.time()
        try:
            p = subprocess.run(args, input='', text=True, capture_output=True,
                               env=env, cwd=tmp, timeout=180)
            result['exit_code'] = p.returncode
            if provider == 'claude':
                response = json.loads(p.stdout)
                result['inference_ok'] = (p.returncode == 0 and not response.get('is_error')
                                          and response.get('result', '').strip() == marker)
            else:
                events = [json.loads(line) for line in p.stdout.splitlines() if line.strip()]
                result['inference_ok'] = (p.returncode == 0
                    and any(e.get('type') == 'turn.completed' for e in events)
                    and any(e.get('item', {}).get('type') == 'agent_message'
                            and e['item'].get('text', '').strip() == marker for e in events))
        except (subprocess.TimeoutExpired, json.JSONDecodeError):
            result['inference_ok'] = False
            result['error_kind'] = 'timeout_or_invalid_output'
        result['ended_at'] = time.time()
        return result


if __name__ == '__main__':
    try:
        print(json.dumps(run(json.load(sys.stdin))))
    except Exception as e:
        # Exception text can contain subprocess input or output; expose its type only.
        print(json.dumps({'inference_ok': False, 'error_kind': type(e).__name__}))
        sys.exit(1)
