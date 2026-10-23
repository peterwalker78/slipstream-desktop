//! The Settings app's pages, by id, with the settings each one holds: the app builds its sidebar
//! from the ids, and the app explorer finds a page by its title or by any setting on it.

/// One page of the Settings app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsPage {
    /// What `slipstream-settings --page` takes.
    pub id: &'static str,
    pub title: &'static str,
    /// The settings on it, and the everyday names people look for them by, as a search shows
    /// them.
    pub holds: &'static [&'static str],
}

pub const SETTINGS_PAGES: [SettingsPage; 13] = [
    SettingsPage {
        id: "appearance",
        title: "Appearance",
        holds: &[
            "Night light",
            "Dark mode",
            "Light mode",
            "Colour scheme",
            "Color scheme",
            "Focus ring",
            "Border colours",
            "Bullet time highlight",
            "Reduced motion",
            "Animations",
            "Effects",
            "Close into the rain",
        ],
    },
    SettingsPage {
        id: "wallpaper",
        title: "Wallpaper",
        holds: &[
            "Background",
            "Desktop picture",
            "Change the picture every",
            "Fade to the wallpaper after",
        ],
    },
    SettingsPage {
        id: "screens",
        title: "Screens",
        holds: &[
            "Display",
            "Monitors",
            "Resolution",
            "Scale",
            "Arrangement",
            "Variable refresh rate",
        ],
    },
    SettingsPage {
        id: "sound",
        title: "Sound",
        holds: &["Volume", "Audio", "Click when the volume changes"],
    },
    SettingsPage {
        id: "power",
        title: "Power",
        holds: &["Battery", "Charging", "Charge limit", "Battery health"],
    },
    SettingsPage {
        id: "notifications",
        title: "Notifications",
        holds: &[
            "Do not disturb",
            "Notification sounds",
            "Wait until I pause",
            "Windows don't take the keyboard",
        ],
    },
    SettingsPage {
        id: "plugins",
        title: "Plugins",
        holds: &["Plugins", "Add a plugin", "Meter", "Bar modules"],
    },
    SettingsPage {
        id: "input",
        title: "Mouse & keyboard",
        holds: &[
            "Pointer speed",
            "Acceleration",
            "Natural scrolling",
            "Tap to click",
            "Ignore the touchpad while typing",
            "Scrolling",
            "Repeat after",
            "Repeat speed",
            "Layouts",
            "Switch layout",
        ],
    },
    SettingsPage {
        id: "shortcuts",
        title: "Shortcuts",
        holds: &[
            "Shortcut style",
            "Windows style",
            "Mac style",
            "Omarchy style",
            "Hotkeys",
            "Zoom",
            "Magnifier",
            "Accessibility",
            "Your own shortcuts",
            "Custom shortcuts",
            "Apps' own shortcuts",
            "Global shortcuts",
            "Push to talk",
        ],
    },
    SettingsPage {
        id: "workspaces",
        title: "Workspaces",
        holds: &["Workspace names", "Virtual desktops", "Delete a workspace"],
    },
    SettingsPage {
        id: "privacy",
        title: "Privacy",
        holds: &["Screen recording", "Screen sharing"],
    },
    SettingsPage {
        id: "session",
        title: "Session",
        holds: &[
            "Lock after",
            "Lock before sleep",
            "Turn the screen off after",
            "Screen timeout",
            "Remember the layout",
            "Reopen without asking",
            "Clipboard history",
        ],
    },
    SettingsPage {
        id: "about",
        title: "About",
        holds: &[
            "Version",
            "Settings file",
            "Bloat score",
            "Where its code goes",
        ],
    },
];
