#!/usr/bin/env bash
# A stand-in for Claude Code, for Workbench's permission tests. It reads the hook URL and
# its bearer token from the `--settings` file Workbench writes and posts hooks the way
# Claude Code 2.1.283 does: SessionStart, then per input line (a "command" to run)
# UserPromptSubmit, PreToolUse (with a tool_use_id), and a PermissionRequest (without one)
# while its own dialog is on screen. The dialog is answered by whichever comes first:
# the held hook's decision, or a key typed in the terminal ("y" allows, "n" declines).
# A hook answer after the terminal's is ignored, as Claude does; it is still logged.
# Every hook response goes to $FAKE_CLAUDE_LOG as `<LABEL> <json>`.
# FAKE_CLAUDE_TOOL_SECS (default 0) is how long an allowed "tool" runs.
# Prefixes of a command line: `slow:` runs the allowed tool for 12 s; `sticky:` ignores
# the hook's decision (like a tool that requires the user's interaction: only the
# terminal answers it); `plan:` asks for ExitPlanMode instead of Bash (its dialog, as
# Claude shows it, has none of the Bash dialog's texts).
set -u
: "${FAKE_CLAUDE_LOG:?FAKE_CLAUDE_LOG must be set}"
settings=""
sid=""
while [ $# -gt 0 ]; do
  case "$1" in
  --settings) settings="$2"; shift 2 ;;
  --session-id | --resume) sid="$2"; shift 2 ;;
  *) shift ;;
  esac
done
url=$(sed -n 's/.*"url": "\(http[^"]*\/api\/hooks\/claude\/[^"]*\)".*/\1/p' "$settings" | head -n 1)
auth=$(sed -n 's/.*"Authorization": "\([^"]*\)".*/\1/p' "$settings" | head -n 1)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# The token goes to curl through a 0600 file, never through argv.
umask 077
printf 'Authorization: %s\n' "$auth" >"$work/headers"
post() { curl -s -m 700 -X POST -H 'Content-Type: application/json' -H @"$work/headers" --data-binary "$1" "$url"; }
json_str() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

post "{\"hook_event_name\":\"SessionStart\",\"source\":\"startup\",\"session_id\":\"$sid\",\"model\":\"fake-haiku\"}" >/dev/null
printf 'Fake Claude Code  %s\n> ' "$PWD"
n=0
while IFS= read -r line; do
  line=${line//$'\r'/}
  [ -z "$line" ] && { printf '> '; continue; }
  n=$((n + 1))
  cmd=$(json_str "$line")
  tid="toolu_fake_$n"
  tool=Bash
  input="{\"command\":\"$cmd\",\"description\":\"Run it\"}"
  secs=${FAKE_CLAUDE_TOOL_SECS:-0}
  sticky=""
  case "$line" in
  slow:*) secs=12 ;;
  sticky:*) sticky=1 ;;
  plan:*) tool=ExitPlanMode; input="{\"plan\":\"$cmd\"}" ;;
  esac
  post "{\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"$cmd\"}" >/dev/null
  post "{\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"$tool\",\"tool_use_id\":\"$tid\",\"tool_input\":$input}" >/dev/null
  if [ "$tool" = ExitPlanMode ]; then
    printf '\n Ready to code?\n\n Here is Claude'"'"'s plan:\n   %s\n\n Would you like to proceed?\n > 1. Yes, and auto-accept edits\n   2. Yes, and manually approve edits\n   3. No, keep planning\n' "$line"
  else
    printf '\n Bash command\n   %s\n\n Do you want to proceed?\n > 1. Yes\n   2. Yes, and don'"'"'t ask again for this command\n   3. No, and tell Claude what to do differently (esc)\n' "$line"
  fi
  rm -f "$work/resp"
  (post "{\"hook_event_name\":\"PermissionRequest\",\"tool_name\":\"$tool\",\"tool_input\":$input,\"cwd\":\"$PWD\",\"permission_mode\":\"default\",\"permission_suggestions\":[{\"type\":\"addRules\",\"rules\":[{\"toolName\":\"Bash\",\"ruleContent\":\"$cmd\"}],\"behavior\":\"allow\",\"destination\":\"localSettings\"}]}" >"$work/resp.part"; mv "$work/resp.part" "$work/resp") &
  hook=$!
  answer=""
  while [ -z "$answer" ]; do
    if [ -f "$work/resp" ]; then
      resp=$(cat "$work/resp")
      echo "HOOK $resp" >>"$FAKE_CLAUDE_LOG"
      case "$sticky$resp" in
      1*'"behavior"'*) echo "IGNORED $resp" >>"$FAKE_CLAUDE_LOG"; rm -f "$work/resp" ;;
      *'"allow"'*) answer=hook-allow ;;
      *'"deny"'*) answer=hook-deny ;;
      *) rm -f "$work/resp" ;; # no decision: the dialog stays
      esac
    elif IFS= read -r -t 0.1 -n 1 key; then
      case "$key" in
      y) answer=terminal-allow ;;
      n) answer=terminal-deny ;;
      esac
    fi
  done
  # The dialog closes.
  printf '\033[2J\033[H'
  echo "ANSWER $answer" >>"$FAKE_CLAUDE_LOG"
  case "$answer" in
  *allow)
    printf 'Running %s\n' "$line"
    sleep "$secs"
    post "{\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"$tool\",\"tool_use_id\":\"$tid\",\"tool_input\":$input,\"tool_response\":{}}" >/dev/null
    post "{\"hook_event_name\":\"Stop\",\"last_assistant_message\":\"Done: $cmd\"}" >/dev/null
    ;;
  *deny)
    printf 'Declined %s\n' "$line"
    post "{\"hook_event_name\":\"Stop\",\"last_assistant_message\":\"Declined: $cmd\"}" >/dev/null
    ;;
  esac
  # A held hook the terminal answered first still ends (with no decision).
  wait "$hook" 2>/dev/null
  if [ "${answer#terminal}" != "$answer" ] && [ -f "$work/resp" ]; then
    echo "LATE $(cat "$work/resp")" >>"$FAKE_CLAUDE_LOG"
  fi
  printf '> '
done
