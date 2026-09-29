# Running Workbench as a service

A Workbench started by hand stops when its terminal or login session ends, and does not come back after a reboot. Installing it as a systemd user service fixes both.

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

Use `workbench service status` to see the real unit name. Agent sessions are brought back on start when `[agents] restore_on_start = true`.

## Checking that it is up

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7777
```

prints `200` when the server answers.
