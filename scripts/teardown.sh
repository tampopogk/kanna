#!/bin/sh
. "$(cd "$(dirname "$0")" && pwd -P)/env.sh"

worktree="$(cd "${1:-.}" && pwd -P)" || exit 1
link="$worktree/target"
[ -L "$link" ] || exit 0
main="$(kanna_main_checkout "$worktree")" || exit 0
root="$(kanna_targets_root)"
case "$root" in
  /*) ;;
  "") exit 0 ;;
  *) echo "==> build.targets isn't an absolute folder: leaving target/ as it is"; exit 0 ;;
esac
if volume="$(kanna_targets_unmounted "$root")"; then
  echo "==> The volume $volume isn't mounted: no build folder to remove"
  exit 0
fi
name="$(basename "$worktree")"
case "$name" in _main|.offline) exit 0 ;; esac
folder="$root/$(basename "$main")/$name"
if [ "$(readlink "$link")" != "$folder" ]; then
  echo "==> target/ links to $(readlink "$link"), not $folder: leaving it"
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
rm "$link"
