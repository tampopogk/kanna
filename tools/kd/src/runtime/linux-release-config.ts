import { constants, closeSync, fstatSync, openSync, readFileSync } from "node:fs";
import { isAbsolute } from "node:path";
import { z } from "zod";

/** Non-secret selectors use the existing owner-only ~/.kanna/.env.release.local
 * loader. No inherited mobile bucket, default archive, key or expiry policy. */
export const linuxReleaseConfigSchema = z.object({
  backend: z.literal("filesystem"),
  root: z.string().refine(isAbsolute, "must be absolute"),
  baseUrl: z.url().refine(value => { const u = new URL(value); return u.protocol === "https:" && !u.username && !u.password && !u.search && !u.hash; }, "must be a public HTTPS archive base URL"),
  validForHours: z.number().finite().positive(),
  publicKeyPath: z.string().refine(isAbsolute),
  fingerprint: z.string().regex(/^[a-f0-9]{40}$/i),
  privateKeyPath: z.string().refine(isAbsolute).optional(),
  passphrasePath: z.string().refine(isAbsolute).optional(),
});
export type LinuxReleaseConfig = z.infer<typeof linuxReleaseConfigSchema>;
export function linuxReleaseConfig(env: NodeJS.ProcessEnv): LinuxReleaseConfig {
  const parsed = linuxReleaseConfigSchema.safeParse({
    backend: env.KANNA_LINUX_ARCHIVE_BACKEND, root: env.KANNA_LINUX_ARCHIVE_ROOT,
    baseUrl: env.KANNA_LINUX_ARCHIVE_BASE_URL, validForHours: Number(env.KANNA_LINUX_ARCHIVE_VALID_HOURS),
    publicKeyPath: env.KANNA_LINUX_APT_PUBLIC_KEY_PATH, fingerprint: env.KANNA_LINUX_APT_FINGERPRINT,
    privateKeyPath: env.KANNA_LINUX_APT_PRIVATE_KEY_PATH, passphrasePath: env.KANNA_LINUX_APT_PASSPHRASE_PATH,
  });
  if (!parsed.success) throw new Error(`Linux release configuration missing/invalid: ${parsed.error.issues.map(i => `${i.path.join(".")}: ${i.message}`).join("; ")}. Configure KANNA_LINUX_* selectors in the machine-local release environment.`);
  return parsed.data;
}
export function readLinuxKeyFile(path: string, secret = false): string {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || (secret && ((stat.mode & 0o077) !== 0 || stat.uid !== process.getuid?.()))) throw new Error("Linux apt secret must be an owner-only regular file.");
    return readFileSync(fd, "utf8");
  } finally { closeSync(fd); }
}
