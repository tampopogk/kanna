/**
 * A page for browser tests of the editor (e2e/run-browser.ts): the real
 * editor on the real session, against an in-memory server.
 */
import * as Y from "yjs";
import "@blocknote/mantine/style.css";
import "../style.css";
import { mountDesignEditor } from "../mount";
import { DESIGN_SCHEMA_VERSION, DOCUMENT_FRAGMENT } from "../schema";
import { DesignSession } from "../session";
import { MemoryDesignServer, setMemorySchema } from "./memoryServer";

setMemorySchema(DESIGN_SCHEMA_VERSION);
const server = new MemoryDesignServer();
// The server seeds one empty paragraph, as kanna-server does.
{
  const group = new Y.XmlElement("blockGroup");
  const container = new Y.XmlElement("blockContainer");
  container.setAttribute("id", "first-block");
  const paragraph = new Y.XmlElement("paragraph");
  paragraph.setAttribute("backgroundColor", "default");
  paragraph.setAttribute("textColor", "default");
  paragraph.setAttribute("textAlignment", "left");
  container.insert(0, [paragraph]);
  group.insert(0, [container]);
  server.doc.getXmlFragment(DOCUMENT_FRAGMENT).insert(0, [group]);
}
const session = new DesignSession(server.transport(), { schemaVersion: DESIGN_SCHEMA_VERSION, flushDelayMs: 20 });
const notices: string[] = [];
declare global {
  interface Window {
    __harness: {
      ready: boolean;
      created: () => unknown[];
      notices: string[];
      serverText: () => string;
    };
  }
}
window.__harness = {
  ready: false,
  created: () => JSON.parse(JSON.stringify(server.created)),
  notices,
  serverText: () => server.doc.getXmlFragment(DOCUMENT_FRAGMENT).toString(),
};
void session.start().then(() => {
  mountDesignEditor(document.getElementById("root")!, {
    session,
    theme: "light",
    editable: true,
    onNotice: (message) => notices.push(message),
  });
  window.__harness.ready = true;
});
