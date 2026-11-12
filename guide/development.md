# Working on Slipstream

[← back to the README](../README.md)

| Path | What it is |
|---|---|
| `slipstream/` | The compositor (Rust and [Smithay](https://github.com/Smithay/smithay)) |
| `slipstream-settings/` | The Settings app (GTK 4) |
| `slipstream-config/` | The settings file both of them share |
| `session/` | The login session's launcher, entry, systemd targets and portal settings |
| `scripts/` | Installing, updating, packaging, and running it in a window |

```sh
scripts/nested              # builds in the slipstream distrobox and opens foot inside Slipstream, as a window
scripts/nested firefox      # or another program
cargo test --workspace      # in the distrobox
scripts/test-install-session
```

In a window, **Alt stands in for Super**, since your desktop keeps Super for itself. The distrobox is [set up here](install.md#building-it-yourself).
