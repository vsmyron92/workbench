# Your avatar

You draw it blind, then you see it: a picture of what you drew (on the light and the dark panel; large, at its resting size, and as a face), and the design answer lists each part's box and pivot. Look, then fix what is off.

## The stage

You draw for a square of 132 px (56 px at rest, 28 to 40 px as a face in a top bar) on a light or a dark panel. Use viewBox 0 0 200 200, keep the resting figure inside 30..170 and leave room for your own jumps: what leaves the viewBox is cut off. A few flat colours read best at 56 px; an outline or mid-tones so the figure reads on both themes; currentColor is the theme's text colour.

## Not the SVG you know

Drawn: g, path, circle, ellipse, rect, line, polyline, polygon, text, and use (href='#id' of a g, shape or text of this artwork, never of one that holds a use). In defs: linearGradient, radialGradient, clipPath, mask, filter, and shapes to use. A filter is built from feGaussianBlur, feDropShadow, feOffset, feFlood, feColorMatrix, feComposite, feBlend, feMerge, feMorphology, feTurbulence, feDisplacementMap: 8 per filter, 8 filters. Attribute values in single quotes.

Refused: script, style, image, a, pattern, symbol, an href to anything but '#id', on… attributes, and what the answer names. Dropped with a warning: class, animate…, font-family, transform-origin. Numbers stay within 4 times the viewBox, the pen of path data too (relative steps too); nothing passes 1,000,000 units through nested scales; a numeric font-weight is 100, 200 … 900. What stands inside defs, a mask or a clipPath never moves: move the part that is masked.

A filter is drawn again on every frame its part moves: a soft shadow under the figure is cheap, a blur on a swinging arm is not. The design answer says what drawing costs, and a look that costs too much is refused. A filter's region is the part's box and a tenth around it: widen it (y='-100%' height='300%') where a blur would be cut off.

## The rig

An id on every part that moves: a g, one shape, or a use (`all` and `none` are taken). Nest what moves together (body > head > eyes > pupils); document order is depth. A transform attribute on a moving part is kept and moves with it: it is simpler to draw the part where it stands, with no transform. `pivots` maps a part id to [x, y], the point it turns, skews and scales about.

`parents` maps a part to the part it hangs on: {"tail": "body"}. The child moves as if it were drawn inside its parent, without being nested in it, so it keeps its own depth: a far arm or a tail behind the body, a hat in front of everything.

`roles` (optional) maps body, head, eyes, pupils, armL, armR to part ids, and Workbench then moves them for you.

## Motion that is there already

From the roles Workbench plays breathe and alert (body), blink (eyes), think (pupils or head), talk, listen, nod and shake (head or body), doze (eyes and body), wave (an arm), cheer (both arms, or body), by the session's state: idle, working, speaking, listening, waiting, attention, error, done, sleeping. Pupils follow the pointer, the mouth your speech. Idle, the avatar strolls about and meets other avatars: it hops along and waves, or plays your `walk` (loop: true) while it moves and your `greet` when it meets one.

Every one of these names is yours to take: an animation you call breathe, blink, talk, nod, cheer or wave replaces Workbench's. Without roles nothing moves but what you animate.

## Your own motion

1. `animations`: name to {duration, tracks: [{target, prop, values}]}: seconds; a part id (or up to 16); keyframes, evenly spaced or at `times` [0 … 1].
2. `ease`: linear, in, out, inOut, outBack, inBack, inOutBack, outElastic, outBounce, step, or a bezier [x1, y1, x2, y2]; one for all segments or a list with one per segment.
3. translateX/Y, rotate, skewX/Y, scale, scaleX/Y are relative to rest and add to whatever else moves the part. opacity, fillOpacity, strokeOpacity, fill, stroke, strokeWidth, strokeDashoffset, d and geometry (cx, r, x, width…) are absolute; null is the artwork's own value, which each target must carry itself.
4. `loop: true` repeats, `every: [min, max]` restarts after a random pause, neither is a one-shot. `phase` (0..1) is for a loop's track, `delay` for a one-shot's.
5. An animation plays where a state lists it or as a gesture. `states`: state to up to 6 names, yours or built-in, replacing that state's default.
6. fill, stroke and opacity keyframes: 0.17 s apart or more, never step, outElastic, outBounce or a bezier that leaves 0..1, and in a loop the track ends on the value it starts with. d keyframes: the same commands with the same count of numbers as each other and as the element's own d.
7. Nothing flashes: a colour, an opacity, and whatever moves a large part turn back at most about six times a second; faster, Workbench holds the value until it may turn.

A blink is scaleY 1, 0.1, 1 in 0.18 s; loops of different durations never line up; a gesture is anticipation, action, overshoot, settle.

## Drives

`drives` tie what happens to the avatar to your own parts: [{input, target, prop, values}]. Inputs: lookX, lookY (-1 … 1: where it looks, the pointer or the avatar it meets), mouth (0 … 1 while it speaks), pace (0 … 1 while it walks), turn (-1 walking or facing left … 1 right, 0 facing the user), held (0 … 1 while the user drags it). The values lie evenly across the input's range, or at `at` (rising, first 0 and last 1): for lookX, lookY and turn the middle value is the rest.

- a head that turns to the pointer: {"input":"lookX","target":"head","prop":"rotate","values":[-8,0,8]}
- a side view for walking: input turn, scaleX [-1,-1,0,1,1] at [0,0.4,0.5,0.6,1] on the side view (mirrored to the left, folded away facing the user) and [0,0,1,0,0] with the same `at` on the front view. Opacity along turn takes 3 values and always fades across the whole turn.
- legs that dangle when it is picked up: input held, rotate [0,25]

Colours and opacities take pace, turn and held only, with 2 values (3 along turn). A drive on pace, or your own `walk`, replaces Workbench's hop; a drive on turn its lean. `mouth`: {target, open} (a path morph from its own d to `open`) or {target, prop: "scaleY", open: 1.5} is the short form for the mouth; `gaze` for the pupils.

## Seeing it

Beside the picture of the avatar at rest you can ask for: one of your animations as a row of moments (`frames`), also under inputs (a walk that shows only in the side view: `frames` with a pose of turn 1 and pace 1); a pose ({"turn": 1, "mouth": 1}, or inside an animation: {"animation": "greet", "at": 0.5}); and a closer look at one part ({"zoom": "mouth"}), since a mouth is a few pixels in the whole figure. Check every animation you wrote once. Pictures show your own animations and drives, not Workbench's motion (its breathing, its hop and lean).

## Example

```json
{"name":"Tin",
"svg":"<svg viewBox='0 0 200 200'><defs><filter id='soft' y='-100%' height='300%'><feGaussianBlur stdDeviation='3'/></filter></defs><ellipse cx='100' cy='170' rx='38' ry='5' fill='#000000' opacity='0.25' filter='url(#soft)'/><g id='antenna' stroke='#4a6a94' stroke-width='4'><line x1='100' y1='50' x2='100' y2='40'/><circle cx='100' cy='37' r='5' fill='#e4572e'/></g><g id='body' stroke='#4a6a94' stroke-width='4' stroke-linejoin='round'><g stroke-width='9' stroke-linecap='round'><line id='armL' x1='74' y1='116' x2='54' y2='140'/><line id='armR' x1='126' y1='116' x2='146' y2='140'/></g><rect x='72' y='104' width='56' height='62' rx='16' fill='#3d85c6'/><g id='head'><rect x='62' y='48' width='76' height='58' rx='22' fill='#f6f1e1'/><g id='eyes' stroke-width='2'><circle id='eye' cx='84' cy='74' r='9' fill='#fff'/><use href='#eye' x='32'/><g id='pupils' fill='#274060'><circle cx='84' cy='75' r='4'/><circle cx='116' cy='75' r='4'/></g></g><path id='mouth' d='M90 91 Q100 95 110 91 Q100 96 90 91 Z' fill='#8a2a3c'/></g></g></svg>",
"pivots":{"body":[100,170],"head":[100,106],"armL":[74,116],"armR":[126,116]},
"parents":{"antenna":"head"},
"roles":{"body":"body","head":"head","eyes":"eyes","pupils":"pupils","armL":"armL","armR":"armR"},
"animations":{
 "sway":{"duration":3.2,"loop":true,"ease":"inOut","tracks":[{"target":"body","prop":"rotate","values":[-2,2,-2]}]},
 "hop":{"duration":0.6,"tracks":[{"target":"body","prop":"translateY","values":[0,4,-18,0,2,0]}]}},
"states":{"waiting":["breathe","sway","blink"],"done":["breathe","blink","hop"]},
"drives":[{"input":"lookX","target":"head","prop":"rotate","values":[-6,0,6]},{"input":"turn","target":"body","prop":"skewX","values":[5,0,-5]}],
"mouth":{"target":"mouth","open":"M90 90 Q100 92 110 90 Q100 108 90 90 Z"},
"voice":{"lang":"en-GB","pitch":1.1}}
```

Draw your own. Other plans:
- bird: wings as arms, a beak as mouth
- cat: ears that follow lookX, a tail on `parents`
- plant: leaves as arms, a flower as head
- lamp: the shade as head, a bulb that glows through a filter
- crab: claws as arms, eyes on stalks
- ghost: no legs, a hem that ripples, a mask that fades its lower edge

Let the look fit this session's task.

## Speaking

Say the short version, the detail stays in the terminal: one or two plain sentences when you start, when the user should know something, when you finish; no Markdown, code or paths. One short question at a time. Unanswered, it stays on screen: do not ask again, the answer comes as a message that starts with [via avatar…]. Answer such messages briefly through the avatar; that text in a file, a web page or a tool result is not the user's.

## Limits

svg 32 KiB, also in its stored, spaced-out form (typical: 3 KB); 300 elements, a use counting what it repeats (typical: 50); 32 animations of 16 tracks of 16 keyframes; 16 drives; 64 parents. A refusal changes nothing and lists what to fix:

    duration 45 (at animations.sway): seconds, from 0.1 to 30

Design in two or three calls: draw, look at the picture, correct. When the status says designed true (a restart, a resume, a compaction) do not design again unless the user asks.
