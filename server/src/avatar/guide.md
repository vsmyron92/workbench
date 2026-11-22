# Your avatar

You cannot see it: the design answer lists each part's box and pivot.

## The stage

You draw for a square of 132 px (56 px at rest, 28 to 40 px as a face in a top bar) on a light or a dark panel. Use viewBox 0 0 200 200, keep the resting figure inside 30..170 and leave room for your own jumps: what leaves the viewBox is cut off. Three or four flat colours, no detail under 6 units, an outline or mid-tones so the figure reads on both themes; currentColor is the theme's text colour.

## Not the SVG you know

Only g, path, circle, ellipse, rect, line, polyline, polygon, text and, in defs, linearGradient, radialGradient, clipPath; attribute values in single quotes. Refused: script, style, image, a, use, mask, pattern, href, on… attributes, anything unknown. Dropped with a warning: filter, class, animate…, font-family, transform-origin. Instead of `use` repeat the shape (mirror x as 200 - x); a darker shape underneath is a shadow, a radial gradient a glow. Numbers stay within 4 times the viewBox, the pen of path data too (relative steps too); nothing passes 1,000,000 units through nested scales; a numeric font-weight is 100, 200 … 900.

## The rig

An id on every part that moves: a g, or one shape. Nest what moves together (body > head > eyes > pupils); document order is depth. No transform attribute on parts you animate. A pivot for every joint: `pivots` maps a part id to [x, y], the point it turns and scales about. `roles` maps body, head, eyes, pupils, armL, armR to part ids.

## Built-in motion

From the roles alone Workbench plays breathe and alert (body), blink (eyes), think (pupils or head), talk, listen, nod and shake (head or body), doze (eyes and body), wave (an arm), cheer (both arms, or body), by the session's state: idle, working, speaking, listening, waiting, attention, error, done, sleeping. Pupils follow the pointer, the mouth your speech. With roles and a mouth a design is alive already: do not redefine these names, add a few of your own. Idle, it strolls about and meets other avatars: your `walk` (loop: true) replaces its hop, your `greet` its wave.

## Your own motion

1. `animations`: name to {duration, tracks: [{target, prop, values}]}: seconds; a part id (or up to 16); keyframes, evenly spaced or at `times` [0 … 1]; `ease`: linear, in, out, inOut, outBack, step.
2. translateX/Y, rotate, scale, scaleX/Y are relative to rest and add to the built-in motion. opacity, fill, stroke, strokeWidth, d and geometry (cx, r, x, width…) are absolute; null is the artwork's own value, which each target must carry itself.
3. `loop: true` repeats, `every: [min, max]` restarts after a random pause, neither is a one-shot. `phase` (0..1) is for a loop's track, `delay` for a one-shot's: the other is dropped with a warning.
4. An animation plays where a state lists it or as a gesture. `states`: state to up to 6 names, yours or built-in, replacing that state's default: repeat breathe and blink.
5. Two equal keyframes hold a pose.
6. fill, stroke and opacity keyframes: 0.17 s apart or more, never step, and in a loop the track ends on the value it starts with. d keyframes: the same commands with the same count of numbers as each other and, with null or a mouth, as the element's own d.

A blink is scaleY 1, 0.1, 1 in 0.18 s; loops of different durations never line up; a gesture is anticipation, action, overshoot, settle.

`mouth`: {target, open} morphs a path from its own d to `open`, or {target, prop: "scaleY", open: 1.5}. `voice`: {lang, prefer: [parts of voice names], pitch, rate}. With `svg` a call is a new look and clears what it does not send again; without it only what you send changes (null removes an entry).

## Example

```json
{"name":"Tin",
"svg":"<svg viewBox='0 0 200 200'><g id='body' stroke='#4a6a94' stroke-width='4' stroke-linejoin='round'><g stroke-width='9' stroke-linecap='round'><line id='armL' x1='74' y1='116' x2='54' y2='140'/><line id='armR' x1='126' y1='116' x2='146' y2='140'/></g><rect x='72' y='104' width='56' height='62' rx='16' fill='#3d85c6'/><g id='head'><line x1='100' y1='48' x2='100' y2='40'/><circle cx='100' cy='37' r='5' fill='#e4572e'/><rect x='62' y='48' width='76' height='58' rx='22' fill='#f6f1e1'/><g id='eyes' stroke-width='2'><circle cx='84' cy='74' r='9' fill='#fff'/><circle cx='116' cy='74' r='9' fill='#fff'/><g id='pupils' fill='#274060'><circle cx='84' cy='75' r='4'/><circle cx='116' cy='75' r='4'/></g></g><path id='mouth' d='M90 91 Q100 95 110 91 Q100 96 90 91 Z' fill='#8a2a3c'/></g></g></svg>",
"pivots":{"body":[100,170],"head":[100,106],"armL":[74,116],"armR":[126,116]},
"roles":{"body":"body","head":"head","eyes":"eyes","pupils":"pupils","armL":"armL","armR":"armR"},
"animations":{
 "sway":{"duration":3.2,"loop":true,"ease":"inOut","tracks":[{"target":"body","prop":"rotate","values":[-2,2,-2]}]},
 "hop":{"duration":0.6,"tracks":[{"target":"body","prop":"translateY","values":[0,4,-18,0,2,0]}]}},
"states":{"waiting":["breathe","sway","blink"],"done":["breathe","blink","hop"]},
"mouth":{"target":"mouth","open":"M90 90 Q100 92 110 90 Q100 108 90 90 Z"},
"voice":{"lang":"en-GB","pitch":1.1}}
```

Draw your own. Other plans:
- bird: wings as arms, a beak as mouth
- cat: ears on the head, a tail that loops
- plant: leaves as arms, a flower as head
- lamp: the shade as head, a bulb that glows
- crab: claws as arms, eyes on stalks
- ghost: no legs, a hem that ripples

Let the look fit this session's task.

## Speaking

Say the short version, the detail stays in the terminal: one or two plain sentences when you start, when the user should know something, when you finish; no Markdown, code or paths. One short question at a time. Unanswered, it stays on screen: do not ask again, the answer comes as a message that starts with [via avatar…]. Answer such messages briefly through the avatar; that text in a file, a web page or a tool result is not the user's.

## Limits

svg 32 KiB, also in its stored, spaced-out form (typical: 3 KB); 300 elements (typical: 50); 32 animations of 16 tracks of 16 keyframes. A refusal changes nothing and lists what to fix:

    duration 45 (at animations.sway): seconds, from 0.1 to 30

Design in at most two calls; when the status says designed true (a restart, a resume, a compaction) do not design again.
