# Help

[← back to the README](../README.md)

## When something goes wrong

The installer's check finds most problems, and writes nothing. If you installed with the one command, run it again with `-- --check` on the end; from a download or a clone, run `scripts/install-session --check`.

- **Back at the login screen straight away:** read `~/.local/state/slipstream/slipstream.log` (the previous session's is `slipstream.log.old`).
- **Super+L doesn't lock:** install again (the one command, or `scripts/install-session`), which puts the lock screen's PAM service in place.
- **Screen sharing offers nothing:** you need xdg-desktop-portal-wlr 0.8 or newer. The check says which version is here, and installing again offers to install a new enough one where it's packaged; log in again afterwards.
- **No login screen appears at all:** the machine logs someone in automatically. The check says which setting does it and how to change it.
- **Anything else:** the check reports what the system is missing.

## A program is asking to record your screen

Slipstream asks before any program can see your screen. A card names the program and offers **just this once** or **always allow**; Esc says no. Nothing is recorded until you answer, so a tool simply waits.

The card can only name a program it can identify, and it will not remember an answer against a program anyone could replace, so a script in your home directory is refused outright rather than asked about. That is deliberate: an answer remembered against a file you can rewrite would be a standing permission for whatever is put there next.

**Settings → Privacy** lists what has been allowed, and the bin beside each one makes it ask again next time. Nothing is ever added there except by answering the card.

Screen *sharing*, for a call through the portal, is a separate thing, with its own picker and the red pill on the bar that stops it.

## Adding apps that come with your desktop

Apps that go with your desktop but are projects of their own are installed as Flatpaks by `scripts/install-extras`. The installer runs it at the end and asks about each one first; a no is remembered, and `--yes` answers yes. Running the install command again brings them up to date, and `update-session` does the same on every rebuild. Slipstream ships two, the Glimmerwood browser and the Westering stargazing game, described in `extras/glimmerwood.conf` and `extras/westering.conf`. Add your own in `~/.config/slipstream/extras/`, say `browser.conf`, and a file there with the same name as a shipped one replaces it (`install=no` on its own leaves it out):

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

## Bringing an AI agent's session back

Slipstream has no AI in it. This is for people who run an AI coding agent of their own in a terminal: when your layout is reopened at login (**Settings → Session → Remember the layout**), a terminal that had an agent's session in it can come back in the same place, in the same folder, with that session open again.

**On its own it does nothing.** Slipstream doesn't know any agent and doesn't look for one. It works only if you have installed an agent, and that agent leaves a note saying how to reopen its session: a file named for the agent's process ID in `$XDG_RUNTIME_DIR/slipstream/resume/`, holding one line, the command to run. Most agents can run a command of yours when a session starts, which is where to write the note from:

```sh
echo "my-agent --resume $SESSION_ID" > "$XDG_RUNTIME_DIR/slipstream/resume/$AGENT_PID"
```

At the next login that terminal is started in the folder its shell was in and runs the line. It runs in the shell you typed the agent into, started the way a shell at a prompt is, and with the search path the agent had, so it finds the same tools as before. When the agent exits you are left at a shell, as if you had started it by hand.

- **Only the note is ever run.** Slipstream never works a command out from what happens to be running, and a terminal with no note comes back as it always has: a new shell.
- **One window, one shell.** A terminal with tabs, or one that keeps all its windows in a single process, can't say which shell belongs to which window, so it comes back as an ordinary terminal.
- **The conversation, not the work in progress.** What comes back is whatever the agent's own command reopens. Anything it was in the middle of running is not carried on.
- **Where it is kept.** The command, the folder, the shell and the agent's search path go in the layout record, `~/.local/state/slipstream/session.toml`, which is deleted when you turn **Remember the layout** off. Nothing else of the agent's environment is kept.

## Seeing an AI agent at work from across the room

Slipstream has no AI in it. This is for people who set an AI coding agent of their own going and step away: once the desktop has faded to the wallpaper, each agent at work has a small oscilloscope trace at the top right, with the name of the folder it is working in. A moving cyan wave is an agent still working. A flat amber line is one that has stopped. There is one trace for each agent, however many are running.

**On its own it does nothing.** Slipstream doesn't know any agent and doesn't look for one. It works only if you have installed an agent, and that agent says when it is working with a note: an empty file named for the agent's process ID in `$XDG_RUNTIME_DIR/slipstream/working/`, made when it starts work and removed when it stops. Most agents can run a command of yours at both moments:

```sh
touch "$XDG_RUNTIME_DIR/slipstream/working/$AGENT_PID"    # a task starts
rm -f "$XDG_RUNTIME_DIR/slipstream/working/$AGENT_PID"    # it ends, or the agent is waiting for you
```

- **Only while the desktop is faded.** Nothing is drawn over your windows; the traces come in after the wallpaper has taken the screen and go as the desktop comes back.
- **Only agents that have been working.** An agent that was already idle when you stepped away has no trace. One that stops while you're away keeps its flat line until you touch a key or the mouse.
- **The agent itself, not its helpers.** A note is named for one process, so the helpers an agent starts for a task don't each get a trace unless they leave notes of their own.
- **Reduced motion** keeps the picture and drops the movement: a standing wave for an agent at work, a flat line for one that has stopped.
- **Nothing is read from the note,** and a note left behind by an agent that has gone counts as stopped.
