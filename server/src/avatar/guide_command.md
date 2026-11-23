## Your commands

    {wb} avatar design look.json
    {wb} avatar say 'One short line.' --gesture hop
    {wb} avatar ask 'Deploy now?' --choice Yes --choice 'Not yet' --wait 60
    {wb} avatar act hop --mode hold
    {wb} avatar status --look
    {wb} avatar picture --frames hop

Bodies are files: never put JSON or artwork into a shell string. design takes the keys of the example, `gaze` and `keep` (`-` is standard input; a file that starts with < is the artwork alone). say also takes --lang and --wait. ask prints JSON and waits up to 110 s; then `ask --again <askId>` or `ask --cancel`. act also takes --times; `act --stop <name|all>` ends a held one. picture writes a PNG and prints where: open it and look, after every design. Alone it is the avatar at rest, `--frames <animation> [--count n]` its moments, `--pose pose.json` a pose (with --frames: its inputs). Exit status 1: refused (the reason is on stderr).
<!-- dev container -->
## Your commands

The calls are HTTP. C stands for

    printf 'Authorization: Bearer %s\n' "$WORKBENCH_AGENT_TOKEN" | curl -sS --fail-with-body -H @-

and U for `$WORKBENCH_URL/api/hooks/avatar/$WORKBENCH_TERMINAL_ID`.

    C --data-binary @look.json "U/design"
    C -H 'Content-Type: text/plain' --data-binary 'One short line.' "U/say?gesture=hop"
    C --data-binary @ask.json "U/ask"
    C --data-binary @act.json "U/act"
    C "U/status?look=1"
    C --data-binary @picture.json -o "${TMPDIR:-/tmp}/avatar-$WORKBENCH_TERMINAL_ID.png" "U/picture"

Bodies are files: never put JSON into a shell string. design: the keys of the example, `gaze` and `keep`, or the artwork alone. ask: {"text", "choices", "freeText", "waitSeconds"}, up to 110 s; then {"askId"} or {"cancel": true}. act: {"play", "mode", "times"} or {"stop"}. picture answers a PNG to open and look at, after every design: {} the avatar at rest, {"frames": "<animation>", "count"} its moments, {"pose": {…}} a pose (both: the moments under the pose's inputs). A refusal is written into that file as JSON: when C fails, print the file. Without curl: python3 urllib.request with that header from os.environ.
