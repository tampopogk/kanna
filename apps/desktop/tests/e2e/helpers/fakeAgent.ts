/**
 * A fake agent CLI, installed by a task's setup and run as its agent.
 *
 * Tests that need a task terminal to *say* something used to arrange it with
 * repository `setup` that printed and then parked. That worked while setup and
 * the agent shared one PTY. They no longer do: setup runs in the launch's own
 * startup terminal and the agent starts only once that shell exits, so a setup
 * that never exits means no agent session at all, and output printed there
 * lands in a terminal these tests never attach to.
 *
 * The output belongs to the agent, so the agent is what produces it. Setup
 * writes a small script, puts it on the PATH it exports — which the launch
 * carries into the agent's spawn through the startup receipt — and exits.
 */

/** Where a task's fake provider CLI is installed, relative to its workspace. */
export const FAKE_AGENT_BIN_DIR = ".kanna/test-provider-bin";

/**
 * Quote one line for a shell.
 *
 * Every one of these scripts contains quotes of its own, and folding them into
 * a format string is what once installed an agent that printed a literal
 * `ORIGINAL_READYn`: the script's quotes closed the writing `printf`'s, and the
 * `\n` it was supposed to print was re-parsed as an argument.
 */
function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

/**
 * Setup commands that install `provider` as a script running `lines`, and put
 * it where the agent this launch spawns will find it.
 *
 * Pass the body as separate lines; each is written verbatim, so a line may
 * contain any quoting it likes.
 */
export function installFakeAgent(provider: string, lines: string[]): string[] {
  const target = `${FAKE_AGENT_BIN_DIR}/${provider}`;
  const script = ["#!/bin/sh", ...lines].map(shellQuote).join(" ");
  return [
    `mkdir -p ${FAKE_AGENT_BIN_DIR}`,
    `printf '%s\\n' ${script} > ${target}`,
    `chmod +x ${target}`,
    // What setup exports reaches the agent through the startup receipt, which
    // is what the launch resolves the provider executable against.
    `export PATH="$PWD/${FAKE_AGENT_BIN_DIR}:$PATH"`,
  ];
}

/** A fake agent that parks forever after running `lines`. */
export function installParkingFakeAgent(provider: string, lines: string[]): string[] {
  return installFakeAgent(provider, [...lines, "while true; do sleep 60; done"]);
}
