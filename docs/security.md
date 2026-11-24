# Security and privacy

Workbench can run commands, read your code and reach your forges with your tokens, so it
treats every way in as something to check. This page is the short version; the complete
model is in [the architecture](ARCHITECTURE.md#security-model).

## Who can use it

- **Only signed-in browsers.** A browser signs in with a one-time code (`workbench open`,
  a pairing QR code) or the master token in `~/.local/share/workbench/token`
  (`%LOCALAPPDATA%\workbench\token` on Windows), and gets a
  session cookie plus a device key. Anything that changes something needs both, so another
  local web page cannot act on your session.
- **Only expected hosts.** Requests for any host name other than loopback, your bind
  address and the names you list are refused, which defeats DNS rebinding.
- **Every device can be revoked** (Settings › Remote access); revoking closes its open
  connections at once.
- **A signed-in device is fully trusted.** It can type into terminals, which is running
  code. Expose Workbench beyond your machine only over TLS and to devices you pair.

## What agents can and cannot do

- A hosted session gets its own token, valid only for Workbench's hooks and MCP endpoint,
  and every MCP tool but the debug ones is confined to the session's project.
- Agents never answer permission requests, deploy, run destructive git operations, start a
  language server, query a database, or update or restart Workbench through Workbench. Run
  configurations that deploy or release are refused to them.
- Agents drive the debugger: start, attach, rerun, step, evaluate expressions and set
  conditional breakpoints (`debug_*`). These are writes, so the agent's own permission prompts
  and the Activity view treat them as such. They are not confined to the session's project:
  an agent may name any project. A debugger's expressions can run commands and a
  start runs the configuration's build step, on your computer and outside any sandbox the
  agent's CLI keeps its own commands in: Workbench does not refuse a sandboxed agent (Codex)
  these, so that CLI's sandbox and approvals are what hold it. A pre-launch run that deploys
  or reaches another host is refused to everyone.
- Workspace reports an agent writes are served sandboxed: they cannot read Workbench's
  cookies, storage or API.
- Skills are instructions an agent follows, so adding one is yours to decide: agents have no
  tool to install or remove a skill, and the install route refuses their requests. Workbench's
  own optional skills are installed only when you press **Install**, only into an agent
  account's own skills folder, and only the files Workbench wrote are ever changed or removed.
  The Skills window reads skills and never runs anything in them; a repository's skills are
  untrusted text, shown like any other repository Markdown.
- Agents fill the data viewer and make its plots (`data_*`, confined to the session's
  project): that is what it is for. `data_follow` reads only files inside the project, and
  not hidden folders, key-like names or anything the project marks `sensitive`. A script in
  the session may post samples with the session's own token; it can only write that
  session's project.

## Avatars

A session's [avatar](avatar.md) is drawn and worded by the session, so everything it sends is
untrusted:

- **The drawing never reaches the page as markup.** The server reads the session's SVG into a
  closed, typed, bounded tree and stores only that: shapes, groups, short texts, copies of them
  (`use`), gradients, clip paths, masks, filters of a fixed set of steps, and dashes. Every
  reference in it is a bare id that must name an element of the same drawing, of the right kind:
  a copy repeats only a part of this drawing that holds no copy itself, and a filter's steps
  only blur, shift, colour, combine, add noise to or distort what is drawn. Scripts, styles,
  images (also inside a filter), links, patterns, symbols, embedded HTML, event attributes and
  anything that reaches outside the drawing are refused. The browser checks the tree again and
  builds the figure with Workbench's own tables of elements and attributes, inside a fixed,
  clipped box of at most 132 pixels (the box itself is not drawn), below Workbench's menus,
  dialogs and toasts and never over the agents column; walking figures keep to the middle of the
  workspace window. A figure that costs too much to draw is held still.
- **Nothing flashes.** The validators keep colour and opacity changes at least 0.17 seconds
  apart, never jumping, springing or bouncing, and let colours and opacities follow only the
  figure's walk, its turn and a drag, which change slowly, in two values (three along the turn),
  never the pointer. That is the first line; the bound is in the browser's animation engine,
  which everything the figure shows passes through: a fill or stroke (its brightness too) or an
  opacity on any part, and every other change of a large part, may turn back only 150 ms after
  its last turn, so nothing
  shows more than three complete flashes a second, also under reduced motion, whatever the
  session's states, gestures, drives and mouth combine to. Not covered: two opacities that
  cancel out on one part or down its tree, and a colour and an opacity or fill-opacity of one
  part or down its tree that change together.
- **Pictures for the session are drawn in a process of their own.** So that a session can see
  what it drew, Workbench draws its stored look as a PNG. The renderer reads the stored look
  again as untrusted data, writes the SVG it renders from its own tables of elements and
  attributes (every value parsed and written again, ids printed only when they resolve inside
  the drawing), and resolves nothing external: no files, no images, no network, fonts only from
  the system. Its estimate of what a picture costs answers most costly looks with a sentence
  first, but the bound is the process: the server never renders. Each picture is drawn by
  Workbench's own program started again for it (`workbench avatar-draw`), which before it reads
  anything limits itself to 1 GiB of address space and 6 seconds of processor time, writes no
  core file, runs at the lowest priority and ends with its parent (Linux); the server kills it
  after 8 seconds, or when the caller goes away. Whatever a look does to the renderer, one
  picture is lost and the session is told so. On Windows only the wait and the kill apply. A
  design call draws the new look there before it stores it and refuses one that cannot be drawn.
  Two pictures are drawn at a time for all sessions, one at a time per session, and after a
  drawing that died of its look that session's pictures pause for half a minute. A picture goes
  only to the session that asked for it, of its own avatar, and none is drawn for an avatar that
  is off.
- **Text is one line of plain text.** A line, a question, a choice, the avatar's name and the
  words in its drawing lose line breaks and control characters, have the session's secrets
  masked, and are shown as text, never as Markdown or links. A refusal that quotes what the
  session sent masks it as well, and so does the Activity view's record of a failed tool call.
  The session's identity (its CLI, title and project) is shown by Workbench; the agent's own
  name follows in quotation marks.
- **An avatar belongs to one session.** Its tools act only on the calling session's own avatar
  and take no session id; the hook routes under `/api/hooks/avatar/<session>` accept only that
  session's own agent token, and refuse the master token. On `/mcp` the master token, which is
  yours, may name a session in the `X-Workbench-Terminal` header and then calls the avatar tools
  as that session, as it calls every other tool: no session can do that for another.
- **It is your choice.** By default (`[avatar] sessions = "any"` in `config.toml`) a session
  takes an avatar with its first avatar call and may start another session with one; with
  `sessions = "asked"` a session gets one only when you start it with the Avatar option or give
  it one (those that have one already keep it until you turn each off), and a session that tries
  to start another with one is refused. You can turn any session's avatar off, and the session
  never turns it on again, nor starts another session with the `avatar` argument. That refusal
  concerns the argument only: under `"any"` a session it starts without one may still take one
  itself. The master token is yours: a session started with it and the Avatar option has an
  avatar, as one started from the composer. An avatar whose stored file Workbench cannot read
  stays off until you turn it on. `enabled = false` turns every avatar off. A
  repository cannot set this.
- **Only your devices answer.** Replying, dismissing a question, turning an avatar on or off,
  resetting its look and telling the server that a line was heard are for signed-in devices
  only: the master token is refused there, and so are agent tokens. Nothing a session sends
  through its avatar is typed into a session by itself. What a reply pastes is a fixed frame, the
  question's id and your own words, or the choice you clicked: no other text of the session's.
  The only other text pasted is Workbench's own brief, when a session is told of its avatar (at
  once when you give it one or turn it on, else when it is next idle).
- **It never touches permission requests.** It shows no Allow or Deny, its choices look unlike a
  permission request, and no reply goes to one. While a session waits for a permission or an
  answer its avatar says and asks nothing. A reply is never typed into a session that shows a
  dialog or is still starting, and the Enter after a paste is held back when a dialog opened
  meanwhile (the echo of what Workbench typed there does not count as a dialog; a dialog beside
  it does).
- **Sound is yours to choose, per browser.** Captions always work; voice is off until you choose
  it with a click. Online browser voices are used only after you allowed them, in a question that
  says that what an avatar says is then sent to the browser maker's speech service. Nothing is
  said in a tab while a dictation listens in that tab, and dictation starts only when you press
  the microphone button; dictated text is sent only when you send it.
- **Claude Code may call the avatar tools without asking** when it starts with its avatar on:
  Workbench adds the five `mcp__workbench__avatar_*` rules to the settings file it starts the
  session with, and none for a session without an avatar (they change at the session's next
  start). CLIs without Workbench's tools run `workbench avatar`, which sends the session's token
  only to this computer; in a dev container `curl` reads it from standard input, so it is on no
  command line.
- **Everything is bounded:** the size of a drawing and of every text, its filters, copies, drives
  and parts that move with another, lines and calls a minute (a picture counts as a status call),
  the memory and time of a picture and how many are drawn at once, waits, sessions with an
  avatar. A refused call changes nothing and says which limit it met.

## What repositories can and cannot do

A repository's own files (`.workbench.toml`, detected configuration, `devcontainer.json`)
may come from someone else's branch or a clone, so they are untrusted:

- they never define secrets, and the secret *names* they use resolve only against your
  machine-local overlay, never your global tokens;
- they never loosen an agent's permissions;
- they never make Workbench run anything by itself: runs, deploys and log commands start
  on your click, like any IDE's run configurations;
- a dev container is built only after you approve its exact plan; any change asks again;
- language servers, debug adapters and debug servers (OpenOCD, J-Link…) come only from your
  `config.toml` and start only after you enable them or press Debug; a repository's
  configuration can name a debug server and add arguments and gdb commands, which run only
  when you start that configuration;
- they never say where Workbench's own updates come from: only the release build and your
  `config.toml` do.

## Your secrets

- Configuration holds secret *references*; values are resolved on the server when needed.
- Values never reach the browser, a command line or a log. Terminal output and approval
  prompts mask them.
- Connection errors, upstream error bodies and database errors are redacted.
- Token files that others can read are flagged in Settings › Secrets, with a fix.

## Your data

Workbench keeps its state on your machine in `~/.local/share/workbench` (mode 0600; on
Windows `%LOCALAPPDATA%\workbench`, whose access list admits only you and SYSTEM) and
talks only to the services you configure. Push notifications go through the browsers' own
push services, encrypted, with titles and short summaries only. There is no telemetry.
