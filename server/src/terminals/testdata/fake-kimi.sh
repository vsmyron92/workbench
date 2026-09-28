#!/usr/bin/env bash
# A stand-in for the Kimi Code CLI, for Workbench's tests and demos. A new session appends
# `{"sessionId","sessionDir","workDir"}` to $KIMI_CODE_HOME/session_index.jsonl and writes
# `<sessionDir>/state.json`, like kimi-code 2.x; `--session <id>` resumes. Every input
# line is a turn that prints a spinner for FAKE_KIMI_TURN_SECS (default 2) seconds.
# FAKE_KIMI_LAZY=1 creates the session at the first turn instead of at startup;
# FAKE_KIMI_SHOW_ID=1 prints the session id once it exists (as a status line would).
set -u
: "${KIMI_CODE_HOME:?KIMI_CODE_HOME must be set}"
[ -n "${FAKE_KIMI_LOG:-}" ] && printf '%s\n' "$*" >>"$FAKE_KIMI_LOG"
id=""
while [ $# -gt 0 ]; do
  case "$1" in
  --session | -S) id="$2"; shift 2 ;;
  *) shift ;;
  esac
done
create() {
  id="session_$(cat /proc/sys/kernel/random/uuid)"
  dir="$KIMI_CODE_HOME/sessions/wd_fake_000000000000/$id"
  mkdir -p "$dir"
  printf '{"title":"Fake Kimi session","lastPrompt":"","createdAt":"%s","updatedAt":"%s"}\n' "$(date -u +%FT%TZ)" "$(date -u +%FT%TZ)" >"$dir/state.json"
  printf '{"sessionId":"%s","sessionDir":"%s","workDir":"%s"}\n' "$id" "$dir" "$PWD" >>"$KIMI_CODE_HOME/session_index.jsonl"
  [ -n "${FAKE_KIMI_SHOW_ID:-}" ] && printf '%s\n' "$id"
}
printf '\033[1mKimi Code\033[0m (fake)  %s\n\n' "$PWD"
if [ -z "$id" ]; then
  [ -z "${FAKE_KIMI_LAZY:-}" ] && create
else
  echo "Resumed $id"
fi
printf '> '
frames='|/-\'
while IFS= read -r line; do
  line=${line//$'\r'/}
  if [ -n "$line" ] && [ -z "$id" ]; then
    create
  fi
  if [ -n "$line" ]; then
    end=$(($(date +%s%N) + ${FAKE_KIMI_TURN_SECS:-2} * 1000000000))
    i=0
    while [ "$(date +%s%N)" -lt "$end" ]; do
      printf '\r%s thinking about %s …' "${frames:$((i % 4)):1}" "$line"
      i=$((i + 1))
      sleep 0.1
    done
    printf '\rDone with %s.                              \n' "$line"
  fi
  printf '> '
done
