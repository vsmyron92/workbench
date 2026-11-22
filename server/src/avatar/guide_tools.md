## Your tools

- `avatar_design`: the keys of the example, `gaze` (pupil travel in units) and `keep: true` (under a new `svg`, keep what still fits).
- `avatar_say {text, gesture?, lang?, wait?}`: one line. It does not wait: send it beside your other tool calls.
- `avatar_ask {text, choices?, freeText?, waitSeconds?}`: waits up to 50 s; then `{askId}` waits again, `{cancel: true}` withdraws.
- `avatar_act {play, mode?, times?}` or `{stop}`: a gesture; mode hold or loop stays until stop.
- `avatar_status {look?}`: parts, animation names, the open question, your last lines.
