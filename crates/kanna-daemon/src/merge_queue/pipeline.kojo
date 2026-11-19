merge   = run git merge --no-ff -m "$KANNA_MERGE_MESSAGE" "$KANNA_MERGE_HEAD" label "merging"
resolve = agent prompt merge-resolve verdict label "resolving conflicts"
test    = run sh -c "$KANNA_MERGE_TEST" label "testing"
judge   = agent prompt merge-judge verdict label "judging a failure"
merge   -[pass]-> test
merge   -[fail]-> resolve
resolve -[pass]-> test
test    -[pass]-> done
test    -[fail]-> judge
judge   -[pass]-> done
