/** Consume prepare's durable bytes, not its deleted build cache. */
import { lstatSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { z } from "zod";
import { verifyLinuxArtifact, type LinuxSource } from "./linux-release-artifacts";

const hash = z.string().regex(/^[a-f0-9]{64}$/);
const sha = z.string().regex(/^[a-f0-9]{40}$/);
export const linuxPreparationSchema = z.object({
  schemaVersion: z.literal(1), kind: z.literal("linux-local-preparation"),
  source: z.object({ revision: sha, tree: sha }).strict(),
  version: z.string().regex(/^\d+\.\d+\.\d+$/), channel: z.literal("staging"),
  iteration: z.number().int().positive(), preparedAt: z.iso.datetime(),
  artifacts: z.array(z.object({
    architecture: z.enum(["x86_64", "arm64"]), sourceRevision: sha, buildRevision: sha, buildTree: sha,
    version: z.string(), channel: z.literal("staging"), iteration: z.number().int().positive(),
    label: z.string(), fileName: z.string().regex(/^kanna-staging_\d+\.\d+\.\d+~staging\.[1-9]\d*-1_(?:amd64|arm64)\.deb$/),
    sha256: hash, sizeBytes: z.number().int().positive(), reportSha256: hash,
  }).strict()).length(2),
}).strict();
function regularBytes(path: string): Buffer {
  if (!lstatSync(path).isFile()) throw new Error("Linux prepared input must be a regular file, not a symlink.");
  return readFileSync(path);
}
export function readLinuxPrepared(manifestPath: string, input: { repoRoot: string; source: LinuxSource; version: string; iteration: number }) {
  const path = resolve(manifestPath);
  const manifest = linuxPreparationSchema.parse(JSON.parse(regularBytes(path).toString()));
  if (!isDeepStrictEqual(manifest.source, input.source) || manifest.version !== input.version || manifest.iteration !== input.iteration || new Set(manifest.artifacts.map(a => a.architecture)).size !== 2) throw new Error("Linux prepared source/tree/version/iteration or architecture pair mismatch.");
  return (["x86_64", "arm64"] as const).map(architecture => {
    const declared = manifest.artifacts.find(a => a.architecture === architecture)!;
    const debPath = join(dirname(path), declared.fileName);
    const verified = verifyLinuxArtifact({ ...input, channel: "staging", architecture, debPath, bytes: regularBytes(debPath), report: regularBytes(`${debPath}.json`) });
    if (!isDeepStrictEqual(verified.identity, declared)) throw new Error("Linux prepared artifact/report hashes or identity differ from the manifest.");
    return verified;
  });
}
