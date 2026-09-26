import type { Node as PMNode } from "@tiptap/pm/model";

/**
 * The block projection an agent reads: the same shape kanna-server's Rust
 * adapter returns from `kanna_design_get_document`. Fixture tests compare the
 * browser's view of a document (through y-prosemirror and this function) with
 * the server's view of the same bytes, so the two must stay identical.
 */
export type ProjectedRun = {
  text: string;
  styles: Record<string, string | boolean>;
  href?: string;
  threads?: string[];
};

export type ProjectedBlock = {
  id: string;
  type: string;
  props: Record<string, unknown>;
  text: string;
  content: ProjectedRun[];
  children: ProjectedBlock[];
};

const STYLE_MARKS = new Set([
  "bold",
  "italic",
  "underline",
  "strike",
  "code",
  "textColor",
  "backgroundColor",
]);

function runOf(node: PMNode): ProjectedRun {
  const run: ProjectedRun = { text: node.text ?? "", styles: {} };
  const threads: string[] = [];
  for (const mark of node.marks) {
    const name = mark.type.name;
    if (name === "comment") {
      const threadId = mark.attrs.threadId as string;
      if (threadId) threads.push(threadId);
    } else if (name === "link") {
      run.href = mark.attrs.href as string;
    } else if (STYLE_MARKS.has(name)) {
      run.styles[name] =
        "stringValue" in mark.attrs ? (mark.attrs.stringValue as string) : true;
    } else {
      throw new Error(`unsupported mark in design document: ${name}`);
    }
  }
  if (threads.length) run.threads = threads.sort();
  return run;
}

function sameAttributes(a: ProjectedRun, b: ProjectedRun): boolean {
  return (
    JSON.stringify([a.styles, a.href ?? null, a.threads ?? []]) ===
    JSON.stringify([b.styles, b.href ?? null, b.threads ?? []])
  );
}

function projectContainer(container: PMNode): ProjectedBlock {
  let block: ProjectedBlock | undefined;
  let children: ProjectedBlock[] = [];
  container.forEach((child) => {
    if (child.type.name === "blockGroup") {
      children = projectGroup(child);
      return;
    }
    const content: ProjectedRun[] = [];
    child.forEach((inline) => {
      if (inline.type.name === "hardBreak") {
        content.push({ text: "\n", styles: {} });
        return;
      }
      const run = runOf(inline);
      const last = content[content.length - 1];
      if (last && sameAttributes(last, run)) last.text += run.text;
      else content.push(run);
    });
    const props: Record<string, unknown> = { ...child.attrs };
    block = {
      id: container.attrs.id as string,
      type: child.type.name,
      props,
      text: content.map((run) => run.text).join(""),
      content,
      children: [],
    };
  });
  if (!block) throw new Error(`block ${container.attrs.id} has no content node`);
  block.children = children;
  return block;
}

function projectGroup(group: PMNode): ProjectedBlock[] {
  const blocks: ProjectedBlock[] = [];
  group.forEach((container) => blocks.push(projectContainer(container)));
  return blocks;
}

/** Project a ProseMirror document in the design schema. */
export function projectDocument(doc: PMNode): ProjectedBlock[] {
  let blocks: ProjectedBlock[] = [];
  doc.forEach((child) => {
    if (child.type.name === "blockGroup") blocks = projectGroup(child);
  });
  return blocks;
}
