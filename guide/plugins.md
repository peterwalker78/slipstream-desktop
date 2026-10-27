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
- `read` names folders outside its own that it reads, such as `read = ["$XDG_CACHE_HOME/weather"]`. They are handed to the program as `SLIPSTREAM_PLUGIN_READ_0`, `SLIPSTREAM_PLUGIN_READ_1` and so on. A folder starts with `$HOME`, one of the `$XDG_…_HOME` folders, or `/`. Folders that hold the sockets programs use to reach each other, or the system's live state, are never given, nor any folder holding one: `$XDG_RUNTIME_DIR`, `/run`, `/tmp`, `/var/tmp`, `/proc`, `/sys`, `/dev`, `~/.ssh`, `~/.gnupg`, `~/.var`, `~/.local/share/containers`, and Flatpak's helpers in `~/.cache`, and so `/`, `/var` and your home folder too. Nor are the plugins' own folders, where plugins write. A link is followed before that's checked. A plugin that asks for one can't be turned on, and Settings says why. Each folder is also looked through before every start of the program: one holding a socket, a named pipe or a device of an app's own, or more than 4096 files and folders to check, stops the plugin, and its card says why. Something put there after its program has started is only found at its next start, so a plugin that neither opens windows nor goes online can't open local sockets at all.
- `write` names folders it changes, as `read` does, for a plugin that needs `write`: `write = ["$HOME/Notes"]`. They are handed to the program as `SLIPSTREAM_PLUGIN_WRITE_0` and on. The same folders are refused as for `read`, and so are those where programs keep their settings and what they start: `~/.config`, `~/.local` and any other hidden folder in your home folder, and `~/bin`. A folder in your home folder that isn't there yet is made when you switch the plugin on, and Settings says so beforehand.
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
- `sections`: named groups of gauges, up to four. Each gauge has a `label` and a `percent`, and can have `resets`, when it starts over in seconds since the Unix epoch, which shows as the time left, and `window`, how long it lasts in seconds. With both, the part of the window gone by is shaded in the gauge, in the focus ring's colour, and anything used beyond it shows in the bullet-time colour. A gauge without a `percent` shows only that, and the time left, as a timer's does. A section can have a `detail` beside its name, a `note` under it, and `stale` to dim it.
- `buttons`: up to three. Pressing one runs the plugin's program again with `press` and the button's `id` after its own words: `./weather.sh press refresh`. It waits for a run already under way to finish, and anything it leaves running is stopped when it exits. A button can instead have Slipstream do one thing for it, when you press it, and then the program isn't run: open a web address in your browser, with `"open": "https://…"`, for a plugin that needs `open`; copy text, with `"copy": "…"`, for one that needs `clipboard`; or control what's playing, with the id `media:play-pause`, `media:next` or `media:previous`, for one that needs `media`.
- A card with sections has a cog, which slides it across to a switch for each one, to hide the ones you don't want.
- `glance`: what the chip shows when this plugin is the one chosen for it. Without it, the chip shows the first section's first two gauges and the larger of their percentages.
- `attention`: turns the chip amber. A gauge at 75% or more does the same.
- `notify`: a notification to send as this card arrives, as `{"title": "Tea's ready", "body": "It's been four minutes"}`, for a plugin that needs `notify`. It comes from the plugin by name, marked as a plugin's so it can't pass for an app's, a few a minute at most. A program that runs every so often should only include it on the run that has something to say.
- `sound`: a sound to play as this card arrives, by its name in the sound theme, as `"sound": "complete"`, for a plugin that needs `sound`. With a notification it is that notification's own sound; without one it plays by itself. Either way it follows your notification sound settings, do not disturb included.

The card is only ever shown. Nothing in it is run, and a button can only run the plugin's own program, with its id as a word of its own, or have Slipstream do one of the things above that the plugin needs. A web address has to start `https://`, and anything else is dropped from the card.

## Beyond its card

A plugin can show up in three more places, each drawn by Slipstream in its own style from what the plugin says, and each doing what a button does when used: running its program with `press` and an id. A plugin never draws on the desktop and never takes keys itself. **Settings → Plugins** says on its card where it shows.

### A tile in quick settings

A plugin whose `plugin.toml` says `tile = true` gets a tile in quick settings, under Slipstream's own, from the `tile` on its card:

```toml
tile = true
```

```json
{"tile": {"id": "awake", "label": "Keep awake", "icon": "sun", "on": true}}
```

- `id` is what pressing the tile sends, as a button's: `press awake`. `label` is its name, and `on` lights it in the accent colour. Under the label is the plugin's name.
- `icon` is one of `bell`, `eye`, `file`, `folder`, `gauge`, `keyboard`, `lock`, `mic`, `moon`, `pause`, `play`, `power`, `run`, `search`, `sun`, `tick` and `volume`; any other, or none, gets a plain one.
- The tile changes when the card does, so print a new card with the new `on` once the press has done its work. Up to four plugins' tiles show, while their plugins are on.

### Answers in the explorer

A plugin whose `plugin.toml` names a `keyword` answers the searches that start with it, and only those: it never sees anything else typed in the explorer.

```toml
keyword = "="
```

- A keyword is one to three symbols, such as `=`, `?` or `!!`, so that no ordinary search starts with one. It can't start with `:`, `/`, `~` or `.`, which Slipstream's own emoji and file searches use.
- Each search runs the plugin's program with `search` and the words typed after the keyword, as one word: `./sums.sh search "2+2"`. That is the same for a program that runs every so often and one that keeps running: the search is a run of its own, beside the program's usual runs, so a program that keeps running needn't read anything for it. A search with nothing after the keyword isn't run.
- The program prints its answers as one line of JSON, and exits:

```json
{"results": [{"id": "four", "label": "4", "detail": "2 + 2", "icon": "tick"}]}
```

- Up to six answers show, as rows under the plugin's name. `detail` is a few words at the end of the row, and `icon` is one of the tile's pictures. Enter, or a click, on one sends its `id` as a button's: `press four`.
- Typing on while a search runs stops it and asks again, and a search still running after 2 seconds is stopped, so a slow plugin only has nothing to show. Its last answers stay up, dimmed, until new ones come.

### Keys for its actions

A plugin can list actions, each with an `id` and a `label`, up to eight:

```toml
[[actions]]
id = "start"
label = "Start the timer"
```

They have no keys until you give them some, in **Settings → Shortcuts**, under **Plugins' actions**, while the plugin is on. Keys Slipstream or an app already uses, or that the shortcut style keeps for typing aids, screen readers and games, are refused there with why, as for your own shortcuts. Pressing the keys sends the action's id as a button's: `press start`. They are kept in the settings file as `[[keys.plugin]]`, with `keys`, `plugin` and `action`.

## What it can reach

A plugin runs in a sandbox, made with [bubblewrap](https://github.com/containers/bubblewrap), the tool Flatpak uses. In it, the program sees the system's programs, libraries and settings, read-only, and of your own files only its folder, read-only, its own folder for what it keeps, and the folders its `read` names, read-only. Your home folder is empty and `/tmp` is its own. It can't see or signal any other program, and it has no network, no display and no session bus. Its environment is only `HOME`, `USER`, `LOGNAME`, `PATH`, `SHELL`, the language and time zone settings, the `$XDG_…_HOME` folders, and the proxy settings for one that goes online, so the SSH and GPG agents are left out too. On top of that, a filter refuses it the kernel's own ways round a sandbox, as Flatpak's does: keyrings, namespaces and mounts, typing into a terminal, loading kernel modules, `bpf`, `ptrace`, `io_uring`, and sockets other than local, internet and netlink.

`needs` adds to that. **Settings → Plugins** lists what each plugin can do in everyday words before you turn it on, and sums it up beside its name: **Safe** when it can't see or send anything of yours that matters, **Worth a thought** when it can see something private or send what it sees online, and **Full access** for one that needs `everything`:

- `network`: the internet and your local network. It shares the computer's own network, so it can also reach any program listening for other programs there.
- `windows`: the display, to open windows of its own. Its windows are treated as a Flatpak app's are: it can't type as your keyboard, read the clipboard while you're elsewhere, or draw over the desktop.
- `notify`: sending you notifications, from its card.
- `notifications`: seeing the notifications you get. Each one is a line of JSON on the program's standard input, `{"notification": {"app": "Mail", "title": "…", "body": "…", "time": 1793121600}}`, the time in seconds since the Unix epoch. A program that keeps running reads them as they come; one that runs every so often reads those since its last run, and then the end of its input. Only notifications kept in the notification centre are passed on, and never the plugin's own.
- `write`: changing the files in the folders its `write` names, as Flatpak's own folder permissions do. Each is opened where it really is before the sandbox is made, and the sandbox is made from what was opened, so a folder swapped for a link once it was checked doesn't run the plugin. Worth a thought.
- `system`: reading `/sys`, read-only, for the battery, temperatures, fans and devices. Your processor, memory and load are in the sandbox's own `/proc` without it. Safe.
- `media`: seeing what's playing, and its card's buttons controlling it. Slipstream looks at the players every couple of seconds and, when that changes, writes `{"media": {"player": "spotify", "title": "…", "artist": "…", "playing": true}}` on the program's standard input, or `{"media": null}` once nothing plays. The plugin never reaches the players itself. Safe.
- `sound`: playing a sound from the sound theme with its card, played by Slipstream. It can't record anything, as it could with your sound server. Safe.
- `open`: its card's buttons opening web addresses in your browser, only when you press one. Safe.
- `open-windows`: seeing which windows you have open, their apps and titles, and which has the keyboard, as `{"windows": [{"app": "org.example.Mail", "title": "…", "focused": true}]}` on its standard input whenever that changes, a second apart at most, and never while the screen is locked. Titles can hold email subjects and document names. Worth a thought.
- `clipboard`: seeing the text you copy, as `{"clipboard": {"text": "…"}}` on its standard input, up to its first thousand characters, and its card's buttons copying text. Pictures aren't passed on, nor anything a password manager marks as secret, nor anything copied while the screen is locked, but a password you copy yourself is. Worth a thought.
- `everything`: no sandbox at all. The program runs as you, with your whole session, the session bus included, and Settings says so in amber, with what that allows: your files, saved passwords and SSH keys included; your screen, what you type and copy, and your other windows; and starting itself at login, even after it's switched off. It can't act as root by itself, but could catch your password as you type it, or use a terminal you've just typed it in. It is one item rather than a sandbox with the bus let in, because anything that reaches the session bus can ask your session to start any program outside the sandbox.

The lines on a program's standard input reach one that keeps running as they come, and one that runs every so often at its next run; of `media` and `open-windows`, only the latest waiting is kept.

A plugin that asks for nothing gets the tightest sandbox. Without bubblewrap 0.8 or newer, a plugin that runs in the sandbox stays off and says why; the installer offers it with the other packages it suggests. A sandboxed plugin also gets a login keyring of its own, empty, so the keys your login keeps there stay out of its reach.

## Where it runs from

Turning a plugin on copies its folder to `~/.local/state/slipstream/plugins/<id>/`, read-only, and the program always runs from that copy, never from the folder itself. So a plugin can't write into its own folder: `SLIPSTREAM_PLUGIN_STATE` names a folder of its own for anything it keeps. When anything in its folder changes, a file or whether a file is a program, the plugin waits, showing why, until you turn it on again. That stops an update slipping in without your say. Programs a plugin offers outside its card can be linked to `~/.local/state/slipstream/plugins/<id>/current/`, which always leads to the copy you turned on, and leads nowhere while it is off. Those run as you, outside the sandbox, whenever you or your tools run them, so they can do all that `everything` allows, and Settings lists them under the plugin and says so. Removing a plugin in Settings removes its copies and what it kept. Settings keeps what a plugin asked for when you turned it on, so a held one says what its update means for you: whether it wants anything it didn't before, and what, in everyday words, and whether Slipstream's own installer put it there from the plugin's published release, with the download matching the release's checksum. It doesn't protect against something already running as you, which could change the settings file just as easily.

An update that asks for more counts anything newly asked for: a need, a folder to read or change, another program, a tile, a keyword or actions.

### Plugins that come with Slipstream

Slipstream's own plugins, such as Focus, are installed with its programs, in `share/slipstream/bundled/` beside the `bin` folder Slipstream runs from: `/usr/local/share/slipstream/bundled/` for the installed download, `~/.local/share/slipstream/bundled/` for `scripts/update-session`'s build. They start off like any other and Settings says they come with Slipstream. Whoever can change the files there can change Slipstream itself, so asking you again about an update that asks for nothing new would protect nothing: once switched on, one stays on through an update that asks for nothing more than it did when you switched it on. One that asks for more waits for you, as any other plugin does. Plugins you add yourself, and extras such as agent-notify, still wait after every update.

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

## A quick one

Any language will do: what counts is the line of JSON. A tea timer in shell, run every half minute, with a button whose press leaves the time the tea is ready in the plugin's own folder:

```toml
name = "Tea"
exec = ["./tea.sh"]
every-secs = 30
needs = ["notify"]
```

```sh
#!/bin/sh
ready=$SLIPSTREAM_PLUGIN_STATE/ready
now=$(date +%s)
if [ "$1" = press ]; then
    echo $((now + 240)) >"$ready"
elif ! [ -f "$ready" ]; then
    echo '{"lines": ["No tea brewing"], "buttons": [{"id": "start", "label": "Start"}]}'
elif [ "$(cat "$ready")" -le "$now" ]; then
    rm "$ready"
    echo '{"lines": ["Tea is ready"], "notify": {"title": "Tea is ready"}}'
else
    echo "{\"lines\": [\"Tea in $((($(cat "$ready") - now + 59) / 60)) min\"]}"
fi
```

## Focus, a worked example

[Focus](https://github.com/peterwalker78/slipstream-desktop/tree/main/plugins/focus) comes with Slipstream: a timer for spells of work, and a log of what you spent them on. It is written in Rust, and asks only for what it uses:

```toml
name = "Focus"
description = "A timer for spells of work, and a log of what you spent them on"
exec = ["./focus"]
needs = ["notify", "sound", "media", "open-windows", "write"]
write = ["$HOME/Documents/Focus"]
tile = true
keyword = "!"

[[actions]]
id = "toggle"
label = "Start or stop a focus session"
```

- Its program keeps running, reading which app has the keyboard and what's playing from its standard input, and printing a new card whenever what it shows changes: a gauge of the session's time with no `percent`, Stop and +5 min, or 25 and 50 minutes while idle, and the minutes left as its headline.
- A press runs beside it, so a press only leaves its id in the plugin's own folder for the running program to act on. Only one program ever changes the timer.
- At the end of a session its card carries a notification, "Time for a break", saying how long it was and which two apps had most of it, and the sound `complete`; a break follows. In a break, something playing gets a Pause button, `media:play-pause`, and a Play button after it if you paused it. A plugin can't press it for you, so it never pauses anything by itself.
- Each session finished adds a line to `log.md` in `~/Documents/Focus`: the date, when it started, how long it ran and the apps most in use. What it keeps between runs, today's total and a session under way, is in `SLIPSTREAM_PLUGIN_STATE`.
- `!45` in the explorer offers "Focus for 45 minutes" and "Take a 45 minute break", its tile in quick settings starts or stops a 25-minute session, and so does its action, on keys you give it.

## A whole plugin, packaged

[agent-notify](https://github.com/peterwalker78/agent-notify) is a complete plugin with releases of its own. Its card program only reads the folders its `read` names, and it leaves anything that needs the network or the session bus to programs that already run with your access. Each release is the plugin's folder packed as a `.tar.gz`, with its SHA-256 and signature (`.sig`) beside it, which `scripts/install-extras` installs from a file in `extras/` naming `plugin=`, `bundle_url=`, `bundle_sha256_url=` and `bundle_signer=`.

## Trying one out

Run the program yourself and check it prints one line of JSON. `~/.local/state/slipstream/slipstream.log` says why a plugin has nothing to show, along with anything its program writes to standard error.
