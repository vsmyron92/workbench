#!/usr/bin/env bash
# A stand-in for aider (0.86.2), for Workbench's tests: logs its arguments to
# $FAKE_AIDER_LOG, prints aider's prompt, and asks `Run shell command? (Y)es/(N)o
# [Yes]:` for an input line starting with `!`; any other line is a short turn, which
# it appends to `.aider.chat.history.md` (as aider does, here in its directory).
# `--restore-chat-history` prints the first line of that file.
set -u
[ -n "${FAKE_AIDER_LOG:-}" ] && printf '%s\n' "$*" >>"$FAKE_AIDER_LOG"
case " $* " in
*" --restore-chat-history "*) echo "Restored previous conversation history: $(head -n 1 .aider.chat.history.md 2>/dev/null)" ;;
esac
printf 'Aider v0.86.2 (fake)\nGit repo: .git\n\n> '
while IFS= read -r line; do
  line=${line//$'\r'/}
  [ -z "$line" ] && { printf '> '; continue; }
  case "$line" in
  !*)
    printf '%s\nRun shell command? (Y)es/(N)o/(D)on'"'"'t ask again [Yes]: ' "${line#!}"
    IFS= read -r yn
    [ -n "${FAKE_AIDER_LOG:-}" ] && echo "CONFIRM ${yn//$'\r'/}" >>"$FAKE_AIDER_LOG"
    ;;
  *)
    for i in 1 2 3 4 5 6 7 8; do printf 'Tokens: %s sent\n' "$i"; sleep 0.1; done
    printf '#### %s\n\nDone: %s\n\n' "$line" "$line" >>.aider.chat.history.md
    printf 'Done: %s\n' "$line"
    ;;
  esac
  printf '\n> '
done
