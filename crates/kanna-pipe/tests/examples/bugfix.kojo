# A bug: reproduce it with a failing test first, fix, review; then the run
# hands its branch back to the task's worktree for you to review and merge.
budget  = 20 sessions, 4h

repro   = claude prompt repro                   # a failing test that shows the bug, nothing else
shows   = run scripts/check.sh --expect-fail    # the new test fails and the rest pass
fix     = claude prompt fix guard check
check   = run scripts/check.sh
review  = codex prompt review verdict
e2e     = run scripts/e2e-kanna.sh on mini     # on another Mac, off your dev Mac

repro -> shows
shows -[fail, max 2]-> repro
shows -> fix -> check
check  -[fail, max 3, progress]-> fix
check  -> review
review -[fail, max 2]-> fix
review -> e2e
e2e    -[fail, max 1]-> fix
e2e    -> done
