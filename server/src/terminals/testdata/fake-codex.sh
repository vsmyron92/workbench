#!/usr/bin/env bash
# A stand-in for the OpenAI Codex CLI, for Workbench's tests and demos. It accepts the
# command lines Workbench builds (`[resume|fork] [options] [<id>] [-- <prompt>]`), prints
# a banner, and writes a rollout the way codex-cli 0.157 does:
#   $CODEX_HOME/sessions/YYYY/MM/DD/rollout-<local time>-<thread id>.jsonl
# whose first line is `session_meta`, created at the first turn and held open, with
# `task_started` / `task_complete` events per turn. Every input line is a turn.
# FAKE_CODEX_TURN_SECS sets how long a turn takes; FAKE_CODEX_LOG records the argv.
# FAKE_CODEX_APPROVAL=1: the first turn typed in asks to approve a command, the way
# codex 0.157's overlay does (nothing in the rollout), and logs `APPROVAL <answer line>`;
# like the real one, whatever line comes next answers it (an Enter approves).
set -u
case " $* " in
*" --help "*)
  printf 'Codex CLI (fake)\n\nOptions:\n      --no-daemon\n      --no-alt-screen\n'
  exit 0
  ;;
esac
: "${CODEX_HOME:?CODEX_HOME must be set}"
[ -n "${FAKE_CODEX_LOG:-}" ] && printf '%s\n' "$*" >>"$FAKE_CODEX_LOG"

uuid_re='^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
mode=new
case "${1:-}" in resume) mode=resume ;; fork) mode=fork ;; esac
src=""
prompt=""
after=0
for a in "$@"; do
  if [ "$after" = 1 ]; then prompt="$a"; continue; fi
  if [ "$a" = "--" ]; then after=1; continue; fi
  if [[ $a =~ $uuid_re ]]; then src="$a"; fi
done

ts() { date -u +%Y-%m-%dT%H:%M:%S.%3NZ; }
esc() { local s=${1//\\/\\\\}; printf '%s' "${s//\"/\\\"}"; }
file=""
open_new() {
  local id dir extra=""
  id=$(cat /proc/sys/kernel/random/uuid)
  dir="$CODEX_HOME/sessions/$(date +%Y/%m/%d)"
  mkdir -p "$dir"
  file="$dir/rollout-$(date +%Y-%m-%dT%H-%M-%S)-$id.jsonl"
  [ "$mode" = fork ] && extra=",\"forked_from_id\":\"$src\""
  printf '{"timestamp":"%s","type":"session_meta","payload":{"id":"%s","timestamp":"%s","cwd":"%s","originator":"codex-tui","cli_version":"0.0.0-fake","source":"cli","model_provider":"fake"%s}}\n' \
    "$(ts)" "$id" "$(ts)" "$(esc "$PWD")" "$extra" >"$file"
  exec 3>>"$file"
  printf '{"timestamp":"%s","type":"turn_context","payload":{"cwd":"%s","model":"fake-model","effort":"high","approval_policy":"on-request"}}\n' "$(ts)" "$(esc "$PWD")" >&3
}
n=0
turn() {
  [ -z "$file" ] && open_new
  n=$((n + 1))
  local t="turn-$$-$n"
  printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"user_message","message":"%s"}}\n' "$(ts)" "$(esc "$1")" >&3
  printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"task_started","turn_id":"%s"}}\n' "$(ts)" "$t" >&3
  printf '\033[2m• Working on: %s\033[0m\n' "$1"
  if [ "${2:-}" = approval ]; then
    printf 'Would you like to run the following command?\n\n  $ rm -rf ./build && git push --force\n\n› 1. Yes, just this once (y)\n  2. No, and tell Codex what to do differently (esc)\n'
    local answer
    IFS= read -r answer
    answer=${answer//$'\e[200~'/}
    answer=${answer//$'\e[201~'/}
    answer=${answer//$'\r'/}
    [ -n "${FAKE_CODEX_LOG:-}" ] && printf 'APPROVAL %s\n' "$answer" >>"$FAKE_CODEX_LOG"
    # The overlay goes away (six lines and the echoed answer).
    printf '\033[7A\033[J✔ Answered: %s\n' "$answer"
  fi
  sleep "${FAKE_CODEX_TURN_SECS:-1}"
  printf '{"timestamp":"%s","type":"event_msg","payload":{"type":"task_complete","turn_id":"%s","last_agent_message":"Done: %s"}}\n' "$(ts)" "$t" "$(esc "$1")" >&3
  printf '• Done: %s\n' "$1"
}

printf '\033[1m>_ OpenAI Codex\033[0m (fake, v0.0.0)\n\n  model:     fake-model high\n  directory: %s\n\n' "$PWD"
if [ "$mode" = resume ]; then
  file=$(ls "$CODEX_HOME"/sessions/*/*/*/rollout-*-"$src".jsonl 2>/dev/null | head -1)
  if [ -z "$file" ]; then
    echo "No saved session found with ID $src"
    exit 1
  fi
  exec 3>>"$file"
  echo "Resumed session $src"
fi
[ -n "$prompt" ] && turn "$prompt"
printf '› '
ask=${FAKE_CODEX_APPROVAL:-}
while IFS= read -r line; do
  line=${line//$'\e[200~'/}
  line=${line//$'\e[201~'/}
  line=${line//$'\r'/}
  if [ -n "$line" ]; then
    if [ -n "$ask" ]; then
      ask=""
      turn "$line" approval
    else
      turn "$line"
    fi
  fi
  printf '› '
done
