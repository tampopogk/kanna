export interface SpawnOptions {
  cwd: string
  prompt: string
  spawnFn: (sessionId: string, cwd: string, prompt: string, cols: number, rows: number) => Promise<void>
}

export interface TerminalOptions {
  kittyKeyboard?: boolean
  agentProvider?: string
  worktreePath?: string
  agentTerminal?: boolean
  skipInitialReconnectEffects?: boolean
  /**
   * Whether a launch could still create this task's agent session.
   *
   * A missing session on an attach-only view means "not yet" while a launch is
   * running its startup terminal, and "never" for a task that is closed or
   * whose agent has exited. The view cannot tell those apart from the daemon's
   * refusal alone, so the caller — which holds the task's record — answers.
   * Read at attach time rather than captured, so a task that finishes while
   * the tab is open stops waiting.
   */
  agentSessionCanStart?: () => boolean
  recoverSession?: (sessionId: string, options?: { cols?: number; rows?: number }) => Promise<void>
}
