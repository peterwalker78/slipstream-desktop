# Help

[← back to the README](../README.md)

## When something goes wrong

- **Back at the login screen straight away:** read `~/.local/state/slipstream/slipstream.log` (the previous session's is `slipstream.log.old`).
- **Super+L doesn't lock:** run `scripts/install-session`, which installs the lock screen's PAM service.
- **Screen sharing offers nothing:** you need xdg-desktop-portal-wlr 0.8 or newer. `scripts/install-session --check` says which version is here and offers to install it where a new enough one is packaged; log in again afterwards.
- **No login screen appears at all:** the machine logs someone in automatically. `scripts/install-session --check` says which setting does it and how to change it.
- **Anything else:** `scripts/install-session --check` reports what the system is missing.

## A program is asking to record your screen

Slipstream asks before any program can see your screen. A card names the program and offers
**just this once** or **always allow**; Esc says no. Nothing is recorded until you answer, so a
tool simply waits.

The card can only name a program it can identify, and it will not remember an answer against a
program anyone could replace — so a script in your home directory is refused outright rather than
asked about. That is deliberate: an answer remembered against a file you can rewrite would be a
standing permission for whatever is put there next.

**Settings → Privacy** lists what has been allowed, and the bin beside each one makes it ask
again next time. Nothing is ever added there except by answering the card.

Screen *sharing* — for a call, through the portal — is a separate thing, with its own picker and
the red pill on the bar that stops it.

## Adding apps that come with your desktop

Apps that go with your desktop but are projects of their own are installed and kept up to date as Flatpaks by `scripts/install-extras`, which the installer runs for you at the end (and `update-session` runs on every rebuild). Slipstream ships one, Glimmerwood, described in `extras/glimmerwood.conf`. Add your own in `~/.config/slipstream/extras/`, say `browser.conf`, and a file there with the same name as a shipped one replaces it (`install=no` on its own leaves it out):

```sh
app_id=org.example.Browser
checkout=~/Projects/browser      # built with its own command whenever the checkout has new commits
build=scripts/flatpak
bundle_url=https://example.org/browser.flatpak       # used without a checkout, or if it fails to build before the app is installed
bundle_sha256_url=https://example.org/browser.flatpak.sha256
default_browser=yes              # set once, so choosing another browser later sticks
```

`scripts/install-extras --check` says what it would do and changes nothing; `--help` lists every key.

## The meter on the bar

The bar can show how much of something is used, from any command that prints a small JSON report. Set it in **Settings → Meter**, where **Run it now** shows what the bar would show or why a command doesn't work, or in `~/.config/slipstream/settings.toml`:

```toml
[meter]
command = "quota-report --json"   # run with sh -c
every-secs = 60
```

The report has sections, each with meters:

```json
{"sections": [{"name": "Storage", "detail": "Pro",
               "meters": [{"label": "Daily", "percent": 42, "resets": 1790000000},
                          {"label": "Weekly", "percent": 18}],
               "note": null, "stale": false}]}
```

The bar shows a small gauge for each section: its first meter as the thick bar, with the percentage beside it, and its second as the thin bar underneath. The gauge turns amber at 75% and orange at 90%. Quick settings lists every meter, with the time left until each starts over (`resets` is in seconds since the Unix epoch). `detail`, `note`, `stale` and `resets` are optional. A section marked `stale` is dimmed, and so is everything if the command fails, times out after 30 seconds, or prints something unreadable. The last good report stays on show in the meantime.
