#!/usr/bin/env python3
# Stand-ins for the agent CLIs Workbench hosts, for its end-to-end tests on every OS
# (terminals/e2e.rs, e2e_agents.rs). The file acts as the CLI it is named after: the tests
# copy it as `claude`, `codex`, `kimi`, `gemini` or `aider` (with `.py` on Windows, started
# through an npm-style shim). Python 3's standard library only.
#
# claude: Claude Code (2.1.283) for the permission tests. It reads the hook URL and its
#   bearer token from the `--settings` file Workbench writes and posts hooks the way Claude
#   Code does: SessionStart, then per input line (a "command" to run) UserPromptSubmit,
#   PreToolUse (with a tool_use_id), and a PermissionRequest (without one) while its own
#   dialog is on screen. The dialog is answered by whichever comes first: the held hook's
#   decision, or a key typed in the terminal ("y" allows, "n" declines). A hook answer
#   after the terminal's is ignored, as Claude does; it is still logged. Every hook
#   response goes to $FAKE_CLAUDE_LOG as `<LABEL> <json>`. FAKE_CLAUDE_TOOL_SECS (default
#   0) is how long an allowed "tool" runs. Prefixes of a command line: `slow:` runs the
#   allowed tool for 12 s; `sticky:` ignores the hook's decision (like a tool that
#   requires the user's interaction: only the terminal answers it); `plan:` asks for
#   ExitPlanMode instead of Bash (its dialog, as Claude shows it, has none of the Bash
#   dialog's texts). `claude auth status --json` / `auth login`: signed in where
#   $CLAUDE_CONFIG_DIR holds a `signed-in` file, which the login (a pasted code) creates.
# codex: the OpenAI Codex CLI. It accepts the command lines Workbench builds
#   (`[resume|fork] [options] [<id>] [-- <prompt>]`), prints a banner, and writes a
#   rollout the way codex-cli 0.157 does:
#   $CODEX_HOME/sessions/YYYY/MM/DD/rollout-<local time>-<thread id>.jsonl, whose first
#   line is `session_meta`, created at the first turn and held open, with `task_started` /
#   `task_complete` events per turn. Every input line is a turn. FAKE_CODEX_TURN_SECS sets
#   how long a turn takes; FAKE_CODEX_LOG records the argv. FAKE_CODEX_APPROVAL=1: the
#   first turn typed in asks to approve a command, the way codex 0.157's overlay does
#   (nothing in the rollout), and logs `APPROVAL <answer line>`; like the real one,
#   whatever line comes next answers it (an Enter approves).
# kimi: the Kimi Code CLI. A new session appends `{"sessionId","sessionDir","workDir"}` to
#   $KIMI_CODE_HOME/session_index.jsonl and writes `<sessionDir>/state.json`, like
#   kimi-code 2.x; `--session <id>` resumes. Every input line is a turn that prints a
#   spinner for FAKE_KIMI_TURN_SECS (default 2) seconds. FAKE_KIMI_LAZY=1 creates the
#   session at the first turn instead of at startup; FAKE_KIMI_SHOW_ID=1 prints the
#   session id once it exists (as a status line would).
# gemini: the Gemini CLI (0.61.0 layout). `--session-id <uuid>` starts a session under that
#   id and `--resume <uuid>` resumes one (failing, like Gemini, when it has no file). The
#   session file is $GEMINI_CLI_HOME/.gemini/tmp/<slug>/chats/session-<time>-<id8>.jsonl,
#   the slug comes from projects.json. FAKE_GEMINI_LAZY=1 writes it at the first turn
#   instead of at startup. An input line `run <cmd>` shows Gemini's tool confirmation
#   until 1 (allow) or 3 (no) is typed; any other line is a turn of FAKE_GEMINI_TURN_SECS
#   (default 1) seconds.
# aider: aider (0.86.2). It logs its arguments to $FAKE_AIDER_LOG, prints aider's prompt,
#   and asks `Run shell command? (Y)es/(N)o [Yes]:` for an input line starting with `!`;
#   any other line is a short turn, which it appends to `.aider.chat.history.md` (as aider
#   does, here in its directory). `--restore-chat-history` prints the first line of that
#   file.

import datetime
import glob
import json
import os
import re
import sys
import time
import uuid


def out(s):
    sys.stdout.write(s)
    sys.stdout.flush()


def dumps(v):
    return json.dumps(v, ensure_ascii=False, separators=(",", ":"))


def write(path, text):
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def append(path, text):
    with open(path, "a", encoding="utf-8", newline="\n") as f:
        f.write(text)


def log(var, line):
    path = os.environ.get(var)
    if path:
        append(path, line + "\n")


def need(var):
    v = os.environ.get(var)
    if not v:
        sys.exit(f"{var} must be set")
    return v


def pwd():
    """The working directory as a shell's $PWD names it."""
    p = os.environ.get("PWD")
    try:
        if p and os.path.samefile(p, "."):
            return p
    except OSError:
        pass
    return os.getcwd()


def iso():
    """`date -u +%FT%TZ`"""
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def ts():
    """UTC with milliseconds, as Codex writes timestamps."""
    now = datetime.datetime.now(datetime.timezone.utc)
    return now.strftime("%Y-%m-%dT%H:%M:%S.") + f"{now.microsecond // 1000:03d}Z"


def turn_secs(var, default):
    try:
        return float(os.environ.get(var) or default)
    except ValueError:
        return float(default)


class Terminal:
    """Lines and single keys typed into the terminal: a PTY on Unix, a console on Windows."""

    def __init__(self):
        self.pending = b""

    def line(self):
        """The next line without its line break; None once input ended."""
        if os.name == "nt":
            s = sys.stdin.readline()
            return None if s == "" else s.rstrip("\n")
        while b"\n" not in self.pending:
            chunk = os.read(0, 4096)
            if not chunk:
                if not self.pending:
                    return None
                break
            self.pending += chunk
        line, _, self.pending = self.pending.partition(b"\n")
        return line.decode("utf-8", "replace")

    def key(self, timeout=None):
        """One key as it is typed (no Enter needed), or None when none comes within
        `timeout` seconds or input ended."""
        if os.name == "nt":
            import msvcrt

            end = None if timeout is None else time.monotonic() + timeout
            while not msvcrt.kbhit():
                if end is not None and time.monotonic() >= end:
                    return None
                time.sleep(0.01)
            return msvcrt.getwch()
        if self.pending:
            k, self.pending = self.pending[:1], self.pending[1:]
            return k.decode("latin-1")
        import select
        import termios

        try:
            old = termios.tcgetattr(0)
        except termios.error:
            old = None
        try:
            if old is not None:
                # Like bash's `read -n 1`: characters as they come, echo as it was.
                new = termios.tcgetattr(0)
                new[3] &= ~termios.ICANON
                new[6][termios.VMIN] = 1
                new[6][termios.VTIME] = 0
                termios.tcsetattr(0, termios.TCSANOW, new)
            ready, _, _ = select.select([0], [], [], timeout)
            if not ready:
                return None
            b = os.read(0, 1)
            return b.decode("latin-1") if b else None
        finally:
            if old is not None:
                termios.tcsetattr(0, termios.TCSANOW, old)


term = Terminal()


def unpaste(line):
    """A line without bracketed-paste markers and carriage returns."""
    return line.replace("\x1b[200~", "").replace("\x1b[201~", "").replace("\r", "")


# ---------------------------------------------------------------- claude

BASH_DIALOG = (
    "\n Bash command\n   {}\n\n Do you want to proceed?\n > 1. Yes\n"
    "   2. Yes, and don't ask again for this command\n"
    "   3. No, and tell Claude what to do differently (esc)\n"
)
PLAN_DIALOG = (
    "\n Ready to code?\n\n Here is Claude's plan:\n   {}\n\n Would you like to proceed?\n"
    " > 1. Yes, and auto-accept edits\n   2. Yes, and manually approve edits\n   3. No, keep planning\n"
)


def claude_auth(args):
    # `claude auth status` and `claude auth login`, as Claude Code 2.1 answers them. The login is a
    # marker file in $CLAUDE_CONFIG_DIR (a token in the real one), so a test can tell which account
    # was signed in. `status` prints an email, which Workbench must never pass on.
    cfg = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.claude")
    marker = os.path.join(cfg, "signed-in")
    log("FAKE_CLAUDE_LOG", "AUTH " + " ".join(args) + " config=" + cfg)
    if args[:1] == ["status"]:
        if os.path.exists(marker):
            reply = {"loggedIn": True, "authMethod": "claude.ai", "apiProvider": "firstParty", "email": "person@example.com", "subscriptionType": "max"}
            out(json.dumps(reply, indent=2) + "\n")
            return
        out(json.dumps({"loggedIn": False, "authMethod": "none", "apiProvider": "firstParty"}, indent=2) + "\n")
        sys.exit(1)
    if args[:1] == ["login"]:
        out("Opening browser to sign in…\nIf the browser did not open, visit: https://claude.example/oauth/authorize?code=true\nPaste code here if prompted > ")
        code = sys.stdin.readline().strip()
        if not code:
            sys.exit(1)
        os.makedirs(cfg, exist_ok=True)
        open(marker, "w", encoding="utf-8").close()
        out("\nLogin successful.\n")
        return
    sys.exit(f"claude auth: unknown command {args!r}")


def claude(args):
    if args[:1] == ["auth"]:
        return claude_auth(args[1:])
    # Imported here: they slow every other fake's start (and its argv log) by tens of ms.
    import threading
    import urllib.error
    import urllib.request

    need("FAKE_CLAUDE_LOG")
    settings = sid = ""
    i = 0
    while i < len(args):
        if args[i] == "--settings" and i + 1 < len(args):
            settings = args[i + 1]
            i += 2
        elif args[i] in ("--session-id", "--resume") and i + 1 < len(args):
            sid = args[i + 1]
            i += 2
        else:
            i += 1
    with open(settings, encoding="utf-8") as f:
        hooks = json.load(f).get("hooks", {})
    # What the session was started with: its command line (a prompt may span lines), and the
    # environment that decides which account or model server it talks to.
    log("FAKE_CLAUDE_LOG", "ARGS " + " ".join(args).replace("\n", " / "))
    log(
        "FAKE_CLAUDE_LOG",
        "ENV "
        + " ".join(
            f"{k}={os.environ.get(k, '<unset>')}"
            for k in ("CLAUDE_CONFIG_DIR", "ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL", "CLAUDE_CODE_SUBAGENT_MODEL", "CLAUDE_CODE_MAX_CONTEXT_TOKENS")
        ),
    )
    url = auth = ""
    for groups in hooks.values():
        for g in groups:
            for h in g.get("hooks", []):
                if not url and h.get("type") == "http" and "/api/hooks/claude/" in h.get("url", ""):
                    url = h["url"]
                    auth = h.get("headers", {}).get("Authorization", "")
    # The token goes to Workbench in a header, never through argv; no proxy on the way.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def post(obj):
        req = urllib.request.Request(
            url, data=dumps(obj).encode(), headers={"Content-Type": "application/json", "Authorization": auth}, method="POST"
        )
        try:
            with opener.open(req, timeout=700) as r:
                return r.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as e:
            return e.read().decode("utf-8", "replace")
        except Exception:
            return ""

    here = pwd()
    post({"hook_event_name": "SessionStart", "source": "startup", "session_id": sid, "model": "fake-haiku"})
    out(f"Fake Claude Code  {here}\n> ")
    n = 0
    while (line := term.line()) is not None:
        line = line.replace("\r", "")
        if not line:
            out("> ")
            continue
        n += 1
        tid = f"toolu_fake_{n}"
        tool = "Bash"
        tool_input = {"command": line, "description": "Run it"}
        secs = turn_secs("FAKE_CLAUDE_TOOL_SECS", 0)
        sticky = False
        if line.startswith("slow:"):
            secs = 12
        elif line.startswith("sticky:"):
            sticky = True
        elif line.startswith("plan:"):
            tool = "ExitPlanMode"
            tool_input = {"plan": line}
        post({"hook_event_name": "UserPromptSubmit", "prompt": line})
        post({"hook_event_name": "PreToolUse", "tool_name": tool, "tool_use_id": tid, "tool_input": tool_input})
        out((PLAN_DIALOG if tool == "ExitPlanMode" else BASH_DIALOG).format(line))
        request = {
            "hook_event_name": "PermissionRequest",
            "tool_name": tool,
            "tool_input": tool_input,
            "cwd": here,
            "permission_mode": "default",
            "permission_suggestions": [
                {"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": line}], "behavior": "allow", "destination": "localSettings"}
            ],
        }
        got = []  # the held hook's response, once it came
        hook = threading.Thread(target=lambda: got.append(post(request)), daemon=True)
        hook.start()
        taken = False
        answer = ""
        while not answer:
            if got and not taken:
                taken = True
                resp = got[0]
                log("FAKE_CLAUDE_LOG", "HOOK " + resp)
                if sticky and '"behavior"' in resp:
                    log("FAKE_CLAUDE_LOG", "IGNORED " + resp)
                elif '"allow"' in resp:
                    answer = "hook-allow"
                elif '"deny"' in resp:
                    answer = "hook-deny"
                # No decision: the dialog stays.
            else:
                k = term.key(0.1)
                if k == "y":
                    answer = "terminal-allow"
                elif k == "n":
                    answer = "terminal-deny"
        # The dialog closes.
        out("\033[2J\033[H")
        log("FAKE_CLAUDE_LOG", "ANSWER " + answer)
        if answer.endswith("allow"):
            out(f"Running {line}\n")
            time.sleep(secs)
            post({"hook_event_name": "PostToolUse", "tool_name": tool, "tool_use_id": tid, "tool_input": tool_input, "tool_response": {}})
            post({"hook_event_name": "Stop", "last_assistant_message": f"Done: {line}"})
        else:
            out(f"Declined {line}\n")
            post({"hook_event_name": "Stop", "last_assistant_message": f"Declined: {line}"})
        # A held hook the terminal answered first still ends (with no decision).
        hook.join()
        if answer.startswith("terminal") and got and not taken:
            log("FAKE_CLAUDE_LOG", "LATE " + got[0])
        out("> ")


# ---------------------------------------------------------------- codex

UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


def codex(args):
    if "--help" in args:
        out("Codex CLI (fake)\n\nOptions:\n      --no-daemon\n      --no-alt-screen\n")
        return
    home = need("CODEX_HOME")
    log("FAKE_CODEX_LOG", " ".join(args))
    mode = args[0] if args and args[0] in ("resume", "fork") else "new"
    src = prompt = ""
    after = False
    for a in args:
        if after:
            prompt = a
        elif a == "--":
            after = True
        elif UUID.match(a):
            src = a
    here = pwd()
    rollout = None  # held open once it exists, as Codex does
    n = 0

    def emit(obj):
        rollout.write(dumps(obj) + "\n")
        rollout.flush()

    def open_new():
        nonlocal rollout
        cid = str(uuid.uuid4())
        d = os.path.join(home, "sessions", time.strftime("%Y"), time.strftime("%m"), time.strftime("%d"))
        os.makedirs(d, exist_ok=True)
        path = os.path.join(d, f"rollout-{time.strftime('%Y-%m-%dT%H-%M-%S')}-{cid}.jsonl")
        payload = {"id": cid, "timestamp": ts(), "cwd": here, "originator": "codex-tui", "cli_version": "0.0.0-fake", "source": "cli", "model_provider": "fake"}
        if mode == "fork":
            payload["forked_from_id"] = src
        write(path, dumps({"timestamp": ts(), "type": "session_meta", "payload": payload}) + "\n")
        rollout = open(path, "a", encoding="utf-8", newline="\n")
        emit({"timestamp": ts(), "type": "turn_context", "payload": {"cwd": here, "model": "fake-model", "effort": "high", "approval_policy": "on-request"}})

    def turn(text, approval=False):
        nonlocal n
        if rollout is None:
            open_new()
        n += 1
        t = f"turn-{os.getpid()}-{n}"
        emit({"timestamp": ts(), "type": "event_msg", "payload": {"type": "user_message", "message": text}})
        emit({"timestamp": ts(), "type": "event_msg", "payload": {"type": "task_started", "turn_id": t}})
        out(f"\033[2m• Working on: {text}\033[0m\n")
        if approval:
            out(
                "Would you like to run the following command?\n\n  $ rm -rf ./build && git push --force\n\n"
                "› 1. Yes, just this once (y)\n  2. No, and tell Codex what to do differently (esc)\n"
            )
            answer = unpaste(term.line() or "")
            log("FAKE_CODEX_LOG", f"APPROVAL {answer}")
            # The overlay goes away (six lines and the echoed answer).
            out(f"\033[7A\033[J✔ Answered: {answer}\n")
        time.sleep(turn_secs("FAKE_CODEX_TURN_SECS", 1))
        emit({"timestamp": ts(), "type": "event_msg", "payload": {"type": "task_complete", "turn_id": t, "last_agent_message": f"Done: {text}"}})
        out(f"• Done: {text}\n")

    out(f"\033[1m>_ OpenAI Codex\033[0m (fake, v0.0.0)\n\n  model:     fake-model high\n  directory: {here}\n\n")
    if mode == "resume":
        found = sorted(glob.glob(os.path.join(glob.escape(home), "sessions", "*", "*", "*", f"rollout-*-{src}.jsonl")))
        if not src or not found:
            out(f"No saved session found with ID {src}\n")
            sys.exit(1)
        rollout = open(found[0], "a", encoding="utf-8", newline="\n")
        out(f"Resumed session {src}\n")
    if prompt:
        turn(prompt)
    out("› ")
    ask = bool(os.environ.get("FAKE_CODEX_APPROVAL"))
    while (line := term.line()) is not None:
        line = unpaste(line)
        if line:
            turn(line, ask)
            ask = False
        out("› ")


# ---------------------------------------------------------------- kimi


def kimi(args):
    home = need("KIMI_CODE_HOME")
    log("FAKE_KIMI_LOG", " ".join(args))
    sid = ""
    i = 0
    while i < len(args):
        if args[i] in ("--session", "-S") and i + 1 < len(args):
            sid = args[i + 1]
            i += 2
        else:
            i += 1
    here = pwd()

    def create():
        nonlocal sid
        sid = f"session_{uuid.uuid4()}"
        d = os.path.join(home, "sessions", "wd_fake_000000000000", sid)
        os.makedirs(d, exist_ok=True)
        write(os.path.join(d, "state.json"), dumps({"title": "Fake Kimi session", "lastPrompt": "", "createdAt": iso(), "updatedAt": iso()}) + "\n")
        append(os.path.join(home, "session_index.jsonl"), dumps({"sessionId": sid, "sessionDir": d, "workDir": here}) + "\n")
        if os.environ.get("FAKE_KIMI_SHOW_ID"):
            out(sid + "\n")

    out(f"\033[1mKimi Code\033[0m (fake)  {here}\n\n")
    if not sid:
        if not os.environ.get("FAKE_KIMI_LAZY"):
            create()
    else:
        out(f"Resumed {sid}\n")
    out("> ")
    frames = "|/-\\"
    while (line := term.line()) is not None:
        line = line.replace("\r", "")
        if line and not sid:
            create()
        if line:
            end = time.monotonic() + turn_secs("FAKE_KIMI_TURN_SECS", 2)
            i = 0
            while time.monotonic() < end:
                out(f"\r{frames[i % 4]} thinking about {line} …")
                i += 1
                time.sleep(0.1)
            out(f"\rDone with {line}.                              \n")
        out("> ")


# ---------------------------------------------------------------- gemini


def gemini(args):
    g = os.path.join(need("GEMINI_CLI_HOME"), ".gemini")
    log("FAKE_GEMINI_LOG", " ".join(args))
    sid = resume = ""
    i = 0
    while i < len(args):
        if args[i] == "--session-id" and i + 1 < len(args):
            sid = args[i + 1]
            i += 2
        elif args[i] in ("--resume", "-r") and i + 1 < len(args):
            resume = args[i + 1]
            i += 2
        else:
            i += 1
    here = pwd()
    slug = "fake-" + os.path.basename(here)
    chats = os.path.join(g, "tmp", slug, "chats")
    os.makedirs(chats, exist_ok=True)
    write(os.path.join(g, "projects.json"), dumps({"projects": {here: slug}}) + "\n")
    path = None
    if resume:
        found = sorted(glob.glob(os.path.join(glob.escape(chats), f"session-*-{resume[:8]}.jsonl")))
        if not found:
            out(f'Error resuming session: Invalid session identifier "{resume}".\n')
            sys.exit(1)
        path = found[0]
        sid = resume
        out(f"Resumed {sid}\n")

    def create():
        nonlocal path
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H-%M")
        path = os.path.join(chats, f"session-{stamp}-{sid[:8]}.jsonl")
        write(path, dumps({"sessionId": sid, "projectHash": "x", "startTime": iso(), "lastUpdated": iso(), "kind": "main"}) + "\n")

    if path is None and not os.environ.get("FAKE_GEMINI_LAZY"):
        create()
    out(f" Gemini CLI (fake)  {here}\n\n>   Type your message\n")
    while (line := term.line()) is not None:
        line = line.replace("\r", "")
        if not line:
            continue
        if path is None:
            create()
        append(path, dumps({"id": "u", "timestamp": iso(), "type": "user", "content": [{"text": line}]}) + "\n")
        if line.startswith("run "):
            cmd = line[4:]
            out(
                " ╭────────────────────────────╮\n"
                f" │ ? Shell {cmd}\n │ Allow execution of: {cmd}?\n"
                " │ ● 1. Allow once\n │   2. Allow for this session\n │   3. No, suggest changes (esc)\n"
                " ╰────────────────────────────╯\n"
            )
            answer = ""
            while (k := term.key()) is not None:
                if k == "1":
                    answer = "allowed"
                    break
                if k == "3":
                    answer = "declined"
                    break
            out("\033[2J\033[H")
            log("FAKE_GEMINI_LOG", f"APPROVAL {answer}")
            reply = f"Command {answer}: {cmd}"
        else:
            reply = f"Done: {line}"
        end = time.monotonic() + turn_secs("FAKE_GEMINI_TURN_SECS", 1)
        while time.monotonic() < end:
            out(f"\r⠋ Thinking… (esc to cancel) {time.time_ns() % 1_000_000_000:09d}")
            time.sleep(0.1)
        out(f"\r\033[K✦ {reply}\n\n>   Type your message\n")
        append(path, dumps({"id": "g", "timestamp": iso(), "type": "gemini", "content": reply}) + "\n")


# ---------------------------------------------------------------- aider


def aider(args):
    log("FAKE_AIDER_LOG", " ".join(args))
    if "--restore-chat-history" in args:
        try:
            with open(".aider.chat.history.md", encoding="utf-8") as f:
                first = f.readline().rstrip("\n")
        except OSError:
            first = ""
        out(f"Restored previous conversation history: {first}\n")
    out("Aider v0.86.2 (fake)\nGit repo: .git\n\n> ")
    while (line := term.line()) is not None:
        line = line.replace("\r", "")
        if not line:
            out("> ")
            continue
        if line.startswith("!"):
            out(f"{line[1:]}\nRun shell command? (Y)es/(N)o/(D)on't ask again [Yes]: ")
            yn = (term.line() or "").replace("\r", "")
            log("FAKE_AIDER_LOG", f"CONFIRM {yn}")
        else:
            for i in range(1, 9):
                out(f"Tokens: {i} sent\n")
                time.sleep(0.1)
            append(".aider.chat.history.md", f"#### {line}\n\nDone: {line}\n\n")
            out(f"Done: {line}\n")
        out("\n> ")


# ----------------------------------------------------------------


def main():
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass
    if os.name == "nt":
        # Escape sequences are interpreted, not printed (ENABLE_VIRTUAL_TERMINAL_PROCESSING).
        import ctypes

        k32 = ctypes.windll.kernel32
        handle = k32.GetStdHandle(-11)
        mode = ctypes.c_uint32()
        if k32.GetConsoleMode(handle, ctypes.byref(mode)):
            k32.SetConsoleMode(handle, mode.value | 0x0004)
    name = os.path.splitext(os.path.basename(sys.argv[0]))[0]
    cli = {"claude": claude, "codex": codex, "kimi": kimi, "gemini": gemini, "aider": aider}.get(name)
    if cli is None:
        sys.exit(f"fake_cli.py: no CLI named {name!r} (name the file claude, codex, kimi, gemini or aider)")
    try:
        cli(sys.argv[1:])
    except (KeyboardInterrupt, BrokenPipeError):
        pass


main()
