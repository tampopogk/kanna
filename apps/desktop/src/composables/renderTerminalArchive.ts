import { Terminal } from "@xterm/xterm";

/** Parse the retained terminal state locally. No PTY, transport, input handler,
 * fitting or backend resize exists in this renderer. Both buffers remain readable. */
export async function renderTerminalArchive(snapshot: { vt: string; cols: number; rows: number }): Promise<string> {
  const terminal = new Terminal({ cols: snapshot.cols, rows: snapshot.rows, scrollback: Math.max(10_000, snapshot.vt.length), allowProposedApi: true, disableStdin: true });
  try {
    await new Promise<void>(resolve => terminal.write(snapshot.vt, resolve));
    const normal = terminal.buffer.normal;
    const lines: string[] = [];
    for (let i = 0; i < normal.length; i++) lines.push(normal.getLine(i)?.translateToString(true) ?? "");
    if (terminal.buffer.active.type === "alternate") {
      lines.push("", "— Final alternate screen —");
      const alternate = terminal.buffer.alternate;
      for (let i = 0; i < alternate.length; i++) lines.push(alternate.getLine(i)?.translateToString(true) ?? "");
    }
    return lines.join("\n");
  } finally { terminal.dispose(); }
}
