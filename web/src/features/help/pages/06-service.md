# Running Workbench as a service

A Workbench started by hand stops when its terminal or login session ends, and does not come back after a reboot. Installing it as a service fixes both: a systemd user service on Linux, a sign-in entry on Windows (see Windows below).

```bash
workbench service install --enable   # writes the unit and a desktop launcher, starts it now
workbench service status
workbench service uninstall
```

- `install` without `--enable` only writes the files and prints the next steps. `--dry-run` shows them first.
- The desktop launcher runs `workbench open`, which signs the browser in with a one-time code and keeps its existing session.
- To keep the service running while you are logged out, so a phone can reach it any time, run `loginctl enable-linger $USER`.

## Restarting

Some settings only apply after a restart, and Settings tells you when: *Restart required*. With the service installed, restart from a terminal outside Workbench:

```bash
systemctl --user restart workbench
```

Use `workbench service status` to see the real unit name. Agent sessions are brought back on start when `[agents] restore_on_start = true`. On Windows, run `workbench service stop`, then open Workbench from the Start Menu.

## Updating

A Workbench installed from a release looks for a newer one once a day and shows **Update X.Y.Z** in the status bar. **Settings › Updates** has the release notes and two buttons:

- **Check now** looks at once.
- **Update and restart** downloads the release, compares its SHA-256 with the release's checksum, replaces the `workbench` program and restarts into it. It asks first and says what the restart stops: agent sessions resume afterwards (with `restore_on_start`), shells start again under their last screen, runs do not. The page reloads by itself when the new version is up.

Nothing is installed without your click, and agents cannot start an update. The version that was replaced stays beside the new one as `workbench.prev`: to go back, rename it over `workbench` and restart.

From a terminal, `workbench update` does the same (`--check` only looks, `--restart` also restarts the running Workbench).

Looking is one request a day to GitHub for the latest release's description, without a token. Turn it off in Settings › Updates, or in `config.toml`:

```toml
[update]
check = false
```

A Workbench you built yourself does not know where its releases are published. Name the GitHub repository to get updates from it: `repo = "owner/name"` under `[update]`.

When Workbench cannot write to the folder it is installed in (a system-wide install), or on Windows, it only tells you about the new version. Install it by hand: download the release archive, unpack it and run its `install.sh`, which replaces `~/.local/bin/workbench` (a running Workbench keeps using the old file until it restarts), then press **Restart now** in Settings › Updates or restart the service as above. Your configuration and `~/.local/share/workbench` stay as they are.

On Windows, run the new archive's `install.ps1`, which works while Workbench runs (it also updates `conpty.dll` and `OpenConsole.exe`), then `workbench service stop` and open Workbench from the Start Menu.

## Checking that it is up

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7777
```

prints `200` when the server answers. `workbench service status` also says whether a server runs.

## Windows

A Windows service would lose your desktop and Credential Manager and need administrator rights, so on Windows Workbench starts when you sign in instead, as your own user.

```powershell
workbench service install --enable   # a Start Menu shortcut and a sign-in entry; starts it now
workbench service status
workbench service stop
workbench service uninstall
```

- `install` writes `%LOCALAPPDATA%\workbench\service.json`, which keeps the `WORKBENCH_CONFIG_DIR`, `WORKBENCH_DATA_DIR` and `WORKBENCH_LOG` set in the shell you run it from, and a **Workbench** shortcut in the Start Menu. `--enable` adds the sign-in entry (`Workbench` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`) and starts Workbench now.
- Both run `workbenchw.exe`, which comes with `workbench.exe` and has to stay in the same folder. It shows no console window. It runs `workbench serve`, restarts it 5 seconds after it fails, and gives up after 5 failures within a minute, with a message saying so. It starts nothing while a Workbench you started by hand serves the same data folder.
- The Start Menu's **Workbench** opens a signed-in window like `workbench open`, and starts Workbench first when it is not running. Without `--enable` you get Workbench on demand, from the Start Menu.
- The server's output goes to `%LOCALAPPDATA%\workbench\service.log`.
- Task Manager › Startup apps lists the entry; turning it off there keeps Windows from starting Workbench at sign-in, and `workbench service status` shows it as turned off.
- `workbench service stop` asks Workbench to stop and waits until its port is free. `install --enable` over a running service restarts it with the new settings. Run from a Workbench terminal, that terminal closes as the old Workbench stops; from a terminal that keeps everything it starts (one whose processes cannot leave its job), it restarts nothing and tells you what to do instead.
- In a terminal started with *Run as administrator*, `install --enable` writes everything but starts nothing: Workbench and its agents would run as administrator too. The sign-in entry and the Start Menu start it as you.
- `workbench service` reaches only the Workbench of its own Windows sign-in. Over SSH it runs in another session than your desktop's: `status` then says Workbench runs in another session, and `stop` and `install --enable` refuse. Run them on the desktop, or end Workbench in Task Manager.
- To update, run the new archive's `install.ps1` (see Updating above).
- `install --enable` and the Start Menu start Workbench with your sign-in environment, as Windows does when you sign in, not with the environment of the shell you run them from (an activated virtual environment's `PATH` is not carried over). Only `WORKBENCH_CONFIG_DIR`, `WORKBENCH_DATA_DIR` and `WORKBENCH_LOG` come from `service.json`.
- A program you install later, such as Node.js or Python, is found without a restart: by new terminals, runs and agent sessions, and when Workbench looks for a language server or debugger. Git for Windows is the exception: the Git features find it only after a restart (`workbench service stop`, then open Workbench from the Start Menu).
