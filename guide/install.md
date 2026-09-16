# Installing Slipstream

Slipstream installs **alongside** the desktop you use now. Nothing about your
current setup changes: you pick Slipstream from the list at the login screen,
and you can go back at any time.

[← back to the README](../README.md)

## What you need

- Linux with **systemd-logind** or **seatd**, and a GPU whose driver supports GBM and EGL. Mesa (Intel, AMD) is what it's tested on.
- A display manager that lists Wayland sessions: Plasma Login, SDDM, GDM, LightDM, greetd or ly. Without one, start it from a text console with `slipstream --session`. If your system logs in automatically, the installer says how to reach the session list.
- Libraries as old as Debian 13's or newer. The download links in anything that changes too often to rely on, so one build runs on every current distribution.
- Recommended, each for one feature: Xwayland, xdg-desktop-portal 1.17+ (dark and light, file dialogs), xdg-desktop-portal-wlr 0.8+ (screen sharing), WirePlumber and PipeWire tools (volume), NetworkManager (Wi-Fi), BlueZ (Bluetooth), power-profiles-daemon. The installer offers to install whichever are missing.

Your existing desktop stays installed and on the login screen: Slipstream is added beside it.

## Installing

One command. It downloads the newest release, checks it against its published checksum, and runs the installer, which says what it will do before writing anything:

```sh
sh -c "$(curl -fsSL https://raw.githubusercontent.com/peterwalker78/slipstream-desktop/main/install.sh)"
```

Then log out and choose **Slipstream** on the login screen. Super+/ shows every key.

To look before you leap, put an option after `--`. `--try` opens Slipstream in a window on the desktop you're using now; `--check` says what installing would do and writes nothing:

```sh
sh -c "$(curl -fsSL https://raw.githubusercontent.com/peterwalker78/slipstream-desktop/main/install.sh)" -- --try
```

Prefer to read what you run? [`install.sh`](../install.sh) is a short shell script that does nothing clever, and the [releases page](https://github.com/peterwalker78/slipstream-desktop/releases) has the same download to unpack yourself:

```sh
sha256sum -c slipstream-*-linux-x86_64.tar.gz.sha256
tar xf slipstream-*-linux-x86_64.tar.gz
cd slipstream-*-linux-x86_64
scripts/try-slipstream             # optional: run it in a window first
scripts/install-session --check    # what it would do, and what's missing; writes nothing
scripts/install-session            # installs it; asks for your password once
```

Every download is built by GitHub from a tagged commit, and comes with a signed record of where it was built. With the GitHub CLI: `gh attestation verify slipstream-*.tar.gz --repo peterwalker78/slipstream-desktop`.

## Which distributions it runs on

| | |
|---|---|
| **Used daily** | Bazzite (Fedora Atomic), on real hardware. Where the rough edges are found. |
| **Checked on every release** | Debian 13, Ubuntu 26.04, Fedora 43 and 44, Arch and openSUSE Tumbleweed: each one installs the download in a container and starts it. Repeated weekly against Debian testing, Ubuntu rolling, Fedora Rawhide, Arch and Tumbleweed, so a change in a distribution shows up here first. |
| **Should work** | Any distribution with Debian 13's libraries or newer, and logind or seatd: Linux Mint 23, Pop!_OS 26.04, Manjaro, EndeavourOS, CachyOS, Nobara, Omarchy, Aurora, Bluefin, Silverblue, Kinoite. Please report it if it doesn't. |
| **Not yet** | Ubuntu 24.04 and what's built on it (Linux Mint 22, Pop!_OS 24.04, Zorin 18): the Settings app needs Pango 1.56, and they have 1.52. Debian 12 and the RHEL family, which are older still. Alpine and other musl systems, NixOS and Guix, which all need a build of their own. Systems with neither systemd nor seatd. ARM machines. |

Screen sharing needs xdg-desktop-portal-wlr 0.8 or newer, which is where the capture protocols Slipstream speaks arrived. Debian 13 and Ubuntu 25.10 package 0.7, so sharing doesn't work there yet; everything else does, and the installer says so plainly.

## What the installer does

- **The programs go in `/usr/local/bin`**, so everyone with an account on the machine can use Slipstream, along with the Settings app's entry, the session's systemd units and the portal settings.
- **A copy in your own `~/.local/bin` comes first**, so a build of your own runs for you and the installed release for everyone else.
- **It checks first, and writes all or nothing.** Its system files are staged and moved into place together, so a failure part way changes nothing. A refusal writes nothing at all.
- **It leaves other files alone**, and never replaces or removes a file it didn't write.
- **Packages are your call.** It names what's missing and offers to install it with your own package manager; on atomic systems, where that rewrites the system image, it prints the command instead of running it. Without a terminal it asks nothing and installs nothing.
- **`scripts/install-session --uninstall`** removes exactly what it wrote, keeping your settings unless you add `--purge`.

## Building it yourself

On Ubuntu 26.04, install the build tools and libraries, then build and install from your clone:

```sh
sudo apt install git build-essential clang mold pkg-config rustup libxkbcommon-dev libwayland-dev \
    libsystemd-dev libudev-dev libinput-dev libgbm-dev libseat-dev libdrm-dev libegl-dev \
    libpixman-1-dev libdisplay-info-dev libgtk-4-dev
git clone https://github.com/peterwalker78/slipstream-desktop.git
cd slipstream-desktop
scripts/update-session     # builds (rustup fetches the Rust version Slipstream needs) and installs
scripts/install-session    # once
```

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

Packaging it, or curious how the downloads are built? `scripts/build-portable` is what CI runs: it builds libdisplay-info and links it in, and `scripts/check-portable` then proves the result asks for nothing newer than Debian 13's glibc and links nothing unusual. For a distribution package, a plain `cargo build --release --locked` against the system's own libraries is the right thing; nothing is vendored.

## Updating

Run the install command again: it fetches the newest release and installs it over the old one, which is safe to do at any time. From a clone, `scripts/update-session` builds and installs your own copy instead. Either way, the new version starts at your next login.

Slipstream never checks for updates by itself, and nothing here contacts the internet unless you ask it to.

## Uninstalling

From another desktop:

```sh
scripts/install-session --uninstall           # the session, the programs, their units and portal settings
scripts/install-session --uninstall --purge   # and your Slipstream settings, log and saved layout
```

Packages you added for it stay, since other things may use them.
