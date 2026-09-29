#!/usr/bin/env python3
# A stand-in for the docker CLI, for the Services tests on every OS (devcontainer/services.rs):
# it appends each call's arguments to `calls` and answers from the canned files beside it
# (`ps.txt`, `container.json`, `images.txt`): one compose container and a stray one, two
# images. The tests copy it as `docker` (as `docker.py` on Windows, started through an
# npm-style shim). Python 3's standard library only.

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ARGS = sys.argv[1:]
LINE = " ".join(ARGS)

with open(os.path.join(HERE, "calls"), "a", encoding="utf-8", newline="\n") as f:
    f.write(LINE + "\n")

# Bytes as they are: no `\r\n` on Windows.
out = sys.stdout.buffer


def cat(name):
    with open(os.path.join(HERE, name), "rb") as f:
        out.write(f.read())


def fail(message):
    out.flush()
    sys.stderr.write(message + "\n")
    sys.exit(1)


command = ARGS[0] if ARGS else ""
if command == "ps":
    if "--filter" in LINE:
        pass
    elif "--quiet" in LINE:
        with open(os.path.join(HERE, "container.json"), encoding="utf-8") as f:
            out.write((json.load(f)[0]["Id"] + "\n").encode())
    else:
        cat("ps.txt")
elif command == "inspect":
    if "--format" in LINE:
        out.write(b"sha256:img1\t/shop-web-1\n")
    elif "--type container" in LINE:
        cat("container.json")
    else:
        out.write(b"[]\n")
        fail("Error: No such image")
elif command == "images":
    cat("images.txt")
elif command == "rm":
    if "--force" not in LINE:
        fail(
            'Error response from daemon: cannot remove container "/shop-web-1": container is running: '
            "stop the container before removing or force remove"
        )
elif command == "image":
    out.write(b"Total reclaimed space: 12MB\n")
# Anything else (stop, compose, rmi, logs, exec…) succeeds silently.
