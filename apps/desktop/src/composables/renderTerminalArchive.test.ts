// @vitest-environment happy-dom
import { expect, it } from "vitest";
import { renderTerminalArchive } from "./renderTerminalArchive";
it("renders all retained primary and alternate history above 256 KiB", async () => {
  const vt = "FIRST\r\n" + Array.from({length:5000},(_,i)=>`${i} café ${"x".repeat(70)}\r\n`).join("") + "LAST\r\n\x1b[?1049h\x1b[HFINAL_ALT";
  expect(vt.length).toBeGreaterThan(256*1024);
  const rendered=await renderTerminalArchive({vt,cols:100,rows:24});
  expect(rendered).toContain("FIRST");expect(rendered).toContain("LAST");expect(rendered).toContain("FINAL_ALT");expect(rendered).toContain("4999 café");
});
