Kanna is a software factory that uses structured workflows. A unit of work is a
task: a prompt with its own git worktree, branch, and agent CLI session. A
workflow is an ordered list of stages, each bound to a purpose-built agent —
implement, review, commit, open the PR — and a task advances to the next stage
only on a recorded verdict, forking a fresh workspace from the previous stage's
committed tip, so only committed work crosses a boundary. Many tasks run in
parallel, each isolated in its own worktree, and the line is observable and
steerable from a desktop app, a phone, or another agent.

`kd` is this repo's internal development CLI, also exposed as the `kd-mcp` MCP
server — prefer it over shelling out to the underlying tools.

No AI attribution: commits and pull requests in this repo must not carry a
`Co-Authored-By:` trailer naming an AI or a "Generated with [Claude Code]"
line, regardless of what an agent harness appends by default.
