#!/usr/bin/env python3
"""A small language server for Workbench's tests.

Documents are plain text; `def NAME` defines a symbol, any word is a usage, and a line
containing ERROR gets an error diagnostic (WARN a warning; LINK an info whose message
and related information are the file URI of $FAKE_LS_EXTERNAL). Custom requests:
`fake/state` (what the server saw), `fake/slow` (answers after 10 s unless cancelled),
`fake/crash` (exits with code 3), `fake/applyEdit` (asks the client to apply an edit and
returns its answer). The word `external` is defined in $FAKE_LS_EXTERNAL (a file outside
the project); completion also offers that file's URI as a string literal.
$FAKE_LS_NO_VERSION: publishDiagnostics without `version` (like typescript-language-server).
$FAKE_LS_NO_WORKSPACE_SYMBOL: no `workspaceSymbolProvider` (like Verible).
"""

import json
import os
import re
import sys
import threading
import time
from pathlib import Path

out_lock = threading.Lock()
docs = {}  # uri -> {"text", "version"}
state = {"config": None, "watched": [], "cancelled": [], "initOptions": None, "root": None, "saved": []}
pending_client = {}  # our request id -> threading.Event, result
cancel_events = {}
next_id = [1000]

if os.environ.get("FAKE_LS_CRASH_ON_START"):
    sys.stderr.write("crashing on start as asked\n")
    sys.stderr.flush()
    sys.exit(7)


def file_uri(path):
    """The file URI of an absolute path (file:///C:/... on Windows)."""
    return Path(path).as_uri()


def send(msg):
    body = json.dumps(msg).encode()
    with out_lock:
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()


def respond(id_, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": id_}
    if error is not None:
        msg["error"] = error
    else:
        msg["result"] = result
    send(msg)


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


def ask(method, params, timeout=10):
    next_id[0] += 1
    rid = next_id[0]
    ev = threading.Event()
    pending_client[rid] = [ev, None]
    send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
    ev.wait(timeout)
    return pending_client.pop(rid, [None, None])[1]


def read_message():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            if length is not None:
                break
            continue
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(length))


def word_at(uri, pos):
    d = docs.get(uri)
    if not d:
        return None
    lines = d["text"].split("\n")
    if pos["line"] >= len(lines):
        return None
    line = lines[pos["line"]]
    for m in re.finditer(r"[A-Za-z0-9_]+", line):
        if m.start() <= pos["character"] <= m.end():
            return m.group(0)
    return None


def occurrences(word):
    for uri, d in docs.items():
        for i, line in enumerate(d["text"].split("\n")):
            for m in re.finditer(r"\b%s\b" % re.escape(word), line):
                yield uri, i, m.start(), m.end()


def rng(line, a, b):
    return {"start": {"line": line, "character": a}, "end": {"line": line, "character": b}}


def publish(uri):
    d = docs.get(uri)
    diags = []
    if d:
        for i, line in enumerate(d["text"].split("\n")):
            if "ERROR" in line:
                c = line.index("ERROR")
                diags.append({"range": rng(i, c, c + 5), "severity": 1, "source": "fake", "code": "E1", "message": "error here"})
            if "WARN" in line:
                c = line.index("WARN")
                diags.append({"range": rng(i, c, c + 4), "severity": 2, "source": "fake", "message": "warning here"})
            if "LINK" in line and os.environ.get("FAKE_LS_EXTERNAL"):
                # A message that is a file URI (text), related information that points
                # outside the project (a location), and data the server wants back as is.
                c = line.index("LINK")
                lit = file_uri(os.environ["FAKE_LS_EXTERNAL"])
                diags.append({"range": rng(i, c, c + 4), "severity": 3, "source": "fake", "message": lit,
                              "relatedInformation": [{"location": {"uri": lit, "range": rng(0, 4, 12)}, "message": lit}],
                              "data": {"uri": uri}})
    msg = {"uri": uri, "diagnostics": diags}
    if not os.environ.get("FAKE_LS_NO_VERSION"):
        msg["version"] = d["version"] if d else None
    notify("textDocument/publishDiagnostics", msg)


def slow(id_):
    ev = threading.Event()
    cancel_events[id_] = ev
    if ev.wait(10):
        respond(id_, error={"code": -32800, "message": "cancelled"})
    else:
        respond(id_, "slow done")
    cancel_events.pop(id_, None)


def handle(msg):
    method = msg.get("method")
    id_ = msg.get("id")
    params = msg.get("params") or {}
    if method is None:
        # A response to one of our requests.
        entry = pending_client.get(id_)
        if entry:
            entry[1] = msg.get("result", msg.get("error"))
            entry[0].set()
        return
    if method == "initialize":
        state["initOptions"] = params.get("initializationOptions")
        state["root"] = params.get("rootUri")
        respond(id_, {
            "capabilities": {
                "textDocumentSync": {"openClose": True, "change": 1, "save": {"includeText": False}},
                "hoverProvider": True,
                "definitionProvider": True,
                "referencesProvider": True,
                "renameProvider": {"prepareProvider": False},
                "completionProvider": {"triggerCharacters": ["."], "resolveProvider": True},
                "documentSymbolProvider": True,
                "workspaceSymbolProvider": not os.environ.get("FAKE_LS_NO_WORKSPACE_SYMBOL"),
            },
            "serverInfo": {"name": "fake-ls", "version": "1.0"},
        })
    elif method == "initialized":
        def after():
            sys.stderr.write("fake-ls initialized\n")
            sys.stderr.flush()
            ask("client/registerCapability", {"registrations": [{
                "id": "w1", "method": "workspace/didChangeWatchedFiles",
                "registerOptions": {"watchers": [{"globPattern": "**/*.fk"}, {"globPattern": "**/*.txt", "kind": 4}]},
            }]})
            state["config"] = ask("workspace/configuration", {"items": [{"section": "fake"}, {"section": "fake.nested"}, {}]})
            ask("window/workDoneProgress/create", {"token": "idx"})
            notify("$/progress", {"token": "idx", "value": {"kind": "begin", "title": "Indexing", "percentage": 0}})
            notify("$/progress", {"token": "idx", "value": {"kind": "report", "percentage": 50}})
            notify("$/progress", {"token": "idx", "value": {"kind": "end"}})
            notify("window/logMessage", {"type": 3, "message": "fake-ls ready"})
        threading.Thread(target=after, daemon=True).start()
    elif method == "textDocument/didOpen":
        td = params["textDocument"]
        docs[td["uri"]] = {"text": td["text"], "version": td["version"]}
        publish(td["uri"])
    elif method == "textDocument/didChange":
        td = params["textDocument"]
        docs[td["uri"]] = {"text": params["contentChanges"][-1]["text"], "version": td["version"]}
        publish(td["uri"])
    elif method == "textDocument/didClose":
        docs.pop(params["textDocument"]["uri"], None)
    elif method == "textDocument/didSave":
        state["saved"].append(params["textDocument"]["uri"])
    elif method == "workspace/didChangeWatchedFiles":
        state["watched"].extend(params.get("changes", []))
    elif method == "$/cancelRequest":
        state["cancelled"].append(params.get("id"))
        ev = cancel_events.get(params.get("id"))
        if ev:
            ev.set()
    elif method == "textDocument/hover":
        uri = params["textDocument"]["uri"]
        w = word_at(uri, params["position"])
        if not w:
            respond(id_, None)
        else:
            respond(id_, {"contents": {"kind": "markdown", "value": "**%s** (v%d)" % (w, docs[uri]["version"])}})
    elif method == "textDocument/definition":
        w = word_at(params["textDocument"]["uri"], params["position"])
        ext = os.environ.get("FAKE_LS_EXTERNAL")
        if w == "external" and ext:
            respond(id_, [{"uri": file_uri(ext), "range": rng(0, 4, 12)}])
            return
        locs = []
        for uri, d in docs.items():
            for i, line in enumerate(d["text"].split("\n")):
                m = re.search(r"\bdef (%s)\b" % re.escape(w or "\0"), line)
                if m:
                    locs.append({"uri": uri, "range": rng(i, m.start(1), m.end(1))})
        respond(id_, locs)
    elif method == "textDocument/references":
        w = word_at(params["textDocument"]["uri"], params["position"])
        respond(id_, [{"uri": u, "range": rng(l, a, b)} for u, l, a, b in occurrences(w)] if w else [])
    elif method == "textDocument/rename":
        w = word_at(params["textDocument"]["uri"], params["position"])
        changes = {}
        for u, l, a, b in occurrences(w or "\0"):
            changes.setdefault(u, []).append({"range": rng(l, a, b), "newText": params["newName"]})
        respond(id_, {"changes": changes})
    elif method == "textDocument/completion":
        items = [
            {"label": "alpha", "kind": 3, "data": {"uri": params["textDocument"]["uri"]}},
            {"label": "beta", "kind": 6, "insertText": "beta($1)", "insertTextFormat": 2},
        ]
        ext = os.environ.get("FAKE_LS_EXTERNAL")
        if ext:
            # A string literal that happens to be a file URI (TypeScript offers these for
            # literal union types): text, not a location.
            lit = file_uri(ext)
            p = params["position"]
            items.append({"label": lit, "kind": 21, "detail": lit, "filterText": lit, "sortText": lit,
                          "textEdit": {"range": rng(p["line"], p["character"], p["character"]), "newText": lit},
                          "documentation": {"kind": "plaintext", "value": lit},
                          "command": {"title": "t", "command": "fake.cmd", "arguments": [lit]}})
        respond(id_, {"isIncomplete": False, "items": items})
    elif method == "completionItem/resolve":
        item = dict(params)
        item["documentation"] = {"kind": "markdown", "value": "docs for " + item["label"]}
        respond(id_, item)
    elif method == "textDocument/documentSymbol":
        d = docs.get(params["textDocument"]["uri"], {"text": ""})
        syms = []
        for i, line in enumerate(d["text"].split("\n")):
            m = re.search(r"\bdef (\w+)", line)
            if m:
                r = rng(i, m.start(1), m.end(1))
                syms.append({"name": m.group(1), "kind": 12, "range": r, "selectionRange": r})
        respond(id_, syms)
    elif method == "workspace/symbol":
        q = params.get("query", "")
        out = []
        for uri, d in docs.items():
            for i, line in enumerate(d["text"].split("\n")):
                m = re.search(r"\bdef (\w+)", line)
                if m and q in m.group(1):
                    out.append({"name": m.group(1), "kind": 12, "location": {"uri": uri, "range": rng(i, m.start(1), m.end(1))}})
        respond(id_, out)
    elif method == "fake/state":
        respond(id_, {**state, "docs": {u: d["version"] for u, d in docs.items()}, "texts": {u: d["text"] for u, d in docs.items()}})
    elif method == "fake/slow":
        threading.Thread(target=slow, args=(id_,), daemon=True).start()
    elif method == "fake/crash":
        os._exit(3)
    elif method == "fake/applyEdit":
        def go():
            uri = next(iter(docs), None)
            r = ask("workspace/applyEdit", {"label": "fake", "edit": {"changes": {uri: [{"range": rng(0, 0, 0), "newText": "// edited\n"}]}}}, 20)
            respond(id_, r)
        threading.Thread(target=go, daemon=True).start()
    elif method == "shutdown":
        respond(id_, None)
    elif method == "exit":
        sys.exit(0)
    elif id_ is not None:
        respond(id_, error={"code": -32601, "message": "unknown method " + method})


def main():
    while True:
        msg = read_message()
        if msg is None:
            return
        handle(msg)


if __name__ == "__main__":
    main()
