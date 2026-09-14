# Changelog

Slipstream is in beta: anything can change between versions, including settings and keys.

## Unreleased

- **Floating windows, as niri has them.** Dialogs and fixed-height windows (splash screens, Firefox's picture-in-picture) open floating above the tiling: a dialog centred over the window it belongs to, anything else centred on the screen at the size it asks for. They stay above the tiles with dialogs above their parents, move with their workspace, and keep their place when a screen changes size. **Super+Shift+V** floats a window or tiles it again, and **Super+Ctrl+V** moves the keyboard between the floating windows and the tiles. While a floating window has the keyboard, Super+arrows go between floating windows, Super+Alt+arrows move it and Super+[ ] resize it; Super+drag or its own title bar moves it and its edges resize it. A floating window asking for fullscreen fills the screen from the tiling and floats again afterwards.
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
