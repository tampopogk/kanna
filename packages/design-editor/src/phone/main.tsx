/**
 * Entry of the phone's design page: bundled into one script and one
 * stylesheet (`pnpm --filter @kanna/design-editor build:phone`) that the
 * mobile app ships inside itself and loads into a WebView with no network
 * origin. Everything the page needs arrives over the app's bridge.
 */
import { createRoot } from "react-dom/client";
import "@blocknote/mantine/style.css";
import "../style.css";
import "./phone.css";
import { DESIGN_SCHEMA_VERSION } from "../schema";
import { DesignSession } from "../session";
import { PageBridge, type DesignBridgeResponse } from "./bridge";
import { ENGLISH_PHONE_LABELS, PhoneDesign, type PhoneLabels } from "./PhoneDesign";

interface StartOptions {
  theme: "light" | "dark";
  labels?: Partial<PhoneLabels>;
}

declare global {
  interface Window {
    ReactNativeWebView?: { postMessage(message: string): void };
    __kannaDesign?: {
      start(options: StartOptions): void;
      receive(message: DesignBridgeResponse | { type: "resume" } | { type: "theme"; theme: "light" | "dark" }): void;
      schemaVersion: string;
    };
  }
}

const bridge = new PageBridge((message) => window.ReactNativeWebView?.postMessage(message));
let session: DesignSession | null = null;
let theme: "light" | "dark" = "light";
let labels: PhoneLabels = ENGLISH_PHONE_LABELS;
const root = createRoot(document.getElementById("root")!);

function render() {
  if (!session) return;
  document.documentElement.dataset.theme = theme;
  root.render(<PhoneDesign session={session} theme={theme} labels={labels} />);
}

window.__kannaDesign = {
  schemaVersion: DESIGN_SCHEMA_VERSION,
  start(options) {
    theme = options.theme;
    labels = {
      ...ENGLISH_PHONE_LABELS,
      ...options.labels,
      delivery: { ...ENGLISH_PHONE_LABELS.delivery, ...options.labels?.delivery },
    };
    session?.close();
    session = new DesignSession(bridge.transport(), {
      schemaVersion: DESIGN_SCHEMA_VERSION,
      // Short enough for a relayed request to answer well inside its timeout.
      pollTimeoutMs: 8_000,
    });
    const started = session;
    session.subscribe(() => undefined);
    void started.start().catch(() => undefined).finally(render);
  },
  receive(message) {
    if (message.type === "response") bridge.receive(message);
    else if (message.type === "theme") {
      theme = message.theme;
      render();
    } else if (message.type === "resume" && session) {
      // Back from the background: catch up on whatever changed meanwhile.
      void session.sync().catch(() => undefined);
      void session.refreshView();
    }
  },
};
window.ReactNativeWebView?.postMessage(JSON.stringify({ kind: "kanna-design", type: "ready" }));
