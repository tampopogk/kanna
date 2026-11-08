#!/usr/bin/env python3
import argparse
import json
import os
import shlex
import shutil
from pathlib import Path


def toml_value(v):
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, list):
        return "[" + ", ".join(toml_value(x) for x in v) + "]"
    return json.dumps(v)


def configure(agent, home, port, trusted, settings=None, ask=False):
    settings = settings or {}
    home.mkdir(parents=True, exist_ok=True)
    env = {"HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config"), "XDG_DATA_HOME": str(home / ".local/share"), "XDG_STATE_HOME": str(home / ".local/state"), "XDG_CACHE_HOME": str(home / ".cache"), "LANG": "en_US.UTF-8"}
    if agent == "claude":
        project = {"hasTrustDialogAccepted": True, "hasCompletedProjectOnboarding": True, "allowedTools": []}
        config = {"hasCompletedOnboarding": True, "lastOnboardingVersion": "2.1.296", "theme": "dark", "numStartups": 5, "bypassPermissionsModeAccepted": True, "customApiKeyResponses": {"approved": ["mock"], "rejected": []}, "projects": {str(t): project for t in trusted}}
        (home / ".claude").mkdir(exist_ok=True)
        (home / ".claude.json").write_text(json.dumps(config, indent=2))
        (home / ".claude/.claude.json").write_text(json.dumps(config, indent=2))
        (home / ".claude/settings.json").write_text(json.dumps({"theme": "dark", "skipDangerousModePermissionPrompt": True, **settings}, indent=2))
        env.update({"ANTHROPIC_BASE_URL": f"http://127.0.0.1:{port}", "ANTHROPIC_API_KEY": "mock", "CLAUDE_CONFIG_DIR": str(home / ".claude"), "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_AUTOUPDATER": "1"})
        return env, ["--model", "mock-model"] + (["--permission-mode", "default"] if ask else [])
    codex_home = home / ".codex"
    codex_home.mkdir(exist_ok=True)
    lines = ['model = "mock-model"', 'model_provider = "mock"'] + (['approval_policy = "on-request"', 'sandbox_mode = "workspace-write"'] if ask else []) + ["check_for_update_on_startup = false", "", "[model_providers.mock]", 'name = "mock"', f'base_url = "http://127.0.0.1:{port}/v1"', 'wire_api = "responses"', 'env_key = "MOCK_API_KEY"', ""]
    for t in trusted:
        lines += [f'[projects."{t}"]', 'trust_level = "trusted"', ""]
    if settings:
        lines += ["[tui]"] + [f"{k} = {toml_value(v)}" for k, v in settings.items()] + [""]
    (codex_home / "config.toml").write_text("\n".join(lines))
    env.update({"CODEX_HOME": str(codex_home), "MOCK_API_KEY": "mock"})
    return env, []


def interpreter_dir(real):
    try:
        with open(real, "rb") as f:
            line = f.readline().decode(errors="replace").split()
    except OSError:
        return None
    if len(line) < 2 or not line[0].endswith("/env") or shutil.which(line[1]):
        return None
    beside = Path(real).parent
    return str(beside) if (beside / line[1]).exists() else None


def wrapper(agent, real, home, port, trusted, path, cli=None):
    env, args = configure(agent, home, port, trusted)
    if cli:
        env["KANNA_CLI"] = cli
    beside = interpreter_dir(real)
    if beside:
        env["PATH"] = beside + ":" + os.environ.get("PATH", "/usr/bin:/bin")
    exports = "".join(f"export {k}={shlex.quote(v)}\n" for k, v in env.items())
    path.parent.mkdir(parents=True, exist_ok=True)
    clear = "for v in $(env | sed -nE 's/^((CLAUDE|CODEX|ANTHROPIC|OPENAI)[A-Z_]*)=.*/\\1/p'); do unset \"$v\"; done\n"
    path.write_text(f"#!/bin/sh\n{clear}{exports}exec {shlex.quote(real)} {' '.join(map(shlex.quote, args))} \"$@\"\n")
    path.chmod(0o755)


def main():
    p = argparse.ArgumentParser(description="Write an executable that runs an agent against the mock model, in a HOME of its own")
    p.add_argument("--agent", choices=["claude", "codex"], required=True)
    p.add_argument("--real", help="the agent's executable (default: the one on PATH)")
    p.add_argument("--home", required=True)
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--trust", action="append", default=[], help="a folder the agent trusts without asking (repeatable)")
    p.add_argument("--out", required=True, help="where to write the executable")
    p.add_argument("--cli", help="the kanna CLI the agent's commands run, as $KANNA_CLI")
    a = p.parse_args()
    real = a.real or shutil.which(a.agent)
    if not real:
        raise SystemExit(f"isolate: {a.agent} isn't installed")
    wrapper(a.agent, real, Path(a.home).resolve(), a.port, a.trust, Path(a.out), a.cli)


if __name__ == "__main__":
    main()
