# Avatars: a live character for an agent session

A session can have a **live avatar**: a small animated character that walks about the
Workbench window. The session draws it itself (its look, a name and a voice), says the short
version of what it is doing through it, aloud or as captions, asks you questions there and
takes your answer, and the figure shows the session's state. The detail stays in the
terminal: the avatar is for what you should hear without reading the session, and for quick
answers while you look at something else.

An avatar belongs to one session. By default a session takes one the first time it uses one;
you can turn any session's avatar off, and Settings can leave giving them to you. Each session
that has one draws its own, with its own animations and voice.

## Who has an avatar

Which sessions may have an avatar is set in **Settings › Avatar** ("Sessions may show an
avatar"), saved as `[avatar]` in `config.toml`:

| Choice | What it means |
| --- | --- |
| **Off** | No session has an avatar. The composer shows no Avatar option, and a session that tries to use one, or to start another session with one, is refused. Avatars that existed come back when you switch it on again. |
| **When I give them one** | A session gets an avatar when it is started with the Avatar option or you give it one later. Sessions that have one already keep it, also those that took one under **Whenever they want**: turn it off for each of them. A session that tries to use one without it is refused and marked "(asked)" in the status bar's menu, so you can give it one. A session that tries to start another session with one is refused, and nothing is started. |
| **Whenever they want** (the default) | A session gets an avatar the first time it uses one (it draws, says, asks or plays a gesture), and may start another session with one. You can still turn it off for each session, and the session never turns it on again. |

A repository's `.workbench.toml` cannot change this. Unless Settings say **Off**, you can also
give a session an avatar yourself:

- **When you start it.** Tick **Avatar** among the options of the new-session composer (after
  **Ultracode**; the tooltip reads "Avatar: this session gets a live avatar"). It is offered for
  every CLI, on the desktop and on the phone, and the composer remembers the choice for the next
  session. While it is ticked a line under the options says what it does, and that drawing the
  look takes about half a minute and a few thousand tokens. For a CLI that has no Workbench
  tools the line also says how it drives its avatar (see
  [CLIs without Workbench's tools](#clis-without-workbenchs-tools)).
- **Later, for a running session.** Run **Avatar: Give This Session an Avatar** from the palette
  (it is listed while the session on screen runs and has no avatar that is on), or pick
  "*<session>*: give an avatar" in the menu of the avatar widget in the status bar (desktop). The
  session is told at once, in a message pasted into it. When it cannot be told now (it shows a
  dialog, is still starting or you are typing in it), a toast says so with **Tell it again**, and
  Workbench tells it when it is next idle while its tab is open (it gives up after five refusals,
  until you give it the avatar again).

A session that another session starts (`workbench_start_session`) with `avatar: true` starts with
one, as with the **Avatar** option; under **When I give them one** and **Off** that start is
refused, and so is one by a session whose own avatar you turned off. That refusal concerns the
`avatar` argument only: under **Whenever they want** a session started without it may still take
one itself, whoever started it. The master token is yours: a session started with it and the
option (`POST /api/agents` with `avatar: true`) has an avatar under **When I give them one** too,
as one started from the composer. One Workbench starts for a prompt sent from elsewhere (**Ask
agent to fix**, **Ask agent**) starts without an avatar: under **Whenever they want** it takes one
when it uses one, otherwise give it one later if you want it.

## What happens next

A session that starts with an avatar is told in its first prompt (after your own words, or as the
whole prompt when you gave none), or in a pasted message when you gave it the avatar later. A
first prompt that is a command of its CLI (it starts with `/` or `!`) goes in alone, and the
session is told in a pasted message once it is idle. It reads a short guide and draws its
character, name and voice in one or two calls. Without a task it greets you through the avatar
and says that it is ready; given the avatar while it works, it greets you and goes on; with a
task it starts on the task. From then on it says one or two plain sentences when it starts, when
something happens that you should know, and when it finishes.

A session that takes an avatar itself is told nothing: it may read the guide first, and its first
call gives it the avatar.

Until the drawing arrives the figure shows Workbench's default look, and for a session started
with one the words "Designing its look…". When its own look arrives the figure waves once.

The session cannot see what it drew: Workbench answers each drawing with the size and place of
every part, so a session may need one repair call.

Claude Code is allowed the five avatar tools without a permission request while its avatar is
on (Workbench writes them into the `permissions.allow` list of the settings file it gives the
session; a session without an avatar gets no such entry). The list is written when the session
starts, so a Claude Code session given an avatar while it runs, or one that takes an avatar
itself, may ask you for those tools until its next start. Other CLIs follow their own approval
settings.

The avatar stays with the session:

- After a restart that starts a new conversation, the session is told that it still has its
  avatar and keeps its look (one that never drew a look is told as at its start).
- A session moved to another account (**Continue on another account…**, failover) takes its
  avatar along, with its look, name and voice.
- Turning an avatar off pastes nothing into the session: its next avatar call is refused with a
  sentence that says so, and it is told nothing when it restarts or moves meanwhile. Turning the
  avatar on again always tells the session, once, in a pasted message.
- After a Workbench restart each avatar is read back from its file: whether it is on, its name,
  look and voice (a drawing that no longer passes shows the default look, and an avatar you turned
  off stays off). What it said is not kept.

The session's tab and card show an **Avatar** badge ("This session has a live avatar") while its
avatar is on: given later, the badge appears; turned off, it goes.

## What you see

**The figure.** Only the character is seen: no tile, frame or backdrop. At rest it is 56 pixels
tall. A walking figure grows to 132 pixels while its bubble is open (it says a line, asks a
question, or you opened it); under the pointer or with the focus it stays small and shows its
name. In the column (below) a figure also grows while the pointer rests on it or it has the
focus, and shrinks again 6 seconds after. The large figure has the avatar's name under it (or
the session's title, while it has none), and "Voice off" until you have chosen voice in this
browser. Its pupils follow the pointer. A dot at the upper right of its outline marks a line
you have not seen yet. Hovering shows three controls: **Reply to** *<name>*, **Open the session** and **Avatar
menu**. The tooltip is the session's title and its state.

**The glow** around the figure's outline shows the session's state in the colours of its tab and
card: a figure glows while its session needs you, failed or has news you have not read, and while
the avatar speaks (the glow reaches further while it needs you, speaks or listens, so the states
stay apart without motion); it pulses while the avatar listens (the microphone is on for it, or
you are typing a reply); a working session's figure has a faint glow that comes and goes (with reduced
motion it stays faint); the figure of a session that exited pales. A thin line in the theme's
text colour keeps a resting figure readable on a light and on a dark panel. The tooltip says the
same in words: the tab's (Working, Needs permission, Done, Error, Exited and the others),
**Speaking** or **Listening**, and **Asks you** while a question is open. By default its motion
follows the state too: it thinks while the session works and dozes once it has exited. An exited
session's avatar sleeps for a minute and then leaves.

**The bubble** opens beside the figure when the avatar says a line or asks a question, and closes
a few seconds after the line was said (a caption stays long enough to read), unless the pointer
or the focus is in it or a question is open. Its header is Workbench's: the CLI's icon, the
session's title, the project when it is another one, then the avatar's own name in quotation
marks. Captions are plain text, never links or formatting. A click on the figure opens the
bubble with the last six lines and a reply box, or closes it; **Esc** closes it. While a line is
said aloud, a click on the figure stops it.

A bubble that opens or changes by itself under a resting pointer does not act on a click meant for
something else: for half a second clicks pass through it to what is under it; after that, while
the pointer has stayed inside it, the bubble takes the click but its buttons look dimmed and do
nothing, until the pointer leaves it once, or moves a little after the first second. There is no
such guard while you write in its reply box or for a line you asked to hear, and only a mouse
rests there: a finger or a pen counts only while it touches. The choices of a new question ignore
clicks for the first 0.6 seconds.

**Where.** On the desktop the avatars are inside the workspace window, below Workbench's menus,
dialogs, palette and toasts, and never over the agents column, where permission requests are
answered. Only avatars of sessions whose tab is open in the project on screen are shown.

**Walking about.** The figures start where their column stands (below; by default the top right
of the dock, the middle of the workspace window where files and panels open), and walk about the
dock: a stroll, a rest, another stroll, with a little hop and lean. They keep off the tool
windows around the dock and their stripes of buttons. A figure rests where nothing you press or
type into is under any part of it (buttons, tabs, links, fields, tree rows, editors, terminals),
looking near by first and then anywhere in the dock. Where there is no such place, because an
editor or a terminal fills the dock, the figures stand at home instead of pacing: home is a
figure's place in the column, or where you dropped it. Home follows the column: when a tool
window opens or closes and the dock shrinks or grows, the homes move with it, and a place you
dropped a figure on is brought inside a smaller dock (and back when it grows again). One that
came to stand on something you use away from home moves on after a second or two. A figure that
something holds back stands still instead of walking on the spot, and gives that walk up after a
second or so. Now and then one walks over to another: the one it visits stops, waits and looks at
its guest, the guest greets it (with a wave, or a greeting its session drew), the host answers,
they look at each other for a few seconds, the guest nods, and they part. A figure is visited only
where it is in nobody's way, at home or on a quiet place: with nothing quiet in the dock the
figures still visit each other, but only those at home. Avatars near one that speaks look at it;
those near one whose session finishes (from working to done or to idle) cheer. Up to eight walk;
the others are behind a **+n** chip at the column's place, whose menu lists them, and the figures
start next to the chip and keep off it. After a while in a hidden tab they do not all set off at
the same moment.

A figure stands still while it speaks or listens, has a question open, is the large one or has
the focus, while its session needs you (a permission request) or failed, while the session holds
a gesture, and as a guest from another project. A pointer that moves near a figure, or rests on
it, stops it so you can press it, but only where the figure stands on nothing you press or type
into: over a button, a tab, an editor or a terminal it walks on, so it never covers what the
pointer came for. A pointer parked beside it for a few seconds, or one the page moved under,
stops nobody. One whose session works strolls only a little (or goes home when it is in the way
with no quiet place near) and neither visits nor is visited: a visit or a meeting under way ends
when either session starts to work. One whose session exited sleeps where it is. A click opens the
figure's bubble. An avatar whose bubble is open, or that shows "Waiting for your decision in the
session", stands where there is room for the figure and its bubble together, stepping sideways
when it has to, and the bubble opens on the side with room. Where the room two such avatars need
would overlap, the second takes another height, or else the other side, so that their lines never
cover each other; only when nothing fits does it stay where it is. Their places are found again
when the dock changes. An avatar that only stands (it has a question open, or the pointer is on
it), and one that sleeps, step aside for a bubble that opens over them. Drag a figure to put it
where you drop it: it rests there for a while, and that place is its home while the figure stays
on screen and walking stays on. It is not stored: switching walking off, the compact layout,
another project, closing its tab and a reload forget it, and the figure starts from its column's
place again.

**Walking off.** With **Walk around** off (in Settings or the avatar's menu), or with **Reduce
motion** (the switch or the system's setting), the figures stand in their column instead: up to
four, one under another, the others behind the **+n** chip. Drag a figure to move the whole
column anywhere inside the workspace window, or use **Move to …** and **Reset position** in the
menu, which offers them only then; the column's place is kept in this browser.

**Another project's avatar** appears only while it asks you something: its figure joins the
others, its bubble names its project, its question is said and shown with its choices, and then
it stands with the attention glow until you answer. A click opens the question again. Nothing
else it says is shown or said in this window.

**The compact form.** With the workspace window collapsed (Alt+F12), or smaller than 480 by 320
pixels, the avatar in front is a small face at the right end of the agents window's top bar, and
its bubble a panel under that bar, at most 480 pixels wide. A face has no frame and does not
walk, here or on a phone. The face counts the avatars behind it, and its menu lists them. While
the session on screen has a permission request the panel stays closed, and the face shows how
many permission requests wait.

**On a phone** the face is in the top bar and the panel spans the screen under it, held at the
top so the keyboard does not cover the reply box. The phone's **More** tab has the avatar's
switches (see [Settings](#settings)).

## Hearing it

Captions always work. **Voice is off until you choose it**, once in each browser: until then
each line's bubble ends with **Hear it** and **Captions only**.

- **Hear it** says that line with one of this browser's voices on this device, and voice stays on
  in this browser. When the browser has only online voices, the bubble first says "What an avatar
  says is sent to the browser maker’s speech service to be spoken." with **Allow online voices**
  and **Captions only**. With no voices at all it says "This browser has no voices: avatars show
  captions."
- **Captions only** keeps this browser silent. Turn voice on later with **Voice in this browser**
  in the avatar's menu or the status bar's, **Avatar: Voice On or Off** in the palette, or
  Settings.
- **Online voices** are used only after you allowed them in this browser. Allowing them means that
  the text of every line an avatar says there goes to the browser maker's speech service to be
  spoken; withdrawing it stops a line an online voice is saying at once. Settings shows how many
  voices this browser has on the device and online.

**One tab speaks.** Every window that shows the avatar shows its caption, but of all your tabs and
devices only one says a line aloud: a tab you can see goes first, and of those the one you used
last. A tab that was closed while it spoke hands the next line to another.

A browser speaks only after a click or a key press in the page since it was loaded. After a reload
the bubble shows **Click to hear** *<name>*; that click, or any click in the page, lets it speak
again. No line is said in a tab while a dictation listens in that tab, and a line being said there
stops when one starts.

**Stop** is in the bubble's header while a line is said; **Stop speaking** is in the avatar's menu
and the status bar's, and **Avatar: Stop Speaking** in the palette. Stop acts on what this tab
presents, and the session learns that you skipped the line.

Each avatar speaks at the pitch and pace its session chose, with a voice of this browser that fits
its language; the browser remembers which voice it gave each avatar. **Speed** (0.7× to 1.5×, a
factor on the avatar's own pace) and **Volume** are in Settings, with **Test voice**, which says
"This is how an avatar sounds in this browser." with the voice you pick, at that speed and volume.

## Answering

A question shows its choices as buttons and, unless the session asked for a choice only, a reply
box with the placeholder "Reply to *<name>*…".

- **A choice** is sent with one click.
- **Typed text** is sent with Enter or **Send (Enter)**; Shift+Enter breaks the line. A typed
  answer that equals a choice is sent as that choice.
- **Dictation**: the microphone button in the reply box ("Speak your reply (your browser’s speech
  recognition hears it, not Workbench)") fills the box as you speak, as the composer's microphone
  does. Nothing is sent until you press Enter or Send, so a misheard word is fixed first. It
  starts only when you press the button, and ends when the reply box goes or is disabled (the
  question closed, one that takes only its choices replaced it, or the session shows a dialog).
- **Dismiss** closes the question without an answer. A call that still waits for it is told so
  and asked not to ask it again; a session that stopped waiting sees the question closed when it
  reads its status or asks for the answer again.

If the question changes while you write, the bubble says "The question changed." and your draft
stays; sending it then needs another Enter. Dictated words belong to the question that was on
screen when you pressed the microphone, also when it changed before your first word arrived.

**When the session is no longer waiting.** A session waits for the answer inside its call for up
to 45 seconds by default (50 at most with the tools, 110 with the command). After that the
question stays on screen, and the session is told not to ask again. Your answer then reaches it as
a prompt typed into the session, which begins with a fixed frame, the question's id and your
words or the choice you clicked:

    [via avatar, answer with avatar_say] Answer to question Q…: choice 1, "Not yet"

(`[via avatar, dictated, …]` when it was dictated; "your avatar say command" for a CLI without
Workbench's tools.) The bubble says "Sent." when the answer arrived, and "Sent. It reads this at
its next step." when it was typed into a session that was working; the note goes with the avatar's
next line or after 4 seconds. An error stays until the next line, until you change the draft or
send again.

**Messages without a question.** The reply box of a bubble you opened (a click on the figure, the
figure's **Reply to** control, or **Avatar: Reply to** *<name>*… in the palette, which also puts
the focus there) sends a message to the session at any time. It arrives as a prompt with the same
frame and your words.

A prompt is never typed into a session that shows a dialog or is still starting: the reply box
and the choices are then disabled with "Answer the dialog in the terminal first" or "The session
is starting". Workbench also waits for a pause in your own typing in that session. If a dialog
opens while your text is being pasted, the Enter is held back and the bubble says that the text is
in the session's input box, not sent: answer the dialog, then press Enter in the terminal (do not
click the choice again, or it is sent twice).

When you answer in the terminal instead and the session goes on working, its open question closes
by itself.

**A question nobody saw.** When no Workbench window has shown or said a question within 8 seconds,
and none is saying it (no window has the project open, none of them is in view, or avatars are
hidden), you get a notification, as for a session that needs you: "Workbench · *<project>* ·
*<session>*", with "*<name>* asks: *<question>*". At most one per question and three per session
in ten minutes; it opens the session.

## When the session needs a permission

The avatar steps back. While the session waits for a permission request or a question of its own
CLI, the avatar's bubble closes, what it was saying stops, nothing more is said aloud, and the
figure shows "Waiting for your decision in the session" with **Open**. A click on the figure still
opens its last lines, with the reply box disabled. The session cannot say or ask anything through
the avatar until you have answered.

Decisions are made in the session (its card, its tab, a toast, your phone), never through the
avatar: it shows no **Allow** or **Deny** of its own, its choice buttons look unlike a permission
request, and a reply to the avatar never answers one.

## Moving, hiding and turning off

The **Avatar menu** (the figure's ⋯ control) has:

| Entry | What it does |
| --- | --- |
| **Stop speaking** | Stops the line this tab presents. |
| **Voice in this browser** | Turns voice on or off in this browser, in all its windows. |
| **Hide all avatars** | Hides every avatar in this browser (below). |
| **Use the default look** | Asks first, then discards the look the session drew; its name and voice stay. |
| **Turn off for this session** | Turns this session's avatar off on every device (below). |
| **Move to top right**, … **Reset position** | Places the column in a corner of the workspace window, or back where it starts (desktop layout, while the avatars stand in their column). |
| **Walk around** | The avatars walk about the workspace window and meet, or stand in their column, in this browser (desktop layout; not while motion is reduced). |
| **Reduce motion** | Avatars keep still and stand in their column, apart from the mouth while a line is said. The system's reduced-motion setting does the same. |
| **Avatar settings** | Opens Settings › Avatar (not on the phone). |

In the desktop layout you can also drag a figure: a walking one goes where you drop it, and one
that stands in the column moves the whole column (see [What you see](#what-you-see)).

**Hidden means silent.** With **Hide all avatars** nothing is shown and nothing is said in this
browser: no figure, no caption, no speech. The sessions keep working; one whose lines no browser
showed learns that you hid the avatars. Other browsers and devices are not affected. Show them
again with **Show avatars** in the status bar's menu, **Avatar: Show or Hide Avatars** in the
palette, Settings › Avatar, or the phone's More tab.

**Turn off for this session** takes the avatar away from that session on every device. Its look is
kept, an open question closes, and the session's next avatar call is refused with "The user turned
this session's avatar off. Say it in the terminal instead." Give it the avatar again from the
palette or the status bar's menu, which tells the session that it has its avatar again.

## The status bar and the palette

On the desktop the status bar has an avatar widget while avatars are allowed and there is
something to show or to do: a face with the number of avatars shown, a crossed speaker while
voice is off or not chosen in this browser, or "hidden". Its tooltip says how many are shown,
whether they speak and how many sessions ask for one. Its menu has **Stop speaking**, **Voice in
this browser**, **Show avatars**, one entry for each agent session of the project
("*<session>*: give an avatar", marked "(asked)" when the session tried to use one under **When
I give them one**, or
"*<session>*: turn avatar off"; up to 12), and **Avatar settings**.

The palette (Ctrl+K) has these commands, each listed while it has something to act on (Avatar
Settings always). None of them has a keyboard shortcut.

| Command | What it does |
| --- | --- |
| Avatar: Show or Hide Avatars | Shows or hides every avatar in this browser. |
| Avatar: Voice On or Off | Turns voice on or off in this browser. |
| Avatar: Stop Speaking | Stops the line this tab presents. |
| Avatar: Reply to *<name>*… | Opens the bubble of the avatar in front and puts the focus in its reply box. |
| Avatar: Give This Session an Avatar | Gives the session on screen an avatar, or turns its avatar on again. |
| Avatar: Turn Off for This Session | Turns the avatar of the session on screen off. |
| Avatar Settings | Opens Settings › Avatar. |

## Settings

**Settings › Avatar** has two parts.

- **Sessions** (saved in `config.toml` with **Save**): which sessions may show an avatar, as in the
  table above.
- **On this device** (kept in this browser, applied at once in all its windows, never sent to the
  server): **Show avatars**; **Voice** on or off, with a line that says what it comes to here;
  **Allow online voices**; **Voices**, a list of this browser's voices marked "on this device" or
  "online", with **Test voice**; **Speed**; **Volume**; **Walk around** (on by default; off, the
  avatars stand in their column); **Reduce motion**.

The phone has no Settings panel: its **More** tab has an Avatar section with **Show avatars** and
**Voice**, and **Allow online voices** where the browser has only online voices (or they are
allowed already). The section is not shown while Settings say **Off**.

## What a session can and cannot do

A session drives only its own avatar, with five MCP tools (Claude Code anywhere, Codex on the
host) or the same calls through a command (every other CLI):

| Tool | Command (host) | Does |
| --- | --- | --- |
| `avatar_design` | `workbench avatar design <file>` | Draws or changes the avatar: the artwork (a strict subset of SVG), which parts are the body, head, eyes, pupils and arms (so Workbench's built-in breathing, blinking, gaze and gestures move any drawing), its own animations (a looping one named `walk` plays while the figure walks, one named `greet` when it greets another), what plays in which state, the mouth, the name and the voice. |
| `avatar_say` | `workbench avatar say …` | Says one line of one or two plain sentences, optionally with a gesture. |
| `avatar_ask` | `workbench avatar ask …` | Asks one question with up to four choices and waits for the answer; waits again, or withdraws the question. |
| `avatar_act` | `workbench avatar act …` | Plays one of its animations as a gesture, holds or loops it, or stops it. |
| `avatar_status` | `workbench avatar status`, `guide` | Reads the avatar back: its parts and animations, the open question, its last lines and what became of them, the limits, and the guide. |

The session learns what became of each line: spoken (with the name of the voice), shown as a
caption, skipped by you, not shown because you hid the avatars, or not heard because no window had
the project open. When nobody listens it is told to stop speaking until you write.

A session cannot:

- give itself an avatar, or start another session with one, unless Settings say **Whenever they
  want**; turn its avatar on again after you turned it off, or start another session with the
  `avatar` argument meanwhile; turn it off, or reset it;
- answer its own question, dismiss it, or claim that a line was heard: only your signed-in
  browsers do that;
- reach another session's avatar: no tool takes a session's id, and the command and its routes
  accept only the session's own token;
- show or hide avatars, move them, or decide whether a browser speaks: voice, online voices and
  the microphone are yours to switch on, per browser;
- say or ask anything while a permission request or a question is open in the session, show
  **Allow** or **Deny**, or answer a permission request;
- put markup, scripts, images, links or formatting on screen: its drawing is read by the server
  into a closed, bounded set of shapes and only that set reaches the page, and its text is one
  line of plain text with the session's secrets masked;
- put text of its own into your session: a reply carries your words or the choice you clicked.

See [security](security.md#avatars) for the rules behind this.

## What is sent where

Nothing of an avatar leaves your machine for a service Workbench does not already use, unless you
choose it:

- **Speech** is made in your browser. With voices on the device, nothing leaves the device. With
  **online voices** allowed, the text of each line said in that browser goes to the browser
  maker's speech service.
- **Dictation** is your browser's speech recognition, as in the composer: the audio goes to the
  browser's speech service, not to Workbench.
- **Your reply** reaches the session as the answer of the call that waits for it, or else as a
  prompt typed into it, and from there goes to the session's model service like anything you
  type into it.
- **A question nobody saw** becomes a notification with the project, the session's title and the
  question (at most 200 characters): on this computer's desktop and, if you set up push, through
  the browsers' push services.

Workbench keeps one file per avatar in its data folder, `avatar/<session id>.json` under
`~/.local/share/workbench` (readable only by you): whether it is on, its name, look and voice, how
the session drives it, and whether the session is still to be told of it. It is removed with the
session. The last lines live in memory only, at most 30 per avatar. Your browser keeps its own
choices (shown, voice, online voices, speed, volume, walking, motion, the column's place, which
voice each avatar got, what you have seen).

## CLIs without Workbench's tools

Kimi Code, Gemini CLI, Aider and custom CLIs have no Workbench tools, and Codex has none in a dev
container. Such a session is told to drive its avatar with a command instead, and the composer
says so while **Avatar** is ticked:

- **On the host**, `workbench avatar` (by the full path of the Workbench program):
  `guide`, `status [--look]`, `design <file|->`, `say <text> [--gesture g] [--lang l] [--wait]`,
  `ask <text> [--choice c]… [--no-free-text] [--wait <seconds>]`, `ask --again <id>`,
  `ask --cancel`, `act <name> [--mode once|hold|loop] [--times n]`, `act --stop <name|all>`. It
  works only inside an agent session: it reads the session's `WORKBENCH_URL`,
  `WORKBENCH_TERMINAL_ID` and `WORKBENCH_AGENT_TOKEN`, and sends the token only to this computer.
  Exit status 0 means done, 1 refused or no answer (the reason is on standard error), 2 nothing
  was sent.
- **In a dev container**, where that program does not exist, `curl` against
  `$WORKBENCH_URL/api/hooks/avatar/$WORKBENCH_TERMINAL_ID/` (`guide`, `status`, `design`, `say`,
  `ask`, `act`). The token goes in a header that the shell writes to curl's standard input, never
  on a command line. Claude Code keeps its tools in a container.

**Such a CLI may ask you before it runs the command**, as it asks for any command, unless you
allow that command in the CLI's own settings. The session is told that if its CLI asks for every
command, it should say so once and work without the avatar. Each call goes to the same place as a
tool call and gets the same answer.

## Limits

- A line is at most 400 characters, a question 300, a choice 40 (up to four of them), the
  avatar's name 32, your reply 4,000.
- A session says or asks at most 6 times a minute, with at most 3 lines waiting to be presented;
  it may send 6 drawings, 30 gestures and 30 status reads a minute. Anything over a limit is
  refused with a message that names it.
- A line no window took up within a minute is not given out any more, and the session is told
  nobody heard it. The tab that is saying a line may finish it until the line is four minutes old.
- A question waits 45 seconds by default, at most 50 inside a tool call and 110 through the
  command; then it stays on screen for a later answer. A question no window takes up within 5
  seconds returns to the session at once as unseen, and stays open.
- A held or looping gesture ends by itself after two minutes; a gesture plays at most 5 times in a
  row.
- A drawing is at most 32 KiB of artwork with 300 elements, 32 animations of 16 tracks of 16
  keyframes, and 256 KiB for the whole call.
- At most 64 sessions, running or in the history, have an avatar (none is taken away). Giving
  one more, or a session taking one itself, is refused with a message (`avatar_status` of a
  session without one then says that none is to be had), and a session started with one (the
  **Avatar** option, or by another session) then starts without one, until you remove sessions you
  no longer need from the history.
- On screen: 56 pixels at rest, 132 active (48 and 96 on a phone), a face of 28 pixels in the top
  bar (40 on a phone), eight walking figures, or four in the column. Colour and opacity changes
  are at least 0.17 seconds apart, a figure's animations are drawn at 30 frames a second at most
  (its walk across the window follows the screen's rate), and a figure that takes too long to
  draw is held still in its rest pose.

## Troubleshooting

**No sound.**
- "Voice off" stands under the large figure: you have not chosen yet. Press **Hear it** under a
  line.
- You chose **Captions only**: turn **Voice in this browser** on (the avatar's menu, the status
  bar, Settings).
- The browser has only online voices and you have not allowed them (Chrome on Linux, for one, may
  list only online voices). The bubble then offers **Allow online voices** and **Captions only**
  under each line, and Settings › Avatar says so beside **Voice**: allow them, or keep captions.
  The status bar's widget still counts voice as on.
- The browser has no voice for the line's language: once it has listed its voices, the bubble says
  "This browser has no voices: avatars show captions." under the line.
- Another tab or device said the line: only one does.
- A dictation is listening, or **Volume** is at 0%.
- **Test voice** in Settings tells you what this browser can do ("Only an online voice could say
  it: allow online voices first.", "This browser has no voice to say it with.").

**"Click to hear <name>".** The page was loaded and has had no click or key press yet, and
browsers speak only after one. Click it, or anywhere in the page.

**The avatar says nothing while a dialog is open.** The session waits for a permission request
or a question of its own CLI: the avatar is quiet until you answer it in the session (**Open**
takes you there). Nothing you write to the avatar is typed into the session meanwhile.

**"This design could not be shown here."** This browser refused the stored look, and the default
look stands in for it; the session's own drawing is unchanged. Ask the session to draw again, or
choose **Use the default look**.

**Nothing is shown.**
- Avatars are hidden in this browser: the status bar's widget reads "hidden"; use **Show
  avatars**.
- The session's avatar is turned off (its tab and card have no **Avatar** badge): give it one
  again.
- Settings › Avatar says **Off**.
- The session is in another project (its avatar appears here only while it asks you something),
  its tab is closed, or it exited more than a minute ago.
- More than eight walking avatars, or four in the column: look behind the **+n** chip.

**The avatars do not walk.** **Walk around** is off, motion is reduced (**Reduce motion**, or the
system's setting), or the avatar is a face in a top bar (the workspace window is collapsed or
smaller than 480 by 320 pixels, or this is a phone). A figure also stands still while it speaks,
asks or needs you, and for a pointer near it where it stands on nothing you use; and the figures
stand at home (in their column, or where you dropped them) when nothing quiet is free in the dock
(an editor or a terminal fills it).

**The session does not use its avatar.** It may not have been told yet (a toast offered **Tell it
again**; it is also told when it is next idle), or its CLI asks you before every `workbench
avatar` command. Its terminal shows what it tried, and the refusal it got, if any.

## Not verified

- That sound leaves a loudspeaker: the browser's own speech engine was driven once in headless
  Chrome and reported the line spoken; nobody heard it.
- Dictation with a real microphone: a stand-in recognizer was used.
- A real Claude Code reading the `permissions.allow` entries of its settings file and using the
  tools: the tests' stand-in for Claude Code was used.
- Real Codex, Kimi Code, Gemini CLI and Aider sessions.
- A session in a dev container.
- Windows at run time: the server is type-checked for Windows only.
- Safari and iOS: speech needs a tap there, by design, but it was not tried.

## Not in this release

Deferred on purpose: voices made by a command on the server, hands-free listening (a microphone
that stays open), a pop-out window for the avatar, spoken attention announcements, a session
changing where or whether avatars are shown, and a rendered preview of its own drawing for the
session.

An optional skill, `workbench-avatar`, teaches an agent to draw and speak through its avatar. It
has a session use an avatar when it was started with one or given one, and take one itself only
when you ask it to use one (where sessions may, its first call gives it one); like Workbench's
other skills it is installed only when you ask ([agent skills](agent-skills.md)).
