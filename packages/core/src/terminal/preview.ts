/**
 * Drop the escape sequences a PTY or a colourising CLI wrote into captured
 * bytes. Shared because two surfaces present recorded terminal text without a
 * terminal to interpret it: the sidebar preview line and the stored
 * workspace-setup transcript.
 */
export function stripAnsi(value: string): string {
  return value.replace(/\u001b\[[0-9;?]*[ -/]*[@-~]/g, "");
}

export function extractPreviewLine(data: Uint8Array): string {
  const text = stripAnsi(new TextDecoder().decode(data));
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  return lines.at(-1) ?? "";
}
