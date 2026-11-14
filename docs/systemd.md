# systemd user service

`packaging/systemd/radmin-vpn.service` runs `radmin-vpn --json-logs service` as a `Type=notify` **user** service in the calling user's `systemd --user` manager (`/usr/lib/systemd/user/radmin-vpn.service`).

The Radmin VPN control plane (RvControlSvc) and the Radmin GUI must share the same Wine session — running as a system service under an arbitrary user or `root` breaks that. A user service sidesteps both problems: no `User=`/`Group=` to configure, and the service shares the display session, Wine prefix and security context of the desktop user.

Enable it with `systemctl --user`, not `systemctl`:

```sh
systemctl --user daemon-reload
systemctl --user enable --now radmin-vpn.service
# optional — start at boot without logging in:
loginctl enable-linger "$USER"
journalctl --user -u radmin-vpn.service -f
```

## Service behavior

```text
ExecStart=/usr/bin/radmin-vpn --json-logs service
```

- `ExecStartPre` checks that `/dev/net/tun` is a character device. If it is missing, systemd refuses to start the unit and the failure is visible in `journalctl --user -u radmin-vpn.service`.
- `Type=notify` + `NotifyAccess=main`: the process calls `READY=1` after the TAP adapter, routes, firewall rules, and lock are established. Until then, systemd considers the unit not started.
- The runtime directory is created as `$XDG_RUNTIME_DIR/radmin-vpn` with `RuntimeDirectory=radmin-vpn` and `RuntimeDirectoryMode=0750`. The lock file and the control socket (`rvpn.control`) live there.
- The persistent state directory is created as `$XDG_STATE_HOME/radmin-vpn` (usually `$HOME/.local/state/radmin-vpn`) with `StateDirectory=radmin-vpn` and `StateDirectoryMode=0750`. The TAP ownership record lives in `tap.state` there so it survives `systemctl --user stop` (systemd removes the runtime directory on stop, but the persistent TAP interface does not disappear); the adapter MAC is persisted there too.

## Reload behavior (`ExecReload`)

```text
ExecReload=/bin/kill -s SIGHUP $MAINPID
```

`SIGHUP` is handled by the Rust service as a graceful shutdown request. `Restart=always` then restarts the unit, effectively performing a **reload-by-restart**.

> There is no hot-reload path for a running VPN session. `systemctl --user reload radmin-vpn.service` is a convenience alias for "gracefully stop and immediately start again". Use `systemctl --user restart` if you prefer explicit semantics.

To inspect the outcome:

```sh
systemctl --user reload radmin-vpn.service
journalctl --user -u radmin-vpn.service -f
```

## Privilege model (file capabilities)

The user's `systemd --user` manager has no network capabilities and cannot grant them to children (`AmbientCapabilities` on a user unit makes the child fail at the `CAPABILITIES` step, or silently runs without them). Instead the binary itself carries the capabilities, installed via `setcap`:

```sh
setcap 'cap_net_admin,cap_net_raw+ep' /usr/bin/radmin-vpn
```

`make install` and the package postinsts apply it automatically; a reinstall always re-applies it because `install` recreates the file. Only `cap_net_admin` (TAP creation, routes, addresses) and `cap_net_raw` are granted — never broader caps.

`NoNewPrivileges=false` is required for file capabilities to be honoured on `exec`. Likewise, **no namespace-based hardening may be enabled on this unit**: `ProtectSystem`, `PrivateTmp=true`, `RestrictSUIDSGID`, `LockPersonality` and `RestrictAddressFamilies` all cause the unit to be run inside a user namespace, where the file capabilities do not apply to the host network namespace and `TUNSETIFF`/netlink operations fail with `EPERM`. (This was verified by bisecting the unit on a live system: any one of those directives is enough to break the TAP ioctl.) Keep the unit free of namespace-based options; keep `ProtectHome=false` because the service and Wine must read/write the prefix under the user's home.

As a consequence of having no privilege boundary available in a user service, network configuration (`26.0.0.0/8` routes, `fdfd::` addresses, link up/delete) is done **in-process via rtnetlink** — no `iproute2` dependency, and no subprocess ever needs the capabilities.

The firewall rules (`nft`/`iptables`) still require a host-side privilege the user service does not have and are applied on a best-effort basis (failure is a warning, never fatal). On distros where the default `INPUT` policy is `ACCEPT` (typical desktops) the VPN works without them; on hardened hosts add the rule once: `sudo nft insert rule inet filter input iifname "radminvpn0" accept` (or `sudo iptables -I INPUT -i radminvpn0 -j ACCEPT`).

## Logs

Structured JSON logs are emitted by default in the unit (`--json-logs`).

```sh
journalctl --user -u radmin-vpn.service -f
```

## Graphical session environment

The service runs without a session `DISPLAY` until the graphical session comes up, so it detects the user's graphical session by reading `/proc` of same-uid processes (e.g. plasmashell, kwin, Xwayland) and injects `DISPLAY`/`XAUTHORITY` (or `WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR` for pure Wayland sessions) into every Wine process it launches. Without this, the Wine session's desktop is created before any process has a display and no window can open (`err:winediag:nodrv_CreateWindow`).

At boot before login there is no session to inject; the VPN still comes up headless, and after logging in a one-time `systemctl --user restart radmin-vpn.service` re-creates the Wine session with the session display.

## Ownership and crash cleanup

The service records the interface it owns in the state directory (`$HOME/.local/state/radmin-vpn/tap.state`). `$XDG_RUNTIME_DIR/radmin-vpn/tap.state` would be deleted together with the runtime directory on `systemctl --user stop`, which previously left a stale persistent `radminvpn0` behind that the next start refused to touch. Clean shutdown deletes that interface. `ExecStopPost` performs best-effort cleanup for crash/forced-kill scenarios so the next boot or a manual teardown is consistent.

Manual teardown (idempotent):

```sh
radmin-vpn teardown --tap-dev radminvpn0 --remove-tap
```

## Verification checklist

```sh
systemctl --user status radmin-vpn.service
journalctl --user -u radmin-vpn.service --no-pager -n 100
ip addr show radminvpn0
ls -l "$XDG_RUNTIME_DIR/radmin-vpn/service.lock"
cat "$HOME/.local/state/radmin-vpn/tap.state"
systemctl --user stop radmin-vpn.service
ip link show radminvpn0 || echo "TAP removed after stop"
```
