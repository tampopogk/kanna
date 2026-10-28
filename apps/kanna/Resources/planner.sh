#!/bin/sh

kanna= zsh= repos="$HOME/.kanna-v4/repos"
while [ $# -gt 0 ]; do
  case $1 in
    --kanna) kanna=$2; shift 2 ;;
    --zsh) zsh=$2; shift 2 ;;
    --repos) repos=$2; shift 2 ;;
    --) shift; break ;;
    *) break ;;
  esac
done

shell() {
  trap - INT
  if [ -n "$zsh" ]; then
    case "${SHELL##*/}" in
      zsh) [ -z "${ZDOTDIR+x}" ] || export KANNA_ZSH_ZDOTDIR="$ZDOTDIR"; export ZDOTDIR="$zsh" ;;
    esac
  fi
  exec "${SHELL:-/bin/sh}" -l
}
trap 'echo; shell' INT

say() {
  printf '%s\n' "$*" | tr -d '\000-\010\013-\037\177'
}

tilde() {
  case $1 in
    "$HOME") echo "~" ;;
    "$HOME"/*) echo "~${1#"$HOME"}" ;;
    *) echo "$1" ;;
  esac
}

ask() {
  printf '%s ' "$1" | tr -d '\000-\010\013-\037\177'
  read -r answer || { echo; shell; }
}

moved() {
  url=$(pwd -P | sed 's/%/%25/g; s/ /%20/g')
  printf '\033]7;file://localhost%s\033\\' "$url"
}

origin_is() {
  origin=$(git -C "$1" remote get-url origin 2>/dev/null) || return 1
  case ${origin%.git} in
    *"${2%.git}") return 0 ;;
  esac
  return 1
}

place() {
  url=${1%/}
  url=${url%.git}
  name=$2
  case $url in
    file://*|/*|.*|'') echo "local/$name"; return ;;
    *://*) rest=${url#*://}; rest=${rest##*@}; host=${rest%%/*}; path=${rest#*/} ;;
    *:*) rest=${url##*@}; host=${rest%%:*}; path=${rest#*:} ;;
    *) echo "local/$name"; return ;;
  esac
  host=${host%%:*}
  case /$path/ in
    *//*|*/.*) echo "local/$name"; return ;;
  esac
  case $path in
    */*) echo "$path" ;;
    *) echo "$host/$path" ;;
  esac
}

misplaced() {
  common=$(git -C "$1" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
  case $common in
    */.git) main=$(cd "${common%/.git}" && pwd -P) ;;
    *) main=$(cd "$1" && pwd -P) ;;
  esac
  origin=$(git -C "$main" remote get-url origin 2>/dev/null)
  root=$(cd "$repos" 2>/dev/null && pwd -P) || root=$repos
  want="$root/$(place "$origin" "$(basename "$main")")"
  [ "$main" != "$want" ]
}

offer() {
  say "$(tilde "$main") isn't in $(tilde "$repos"), where Kanna keeps its projects so each has the same place on every machine."
  say "Move it there: kanna project move $(tilde "$main")"
  [ -n "$kanna" ] || return 1
  ask "Move it to $(tilde "$want") now? [y/N]"
  case $answer in
    [yY]*) "$kanna" project move "$main" && cd "$want" ;;
    *) return 1 ;;
  esac
}

go() {
  if [ "$(cd "$1" && pwd -P)" = "$(cd "$HOME" && pwd -P)" ]; then
    cd "$1"
    return
  fi
  if misplaced "$1"; then
    offer
    return
  fi
  cd "$1"
}

clone() {
  how=$1 from=$2 name=$3
  dest="$repos/$(place "$from" "$name")"
  [ "$how" = gh ] && dest="$repos/$from"
  if [ -e "$dest" ]; then
    if origin_is "$dest" "$from"; then
      say "Opened $(tilde "$dest") (already cloned)"
      cd "$dest" && return 0
    fi
    say "$(tilde "$dest") is already there, and isn't a clone of $from."
    return 1
  fi
  mkdir -p "${dest%/*}" || return 1
  say "Cloning $from into $(tilde "$dest")…"
  case $how in
    gh)
      command -v gh >/dev/null 2>&1 || { echo "gh isn't installed: give the repo's URL."; return 1; }
      gh repo clone "$from" "$dest" ;;
    *) git clone -- "$from" "$dest" ;;
  esac || { say "Couldn't clone $from."; return 1; }
  cd "$dest"
}

where() {
  say "Open a project, clone a repo (a URL or owner/repo), or name a new project. Kanna keeps them in $(tilde "$repos"): a clone as OWNER/NAME, a new project as local/NAME."
  echo
  recent= recents=0
  listed=$([ -n "$kanna" ] && "$kanna" projects --json 2>/dev/null)
  count=$(printf '%s' "$listed" | jq -e '.projects | length' 2>/dev/null) || count=0
  question="Folder, repo or name:"
  if [ "$count" -gt 0 ] 2>/dev/null; then
    echo "Recent:"
    while [ "$recents" -lt "$count" ]; do
      project=$(printf '%s' "$listed" | jq -er ".projects[$recents].path // empty" 2>/dev/null) || break
      label=$(printf '%s' "$listed" | jq -er ".projects[$recents].label // empty" 2>/dev/null) || label=$(tilde "$project")
      recents=$((recents + 1))
      recent="$recent$project$nl"
      say "  $recents  $label"
    done
    echo
    question="Folder, repo, name or number [1]:"
  fi
  while :; do
    ask "$question"
    if [ "$recents" -gt 0 ]; then
      case $answer in
        '') answer=1 ;;
      esac
      case $answer in
        *[!0-9]*) ;;
        *)
          if [ "$answer" -ge 1 ] && [ "$answer" -le "$recents" ]; then
            folder=$(printf '%s\n' "$recent" | sed -n "${answer}p")
            if [ -d "$folder" ]; then
              go "$folder" && break
            else
              say "No folder $(tilde "$folder")."
            fi
          else
            say "Choose 1 to $recents, or a folder, repo or name."
          fi
          continue ;;
      esac
    fi
    case $answer in
      '') continue ;;
      https://*|http://*|ssh://*|git://*|file://*|git@*)
        name=$(basename "${answer%/}" .git)
        name=${name##*:}
        clone git "$answer" "$name" && break
        continue ;;
    esac
    case $answer in
      '~') folder=$HOME ;;
      '~/'*) folder="$HOME/${answer#'~/'}" ;;
      *) folder=$answer ;;
    esac
    if [ -d "$folder" ]; then
      go "$folder" && break
      continue
    fi
    case $answer in
      '~'*|.*|/*) say "No folder $answer." ;;
      */*)
        case $answer in
          */*/*|*[!A-Za-z0-9._/-]*) say "No folder $answer." ;;
          *) clone gh "$answer" "${answer#*/}" && break ;;
        esac ;;
      *[!A-Za-z0-9._-]*|[!A-Za-z0-9]*) echo "A project's name is letters, digits, '.', '-' and '_'." ;;
      *)
        dest="$repos/local/$answer"
        if [ -d "$dest" ]; then
          say "Opened $(tilde "$dest")"
        else
          mkdir -p "$dest" || continue
          say "Made $(tilde "$dest")"
        fi
        cd "$dest" && break ;;
    esac
  done
  echo
  moved
}

setting() {
  top=$(git rev-parse --show-toplevel 2>/dev/null)
  for file in ${top:+"$top/.kanna/config.json"} "$HOME/.kanna-v4/config.json"; do
    [ -f "$file" ] || continue
    version=$(jq -r '.version // empty' "$file" 2>/dev/null)
    case $version in
      ''|1) ;;
      *) continue ;;
    esac
    value=$(jq -er '.planning.agent // empty' "$file" 2>/dev/null) && [ -n "$value" ] && { echo "$value"; return; }
  done
}

save() {
  file="$1/.kanna/config.json"
  [ -L "$1/.kanna" ] || [ -L "$file" ] && return 1
  mkdir -p "$1/.kanna" || return 1
  if [ ! -s "$file" ]; then
    printf '{\n  "version": 1,\n  "planning": {\n    "agent": "%s"\n  }\n}\n' "$2" >"$file"
    return
  fi
  json=$(jq --arg agent "$2" '.planning.agent = $agent' "$file") && printf '%s\n' "$json" >"$file"
}

nl='
'
installed=
while [ $# -ge 3 ]; do
  command -v "$2" >/dev/null 2>&1 && installed="$installed$1	$3$nl"
  shift 3
done

if [ -n "$kanna" ]; then
  "$kanna" agents setup --ask
  "$kanna" agents update --ask
fi

if [ "$(pwd -P)" = "$(cd "$HOME" && pwd -P)" ]; then
  where
elif misplaced .; then
  cd "$HOME"
  moved
  if offer; then
    echo
    moved
  else
    echo
    where
  fi
fi

if [ -z "$installed" ]; then
  echo "No agent Kanna knows is installed (claude, codex, copilot, opencode or agy). Install one, then start it here."
  shell
fi

count=$(printf '%s' "$installed" | grep -c .)
chosen=
wanted=$(setting)
if [ -n "$wanted" ]; then
  chosen=$(printf '%s' "$installed" | awk -F '\t' -v n="$wanted" '$1 == n { print; exit }')
  [ -n "$chosen" ] || say "planning.agent is $wanted, which isn't installed."
fi
if [ -z "$chosen" ] && [ "$count" -eq 1 ]; then
  chosen=$installed
fi
if [ -z "$chosen" ]; then
  echo "Choose an agent for planning and task creation"
  echo
  printf '%s' "$installed" | awk -F '\t' '{ printf "  %d  %s\n", NR, $1 }'
  echo
  while :; do
    ask "Choose [1]:"
    [ -n "$answer" ] || answer=1
    case $answer in
      *[!0-9]*) ;;
      *) [ "$answer" -ge 1 ] && [ "$answer" -le "$count" ] && break ;;
    esac
    say "Choose 1 to $count."
  done
  chosen=$(printf '%s' "$installed" | sed -n "${answer}p")
  name=${chosen%%	*}
  top=$(git rev-parse --show-toplevel 2>/dev/null)
  if [ -n "$top" ]; then
    ask "Make $name the planning agent for $(basename "$top")? [y/N]"
    case $answer in
      [yY]*)
        if save "$top" "$name"; then
          say "$name plans $(basename "$top") from now on (planning.agent in .kanna/config.json)."
        else
          say "Couldn't write $top/.kanna/config.json."
        fi ;;
    esac
  fi
  echo
fi

planner() {
  [ -n "$kanna" ] && [ -n "$KANNA_TERMINAL" ] || return 1
  group=$(ps -o pgid= -p $$ 2>/dev/null | tr -d ' ')
  request=$(jq -nc --arg pane "$KANNA_TERMINAL" --arg folder "$(pwd -P)" --arg agent "$1" --arg step "$2" --arg group "$group" '{cmd: "backlog", args: {action: "planner", step: $step, pane: $pane, folder: $folder, agent: $agent, group: $group}}') || return 1
  "$kanna" raw "$request" 2>/dev/null
}

command=${chosen#*	}
command=${command%"$nl"}
name=${chosen%%	*}
reply=$(planner "$name" start)
given=$(printf '%s' "$reply" | jq -er '.command // empty' 2>/dev/null) && command=$given
said=$(printf '%s' "$reply" | jq -r '.said // empty' 2>/dev/null)
[ -z "$said" ] || { say "$said"; echo; }
after=$(printf '%s' "$reply" | jq -er '.after // empty' 2>/dev/null) && {
  waited=0
  while [ "$waited" -lt 50 ] && kill -0 -- "-$after" 2>/dev/null; do
    [ "$waited" -eq 5 ] && echo "Waiting for the last planner to end…"
    sleep 0.2
    waited=$((waited + 1))
  done
}
trap : INT
eval "$command"
echo
again=$(planner "$name" quit | jq -er '.again // empty' 2>/dev/null) && command=$again
say "The planning agent quit. Start it again: $command"
shell
