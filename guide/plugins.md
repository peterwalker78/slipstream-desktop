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
```

- `name` and `exec` are needed; `description` shows in **Settings → Plugins**.
- `exec` is the program and the words it is run with, as a list. It is run as written, with no shell. A program with a `/` in it is found from the plugin's folder; one without is looked for on `PATH`.
- `every-secs` runs the program that often, and again after one of its buttons is pressed. Leave it out for a program that keeps running and prints a new card whenever something changes.
- `read` names folders outside its own that it reads, such as `read = ["$XDG_CACHE_HOME/weather"]`. They are handed to the program as `SLIPSTREAM_PLUGIN_READ_0`, `SLIPSTREAM_PLUGIN_READ_1` and so on. A folder starts with `$HOME`, `$XDG_RUNTIME_DIR`, one of the `$XDG_…_HOME` folders, or `/`.

The folder's name is the plugin's id: letters, digits, `.`, `-` and `_`. A folder can't hold links or more than 64 MB.

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
- `sections`: named groups of gauges, up to four. Each gauge has a `label` and a `percent`, and can have `resets`, when it starts over in seconds since the Unix epoch, which shows as the time left. A section can have a `detail` beside its name, a `note` under it, and `stale` to dim it.
- `buttons`: up to three. Pressing one runs the plugin's program again with `press` and the button's `id` after its own words: `./weather.sh press refresh`.
- `glance`: what the chip shows when this plugin is the one chosen for it. Without it, the chip shows the first section's first two gauges and the larger of their percentages.
- `attention`: turns the chip amber. A gauge at 75% or more does the same.

The card is only ever shown. Nothing in it is run, and a button can only run the plugin's own program, with its id as a word of its own.

## What the program is given

The program runs as you, from its own folder. It doesn't get your session's environment: only `HOME`, `USER`, `LOGNAME`, `PATH`, `SHELL`, the language and time zone settings, the `$XDG_…_HOME` folders and any proxy settings. The session bus, the display and the SSH and GPG agents are left out.

`SLIPSTREAM_PLUGIN_STATE` names a folder of its own, under `~/.local/state/slipstream/plugins/`, for anything it keeps. Its own folder is its code: when anything in it changes, the plugin waits, showing why, until you turn it on again. That stops an update slipping in without your say. It doesn't protect against something already running as you, which could change the settings file just as easily.

A program that fails, takes longer than 30 seconds or prints something that isn't a card leaves its last card on show, dimmed and saying why. One that keeps running and exits is started again, waiting longer each time it keeps exiting straight away.

## The meter

A `[meter]` command in the settings file shows as a plugin called Meter, with its headline on the chip unless you choose another. It runs with `sh -c` and your session's environment, as it always has. Its report is a card with only `sections`.

## Trying one out

Run the program yourself and check it prints one line of JSON. `~/.local/state/slipstream/slipstream.log` says why a plugin has nothing to show, along with anything its program writes to standard error.
