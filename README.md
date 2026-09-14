# Slipstream Desktop

A keyboard-first Wayland desktop for Linux: its own compositor, bar, notifications, lock screen and Settings app. Windows are places you fly to (bullet time), weight by importance (gravity) and park at the edge of the screen while they keep running (code rain). The keys follow Windows wherever they don't fight a keyboard-first design, so Windows habits carry over.

It's built to protect your concentration: windows that open while you type don't take the keyboard, and notifications wait for a natural pause.

**Status: beta.** It's developed and used on Bazzite (Fedora Atomic). Expect rough edges, and changes between versions. See [CHANGELOG.md](CHANGELOG.md).

## What it does

- **Tiling** across named workspaces (up to 20), with several screens, fullscreen, and X11 apps through XWayland.
- **Gravity** (Super+T): weight windows from distant to tiled with Super+PgUp and PgDn.
- **Bullet time** (Super+Tab): a 3D overview of every workspace, with letter hints to jump anywhere. Everything the compositor draws slows down while it's open.
- **Code rain** (Super+M): minimised windows become streams down the edge of the screen, showing each app's CPU and memory as they run.
- **App explorer** (Super+Space or Super+R): apps, files, and commands to run.
- **Living wallpaper**: when you step away, the desktop fades to one of twenty animated variations.
- **Concentration first**: see [below](#concentration-first).
- **Bar and quick settings** (Super+A): clock and calendar, Wi-Fi, Bluetooth, volume, brightness, night light, power mode. Media keys work with any player that speaks MPRIS.
- **Notifications** (Super+N): pop-ups with actions, a notification centre, do not disturb.
- **Lock screen** (Super+L), before sleep and optionally after a while idle.
- **Screen sharing** through xdg-desktop-portal-wlr, with Slipstream's own chooser for a screen or a single window. It works end to end in testing, but is still new with real apps.
- **Session**: logging out asks apps to close and waits for them; your layout can be remembered and reopened at login.
- **Settings** (Super+I): applied as soon as you change them, and saved in `~/.config/slipstream/settings.toml`.

## Concentration first

- **Typing** is judged from when keys reach an app, never from which keys. It ends at a pause of four seconds, a shortcut, a click, or a change of focus.
- **Windows** you didn't ask for, such as an app opening one by itself or a window from another app while you type, pour into the code rain with a toast instead of taking the screen and the keyboard. Windows you open yourself, dialogs of the app you're in, programs started from it, and anything that appears just after a click still come straight to you.
- **Notifications** that arrive while you type wait for the pause, then show as one card. The bell counts them straight away and Super+N shows them at any time. Critical ones never wait, and nothing waits more than 15 minutes.
- **The way back**: after a notification takes you to another app, the bar offers "Back to *app* · Alt+Tab" for a minute.

Settings → Notifications has the switches.

## Requirements

- Linux with **systemd-logind** or **seatd**, and a GPU whose driver supports GBM and EGL. Mesa (Intel, AMD) is what it's tested on.
- A display manager that lists Wayland sessions: Plasma Login, SDDM, GDM, LightDM, greetd or ly. Without one, start it from a text console with `slipstream --session`.
- Recommended, each for one feature: Xwayland, xdg-desktop-portal-wlr (screen sharing), WirePlumber and PipeWire tools (volume), NetworkManager (Wi-Fi), BlueZ (Bluetooth), power-profiles-daemon. `scripts/install-session --check` says what's missing on your system and how to get it.

Your existing desktop stays installed and on the login screen: Slipstream is added beside it.

## Installing

### From a release (Fedora 44, Bazzite, Aurora, Bluefin)

Releases carry ready-built programs for Fedora 44 and the systems built on it. On anything else, build from source.

Download the `.tar.gz` and its `.sha256` from the [releases page](https://github.com/peterwalker78/slipstream-desktop/releases), then:

```sh
sha256sum -c slipstream-0.0.3-fedora44-x86_64.tar.gz.sha256
tar xf slipstream-0.0.3-fedora44-x86_64.tar.gz
cd slipstream-0.0.3-fedora44-x86_64
scripts/install-session --check    # what it will do, and what's missing; writes nothing
scripts/update-session             # puts the programs in ~/.local/bin; no root
scripts/install-session            # adds Slipstream to the login screen; asks for sudo once
```

### From source

On an atomic system (Bazzite, Silverblue, Kinoite), build in a Fedora 44 distrobox called `slipstream`, which `update-session` uses when it's there:

```sh
distrobox create --name slipstream --image registry.fedoraproject.org/fedora-toolbox:44
distrobox enter slipstream -- sudo dnf install -y gcc clang mold rustup pkgconf-pkg-config \
    libxkbcommon-devel wayland-devel systemd-devel libinput-devel mesa-libgbm-devel \
    libseat-devel libdrm-devel mesa-libEGL-devel pixman-devel libdisplay-info-devel gtk4-devel
distrobox enter slipstream -- rustup-init -y
```

Then, from the host, in your clone:

```sh
scripts/update-session     # builds in the box (or with the cargo on your system) and installs
scripts/install-session    # once
```

### Then

Log out, choose **Slipstream** in the login screen's session menu, and log in. Super+/ shows every key.

### How the installer keeps your system safe

- **It checks everything first.** A refusal writes nothing, and it never installs packages.
- **It writes all or nothing.** Its few system files (the session entry, its launcher, the lock screen's PAM service and a manifest) are staged and moved into place together, so a failure part way changes nothing.
- **It leaves other files alone.** It never replaces or removes a file it didn't write.
- **Updates can't leave a broken program.** `update-session` checks the new programs run on your system before swapping them in.

## Updating

Pull or download the new version, run `scripts/update-session`, and log back in. Run `scripts/install-session` again too when the changelog mentions the installer or the session files; it's safe to run any time.

## Uninstalling

From another desktop:

```sh
scripts/install-session --uninstall           # the session, the programs, their units and portal settings
scripts/install-session --uninstall --purge   # and your Slipstream settings, log and saved layout
```

Packages you added for it stay, since other things may use them.

## Keys

| Keys | What they do |
|---|---|
| Super+Space, Super+R | App explorer |
| Super+Return, Super+E, Super+I | Terminal, files, Settings |
| Alt+Tab, Alt+Shift+Tab | Switch windows |
| Alt+F4 | Close the window |
| Super+arrows | Move focus |
| Super+Alt+arrows | Move the tile |
| Super+[ and ], with Shift for height | Resize |
| Super+F | Fill the screen, and back |
| Super+M, Super+Shift+M | Minimise to the code rain, bring back |
| Super+D | Show the desktop, and back |
| Super+Tab | Bullet time |
| Super+T, Super+PgUp, Super+PgDn | Gravity on or off, heavier, lighter |
| Super+1–9, Super+Ctrl+← → | Go to a workspace |
| Super+Shift+1–9, Super+Shift+← → | Move the window to a workspace |
| Super+P, Super+Shift+P | Next screen, move the window there |
| Super+A, Super+N | Quick settings, notifications |
| Print, Super+Shift+S | Screenshot of the screen, of the window |
| Super+L | Lock |
| Super+Shift+Esc | Log out |
| Super+/ | Every key |

Ctrl+Alt+F1–F12 switch virtual terminals as usual. Keys that input methods, screen readers and games rely on are left alone.

## Troubleshooting

- **Back at the login screen straight away:** read `~/.local/state/slipstream/slipstream.log` (the previous session's is `slipstream.log.old`).
- **Super+L doesn't lock:** run `scripts/install-session`, which installs the lock screen's PAM service.
- **Screen sharing offers nothing:** install xdg-desktop-portal-wlr, run `scripts/update-session`, and log in again.
- **Anything else:** `scripts/install-session --check` reports what the system is missing.

## Development

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

Nested, **Alt is the Mod key**, since your desktop keeps Super for itself.

## Principles

- No telemetry, ever.
- No AI in the desktop. Slipstream has no AI features and none are planned.
- Keybindings never take keys that input methods, screen readers, games or apps rely on.
- Works with logind or seatd through libseat.
- Every effect has a reduced-motion version.

AI coding tools are used in writing Slipstream's code. Every change is built and tested in CI.

## Licence

GPL-3.0-or-later. See `LICENSE`. Parts of the compositor started from Smithay's `smallvil` example (MIT). The fonts are under the SIL Open Font Licence; their licences are in `assets/fonts`.
