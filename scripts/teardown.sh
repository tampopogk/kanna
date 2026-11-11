#!/bin/sh
. "$(cd "$(dirname "$0")" && pwd -P)/env.sh"

worktree="$(cd "${1:-.}" && pwd -P)" || exit 1
target="$worktree/target"
[ -L "$target" ] || [ -d "$target" ] || exit 0
main="$(kanna_main_checkout "$worktree")" || exit 0
root="$(kanna_targets_root "$worktree")"
case "$root" in
  /*) ;;
  "") exit 0 ;;
  *) echo "==> targets in $(kanna_targets_config "$worktree") isn't an absolute folder: leaving target/ as it is"; exit 0 ;;
esac
name="$(basename "$worktree")"
case "$name" in _main) exit 0 ;; esac
folder="$root/$(basename "$main")/$name"
if [ -L "$target" ] && [ "$(readlink "$target")" != "$folder" ]; then
  echo "==> target/ links to $(readlink "$target"), not $folder: leaving it"
  exit 0
fi
if volume="$(kanna_targets_unmounted "$root")"; then
  echo "==> The volume $volume isn't mounted: leaving target/ and its build folder"
  exit 0
fi
if [ -L "$folder" ]; then
  echo "==> $folder is a link, not a build folder: leaving it"
  exit 0
fi
if [ -d "$folder" ]; then
  echo "==> Removing $folder"
  rm -rf "$folder" || exit 1
fi
if [ -L "$target" ]; then
  rm "$target"
  exit 0
fi
echo "==> Removing $target"
kanna_remove "$target"
