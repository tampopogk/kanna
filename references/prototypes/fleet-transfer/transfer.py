#!/usr/bin/env python3
"""Disposable stopped-task transfer model. Only operates on fixtures it creates."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import uuid


def run(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, stderr=subprocess.PIPE).decode().strip()


def git(root, *args):
    return run("git", "-C", str(root), *args)


def read(path):
    return json.loads(Path(path).read_text())


def write(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(".tmp")
    with temp.open("w") as f:
        json.dump(value, f, indent=2)
        f.write("\n")
        f.flush()
        os.fsync(f.fileno())
    os.replace(temp, path)
    fd = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def checkpoint(root, state, phase, stop):
    state["phase"] = phase
    write(root / "state.json", state)
    if phase == stop:
        # A new process must recover exclusively from disk.
        os._exit(75)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def agent_home(agent):
    variable = "CLAUDE_CONFIG_DIR" if agent == "claude" else "CODEX_HOME"
    return Path(os.environ.get(variable, str(Path.home() / ("." + agent))))


def transcript(agent, session):
    home = agent_home(agent)
    matches = list((home / ("projects" if agent == "claude" else "sessions")).rglob(
        session + ".jsonl" if agent == "claude" else "*" + session + ".jsonl"))
    if len(matches) != 1:
        raise RuntimeError(f"Expected one {agent} transcript for {session}, got {matches}")
    path = matches[0]
    records = [json.loads(line) for line in path.read_text().splitlines() if line]
    if agent == "claude":
        assert any(r.get("sessionId") == session and r.get("type") == "user" for r in records)
        assert any(r.get("type") == "assistant" for r in records)
    else:
        assert records[0]["type"] == "session_meta"
        assert records[0]["payload"]["id"] == session
        assert any(r.get("type") == "response_item" for r in records)
    return path


def invoke(agent, cwd, prompt, log, session=None, with_tools=False):
    executable = shutil.which(agent) or str(Path.home() / ".local/bin" / agent)
    if agent == "claude":
        args = [executable, "-p", "--output-format", "json", "--tools", "Bash" if with_tools else "",
                "--strict-mcp-config", "--setting-sources", ""]
        if with_tools:
            args += ["--allowedTools", "Bash", "--permission-mode", "dontAsk"]
        if session:
            args += ["--resume", session]
    else:
        args = [executable, "exec", "--ignore-user-config", "--ignore-rules",
                "--sandbox", "read-only", "--json"]
        if session:
            args += ["resume", session]
    env = dict(os.environ)
    env.pop("CLAUDECODE", None)
    # No secrets are read or copied by the model: each CLI uses this machine's login.
    with Path(log).open("w") as out, Path(str(log) + ".stderr").open("w") as err:
        result = subprocess.run(args + [prompt], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                stdout=out, stderr=err, timeout=240)
    if result.returncode:
        raise RuntimeError(f"{agent} failed ({result.returncode}); inspect {log} and its .stderr locally")
    if agent == "codex" and not with_tools:
        events = [json.loads(line) for line in Path(log).read_text().splitlines()]
        assert all(e["item"]["type"] in ("agent_message", "reasoning") for e in events
                   if e["type"] in ("item.started", "item.completed")), "Memory probe used a tool"
    return answer(agent, log)


def answer(agent, log):
    records = [json.loads(line) for line in Path(log).read_text().splitlines() if line]
    if agent == "claude":
        result = records[-1]
        assert result["type"] == "result" and not result.get("is_error")
        return result["session_id"], result["result"].strip()
    assert any(r["type"] == "turn.completed" for r in records), "No completed turn"
    session = next(r["thread_id"] for r in records if r["type"] == "thread.started")
    messages = [r["item"]["text"] for r in records if r["type"] == "item.completed"
                and r["item"]["type"] == "agent_message"]
    assert messages
    return session, messages[-1].strip()


def init(root, agent, synthetic=False):
    root.mkdir(parents=True, exist_ok=False)
    repo = root / "work"
    repo.mkdir()
    git(repo, "init", "-b", "task")
    git(repo, "config", "user.name", "Fleet model")
    git(repo, "config", "user.email", "fleet-model@example.invalid")
    (repo / "tracked.txt").write_text("base\n")
    (repo / "deleted.txt").write_text("delete me\n")
    (repo / ".gitignore").write_text("ignored.txt\n")
    git(repo, "add", ".")
    git(repo, "commit", "-m", "Fixture base")
    base = git(repo, "rev-parse", "HEAD")
    git(repo, "bundle", "create", str(root / "base.bundle"), "task")
    # A task commit unknown to the receiver must travel along with the WIP.
    (repo / "task.txt").write_text("committed task work\n")
    git(repo, "add", ".")
    git(repo, "commit", "-m", "Task work")
    before = git(repo, "rev-parse", "HEAD")
    (repo / "tracked.txt").write_text("staged\n")
    git(repo, "add", ".")
    (repo / "tracked.txt").write_text("staged plus unstaged\n")
    (repo / "deleted.txt").unlink()
    (repo / "binary.bin").write_bytes(bytes(range(256)))
    (repo / "new file.txt").write_text("untracked\n")
    (repo / "executable").write_text("#!/bin/sh\necho fixture\n")
    (repo / "executable").chmod(0o755)
    (repo / "link").symlink_to("tracked.txt")
    (repo / "ignored.txt").write_text("not transported\n")
    state = dict(agent=agent, base=base, before=before, phase="created", synthetic=synthetic,
                 transfer_id=str(uuid.uuid4()), nonce="fleet-" + uuid.uuid4().hex)
    write(root / "state.json", state)


def seed(root, stop):
    state = read(root / "state.json")
    if state["phase"] == "stopped":
        return
    assert state["phase"] == "created", "Seed is not replayed after an uncertain start"
    checkpoint(root, state, "seeding", stop)
    if state["synthetic"]:
        session = str(uuid.uuid4())
        path = root / "synthetic.jsonl"
        path.write_text(json.dumps({"sessionId": session, "type": "user",
                                    "message": state["nonce"]}) + "\n" +
                        json.dumps({"sessionId": session, "type": "assistant", "message": "OK"}) + "\n")
    else:
        session, _ = invoke(state["agent"], root / "work",
                            "Remember this exact private phrase for our next turn: " + state["nonce"] +
                            ". Reply only STORED. Do not use tools or write it to any file.",
                            root / "seed.jsonl")
        path = transcript(state["agent"], session)
    # The noninteractive agent has exited normally at the end of its turn.
    state.update(session=session, transcript=str(path), transcript_sha256=digest(path))
    checkpoint(root, state, "stopped", stop)


def pack(root, stop):
    state = read(root / "state.json")
    assert state["phase"] in ("stopped", "prepared", "packed"), "Only a stopped task may move"
    repo = root / "work"
    if state["phase"] == "stopped":
        assert digest(state["transcript"]) == state["transcript_sha256"], "Transcript changed since stop"
        git(repo, "add", "-A")
        tree = git(repo, "write-tree")
        wip = git(repo, "commit-tree", tree, "-p", state["before"], "-m", "Fleet model WIP")
        state.update(wip=wip, tree=tree)
        # Persist the exact commit before touching the branch; retry cannot add a second WIP.
        checkpoint(root, state, "prepared", stop)
    if state["phase"] == "prepared":
        head = git(repo, "rev-parse", "HEAD")
        assert head in (state["before"], state["wip"])
        git(repo, "update-ref", "refs/heads/task", state["wip"], head)
        git(repo, "bundle", "create", str(root / "task.bundle"), "task", "^" + state["base"])
        shutil.copyfile(state["transcript"], root / "transcript.jsonl")
        manifest = {k: state[k] for k in ("agent", "base", "before", "wip", "tree", "session",
                    "transfer_id", "synthetic", "transcript_sha256")}
        manifest.update(bundle_sha256=digest(root / "task.bundle"),
                        transcript_name=Path(state["transcript"]).name,
                        source_home=str(Path.home()), source_work=str(repo))
        write(root / "manifest.json", manifest)
        checkpoint(root, state, "packed", stop)


def receive(root, stop):
    manifest = read(root / "manifest.json")
    state_path = root / "state.json"
    state = read(state_path) if state_path.exists() else dict(phase="received", **manifest)
    assert state["transfer_id"] == manifest["transfer_id"]
    assert digest(root / "task.bundle") == manifest["bundle_sha256"]
    assert digest(root / "transcript.jsonl") == manifest["transcript_sha256"]
    if state["phase"] == "received":
        checkpoint(root, state, "received", stop)
        clone = root / "clone"
        if not clone.exists():
            git(root, "init", "--bare", str(clone))
        git(clone, "fetch", str(root / "base.bundle"), "task:refs/heads/base")
        git(clone, "bundle", "verify", str(root / "task.bundle"))
        git(clone, "fetch", str(root / "task.bundle"), "task:refs/heads/task")
        checkpoint(root, state, "fetched", stop)
    if state["phase"] == "fetched":
        work = root / "work"
        if not work.exists():
            git(root / "clone", "worktree", "add", str(work), "task")
        # Repeatable: HEAD moves back, index and files remain at the WIP tree.
        git(work, "reset", "--soft", state["before"])
        assert git(work, "write-tree") == state["tree"]
        assert not git(work, "diff", "--name-only")
        assert git(work, "diff", "--cached", "--name-only")
        assert not git(work, "remote")
        checkpoint(root, state, "restored", stop)
    if state["phase"] == "restored":
        if state["synthetic"]:
            target = root / "installed-synthetic.jsonl"
        elif state["agent"] == "claude":
            encoded = re.sub(r"[^a-zA-Z0-9]", "-", str(root / "work"))
            target = agent_home("claude") / "projects" / encoded / (state["session"] + ".jsonl")
        else:
            # Rollout files are sufficient; do not transplant state SQLite databases.
            source = Path(state["transcript_name"])
            date = source.name[len("rollout-"):len("rollout-") + 10].split("-")
            assert len(date) == 3
            target = agent_home("codex") / "sessions" / Path(*date) / source
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            assert digest(target) == state["transcript_sha256"], "Refusing to overwrite a changed session"
        else:
            shutil.copyfile(root / "transcript.jsonl", target)
        state["installed_transcript"] = str(target)
        checkpoint(root, state, "installed", stop)


def resume(root, stop):
    state = read(root / "state.json")
    if state["phase"] == "done":
        return
    assert not state["synthetic"], "Synthetic transcripts cannot prove a conversation resumed"
    assert state["phase"] in ("installed", "resuming", "answered")
    log = root / "resume.jsonl"
    if state["phase"] == "installed":
        checkpoint(root, state, "resuming", stop)
        invoke(state["agent"], root / "work",
               "What exact private phrase did I ask you to remember? Reply with only that phrase. "
               "Use conversation memory only; do not use tools or read files.", log, state["session"])
    if state["phase"] == "resuming":
        # A lost reply must not start another turn. Recover a complete output or stop for inspection.
        session, reply = answer(state["agent"], log)
        assert session == state["session"], "Agent forked instead of resuming"
        state["answer_sha256"] = hashlib.sha256(reply.encode()).hexdigest()
        checkpoint(root, state, "answered", stop)
    if state["phase"] == "answered":
        assert digest(state["installed_transcript"]) != state["transcript_sha256"], "Transcript did not advance"
        receipt = {k: state[k] for k in ("transfer_id", "session", "answer_sha256")}
        receipt.update(receiver_home=str(Path.home()), receiver_work=str(root / "work"))
        write(root / "receipt.json", receipt)
        checkpoint(root, state, "done", stop)


def acknowledge(root, stop):
    state = read(root / "state.json")
    assert state["phase"] in ("packed", "done")
    receipt = read(root / "receipt.json")
    assert receipt["transfer_id"] == state["transfer_id"]
    assert receipt["session"] == state["session"]
    assert receipt["answer_sha256"] == hashlib.sha256(state["nonce"].encode()).hexdigest(), "Memory lost"
    # Keep evidence, branch and original transcript even after done. No destructive cleanup.
    checkpoint(root, state, "done", stop)


def check_workdir(root, stop):
    """A separate real turn: remember the conversation AND actually use the new files."""
    state = read(root / "state.json")
    assert state["phase"] == "done" and not state["synthetic"]
    proof = root / "workdir-proof.json"
    if proof.exists():
        assert read(proof)["ok"]
        return
    log = root / "workdir.jsonl"
    marker = root / "workdir-started.json"
    if not marker.exists():
        write(marker, {"started": True})
        invoke(state["agent"], root / "work",
               "Use the shell tool to run pwd and read tracked.txt in your CURRENT working directory. "
               "Do not cd, use absolute paths, or change files. Return only JSON with keys cwd "
               "(the pwd output) and text (the full file contents).", log, state["session"], with_tools=True)
    session, reply = answer(state["agent"], log)
    assert session == state["session"]
    if reply.startswith("```"):
        reply = "\n".join(reply.splitlines()[1:-1])
    result = json.loads(reply)
    assert Path(result["cwd"]).resolve() == (root / "work").resolve(), result
    assert result["text"].rstrip("\n") == "staged plus unstaged", result
    # Require tool output in the transcript, not merely an agent's claim about cwd.
    records = [json.loads(line) for line in Path(state["installed_transcript"]).read_text().splitlines()]
    outputs = []
    for record in records:
        if state["agent"] == "claude":
            content = record.get("message", {}).get("content", [])
            if isinstance(content, list):
                outputs.extend(json.dumps(c) for c in content if c.get("type") == "tool_result")
        elif record.get("type") == "response_item" and record["payload"].get("type") in (
                "function_call_output", "custom_tool_call_output"):
            outputs.append(json.dumps(record["payload"]))
    assert any(str(root / "work") in output for output in outputs), "No tool proved the receiver cwd"
    assert any("staged plus unstaged" in output for output in outputs), "No tool read the transferred edit"
    write(proof, dict(ok=True, session=session, cwd=result["cwd"], text=result["text"]))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("action", choices=["init", "seed", "pack", "receive", "resume", "check-workdir", "ack"])
    p.add_argument("root", type=lambda s: Path(s).expanduser().resolve())
    p.add_argument("--agent", choices=["claude", "codex"], default="claude")
    p.add_argument("--synthetic", action="store_true", help="Git tests only; never a live resume")
    p.add_argument("--stop-after", help="Exit 75 at this durable checkpoint")
    args = p.parse_args()
    if args.action == "init":
        init(args.root, args.agent, args.synthetic)
    else:
        {"seed": seed, "pack": pack, "receive": receive, "resume": resume,
         "check-workdir": check_workdir, "ack": acknowledge}[args.action](
            args.root, args.stop_after)
    print(json.dumps(read(args.root / "state.json")))


if __name__ == "__main__":
    main()
