"""A fake debug server (OpenOCD, J-Link GDB Server…) for Workbench's tests.

It listens on --port like a gdb stub (and on --also <port> for the telnet port other
servers open) after --delay seconds, and says so on stdout; it notes a client that
connects (a readiness probe must not be one). --fail <text> prints the text to stderr
and exits 1 before listening, --die-after <s> exits 7 later, --ignore-term makes it
shrug off SIGTERM, --pidfile <path> records its pid. --channel PORT=text:<text> (or
PORT=hex:<hex>) serves that output to every client that connects to PORT (`\\n` is a newline),
like an RTT or UART port; it may be given more than once. --say <text> prints a line to
stderr at once (`\\n` separates lines; repeatable), --chatter <interval>:<count>:<text> prints
<text> <count> times <interval> seconds apart once it listens (what a real server prints while
its probe stops answering).
"""
import os
import signal
import socket
import sys
import threading
import time

args = sys.argv[1:]
opt = {}
flags = set()
channels = []
says = []
chatter = []
i = 0
while i < len(args):
    if args[i] == "--ignore-term":
        flags.add("ignore-term")
        i += 1
    elif args[i] == "--channel":
        channels.append(args[i + 1])
        i += 2
    elif args[i] == "--say":
        says.append(args[i + 1])
        i += 2
    elif args[i] == "--chatter":
        chatter.append(args[i + 1])
        i += 2
    else:
        opt[args[i]] = args[i + 1] if i + 1 < len(args) else ""
        i += 2

if opt.get("--pidfile"):
    with open(opt["--pidfile"], "w") as f:
        f.write(str(os.getpid()))
if "ignore-term" in flags and hasattr(signal, "SIGTERM"):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
print("fake server starting", flush=True)
sys.stderr.write("fake server note\n")
for text in says:
    sys.stderr.write(text.replace("\\n", "\n") + "\n")
sys.stderr.flush()
if "--fail" in opt:
    sys.stderr.write(opt["--fail"] + "\n")
    sys.exit(1)
time.sleep(float(opt.get("--delay", "0")))


def serve(port):
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", int(port)))
    s.listen(5)
    print("listening on %s" % port, flush=True)
    while True:
        c, _ = s.accept()
        print("connection from a client", flush=True)
        c.close()


def serve_channel(spec):
    port, _, rest = spec.partition("=")
    kind, _, data = rest.partition(":")
    payload = bytes.fromhex(data) if kind == "hex" else data.replace("\\n", "\n").encode()
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", int(port)))
    s.listen(5)
    print("channel on %s" % port, flush=True)
    while True:
        c, _ = s.accept()
        c.sendall(payload)
        threading.Thread(target=lambda c=c: [None for _ in iter(lambda: c.recv(1024), b"")], daemon=True).start()


for spec in channels:
    threading.Thread(target=serve_channel, args=(spec,), daemon=True).start()
for key in ("--port", "--also"):
    if key in opt:
        threading.Thread(target=serve, args=(opt[key],), daemon=True).start()
def talk(spec):
    interval, count, text = spec.split(":", 2)
    for _ in range(int(count)):
        time.sleep(float(interval))
        sys.stderr.write(text + "\n")
        sys.stderr.flush()


for spec in chatter:
    threading.Thread(target=talk, args=(spec,), daemon=True).start()
if "--die-after" in opt:
    time.sleep(float(opt["--die-after"]))
    sys.exit(7)
while True:
    time.sleep(1)
