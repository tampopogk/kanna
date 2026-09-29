import {
  BlockNoteSchema,
  defaultBlockSpecs,
  defaultInlineContentSpecs,
  defaultStyleSpecs,
} from "@blocknote/core";

/**
 * The one editor schema every client of an App Design document uses.
 *
 * docs/specs/app-design.md §9.1: a client that reads a Yjs document with a
 * narrower schema than the browser's makes y-prosemirror delete what it cannot
 * convert, and the deletion syncs to everyone. So the desktop editor, the phone
 * view, the fixture generator and the server's Rust adapter all agree on this
 * schema, and a document records the version it was written with. A client
 * whose version differs refuses to join rather than normalising the document.
 *
 * Slice 1 carries text blocks only: no uploads (image, file, audio, video) and
 * no tables. Changing this list, or upgrading BlockNote, changes
 * {@link DESIGN_SCHEMA_VERSION}, regenerates `fixtures/` with
 * `pnpm --filter @kanna/design-editor fixtures`, and must pass the Rust
 * adapter's fixture tests in `crates/kanna-server/src/design/document.rs`.
 */
export const DESIGN_SCHEMA_VERSION = "kanna-design-doc/1 blocknote@0.55.0";

/** The Yjs root the document's blocks live in. */
export const DOCUMENT_FRAGMENT = "document-store";

const {
  paragraph,
  heading,
  bulletListItem,
  numberedListItem,
  checkListItem,
  toggleListItem,
  quote,
  codeBlock,
  divider,
} = defaultBlockSpecs;

export const designBlockSpecs = {
  paragraph,
  heading,
  bulletListItem,
  numberedListItem,
  checkListItem,
  toggleListItem,
  quote,
  codeBlock,
  divider,
};

export const designSchema = BlockNoteSchema.create({
  blockSpecs: designBlockSpecs,
  inlineContentSpecs: defaultInlineContentSpecs,
  styleSpecs: defaultStyleSpecs,
});

export type DesignSchema = typeof designSchema;

/**
 * A machine-readable description of {@link designSchema}, written to
 * `fixtures/schema.json` so the Rust adapter checks it knows every block type,
 * prop and style the editor can produce.
 */
export function describeSchema() {
  const blocks = Object.fromEntries(
    Object.entries(designSchema.blockSchema).map(([type, config]) => [
      type,
      {
        content: (config as { content: string }).content,
        props: Object.fromEntries(
          Object.entries((config as { propSchema: Record<string, { default?: unknown; type?: string; values?: readonly unknown[] }> }).propSchema).map(
            ([name, prop]) => [
              name,
              {
                default: prop.default ?? null,
                type: prop.type ?? typeof prop.default,
                values: prop.values ? [...prop.values] : undefined,
              },
            ],
          ),
        ),
      },
    ]),
  );
  const styles = Object.fromEntries(
    Object.entries(designSchema.styleSchema).map(([name, config]) => [
      name,
      (config as { propSchema: string }).propSchema,
    ]),
  );
  return {
    version: DESIGN_SCHEMA_VERSION,
    fragment: DOCUMENT_FRAGMENT,
    blocks,
    styles,
    inlineContent: Object.keys(designSchema.inlineContentSchema),
    // Not a BlockNote style: the comment mark is tiptap's, and y-prosemirror
    // stores it under `comment--<hash>` because it may overlap itself.
    marks: ["comment"],
  };
}
