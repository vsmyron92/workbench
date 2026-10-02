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
  and every MCP tool is confined to the session's project.
- Agents never answer permission requests, deploy, run destructive git operations, start
  a language server or a debug session, query a database, or update or restart Workbench
  through Workbench. Run configurations that deploy or release are refused to them.
- Agents can steer a debug session you started (continue, pause, step, stop, plain
  breakpoints): those tools are writes, so the agent's permission prompts and the Activity
  view treat them as such. They cannot evaluate expressions or set conditional
  breakpoints and log points, because a debugger's expressions can run commands.
- Workspace reports an agent writes are served sandboxed: they cannot read Workbench's
  cookies, storage or API.

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
