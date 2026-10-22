#!/usr/bin/env python3
"""Validate raw fleet-net runs and print a comparison table; never invent missing paths."""
import argparse
import json
from pathlib import Path

KINDS = {"file", "port", "sync", "job", "transfer"}


def selected(paths, expected):
    chosen = [p for p in paths if p["selected"]]
    assert len(chosen) == 1 and chosen[0]["kind"] == expected, (expected, chosen)


def summarize(path):
    r = json.loads(path.read_text())
    m = r["measurement"]
    assert "error" not in m, f"{path}: {m}"
    assert r["alpn"] == "kanna/1"
    expected = r["expected"].lower()
    selected(m["settled_paths"], expected)
    selected(r["final_paths"], expected)
    assert {(b["kind"], b["direction"]) for b in m["bulk"]} == {
        (kind, direction) for kind in KINDS for direction in ("push", "pull")
    }
    assert len(m["bulk"]) == 10
    idle = m["echo_idle"]
    assert idle["n"] == len(idle["samples_ms"]) > 0
    for b in m["bulk"]:
        selected(b["paths_before"], expected)
        selected(b["paths_after"], expected)
        assert b["bytes"] > 0 and b["seconds"] > 0
        assert abs(b["mbps"] - b["bytes"] * 8 / b["seconds"] / 1e6) < 1e-8
        assert b["echo_under_load"]["n"] > 0, "no overlapping echo samples; increase --mib"
    def rate(direction):
        runs = [b for b in m["bulk"] if b["direction"] == direction]
        return sum(b["bytes"] for b in runs) * 8 / sum(b["seconds"] for b in runs) / 1e6
    loaded = max(b["echo_under_load"]["p95_ms"] for b in m["bulk"])
    return [r["label"], expected, idle["p50_ms"], idle["p95_ms"], loaded, rate("push"), rate("pull")]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", nargs="*", type=Path)
    args = parser.parse_args()
    paths = args.results or sorted((Path(__file__).parent / "results").glob("*.json"))
    assert paths, "no result files"
    print("| Run | Selected path | Echo p50 ms | Echo p95 ms | Worst loaded p95 ms | Upload Mbps | Download Mbps |")
    print("|---|---|---:|---:|---:|---:|---:|")
    for path in paths:
        row = summarize(path)
        print("| " + " | ".join(f"{x:.2f}" if isinstance(x, float) else str(x) for x in row) + " |")
