#!/usr/bin/env python3
"""Controlled app-server fixture: files release initialization and turns."""

import json
import os
import pathlib
import sys
import threading
import time

if "--version" in sys.argv:
    print("codex-cli 0.158.0")
    sys.exit(0)
root = pathlib.Path(os.environ["KANNA_HOST_TEST_CONTROL"])
lock = threading.Lock()
thread = "11111111-1111-4111-8111-111111111111"
turn = 0
pending = None
assert "KANNA_HOSTED_FRONTEND_CONFIG" not in os.environ, "host capability leaked"


def emit(value):
    with lock:
        print(json.dumps(value), flush=True)


def wait_file(name):
    deadline = time.monotonic() + 60
    while not (root / name).exists():
        if time.monotonic() > deadline:
            os._exit(5)
        time.sleep(0.02)


def complete(number, gate=None):
    if gate:
        wait_file(gate)
    emit(
        {
            "method": "turn/completed",
            "params": {
                "threadId": thread,
                "turn": {"id": str(number), "status": "completed"},
            },
        }
    )


for line in sys.stdin:
    value = json.loads(line)
    method = value.get("method")
    ident = value.get("id")
    if method == "initialize":
        wait_file("initialize")
        emit({"id": ident, "result": {"userAgent": "fixture"}})
    elif method in ("thread/start", "thread/resume"):
        emit(
            {
                "id": ident,
                "result": {
                    "thread": {"id": thread, "turns": []},
                    "model": "fixture",
                    "reasoningEffort": "low",
                },
            }
        )
    elif method == "turn/start":
        turn += 1
        text = "\n".join(x.get("text", "") for x in value["params"]["input"])
        with (root / "turns.jsonl").open("a") as out:
            out.write(json.dumps({"turn": turn, "text": text}) + "\n")
        if text == "UNCERTAIN":
            os._exit(7)
        emit({"id": ident, "result": {"turn": {"id": str(turn)}}})
        emit(
            {
                "method": "turn/started",
                "params": {"threadId": thread, "turn": {"id": str(turn)}},
            }
        )
        if text == "APPROVAL":
            pending = turn
            emit(
                {
                    "id": 900,
                    "method": "item/commandExecution/requestApproval",
                    "params": {
                        "threadId": thread,
                        "turnId": str(turn),
                        "itemId": "tool",
                        "command": "printf fixture",
                        "availableDecisions": ["accept", "decline", "cancel"],
                    },
                }
            )
        else:
            gate = "release-initial" if turn == 1 else None
            threading.Thread(target=complete, args=(turn, gate), daemon=True).start()
    elif ident == 900 and "result" in value:
        (root / "decision.json").write_text(json.dumps(value))
        complete(pending)
        pending = None
    elif method == "turn/interrupt":
        emit({"id": ident, "result": {}})
        complete(turn)
