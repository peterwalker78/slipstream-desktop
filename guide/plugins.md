# Writing a plugin

A plugin is a little program that adds a card to the panel beside the clock. It can be written in anything: a shell script, Python, a compiled program. Slipstream runs it and draws what it prints, in Slipstream's own style.

## The folder

A plugin is a folder holding a `plugin.toml` and whatever its program needs:

```
weather/
├── plugin.toml
└── weather.sh
```

```toml
name = "Weather"
description = "Today's weather where you are"
exec = ["./weather.sh"]
every-secs = 600
needs = ["network"]
```

- `name` and `exec` are needed; `description` shows in **Settings → Plugins**.
- `exec` is the program and the words it is run with, as a list. It is run as written, with no shell. A program with a `/` in it is found from the plugin's folder, and can't start with `/` or go up with `..`; one without is looked for on `PATH`.
- `every-secs` runs the program that often, and again after one of its buttons is pressed. Leave it out for a program that keeps running and prints a new card whenever something changes.
- `read` names folders outside its own that it reads, such as `read = ["$XDG_CACHE_HOME/weather"]`. They are handed to the program as `SLIPSTREAM_PLUGIN_READ_0`, `SLIPSTREAM_PLUGIN_READ_1` and so on. A folder starts with `$HOME`, one of the `$XDG_…_HOME` folders, or `/`. Folders that hold the sockets programs use to reach each other, or the system's live state, are never given, nor any folder holding one: `$XDG_RUNTIME_DIR`, `/run`, `/tmp`, `/var/tmp`, `/proc`, `/sys`, `/dev`, `~/.ssh` and `~/.gnupg`, and so `/`, `/var` and your home folder too. Nor are the plugins' own folders, where plugins write. A link is followed before that's checked. A plugin that asks for one can't be turned on, and Settings says why. Each folder is also looked through before every start of the program: one holding a socket, a named pipe or a device of an app's own, or more than 4096 files and folders to check, stops the plugin, and its card says why. Something put there after its program has started is only found at its next start.
- `needs` lists what else it reaches: see [What it can reach](#what-it-can-reach). Leave it out for a plugin that needs nothing more.

The folder's name is the plugin's id: letters, digits, `.`, `-` and `_`. A folder can hold only files and folders, no links. One with more than 4096 of them, folders more than 16 deep or more than 64 MB is listed as too big to check, and can't be turned on.

Your own plugins go in `~/.local/share/slipstream/plugins/`, and plugins installed for everyone in `/usr/share/slipstream/plugins/`. **Settings → Plugins** adds one from a folder or an `https://` git address, and a plugin starts off until you turn it on there.

## The card

Each time it runs, or each time something changes for a program that keeps running, the program prints a card as one line of JSON:

```json
{"lines": ["Light rain, 12°C"],
 "sections": [{"name": "Today", "meters": [{"label": "Rain", "percent": 80}]}],
 "buttons": [{"id": "refresh", "label": "Refresh"}],
 "glance": "12°C",
 "attention": false}
```

Every field is optional.

- `lines`: up to six lines of text.
- `sections`: named groups of gauges, up to four. Each gauge has a `label` and a `percent`, and can have `resets`, when it starts over in seconds since the Unix epoch, which shows as the time left, and `window`, how long it lasts in seconds. With both, the part of the window gone by is shaded in the gauge, in the focus ring's colour, and anything used beyond it shows in the bullet-time colour. A section can have a `detail` beside its name, a `note` under it, and `stale` to dim it.
- `buttons`: up to three. Pressing one runs the plugin's program again with `press` and the button's `id` after its own words: `./weather.sh press refresh`. It waits for a run already under way to finish, and anything it leaves running is stopped when it exits.
- A card with sections has a cog, which slides it across to a switch for each one, to hide the ones you don't want.
- `glance`: what the chip shows when this plugin is the one chosen for it. Without it, the chip shows the first section's first two gauges and the larger of their percentages.
- `attention`: turns the chip amber. A gauge at 75% or more does the same.
- `notify`: a notification to send as this card arrives, as `{"title": "Tea's ready", "body": "It's been four minutes"}`, for a plugin that needs `notify`. It comes from the plugin by name, marked as a plugin's so it can't pass for an app's, a few a minute at most. A program that runs every so often should only include it on the run that has something to say.

The card is only ever shown. Nothing in it is run, and a button can only run the plugin's own program, with its id as a word of its own.

## What it can reach

A plugin runs in a sandbox, made with [bubblewrap](https://github.com/containers/bubblewrap), the tool Flatpak uses. In it, the program sees the system's programs, libraries and settings, read-only, and of your own files only its folder, read-only, its own folder for what it keeps, and the folders its `read` names, read-only. Your home folder is empty and `/tmp` is its own. It can't see or signal any other program, and it has no network, no display and no session bus. Its environment is only `HOME`, `USER`, `LOGNAME`, `PATH`, `SHELL`, the language and time zone settings, the `$XDG_…_HOME` folders, and the proxy settings for one that goes online, so the SSH and GPG agents are left out too.

`needs` adds to that, and **Settings → Plugins** says in plain words what each plugin can reach before you turn it on:

- `network`: the internet and your local network. It shares the computer's own network, so it can also reach any program listening for other programs there.
- `windows`: the display, to open windows of its own. Its windows are treated as a Flatpak app's are: it can't type as your keyboard, read the clipboard while you're elsewhere, or draw over the desktop.
- `notify`: sending you notifications, from its card.
- `notifications`: seeing the notifications you get. Each one is a line of JSON on the program's standard input, `{"notification": {"app": "Mail", "title": "…", "body": "…", "time": 1793121600}}`, the time in seconds since the Unix epoch. A program that keeps running reads them as they come; one that runs every so often reads those since its last run, and then the end of its input. Only notifications kept in the notification centre are passed on, and never the plugin's own.
- `everything`: no sandbox at all. The program runs as you, with your whole session, the session bus included, and Settings says so in amber. It is one item rather than a sandbox with the bus let in, because anything that reaches the session bus can ask your session to start any program outside the sandbox.

A plugin that asks for nothing gets the tightest sandbox. Without bubblewrap 0.8 or newer, a plugin that runs in the sandbox stays off and says why; the installer offers it with the other packages it suggests. A sandboxed plugin also gets a login keyring of its own, empty, so the keys your login keeps there stay out of its reach.

## Where it runs from

Turning a plugin on copies its folder to `~/.local/state/slipstream/plugins/<id>/`, read-only, and the program always runs from that copy, never from the folder itself. So a plugin can't write into its own folder: `SLIPSTREAM_PLUGIN_STATE` names a folder of its own for anything it keeps. When anything in its folder changes, a file or whether a file is a program, the plugin waits, showing why, until you turn it on again. That stops an update slipping in without your say. Programs a plugin offers outside its card can be linked to `~/.local/state/slipstream/plugins/<id>/current/`, which always leads to the copy you turned on, and leads nowhere while it is off. Those run as you, outside the sandbox, whenever you or your tools run them, and Settings lists them under the plugin. Removing a plugin in Settings removes its copies and what it kept. Settings keeps what a plugin asked for when you turned it on, so a held one says what its update means for you: whether it wants anything it didn't before, and what, in everyday words, and whether Slipstream's own installer put it there from the plugin's published release, with the download matching the release's checksum. It doesn't protect against something already running as you, which could change the settings file just as easily.

A program that fails, takes longer than 30 seconds or prints something that isn't a card leaves its last card on show, dimmed and saying why. One that keeps running and exits is started again, waiting longer each time it keeps exiting straight away.

## A meter

A meter is a plugin whose card has only `sections`:

```toml
name = "Disk space"
exec = ["./disk.sh"]
every-secs = 60
```

```sh
#!/bin/sh
used=$(df --output=pcent / | tail -n 1 | tr -dc 0-9)
printf '{"sections": [{"name": "Disk", "meters": [{"label": "/", "percent": %s}]}]}\n' "$used"
```

A `[meter]` command in a settings file from before plugins becomes a plugin like this, called Meter, the first time Slipstream or the Settings app reads that file, in `meter-2` or the next free name if a `meter` plugin is there already. It waits until you turn it on, and since the command is your own and ran with everything you have, it needs `everything` and still does.

## A whole plugin, packaged

[agent-notify](https://github.com/peterwalker78/agent-notify) is a complete plugin with releases of its own. Its card program only reads the folders its `read` names, and it leaves anything that needs the network or the session bus to programs that already run with your access. Each release is the plugin's folder packed as a `.tar.gz`, with its SHA-256 and signature (`.sig`) beside it, which `scripts/install-extras` installs from a file in `extras/` naming `plugin=`, `bundle_url=`, `bundle_sha256_url=` and `bundle_signer=`.

## Trying one out

Run the program yourself and check it prints one line of JSON. `~/.local/state/slipstream/slipstream.log` says why a plugin has nothing to show, along with anything its program writes to standard error.
