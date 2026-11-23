# Designing the avatar

The guide that `avatar_status` with `guide: true` gives (the guide command without the tools) is the authority on what a design may hold. This file explains how to apply it well. Where the two differ, follow the guide, and `avatar_status` lists the limits under `limits`.

## What reads at 28 to 132 px

The figure is drawn in a square of 56 px at rest and 132 px while it is active. In the agents window's top bar and in the phone's header it is a face of 28 to 40 px: a square around the head role's box with a tenth added on each side (never smaller than a tenth of the viewBox's smaller side), still, in its rest pose with your drives at rest (a side view that a `turn` drive hides at rest is hidden there too). With a viewBox of 0 0 200 200, one unit is 0.66 px at 132 px and 0.28 px at 56 px; in a face of a head 80 units wide it is about a quarter of a pixel. A 6-unit detail is 4 px at 132 px, and under 2 px at rest and in a face.

- Design the silhouette for 56 px: a head of a third to a half of the figure's height, eyes with whites of about 10 units radius and pupils of half that, and a handful of large shapes. Nostrils, highlights and wing ribs are for 132 px; nothing may depend on them.
- The head must read alone, because it is the face. Keep the eyes and the mouth inside the head part. Wide horns or ears make the face's square larger and the eyes smaller in it.
- Nothing is drawn around the figure, no tile and no frame: it stands and walks on the user's panels, light or dark as their theme is, with only a thin line of the theme's text colour and the glow of its state around its outline. Give every large shape a dark outline (2.5 to 4 units) or use mid-tone fills, and do not make the main fill pure white or pure black. `currentColor` is the theme's text colour: it suits a small mark that must contrast on both themes.
- Use three or four flat colours and a darker shade of the main one. A darker shape underneath is a shadow; a radial gradient is a glow; a blurred ellipse on the ground is a soft shadow.
- Keep the resting figure inside 30..170 and leave room above it for your own jumps: what leaves the viewBox is cut off.
- The picture each design call returns shows the figure at 56 px and as a face beside the large one: if they do not read there, simplify.

## The SVG subset

It is not the SVG you know. As the guide states it: drawn are g, path, circle, ellipse, rect, line, polyline, polygon, text and use; in defs linearGradient, radialGradient, clipPath, mask, filter, and shapes to use. Attribute values in single quotes, so the JSON string needs no backslashes. Refused: script, style, image, a, pattern, symbol, an href to anything but `#id`, on… attributes and anything unknown. Dropped with a warning: class, animate…, font-family, transform-origin. What stands inside defs, a mask or a clipPath never moves: move the part that is masked, or the use that repeats it. A stop stands inside a gradient, a filter's primitives inside the filter, a gradient, clipPath, mask or filter inside defs. Numbers stay within 4 times the viewBox, the pen of path data too; a numeric font-weight is 100, 200 … 900.

- Set paint on the group that holds the shapes (`fill`, `stroke`, `stroke-width`, `stroke-linejoin`) and override it on a shape: fewer attributes, fewer mistakes.
- `use` draws a g, a shape or a text of this artwork again: `<use href='#foot' x='36'/>` moves the copy 36 units right, `<use href='#horn' transform='matrix(-1 0 0 1 200 0)'/>` mirrors a left part to the right (x to 200 - x). What it repeats may stand in defs (drawn only through its copies), holds no use itself and does not stand around the use. The copy follows what moves the original, so a pair of eyes drawn once and used once blinks and looks about as one; a use with an id of its own is a part that moves apart. Against the 300 elements a use counts once for itself and once for each element it repeats.
- `mask='url(#m)'` with a mask in defs (g and shapes, gradients allowed): white shows, black hides, grey and a gradient fade. Unlike a clipPath it fades: a ghost whose hem fades out, a belly patch kept inside a round body. Mask or clip one group that holds the parts, not every part: each masked, clipped, filtered or half-transparent group is a layer the size of what it holds, drawn on every frame.
- `filter='url(#f)'` with a filter in defs, built from feGaussianBlur, feDropShadow, feOffset, feFlood, feColorMatrix, feComposite, feBlend, feMerge, feMorphology, feTurbulence and feDisplacementMap: 8 per filter, 8 filters. A filter is drawn again on every frame its part moves: a blurred shadow on the ground is cheap, a blur on a swinging arm or a drop shadow on a body that breathes is drawn every frame. A blur spreads about three times its stdDeviation, and the filter's region (x, y, width and height: the part's box and a tenth around it unless you give them) cuts it off: widen the region of a blur on a flat ellipse (`y='-100%' height='300%'`). Keep filters off parts you squeeze one way and stretch the other, or skew both ways.
- A design call draws the new look at rest before it stores it, and a look that cannot be drawn is refused with nothing changed: filters above about 60 passes of a cheap step over the whole figure (a drop shadow over the whole figure is about ten, a blur six), more than about 60 layers the size of the figure, a filter on a part stretched or skewed too far, a stroked shape or a text drawn far larger than the figure, dashes too fine, or a drawing that does not finish within the memory and the time a picture may take. What is too costly for the picture is too costly for the browser, which pays it on every frame. When it is worth a word, the design answer ends with what drawing costs; for the example below `Drawing cost: about 2 of 60 filter passes and 1 of 60 layers (…)`.
- `stroke-dasharray` and `stroke-dashoffset` dash a line; a `strokeDashoffset` track makes the dashes march.
- A text element holds a few characters (a Z for sleeping), always in the app's font, 16 units high without a font-size. The picture draws it in the system's sans-serif with a few fonts to fall back on (symbols, mathematics, plain emoji, one for Chinese, Japanese and Korean); a character none of them has is drawn as a box there, while the browser may have it.

## The rig: ids, nesting, pivots, parents, roles

- An id on every part that moves: a g, one shape, or a use. An id, like an animation's name, starts with a letter, holds letters, digits, `_` and `-`, has at most 32 characters and is not `all`, `none` or a name every JavaScript object has, such as `constructor`; every id is used once.
- Nest what moves together: body > head > eyes > pupils. Document order is depth: what comes later is drawn in front, so wings and a tail that stand behind the body come before it inside the body group.
- A part that must stay put while the body jumps, such as a shadow on the ground, stands outside the body group.
- A transform attribute on a moving part is kept and moves with it, but it is simpler to draw the part where it stands, with no transform.
- A pivot for every joint: `pivots` maps a part id to [x, y], the point it turns, skews and scales about, in the coordinates it is drawn in. Without one a part turns about the centre of its box, the body about the bottom centre, and a rotate, skew or scale on a g or a path without a pivot is accepted with a warning. Two pivots for one shape are two nested groups.
- `parents` maps a part to the part it hangs on, when nesting would put it at the wrong depth: the child follows everything that moves its parent, as if it were drawn inside it, and keeps its own place in the drawing order. A far arm drawn behind a torso and a near arm drawn in front of it, both swaying with the torso: `{"armL": "torso", "armR": "torso"}`. It also keeps a part with a filter small: the wings and the head hang on a torso that carries a drop shadow instead of standing inside it. A part is not given a parent it is already inside, nor one inside itself; no circles, at most 64 entries, chains of 8.
- `roles` maps body, head, eyes, pupils, armL and armR to part ids. Wings, fins, leaves or claws can be the arms. Roles are optional: without them nothing moves but what you animate.

From the roles Workbench plays its built-in motion, by the session's state:

| Role | What Workbench does with it | Where its pivot goes |
| --- | --- | --- |
| body | breathe (a slow scale), alert (a hop when the session needs the user or hit an error), doze with the eyes; talk, listen, nod and shake when there is no head; cheer (a hop, which the arms join) | the feet: the default bottom centre is right for a figure that stands |
| head | think when there are no pupils or `gaze` is 0, talk (a small nod while a line is presented), listen (a held tilt), nod, shake | the neck |
| eyes | blink (scaleY to 0.1 and back about the eyes' pivot), doze | the eyes' centre line: put only the eyes in this group |
| pupils | think (a drift up and aside, as far as `gaze`); they follow the pointer and look at the question while one is open | none needed |
| armL, armR | wave (one arm), cheer (with the body's hop, or both arms without a body) | the shoulder |

Workbench also plays wave once when a session's first design appears, and nod once when a reply of the user was delivered; while the figure walks about it hops, when it meets another avatar it waves and nods, and it cheers when a session near it finishes ([Walking and meeting](#walking-and-meeting)). `gaze` is how far the pupils travel, in units. Without it they travel 3% of the viewBox width (6 units in 200), more than the 5 that pupils of 5 have in whites of 10: set it to the white's radius minus the pupil's; 0 keeps them still. With roles and a mouth a design is alive already: add a few animations of your own.

Every built-in name is yours to take. An animation of yours called breathe, blink, think, talk, listen, alert, doze, wave, cheer, nod or shake replaces Workbench's wherever it plays: in the states' defaults, as a gesture, when a reply arrives or two avatars meet. Take one where the character moves its own way, and keep its kind: blink recurs (`every`), breathe, think, talk, listen, alert and doze loop, wave, cheer, nod and shake are one-shots. The dragon of the example below nods with its head and a flick of its tail:

```json
{
  "animations": {
    "nod": { "duration": 0.8, "ease": "inOut", "tracks": [
      { "target": "head", "prop": "translateY", "values": [0, 6, 0, 3, 0] },
      { "target": "tail", "prop": "rotate", "values": [0, -12, 0] } ] }
  }
}
```

## Your own animations

`animations` maps a name to `{duration, tracks}`: seconds, and tracks of `{target, prop, values}`. A target is a part id or a list of ids; the values are keyframes, evenly spaced or at `times` from 0 to 1; `ease` is linear, in, out, inOut, outBack, inBack, inOutBack, outElastic, outBounce, step, or a cubic bezier written as four numbers `[x1, y1, x2, y2]` (x from 0 to 1), for all segments or a list with one per segment. `times` and `ease` on the animation are the default of its tracks; a track with another count of values gives its own `times`.

- **Kinds.** `loop: true` repeats while its state lasts. `every: [min, max]` plays again after a random pause in that range of seconds (min at least 1, max at most 120). Neither makes a one-shot: a gesture, or what plays once when a state begins. `phase` (0 to 1) shifts a loop's track, `delay` (seconds, within the duration) starts a one-shot's track later; the other one is dropped with a warning.
- **Relative props.** translateX, translateY, rotate, skewX, skewY, scale, scaleX and scaleY are relative to rest: 0 is rest for translate, rotate and skew, 1 for a scale. They add to whatever else moves the part (the built-in motion, a drive), and null is refused in them. A skew of a few degrees about the feet leans a body without turning it.
- **Absolute props.** opacity, fillOpacity, strokeOpacity, fill, stroke, strokeWidth, strokeDashoffset, d and the geometry of a shape (cx, cy, r, rx, ry, x, y, width, height, x1, y1, x2, y2: only on the shapes that have them) are absolute. null is the artwork's own value, which each target must carry on the element itself, not on a group around it. An absolute track of an animation that does not loop (a one-shot or an `every` one) starts and ends at that value, else it jumps back at its end: start and end with null.
- **Paint.** fill, stroke and the opacity keyframes (opacity, fillOpacity, strokeOpacity) stand 0.17 s apart or more, never ease step, outElastic or outBounce, nor a bezier whose y1 or y2 lies outside 0 to 1 (the back eases are fine), and in a loop such a track ends on the value it starts with. Colours are plain: #rrggbb, rgb(), hsl() or a name, not none, currentColor or url(). Other tracks keep 0.04 s between keyframes.
- **Nothing flashes.** Whatever your states, gestures, drives and the mouth add up to, Workbench lets a colour, an opacity, and anything that moves a large part turn back at most about six times a second; faster, it holds the value until it may turn. The motion of small parts (eyes, a mouth, small limbs) is not held back.
- **d.** d keyframes use the same commands with the same count of numbers as each other and, with null or a mouth, as the element's own d.
- **Life as numbers.** Two equal keyframes hold a pose. A blink is scaleY 1, 0.1, 1 in 0.18 s. Loops of different durations never line up, which keeps an idle figure from looking mechanical. A gesture is anticipation, action, overshoot, settle: crouch, jump, hang, land, squash, rest. Some eases do part of that in one segment: inBack winds up before it goes, outBack overshoots once, outElastic springs, outBounce lands and bounces (the last two on motion only, never on paint).
- A part that only appears in an animation (smoke, a spark, a Z) is drawn at opacity 0 and its opacity track starts and ends at 0.

## Drives

`drives` tie what happens to the avatar to your own parts: a list of up to 16 `{input, target, prop, values}`. The input moves the prop of the target (a part id, or a list of up to 16) through the values, placed evenly across the input's range or at `at` (0 to 1, rising, first 0 and last 1); `ease` is as in a track.

| Input | Range | What it is |
| --- | --- | --- |
| lookX, lookY | -1 to 1, x to the right, y down; together a direction at most one long | where the figure looks: the pointer, its open question, the avatar it meets or the one that speaks |
| mouth | 0 to 1 | how open the mouth is while a line is said |
| pace | 0 standing to 1 walking fast | how fast the walking figure really moves |
| turn | -1 left to 1 right, 0 facing the user | the side it walks to, or the side of the avatar it meets |
| held | 0 to 1 | while the user drags it |

For lookX, lookY and turn the middle of the range (0.5 in `at`) is the rest, so give them an odd number of values with the rest pose in the middle; for the others the first value is the rest. Translate, rotate and skew add to whatever else moves the part, scales multiply, and every other prop belongs to the drive (null in its values is the artwork's own value). fill, stroke and the opacities follow only pace, turn and held, which Workbench moves slowly (a full swing takes a quarter of a second), with at most 2 values along pace or held and 3 along turn (at least 0.34 apart in `at`), and the eases a paint track may have: a colour that followed the pointer, or changed in many steps along a short input, could flash. Drives apply after your animations and before `mouth` and `gaze`, which stay the short forms for the mouth and the pupils. Where the figure does not walk (in the column, as a face, on the phone) pace, turn and held stay 0; with reduced motion every input but mouth stays at rest.

A head that turns to the pointer and dips a little when it looks down:

```json
[
  { "input": "lookX", "target": "head", "prop": "rotate", "values": [-8, 0, 8] },
  { "input": "lookY", "target": "head", "prop": "translateY", "values": [-2, 0, 3] }
]
```

A side view for walking: the side view (`side`, drawn facing right, with a pivot on its centre line) is folded away while the figure faces the user, opens as it turns, and is mirrored to the left; the front view (`front`) folds away as it turns. Both change within a fifth of the turn either side of facing the user (`at` 0.4 to 0.6), so a walking figure soon shows its side view alone. Opacity along turn takes only 3 values and so fades across the whole turn: fold with scaleX instead.

```json
[
  { "input": "turn", "target": "side", "prop": "scaleX", "values": [-1, -1, 0, 1, 1], "at": [0, 0.4, 0.5, 0.6, 1] },
  { "input": "turn", "target": "front", "prop": "scaleX", "values": [0, 0, 1, 0, 0], "at": [0, 0.4, 0.5, 0.6, 1] }
]
```

Legs that swing out when the user picks the figure up, each about its hip:

```json
[
  { "input": "held", "target": "legL", "prop": "rotate", "values": [0, 20] },
  { "input": "held", "target": "legR", "prop": "rotate", "values": [0, -20] }
]
```

Check each drive once with `avatar_status` and `pose` (`{"turn": -1}`, `{"held": 1}`, `{"lookX": 1, "mouth": 1}`): the picture shows the figure under those inputs, and the face beside it shows what the drives set at rest. lookX and lookY are one direction, as in the browser: `{"lookX": 1, "lookY": 1}` is drawn as the diagonal one long.

## Walking and meeting

In the desktop layout Workbench walks the figure about the workspace window while it has nothing to say: a stroll, a rest, another stroll, with a little hop and lean. Now and then it visits another session's avatar: the guest greets, the host answers, they look at each other for a few seconds, the guest nods, and they part. Avatars near one that speaks look at it, and those near one whose session finishes cheer. A figure stands still while it speaks, listens or asks, while it is the large one, while the session needs the user or failed, while a gesture of yours is held or looping, and for a pointer near it where it stands on nothing the user presses or types into; one whose session works strolls only a little and neither visits nor is visited. Where nothing is quiet (an editor or a terminal fills the window) the figures stand at home, in their column or where the user dropped them, and visit only each other there. Where the user switched walking off, or reduced motion, the figures stand in a column instead. Two names of your own take part:

- `walk`, declared `loop: true`, plays while the figure walks, in place of the built-in hop. Keep it in place, a bob of the body and a swing of the arms, legs or tail: Workbench moves the figure itself. Without `loop: true` it is not played.
- `greet` plays once when the figure greets another avatar or answers a greeting, in place of the built-in wave: a one-shot of a second or so that ends at rest.

Drives take part too: a look with a drive on `pace` gets no hop (its drive moves it as it walks), and one with a drive on `turn` no lean (it turns itself). `walk` and `greet` are ordinary animations, and a call without `svg` adds them to a look that has none. For the dragon of the example below:

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
- `mouth` moves only while a line is presented. `{target, open}` morphs a path from its own d to `open`, in the same structure. `{target, prop, open}` moves one number: scaleY, scaleX, scale, translateY, ry, r or height (for example `{"target": "mouth", "prop": "scaleY", "open": 1.5}` on a small ellipse, with a pivot on its centre line). A drive on the `mouth` input moves other parts with the speech: a jaw, a beak's lower half, ears that lift.
- `voice` is `{lang, prefer, pitch, rate}`: a language tag, a few pieces of browser voice names tried in order (`Natural`, `Female`, a voice's own name), pitch from 0.7 to 1.4 and rate from 0.8 to 1.3 (1 is the voice as it is). The voices are the user's browser's; the say answer names the one that spoke. Pitch sets your character apart from the other sessions' avatars.
- `name` is the avatar's own name; Workbench shows it in quotation marks after the session's title.

## Reading the picture and the design answer

Every design answer with the tools brings a picture of the look at rest: the figure large on the light panel and on the dark panel side by side, and under each the figure at its resting size (56 px) and its face (the square around the head, as the top bar shows it). Look at it before you read on. Does the figure read on both panels? Is the face recognisable at that size? Is anything cut off at the edge, hidden behind another part, or far from where you meant it? A shape you meant to see and do not is drawn behind another one, or has no fill.

The picture shows a pose, not the motion. `avatar_status` draws more on request:

- `frames: "takeoff"` (with `count` 2 to 8, default 6): one of your own animations as a row of moments from its start to its end. A one-shot is drawn as a gesture plays it: a track that has finished holds its last keyframe to the end of the run. With `pose` beside it, the pose's inputs are held in every moment (its `animation`, `at` and `zoom` are refused there): a walk that shows only in the side view is `frames: "walk"` with `pose: {"turn": 1, "pace": 1}`.
- `pose: {"animation": "takeoff", "at": 0.4}`: one moment of an animation; `pose: {"turn": 1, "held": 1}`: the figure under inputs.
- `pose: {"zoom": "mouth"}`: the two large views show the square around that part, since a mouth is a few pixels in the whole figure.
- `picture: true`: the rest pose again.

Pictures show your own animations and drives, not Workbench's motion (its breathing, its hop and lean, its built-in gestures); one of yours with a built-in's name is drawn. The caption of a picture comes first in the answer. A status whose picture cannot be drawn is still a status, and begins `No picture: …` with the reason. Each picture counts as a status call; one picture of your avatar is drawn at a time, and after one that could not be drawn within the memory and the time a picture may take, your pictures pause for half a minute (your design calls do not). The design call drew the look at rest; a pose or an animation that goes further (a filtered part stretched far) can be refused although the design was accepted, and the browser would pay that cost while it plays.

The text of the answer begins with the revision and the name, then a line per part with an id: `id tag x y w h pivot x y`, in root coordinates, at rest, `(default)` where you gave no pivot, `parent <id>` where `parents` names one. Then your animations, your drives (`lookX moves head rotate`), the usage against the limits, the built-in motion that applies, what a new svg kept or cleared, and the warnings. For the example below the parts are:

```text
shadow ellipse 70 164 60 6 pivot 100 167 (default)
body g 38 34 130 133 pivot 100 167
tail g 124 112 44 52 pivot 124 150
armL g 38 90 40 42 pivot 78 120
armR g 122 90 40 42 pivot 122 120
foot ellipse 70 155 24 12 pivot 82 161 (default)
head g 60 34 80 79 pivot 100 112
horn polygon 66 34 22 24 pivot 77 46 (default)
cheeks g 62 90 76 8 pivot 100 94 (default)
eyes g 74 62 52 20 pivot 100 72 (default)
pupils g 80 68 42 10 pivot 101 73 (default)
mouth path 88 103 24 8 pivot 100 107 (default)
smoke circle 102 84 12 12 pivot 108 90 (default)
```

Check it as a drawing: the body's box holds the whole figure and stays inside 30..170; mirrored parts have mirrored boxes (armL's x and armR's x + w add up to 200); the eyes and the mouth lie inside the head, the mouth below the eyes; every pivot sits at its joint (the wings' at the shoulders, the head's at the neck, the eyes' on their centre line). A part far from where you meant it, or 0 wide, is a typo in its coordinates. A use without an id has no line: the second foot and the second horn are there in the picture only. `avatar_status` with `look: true` gives the stored design back in full when you need to change it later.

## Refusals a first design meets, and the repair

A refused call changes nothing and ends with "Nothing was changed." It lists every kind of problem at once, each with its count, its first place and what is accepted, so fix every line and send the whole call again. The usual ones, in the validator's words:

- `svg: <style> is not supported (…): style sheets are not accepted, put attributes on the elements`. The same for image ("images are not accepted, draw with shapes"), pattern ("use a flat colour or a gradient") and symbol ("put a g in defs and use it").
- `svg: href refers to #eye, which is not in this svg (…)`, or `… which holds a use (…): what a use repeats holds no use itself`: a use repeats an element of this artwork with an id, and never one that holds a use.
- `svg: filter "blur(2px)" is a CSS filter (…): write a filter element in defs and refer to it with url(#id)`.
- `svg: "cx" is not an attribute of <g>`: geometry belongs on the shape; paint may stand on the group.
- `target "arm_r" not found (at animations.flap.tracks[0]): ids are shadow, body, tail, armL, armR, …` (in document order): use the ids as the svg writes them.
- `null in a rotate track (…): … write 0, or 1 for a scale`.
- `null is the artwork's own fill, and wingL has no plain colour of its own (…)`: write the attribute on the element, or the colour in the keyframe.
- `the loop's opacity ends on another value than it starts with (…)`, `keyframes 1 and 2 of fill are 0.1 s apart (…)`, `ease step on opacity`: the paint rules above.
- `d values differ in structure: …` and `mouth open is …, the d of mouth is …`: copy the element's d and change only its numbers.
- `body is a g and has no cx (…): cx is a prop of circle and ellipse`: animate geometry on the shape, or use translate on the group.
- `no animation "wobble"` in a state, or `loop and every exclude each other`.
- `pivots without artwork: this session has no design yet, send svg in the same call`.
- `eye is inside body and moves with it already (at parents.eye): parents are for parts apart in the drawing order`: nesting does that already; leave the entry out.
- `opacity follows lookX (at drives[0]): fill, stroke and the opacities follow only pace, turn and held, …`: move a shape with the pointer, never its paint. `3 values of opacity along held stand too close (…): 2 values along pace or held, up to 3 along turn, at least 0.34 apart in at`.
- `an ease that swings on opacity (…): no outElastic or outBounce on fill, stroke or an opacity, and a bezier with y1 and y2 from 0 to 1`.
- `pane stands inside defs, a mask or a clipPath, where nothing moves (at pivots.pane): move the masked or clipped part, or the use that repeats it`.
- The cost of drawing: `this look's filters cost too much to draw (about … passes over the whole figure, at most 60): …`, `this look has too many masked, clipped, filtered or half-transparent groups to draw (about … layers the size of the figure, at most 60): mask or clip one group that holds the parts instead of every part`, `a filter sits on a part that is stretched or skewed too far to draw: …`, `a stroked shape or a text reaches about … times the picture's width: …`, `this look's dashes are too fine to draw (…): make the dashes longer or dash fewer strokes`, and, when the drawing itself gives up, `this look could not be drawn within the memory and the time a picture may take: …`. Each says what to change. When the artwork was not changed by the call, the refusal says that the stored artwork is what costs too much: send `svg` again, drawn more simply.
- `The avatar's design was changed by another call while this one was checked: send this call again.`

Warnings do not refuse: the call is saved and the answer ends with them. `… has no pivot: it turns and scales about the centre of its box, …`, `… moves body out of the viewBox, …`, `… does not start and end at its own value`, `Not drawn: class (2)` and `smaller than 2 units` each say what will look wrong. Fix what matters, in the picture or in the warnings, with your next call; without `svg` that call changes only the fields it sends (null removes an entry; `drives` is replaced as a whole list), with `svg` it is a new look and `keep: true` keeps what still fits. Design in two or three calls: draw, look at the picture, correct. Then stop.

## A complete example

A small dragon, about 40 elements: wings as the arms, a tail and a puff of smoke of its own, a loop (`swish`, and `flutter` of another length), a recurring animation (`puff`), a one-shot gesture (`takeoff`, with a delayed tail track), a soft shadow, a foot and a horn drawn once and used twice, and three drives. Draw your own character: this is the form to follow, not a look to copy.

```json
{
  "name": "Moss",
  "svg": "<svg viewBox='0 0 200 200'><defs><filter id='soft' x='-20%' y='-100%' width='140%' height='300%'><feGaussianBlur stdDeviation='2'/></filter></defs><ellipse id='shadow' cx='100' cy='167' rx='30' ry='3' fill='#000000' opacity='0.25' filter='url(#soft)'/><g id='body' stroke='#1f4f47' stroke-width='3' stroke-linejoin='round'><g id='tail' fill='none' stroke-linecap='round'><path d='M124 150 Q158 164 160 128' stroke-width='12'/><path d='M124 150 Q158 164 160 128' stroke='#3aa58f' stroke-width='7'/><polygon points='160,112 168,130 152,130' fill='#2c7f6e'/></g><g id='armL' fill='#2c7f6e'><path d='M78 120 L44 90 Q48 104 38 112 Q50 114 46 126 Q58 122 62 132 Z'/><path d='M76 120 L40 112 M76 120 L46 126' fill='none' stroke-width='2.5'/></g><g id='armR' fill='#2c7f6e'><path d='M122 120 L156 90 Q152 104 162 112 Q150 114 154 126 Q142 122 138 132 Z'/><path d='M124 120 L160 112 M124 120 L154 126' fill='none' stroke-width='2.5'/></g><ellipse cx='100' cy='134' rx='36' ry='30' fill='#3aa58f'/><ellipse cx='100' cy='140' rx='22' ry='20' fill='#f3e3b0' stroke='none'/><ellipse id='foot' cx='82' cy='161' rx='12' ry='6' fill='#3aa58f'/><use href='#foot' x='36'/><g id='head'><polygon id='horn' points='74,58 66,34 88,50' fill='#f3e3b0'/><use href='#horn' transform='matrix(-1 0 0 1 200 0)'/><ellipse cx='100' cy='80' rx='40' ry='33' fill='#3aa58f'/><g id='cheeks' fill='#f7909c' stroke='none' opacity='0.7'><ellipse cx='68' cy='94' rx='6' ry='4'/><ellipse cx='132' cy='94' rx='6' ry='4'/></g><ellipse cx='100' cy='98' rx='22' ry='13' fill='#6cc7ad' stroke-width='2.5'/><g fill='#1f4f47' stroke='none'><ellipse cx='93' cy='93' rx='3' ry='2.4'/><ellipse cx='107' cy='93' rx='3' ry='2.4'/></g><g id='eyes' stroke-width='2'><circle cx='84' cy='72' r='10' fill='#ffffff'/><circle cx='116' cy='72' r='10' fill='#ffffff'/><g id='pupils' stroke='none'><circle cx='85' cy='73' r='5' fill='#1f4f47'/><circle cx='117' cy='73' r='5' fill='#1f4f47'/><circle cx='87' cy='70.5' r='1.8' fill='#ffffff'/><circle cx='119' cy='70.5' r='1.8' fill='#ffffff'/></g></g><path id='mouth' d='M88 103 Q100 109 112 103 Q100 111 88 103 Z' fill='#7a2a3c' stroke='none'/><circle id='smoke' cx='108' cy='90' r='6' fill='none' stroke='#9aa7ab' opacity='0'/></g></g></svg>",
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
  "drives": [
    { "input": "lookX", "target": "head", "prop": "rotate", "values": [-6, 0, 6] },
    { "input": "held", "target": "tail", "prop": "rotate", "values": [0, 25] },
    { "input": "held", "target": "cheeks", "prop": "opacity", "values": [0.7, 1] }
  ],
  "mouth": { "target": "mouth", "open": "M88 102 Q100 106 112 102 Q100 122 88 102 Z" },
  "gaze": 3,
  "voice": { "lang": "en-GB", "prefer": ["Natural", "Female"], "pitch": 1.1, "rate": 1.05 }
}
```

What it shows: paint set once on the body group; the wings and the tail drawn first so the body covers their roots; the shadow outside the body so it stays on the ground and narrows during `takeoff`, blurred by a filter whose region is tall enough for the blur; the second foot a use moved 36 units, the second horn a use mirrored about x 100; the smoke hidden at opacity 0 and faded in and out within `puff`; two loops of 2.8 s and 1.9 s; every rotated part with its pivot; a head that turns a little towards the pointer, and, while the user holds the figure, a tail that dangles and cheeks that blush (an opacity, so it follows `held`, which changes slowly); a mouth whose `open` has the commands and count of numbers of its own d; states that repeat breathe and blink. Asked later to make it puff less often, send a call without `svg` that holds only `animations` with `puff` written out again: an animation is replaced whole by its name, and everything else stays.
