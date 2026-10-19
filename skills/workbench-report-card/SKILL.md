---
name: workbench-report-card
description: >-
  Hands findings, reports, comparisons, image galleries and plans to the user as a Workspace card (tabs next to the code) in a Workbench session, instead of terminal scrollback: reuse a fitting card, write a sandbox-safe HTML or Markdown report, register steps, open the card. Use in a Workbench session when the user asks for a write-up, report, comparison, analysis, gallery, plan or anything they will come back to, or says to put it in a card. Not for short answers.
license: MIT
compatibility: "Needs Workbench's MCP tools (a Claude Code or Codex session started from Workbench, version 0.16 or later). Without them, ignore this skill."
---

# Handing work over as a Workspace card

You are in a Workbench session only if tools of the `workbench` MCP server are in your tool list. If they are not, stop using this skill and write the deliverable as an ordinary file. Claude Code shows the tools as `mcp__workbench__<name>` (for example `mcp__workbench__workspace_create_card`); this skill uses the bare names. A tool that seems missing may only be deferred: load it with the tool-search facility before concluding it is unavailable.

A card is a deliverable with tabs, called steps: an HTML report, Markdown, a folder of images, a PDF, a 3D comparison. The user opens it next to the code, pins it, comes back to it later and can edit its Markdown.

## When to use

- A report, comparison, analysis, plan or gallery; anything with tables or figures, longer than a screenful, or that the user will come back to or edit.
- Not for a short answer, a status line or a code change: answer in the terminal, and show code with `workbench_show_diff`.
- A prompt that starts "About the Workspace card ... (cardId ..., scope ...)" comes from that card's Ask agent button: change that card, do not create another.

## Workflow

1. `workspace_list_cards` (`query`: words of a title, description, category or id; a query also finds archived cards). Reuse a card that fits and add a step to it. Note the categories already in use.
2. Otherwise `workspace_create_card` (`title`, `description`, `category`). It returns `cardId` and `folder`, an absolute path outside the repository. Use the returned `cardId` from then on, not the title.
3. Write the files into `folder` with your own file tools, under relative names (`report.html`, `img/chart.png`). Sessions started by Workbench may write there for their own project's cards and for `home`. For `wb-sandbox`, or inside a dev container, or when your sandbox refuses, use `workspace_write_file` (`cardId`, `path`, `content`; `encoding: "base64"` for images; 20 MB a call).
4. `workspace_add_step` (`cardId`, `name`, `path`, optional `viewer`) once per tab, the summary first. A file is not a tab until it is registered. `path` is relative to `folder`. A file elsewhere in your project is copied into the card at that moment; later edits to the original are not followed.
5. Read the answer: `exists: false` means the file is not in the folder yet, and the tab shows "File missing" until you write it. If you can render HTML (a headless browser), screenshot `file://<folder>/report.html` first: the `../_shared/` styles resolve there too. It is not sandboxed, so still follow the rules below.
6. `workspace_open_card` (`cardId`, `step`: a 0-based tab) once, when the deliverable is ready. Without it the card sits in the Workspace list and the user is not pointed at it.
7. In the terminal, say in one or two lines what the card holds and give its title. Do not paste the report as well.
8. To change it later, edit the file in place (an open tab reloads by itself) and call `workspace_update_card` (`cardId`) so the card rises to the top, or add a step for a new version. `workspace_add_step` and `workspace_write_file` already count as a change; writing files with your own tools does not.

## What makes a good card

- One task, one card. Iterations are steps, named like `v1 · first pass` and `v2 · tighter layout`; keep an earlier step unless it was wrong. The card opens on its last step.
- A short, readable title. A description of one or two sentences: what it is and why. A short lower-case category, hyphenated if needed, reused from the cards that exist (research, report, design, image-gen, analytics, dev): the Workspace groups cards by it.
- Answer first. A report opens with the result, then the evidence, then what was checked and what was not.
- Honest status. Leave it `active` while the card is a draft or waits on a decision, and name the decision in the report. Set `status: "done"` with `workspace_update_card` only when nothing is open.
- Archive (`status: "archived"`) only a card of yours that a newer card replaces, or when the user says it no longer matters. Cards untouched for 7 days archive themselves unless pinned. Pin (`pinned: true`) only when asked.
- Leave `scope` out: the default is your project. Use `home` for work that belongs to no project, and `wb-sandbox` only for throwaway experiments the user asked for.
- A card whose id starts with `repo:` comes from the repository's own `workspace/workspace.json`: you can change only its status and pin.

## Writing the HTML report so it works in the sandbox

A report is untrusted HTML that runs scripts, so Workbench shows it in a frame with an opaque origin: it can never reach Workbench, its cookies, storage or API, and it cannot load or call anything from other hosts. Only files of the card folder and `../_shared/` are served.

- Blocked: anything from another host (CDN scripts and styles, web fonts, hotlinked images, `fetch` to the web); `eval`, `new Function` and WebAssembly; `localStorage`, `sessionStorage`, cookies and IndexedDB (they throw, so wrap them in `try`/`catch`); form submission, and `alert` or `confirm` (ignored); Web Workers loaded from files; any access to Workbench or its page.
- Fine: inline `<style>` and `<script>`, script files and ES modules in the card folder, `fetch('data.json')` of a file there, font files in the folder, `data:` and `blob:` URLs, inline SVG and canvas, `#anchor` and relative links. `https:` links open in a new browser tab.
- Paths are relative: `img/chart.png`, `../_shared/report.css`. Never an absolute path, `file://` or `/img/x.png`. Dotfiles, `node_modules` and credential-like names are never served.
- Start from [references/html-skeleton.md](references/html-skeleton.md): `data-wb-report="document"` on `<html>`, `../_shared/report.css`, and `../_shared/report.js` with `defer`. Without the attribute no style applies.
- Predefined classes: `card`, `grid-2`, `grid-3`, `grid-4`, `gallery`, `kpis` with `kpi`, `good`, `warn` and `bad` (text colour), `meta`, and the elements `main`, `h1` to `h3`, `table`, `pre`, `figure`. Nothing else exists (no badges, callouts or buttons): add a small `<style>` that uses the `--wbr-*` variables. The authority is `../_shared/report.css`, next to your card folder: read it when unsure.
- Images are files in the card folder, with alt text and `loading="lazy"`. `report.js` opens every `<img>` that is not inside a link in a lightbox (arrow keys, 100% toggle, download) and uses its `alt` as the caption: write alt text that works as a caption, and use inline SVG, not `<img>`, for decoration.
- Tables and `pre` blocks scroll sideways by themselves under `report.css`; wrap other wide content in an `overflow-x: auto` box.
- Charts: render PNG or SVG files with your own tools into the card folder, or draw inline SVG. A charting library must be a local file (not under `node_modules`) that does not use `eval`.
- Lead with the key figure: the card's thumbnail is taken from the first local image of its report (or from an image step).

## Other step types

`viewer` is `auto` by default and follows the extension. [references/step-types.md](references/step-types.md) has the table, the limits and the 3D schema.

- Markdown for plans and notes. The user can edit and save it in the card, so read the file again before you overwrite it.
- A folder of images is one gallery step. The file names are the captions, so name them `01-baseline.png`; put rounds in subfolders.
- PDF, video, audio, a single image and text files (JSON, CSV, logs, up to 2 MB) have their own viewers.
- A 3D comparison is a JSON manifest registered with `viewer: "compare3d"`; a single `.glb` or `.gltf` opens by itself.
- An unknown extension (zip, xlsx, docx) shows only a download button: say in the description what it is.

## Mistakes to avoid

- CDN scripts, web fonts and remote images, or Tailwind, Bootstrap or a chart library from a CDN: they are blocked and the page renders without them. Save what you need into the card folder.
- Absolute or root-relative paths in HTML or Markdown.
- A card per iteration instead of a step per iteration.
- Writing files and stopping: unregistered files are not tabs, and an unopened card is easy to miss. Register each step, then open the card once.
- Registering a step before its file exists.
- Forgetting `data-wb-report="document"`, or the wrong `../_shared/` depth in a report inside a subfolder.
- A 3D manifest registered without `viewer: "compare3d"`: it shows as JSON text.
- Overwriting a Markdown step the user edited, or using `localStorage` in a report.
- Pasting the whole report into the terminal as well, or pinning cards unasked.

## Reading more

- [references/html-skeleton.md](references/html-skeleton.md): a complete working report to copy, extras (a custom style, an inline SVG chart) and what each class is for. Read before writing a report.
- [references/step-types.md](references/step-types.md): viewer by extension, what each viewer does, the 3D manifest schema with an example, and the size and name limits. Read before adding anything other than a report or Markdown, and when a tool refuses a name or a size.
