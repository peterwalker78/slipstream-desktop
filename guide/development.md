# Working on Slipstream

[← back to the README](../README.md)

| Path | What it is |
|---|---|
| `slipstream/` | The compositor (Rust and [Smithay](https://github.com/Smithay/smithay)) |
| `slipstream-settings/` | The Settings app (GTK 4) |
| `slipstream-config/` | The settings file both of them share |
| `session/` | The login session's launcher, entry, systemd targets and portal settings |
| `scripts/` | Installing, updating, packaging, and running nested |

```sh
scripts/nested              # builds in the box and opens foot inside Slipstream, as a window
scripts/nested firefox      # or another program
cargo test --workspace      # in the box
scripts/test-install-session
```

Nested, the window holds the keyboard while it has focus, Super included, so the keys are the same as in the session. Press and let go of **Ctrl+Alt** to give the keyboard back to your desktop, and click the window to take it again; the window's title says which. On X11, or a Wayland desktop that doesn't support keyboard-shortcuts-inhibit, your desktop keeps the keys it uses itself.
