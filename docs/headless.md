# Headless operation

Radmin can be run without a local desktop for servers/VPSs.

## Service-only mode

Use the systemd user service:

```bash
systemctl --user enable --now radmin-vpn.service
```

The service starts the TAP adapter, bridge, firewall/routes, and Wine/Radmin processes according to its configuration. Interaction with the Radmin GUI is optional; when `--no-ui` is set, no desktop session is required. When a graphical session exists, the service injects its `DISPLAY`/`XAUTHORITY` into Wine processes so the desktop and the GUI work normally; see [installation.md](installation.md#graphical-session-and-the-radmin-gui).

Example unit command line:

```ini
ExecStart=/usr/local/bin/radmin-vpn --json-logs service --no-ui
```

## GUI in a browser (manual)

If you want the real GUI remotely, run it under a virtual framebuffer and expose it with VNC/noVNC. This is outside the scope of the built-in service, but the same Wine prefix and TAP adapter are used.

Minimal flow:

1. Start the service with `--no-ui` or with the GUI enabled on a virtual display.
2. Expose the display via `x11vnc` and noVNC as desired.
3. Tunnel or bind appropriately with firewall rules.

## Notes

- The service manages TAP routes/firewall state and must be stopped cleanly.
- Running as root is required for TAP, routing, and firewall configuration.
- See [systemd.md](systemd.md) for hardening guidance and [installation.md](installation.md) for distro-specific steps.
