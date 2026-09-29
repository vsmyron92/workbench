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

Download the new release archive, unpack it and run its `install.sh`, which replaces `~/.local/bin/workbench` (a running Workbench keeps using the old file until it restarts). Then restart the service as above. Your configuration and `~/.local/share/workbench` stay as they are.

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
- `workbench service stop` asks Workbench to stop and waits until its port is free. `install --enable` over a running service restarts it with the new settings.
- In a terminal running as administrator, `install --enable` writes everything but starts nothing: Workbench and its agents would run as administrator too. The sign-in entry and the Start Menu start it as you.
- To update, stop the service, replace `workbench.exe` and `workbenchw.exe`, and open Workbench from the Start Menu.
