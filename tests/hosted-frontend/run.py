#!/usr/bin/env python3
"""Real server -> daemon -> PTY frontend -> controlled provider acceptance.
Run against a task-owned `./kd dev up --db ...` instance, never production.
Usage: python3 tests/hosted-frontend/run.py http://127.0.0.1:<reserved-port>
"""

import concurrent.futures
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

base = sys.argv[1].rstrip("/")
workspace = pathlib.Path(__file__).resolve().parents[2]
(workspace / ".tmp").mkdir(exist_ok=True)
root = pathlib.Path(tempfile.mkdtemp(prefix="hosted-contract-", dir=workspace / ".tmp"))
repo = root / "repo"
control = root / "control"
repo.mkdir()
control.mkdir()
task = None


def api(method, path, body=None):
    req = urllib.request.Request(
        base + path,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
        method=method,
    )
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            return r.status, json.loads(r.read() or "null")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read())


def poll(check, description):
    until = time.monotonic() + 40
    while time.monotonic() < until:
        value = check()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError("timed out: " + description)


def git(*args):
    return subprocess.run(
        ["git", "-C", str(repo), *args], check=True, capture_output=True, text=True
    ).stdout.strip()


def detail():
    return api("GET", "/v1/tasks/" + task)[1]


def deliveries():
    return api("GET", "/v1/tasks/" + task + "/input-deliveries")[1]["deliveries"]


def raw(body):
    status, result = api(
        "POST", "/v1/tasks/" + task + "/raw-input", dict(body, source="manager")
    )
    assert status == 200, result


def send(text, ident=None):
    ident = ident or str(uuid.uuid4())
    for _ in range(100):
        status, result = api(
            "POST",
            "/v1/tasks/" + task + "/input",
            {"input": text, "deliveryId": ident, "source": "manager"},
        )
        if status != 409 or result.get("reason") != "no_live_agent_session":
            break
        # An explicitly unaccepted mutation-lease collision is safe to retry
        # with the same id. No accepted or ambiguous outcome is replayed.
        time.sleep(0.05)
    assert status == 202, (status, result)
    return result


def turns():
    p = control / "turns.jsonl"
    return (
        [json.loads(line) for line in p.read_text().splitlines()] if p.exists() else []
    )


try:
    assert api("GET", "/v1/status")[1]["environment"] == "development", (
        "requires an isolated development server"
    )
    (repo / ".kanna/bin").mkdir(parents=True)
    (repo / ".kanna/agents/fixture").mkdir(parents=True)
    (repo / ".kanna/workflows").mkdir()
    fake = repo / ".kanna/bin/codex"
    shutil.copyfile(pathlib.Path(__file__).with_name("fake_codex.py"), fake)
    fake.chmod(0o755)
    (repo / ".kanna/config.json").write_text(
        json.dumps(
            {
                "workflow": "fixture",
                "agentFrontends": {"codex": "agent-tui"},
                "workspace": {
                    "path": {"prepend": ["./.kanna/bin"]},
                    "env": {"KANNA_HOST_TEST_CONTROL": str(control)},
                },
            }
        )
    )
    (repo / ".kanna/agents/fixture/AGENT.md").write_text(
        "---\nname: fixture\nrole: Protocol fixture\nproviders: codex\nagent_provider: codex\n---\nFixture.\n"
    )
    (repo / ".kanna/workflows/fixture.json").write_text(
        json.dumps(
            {
                "name": "fixture",
                "stages": [
                    {
                        "name": "in progress",
                        "agent": "fixture",
                        "prompt": "$TASK_PROMPT",
                        "policy": {"transition": "manual"},
                    }
                ],
            }
        )
    )
    git("init", "-b", "main")
    git("add", ".")
    git(
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-m",
        "Create hosted protocol fixture",
    )
    subprocess.run(
        ["git", "clone", "--bare", str(repo), str(root / "origin.git")],
        check=True,
        capture_output=True,
    )
    git("remote", "add", "origin", str(root / "origin.git"))
    git("fetch", "origin")
    git("remote", "set-head", "origin", "main")
    status, registered = api(
        "POST", "/v1/repos", {"path": str(repo), "name": "Hosted protocol contract"}
    )
    assert status in (200, 201), registered
    status, created = api(
        "POST",
        "/v1/tasks",
        {
            "repoId": registered["id"],
            "prompt": "INITIAL",
            "workflowName": "fixture",
            "agent": "fixture",
            "agentProvider": "codex",
            "model": "fixture",
            "effort": "low",
        },
    )
    assert status in (200, 201), created
    task = created["taskId"]
    poll(lambda: detail().get("runtimeState") == "busy", "starting busy")
    raw({"bytes": b"DRAFT".hex()})
    poll(
        lambda: detail().get("composer", {}).get("text") == "DRAFT", "draft attestation"
    )
    a = send("BEFORE-INITIALIZATION")
    assert a["state"] == "queued"
    assert turns() == []
    (control / "initialize").touch()
    poll(lambda: len(turns()) == 1, "initial turn first")
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        accepted = list(pool.map(lambda n: send("CONCURRENT-" + str(n)), range(6)))
    retry = send("BEFORE-INITIALIZATION", a["id"])
    assert retry["id"] == a["id"]
    status, _ = api(
        "POST",
        "/v1/tasks/" + task + "/input",
        {"input": "changed", "deliveryId": a["id"]},
    )
    assert status == 409
    assert len(turns()) == 1
    assert detail()["composer"]["text"] == "DRAFT"
    (control / "release-initial").touch()
    poll(
        lambda: (
            len(turns()) == 8 and all(d["state"] == "submitted" for d in deliveries())
        ),
        "ordered provider receipts",
    )
    receipts = sorted(deliveries(), key=lambda d: d["sequence"])
    assert [t["text"] for t in turns()[1:]] == [d["message"] for d in receipts[1:]]
    assert api("GET", "/v1/tasks/" + task + "/inputs")[1]["total"] == 7
    send("APPROVAL")
    poll(lambda: detail().get("runtimeState") == "waiting", "approval waiting")
    queued = send("AFTER-APPROVAL")
    assert queued["state"] == "queued"
    assert not (control / "decision.json").exists()
    assert detail()["composer"]["text"] == "DRAFT"
    raw({"keys": ["enter"]})
    assert not (control / "decision.json").exists()
    raw({"keys": ["tab", "right", "enter"]})
    poll(lambda: (control / "decision.json").exists(), "explicit deny")
    poll(
        lambda: any(t["text"] == "AFTER-APPROVAL" for t in turns()),
        "queue after approval",
    )
    assert detail()["composer"]["text"] == "DRAFT"
    uncertain = send("UNCERTAIN")
    poll(
        lambda: any(
            d["id"] == uncertain["id"] and d["state"] == "uncertain"
            for d in deliveries()
        ),
        "provider crash gap",
    )
    assert sum(t["text"] == "UNCERTAIN" for t in turns()) == 1
    assert not any(
        i["message"] == "UNCERTAIN"
        for i in api("GET", "/v1/tasks/" + task + "/inputs")[1]["inputs"]
    )
    (root / "evidence.json").write_text(
        json.dumps(
            {"task": task, "turns": turns(), "deliveries": deliveries()}, indent=2
        )
    )
    api("POST", "/v1/tasks/" + task + "/actions/close", {})
    status, created = api(
        "POST",
        "/v1/tasks",
        {
            "repoId": registered["id"],
            "prompt": "INITIAL",
            "workflowName": "fixture",
            "agent": "fixture",
            "agentProvider": "codex",
            "model": "fixture",
            "effort": "low",
        },
    )
    assert status in (200, 201), created
    task = created["taskId"]
    poll(lambda: detail().get("runtimeState") == "idle", "second fixture ready")
    send("APPROVAL")
    poll(lambda: detail().get("runtimeState") == "waiting", "bounded queue approval")
    for n in range(64):
        send("BOUND-" + str(n))
    status, rejected = api(
        "POST",
        "/v1/tasks/" + task + "/input",
        {"input": "OVER-BOUND", "deliveryId": str(uuid.uuid4())},
    )
    assert status == 409, (status, rejected)
    assert detail()["runtimeState"] == "waiting"
    api("POST", "/v1/tasks/" + task + "/actions/close", {})
    poll(
        lambda: all(
            d["state"] in ("submitted", "failed", "uncertain") for d in deliveries()
        ),
        "retirement records every pending delivery",
    )
    assert (
        len(
            [
                d
                for d in deliveries()
                if d["message"].startswith("BOUND-") and d["state"] == "failed"
            ]
        )
        == 64
    )
    (root / "retirement.json").write_text(json.dumps(deliveries(), indent=2))
    print(
        "PASS: queue bound/retirement, delayed initialization, FIFO/concurrent inputs, idempotency, draft/card isolation, deny, confirmed ledger, crash uncertainty; evidence "
        + str(root / "evidence.json")
    )
finally:
    if task:
        api("POST", "/v1/tasks/" + task + "/actions/close", {})
