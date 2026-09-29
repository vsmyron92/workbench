# Remote access, phone and notifications

By default Workbench listens only on this computer (`127.0.0.1:7777`). To use it from another laptop or a phone you need three things: a route to the machine, an address Workbench accepts, and a signed-in device.

## 1. A route: Tailscale (recommended)

Tailscale gives every device you own a private address, and the traffic never crosses the public internet.

1. Install Tailscale on this computer, the other laptop and the phone, and sign in to the same account on all of them.
2. On this computer run:

   ```bash
   sudo tailscale serve --bg 7777
   ```

   This publishes Workbench over HTTPS **to your tailnet only** and leaves Workbench itself bound to loopback. `tailscale serve status` shows the address, for example `https://my-machine.tail1234.ts.net`. If Tailscale asks, enable HTTPS certificates in its admin console under DNS.

Other routes work too: a Caddy reverse proxy, `[server.tls]` in the config, or binding to a Tailscale or LAN address with `bind = "100.x.y.z:7777"`. The local listener keeps working in every case.

> [!WARNING]
> Workbench can run commands and holds your service tokens. Do not bind it to `0.0.0.0` on a network you do not control.

## 2. An address Workbench accepts

Workbench refuses requests whose `Host` is not one it knows, which defends against DNS rebinding. Without this step the page answers **403: unrecognized Host header**. Add the address in `config.toml`:

```toml
[server]
allowed_hosts = ["my-machine.tail1234.ts.net"]
public_url = "https://my-machine.tail1234.ts.net"
```

`public_url` also makes the pairing QR code carry that address whatever page you generated it from. You can set both in **Settings → Remote access**. A change to `bind`, `allowed_hosts` or `public_url` needs a restart, and Settings shows *Restart required* until you do it.

## 3. Sign in the device

Opening the address shows a sign-in page. Do not paste the master token on a phone. Pair instead:

1. On a device that is already signed in, open **Settings → Remote access** (or press Ctrl+K and run *Pair a device…*).
2. Scan the **QR code** with the phone, or open the link on the laptop. The code works once and expires after 10 minutes.
3. The device stays signed in. Every device is listed on the same screen and can be **revoked**; revoking closes its open connections at once.

On the computer itself, `workbench open` signs the browser in with a one-time code. `workbench url` prints a login URL that contains the master token: keep it to yourself.

If the QR code shows a `localhost` address, `public_url` is not set. Open Settings from the tailnet address, or set `public_url` and restart.

## The phone app

Open the address on the phone and choose *Add to Home Screen*: Workbench then runs as an app. The phone shows Agents (with the permission requests), Git, Workspace, Files, CI, Apps and More. Code intelligence, the debugger and Local History are desktop-only.

## Push notifications

In **Settings → Notifications → Push** (or the phone's More tab) you can turn on notifications that arrive while Workbench is closed:

- an agent that needs you, with **Allow** and **Deny** on the notification itself when the request is short enough to read there;
- a finished turn;
- an environment going down;
- a failed deploy or pipeline.

Push needs **HTTPS**. Browsers offer it only to secure pages, so a plain-http LAN or Tailscale IP does not work; use `tailscale serve`, Caddy or `[server.tls]`. On iPhone and iPad it works only in the Home Screen app (iOS 16.4 or later). Notifications go through the browsers' own push services (Google, Mozilla, Apple, Microsoft), end-to-end encrypted, with titles and short summaries only.

## Troubleshooting

| You see | Cause and fix |
|---|---|
| **403** *unrecognized Host header* | The address is not in `allowed_hosts`. Add it and restart. |
| Sign-in page with an *Access token* box | The device is not paired yet. Pair it from a signed-in device. |
| QR code points at `localhost` | Set `public_url`, or generate the code from the tailnet address. |
| Page does not load at all | Check `tailscale serve status`, that Workbench is running, and that both devices are on the tailnet. |
| Works, then stops after a reboot | Workbench is not running as a service. See [Running Workbench as a service](service). |
