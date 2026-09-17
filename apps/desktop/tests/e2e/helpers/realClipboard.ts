/**
 * The desktop's real clipboard, written from outside the app.
 *
 * A paste test that seeds the clipboard through the app is testing a loop it
 * owns both ends of. What the owner does is copy in some other window and
 * press the chord here, so the selection has to be put on the *desktop's*
 * clipboard by something that is not Kanna — `wl-copy`, which owns it as an
 * ordinary Wayland client, exactly as a browser or a text editor would.
 *
 * The app reads it back through `arboard`'s X11 backend, bridged to that
 * Wayland selection by Xwayland (see `read_clipboard_text` in
 * `commands/fs.rs`). So this helper also exercises the bridge, which is the
 * part no unit test can reach.
 */
import { execFile, spawn } from "node:child_process";
import { promisify } from "node:util";

const run = promisify(execFile);

export interface RealClipboardStatus {
  usable: boolean;
  /** Why not, in a sentence a person reading a failed lane can act on. */
  reason: string;
}

/** Whether this host can put a selection on the desktop clipboard at all. */
export async function inspectRealClipboard(): Promise<RealClipboardStatus> {
  if (process.platform !== "linux") {
    return { usable: false, reason: `the real clipboard lane is Linux-only; this host is ${process.platform}` };
  }
  if (!process.env.WAYLAND_DISPLAY) {
    return {
      usable: false,
      reason: "WAYLAND_DISPLAY is unset, so wl-copy has no compositor to own the selection on",
    };
  }
  try {
    await run("wl-copy", ["--version"], { env: process.env, timeout: 10_000 });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return { usable: false, reason: "wl-copy is not on PATH (apt install wl-clipboard)" };
    }
    const detail = error instanceof Error ? error.message : String(error);
    return { usable: false, reason: `wl-copy is installed and refused a version check (${detail.split("\n")[0]})` };
  }
  return { usable: true, reason: "" };
}

/**
 * Put `text` on the desktop clipboard as plain UTF-8 text.
 *
 * A Wayland selection is served by whoever owns it, so `wl-copy` forks a
 * server that holds the data and the parent returns. Wait on `exit`, never on
 * `close`: the fork inherits the stderr pipe and keeps it open for as long as
 * it owns the clipboard, and `close` waits for every stdio stream to end — so
 * it never fires and the caller hangs until the test times out, which is
 * exactly what this did on the first run. The server is detached and unref'd
 * for the same reason: it has to outlive this call, and Node must not wait for
 * it at exit.
 *
 * The parent exiting means the selection was offered, not that the compositor
 * has published it yet, so callers should read it back rather than assume.
 */
export async function writeRealClipboard(text: string): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const child = spawn("wl-copy", ["--type", "text/plain;charset=utf-8"], {
      env: process.env,
      stdio: ["pipe", "ignore", "pipe"],
      detached: true,
    });
    let stderr = "";
    child.stderr.on("data", (chunk) => {
      stderr += String(chunk);
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      child.stderr.destroy();
      child.unref();
      if (code === 0) resolve();
      else reject(new Error(`wl-copy exited ${code}: ${stderr.trim() || "no output"}`));
    });
    child.stdin.end(text);
  });
}
