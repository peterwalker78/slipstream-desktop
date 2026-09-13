# Changelog

Slipstream is in beta: anything can change between versions, including settings and keys.

## [0.0.4] - 2026-09-14

- **A window that opens while you type waits for your pause.** It no longer splits the window you're typing in mid-sentence: it stays off screen until you stop for four seconds, then tiles beside you without taking the keyboard. Like notifications, it never waits longer than the longest wait in Settings → Notifications.
- **The bar says why it appeared.** "*App* opened while you typed · Alt+Tab" shows from the moment it opens; clicking it or pressing Alt+Tab brings the window in. The new tile keeps a dim ring until you visit it, and both go a minute after it tiles.

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
