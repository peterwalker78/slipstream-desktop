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

Apps and plugins that go with your desktop but are projects of their own are installed by `scripts/install-extras`: apps as Flatpaks, and plugins into `~/.local/share/slipstream/plugins/`. The installer runs it at the end and asks about each one first; a no is remembered, and `--yes` answers yes. Running the install command again brings them up to date, and `update-session` does the same on every rebuild. Slipstream ships three, the Glimmerwood browser and the Westering stargazing game, described in `extras/glimmerwood.conf` and `extras/westering.conf`, and the agent-notify plugin, in `extras/agent-notify.conf`. Add your own in `~/.config/slipstream/extras/`, say `browser.conf`, and a file there with the same name as a shipped one replaces it (`install=no` on its own leaves it out):

```sh
app_id=org.example.Browser
checkout=~/Projects/browser      # built with its own command whenever the checkout has new commits
build=scripts/flatpak
bundle_url=https://example.org/browser.flatpak       # used without a checkout, or if it fails to build before the app is installed
bundle_sha256_url=https://example.org/browser.flatpak.sha256
default_browser=yes              # set once, so choosing another browser later sticks
```

A plugin names its folder with `plugin=` instead, and its release is that folder packed as a `.tar.gz`. It's checked against its SHA-256, refused if it holds links or no `plugin.toml`, and installed off, like every plugin, until you turn it on in **Settings → Plugins**:

```sh
plugin=weather
bundle_url=https://example.org/weather.tar.gz
bundle_sha256_url=https://example.org/weather.tar.gz.sha256
bundle_signer=ssh-ed25519 AAAA…  # then the release must carry a .sig made with this key
commands=bin/weather             # linked into ~/.local/bin, to the copy you turned on
```

Downloads are over https only. With `bundle_signer=`, a bundle is installed only when the `.sig` published beside it shows it was signed with that key (`ssh-keygen -Y sign -n slipstream-extra`), which needs `ssh-keygen` from OpenSSH. A plugin's commands run the copy you approved in Settings, so they lead nowhere until you turn it on there.

`scripts/install-extras --check` says what it would do and changes nothing; `--help` lists every key.

## Apps running in the background

The notification centre (Super+N) lists the apps running without a window up, under **Background Apps**: an app whose windows you closed while it carried on, and an app that keeps an icon in the system tray, such as a chat or sync client, even one that started straight to its icon and never opened a window. An app that does both shows once.

- **Click** one, or choose it and press Enter, to bring it back. An app with a tray icon is asked to show itself, so one that was only hidden isn't started a second time. An app whose icon only has a menu shows its menu instead.
- **Right-click** one, or press the Menu key or Shift+F10 on it, for its tray menu, drawn by Slipstream. The arrows move, Enter or Space chooses, → and ← open and close a submenu, and an entry's underlined letter chooses it straight away. Esc puts the menu away.
- **Middle-click** for the app's second action, where it has one.

An amber dot on an app's icon, and a ring that slowly brightens and fades around it, means the app wants your attention: a message waiting, say. With reduced motion the ring holds still. A tray icon that says it has nothing to tell you isn't listed unless its app is already there.

What an app puts in its menu is only ever shown. Choosing an entry tells that app which entry you chose, and nothing else: Slipstream runs nothing from a menu, an icon or a label.

Slipstream serves the tray itself, through the StatusNotifierItem standard most Linux apps use. If another program on your desktop already does, Slipstream shows the icons that program collects.

## Plugins beside the clock

Plugins are little apps that add a card to the panel beside the clock: the weather, a timer, your plan's limits, anything a program can report. The chip right of the date has a square for each plugin, lit for each one that's on, and the headline of the one you choose. It turns amber when a plugin wants your attention. Click it, or press Super+Y, for the panel: Tab moves between the cards' cogs and buttons, Enter presses, and Esc closes it. A card's cog chooses which of its sections show. Where a gauge knows how long its allowance lasts, the part of that time gone by is shaded in it, and anything used faster than that shows in your second colour.

Add plugins in **Settings → Plugins**, from a folder or an `https://` git address. A new plugin starts off, so you can read its files before turning it on, and nothing of it runs until you do. Each plugin is a program of its own and runs as you, so only add plugins you trust. One that changes after you turned it on waits until you turn it on again.

To write one, see [Writing a plugin](plugins.md).

## Bringing an AI agent's session back

Slipstream has no AI in it. This is for people who run an AI coding agent of their own in a terminal: when your layout is reopened at login (**Settings → Session → Remember the layout**), a terminal that had an agent's session in it can come back in the same place, in the same folder, with that session open again.

**On its own it does nothing.** Slipstream doesn't know any agent and doesn't look for one. It works only if you have installed an agent, and that agent leaves a note saying how to reopen its session: a file named for the agent's process ID in `$XDG_RUNTIME_DIR/slipstream/resume/`, holding one line, the command to run. Most agents can run a command of yours when a session starts, which is where to write the note from:

```sh
echo "my-agent --resume $SESSION_ID" > "$XDG_RUNTIME_DIR/slipstream/resume/$AGENT_PID"
```

At the next login that terminal is started in the folder its shell was in and runs the line. It runs in the shell you typed the agent into, started the way a shell at a prompt is, and with the search path the agent had, so it finds the same tools as before. When the agent exits you are left at a shell, as if you had started it by hand.

- **Logging out doesn't stop to ask about it.** With **Remember the layout** on, a terminal with an agent's note is closed without its own "a program is still running" question, since the session comes back with the layout. Every other window is asked to close as usual.
- **Only the note is ever run.** Slipstream never works a command out from what happens to be running, and a terminal with no note comes back as it always has: a new shell.
- **One window, one shell.** A terminal with tabs, or one that keeps all its windows in a single process, can't say which shell belongs to which window, so it comes back as an ordinary terminal.
- **The nearest note is the window's.** If the agent starts another agent of its own, which leaves a note too, the note nearest the terminal's shell is the one that comes back. Notes whose process has gone are cleared away, so nothing from an earlier login is taken for a session open now.
- **When one doesn't come back,** the log, `~/.local/state/slipstream/slipstream.log`, says why for each terminal at logout: no note under its shell, more than one, a note older than its process, or a terminal with several windows or tabs.
- **The conversation, not the work in progress.** What comes back is whatever the agent's own command reopens. Anything it was in the middle of running is not carried on.
- **Where it is kept.** The command, the folder, the shell and the agent's search path go in the layout record, `~/.local/state/slipstream/session.toml`, which is deleted when you turn **Remember the layout** off. Nothing else of the agent's environment is kept.

## Keeping an AI agent working while you're logged out

Slipstream has no AI in it. This is for people who run an AI coding agent of their own in a terminal, and whose agent can run a session in the background with a terminal attached to it. Then logging out doesn't have to stop it: the agent carries on while you're away, and at the next login the terminal comes back in the same place, attached to the same session, still working or showing what it did.

It's what a terminal multiplexer is usually for, without the multiplexer. You keep the terminal you like, with its own keys, look and scrollback, and Slipstream's workspaces and tiling arrange the windows, the way they do for everything else.

**On its own it does nothing.** It works only if you have installed an agent that can run in the background, and it uses the same note as [bringing a session back](#bringing-an-ai-agents-session-back), with a line that attaches rather than reopens. Leave the note from the process that does the attaching, since that is the one in the window; the session itself isn't:

```sh
echo "my-agent attach $SESSION_ID" > "$XDG_RUNTIME_DIR/slipstream/resume/$ATTACHING_PID"
```

A session working in the background has no window above it, so to have its notifications take you to its terminal, send them with the `sender-pid` hint set to the attaching process. Slipstream only looks at the hint when the process that sent the notification has no window of its own.

`agent-notify keep` does all of this for an agent that can work in the background; `agent-notify --help` says which, and how to make it the way your agent always starts.

- **Logging out, not shutting down.** After a restart nothing is still running, and the same note reopens the session instead: the conversation comes back, not the work in progress.
- **Some distributions stop everything at logout.** Where logind's `KillUserProcesses` is on, nothing you started outlives your login, and logging out is like a restart.
- **A sleeping laptop sleeps.** With the lid closed, everything is paused, your agent included.
- **Only the agent is kept.** Anything else running in that terminal, and every other window, closes at logout as it always has.
- **Close the window to leave it working.** With `agent-notify keep`, closing the window or logging out leaves the session running; quitting the agent in its window ends the session, as quitting it anywhere else does, and its conversation is kept.

## Seeing an AI agent at work from across the room

Slipstream has no AI in it. This is for people who set an AI coding agent of their own going and step away: once the desktop has faded to the wallpaper, every open agent session has a small oscilloscope trace at the top right, under whatever the agent says to call it. There is one trace for each open session, however many are running.

- **Cyan wave:** the agent is working, itself or through subagents.
- **Flat amber line:** the agent is open but between tasks, or the session has ended.
- **A row under the trace for each subagent** it has working: a small wave of its own and what that subagent is doing.

**On its own it does nothing.** Slipstream doesn't know any agent and doesn't look for one. It works only if you have installed an agent, and that agent keeps a note while its session is open: a file named for the agent's process ID in `$XDG_RUNTIME_DIR/slipstream/working/`. The first line is what to call the trace; the second line says what the agent is doing: `idle` between tasks, or nothing while it works. Each line after that is a subagent it has working, by what that subagent is doing. Most agents can run a command of yours at the right moments:

```sh
# Session opens: leave an idle note (first line empty falls back to the working folder).
printf '\nidle\n' > "$XDG_RUNTIME_DIR/slipstream/working/$AGENT_PID"

# Task starts:
echo "my-agent: $TASK" > "$XDG_RUNTIME_DIR/slipstream/working/$AGENT_PID"

# Subagents working for it:
printf 'my-agent: the task\n\nreading the tests\nfixing the docs\n' > "$XDG_RUNTIME_DIR/slipstream/working/$AGENT_PID"

# Task ends, or agent is waiting on you: mark idle.
printf 'my-agent: the task\nidle\n' > "$XDG_RUNTIME_DIR/slipstream/working/$AGENT_PID"
```

The note is left in place until the session's process exits; the desktop cleans up notes whose process is gone. Tools that start agents outside the normal process tree, such as multiplexers with their own session management, can set `AGENT_NOTIFY_PID` to the agent's PID when running any hook command, to use that PID directly instead of walking the process ancestry.

A ready-made hook program for several popular agents comes with Slipstream as an extra: [agent-notify](https://github.com/peterwalker78/agent-notify), which the installer asks about along with the companion apps, and `scripts/install-extras --ask` offers later. It is a plugin too, whose card shows your plan limits; it starts off, like every plugin, until you turn it on in **Settings → Plugins**. Run `agent-notify setup` to wire up the hooks and `agent-notify status` to see what it has done. It also handles desktop notifications and leaves the notes for [bringing a session back](#bringing-an-ai-agents-session-back); `agent-notify --help` covers those and lists the agents it supports.

- **Only while the desktop is faded.** Nothing is drawn over your windows; the traces come in after the wallpaper has taken the screen and go as the desktop comes back.
- **Every open session.** A trace shows for each session that has left a note, whether or not it is doing anything right now. A stopped session keeps its amber line until you touch a key or the mouse.
- **One trace for each session, not each subagent.** Subagents are listed under the trace of the session that started them, up to four rows, the last saying how many more there are.
- **Reduced motion** keeps the picture and drops the movement: standing waves for an agent at work and its subagents, and a flat line for one that is idle or has stopped.
- **The name is the agent's to give.** Write the note again to change it. An empty first line falls back to the folder the agent is working in.
