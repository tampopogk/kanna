#!/usr/bin/env python3
"""Throwaway probe: the ordinary Codex TUI attached to an app-server whose auth a separate holder connection supplies."""
import base64
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import pty
import re
import select
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

MARKER = 'KANNA' + '_TUI_OK'
ASK = 'Reply with the word KANNA immediately followed by _TUI_OK and nothing else. Do not run tools.'


class Socket:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.sock.settimeout(20)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall(f'GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode())
        head = b''
        while b'\r\n\r\n' not in head:
            head += self.sock.recv(1)
        self.sock.settimeout(None)
        if b' 101 ' not in head.split(b'\r\n')[0]:
            raise RuntimeError('no websocket upgrade')
        self.lock = threading.Lock()
        self.buffer = b''

    def send(self, message):
        data = json.dumps(message).encode()
        mask = os.urandom(4)
        header = bytes([0x81])
        n = len(data)
        if n < 126:
            header += bytes([0x80 | n])
        elif n < 65536:
            header += bytes([0x80 | 126]) + struct.pack('>H', n)
        else:
            header += bytes([0x80 | 127]) + struct.pack('>Q', n)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        with self.lock:
            self.sock.sendall(header + mask + masked)

    def exact(self, n):
        while len(self.buffer) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise EOFError
            self.buffer += chunk
        out, self.buffer = self.buffer[:n], self.buffer[n:]
        return out

    def receive(self):
        while True:
            b0, b1 = self.exact(2)
            n = b1 & 0x7f
            if n == 126:
                n = struct.unpack('>H', self.exact(2))[0]
            elif n == 127:
                n = struct.unpack('>Q', self.exact(8))[0]
            payload = self.exact(n)
            opcode = b0 & 0x0f
            if opcode == 0x9:
                with self.lock:
                    self.sock.sendall(bytes([0x8a, 0x80 | len(payload)]) + os.urandom(4) + payload)
                continue
            if opcode == 0x8:
                raise EOFError
            if opcode in (0x1, 0x2):
                return json.loads(payload)


class Holder:
    def __init__(self, path, credentials):
        self.ws = Socket(path)
        self.credentials = credentials
        self.pending = {}
        self.refreshes = []
        self.notes = []
        self.next = 0
        threading.Thread(target=self.read, daemon=True).start()
        self.call('initialize', {'clientInfo': {'name': 'kanna_holder_probe', 'version': '0.1'}, 'capabilities': {'experimentalApi': True}})
        self.ws.send({'method': 'initialized', 'params': {}})

    def read(self):
        while True:
            try:
                message = self.ws.receive()
            except (EOFError, OSError):
                return
            if 'id' in message and 'method' in message:
                if message['method'] == 'account/chatgptAuthTokens/refresh':
                    self.refreshes.append({'at': time.time(), 'reason': message['params'].get('reason'), 'account_matches': message['params'].get('previousAccountId') == self.credentials()['chatgptAccountId']})
                    self.ws.send({'id': message['id'], 'result': self.credentials()})
                else:
                    self.ws.send({'id': message['id'], 'error': {'code': -32601, 'message': 'unsupported'}})
            elif 'id' in message:
                slot = self.pending.pop(message['id'], None)
                if slot:
                    slot.append(message)
                    slot[0].set()
            elif message.get('method') in ('account/updated', 'authRecovery', 'error'):
                self.notes.append(message['method'])

    def call(self, method, params):
        log('call', method)
        self.next += 1
        slot = [threading.Event()]
        self.pending[self.next] = slot
        self.ws.send({'id': self.next, 'method': method, 'params': params})
        if not slot[0].wait(60):
            raise RuntimeError(f'{method} timed out')
        reply = slot[1]
        if 'error' in reply:
            raise RuntimeError(f'{method} failed: {reply["error"].get("message", "")[:200]}')
        return reply['result']


def log(*parts):
    print(time.strftime('%H:%M:%S'), *parts, file=sys.stderr, flush=True)


def invalid(credentials):
    parts = credentials['accessToken'].split('.')
    parts[2] = ('A' if parts[2][0] != 'A' else 'B') + parts[2][1:]
    return {**credentials, 'accessToken': '.'.join(parts)}


ANSI = re.compile(rb'\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|\x1b[()][0-9A-Za-z]|\x1b[=>]')


class Tui:
    def __init__(self, argv, env, cwd):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd)
            os.execvpe(argv[0], argv, env)
        os.set_blocking(self.fd, False)
        import fcntl, termios
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack('HHHH', 50, 160, 0, 0))
        self.raw = b''

    def pump(self, seconds):
        deadline = time.time() + seconds
        while time.time() < deadline:
            ready, _, _ = select.select([self.fd], [], [], 0.2)
            if ready:
                try:
                    chunk = os.read(self.fd, 65536)
                except OSError:
                    return
                if not chunk:
                    return
                self.raw += chunk
                if b'\x1b[6n' in chunk:
                    os.write(self.fd, b'\x1b[1;1R')

    def text(self):
        return ANSI.sub(b'', self.raw).decode(errors='replace')

    def count(self):
        return self.text().count(MARKER)

    def wait_for(self, predicate, seconds):
        deadline = time.time() + seconds
        while time.time() < deadline:
            self.pump(0.5)
            if predicate():
                return True
        return False

    def type(self, text):
        os.write(self.fd, text.encode())
        time.sleep(0.3)
        os.write(self.fd, b'\r')

    def stop(self):
        for signal in (15, 9):
            try:
                os.kill(self.pid, signal)
            except ProcessLookupError:
                return
            deadline = time.time() + 5
            while time.time() < deadline:
                try:
                    if os.waitpid(self.pid, os.WNOHANG)[0]:
                        return
                except ChildProcessError:
                    return
                time.sleep(0.1)


def thread_ids(holder):
    loaded = holder.call('thread/loaded/list', {}).get('data', [])
    real = []
    for thread in loaded:
        read = holder.call('thread/read', {'threadId': thread})['thread']
        if not read.get('ephemeral'):
            real.append(thread)
    return real


def answers(holder, threads):
    count = 0
    for thread in threads:
        turns = holder.call('thread/read', {'threadId': thread, 'includeTurns': True})['thread'].get('turns', [])
        for turn in turns:
            for item in turn.get('items', []):
                if item.get('type') == 'agentMessage' and MARKER in item.get('text', ''):
                    count += 1
    return count


def wait_answers(holder, tui, threads, at_least, seconds):
    deadline = time.time() + seconds
    while time.time() < deadline:
        tui.pump(2)
        found = thread_ids(holder) if not threads else threads
        if found and answers(holder, found) >= at_least:
            return True, found
    return False, threads


def main():
    from probe import holder_token
    from keychain import entry

    def credentials(refresh=False):
        access = holder_token(refresh=refresh)
        tokens = json.loads(entry('codex-holder'))['tokens']
        return {'accessToken': access, 'chatgptAccountId': tokens['account_id']}

    report = {'timestamp': datetime.now(timezone.utc).isoformat(), 'codex': subprocess.run(['codex', '--version'], capture_output=True, text=True).stdout.strip(), 'phases': []}
    log('holder token')
    current = {'value': credentials()}
    log('holder token ok')
    with tempfile.TemporaryDirectory(prefix='kanna-tui-probe-') as home:
        work = Path(home) / 'work'
        work.mkdir()
        sock = str(Path(home) / 'codex.sock')
        env = {'HOME': home, 'CODEX_HOME': home, 'PATH': os.environ['PATH'], 'LANG': 'en_US.UTF-8', 'TERM': 'xterm-256color'}
        server = subprocess.Popen(['codex', 'app-server', '-c', 'approval_policy="never"', '-c', 'sandbox_mode="danger-full-access"', '--listen', f'unix://{sock}'], env=env, cwd=work, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        tui = None
        try:
            deadline = time.time() + 15
            while not os.path.exists(sock):
                if time.time() > deadline:
                    raise RuntimeError('app-server did not listen')
                time.sleep(0.05)
            holder = Holder(sock, lambda: current['value'])
            holder.call('account/login/start', {'type': 'chatgptAuthTokens', **invalid(current['value'])})
            account = holder.call('account/read', {'refreshToken': False})
            report['account_type'] = (account.get('account') or {}).get('type')
            tui = Tui(['codex', '-c', 'approval_policy="never"', '-c', 'sandbox_mode="danger-full-access"', ASK, '--remote', f'unix://{sock}'], env, str(work))
            log('tui started')
            first, threads = wait_answers(holder, tui, [], 1, 150)
            report['phases'].append({'phase': 'tui_starts_on_injected_invalid_token', 'marker': first, 'renewals': len(holder.refreshes), 'threads': len(threads)})
            tui_pid_before = tui.pid
            current['value'] = credentials(refresh=True)
            holder.call('account/login/start', {'type': 'chatgptAuthTokens', **invalid(current['value'])})
            before = len(holder.refreshes)
            tui.type(ASK)
            second, _ = wait_answers(holder, tui, threads, 2, 150)
            report['phases'].append({'phase': 'same_tui_after_holder_refresh_and_second_invalid_token', 'marker': second, 'renewals': len(holder.refreshes) - before, 'same_process': tui.pid == tui_pid_before, 'same_thread': thread_ids(holder) == threads})
            report['renewal_reasons'] = sorted({r['reason'] for r in holder.refreshes})
            report['renewal_account_matches'] = all(r['account_matches'] for r in holder.refreshes)
            report['worker_login_file'] = (Path(home) / 'auth.json').exists()
            report['notifications'] = sorted(set(holder.notes))
            log('phases done')
            if not (first and second):
                tail = tui.text()[-3000:]
                for secret in (current['value']['accessToken'], current['value']['chatgptAccountId']):
                    tail = tail.replace(secret, '[redacted]')
                report['tui_tail'] = tail
        finally:
            if tui:
                tui.stop()
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
    report['passed'] = all(p['marker'] for p in report['phases']) and all(p['renewals'] >= 1 for p in report['phases']) and not report['worker_login_file'] and report['phases'][1]['same_process'] and report['phases'][1]['same_thread']
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
