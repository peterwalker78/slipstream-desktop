//! The Featherweight score, which Settings' About page shows: how light a desktop is in real use.
//! Three things count equally: the space it takes once installed, the memory it uses once you're
//! logged in, and how much of a typical computer it keeps busy while nobody touches it. Each is put
//! on a 0–100 scale, and lower is lighter.
//!
//! Every desktop is measured the same way, Slipstream included: installed complete on the same
//! Arch Linux system, then logged in on the same virtual machine and left alone (the weigh
//! workflow, `scripts/weigh-install` and `scripts/weigh-session`). The figures are taken again
//! before each release.

/// One desktop's figures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Desktop {
    pub name: &'static str,
    /// Packages it adds when installed, counting what those depend on.
    pub packages: u32,
    /// Megabytes it adds on disk.
    pub disk_mb: f64,
    /// Megabytes of memory used by what it starts once logged in.
    pub memory_mb: f64,
    /// Percent of one of the measuring machine's processors it keeps busy at rest.
    pub processor: f64,
}

impl Desktop {
    pub fn score(&self) -> f64 {
        score(self.disk_mb, self.memory_mb, self.processor)
    }

    /// The share of a typical computer it keeps busy at rest, in percent.
    pub fn share_at_rest(&self) -> f64 {
        self.processor / f64::from(TYPICAL_THREADS)
    }
}

const fn desktop(
    name: &'static str,
    packages: u32,
    disk_mb: f64,
    memory_mb: f64,
    processor: f64,
) -> Desktop {
    Desktop {
        name,
        packages,
        disk_mb,
        memory_mb,
        processor,
    }
}

/// Slipstream as it comes, with the living wallpaper moving.
pub const SLIPSTREAM: Desktop = desktop("Slipstream", 104, 287.0, 118.0, 11.1);
/// Slipstream with reduced motion, where the wallpaper holds still.
pub const SLIPSTREAM_REDUCED: Desktop =
    desktop("Slipstream, reduced motion", 104, 287.0, 118.0, 1.7);

/// The desktops Slipstream is weighed against, each complete: with a bar, launcher,
/// notifications, lock screen and portals where it doesn't have its own.
pub const OTHERS: [Desktop; 7] = [
    desktop("sway", 134, 409.0, 78.0, 0.7),
    desktop("Xfce", 122, 313.0, 219.0, 0.2),
    desktop("Hyprland", 158, 576.0, 207.0, 0.2),
    desktop("niri", 265, 805.0, 175.0, 0.2),
    desktop("GNOME", 505, 1912.0, 556.0, 0.3),
    desktop("KDE Plasma", 494, 2693.0, 776.0, 0.6),
    desktop("COSMIC", 239, 2687.0, 923.0, 0.6),
];

/// A modest laptop's processor threads: what "a typical computer" means for time at rest. Most
/// have more.
pub const TYPICAL_THREADS: u32 = 8;

/// One figure's share of the score: the values that score 0 and 100.
struct Part {
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

const DISK: Part = Part {
    low: 100.0,
    high: 5000.0,
};
const MEMORY: Part = Part {
    low: 25.0,
    high: 2000.0,
};
/// A share of a typical computer: under 1% is lost in what any system does in the background,
/// and by 25% fans and slowdowns are noticed.
const AT_REST: Part = Part {
    low: 1.0,
    high: 25.0,
};

/// The score from 0 (lightest) to 100, for `processor` percent of one measuring processor.
pub fn score(disk_mb: f64, memory_mb: f64, processor: f64) -> f64 {
    (DISK.scale(disk_mb)
        + MEMORY.scale(memory_mb)
        + AT_REST.scale(processor / f64::from(TYPICAL_THREADS)))
        / 3.0
}

/// Where a score of `ours` places among the others: 1 is the lightest.
pub fn place(ours: f64) -> usize {
    1 + OTHERS.iter().filter(|other| other.score() < ours).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slipstream_scores_about_24_and_about_21_with_reduced_motion() {
        assert!(
            (SLIPSTREAM.score() - 24.2).abs() < 0.5,
            "{}",
            SLIPSTREAM.score()
        );
        assert!(
            (SLIPSTREAM_REDUCED.score() - 20.8).abs() < 0.5,
            "{}",
            SLIPSTREAM_REDUCED.score()
        );
    }

    #[test]
    fn the_desktops_come_out_lightest_first() {
        let mut all: Vec<(&str, f64)> = OTHERS.iter().map(|d| (d.name, d.score())).collect();
        all.push((SLIPSTREAM.name, SLIPSTREAM.score()));
        all.sort_by(|a, b| a.1.total_cmp(&b.1));
        let names: Vec<&str> = all.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            [
                "sway",
                "Slipstream",
                "Xfce",
                "Hyprland",
                "niri",
                "GNOME",
                "KDE Plasma",
                "COSMIC",
            ]
        );
        assert_eq!(place(SLIPSTREAM.score()), 2);
    }

    #[test]
    fn each_part_stays_between_0_and_100() {
        assert_eq!(score(50.0, 10.0, 0.1), 0.0);
        assert_eq!(score(9000.0, 5000.0, 800.0), 100.0);
        assert!((MEMORY.scale(223.6) - 50.0).abs() < 0.1);
    }
}
