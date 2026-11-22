# Designing the avatar

The guide that `avatar_status` with `guide: true` gives (the guide command without the tools) is the authority on what a design may hold. This file explains how to apply it well. Where the two differ, follow the guide, and `avatar_status` lists the limits under `limits`.

## What reads at 28 to 132 px

The figure is drawn in a square of 56 px at rest and 132 px while it is active. In the agents window's top bar and in the phone's header it is a face of 28 to 40 px: a square around the head role's box with a tenth added on each side, in its rest pose. With a viewBox of 0 0 200 200, one unit is 0.66 px at 132 px and 0.28 px at 56 px; in a face of a head 80 units wide it is about a quarter of a pixel. A 6-unit detail is 4 px at 132 px, and under 2 px at rest and in a face.

- Design the silhouette for 56 px: a head of a third to a half of the figure's height, eyes with whites of about 10 units radius and pupils of half that, and a handful of large shapes. Nostrils, highlights and wing ribs are for 132 px; nothing may depend on them.
- The head must read alone, because it is the face. Keep the eyes and the mouth inside the head part. Wide horns or ears make the face's square larger and the eyes smaller in it.
- Nothing is drawn around the figure, no tile and no frame: it stands and walks on the user's panels, light or dark as their theme is, with only a thin line of the theme's text colour and the glow of its state around its outline. Give every large shape a dark outline (2.5 to 4 units) or use mid-tone fills, and do not make the main fill pure white or pure black. `currentColor` is the theme's text colour: it suits a small mark that must contrast on both themes.
- Use three or four flat colours and a darker shade of the main one. A darker shape underneath is a shadow; a radial gradient is a glow.
- Keep the resting figure inside 30..170 and leave room above it for your own jumps: what leaves the viewBox is cut off.

## The SVG subset

It is not the SVG you know. As the guide states it: only g, path, circle, ellipse, rect, line, polyline, polygon, text and, in defs, linearGradient, radialGradient and clipPath; attribute values in single quotes, so the JSON string needs no backslashes. Refused: script, style, image, a, use, mask, pattern, href, on… attributes and anything unknown. Dropped with a warning: filter, class, animate…, font-family, transform-origin. A stop stands inside a gradient, a gradient or a clipPath inside defs. Numbers stay within 4 times the viewBox, the pen of path data too; a numeric font-weight is 100, 200 … 900.

- Instead of `use`, repeat the shape. Mirror a left part for the right side as x to 200 - x, and keep the y values.
- Set paint on the group that holds the shapes (`fill`, `stroke`, `stroke-width`, `stroke-linejoin`) and override it on a shape: fewer attributes, fewer mistakes.
- A text element holds a few characters (a Z for sleeping), always in the app's font.

## The rig: ids, nesting, pivots, roles

- An id on every part that moves: a g, or one shape. An id, like an animation's name, starts with a letter, holds letters, digits, `_` and `-`, has at most 32 characters and is not `all`, `none` or a name every JavaScript object has, such as `constructor`; every id is used once.
- Nest what moves together: body > head > eyes > pupils. Document order is depth: what comes later is drawn in front, so wings and a tail that stand behind the body come before it inside the body group.
- A part that must stay put while the body jumps, such as a shadow on the ground, stands outside the body group.
- No transform attribute on a part you animate.
- A pivot for every joint: `pivots` maps a part id to [x, y], the point it turns and scales about, in the coordinates it is drawn in. Without one a part turns about the centre of its box, the body about the bottom centre, and a rotate or scale track on a g or a path without a pivot is accepted with a warning. Two pivots for one shape are two nested groups.
- `roles` maps body, head, eyes, pupils, armL and armR to part ids. Wings, fins, leaves or claws can be the arms.

From the roles alone Workbench plays its built-in motion, by the session's state:

| Role | What Workbench does with it | Where its pivot goes |
| --- | --- | --- |
| body | breathe (a slow scale), alert (a hop when the session needs the user or hit an error), doze with the eyes; talk, listen, nod and shake when there is no head; cheer (a hop, which the arms join) | the feet: the default bottom centre is right for a figure that stands |
| head | think when there are no pupils or `gaze` is 0, talk (a small nod while a line is presented), listen (a held tilt), nod, shake | the neck |
| eyes | blink (scaleY to 0.1 and back about the eyes' pivot), doze | the eyes' centre line: put only the eyes in this group |
| pupils | think (a drift up and aside, as far as `gaze`); they follow the pointer and look at the question while one is open | none needed |
| armL, armR | wave (one arm), cheer (with the body's hop, or both arms without a body) | the shoulder |

Workbench also plays wave once when a session's first design appears, and nod once when a reply of the user was delivered; while the figure walks about it hops, when it meets another avatar it waves and nods, and it cheers when a session near it finishes ([Walking and meeting](#walking-and-meeting)). `gaze` is how far the pupils travel, in units. Without it they travel 3% of the viewBox width (6 units in 200), more than the 5 that pupils of 5 have in whites of 10: set it to the white's radius minus the pupil's; 0 keeps them still. With roles and a mouth a design is alive already: add a few animations of your own, and do not redefine the built-in names.

## Your own animations

`animations` maps a name to `{duration, tracks}`: seconds, and tracks of `{target, prop, values}`. A target is a part id or a list of ids; the values are keyframes, evenly spaced or at `times` from 0 to 1; `ease` is linear, in, out, inOut, outBack or step, for all segments or one per segment. `times` and `ease` on the animation are the default of its tracks; a track with another count of values gives its own `times`.

- **Kinds.** `loop: true` repeats while its state lasts. `every: [min, max]` plays again after a random pause in that range of seconds (min at least 1, max at most 120). Neither makes a one-shot: a gesture, or what plays once when a state begins. `phase` (0 to 1) shifts a loop's track, `delay` (seconds, within the duration) starts a one-shot's track later; the other one is dropped with a warning.
- **Relative props.** translateX, translateY, rotate, scale, scaleX and scaleY are relative to rest: 0 is rest for translate and rotate, 1 for a scale. They add to the built-in motion, and null is refused in them.
- **Absolute props.** opacity, fill, stroke, strokeWidth, d and the geometry of a shape (cx, cy, r, rx, ry, x, y, width, height, x1, y1, x2, y2: only on the shapes that have them) are absolute. null is the artwork's own value, which each target must carry on the element itself, not on a group around it. An absolute track of an animation that does not loop (a one-shot or an `every` one) starts and ends at that value, else it jumps back at its end: start and end with null.
- **Paint.** fill, stroke and opacity keyframes stand 0.17 s apart or more, never ease step, and in a loop such a track ends on the value it starts with. Colours are plain: #rrggbb, rgb(), hsl() or a name, not none, currentColor or url(). Other tracks keep 0.04 s between keyframes.
- **d.** d keyframes use the same commands with the same count of numbers as each other and, with null or a mouth, as the element's own d.
- **Life as numbers.** Two equal keyframes hold a pose. A blink is scaleY 1, 0.1, 1 in 0.18 s. Loops of different durations never line up, which keeps an idle figure from looking mechanical. A gesture is anticipation, action, overshoot, settle: crouch, jump, hang, land, squash, rest.
- A part that only appears in an animation (smoke, a spark, a Z) is drawn at opacity 0 and its opacity track starts and ends at 0.

## Walking and meeting

In the desktop layout Workbench walks the figure about the workspace window while it has nothing to say: a stroll, a rest, another stroll, with a little hop and lean. Now and then it visits another session's avatar: the guest greets, the host answers, they look at each other for a few seconds, the guest nods, and they part. Avatars near one that speaks look at it, and those near one whose session finishes cheer. A figure stands still while it speaks, listens or asks, while it is the large one, while the session needs the user or failed, while a gesture of yours is held or looping, and for a pointer near it where it stands on nothing the user presses or types into; one whose session works strolls only a little and neither visits nor is visited. Where nothing is quiet (an editor or a terminal fills the window) the figures stand at home, in their column or where the user dropped them, and visit only each other there. Where the user switched walking off, or reduced motion, the figures stand in a column instead. Two names of your own take part:

- `walk`, declared `loop: true`, plays while the figure walks, in place of the built-in hop (the lean stays). Keep it in place, a bob of the body and a swing of the arms, legs or tail: Workbench moves the figure itself. Without `loop: true` it is not played.
- `greet` plays once when the figure greets another avatar or answers a greeting, in place of the built-in wave: a one-shot of a second or so that ends at rest.

Both are ordinary animations, and a call without `svg` adds them to a look that has none. For the dragon of the example below:

```json
{
  "animations": {
    "walk": { "duration": 0.6, "loop": true, "ease": "inOut", "tracks": [
      { "target": "body", "prop": "translateY", "values": [0, -4, 0] },
      { "target": "armL", "prop": "rotate", "values": [0, 7, 0] },
      { "target": "armR", "prop": "rotate", "values": [0, -7, 0] },
      { "target": "tail", "prop": "rotate", "values": [-6, 9, -6] } ] },
    "greet": { "duration": 1.2, "ease": "inOut", "tracks": [
      { "target": "armR", "prop": "rotate", "values": [0, -16, -4, -16, 0] },
      { "target": "tail", "prop": "rotate", "values": [0, 14, 0] } ] }
  }
}
```

## States, mouth, voice, name

- `states` maps a state (idle, working, speaking, listening, waiting, attention, error, done, sleeping) to up to 6 animation names, yours or built in, replacing that state's default. Repeat breathe and blink where you still want them; `[]` is stillness. Waiting means a question is open; attention means the session needs the user; done means it finished with output the user has not read. Leave attention and error to the built-in alert unless yours is as plain a call for the user.
- `mouth` moves only while a line is presented. `{target, open}` morphs a path from its own d to `open`, in the same structure. `{target, prop, open}` moves one number: scaleY, scaleX, scale, translateY, ry, r or height (for example `{"target": "mouth", "prop": "scaleY", "open": 1.5}` on a small ellipse, with a pivot on its centre line).
- `voice` is `{lang, prefer, pitch, rate}`: a language tag, a few pieces of browser voice names tried in order (`Natural`, `Female`, a voice's own name), pitch from 0.7 to 1.4 and rate from 0.8 to 1.3 (1 is the voice as it is). The voices are the user's browser's; the say answer names the one that spoke. Pitch sets your character apart from the other sessions' avatars.
- `name` is the avatar's own name; Workbench shows it in quotation marks after the session's title.

## Reading the design answer

You cannot see what you drew; the answer describes it. It begins with the revision and the name, then a line per part with an id: `id tag x y w h pivot x y`, in root coordinates, at rest, `(default)` where you gave no pivot. Then your animations, the usage against the limits, the built-in motion that applies, what a new svg kept or cleared, and the warnings. For the example below the parts are:

```text
shadow ellipse 70 164 60 6 pivot 100 167 (default)
body g 38 34 130 133 pivot 100 167
tail g 124 112 44 52 pivot 124 150
armL g 38 90 40 42 pivot 78 120
armR g 122 90 40 42 pivot 122 120
head g 60 34 80 79 pivot 100 112
eyes g 74 62 52 20 pivot 100 72 (default)
pupils g 80 68 42 10 pivot 101 73 (default)
mouth path 88 103 24 8 pivot 100 107 (default)
smoke circle 102 84 12 12 pivot 108 90 (default)
```

Check it as a drawing: the body's box holds the whole figure and stays inside 30..170; mirrored parts have mirrored boxes (armL's x and armR's x + w add up to 200); the eyes and the mouth lie inside the head, the mouth below the eyes; every pivot sits at its joint (the wings' at the shoulders, the head's at the neck, the eyes' on their centre line). A part far from where you meant it, or 0 wide, is a typo in its coordinates. `avatar_status` with `look: true` gives the stored design back in full when you need to change it later.

## Refusals a first design meets, and the repair

A refused call changes nothing and ends with "Nothing was changed." It lists every kind of problem at once, each with its count, its first place and what is accepted, so fix every line and send the whole call again. The usual ones, in the validator's words:

- `svg: <use> is not supported (… in g#armL): repeat the shape`. The same for style ("put attributes on the elements"), mask ("use a clipPath") and image ("draw with shapes").
- `svg: "cx" is not an attribute of <g>`: geometry belongs on the shape; paint may stand on the group.
- `target "arm_r" not found (at animations.flap.tracks[0]): ids are shadow, body, tail, armL, armR, …` (in document order): use the ids as the svg writes them.
- `null in a rotate track (…): … write 0, or 1 for a scale`.
- `null is the artwork's own fill, and wingL has no plain colour of its own (…)`: write the attribute on the element, or the colour in the keyframe.
- `the loop's opacity ends on another value than it starts with (…)`, `keyframes 1 and 2 of fill are 0.1 s apart (…)`, `ease step on opacity`: the paint rules above.
- `d values differ in structure: …` and `mouth open is …, the d of mouth is …`: copy the element's d and change only its numbers.
- `body is a g and has no cx (…): cx is a prop of circle and ellipse`: animate geometry on the shape, or use translate on the group.
- `no animation "wobble"` in a state, or `loop and every exclude each other`.
- `pivots without artwork: this session has no design yet, send svg in the same call`.

Warnings do not refuse: the call is saved and the answer ends with them. `… has no pivot: it turns and scales about the centre of its box`, `… moves body out of the viewBox`, `… does not start and end at its own value`, `Not drawn: filter (2)` and `smaller than 2 units` each say what will look wrong. Fix one that matters with your second call; without `svg` that call changes only the fields it sends (null removes an entry), with `svg` it is a new look and `keep: true` keeps what still fits. Then stop: design in at most two calls.

## A complete example

A small dragon, about 40 elements: wings as the arms, a tail and a puff of smoke of its own, a loop (`swish`, and `flutter` of another length), a recurring animation (`puff`) and a one-shot gesture (`takeoff`, with a delayed tail track). Draw your own character: this is the form to follow, not a look to copy.

```json
{
  "name": "Moss",
  "svg": "<svg viewBox='0 0 200 200'><ellipse id='shadow' cx='100' cy='167' rx='30' ry='3' fill='#000000' opacity='0.18'/><g id='body' stroke='#1f4f47' stroke-width='3' stroke-linejoin='round'><g id='tail' fill='none' stroke-linecap='round'><path d='M124 150 Q158 164 160 128' stroke-width='12'/><path d='M124 150 Q158 164 160 128' stroke='#3aa58f' stroke-width='7'/><polygon points='160,112 168,130 152,130' fill='#2c7f6e'/></g><g id='armL' fill='#2c7f6e'><path d='M78 120 L44 90 Q48 104 38 112 Q50 114 46 126 Q58 122 62 132 Z'/><path d='M76 120 L40 112 M76 120 L46 126' fill='none' stroke-width='2.5'/></g><g id='armR' fill='#2c7f6e'><path d='M122 120 L156 90 Q152 104 162 112 Q150 114 154 126 Q142 122 138 132 Z'/><path d='M124 120 L160 112 M124 120 L154 126' fill='none' stroke-width='2.5'/></g><ellipse cx='100' cy='134' rx='36' ry='30' fill='#3aa58f'/><ellipse cx='100' cy='140' rx='22' ry='20' fill='#f3e3b0' stroke='none'/><ellipse cx='82' cy='161' rx='12' ry='6' fill='#3aa58f'/><ellipse cx='118' cy='161' rx='12' ry='6' fill='#3aa58f'/><g id='head'><polygon points='74,58 66,34 88,50' fill='#f3e3b0'/><polygon points='126,58 134,34 112,50' fill='#f3e3b0'/><ellipse cx='100' cy='80' rx='40' ry='33' fill='#3aa58f'/><ellipse cx='68' cy='94' rx='6' ry='4' fill='#f29e9e' stroke='none'/><ellipse cx='132' cy='94' rx='6' ry='4' fill='#f29e9e' stroke='none'/><ellipse cx='100' cy='98' rx='22' ry='13' fill='#6cc7ad' stroke-width='2.5'/><g fill='#1f4f47' stroke='none'><ellipse cx='93' cy='93' rx='3' ry='2.4'/><ellipse cx='107' cy='93' rx='3' ry='2.4'/></g><g id='eyes' stroke-width='2'><circle cx='84' cy='72' r='10' fill='#ffffff'/><circle cx='116' cy='72' r='10' fill='#ffffff'/><g id='pupils' stroke='none'><circle cx='85' cy='73' r='5' fill='#1f4f47'/><circle cx='117' cy='73' r='5' fill='#1f4f47'/><circle cx='87' cy='70.5' r='1.8' fill='#ffffff'/><circle cx='119' cy='70.5' r='1.8' fill='#ffffff'/></g></g><path id='mouth' d='M88 103 Q100 109 112 103 Q100 111 88 103 Z' fill='#7a2a3c' stroke='none'/><circle id='smoke' cx='108' cy='90' r='6' fill='none' stroke='#9aa7ab' opacity='0'/></g></g></svg>",
  "pivots": { "body": [100, 167], "head": [100, 112], "tail": [124, 150], "armL": [78, 120], "armR": [122, 120] },
  "roles": { "body": "body", "head": "head", "eyes": "eyes", "pupils": "pupils", "armL": "armL", "armR": "armR" },
  "animations": {
    "swish": { "duration": 2.8, "loop": true, "ease": "inOut", "tracks": [
      { "target": "tail", "prop": "rotate", "values": [-6, 9, -6] } ] },
    "flutter": { "duration": 1.9, "loop": true, "ease": "inOut", "tracks": [
      { "target": "armL", "prop": "rotate", "values": [0, 7, 0] },
      { "target": "armR", "prop": "rotate", "values": [0, -7, 0] } ] },
    "puff": { "duration": 1.4, "every": [6, 14], "ease": "out", "tracks": [
      { "target": "smoke", "prop": "opacity", "values": [0, 0.9, 0], "times": [0, 0.25, 1] },
      { "target": "smoke", "prop": "translateX", "values": [0, 28] },
      { "target": "smoke", "prop": "translateY", "values": [0, -20] },
      { "target": "smoke", "prop": "scale", "values": [0.6, 1.5] } ] },
    "takeoff": { "duration": 1.2, "times": [0, 0.15, 0.4, 0.6, 0.85, 0.93, 1], "ease": "inOut", "tracks": [
      { "target": "body", "prop": "translateY", "values": [0, 5, -24, -20, 0, 3, 0] },
      { "target": "armL", "prop": "rotate", "values": [0, 10, -16, 14, -10, 4, 0] },
      { "target": "armR", "prop": "rotate", "values": [0, -10, 16, -14, 10, -4, 0] },
      { "target": "shadow", "prop": "scaleX", "values": [1, 1.06, 0.6, 0.65, 1, 1.04, 1] },
      { "target": "tail", "prop": "rotate", "values": [0, 14, -6, 0], "times": [0, 0.3, 0.7, 1], "delay": 0.1 } ] }
  },
  "states": {
    "idle": ["breathe", "blink", "swish", "flutter", "puff"],
    "working": ["breathe", "think", "blink", "swish"],
    "done": ["breathe", "blink", "takeoff"]
  },
  "mouth": { "target": "mouth", "open": "M88 102 Q100 106 112 102 Q100 122 88 102 Z" },
  "gaze": 3,
  "voice": { "lang": "en-GB", "prefer": ["Natural", "Female"], "pitch": 1.1, "rate": 1.05 }
}
```

What it shows: paint set once on the body group; the wings and the tail drawn first so the body covers their roots; the shadow outside the body so it stays on the ground and narrows during `takeoff`; the smoke hidden at opacity 0 and faded in and out within `puff`; two loops of 2.8 s and 1.9 s; every rotated part with its pivot; a mouth whose `open` has the commands and count of numbers of its own d; states that repeat breathe and blink. Asked later to make it puff less often, send a call without `svg` that holds only `animations` with `puff` written out again: an animation is replaced whole by its name, and everything else stays.
