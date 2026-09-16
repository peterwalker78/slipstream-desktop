# Changelog

Slipstream is in beta: anything can change between versions, including settings and keys.

## [Unreleased]

- **A monitor you plug in gets a workspace of its own** instead of taking one of yours. Before, a
  second screen claimed the lowest-numbered free workspace, so Super+2 stopped showing workspace 2
  on the screen you were working on and jumped the keyboard to the monitor instead. Now the
  numbered workspaces stay where they are, and the monitor gets an empty one of its own — a blank
  area to throw windows at. The first time a given screen is seen, a card offers the other answer;
  the choice is remembered against that screen, so it's asked once per monitor rather than once per
  dock. A workspace made this way disappears when it is empty and its screen has gone, and stays
  as an ordinary workspace at the end of the list for as long as anything is open on it.
- **Super+Shift+P sends a window to the next screen without following it.** It used to take the
  keyboard along, which is the wrong reflex for throwing something on to a second monitor; Super+P
  still moves the keyboard when you do want to go. A toast names the screen and workspace it
  landed on.
- **Screens are told apart by what they are, not which port they are in**, so the same monitor on a
  different port is recognised and a different monitor on the same port is not.

- **The volume and brightness card stays put while you hold the key.** It played its rise and fade
  in again on every press, so the card flickered under a held key instead of the level simply
  sliding along. The entrance now happens once, when the card appears; further presses only move
  the bar and put its departure off.
- **A notification pop-up has a cross to close it with**, in its top right corner as the
  notification centre's cards do, so one that's in the way goes with a click rather than a trip
  through the centre. The notification stays in the centre; a click elsewhere on the pop-up still
  opens it.
- **Quick settings' volume slider clicks at the new level**, as the volume keys already did, so the
  volume can be heard as well as seen when it's set with the mouse or the arrow keys. The same
  Sound setting turns both off.
- **Installing works when there's no terminal to type a password into** — run from a file manager, a launcher, or a shell that isn't attached to one. sudo asks through the desktop's graphical helper instead of refusing.

## [0.5.1] - 2026-09-16

- **Installing works on systems whose sudo asks for the root password** (openSUSE's default). The installer asked sudo to check the password up front, which such a configuration refuses outright even where it would let the install through; it now tries a real command before giving up.
- **A first-boot wizard's leftover automatic login is no longer reported as one.** Some systems keep an `[Autologin]` section naming their own setup account long after the login screen has come back, and the installer wrongly said no login screen would appear. Only an account that belongs to a person counts.

## [0.5.0] - 2026-09-16

- **One download for every distribution.** Releases had one build per system, each linking that system's libraries; there is now a single `slipstream-VERSION-linux-x86_64.tar.gz` that runs on any current distribution with Debian 13's libraries or newer. libdisplay-info, whose soname changes with every release, is linked in, and each release is checked before publishing that nothing asks for a newer glibc or an unusual library.
- **One command installs it**, from the README: it fetches the newest release, checks it against its published checksum and runs the installer. `--try` opens Slipstream in a window on your current desktop first, `--check` says what installing would do and writes nothing. Downloads carry a signed record of where they were built, which `gh attestation verify` checks.
- **Slipstream installs for everyone on the machine.** The programs, the Settings app's entry, the session's systemd units and the portal settings go in `/usr/local`; a copy in your own `~/.local/bin` still runs first, so a build of your own keeps working while other accounts use the installed release. A second account that chose Slipstream used to get a session that failed to start.
- **The installer offers to install what's missing** with the system's own package manager, having named it first, rather than only printing the command. On atomic systems, where that rewrites the system image, it still prints the command. Without a terminal it asks nothing and installs nothing.
- **Slipstream is its own desktop, and treats every other one alike.** It no longer names KDE in `XDG_CURRENT_DESKTOP`; instead, the portal settings list the backends the machine actually has, in the order found, with the generic GTK one last, so file choosers, secrets and dark mode work whatever desktop the machine came with. Qt apps take their dialogs and colours from the portal too, rather than from another desktop's plugin. The cursor theme is the one your system calls its default (`icons/default/index.theme`), which on a KDE machine may mean a different pointer than before.
- **It says when screen sharing can't work**: the capture protocols Slipstream speaks need xdg-desktop-portal-wlr 0.8 or newer, and Debian 13 and Ubuntu 25.10 package 0.7. Everything else works there.
- **It spots a machine that logs straight in** past the login screen (SDDM, GDM, LightDM or greetd) and says how to reach the session list, or how to make Slipstream the session it logs into.
- **Every release is installed and started on Debian 13, Ubuntu 26.04, Fedora 43 and 44, Arch and openSUSE Tumbleweed** before it's called done, and the newest release is checked again weekly against the distributions that keep moving, so a change in one of them shows up before somebody's first login.
- `scripts/try-slipstream` runs Slipstream in a window on the desktop you're already using, with its settings and state in a scratch folder, so nothing changes.

## [0.4.3] - 2026-09-15

- **Releases have a download for Ubuntu 26.04 LTS** and the systems built on it, such as Kubuntu 26.04, beside the one for Fedora 44, so neither needs building from source. The README says which download is whose, and how to build on Ubuntu. Ubuntu 24.04 and the systems built on it are too old for the Settings app.

## [0.4.2] - 2026-09-15

- **Slipstream shows up on SDDM's login screen on Ubuntu 22.04, Kubuntu, Debian 12 and other systems with SDDM 0.19.** That version only reads sessions from `/usr/share/wayland-sessions`, so on a system whose `/usr` can be written, `install-session` now puts the entry there for SDDM and Plasma Login (atomic systems keep `/usr/local`), and moves an entry an earlier install left in `/usr/local`. A `SessionDir` is only taken from the `[Wayland]` section of SDDM's configuration, and `sddm.conf` wins over `sddm.conf.d`, as in SDDM itself.
- `install-session` says when GDM has Wayland turned off (in `custom.conf`, or by its own rules for NVIDIA's driver without `nvidia-drm.modeset=1`, `nomodeset` and some virtual machines), since GDM then lists no Wayland session at all, and no longer claims Slipstream is on the list.

## [0.4.1] - 2026-09-15

- **The browser that comes with Slipstream is now called Glimmerwood.** Another web browser already goes by Wisp, so it has a name of its own; its companion is still the wisp. `extras/glimmerwood.conf` installs it from [Glimmerwood's releases](https://github.com/peterwalker78/glimmerwood/releases) under its new app ID, `io.github.peterwalker78.Glimmerwood`, beside any Wisp 0.1.0 already installed, which `flatpak uninstall --user io.github.peterwalker78.Wisp` removes. A `~/.config/slipstream/extras/wisp.conf` of your own no longer replaces the shipped entry: rename it to `glimmerwood.conf`.

## [0.4.0] - 2026-09-15

- **Slipstream comes with Wisp**, a calm web browser with a living companion that reflects how your time online feels, from [its own project](https://github.com/peterwalker78/wisp). `update-session` installs it from Wisp's latest release as a Flatpak, checks it against the release's SHA-256, and updates it when a new release comes out. It isn't made the default browser. A file of your own in `~/.config/slipstream/extras/` with the same name replaces the shipped one, and `install=no` on its own leaves Wisp out. Release packages carry the shipped `extras/`.
- The README says why Slipstream exists: a desktop that's private by design, builds healthy habits in, and keeps distractions out.
- **A meter of your own on the bar.** `[meter] command` runs any command that prints a small JSON report of how much of something is used (a quota, a plan's limits, a disk), every `every-secs`. The bar shows a gauge for each section beside quick settings, amber at 75% and orange at 90%, and clicking it opens quick settings, which lists every meter with the time left until it starts over. Numbers the report marks old, or that a failing command couldn't refresh, stay on show dimmed. See the README.
- **Settings has a Meter page** for the meter's command and how often it runs, with **Run it now**, which shows what the bar would show or why the command doesn't work: its exit code and the last thing it said, a report that isn't readable, or a command too slow to answer.
- `install-extras` installs an app's bundle when building it from its checkout fails and the app isn't installed yet, so a broken build never leaves it missing. An app that's already installed keeps its own build.

## [0.3.0] - 2026-09-15

- **More than one screen behaves predictably.** Super+1–9 to a workspace another screen is already showing now moves the keyboard to that screen, and neither screen changes what it shows. Before, the two screens traded workspaces. Trading is now its own key, **Super+Ctrl+P**, which swaps workspaces with the next screen, windows and all. Super+Ctrl+←/→ passes over workspaces another screen is showing, and bar clicks and bullet time follow the same rule.
- **Windows no longer appear on the wrong screen.** Whenever the left screen showed a higher-numbered workspace than the right one, each screen drew the other's windows over its own. Each screen now draws only its own windows.
- **Screens remember their workspaces.** A monitor unplugged and plugged back in comes back on the workspace it was showing. Shutting the lid carries the laptop's workspace on to the other screen as before, and opening it puts both screens back as they were. A workspace that isn't showing keeps the size of the screen it was last on, so its windows aren't resized every time the keyboard moves between screens of different sizes.
- **Super+←/→ at the edge of a screen carries on to the screen beside it**, landing on the window nearest that edge, and **Super+Alt+←/→** at the edge moves the window there.
- **Each screen's bar says where the keyboard is.** The workspace is highlighted fully only on the screen with the keyboard, workspaces another screen is showing are underlined in grey, and each bar names its own screen's window, dimmed where the keyboard isn't. A fullscreen window hides only its own screen's bar.
- **The battery shows when it's charging.** On a charger the battery icon turns mint with a lightning bolt through it, and the percentage beside it turns mint too. Plugging in or pulling out the charger shows a card with the charge. A battery that's full or held at a charge limit counts as plugged in, so it never gets low-battery warnings.
- **Settings has a Power page:** the charge and the time until full or empty, power going in or out, the charger's rating, the charge limit, the battery's health against when it was new, and its charge cycles, read every couple of seconds. Clicking the battery in quick settings, or choosing it with the keyboard, opens it.
- **The battery's time estimate is steady.** It's averaged over a few minutes instead of jumping with every change in load, starts over when a charger is plugged in or pulled out, and is rounded to five minutes past an hour.
- Each Settings page keeps its own scroll position, so a short page is never shown scrolled down past its end. Launching Settings on a page while it's open turns the open window to that page.
- **`update-session` can install apps that are projects of their own** as Flatpaks, described in `~/.config/slipstream/extras/`: built from a local checkout whenever it has new commits, or downloaded as a bundle and checked against its SHA-256, and optionally made the default web browser once. See the README.
- The living wallpaper changes exactly when "Change every" is up, cross-fading mid-cycle rather than finishing its cycle first, including while it's slowed on low battery.
- The lock screen and quick settings no longer show an initial in a circle. The lock screen names the user quietly above the password box, and quick settings leads with the battery.
- **Notifications can make a sound.** Slipstream plays the sound a notification asks for (the spec's `sound-file`, `sound-name` and `suppress-sound` hints), looking names up in your sound theme the way the XDG Sound Theme spec describes: the theme, the themes it inherits from, then `freedesktop`, trying shorter names (`message-new-instant`, then `message-new`, then `message`) so a theme without the exact sound still has something close. They play with PipeWire's Notification role, one at a time. Under do not disturb only critical notifications sound. Settings → Notifications has the switch, and `[notifications] sound-theme` picks the theme.
- **Notifications know which window they came from.** Slipstream asks the bus which process sent a notification, pins it with a pidfd, and follows its parents up to the first one with a window, so `notify-send` from a script in a terminal belongs to that terminal. Opening the notification brings that exact window forward, even with several windows of the same app open. While you're working in that window, its notifications make no sound, don't wake anything, and a critical one comes and goes like any other instead of staying until dismissed.
- **A critical notification brings the desktop back** from the living wallpaper, so it can be seen from across the room. It doesn't count as you being there, so the screen still locks on time, and nothing shows over the lock screen.

## [0.2.0] - 2026-09-14

- **Floating windows.** Dialogs and fixed-height windows (splash screens, Firefox's picture-in-picture) open floating above the tiling: a dialog centred over the window it belongs to, anything else centred on the screen at the size it asks for. They stay above the tiles with dialogs above their parents, move with their workspace, and keep their place when a screen changes size. **Super+Shift+V** floats a window or tiles it again, and **Super+Ctrl+V** moves the keyboard between the floating windows and the tiles. While a floating window has the keyboard, Super+arrows go between floating windows, Super+Alt+arrows move it and Super+[ ] resize it; Super+drag or its own title bar moves it and its edges resize it. A floating window asking for fullscreen fills the screen from the tiling and floats again afterwards.
- **Night light can follow a schedule**: sunset to sunrise, worked out from your time zone with no location needed, or hours you set (Settings → Appearance). When the schedule turns it, the screen warms up over half an hour the way the light outside changes, and cools the same way in the morning; switching it by hand in quick settings eases over a second and lasts until the next change.
- **Low battery is looked after.** Unplugged at 20%, the living wallpaper slows to half speed to save power, and a toast says so. At 10% a toast says how long is left. At 5% a card counts down a minute to sleep, so what you have open survives in memory: Enter sleeps now, Esc holds it off for another 2%, and plugging in takes it away. Locked at 5%, the laptop simply sleeps.
- **The app explorer answers sums and conversions, and finds emoji.** Type `12*7`, `2^10`, `sqrt 2` or `15% of 80`, or `5 km in miles`, `100f to c`, `3.5 GiB in MB`, `70 kg in st`: the answer appears on the right, decoding out of rain glyphs as it changes, and Enter copies it. Type `:` and a word (`:fire`, `:thumbs`, `:party`) and Enter types the emoji into the app you're in.
- **Super+V opens the clipboard history**, as on Windows: the last 25 things you copied, text and pictures (screenshots and snips included), newest first. Choose one and press Enter to paste it into the app you're in; Delete forgets one. The chosen entry's text decodes out of rain glyphs as you reach it. It's kept in memory only and forgotten when you log out, anything a password manager marks as secret is never kept, and Settings → Session has a switch to turn it off.
- While Caps Lock is on, an amber **CAPS** chip sits beside the tray, and on the lock screen the password box shows the Caps Lock arrow at its right end, as Windows does.
- Caps Lock's and Num Lock's cards read the same either way: a title saying which way the key went, and a note under it ("Screensaver paused" and "Screensaver back on"; on the lock screen, "Typing in capitals" and "Typing in lower case"; "Number pad types numbers" and "Number pad moves the cursor"). Both stay up equally long.
- Renaming a workspace in Settings no longer moves the keyboard to another window after every letter typed.
- **Super+Shift+S snips** part of the screen, as on Windows. The screen freezes and dims: drag out a region, press the letter shown on a window (or click it) for just that window, or press Enter for the whole screen. Esc or a right click gives up. The snip is saved and copied like any screenshot, and breaks into fragments that turn into rain glyphs and fly into the toast saying where it went. With reduced motion it simply fades.
- Games can lock the mouse for mouse look, or keep it inside their window, and hear how far it moved; virtual machines and remote desktops can ask for every key. Only the window you're using gets either, and the first time it takes them a toast says so. **Super+Esc** takes the mouse and keys back, and pressed again gives them back to the window. The volume, brightness and media keys always stay the desktop's.
- A minimised app's button above its code rain shows only its icon and its meter, so every button is the same height. The rain still spells the app's name, and an app with no icon shows its initial.
- Apps can ask for a cursor from your theme by name, so GTK 4, Qt 6 and Chromium apps show the same pointer as everything else, at the right size, instead of drawing their own.
- Video players and games are told exactly when each frame reached the screen, so they can pace playback smoothly.
- File choosers and other portal dialogs are attached to the app that opened them.
- Input methods work in Wayland apps: IBus and fcitx5 can type Chinese, Japanese, Korean and other scripts, with their candidate pop-up beside the text, and on-screen keyboards can type. Only programs outside a sandbox can act as an input method or keyboard, since either sees or sends every key.
- Other programs' launchers, pickers, docks and overlays work (layer shell): fuzzel, wofi, slurp, colour pickers, on-screen displays and wallpaper setters. Launchers that want the keyboard get it while they're up and hand it back when they close, docks along an edge take that strip out of the tiling, a window filling the screen covers all but overlays, and they fade with the rest of the desktop. Only programs outside a sandbox can draw them.
- Touchpad swipes, pinches and holds reach apps, so pinching zooms pages and pictures.
- Coming back from the living wallpaper takes 0.5 s and speeds up all the way into place, mirroring the fade out, where the desktop speeds away. It hurries through the first moment, before any of the desktop can be seen, so a key press is answered at once.

## [0.1.0] - 2026-09-14

- The fade to the living wallpaper is now two panes of glass. The desktop's pane tips back about its bottom right corner as it sinks away, and where it passes through the wallpaper's pane the soft diagonal edge sweeps across the screen from the top left, blurring and catching the light without pulling the picture aside, so the desktop moves smoothly as the edge passes over it. Past the edge the desktop is behind the wallpaper, darkening as it falls away. Each way takes 0.6 s. Waking runs the same movement in reverse, but starts at full speed and settles, so the desktop shows up as soon as a key is pressed.
- Caps Lock no longer holds off the screen lock. It still keeps the screensaver off, but a desktop left with Caps Lock on now locks after its idle minutes like any other. Its card says so ("Screensaver paused", adding "lock still on" when the screen locks by itself) and stays up half a second longer. The key hints on an empty workspace and the Super+/ sheet list Caps Lock too.

## [0.0.8] - 2026-09-14

- Typing now ends after 15 seconds without a key press instead of four, so notifications and windows from other apps wait through the short breaks while you think.
- The bar no longer shows "Back to *app* · Alt+Tab" after a notification or another app takes you to a window. Alt+Tab still goes back.

## [0.0.7] - 2026-09-14

- The bar's buttons take clicks the bar's whole height, right up to the top edge of the screen, and the apps button and the bell reach out to the screen's corners, so a pointer pushed against the edge still clicks what's under it.
- The app name the code rain spells out is lit in the focus ring's colour, like the name on the stream's header, instead of the rain's own, with its vowels in lower case and the rest in capitals ("KoNSoLe").

## [0.0.6] - 2026-09-14

- The code rain spells out each app's name again as it falls, as well as writing it on the stream's header.

## [0.0.5] - 2026-09-14

- Three new living wallpapers, making twenty. **Galaxies:** the two halves of the logo wind up into spiral galaxies that collide and merge, and then the collision runs backwards until the letters are whole again. **Chladni:** the logo's dots are sand on a ringing plate, shaken into a new figure by each note, and they walk home when the plate falls quiet. **Frost:** frost grows out of the letters in branching ferns, glitters, and melts back the way it came.

## [0.0.4] - 2026-09-14

- **Windows you didn't ask for go to the code rain.** A new window tiles and takes the keyboard only when you asked for it: a dialog or window of the app you're using, a program started from it (a command in a terminal), something launched from the desktop, or a window that appears within a few seconds of a click or key press. Anything else, such as an app opening a window by itself, pours into the code rain with a toast naming it; Super+Shift+M or a click on its stream brings it in. While you type in one app, a window from another goes to the rain too, so the window you're typing in never changes size.
- The code rain's app names are written in the focus ring's colour, and follow it when it's changed in Settings.

## [0.0.3] - 2026-09-13

- The living wallpaper's variations take turns in a random order: each ticked one shows once, shuffled, before any comes back, and none follows itself. The first one after login is random too.
- The Caps Lock card said "on" whichever way the key went; it now says which. While Caps Lock is on, the card notes that the screensaver and screen lock are paused.
- Caps Lock now holds off the idle lock as well as the wallpaper fade.

## [0.0.2] - 2026-09-13

### Concentration first

- **Windows opened while you type wait beside you.** While keys are reaching an app, only that app, its dialogs, or a program it started (a command run in a terminal) can move the keyboard. Anything else tiles beside the focused window and is next in Alt+Tab. Typing is judged from key timing alone, never which keys, and ends at a four-second pause, a shortcut, a click, a desktop key or a change of focus.
- **Notifications wait for a pause.** A pop-up that arrives while you type is kept and counted on the bell, and shows at the next pause. Several come up as one card that opens the notification centre. Critical notifications never wait, and nothing waits longer than 15 minutes (`notifications.longest-wait-mins`). Settings → Notifications turns it off.
- **The way back.** After a notification or another app takes you to a window, the bar shows "Back to *app* · Alt+Tab" for a minute.
- **xdg-activation.** Apps can bring their windows forward when you ask them to, such as a link clicked in one app opening in a browser that's already running.

### Installing

- `install-session` writes its system files all or nothing: every file is staged beside its destination and moved into place only once all are there, so a failed or interrupted install leaves the system as it was. One sudo prompt.
- It never replaces or removes a file it didn't write (a PAM service, session entry or launcher already there), ignores anything in its manifest that isn't a Slipstream file, and removes files an earlier install wrote that are no longer needed (such as a drop-in for a display manager you've moved away from).
- The lock screen's PAM service is chosen from the stacks this system actually has (password-auth, system-auth, common-auth or login), and it's refused before anything is written if there are none.
- Files are readable by the login screen whatever your umask; paths with spaces work; a system with no display-manager unit is no longer misreported.
- `update-session` installs each program beside the old one and swaps it in, checks both programs' libraries and that the compositor runs (`slipstream --version`), and installs a release's ready-built programs without building.
- The session launcher falls back to a runtime folder for its log if the state folder can't be written, and gives `localectl` five seconds at most.
- A test suite for `install-session` runs in CI under sh and dash.

### Releases

- Version tags build the programs on Fedora 44 in CI and attach them, with the install scripts, to the release.
- `slipstream --version`.

## [0.0.1] - 2026-09-13

The first beta.

- **Tiling** across named workspaces (up to 20), several screens, fullscreen, XWayland, Alt+Tab, resizing and moving tiles from the keyboard.
- **Gravity**: windows weighted by importance, from distant to tiled.
- **Bullet time** (Super+Tab): a 3D overview with letter hints that slows everything the compositor draws.
- **Code rain**: minimised windows become streams that show each app's CPU and memory.
- **App explorer** (Super+Space): apps, files and a run prompt.
- **Living wallpaper**: seventeen animated variations, chosen in Settings.
- **Bar, quick settings and notifications**: clock and calendar, tray, media keys over MPRIS, volume and brightness display, night light, pop-ups with actions, notification centre, do not disturb.
- **Lock screen** (Super+L) through PAM, before sleep and optionally after idle.
- **Screen sharing** through xdg-desktop-portal-wlr, with Slipstream's own chooser for screens and windows.
- **Session**: the way out asks apps to close and waits for them; the layout can be remembered and reopened at login.
- **Settings app** (Super+I), applied live.
- **Installer**: `scripts/install-session` and `scripts/update-session`.
