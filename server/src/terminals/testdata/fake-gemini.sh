#!/usr/bin/env bash
# A stand-in for the Gemini CLI (0.61.0 layout), for Workbench's tests. `--session-id
# <uuid>` starts a session under that id and `--resume <uuid>` resumes one (failing,
# like Gemini, when it has no file). The session file is
# $GEMINI_CLI_HOME/.gemini/tmp/<slug>/chats/session-<time>-<id8>.jsonl, the slug comes
# from projects.json. FAKE_GEMINI_LAZY=1 writes it at the first turn instead of at
# startup. An input line `run <cmd>` shows Gemini's tool confirmation until 1 (allow) or
# 3 (no) is typed; any other line is a turn of FAKE_GEMINI_TURN_SECS (default 1) seconds.
set -u
: "${GEMINI_CLI_HOME:?GEMINI_CLI_HOME must be set}"
[ -n "${FAKE_GEMINI_LOG:-}" ] && printf '%s\n' "$*" >>"$FAKE_GEMINI_LOG"
id=""
resume=""
while [ $# -gt 0 ]; do
  case "$1" in
  --session-id) id="$2"; shift 2 ;;
  --resume | -r) resume="$2"; shift 2 ;;
  *) shift ;;
  esac
done
g="$GEMINI_CLI_HOME/.gemini"
slug="fake-$(basename "$PWD")"
chats="$g/tmp/$slug/chats"
mkdir -p "$chats"
printf '{"projects":{"%s":"%s"}}\n' "$PWD" "$slug" >"$g/projects.json"
file=""
if [ -n "$resume" ]; then
  file=$(ls "$chats"/session-*-"${resume:0:8}".jsonl 2>/dev/null | head -n 1)
  if [ -z "$file" ]; then
    echo "Error resuming session: Invalid session identifier \"$resume\"."
    exit 1
  fi
  id="$resume"
  echo "Resumed $id"
fi
create() {
  file="$chats/session-$(date -u +%Y-%m-%dT%H-%M)-${id:0:8}.jsonl"
  printf '{"sessionId":"%s","projectHash":"x","startTime":"%s","lastUpdated":"%s","kind":"main"}\n' "$id" "$(date -u +%FT%TZ)" "$(date -u +%FT%TZ)" >"$file"
}
[ -z "$file" ] && [ -z "${FAKE_GEMINI_LAZY:-}" ] && create
printf ' Gemini CLI (fake)  %s\n\n>   Type your message\n' "$PWD"
while IFS= read -r line; do
  line=${line//$'\r'/}
  [ -z "$line" ] && continue
  [ -z "$file" ] && create
  printf '{"id":"u","timestamp":"%s","type":"user","content":[{"text":"%s"}]}\n' "$(date -u +%FT%TZ)" "$line" >>"$file"
  case "$line" in
  run\ *)
    printf ' ╭────────────────────────────╮\n │ ? Shell %s\n │ Allow execution of: %s?\n │ ● 1. Allow once\n │   2. Allow for this session\n │   3. No, suggest changes (esc)\n ╰────────────────────────────╯\n' "${line#run }" "${line#run }"
    while IFS= read -r -n 1 key; do
      case "$key" in
      1) answer="allowed"; break ;;
      3) answer="declined"; break ;;
      esac
    done
    printf '\033[2J\033[H'
    [ -n "${FAKE_GEMINI_LOG:-}" ] && echo "APPROVAL $answer" >>"$FAKE_GEMINI_LOG"
    reply="Command $answer: ${line#run }"
    ;;
  *) reply="Done: $line" ;;
  esac
  end=$(($(date +%s%N) + ${FAKE_GEMINI_TURN_SECS:-1} * 1000000000))
  while [ "$(date +%s%N)" -lt "$end" ]; do
    printf '\r⠋ Thinking… (esc to cancel) %s' "$(date +%N)"
    sleep 0.1
  done
  printf '\r\033[K✦ %s\n\n>   Type your message\n' "$reply"
  printf '{"id":"g","timestamp":"%s","type":"gemini","content":"%s"}\n' "$(date -u +%FT%TZ)" "$reply" >>"$file"
done
