# systemd service

`packaging/systemd/radmin-vpn.service` runs `radmin-vpn --json-logs service` as a `Type=notify` service.

## Service behavior

```text
ExecStart=/usr/lib/radmin-vpn/radmin-vpn --json-logs service
```

- `ExecStartPre` checks that `/dev/net/tun` is a character device. If it is missing, systemd refuses to start the unit and the failure is visible in `journalctl -u radmin-vpn.service`.
- `ConditionPathExists=/dev/net/tun` (plus `ExecStartPre=/usr/bin/test -c /dev/net/tun`) documents and enforces the TUN/TAP requirement.
- `Type=notify` + `NotifyAccess=main`: the process calls `READY=1` after the TAP adapter, routes, firewall rules, and lock are established. Until then, systemd considers the unit not started.
- The runtime directory is created as `/run/radmin-vpn` with `RuntimeDirectory=radmin-vpn` and `RuntimeDirectoryMode=0750`. The lock file lives there.
- The persistent state directory is created as `/var/lib/radmin-vpn` with `StateDirectory=radmin-vpn` (`StateDirectoryMode=0750`). The TAP ownership record lives in `/var/lib/radmin-vpn/tap.state` so it survives `systemctl stop` (systemd removes the runtime directory when the unit stops, but the persistent TAP interface does not disappear).

## Reload behavior (`ExecReload`)

```text
ExecReload=/bin/kill -s SIGHUP $MAINPID
```

`SIGHUP` is handled by the Rust service as a graceful shutdown request. `Restart=always` then restarts the unit, effectively performing a **reload-by-restart**.

> There is no hot-reload path for a running VPN session. `systemctl reload radmin-vpn.service` is a convenience alias for "gracefully stop and immediately start again". Use `systemctl restart` if you prefer explicit semantics.

To inspect the outcome:

```sh
systemctl reload radmin-vpn.service
journalctl -u radmin-vpn.service -f
```

## Hardening notes

The unit uses conservative hardening (`NoNewPrivileges`, `ProtectSystem=full`, `ProtectHome=false`, capability bounding set, restricted address families). `ProtectHome=false` is required because the service and Wine must be able to read/write the Wine prefix and Radmin installation directory under the user's home. `ProtectSystem=full` keeps `/usr`, `/boot` and `/etc` read-only while leaving `/run` and `/tmp` writable, which the service needs for the lock file, runtime state, FIFOs, and MAC/control files. `PrivateTmp=false` is intentional because the FIFO/control contract uses fixed paths under `/tmp` for Wine-visible IPC; tightening this requires changing the IPC paths in both Linux and Wine-visible components together.

## Logs

Structured JSON logs are emitted by default in the unit (`--json-logs`).

```sh
journalctl -u radmin-vpn.service -f
```

## Graphical session environment

The systemd unit runs without a session `DISPLAY`, so the service detects the user's graphical session by reading `/proc` of same-uid processes (e.g. plasmashell, kwin, Xwayland) and injects `DISPLAY`/`XAUTHORITY` (or `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` for pure Wayland sessions) into every Wine process it launches. Without this, the Wine session's desktop is created before any process has a display and no window can open (`err:winediag:nodrv_CreateWindow`).

At boot before login there is no session to inject; the VPN still comes up headless, and after logging in a one-time `systemctl restart radmin-vpn.service` re-creates the Wine session with the session display.

## Ownership and crash cleanup

The service records the interface it owns in `/var/lib/radmin-vpn/tap.state` (persistent). `/run/radmin-vpn/tap.state` would be deleted together with the runtime directory on `systemctl stop`, which previously left a stale persistent `radminvpn0` behind that the next start refused to touch. Clean shutdown deletes that interface. `ExecStopPost` performs best-effort cleanup for crash/forced-kill scenarios so the next boot or a manual teardown is consistent.

Manual teardown (idempotent):

```sh
sudo /usr/lib/radmin-vpn/radmin-vpn teardown --tap-dev radminvpn0 --remove-tap
```

## Verification checklist

```sh
systemctl status radmin-vpn.service
journalctl -u radmin-vpn.service --no-pager -n 100
ip addr show radminvpn0
ls -l /run/radmin-vpn/service.lock
cat /var/lib/radmin-vpn/tap.state
systemctl stop radmin-vpn.service
ip link show radminvpn0 || echo "TAP removed after stop"
```
