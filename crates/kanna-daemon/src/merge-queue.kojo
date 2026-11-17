merge   = run git merge --no-ff -m "$KANNA_MERGE_MESSAGE" "$KANNA_MERGE_HEAD"
resolve = {agent} prompt merge-resolve verdict
test    = run {test}
judge   = {agent} prompt merge-judge verdict
merge   -[pass]-> test
merge   -[fail]-> resolve
resolve -[pass]-> test
test    -[pass]-> done
test    -[fail]-> judge
judge   -[pass]-> done
