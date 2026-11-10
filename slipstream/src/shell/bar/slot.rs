//! A drawer's slot in the bar's foot line.

/// The slot a panel's drawer comes out of in the bar's foot line this frame: from `left` to
/// `right` in logical pixels across the screen, and how strongly lit, 1 while the drawer moves and
/// more in its glint. The drawer works it out and the bar lights it, so the line has one owner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarSlot {
    pub left: f64,
    pub right: f64,
    pub strength: f32,
}

impl BarSlot {
    /// The stronger of two, for a frame with more than one drawer moving.
    pub fn strongest(a: Option<Self>, b: Option<Self>) -> Option<Self> {
        match (a, b) {
            (Some(a), Some(b)) => Some(if b.strength > a.strength { b } else { a }),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strongest_slot_is_the_one_the_bar_lights() {
        let slot = |strength| {
            Some(BarSlot {
                left: 10.0,
                right: 20.0,
                strength,
            })
        };
        assert_eq!(BarSlot::strongest(slot(0.5), slot(1.2)), slot(1.2));
        assert_eq!(BarSlot::strongest(slot(0.8), None), slot(0.8));
        assert_eq!(BarSlot::strongest(None, None), None);
    }
}
