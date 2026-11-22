# Changelog

## 0.23.0 - 2026-11-20

Avatars: an agent session can have a live character of its own that walks about the Workbench window. The
session draws it, says the short version of its work through it, asks you questions there and takes your
answer, and the figure shows the session's state. By default a session takes one when it first uses one,
you can turn any of them off, and no browser speaks until you choose voice there. See `docs/avatar.md`.

- **A live avatar for a session.** A session gets one with its first avatar call, or you give it one:
  tick **Avatar** among the new-session composer's options (after Ultracode, for every CLI), or give a
  running session one later with **Avatar: Give This Session an Avatar** in the palette or from the status
  bar's avatar widget. A session given one is told in its first prompt (or in a pasted message when it
  gets the avatar later), draws its look, name and voice in one or two calls, about half a minute and a
  few thousand tokens, and greets you. A first prompt that is a command of its CLI (it starts with `/` or
  `!`) goes in alone, and the session is told once it is idle. Its tab and card show an **Avatar** badge.
  It keeps its avatar across a restart, a new conversation and a move to another account. Turning an
  avatar off pastes nothing into the session; turning it on again tells the session, once.
- **Walking about.** In the desktop layout the figures walk about the workspace window's dock (never over
  the agents column or the tool windows), rest where nothing you press or type into is under them (or
  stand at home, in their column or where you dropped them, when there is no such place, and visit only
  each other there), now and then visit and greet each other (not while their session works), look at one
  that speaks and cheer for one that finishes. A figure stands still while it speaks, asks or needs you,
  and for a pointer near it unless it stands on something you use; it stays small under the pointer and
  grows when its bubble opens; two open bubbles never cover each other. Up to eight walk. With **Walk
  around** off (Settings or the avatar's menu) or **Reduce motion**, they stand in a column of up to four
  instead, with **Move to …** and **Reset position**. Only the character is seen, with no box around it.
- **What a session does with it.** Five MCP tools, for Claude Code anywhere and Codex on the host:
  `avatar_design` draws and rigs it (a strict SVG subset, plus JSON for its rig, animations, mouth and
  voice), `avatar_say` says one or two plain sentences, `avatar_ask` asks a question with up to four
  choices and waits for the answer, `avatar_act` plays a gesture, `avatar_status` reads it back with the
  authoring guide. Workbench's built-in breathing, blinking, gaze and gestures move any drawing that names
  its body, head, eyes, pupils and arms; a look's own `walk` and `greet` take the place of the built-in
  hop and wave. A glow around the figure and its tooltip show the session's state in the words and
  colours of its tab. Claude Code may call the five tools without a permission request when it starts with
  its avatar on (`permissions.allow` in its settings file, which changes only at its next start).
- **Hearing it.** Captions always. Voice is off until you choose it in a browser (**Hear it** or
  **Captions only** under the first line), and uses that browser's own voices. Online voices are used only
  after **Allow online voices**, which says that what an avatar says is then sent to the browser maker's
  speech service. Of all your tabs and devices one says a line aloud (a tab in view, the one used last),
  and no tab while a dictation listens in it. **Stop** is in the bubble and the menus; **Test voice**,
  speed and volume are in Settings; **Reduce motion** is in both.
- **Answering it.** A question shows its choices and a reply box for typed or dictated text (the browser's
  speech recognition; nothing is sent until you press Enter or Send). The answer goes to the call that
  waits for it; once the session stopped waiting, it is typed into the session as a prompt that starts
  with `[via avatar, …]`. The same box sends a message to the session at any time. A question no window
  showed within 8 seconds becomes a notification.
- **Who may do what.** An avatar belongs to its session: a session takes one itself only while
  `[avatar] sessions = "any"` (the default; `"asked"` leaves it to you), never turns on one you turned off,
  and cannot answer for you; its tools and hook routes reach only its own avatar. Replies, dismissing,
  turning on and off, resetting and telling the server a line was heard are for signed-in devices only.
  The drawing is untrusted: the server reads it into a closed, bounded tree, and the browser builds the
  figure from that tree alone, in a fixed, clipped box that is not drawn itself, below Workbench's menus
  and never over the agents column. Text is one plain line with the session's
  secrets masked, also where a refusal quotes it. The avatar never shows or answers a permission request,
  and says nothing while one is open. What is typed into a session is your words or the choice you clicked
  in a fixed frame, nothing else the session wrote. Every size, count, rate and wait has a limit.
- **Status bar, palette and Settings.** A status-bar widget counts the avatars shown, shows whether they
  speak or are hidden, and lists the project's sessions to give an avatar or turn one off. The palette has
  seven *Avatar* commands. **Settings › Avatar** sets which sessions may have one (`[avatar]` in
  `config.toml`: Off, When I give them one, Whenever they want, the default; a repository cannot set it)
  and, per browser, showing, voice, online voices, Test voice, speed, volume, walking and motion. The
  phone's More tab has Show avatars and Voice.
- **For CLIs without Workbench's tools.** Kimi Code, Gemini CLI, Aider, custom CLIs and Codex in a dev
  container drive their avatar with a command: `workbench avatar guide | status | design | say | ask | act`
  on the host (it sends the session's token only to this computer), and `curl` against
  `/api/hooks/avatar/<session>/…` in a dev container, with the token read from standard input, never on a
  command line. Such a CLI may ask you before it runs the command unless you allow it; the composer says
  so.
- **An optional skill.** `workbench-avatar` teaches an agent to draw, rig and speak through its avatar; it
  has a session take one itself only when you ask it to use one. As with Workbench's other skills, it is
  installed only when you press **Install**.

What changed for existing behaviour:

- **A prompt is no longer submitted into a dialog that opened while it was pasted.** When Workbench pastes
  text into a chosen session (an avatar's answer, a prompt sent to that session) and a dialog opens in the
  pause before the Enter, the Enter is held back and the request answers "a dialog opened while your text
  was being pasted: it is in the session's input box, not sent". For CLIs other than Claude Code this is
  read off the screen, where the echo of the texts Workbench typed there (a clicked choice "Yes, proceed")
  does not count as a dialog, and a dialog beside it does.
- **Sessions starting sessions.** `workbench_start_session` takes `avatar: true`: the new session starts
  with a live avatar, told of it with its prompt as with the composer's option. It is refused, and nothing
  is started, under `[avatar] sessions = "asked"`, while avatars are switched off, and when you turned the
  calling session's own avatar off.
- **MCP.** The server's instructions say "this agent session" instead of "this Claude Code session". Tools
  that only do something for you to see or hear are listed as neither read-only nor destructive. A
  successful `avatar_say` or `avatar_act` call stays out of the Activity view (failures are recorded), and
  the argument summary and the error of every agent tool call there are masked with the session's secrets
  before they are cut.
- **Toasts.** A session whose avatar is on screen in a window no longer gets a "finished" toast there: the
  avatar shows it.

Not verified: that sound leaves a loudspeaker (the browser's own speech engine was driven once in headless
Chrome and reported the line spoken; nobody heard it); dictation with a real microphone (a stand-in
recognizer was used); a real Claude Code reading the `permissions.allow` entries of its settings file and
using the tools (the tests' stand-in for Claude Code was used); real Codex, Kimi Code, Gemini CLI and Aider
sessions; a session in a dev container; Windows at run time (the server is type-checked for Windows only);
Safari and iOS (speech needs a tap there, by design, but it was not tried). Deferred on purpose: voices made
by a command on the server, hands-free listening, a pop-out window for the avatar, spoken attention
announcements, a session changing where or whether avatars are shown, and a rendered preview of its own
drawing for the session.

## 0.22.0 - 2026-11-15

DeepSeek support grows up: Codex can use it, its 1M-token window is used, its cache savings and cost
are visible, and an account's balance can be checked without opening the DeepSeek console.

- **DeepSeek in Codex.** A hosted API can now name DeepSeek for Codex as well as for Claude Code and
  Aider: Workbench points Codex at DeepSeek's Responses API (`https://api.deepseek.com`), puts the key
  in a provider of its own, and turns on DeepSeek's standalone web search (Alpha Search). The Codex
  login and its ChatGPT account are not used. The settings page knows the service's address for both
  CLIs.
- **The 1M window.** DeepSeek's V4 models take 1M tokens, but Workbench's example capped them at
  131072 and Claude Code knew only its 200K default. The service's own model names fill the window when
  picked (`deepseek-flash[1m]` for Claude Code, `deepseek-flash`/`deepseek-v4-pro` for Codex), and a
  configured context also sets Claude Code's auto-compact window at 75% of it, as DeepSeek's guide does.
- **A cheaper helper and cheaper sub-agents.** A hosted Claude Code account can name a `helper_model`
  (its Haiku default) and a `subagent_model`, so the main agent can run on Pro while the helper and
  sub-agents run on Flash. Without them every role asks the main model, as before.
- **What the cache saved.** A session's token usage (Claude Code's transcript, Codex's rollout) is
  added up: its tab and card show how much of the input came from the service's prompt cache. For
  DeepSeek the cost is estimated at
  DeepSeek's own prices (peak and off-peak, cache hit and miss) instead of Claude Code's Anthropic
  prices, so a cheap session is not shown with an expensive estimate.
- **Text-only models say so.** DeepSeek's Pro models take no images: attaching one now answers with a
  warning that the service will replace it with "Unsupported Image". The new-session effort choices
  are the levels the model actually has (`low`, `high`, `max`), not five names that collapse to three.
- **A balance, if you ask.** A DeepSeek account can ask Workbench to read its balance
  (`GET /user/balance`, read-only, with the account's key) while Settings → Agents → Accounts is open,
  once a minute; an empty account stops new sessions until it is topped up. It is off by default:
  Workbench otherwise asks no vendor anything.

## 0.21.0 - 2026-11-15

- **Ultracode option.** The new-session composer has an **Ultracode** box for Claude Code: Claude plans a workflow of
  subagents for each substantive task instead of waiting to be asked, at the effort level you chose. Workbench sets
  `"ultracode": true` in the session's own settings (checked live with Claude Code 2.1.325), so the Effort choice stays
  yours, a restart or a move to another Claude account keeps it, and the tab and card show an **Ultracode** badge. It
  uses far more tokens, so sessions never start each other in ultracode; Claude Code leaves it off when workflows are
  off for the account or the model cannot take `xhigh` effort.

## 0.20.0 - 2026-11-15

Dashboards: what Claude can build as an interactive page now has a place inside Workbench, and the one it publishes to
claude.ai is named for what it is.

- **Charts in reports.** Chart.js 4 ships with every scope's shared report files (`../_shared/chart.umd.js`), with
  `report-charts.js` giving it the report's dark look and a palette, so an agent can draw a real chart without a CDN
  (which the sandbox blocks). Existing scopes get the new files at start without their edited `report.css` or
  `report.js` being replaced.
- **Live data in reports.** In a card of a project, `workbench.query(kind, params)` reads the project's agent sessions,
  git branches and changes, recent CI runs (GitHub or GitLab) and data-viewer series. The first request asks you, once
  per card (**Allow** / **Not now**, and a pulse button in the toolbar to turn it off); it is read-only, limited to the
  card's own project, and never available in Home, the Sandbox or a card from a repository. Answers are rebuilt field by
  field, so no folder names, terminal text or message bodies reach a report. A report you allow can still send what it
  reads to other sites, and the question says so. The Sandbox's guide has a **Dashboard** tab that shows sample data
  and, in a project's card, the real thing.
- **Pages published to claude.ai.** When a Claude Code session uses its `Artifact` tool, the permission request now says
  "Permission to publish out/ci.html to claude.ai" and that the file leaves the computer, and the session's tab, card
  and menu keep the link to the page afterwards (only claude.ai addresses are ever offered). Not seen against the real
  service: publishing there is a write Workbench's tests must not do, so the tool's answer is read in any shape.
- **For agents.** The Workspace tools' authoring guide and the `workbench-report-card` skill describe the charts and
  `workbench.query`; the `workbench` skill says when a card is the right place and when the Artifact tool is.

## 0.19.0 - 2026-11-14

Checked against Claude Code 2.1.325 (about 40 releases newer than what 0.18 was verified with). Nothing was broken;
these close the gaps and use what is new.

- **A file dialog Workbench could not see.** Claude Code's Write dialog asks `Do you want to create notes.txt?` and has
  neither of the lines Workbench looked for, so a request answered in the terminal stayed pending until its tool call
  ended. The screen check now also knows any `Do you want to …?` followed by a numbered `1. Yes`, and the footer
  `Esc to cancel · Tab to amend`.
- **A session at a usage limit that waits.** Claude Code now waits in the session at a claude.ai limit and goes on by
  itself. Workbench ends the error state when it does, and says "press Enter in the terminal to continue" when it waits
  for you after the computer slept past the reset. A sign-in Claude Code could not save is told too.
- **What a resumed conversation will cost.** A session resumed or forked after a pause says on its card and tab when its
  prompt cache ran out: "Cache expired · $1.14", with the tokens its next message sends again in the tooltip.
- **Sign-in expiry.** Settings › Agents › Accounts shows "Login ends in 7 days (Nov 20)" in the last two weeks of a Claude
  login, and "Login ended" after it.
- **Restricted sessions.** A *Restricted* box in the new-session composer starts `claude --restricted`: no commands, no web
  fetch, files inside the project, your own Claude settings ignored. For a repository you do not trust. It stays
  restricted when it restarts or moves to another account, it starts only restricted Claude Code sessions itself, and
  Workbench's `run_start` tool refuses it, as it does a sandboxed Codex session.
- **Sessions can message the sessions they start.** `workbench_start_session` tells a starter that gave its child a `name`
  to use Claude Code's own `SendMessage` to that name (`notify_when_idle` says when it is done), instead of reading its
  terminal. Checked with 2.1.325: the message arrives as a prompt in the child.
- **A stray `←` no longer moves a session out of Workbench.** Claude Code's `←` at an empty prompt opens its agent view and
  hands the session to a background service of its own. Hosted sessions now start with agent view off.
- **The Skills window follows Claude Code 2.1.2xx–3xx.** A skill whose file or folder name has an invisible or
  direction-changing character is an error (Claude Code skips it), a name over 256 characters a warning, nested skills say
  they load only in a session started in their folder when its sandbox is on, and a plugin that carries a **mod** (code
  that runs inside Claude Code) is named in a note, since it has no skill to list.

## 0.18.1 - 2026-11-04

- **`jira_create_issue` no longer makes an Epic by mistake.** When the issue type asked for was not in the project
  (or none was given and the project has no Task), the first non-subtask type was used, which is often Epic. It now
  refuses, and the error lists the types the project has.

## 0.18.0 - 2026-10-23

- **Agent sessions can start agent sessions.** A new tool, `workbench_start_session`, lets an agent hand a separate
  piece of work to another session of any provider (Claude Code, Codex, Kimi, Gemini, Aider or a custom CLI): a
  prompt, an optional name, folder, model and effort. The new session opens as a tab beside the starter, works on its
  own and asks you for permission like any other; the starter follows it with `workbench_sessions` and
  `workbench_terminal_output`.
- **Held to what its starter may do.** The new session is in the starter's project and in a folder inside it, and
  the same CLI starts in the permission mode the starter runs in. A session that asks before it acts cannot start one
  that does not (a "never ask" mode, Codex outside its sandbox). Sessions started by sessions go two levels deep, and
  at most 8 run in a project at once; past that the agent is told to ask you.

## 0.17.0 - 2026-10-19

- **A Skills window: what your agents can use here, and what it costs.** Claude Code, Codex, Gemini CLI and Kimi
  Code read the same `SKILL.md` format from different folders, so no one of them can say which skills your agents
  have in a project. The **Skills** tool window (right stripe, or Ctrl+K → *Show Agent Skills*) lists them from
  every place each CLI looks: personal and project skills, nested ones, plugins (through Claude Code's own registry,
  not its cache of old versions), claude.ai's synced skills, slash command files, an organization's, and Claude
  Code's **built-in** skills, which are in no folder and are known from the skill listing it recorded in its newest
  session here. A summary counts what is available, what is off or hidden by another skill of the same name, what
  needs a look and what can act without asking (pre-approved tools, hooks, shell commands), and shows per account what
  the descriptions cost **every session** in tokens: what that session really received for Claude Code, else an
  estimate. A row opens the skill's panel: what it is for, where it is, who reads it, how to start it, what is wrong
  with it (front matter that does not parse, no description, a name that differs from its folder), its files and its
  text. Nothing in a skill is run, and the browser never names a path.
- **Workbench's own skills, optional.** Five skills in the program's `skills/` teach an agent to work well inside
  Workbench: `workbench` (which tool for which intent, CI and review triage, etiquette), `workbench-report-card`
  (Workspace cards: sandbox-safe reports, the shared style, step types), `workbench-data-capture`,
  `workbench-debug-session` and `workbench-project-setup` (the `.workbench.toml` schema, what Workbench drops
  silently). They are never installed unless you press **Install** for a Claude Code or Codex account; Workbench
  then knows exactly which files it wrote, offers **Update**, never replaces a file you edited without asking and
  never touches a folder it did not make. Agents cannot install skills. A test keeps them honest: every tool they
  name exists, every link and TOML example is valid.
- **Fixes in the docs.** `docs/agents.md` and the architecture notes said no tool starts a debugger and every tool is
  confined to its project; the debugger's tools do both. The secrets example read a `dotenv` file by a relative path, which is resolved from
  Workbench's own working directory, not the project's.

## 0.16.1 - 2026-10-16

- **A second account may use the same folder.** The Accounts form no longer refuses the folder of another
  account, or the default `~/.claude`: it says the two share one login and one history, with settings of
  their own, and that the second is no use as a fallback for the first. Use it for a model or effort preset
  of the same subscription, or point a new account's **Shared memory folder** at any folder.

## 0.16.0 - 2026-10-15

- **Live measurements next to the agent: the data viewer.** An agent that captures a logic analyzer trace,
  a differential probe or scope waveform or a power profile can now put the samples in front of you while it
  works. The **Data** tool window lists the series; plots open beside the session and fill live. A plot is
  **lanes** under one time axis, like a scope or a logic analyzer: current on a logarithmic lane in µA and
  mA, rails in volts, each digital line on a thin lane of its own (buses as boxes with their value), decoded
  events as a row of labels, markers as lines through everything. Wheel zooms around the pointer, drag
  pans, **Shift + drag** measures a range (min, max, mean, RMS, peak-to-peak and the area under the trace:
  charge or energy; frequency, duty and edges for digital lines), double-click or End follows the newest
  samples again, and plots can be **linked** so a power plot and a logic plot pan and zoom together.
  Samples are drawn from what the server returns for the window in view, so a million-sample capture costs
  a few thousand points a redraw and a spike always survives; the plot is the same in a second browser.
- **Agents fill it and make the plots while capturing.** `data_append` puts samples or events into series
  (analog, digital, events), `data_plot` creates and changes plots (a plot may be made before its series have
  a sample, and series are added or removed while the capture runs), `data_series` lists, defines, clears and
  deletes, and `data_query` reads back statistics and thinned samples of a window: the sleep current, the
  width of a pulse, whether a rail dipped. A script the agent runs can post samples with the session's own
  token (`POST $WORKBENCH_URL/api/hooks/data/$WORKBENCH_TERMINAL_ID/append`).
- **`data_follow` reads a capture file while the tool is still writing it.** CSV (headers such as
  `Current [uA]` and `Time (ms)` give units and the time base; text cells become events), JSON lines and VCD
  (sigrok, PulseView, simulators) are decoded as they grow, and a file that is replaced starts the capture
  over. Only files inside the project are read, by the rules of every agent file read. **Follow file…** in
  the Data window does the same for you.
- Samples are kept in memory (the newest million per series by default, 30 million across projects); the
  series' names and units and the plots survive a restart. See `docs/data-viewer.md`.

## 0.15.0 - 2026-10-14

- **Accounts can share one memory folder of your choosing.** Memory was shared only with the default
  account's folder. Give Claude Code, Codex or Gemini CLI accounts the same **Shared memory folder**
  (Settings → Agents → Accounts, or `memory_dir` in `[agents.providers.<name>]`) and they read and write one
  memory; the default account can take part too. The folder is laid out like an account's folder
  (`projects/<project>/memory`, `memories`, `.gemini/GEMINI.md`). Memory an account already has is kept, and
  `memory_dir` wins over `own_memory`.
- **Accounts without a login may share a folder.** An account on a hosted API or a model of your own can use
  the folder of another account, the default `~/.claude` included, and so shares its history and settings.
  Two accounts that sign in with a login still need folders of their own. Continuing a session on another
  account of the same folder no longer copies its transcript onto itself.

## 0.14.0 - 2026-10-14

- **Speak a session's prompt.** The new-session composer (the agents column's first tab, the **New agent
  session** dialog, and **New** on the phone) has a microphone button beside **Start session**. Click it,
  say the task, and the words appear in the prompt at the caret as you speak; click it again to stop. The
  session starts only when you press **Start session**, so a misheard word is fixed first, and typing in the
  prompt stops the listening. Recognition is your browser's own (Chrome, Edge, Safari): the audio goes to
  its speech service, never to Workbench. It needs `localhost` or https (over plain http the button says
  so), and Firefox, which has no speech recognition, shows no button. On a touch screen one tap takes one
  sentence.

## 0.13.0 - 2026-10-09

- **One project, several git repositories.** A project folder can hold more than one repository: services
  cloned side by side, a monorepo with a submodule, or a plain folder of repositories. Workbench finds them
  (up to three folders below the project root; or list them with `[[repository]]` in `.workbench.toml`, each
  with its own GitLab or GitHub project and token), and a **repository switcher** in the top bar (and the
  status bar and the phone's Git and CI tabs) picks the one that git and CI show: the Commit window, the Git
  Log, branches, stash and shelf, fetch, pull and push, merge or pull requests, pipelines, jobs, issues and
  Actions runs. Repositories can be on different hosts, and their pipelines are watched separately. Each
  repository keeps its own changelists and shelves. With a single repository nothing changes.
- **A folder of repositories is a project too.** A folder with a `.workbench.toml` and no `.git` of its own
  is listed under a projects root, and its first repository is the default.
- **The file tree, Go to File, Find in Files and the watcher follow nested repositories**, also when the root's
  `.gitignore` lists them: files changed in any repository are coloured, searchable and refreshed live, and a
  commit made in a terminal inside a nested repository updates the views.
- **Agents can pick the repository.** `workbench_projects` lists a project's repositories, and the git,
  GitLab and GitHub tools take an optional `repo`.
- **Git credentials follow the repository.** Fetch, pull and push in a repository on another GitLab host use
  that repository's token.

## 0.12.3 - 2026-10-08

- **3D comparisons with many models show every model whole.** A test with more models than fit in one
  row squeezed its rows until each pane clipped its model's stage, which was still framed at full height,
  so only the tops of the models showed. Rows now keep the height a pane needs (a little less on short
  screens) and the panes scroll. While they scroll, the wheel scrolls them and Ctrl/⌘ + wheel (or a pinch)
  zooms the model under the pointer; on a touch screen a vertical swipe scrolls without tilting the models,
  and a sideways one turns them. A note over the viewer says so. A single row still fits the viewer,
  however short, and a few pixels out of view do not change what the wheel does.
- **A 3D test that lists the same model file twice** no longer leaves panes of the previous test behind
  when you switch tests.
- **Closing a 3D pane frees its WebGL context at once.** Switching between tests with many models used to
  leave old contexts alive until the browser dropped the oldest ("Too many active WebGL contexts").

## 0.12.2 - 2026-10-08

- **The "N working" pill opens a list of sessions.** Clicking it in the top bar used to go to the Agents
  tab. It now drops down the working sessions; picking one opens it, and "All agent sessions" at the
  bottom goes to the overview.
- **Workspace card tabs scroll with the mouse wheel.** A long strip of steps moves sideways when the wheel
  turns over it.

## 0.12.1 - 2026-10-07

- **Phone: switching project leaves the old session.** Changing the project in the phone header while a
  session of the previous project was open in the Agents tab kept showing that session. The tab now goes
  back to the session list of the new project.

## 0.12.0 - 2026-10-05

- **Agent accounts share the default memory.** A second Claude Code, Codex or Gemini CLI account no longer
  starts with an empty memory: it reads and writes the default account's (Claude Code per project). Memory an
  account already has is kept. Set `own_memory = true` on an account to keep a memory of its own.

## 0.11.0 - 2026-10-05

- **Plots: several watched values on one chart.** The Live tab's new **Plots** button opens a plot in its
  own panel: pick **New Plot from Watched Values**, or right-click a value and choose **Plot in New Plot** or
  **Add to …**, and the values are drawn together as lines, each in its own colour, with a crosshair and a
  tooltip listing every series, a legend with the latest values, Pause, and a table of the same readings
  (copyable as CSV). Type another variable above the chart to add it; it is watched first if it is not yet.
- **Plots scroll.** Turn the wheel or drag the chart to move back and forth through everything Workbench has
  kept (up to 12,000 readings per value), drag the scroll bar under it, hold Ctrl and turn the wheel to zoom
  (spans from 5 seconds to 30 minutes), and press Follow to go back to the newest readings. Page Up/Down,
  Home, End and `+`/`-` do the same from the keyboard, and the arrow keys carry the crosshair past the edge.
- **Several plots per project, kept by Workbench.** Every plot is a saved configuration (name, series, span,
  scale), so you can keep one per question and dock them side by side, and every browser and device shows the
  same ones: a change in one appears in the others at once. Plots an earlier build kept in the browser move
  over the first time. They are also in the palette (New Plot, Open Plot …).
- **Values of different sizes** share one axis in two ways: **Same axis** (the values' own unit) or **Each as %
  of range** (every series as a percentage of its own range in view), with the real values in the tooltip.
- **Reload keeps the history.** Workbench now keeps the last readings of every watched value, so a reloaded
  page starts with the last minutes, and the Live tab's sparklines show at least the last 30 seconds however
  fast the readings come (they showed the last 120 readings). 64-bit values show every digit in the tooltip,
  table and CSV.
- **Scripts and agents can read them.** `GET …/debug/plots` and `…/plots/<id>/data` (statistics and thinned
  readings of each series), `…/sessions/<id>/live/history` (the raw readings), and a new read-only
  `debug_plots` tool for agents. Only you make and change plots.
- **Live Watch for the rest of the debug servers.** J-Link, pyOCD, st-util, QEMU and an OpenOCD without a Tcl
  port have no way to read memory while the program runs, so the Live tab is now there for them too, with
  values blank until you allow **reading by pausing the program**: Workbench stops the program for a few
  milliseconds, reads and resumes it (every 100 ms or slower). It disturbs the program, so it is off for every
  session until you allow it, and the Live tab shows how long each read stops the program (on a
  NUCLEO-C092RC through OpenOCD about 50 ms, 17% of the time at 250 ms). The stop is not shown as a pause;
  a breakpoint that hits meanwhile, your own Pause or a step of yours is a real motion and is left alone (a read
  never cuts a step short), and a program that is stopped anyway is read for free, so the values follow your steps.
- Smaller fixes found on the way: copying a plot with a very long name no longer freezes the page, the y-axis
  labels no longer clip at seven figures, a project file with one malformed plot no longer loses the project's
  breakpoints and watches with it, and typing a variable while the saved watches are still coming back at the
  start of a session no longer lists it twice.
- Checked on a NUCLEO-C092RC in both themes: through OpenOCD (`ticks` read 1,004 a second against the chip's
  12 MHz clock, the LED's 500 ms toggle drew as a square wave, 50 ms polling over 5 minutes at about 60 frames
  a second) and, through an OpenOCD without a Tcl port, reading by pausing (the session stayed "running",
  nothing was read until allowed, a user pause stayed a real stop, eight steps in a row each ended at their own
  stop, and the cost shown matched the slowdown of the chip's own tick counter). `st-util` 1.8.0 does not know the STM32C092 and wedged the probe: use OpenOCD
  for that chip. Scrolling, keyboard stepping, two browsers editing one plot, a session ending and a new one
  starting, an unwatched series and deleting a plot were checked on a simulated board.

## 0.10.0 - 2026-10-04

- **Live Watch.** The Debug window's new **Live** tab shows variables of the program *while it
  runs*, without stopping it: type a variable (`ticks`, `cfg.limit`, `buf[3]`) or a fixed address
  (`*(uint32_t*)0x50000014`) and press Enter, and its value is read every 250 ms (50 ms to 5 s) and
  drawn as a line of its recent readings. A counter climbs, an LED flag draws a square wave. The
  values come from OpenOCD's Tcl port, which reads memory over SWD while the core runs, so it needs
  OpenOCD (its preset has it; another OpenOCD takes `live_port` in `[debug.servers.<id>]`). Integers,
  floats, booleans, pointers and enums are shown by type, as decimal, hex or binary.
- Only things at a fixed address can be watched: a pointer's target moves, so `*ptr` and `p->x`
  are refused with the reason. A peripheral register that clears when read is refused when the
  project's SVD file says so; the others carry a warning. The expressions are remembered per project
  and come back with the next session, and agents see the latest values in `debug_state`.
- Checked on a NUCLEO-C092RC: a running blinky's `ticks`, `blink_count` and the LED's output register
  read live, in both themes.

## 0.9.2 - 2026-10-04

- **Stop leaves the firmware running.** OpenOCD leaves the core halted when gdb detaches, so Stop
  froze the board (the LED stuck). The OpenOCD preset now makes the target resume on Stop; the new
  `on_stop = "halt"` in `[debug.remote]` keeps the old behaviour. st-util already let the target run.
- **A debug server that goes wrong says so.** The Console explains `unknown chip id` (st-util on a
  chip newer than its tables) when the server prints it, and a probe that stops answering (three USB
  timeouts in half a minute) ends the session at once with the reason, instead of hanging until the
  debugger's own timeout.
- **Windows: debug servers no longer get a port Windows keeps for itself.** Session ports are drawn
  at random, and a draw inside a reserved range (`WinError 10013`) made the server fail to start.
- Finished "Before debugging" tabs of a configuration that was renamed or removed close on the next
  debug run once they are a day old.

## 0.9.1 - 2026-10-04

- **Debug build tabs no longer pile up.** The terminal tab of a configuration's `pre_launch`
  step ("Before debugging: …", and Cargo's "Build …") closed only when you closed it, so every
  configuration left up to three finished tabs in the Agents column. A step that succeeds now
  closes its own tab (its output stays in the terminal history, and the terminal button in the
  Debug window's header opens it), a step that fails keeps its tab so the error can be read, and
  the next run of the same step replaces the old failed tab. A step that left background
  processes running keeps its tab, since closing it would end them.

## 0.9.0 - 2026-10-04

- **Sign an account in from Settings.** **Settings → Agents → Accounts** shows whether each account's
  CLI is signed in ("Signed in · Claude subscription · max", or "Signed out") and has a **Sign in**
  button that runs the CLI's own login in a terminal tab, in that account's folder. Adding a second
  subscription no longer means starting a session to find its login. The status is what the CLI itself
  says (`claude auth status`, `codex login status`); Gemini CLI and Kimi Code cannot say, and are started
  as they are, asking by themselves. Workbench reads no login, token or email, and the sign-in is only
  for a signed-in device, not for an agent.

## 0.8.0 - 2026-10-03

- **Hosted APIs.** An account can use a hosted model API with an API key: DeepSeek, OpenRouter, Z.ai,
  Moonshot, Fireworks, the Anthropic and OpenAI APIs, or any compatible service, for Claude Code,
  Codex and Aider (**Settings → Agents → Accounts → Add…**). The key is named by a `[secrets]` entry,
  read by the server when a session starts, passed in the CLI's environment only and masked in its
  output; the browser sees only the secret's name. Combine them with fallback lists, so a cheap API
  account can take over when a subscription is at its limit.

- **Conversation transfer.** A session can continue on another account with its conversation:
  **Continue on another account…** in its menu, the toast of a session that hit its limit, or
  `failover = "session"`. Between accounts of the same CLI (Claude Code to Claude Code, Codex to Codex,
  also onto a local model) the conversation itself is resumed, by copying the session's file into the
  other account's folder; to another CLI, what was said is written out as text for the new session to
  read. `[agents] transfer = "notes"` carries only a short note. Checked against the real Claude Code
  and Codex: the history reaches the model of the resumed session.
- Local models: an optional **context window** (`local.context`) is passed to Claude Code
  (`CLAUDE_CODE_MAX_CONTEXT_TOKENS`) and Codex (`model_context_window`), and Claude Code's sub-agents
  use the local model too.

- **Local models.** Claude Code, Codex and Aider can run on a model server of your own (Ollama,
  LM Studio, llama.cpp, vLLM…): **Settings → Agents → Accounts → Add local model**, with **Find
  models** to list what the server serves. Nothing of the vendor's login or API key is passed on, and
  a session never falls back to the vendor if the server's setup is incomplete.
- **Usage limits and automatic failover.** Accounts show how full their 5-hour and weekly windows are
  (from Claude Code's status line and Codex's session log) and when a limit ends. Give an account a
  `fallback` list and a new session skips an account that is at its limit: a second subscription, then
  perhaps a local model. `failover = "session"` also continues a running session that hits its limit on
  the next account, as a new session that carries the conversation; otherwise its toast offers
  **Continue on …**.

- **Several accounts of one agent CLI.** Run Claude Code, Codex, Kimi Code, Gemini CLI or
  Aider under more than one login (Aider: more than one `.env` of API keys), such as a work and a personal subscription. **Settings → Agents → Accounts** adds
  an account (a name, a label and a folder of its own); it appears in the new session picker
  beside the CLI, and the composer has an **Account** button for it. Each account's history, Remote Control links and live sessions follow its
  own folder, and Workbench never reads the logins. Accounts were already possible by hand
  in `config.toml`; Remote Control links and the live-session list of an extra Claude account
  were only looked up in the default `~/.claude` before.

## 0.7.1 - 2026-10-03

- **Agents drive the debugger.** New MCP tools: `debug_start` (a configuration by name),
  `debug_attach` (a process), `debug_restart`, and `debug_evaluate` (an expression, or a
  debugger command echoed into your console as `(agent)`); `debug_breakpoints` now accepts
  conditions and log messages. There is no setting to turn on. They are writes (the agent's
  permission prompt applies, Activity marks them) and, unlike the other MCP tools, are not
  confined to the agent's project: `projectId` may name any.
- They run on your computer, outside the sandbox a CLI such as Codex keeps its own commands
  in (a gdb expression can call a shell command, and a start runs your build step and debug
  server). Workbench does not refuse such a session; the CLI's own sandbox and approval
  prompts are what hold it. A pre-launch run that deploys or reaches another host is still
  refused.

## 0.7.0 - 2026-10-02

- **Embedded debugging:** debug firmware on a microcontroller, or in QEMU, from the Debug
  tool window. A `[[debug]]` entry with a `[debug.remote]` table starts a debug server
  (presets for **OpenOCD**, **J-Link GDB Server**, **pyOCD**, **st-util** and **QEMU**; your own
  under `[debug.servers.<id>]` in `config.toml`), waits until its gdb port listens, connects
  gdb to it, resets the target, downloads the program (`load`), resets it again and runs to
  `main` or to your breakpoint. Stop disconnects gdb and ends the server; if the server dies
  or never comes up, the session says why, with the server's own error line.
- **The right GDB for the chip:** Workbench reads the program's ELF header and picks
  `arm-none-eabi-gdb`, a RISC-V or Espressif GDB, or `gdb-multiarch`; new presets
  `gdb-multiarch`, `arm-none-eabi-gdb`, `riscv-gdb` and `xtensa-gdb` appear under Debug
  adapters. A GDB built without Python (which cannot speak DAP) is now reported as unusable
  instead of failing at start.
- **In the window:** configurations of a remote target have a chip icon and a tooltip that
  lists the server's command line and the commands they will run; the Start view lists the
  debug servers it found and how to install the missing ones; the header shows
  `server · target`; the Console shows the server's output in italics and takes `monitor`
  commands; the CPU registers are among the variables. A target can be left halted at the
  reset vector (`stop_at = "reset"`).
- **Agents** can read the server, its output and, with `registers`, the CPU registers of a
  halted core through `debug_state`, and steer a session you started: `debug_control`
  continues, pauses, steps, runs to a line and stops it and answers with the new state;
  `debug_breakpoints` lists and edits plain line and function breakpoints. Both are writes
  (the agent's permission prompt applies, Activity marks them). Agents cannot start,
  attach or restart a session, evaluate expressions, or set conditions and log points:
  a gdb expression can run a shell command, so you do those from the window.
- **Registers by name:** name the chip's CMSIS-SVD file (`svd` under `[debug.remote]`) and
  the Debug window gets a **Peripherals** tab: peripherals, registers and bit fields with
  the vendor's value names, read from the halted target with each register's own access
  size, edited with a double-click (a register, or one field). Registers that change when
  read are left alone until you ask. Checked against ST's STM32F407 SVD and QEMU's SysTick.
- **Target output:** `[[debug.remote.channels]]` shows text the program streams out of band
  (a UART or RTT telnet port, or a decoded SWO/ITM stream) in the Console. QEMU's UART was
  run for real; a real SWO or RTT stream was not.
- **Extended-remote stubs:** `extended = true` connects with `target extended-remote`
  (`gdbserver --multi`, run for real; Black Magic Probe with `attach`, not verified).
- **Dev containers:** a project that runs in its dev container builds there while gdb and
  the debug server stay on this computer; the workspace's paths are mapped for you, and
  `source_map` maps others. Run against a throwaway Debian container.
- `{port2}` to `{port9}` are free ports for a debug server's other listeners.
- A debug server never outlives Workbench: if Workbench crashes or is killed, the system ends
  the server too, so it cannot keep holding the probe.
- Each session gets ports of its own below the system's ephemeral range, so two sessions, or
  an OpenOCD left running, never fight over 3333. The server's *command* comes only from
  `config.toml` or a preset; a repository adds arguments and gdb commands that run when you
  start the configuration.
- See [embedded debugging](docs/embedded-debugging.md). Run end to end with real tools: a
  Cortex-M3 firmware in QEMU through gdb-multiarch, and a host program through `gdbserver`.
  OpenOCD, J-Link, pyOCD, st-util and Black Magic Probe were not run against a chip: their
  commands follow their documentation.

## 0.6.0 - 2026-10-02

- **Updates:** Workbench updates itself. A Workbench installed from a release looks for a
  newer one once a day and shows **Update X.Y.Z** in the status bar; **Settings › Updates**
  has the release notes, **Check now** and **Update and restart**, which downloads the
  release, compares its SHA-256 with the release's checksum, replaces the program and
  restarts into it. The server restarts in place (the same process, a fraction of a second)
  and the page reloads by itself; other open tabs and devices are offered a reload. Before
  it starts it says what the restart stops: agent sessions resume afterwards, shells start
  again under their last screen, runs do not. The replaced version stays as
  `workbench.prev`. The phone's More tab offers the same.
- **`workbench update`** does it from a terminal: `--check` only looks, `--restart` also
  restarts the running Workbench.
- **Restart now** (Settings › Updates) restarts a Workbench whose program was replaced by
  hand, with `install.sh` or `workbench update`.
- Nothing is installed without your click, and agents cannot update or restart Workbench.
  Looking is one request a day to GitHub without a token; `[update] check = false` turns
  it off. A build from source has no release to look for until `[update] repo =
  "owner/name"` names a repository. On Windows, and where Workbench cannot write to its
  own folder, it tells you about the new version and leaves installing to the archive's
  installer.

## 0.5.3 - 2026-10-02

- **Agents column:** a session opened from an attention toast, the history, a Workspace card
  or a notification is shown in the column of its own project: Workbench switches to that
  project instead of adding the tab to the project on screen, so the columns no longer mix
  sessions of different projects. Tabs an earlier version added that way are moved on load.

## 0.5.2 - 2026-10-01

- **Terminal tool window:** the **Terminal** button is back at the foot of the left stripe,
  first of the bottom group. It opens shells under the editor, as tabs, beside whatever the
  agents column shows; **+** starts one (in the dev container or on the host when one runs).
  Those shells are the tool window's own: the agents column leaves them out, and opening one
  of them from anywhere shows it there. Shells started from the column stay in the column.

## 0.5.1 - 2026-10-01

- **Docs:** the tour card's screenshots (CI tests, a Workspace card, Services, Database), the
  help pages, the welcome card and the README describe the agents column: sessions start
  from the first tab of the column, the project switcher sits over it, Alt+F12 collapses the
  workspace window.

## 0.5.0 - 2026-09-30

- **Layout:** agents and terminals are a column on the left of the window, no longer tabs
  of the centre. Its first tab has the prompt for a new session, **New shell** and the
  project's sessions; every agent session, shell and run output is a tab next to it, and
  **+** starts a session or a shell. Closing a tab stops what runs in it (a session stays in
  the history). 
- **Two windows:** the agents window (with the project switcher on top) and, beside it, the workspace window with everything else; the branch, run configurations, environments, CI and Commands moved into the workspace window's top bar. The button at the top of the agents window (Alt+F12) collapses the workspace window, so the agents have the whole width; in an app window the browser window shrinks to the agents window at the same position, and grows back when you expand it. A session's header wraps onto two rows in the narrow agents window instead of cutting its title. The session tabs follow the Agents button left to right and take as many rows as they need, under that button too, instead of scrolling out of sight in one row; and the "2 working" / "1 needs you" pill sits at the right end of the agents window's top bar.
- **Tool windows:** the icon stripe sits right of that column. **Workspace** is its first
  button, above Files, and a project opens on its Workspace cards. **Settings** moved to the
  foot of the stripe. The Agents and Terminal tool windows are gone: the column replaces
  them. Layouts saved by earlier versions are carried over (their agent and terminal tabs
  leave the centre).

## 0.4.1 - 2026-09-30

- **Add project** has a **Browse…** button: a folder picker on the Workbench computer (subfolders only, hidden ones on request) that fills the path field.

## 0.4.0 - 2026-09-30

- **Sandbox:** the Workspace has a Sandbox scope (`wb-sandbox`) for throwaway documents,
  reports and agent experiments. It has its own tab in the Workspace tool window, the phone
  list, the Workspace home and the New card dialog, a guide card with a playground report that
  shows what a sandboxed report can reach, and **Reset**, which moves every card to the trash
  and brings the guide back. Its cards stay out of the "All" view, and agents may write to it.
- **Tabs follow the project:** each project keeps its own tabs and splits; switching projects
  switches them.
- **Add project** offers to create the directory when it does not exist, and switches to the
  new project.
- **Models:** `fable` is offered when starting a Claude Code session and in Settings › Agents.
- **Settings** moved to the top left of the window.

## 0.3.1 - 2026-09-30

- **Agent sessions (security fix):** removing an agent session from history while a restart
  or a restore was waiting to start it could leave a working agent token behind, valid until
  Workbench restarted. For Claude Code, the session's files also came back with that token
  in them: `claude-settings.json` and `mcp.json` in the data directory, or inside the
  container for a session in a dev container. A restart or restore that was waiting now
  finds the session gone and starts nothing, so it issues no token and writes no files. An
  agent session that fails to start no longer keeps the token it was given either. An agent
  token gives what its session has: Workbench's hooks and its MCP tools.
- **Runs:** a run whose terminal is closed or whose process is ended from outside now ends
  "exited" with a "terminated" warning (after its test counts, if any), and its toast says
  it was terminated, not "failed: exited with code 1". That covers Kill, Close and Restart
  in Workbench and, on Linux, a hang-up, terminate, kill or interrupt signal that ends its
  process, whoever sends it (Ctrl+C in its terminal, or the kernel's out-of-memory killer).
  A crash (such as a segmentation fault or an abort), a non-zero exit code and tests that
  failed before the end still fail, and Stop still stops. A run cut short does not count as
  finished for runs that depend on it, a debug session's pre-launch run says it was
  terminated, and agents see `terminated` in `run_list`. On Windows only Workbench's own
  closes are known: a process ended from Task Manager exits with code 1 and still reads as
  failed.
- **Deploys:** a deploy no longer reports "the repository has no commits" (or "unknown
  commit") when git itself failed. It gives git's own message, such as "not a git
  repository", or says that the folder is gone or that git is missing or timed out. "No
  commits" and "unknown commit" now mean that git ran and found no such commit.
- **Line endings:** Workbench follows git's line-ending conversions on Linux too, as it
  already did on Windows. They apply to a file git reads with LF although it has CRLF on
  disk: checked out with CRLF over LF in the repository (`eol=crlf` in `.gitattributes`,
  `core.autocrlf=true`), or saved with CRLF where git converts it (a `text` attribute,
  `text=auto`, `core.autocrlf=input`). Its diff shows the working tree as git reads it, so
  once every change is staged it says "Nothing unstaged in this file." instead of showing
  two identical sides. A conflict in it is shown with LF, like both sides, and the text you
  resolve it with is written back with CRLF (a file with mixed line endings gets the text as
  sent). Local History keeps the "Last commit (HEAD)" of a CRLF checkout with CRLF, so
  comparing it with the first change shows the edited lines, not every line, and a file
  that only went through the checkout no longer gets that entry. Files git does not convert
  keep their bytes: LF files, `-text` files, and CRLF committed as is under `text=auto` or
  `core.autocrlf`. A `text` attribute, or `eol` without `text=auto`, has git convert CRLF
  committed as is too and show every line changed until the file is renormalized;
  Workbench's working-tree side now has LF to match.
- **File names with `\` (Linux):** a `\` in a file or folder name is part of the name, as
  Linux has it. Workbench used to read it as `/`, so `a\b.txt` became the file `b.txt` in
  the folder `a`: the files in a folder named `d\x` opened as `d/x/…`, search and quick
  open results (and Replace in Files from them) led to that other file, and Local History
  and an agent's `workbench_open_file` kept or opened its path. The file tree, opening,
  saving, renaming, search, quick open, the watcher, Local History and detected run folders
  now keep the name. On Windows `\` still separates.
- **Git messages:** error boxes keep the line breaks of multi-line messages, such as git's.
  A fetch, update, push or remote-branch deletion that fails because ssh would have had to
  ask something now says what to do: for an unknown host key, connect once with ssh in a
  terminal, check the fingerprint and accept the key; for `Permission denied (publickey)`,
  load a key that has a passphrase into ssh-agent (on Windows, start the OpenSSH
  Authentication Agent service first), or else add your public key to your account on the
  server. A changed host key is flagged with a warning to check its fingerprint before
  replacing it, and a revoked one with a warning not to trust it again. When git is missing
  or times out, the GitLab and GitHub pollers still watch only the default branch, and now
  log why once per project.
- **Machine overlays:** an overlay that does not parse or cannot be read is still left out
  whole, secrets included, but a secret missing for that reason now comes with the
  overlay's error (for a parse error, its line and column) instead of the advice to add it
  under `[secrets]` in that same overlay. The usual cause is a Windows path in double quotes
  (`"C:\Users\…"`, where `\` starts an escape);
  [Make Workbench yours](docs/customization.md#keep-secrets-where-they-are) shows the
  spellings that work.
- **Server log:** colour escapes only on a terminal. The systemd journal, output redirected
  to a file or a pipe, and the Windows `service.log` get plain text. `NO_COLOR` still turns
  colour off on a terminal.
- **MCP:** the `workbench_notify` description says what it does: a toast in Workbench, plus
  a desktop notification, your notify command and a push to your devices where you set them
  up (Windows has no desktop notifications yet).
- **API:** a terminal's `exit` (also in `terminal.exited`) and a run (`GET …/runs`,
  `run.state`) have `terminated: true` when they were cut short. `POST …/deploy/check` and
  `…/deploy` answer a git failure with `422 git_error`, `504 timeout`, `412 not_configured`
  (no git) or, on Windows, `403 unsafe_repository`, no longer `400 bad_request`. On
  Windows, a delete the Recycle Bin would not take answers `422 not_recyclable`, not 500.
- **Windows (experimental):** still experimental. Its test suite passes on GitHub's
  `windows-latest` (Windows Server 2025); it has not been tried on a Windows 10 or 11
  desktop yet ([status](docs/windows-port.md)).
- **Run configurations (security fix, Windows):** detection no longer offers a run whose
  command quotes a name from a repository file that has a space and ends in `\`. Windows
  PowerShell 5.1, which runs commands wherever PowerShell 7 is not installed, lets that `\`
  escape the closing quote, so the next quoted name is split into arguments of its own:
  `cargo run -p 'x \' --bin 'y --z'` would hand cargo `--z`.
- **On Windows:** a repository git refuses because another user owns the folder
  (`safe.directory`) no longer just loses its branch. The project shows a warning that names
  the folder and the command that trusts it, the status bar reads "Untrusted repository",
  and the git tool windows show git's message with a button that copies the command.
  Deploys report the refusal too, and the pollers log it once per project.
- **On Windows:** programs installed while Workbench runs (Node.js, Python, rustup, an agent
  CLI, a language server) are found without a restart. New terminals, runs and agent
  sessions get the `PATH` a new sign-in gets, followed by the folders of Workbench's own
  `PATH` that it lacks (such as a virtual environment Workbench was started from).
  Workbench's own lookups (language servers, debug adapters, agent CLIs) try it too, and
  what Workbench starts by itself gets its new folders after Workbench's own, so a newly
  installed gopls finds a newly installed Go. A newly installed Git for Windows is still
  found only after a restart. `workbench service install --enable` and
  `workbench service open` start Workbench in your sign-in environment, not in that of the
  shell they run in.
- **On Windows:** a file outside the project at a drive path (a language server's
  definition in a library, a debug stop in `C:\…`) opens instead of being refused. Copy
  Path and drag and drop join paths with `\`, and tab titles, breadcrumbs and stack frames
  name a file by what follows its last `\`.
- **On Windows:** paths other programs write compare as Windows compares them (any case,
  `\` or `/`): Claude Code's project entries in `~/.claude.json`, Gemini's chat folders,
  the session folder that shortens the paths in a permission prompt, and a file an agent's
  hook names in another case, which no longer starts a second Local History. The
  debugger's attach picker knows a process by its image name (`node.exe`, `javaw.exe`) and
  reads `C:\Program Files\…` command lines, and a launch configuration's `.\cmd\api` in a
  Go module debugs as Go.
- **On Windows:** a terminal whose own process has exited while something it started runs
  on (a background job, a program it opened) no longer keeps a thread waiting for those to
  end. One thread hears from Windows, for every terminal, when the last of them has ended,
  and checks every second in case that report does not come.
- **On Windows:** deleting a file or folder whose own path is 260 characters or longer no
  longer fails with a server error. It goes to the Recycle Bin where Windows takes it;
  where it does not, nothing is deleted and the message says why and how to delete it
  instead (shorten the path, or delete it for good from a terminal). A drive without a
  Recycle Bin, a bin turned off, or a file larger than the bin gets the same kind of
  answer.
- **On Windows:** `install.ps1 -Uninstall` (with the `-Prefix` you installed with) removes
  Workbench: the services started from its folder, the files the install put there, the
  `PATH` entry it added, and the folder once nothing else is in it. It keeps your
  configuration and data, and changes nothing while Workbench runs from that folder, or
  when it cannot tell. The executables carry an application manifest (Windows 10 and 11,
  message boxes in the current style, long paths where Windows allows them).
- **Dev containers on Windows:** the install notes, the Help's Agents page and the "not
  available on Windows" message now say how to get them: run the Linux build inside a WSL 2
  distribution with Docker Engine installed in that distribution, and keep its projects in
  the distribution's own folders. A browser on Windows then opens only ports the container
  publishes (`forwardPorts`, `appPort`). With Docker Desktop's WSL integration instead,
  Workbench may be unable to listen on the container network's gateway, and then refuses
  Claude Code sessions inside the container. Neither setup has been tested yet.

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.3.1-x86_64-unknown-linux-gnu.tar.gz` and run
`./workbench-0.3.1-x86_64-unknown-linux-gnu/install.sh`.

Windows 10 (1809 or newer) or 11 on x86_64, experimental: in PowerShell, run
`Unblock-File` on `workbench-0.3.1-x86_64-pc-windows-msvc.zip`. It removes the Mark of the
Web that Windows puts on downloads, so the unpacked files do not carry it. Unpack the zip
with `Expand-Archive .\workbench-0.3.1-x86_64-pc-windows-msvc.zip -DestinationPath .`, then
run its installer:
`powershell -ExecutionPolicy Bypass -File .\workbench-0.3.1-x86_64-pc-windows-msvc\install.ps1`
(`-ExecutionPolicy Bypass` allows the unsigned script for this one run). It installs into
`%LOCALAPPDATA%\Programs\Workbench` without administrator rights and adds that folder to
your PATH. Then run `workbench serve --open` in a new terminal. The binaries are not
code-signed: SmartScreen or an antivirus may warn, and Windows 11's Smart App Control, when
on, blocks them. To remove Workbench, stop it (`workbench service stop`, or Ctrl+C), then
run the same command with `-Uninstall` from a terminal outside Workbench.

Each archive has a `.sha256` to check it against (`sha256sum -c`, or `Get-FileHash` on
Windows).

**Update:** from 0.3.0 or earlier, install the new archive (or pull and rebuild), then
restart Workbench (`systemctl --user restart workbench.service` or the running
`workbench serve`). The restart also ends any agent token an earlier version left behind.
Your configuration and data stay as they are. On Windows, 0.3.1's `install.ps1` installs
over 0.3.0, also while Workbench runs. Then restart Workbench:
- If you started it with `workbench serve`, stop it (Ctrl+C) and start it again in a new
  terminal.
- If it runs as the service (from the Start Menu or at sign-in), run
  `workbench service stop` and open Workbench from the Start Menu.

## 0.3.0 - 2026-09-29

- **Windows (experimental):** the first release with a Windows build, for Windows 10 (1809
  or newer) and 11 on x86_64. Its whole test suite passes on GitHub's `windows-latest`
  (Windows Server 2025), a required CI job, and the release job installs the build with
  `install.ps1`, starts the server and installs again over it before publishing the
  archive. It has not been tried on a Windows 10 or 11 desktop yet
  ([status](docs/windows-port.md)). The archive holds `workbench.exe` (no Visual C++
  runtime needed), `workbenchw.exe`, `install.ps1`, and `conpty.dll` and `OpenConsole.exe`
  from Microsoft's ConPTY package (MIT, see the third-party notices).
- **On Windows:** terminals, agent sessions included, run in ConPTY. Shells are
  PowerShell 7 (`pwsh`), else Windows PowerShell. Agent CLIs start directly, and
  npm-installed ones start as `node` and their script, never through cmd.exe. Run
  configurations and detected commands also run in PowerShell, so a command written for
  bash needs PowerShell's syntax (Windows PowerShell 5.1 has no `&&`: install
  PowerShell 7). Language servers and debug adapters are found in their Windows forms
  (npm-installed ones run with Node). Rust built with MSVC debugs with lldb-dap or
  CodeLLDB, which you name in `[debug.default_adapter]`, since gdb reads only MinGW builds.
  `workbench service install` adds a Start Menu shortcut, and with `--enable` a sign-in
  entry, without administrator rights. The configuration is in `%APPDATA%\workbench` and
  the state in `%LOCALAPPDATA%\workbench`, both readable only by you and SYSTEM. A
  `keyring` secret reference reads Windows Credential Manager. Nothing Workbench reads in a
  project by itself follows a link to a network path or a device (`\\host\share\x`), which
  would make Windows sign in to that computer. Programs it starts (git, language servers,
  agents, your terminals) are not covered. What else works differently there, such as a
  terminal ending a browser or editor it started, is in
  [Install on Windows](docs/getting-started.md#install-on-windows-experimental).
- **Left out on Windows:** dev containers, the server's desktop notifications (turn on
  browser notifications instead), gdb attaching to a running process (native programs
  attach with lldb-dap or CodeLLDB, Python with debugpy), rust-gdb's pretty printers, and
  projects on a network share or inside WSL. The Services window (Docker Desktop) is
  marked experimental. Workbench hides these or says "Not available on Windows" and why
  (the API answers `unsupported_platform`, and `GET /api/health` lists them).
- **Git on Windows:** version control needs Git for Windows. A CRLF checkout
  (`core.autocrlf`, Git for Windows' default, or `eol=crlf` attributes) diffs as git reads
  it, with LF. A conflict resolved with edited text is written back with CRLF, and Local
  History keeps the last commit with the checkout's line ends. A repository that git
  refuses because of its owner (`safe.directory`) reports git's own message
  (`unsafe_repository`), which names the command that trusts it. Git Credential Manager
  never opens a sign-in window for Workbench's remote operations. It answers with what it
  has stored, and an https host it has nothing for fails at once.
- **Terminals:** `[terminals] shell` in `config.toml` sets the program and arguments of new
  shells (default: `$SHELL -l`, as before; PowerShell on Windows). Kill, Restart and Close
  now wait (up to 5 seconds) until the process's exit is recorded and saved. Before, they
  waited only until the process ended, or not at all for a process that had just ended by
  itself. Once they return, the terminal reads as exited. A terminal removed from history
  stays removed: a save still under way no longer writes its files back, so the terminal
  does not return at the next start.
- **Git credentials (security fix):** Workbench no longer answers a credential prompt
  whose user name contains `/`. Git before its January 2025 security releases prints that
  name unescaped, so a crafted remote or submodule URL could get the GitLab token sent to
  another host. Workbench's fetch, update, push and remote-branch deletion also no longer
  ask git's credential helpers for the GitLab host Workbench has a token for (the
  project's own `[repo.gitlab]` token, else `[gitlab]`), nor hand them that token to
  store. The token no longer ends up in Git Credential Manager, `~/.git-credentials`, a
  credential cache or a keychain. While Workbench has a token for that host, a stored
  credential no longer answers in its place, so a project's own token or a rotated one
  takes effect at once. Other hosts keep your helpers. If a credential helper of yours
  knew the GitLab host, Workbench's remote operations now use Workbench's token there, and
  that token needs Git-over-HTTPS access (write access to push). A token that earlier
  versions left with a credential helper stays there: remove it (for `store`, the GitLab
  host's line in `~/.git-credentials`).
- **Run configurations (security fix):** Unity detection takes the editor version from
  `ProjectVersion.txt` only when it consists of version characters (letters, digits, `.`,
  `_`, `-`). Otherwise a crafted `ProjectVersion.txt` could put shell syntax into the
  detected Unity runs.
- **Setup messages:** several messages now name the project's machine overlay,
  `config.toml` and the config folder this Workbench actually reads (`WORKBENCH_CONFIG_DIR`,
  `XDG_CONFIG_HOME`) instead of always `~/.config/workbench`. They are the messages about a
  missing toolchain, an unknown placeholder or an undefined ssh host, the Confluence setup
  hint and its "not available" message, and the TLS certificate and key placeholders in
  Settings › Remote. A default Linux install shows the same text as before.
- **API:** `GET /api/health` also reports `os`, what that OS leaves out (`unsupported`) and
  what it has only as experimental (`experimental`). Both are empty on Linux. An
  `unsupported_platform` error (HTTP 501) names its `feature`. `GET /api/atlassian/status`
  also returns `configFile`, the `config.toml` this server reads.
- **Build from source:** the build also makes `workbenchw`, the Windows launcher of
  `workbench service`. On Linux it is a stub that only prints a message, so install
  `workbench` alone, as before. The Linux archive still holds `workbench` alone.
- **Docs:** Help and the guides cover Windows (install, folders, the service, agents, what
  is left out). customization.md and ARCHITECTURE.md now give the `dotenv` secret
  reference in the form Workbench reads: `dotenv = { path = "…", key = "…" }`.

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.3.0-x86_64-unknown-linux-gnu.tar.gz` and run `./install.sh`.

Windows 10 (1809 or newer) or 11 on x86_64, experimental: in PowerShell, run
`Unblock-File` on `workbench-0.3.0-x86_64-pc-windows-msvc.zip`. It removes the Mark of the
Web that Windows puts on downloads, so the unpacked files do not carry it. Unpack the zip
with `Expand-Archive .\workbench-0.3.0-x86_64-pc-windows-msvc.zip -DestinationPath .`, then
run its installer:
`powershell -ExecutionPolicy Bypass -File .\workbench-0.3.0-x86_64-pc-windows-msvc\install.ps1`
(`-ExecutionPolicy Bypass` allows the unsigned script for this one run). It installs into
`%LOCALAPPDATA%\Programs\Workbench` without administrator rights and adds that folder to
your PATH. Then run `workbench serve --open` in a new terminal. The binaries are not
code-signed: SmartScreen or an antivirus may warn, and Windows 11's Smart App Control, when
on, blocks them.

Each archive has a `.sha256` to check it against (`sha256sum -c`, or `Get-FileHash` on
Windows).

**Update:** install the new release (or pull and rebuild), then restart Workbench
(`systemctl --user restart workbench.service` or the running `workbench serve`). Your
configuration and `~/.local/share/workbench` stay as they are. On Windows, a later
archive's `install.ps1` installs over this one, also while Workbench runs. Then restart
Workbench:
- If you started it with `workbench serve`, stop it (Ctrl+C) and start it again in a new
  terminal.
- If it runs as the service (from the Start Menu or at sign-in), run
  `workbench service stop` and open Workbench from the Start Menu.

## 0.2.0 - 2026-09-29

- **Help:** the user documentation in the app, bundled so it works offline and on a phone:
  getting started, projects, agents, version control and CI, remote access and the phone
  (including Tailscale), running as a service, and `config.toml`. Open it with F1, the
  palette, the status bar's Help, or More → Help on a phone; search covers every page.
- **Docs:** a plan for porting the server to Windows ([windows-port.md](docs/windows-port.md)).

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.2.0-x86_64-unknown-linux-gnu.tar.gz` and run `./install.sh`, then restart
Workbench (`systemctl --user restart workbench.service` or the running `workbench serve`).

## 0.1.0 - 2026-09-29

First public release.

- **Agents:** Claude Code, Codex, Kimi Code, Gemini CLI, Aider and custom CLIs in real
  terminals; permission requests answered from cards, toasts and the phone; Review Changes
  with per-file and whole-session revert; Claude Code Remote Control; MCP tools for the
  UI, CI, Confluence and Jira, Workspace cards, runs, code intelligence, the debugger and
  Local History.
- **Editor:** Monaco on CLion's keymap, language servers with semantic highlighting,
  hierarchies and structure (presets from rust-analyzer and clangd to Verible for
  Verilog and SystemVerilog and vhdl_ls for VHDL), a DAP debugger, Local History,
  bookmarks, compare, scratch files and an HTTP client for `.http` files.
- **Version control:** line staging and partial commits, changelists, the shelf,
  interactive rebase, bisect and a log graph.
- **Forges:** GitLab merge requests, pipelines, jobs and test reports; GitHub pull
  requests, Actions runs and artifacts; issues on both.
- **Atlassian:** Confluence reading and authoring with inline comments, attachments and
  labels; Jira issues and boards.
- **Running things:** detected run configurations, environments with health checks,
  previews and gated deploys, dev containers, a Services window for Docker and a Database
  window for PostgreSQL.
- **Workspace:** deliverable cards with sandboxed reports, galleries, PDFs and 3D
  comparisons, compatible with Mr. Mak Workspace's registry; four example cards in Home
  on the first start.
- **Phone:** an installable app with its own tabs and push notifications.

**Install:** Linux x86_64 (glibc 2.35 or newer): unpack
`workbench-0.1.0-x86_64-unknown-linux-gnu.tar.gz` and run `./install.sh`, or build from
source. Windows is not supported yet.

**Update:** install the new release (or pull and rebuild), then restart Workbench
(`systemctl --user restart workbench.service` or the running `workbench serve`). Your
configuration and `~/.local/share/workbench` stay as they are.
