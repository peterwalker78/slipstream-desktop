//! How much a desktop weighs, as Settings' About page scores it: memory at rest, the pieces to
//! install for a complete desktop, and the code to carry, each put on a 0–100 scale and weighted.
//! Lower is leaner. The other desktops' figures are fixed here and checked again each release.

/// One desktop's figures. `memory_mb` is the whole session's proportional memory at rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Desktop {
    pub name: &'static str,
    pub memory_mb: f64,
    /// Packages to install for a complete desktop: bar, launcher, notifications and the like.
    pub pieces: u32,
    /// Megabytes of source code.
    pub code_mb: f64,
    pub memory_estimated: bool,
    pub code_estimated: bool,
}

impl Desktop {
    pub fn score(&self) -> f64 {
        score(self.memory_mb, self.pieces, self.code_mb)
    }

    pub fn estimated(&self) -> bool {
        self.memory_estimated || self.code_estimated
    }
}

const fn desktop(
    name: &'static str,
    (memory_mb, memory_estimated): (f64, bool),
    pieces: u32,
    (code_mb, code_estimated): (f64, bool),
) -> Desktop {
    Desktop {
        name,
        memory_mb,
        pieces,
        code_mb,
        memory_estimated,
        code_estimated,
    }
}

const MEASURED: bool = false;
const ESTIMATE: bool = true;

/// The desktops Slipstream is weighed against.
///
/// - Memory: GNOME, KDE Plasma and Xfce from computingforgeeks.com/desktop-environment-ram-usage
///   (2026, the session's PSS). The tiling compositors and COSMIC from forum reports, so estimates.
/// - Pieces: Arch's package groups for GNOME, KDE Plasma, Xfce and COSMIC; the tiling compositors
///   counted as the compositor and the add-ons it needs to make a complete desktop.
/// - Code: GitHub's language sizes for each project's repositories; estimates where a desktop is
///   spread across too many to add up.
pub const OTHERS: [Desktop; 7] = [
    desktop("niri + add-ons", (200.0, ESTIMATE), 8, (4.5, ESTIMATE)),
    desktop("sway + add-ons", (200.0, ESTIMATE), 8, (5.0, ESTIMATE)),
    desktop("Hyprland + add-ons", (300.0, ESTIMATE), 8, (10.0, ESTIMATE)),
    desktop("Xfce", (469.0, MEASURED), 14, (10.0, ESTIMATE)),
    desktop("COSMIC", (500.0, ESTIMATE), 28, (15.0, ESTIMATE)),
    desktop("GNOME", (533.0, MEASURED), 59, (24.5, MEASURED)),
    desktop("KDE Plasma", (691.0, MEASURED), 68, (20.0, MEASURED)),
];

/// Slipstream's own: itself and xdg-desktop-portal-wlr.
pub const PIECES: u32 = 2;
/// Add-on programs needed for a complete desktop.
pub const ADD_ONS: u32 = 0;
/// The session's memory at rest at its heaviest for this version, rounded up: the compositor with
/// XWayland, the portals and the sound server. Measured again before each release.
pub const WORST_MEMORY_MB: f64 = 300.0;
/// System libraries the compositor loads.
pub const SYSTEM_PACKAGES: u32 = 18;
/// Rust crates compiled into the compositor, and into two others for scale.
pub const CRATES: u32 = 370;
pub const CRATES_NIRI: u32 = 486;
pub const CRATES_COSMIC_COMP: u32 = 702;

/// One figure's share of the score: its weight, and the values that score 0 and 100.
struct Part {
    weight: f64,
    low: f64,
    high: f64,
}

impl Part {
    /// 0–100 on a log scale, so halving a figure counts the same anywhere along it.
    fn scale(&self, value: f64) -> f64 {
        let at = (value / self.low).ln() / (self.high / self.low).ln();
        100.0 * at.clamp(0.0, 1.0)
    }
}

const MEMORY: Part = Part {
    weight: 40.0,
    low: 100.0,
    high: 1000.0,
};
const PIECES_PART: Part = Part {
    weight: 35.0,
    low: 1.0,
    high: 100.0,
};
const CODE: Part = Part {
    weight: 25.0,
    low: 1.0,
    high: 30.0,
};

/// The score from 0 (leanest) to 100.
pub fn score(memory_mb: f64, pieces: u32, code_mb: f64) -> f64 {
    let total = MEMORY.weight + PIECES_PART.weight + CODE.weight;
    (MEMORY.weight * MEMORY.scale(memory_mb)
        + PIECES_PART.weight * PIECES_PART.scale(f64::from(pieces))
        + CODE.weight * CODE.scale(code_mb))
        / total
}

/// Where a score of `ours` places among the others: 1 is the leanest.
pub fn place(ours: f64) -> usize {
    1 + OTHERS.iter().filter(|other| other.score() < ours).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slipstream_scores_about_34() {
        let ours = score(WORST_MEMORY_MB, PIECES, 3.6);
        assert!((ours - 34.0).abs() < 0.5, "{ours}");
        assert_eq!(place(ours), 1);
    }

    #[test]
    fn the_desktops_come_out_leanest_first() {
        let mut all: Vec<(&str, f64)> = OTHERS.iter().map(|d| (d.name, d.score())).collect();
        all.push(("Slipstream", score(WORST_MEMORY_MB, PIECES, 3.6)));
        all.sort_by(|a, b| a.1.total_cmp(&b.1));
        let names: Vec<&str> = all.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            [
                "Slipstream",
                "niri + add-ons",
                "sway + add-ons",
                "Hyprland + add-ons",
                "Xfce",
                "COSMIC",
                "GNOME",
                "KDE Plasma",
            ]
        );
    }

    #[test]
    fn each_part_stays_between_0_and_100() {
        assert_eq!(score(10.0, 0, 0.1), 0.0);
        assert_eq!(score(5000.0, 500, 90.0), 100.0);
        assert!((MEMORY.scale(316.2) - 50.0).abs() < 0.1);
    }
}
