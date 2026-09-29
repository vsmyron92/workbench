# Changelog

## Unreleased

- **Editor:** C, Verilog, SystemVerilog and VHDL syntax, and more C++ extensions (`.cxx`,
  `.hh`, `.inl`, `.ipp`, `.tpp`, `.ixx`, `.cppm`, `.ino`, `.cu`…), in the editor, diffs,
  Markdown code blocks and the phone's file viewer. TODO comments, scratch files, file icons
  and Confluence code blocks know them too.
- **Code intelligence:** Verilog and SystemVerilog through Verible, VHDL through vhdl_ls
  (built-in presets; install either and enable code intelligence). clangd also serves the
  new C++ extensions. `code_symbols` says so when the running servers cannot search
  symbols by name, instead of reporting that none runs.
- **Workspace:** four example cards in Home on the first start, as in Mr. Mak Workspace:
  Welcome to Workbench, A tour of Workbench, Hand work to an agent (with a report
  template) and Connect your services. They stay until you archive them.

## 0.1.0 - 2026-09-28

First public release.

- **Agents:** Claude Code, Codex, Kimi Code, Gemini CLI, Aider and custom CLIs in real
  terminals; permission requests answered from cards, toasts and the phone; Review Changes
  with per-file and whole-session revert; Claude Code Remote Control; MCP tools for the
  UI, CI, Confluence and Jira, Workspace cards, runs, code intelligence, the debugger and
  Local History.
- **Editor:** Monaco on CLion's keymap, language servers with semantic highlighting,
  hierarchies and structure, a DAP debugger, Local History, bookmarks, compare, scratch
  files and an HTTP client for `.http` files.
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
  comparisons, compatible with Mr. Mak Workspace's registry.
- **Phone:** an installable app with its own tabs and push notifications.

**Update:** pull, rebuild the web app and the binary, install it, then restart Workbench
(`workbench service` or the running `workbench serve`). Your configuration and
`~/.local/share/workbench` stay as they are.
