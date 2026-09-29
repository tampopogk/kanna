import { createRoot, type Root } from "react-dom/client";
import { DesignEditorWhenReady, type DesignEditorProps } from "./DesignEditor";

export interface MountedDesignEditor {
  update(props: Partial<Omit<DesignEditorProps, "session">>): void;
  destroy(): void;
}

/**
 * Mount the design editor into an element owned by a non-React host (the Vue
 * desktop app, the phone's WebView page). The host keeps the session; the
 * editor renders it.
 */
export function mountDesignEditor(element: HTMLElement, initial: DesignEditorProps): MountedDesignEditor {
  let props = initial;
  const root: Root = createRoot(element);
  const render = () => root.render(<DesignEditorWhenReady {...props} />);
  render();
  return {
    update(next) {
      props = { ...props, ...next };
      render();
    },
    destroy() {
      root.unmount();
    },
  };
}
