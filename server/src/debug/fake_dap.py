"""A fake Debug Adapter Protocol server for Workbench's tests.

It speaks DAP on stdio like gdb does: `launch` (or `attach`) is answered only
after `configurationDone`, then the "debuggee" stops at the first verified
breakpoint. Lines >= 100 cannot hold a breakpoint. `x` starts at 41. `continue`
runs to the next breakpoint below the current line, else prints "bye" and exits
with 3. A `TOKEN` in the launch env is what the program "read": a variable `tok`,
the expression `tok`, and the stop's text. Evaluating `crash()` makes the adapter
die (exit code 9) after a line on stderr. Every request is appended to $FAKE_LOG as
a JSON line (a child connection's with `"_child": true`).
"""
import json
import os
import socket
import sys
import threading

inp = sys.stdin.buffer
out = sys.stdout.buffer
log = open(os.environ["FAKE_LOG"], "a") if os.environ.get("FAKE_LOG") else None
seq = 0


def send(obj):
    global seq
    seq += 1
    obj["seq"] = seq
    b = json.dumps(obj).encode()
    out.write(b"Content-Length: %d\r\n\r\n" % len(b) + b)
    out.flush()


def event(name, body=None):
    m = {"type": "event", "event": name}
    if body is not None:
        m["body"] = body
    send(m)


def respond(req, body=None, success=True, message=None):
    m = {"type": "response", "request_seq": req["seq"], "command": req["command"], "success": success}
    if body is not None:
        m["body"] = body
    if message:
        m["message"] = message
    send(m)


def read(inp=inp):
    length = None
    while True:
        line = inp.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    return json.loads(inp.read(length))


def child_server(listener):
    """The adapter's listener for a subprocess: the child session connects here
    (like debugpy's `configuration.connect`) and attaches."""
    conn, _ = listener.accept()
    rf = conn.makefile("rb")
    cseq = [0]
    lock = threading.Lock()

    def csend(obj):
        with lock:
            cseq[0] += 1
            obj["seq"] = cseq[0]
            b = json.dumps(obj).encode()
            conn.sendall(b"Content-Length: %d\r\n\r\n" % len(b) + b)

    attach = None
    while True:
        m = read(rf)
        if m is None:
            break
        if log:
            log.write(json.dumps({**m, "_child": True}) + "\n")
            log.flush()
        cmd = m.get("command")
        ok = {"type": "response", "request_seq": m["seq"], "command": cmd, "success": True}
        if cmd == "initialize":
            csend({**ok, "body": {"supportsConfigurationDoneRequest": True}})
            csend({"type": "event", "event": "initialized"})
        elif cmd == "attach":
            attach = m
        elif cmd == "setBreakpoints":
            csend({**ok, "body": {"breakpoints": [{"verified": True, "line": b["line"]} for b in m["arguments"].get("breakpoints", [])]}})
        elif cmd == "configurationDone":
            csend(ok)
            if attach:
                csend({"type": "response", "request_seq": attach["seq"], "command": "attach", "success": True})
            csend({"type": "event", "event": "stopped", "body": {"reason": "breakpoint", "threadId": 7, "allThreadsStopped": True}})
        elif cmd == "threads":
            csend({**ok, "body": {"threads": [{"id": 7, "name": "subprocess main"}]}})
        elif cmd == "disconnect":
            csend(ok)
            break
        else:
            csend({**ok, "body": {}})
    conn.close()


sys.stderr.write("fake adapter ready\n")
sys.stderr.flush()
out.write(b"fake adapter banner (not DAP)\n")
out.flush()

def token():
    return (((launch or {}).get("arguments") or {}).get("env") or {}).get("TOKEN")


bps = {}
ids = {}
launch = None
x = 41
cur = None
next_id = 100

while True:
    m = read()
    if m is None:
        break
    if log:
        log.write(json.dumps(m) + "\n")
        log.flush()
    if m["type"] == "response":
        continue
    cmd = m["command"]
    a = m.get("arguments") or {}
    if cmd == "initialize":
        respond(m, {
            "supportsConfigurationDoneRequest": True,
            "supportsConditionalBreakpoints": True,
            "supportsLogPoints": True,
            "supportsFunctionBreakpoints": True,
            "supportsSetVariable": True,
            "supportsTerminateRequest": True,
            "supportsCompletionsRequest": True,
            "exceptionBreakpointFilters": [{"filter": "throw", "label": "C++ throw", "default": True}, {"filter": "catch", "label": "C++ catch"}],
        })
        event("initialized")
    elif cmd in ("launch", "attach"):
        launch = m
        if a.get("wantChildConnect"):
            listener = socket.socket()
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            threading.Thread(target=child_server, args=(listener,), daemon=True).start()
            send({"type": "request", "command": "startDebugging", "arguments": {
                "request": "attach", "configuration": {"name": "fake subprocess", "subProcessId": 99,
                                                       "connect": {"host": "127.0.0.1", "port": listener.getsockname()[1]}}}})
        if a.get("wantChild"):
            send({"type": "request", "command": "startDebugging", "arguments": {
                "request": "launch", "configuration": {"name": "fake child", "program": a.get("program"), "childOf": a.get("name")}}})
        if a.get("wantTerminal"):
            send({"type": "request", "command": "runInTerminal", "arguments": {
                "kind": "integrated", "title": "fake debuggee", "cwd": a.get("cwd", "/"),
                "args": ["sh", "-c", "echo debuggee-in-terminal; sleep 30"], "env": {"FAKE_ENV": "1"}}})
    elif cmd == "setBreakpoints":
        path = a["source"]["path"]
        bps[path] = []
        res = []
        for b in a.get("breakpoints", []):
            next_id += 1
            ok = b["line"] < 100
            res.append({"id": next_id, "verified": ok, "line": b["line"], **({} if ok else {"message": "no code at this line"})})
            if ok:
                bps[path].append((b["line"], next_id))
        respond(m, {"breakpoints": res})
    elif cmd == "setFunctionBreakpoints":
        respond(m, {"breakpoints": [{"id": 900 + i, "verified": True} for i, _ in enumerate(a.get("breakpoints", []))]})
    elif cmd == "setExceptionBreakpoints":
        respond(m, {})
    elif cmd == "configurationDone":
        respond(m)
        if launch is not None:
            respond(launch)
        event("process", {"name": a.get("program", "fake"), "isLocalProcess": True, "startMethod": "launch"})
        event("thread", {"reason": "started", "threadId": 1})
        event("output", {"category": "stdout", "output": "hello from the debuggee\n"})
        first = None
        for path, lines in bps.items():
            for line, bid in lines:
                if first is None or line < first[1]:
                    first = (path, line, bid)
        if first:
            cur = first
            stop = {"reason": "breakpoint", "threadId": 1, "allThreadsStopped": True, "hitBreakpointIds": [first[2]]}
            if token():
                stop["text"] = "the token is " + token()
            event("stopped", stop)
        else:
            event("exited", {"exitCode": 0})
            event("terminated")
    elif cmd == "threads":
        respond(m, {"threads": [{"id": 1, "name": "main"}]})
    elif cmd == "stackTrace":
        la = (launch or {}).get("arguments") or {}
        frames = [
            {"id": 1000, "name": "work", "line": cur[1], "column": 1, "source": {"name": os.path.basename(cur[0]), "path": cur[0]}},
            {"id": 1001, "name": "main", "line": 3, "column": 1, "source": {"name": "stdio.h", "path": la.get("libSource", "/usr/include/stdio.h")}},
        ]
        if la.get("privateSource"):
            frames.append({"id": 1002, "name": "_start", "line": 1, "column": 1, "source": {"name": "x", "path": la["privateSource"]}})
        respond(m, {"stackFrames": frames, "totalFrames": len(frames)})
    elif cmd == "scopes":
        respond(m, {"scopes": [
            {"name": "Locals", "variablesReference": 1, "expensive": False, "presentationHint": "locals"},
            {"name": "Registers", "variablesReference": 3, "expensive": False, "presentationHint": "registers"},
        ]})
    elif cmd == "variables":
        ref = a["variablesReference"]
        if ref == 1:
            v = [{"name": "x", "value": str(x), "type": "int", "variablesReference": 0},
                 {"name": "p", "value": "{...}", "type": "struct point", "variablesReference": 2}]
            if token():
                v.append({"name": "tok", "value": '"%s"' % token(), "type": "const char *", "variablesReference": 0})
        elif ref == 2:
            v = [{"name": "y", "value": "7", "type": "int", "variablesReference": 0}]
        else:
            v = [{"name": "rip", "value": "0x1", "variablesReference": 0}]
        respond(m, {"variables": v})
    elif cmd == "evaluate":
        e = a["expression"]
        if e == "x":
            respond(m, {"result": str(x), "type": "int", "variablesReference": 0})
        elif e == "tok":
            respond(m, {"result": '"%s"' % token(), "type": "const char *", "variablesReference": 0})
        elif e == "crash()":
            sys.stderr.write("fatal: simulated crash\n")
            sys.stderr.flush()
            os._exit(9)
        elif e == "boom":
            respond(m, success=False, message='No symbol "boom" in current context.')
        elif e == "call_stop()":
            # Like gdb when a called function hits a breakpoint.
            event("stopped", {"reason": "breakpoint", "threadId": 1, "allThreadsStopped": True})
            respond(m, success=False, message="The program being debugged stopped while in a function called from GDB.")
        else:
            respond(m, {"result": "eval:" + e, "variablesReference": 0})
    elif cmd == "completions":
        respond(m, {"targets": [{"label": "xylophone"}]})
    elif cmd == "setVariable":
        x = int(a["value"])
        respond(m, {"value": str(x), "type": "int"})
    elif cmd == "next":
        respond(m)
        cur = (cur[0], cur[1] + 1, None)
        event("stopped", {"reason": "step", "threadId": 1, "allThreadsStopped": True})
    elif cmd == "continue":
        respond(m, {"allThreadsContinued": True})
        later = [(line, bid) for line, bid in bps.get(cur[0], []) if line > cur[1]]
        if later:
            line, bid = min(later)
            cur = (cur[0], line, bid)
            event("stopped", {"reason": "breakpoint", "threadId": 1, "allThreadsStopped": True, "hitBreakpointIds": [bid]})
        else:
            event("output", {"category": "stdout", "output": "bye\n"})
            event("exited", {"exitCode": 3})
            event("terminated")
    elif cmd == "terminate":
        respond(m)
        # Like gdb: the killed program "exits" with 0.
        event("exited", {"exitCode": 0})
        event("terminated")
    elif cmd == "disconnect":
        respond(m)
        break
    else:
        respond(m, success=False, message="unsupported " + cmd)
