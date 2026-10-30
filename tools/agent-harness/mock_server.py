#!/usr/bin/env python3
import argparse
import json
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SCENARIO = {}
LOG_PATH = None
LOCK = threading.Lock()


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return " ".join(str(p["text"]) for p in content if isinstance(p, dict) and "text" in p and not str(p["text"]).lstrip().startswith("<system-reminder>"))
    return ""


def user_texts(body):
    items = body.get("messages") or body.get("input") or []
    if isinstance(items, str):
        return [items]
    return [text_of(i.get("content")) for i in items if isinstance(i, dict) and i.get("role") == "user"]


def last_user_text(body):
    texts = [t for t in user_texts(body) if t.strip()]
    return texts[-1] if texts else ""


def items_of(body):
    items = body.get("messages") or body.get("input") or []
    return items if isinstance(items, list) else []


def is_tool_output(item):
    if not isinstance(item, dict):
        return False
    if item.get("role") == "tool" or item.get("type") in ("function_call_output", "custom_tool_call_output"):
        return True
    content = item.get("content")
    return isinstance(content, list) and any(isinstance(p, dict) and p.get("type") == "tool_result" for p in content)


def tool_answered(body, match):
    asked, answered = -1, -1
    for index, item in enumerate(items_of(body)):
        if is_tool_output(item):
            answered = index
        elif isinstance(item, dict) and item.get("role") == "user" and match in text_of(item.get("content")):
            asked = index
    return answered > asked


def tool_names(body):
    names = []
    for tool in body.get("tools", []) or []:
        if isinstance(tool, dict):
            names.append(tool.get("name") or (tool.get("function") or {}).get("name"))
    return [n for n in names if n]


def turn_for(body):
    prompt = last_user_text(body)
    for rule in SCENARIO["rules"]:
        if rule["match"] in prompt:
            if rule.get("tool") and tool_names(body) and not tool_answered(body, rule["match"]):
                return {"kind": "tool", "command": rule["tool"]}
            return {"kind": "text", "text": "\n".join(rule.get("after_tool" if rule.get("tool") else "text", rule.get("text", [])))}
    return {"kind": "text", "text": SCENARIO["fallback"]}


def codex_call(body, command):
    names = tool_names(body)
    escalate = {"sandbox_permissions": "require_escalated", "justification": "Create the probe file outside the sandbox"}
    if "exec_command" in names:
        return "exec_command", json.dumps({"cmd": command, **escalate})
    if "shell_command" in names:
        return "shell_command", json.dumps({"command": command, **escalate})
    return "shell", json.dumps({"command": ["bash", "-lc", command], **escalate})


def chunks(text):
    buf = ""
    for word in text.split(" "):
        buf = f"{buf} {word}" if buf else word
        if len(buf) >= 30:
            yield buf
            buf = ""
    if buf:
        yield buf


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def log(self, body, turn=None):
        entry = {"time": time.time(), "path": self.path, "model": body.get("model"), "stream": body.get("stream"), "prompt": last_user_text(body), "history": user_texts(body), "tools": tool_names(body)[:40], "turn": turn}
        with LOCK, LOG_PATH.open("a") as f:
            f.write(json.dumps(entry) + "\n")

    def send_json(self, obj, status=200):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def sse(self, events, named=True):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            for item in events:
                if isinstance(item, float):
                    time.sleep(item)
                    continue
                event, payload = item
                line = (f"event: {event}\n" if named else "") + f"data: {json.dumps(payload)}\n\n"
                self.wfile.write(line.encode())
                self.wfile.flush()
            if not named:
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        if self.path in ("/", "/health"):
            return self.send_json({"ok": True})
        self.log({})
        if self.path.split("?")[0].endswith("/models"):
            return self.send_json({"object": "list", "data": [], "models": []})
        self.send_json({"ok": True})

    def do_POST(self):
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))) or b"{}")
        except Exception:
            body = {}
        route = self.path.split("?", 1)[0]
        if route.endswith("/count_tokens"):
            self.log(body)
            return self.send_json({"input_tokens": 10})
        turn = turn_for(body)
        self.log(body, turn if tool_names(body) else None)
        if route == "/v1/messages":
            return self.anthropic(body, turn)
        if route == "/v1/responses":
            return self.responses_api(body, turn)
        if route == "/v1/chat/completions":
            return self.chat(body, turn)
        self.send_json({"ok": True, "data": []})

    def text_stream(self, text, make):
        out = [SCENARIO["think_seconds"]]
        for piece in chunks(text):
            out.append(make(piece))
            out.append(SCENARIO["chunk_seconds"])
        return out

    def anthropic(self, body, turn):
        mid = "msg_" + uuid.uuid4().hex[:12]
        model = body.get("model", "mock-model")
        if turn["kind"] == "tool":
            block = {"type": "tool_use", "id": "toolu_" + uuid.uuid4().hex[:12], "name": "Bash", "input": {"command": turn["command"], "description": "Create the probe file"}}
            content, stop = [block], "tool_use"
        else:
            content, stop = [{"type": "text", "text": turn["text"]}], "end_turn"
        if not body.get("stream"):
            return self.send_json({"id": mid, "type": "message", "role": "assistant", "model": model, "content": content, "stop_reason": stop, "stop_sequence": None, "usage": {"input_tokens": 10, "output_tokens": 10}})
        events = [("message_start", {"type": "message_start", "message": {"id": mid, "type": "message", "role": "assistant", "model": model, "content": [], "stop_reason": None, "stop_sequence": None, "usage": {"input_tokens": 10, "output_tokens": 1}}})]
        if turn["kind"] == "tool":
            events += [SCENARIO["think_seconds"], ("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {**block, "input": {}}}), ("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": json.dumps(block["input"])}})]
        else:
            events.append(("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}))
            events += self.text_stream(turn["text"], lambda t: ("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": t}}))
        events += [("content_block_stop", {"type": "content_block_stop", "index": 0}), ("message_delta", {"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": None}, "usage": {"output_tokens": 10}}), ("message_stop", {"type": "message_stop"})]
        self.sse(events)

    def responses_api(self, body, turn):
        rid = "resp_" + uuid.uuid4().hex[:12]
        model = body.get("model", "mock-model")
        usage = {"input_tokens": 10, "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 10, "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 20}
        if turn["kind"] == "tool":
            name, args = codex_call(body, turn["command"])
            item = {"type": "function_call", "id": "fc_" + uuid.uuid4().hex[:12], "call_id": "call_" + uuid.uuid4().hex[:12], "name": name, "arguments": args, "status": "completed"}
        else:
            item = {"type": "message", "id": "msg_" + uuid.uuid4().hex[:12], "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": turn["text"], "annotations": []}]}
        if not body.get("stream"):
            return self.send_json({"id": rid, "object": "response", "status": "completed", "model": model, "output": [item], "usage": usage})
        events = [("response.created", {"type": "response.created", "response": {"id": rid, "object": "response", "status": "in_progress", "model": model}})]
        if turn["kind"] == "tool":
            events += [SCENARIO["think_seconds"], ("response.output_item.added", {"type": "response.output_item.added", "output_index": 0, "item": {**item, "arguments": ""}}), ("response.function_call_arguments.done", {"type": "response.function_call_arguments.done", "item_id": item["id"], "output_index": 0, "arguments": item["arguments"]})]
        else:
            events.append(("response.output_item.added", {"type": "response.output_item.added", "output_index": 0, "item": {**item, "content": []}}))
            events += self.text_stream(turn["text"], lambda t: ("response.output_text.delta", {"type": "response.output_text.delta", "item_id": item["id"], "output_index": 0, "content_index": 0, "delta": t}))
        events += [("response.output_item.done", {"type": "response.output_item.done", "output_index": 0, "item": item}), ("response.completed", {"type": "response.completed", "response": {"id": rid, "object": "response", "status": "completed", "model": model, "output": [item], "usage": usage}})]
        self.sse(events)

    def chat(self, body, turn):
        cid = "chatcmpl-" + uuid.uuid4().hex[:12]
        def chunk(delta, finish=None):
            return ("", {"id": cid, "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
        if turn["kind"] == "tool":
            call = {"id": "call_" + uuid.uuid4().hex[:12], "type": "function", "function": {"name": "bash", "arguments": json.dumps({"command": turn["command"]})}}
            if not body.get("stream"):
                return self.send_json({"id": cid, "object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "content": None, "tool_calls": [call]}, "finish_reason": "tool_calls"}]})
            events = [chunk({"role": "assistant"}), SCENARIO["think_seconds"], chunk({"tool_calls": [{"index": 0, **call}]}), chunk({}, "tool_calls")]
        else:
            if not body.get("stream"):
                return self.send_json({"id": cid, "object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "content": turn["text"]}, "finish_reason": "stop"}]})
            events = [chunk({"role": "assistant"})] + self.text_stream(turn["text"], lambda t: chunk({"content": t})) + [chunk({}, "stop")]
        self.sse(events, named=False)


def main():
    global LOG_PATH, SCENARIO
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--log", required=True)
    parser.add_argument("--scenario", default=str(ROOT / "scenarios/status.json"))
    args = parser.parse_args()
    SCENARIO = json.loads(Path(args.scenario).read_text())
    LOG_PATH = Path(args.log)
    LOG_PATH.parent.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(server.server_address[1], flush=True)
    try:
        server.serve_forever()
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
