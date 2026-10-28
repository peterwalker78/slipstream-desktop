//! A row's layout: its text, with the control beside it as a horizontal box would place it, or
//! below it once beside would leave the text less than half the row. A window in a half-screen
//! tile then keeps whole words on a line instead of a column of broken ones.

use gtk::{glib, graphene, gsk, prelude::*, subclass::prelude::*};
use gtk4 as gtk;

/// Between the text and the control when they sit side by side.
const BESIDE: i32 = 14;
/// Between the text and the control below it.
const BELOW: i32 = 8;

glib::wrapper! {
    pub struct RowBox(ObjectSubclass<imp::RowBox>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl RowBox {
    pub fn new(text: &impl IsA<gtk::Widget>, control: &impl IsA<gtk::Widget>) -> Self {
        let row: Self = glib::Object::new();
        row.add_css_class("row");
        text.set_parent(&row);
        control.set_parent(&row);
        row
    }

    fn parts(&self) -> Option<(gtk::Widget, Option<gtk::Widget>)> {
        let text = self.first_child()?;
        let control = self
            .last_child()
            .filter(|control| *control != text && control.should_layout());
        Some((text, control))
    }
}

fn widths(widget: &gtk::Widget) -> (i32, i32) {
    let (min, nat, _, _) = widget.measure(gtk::Orientation::Horizontal, -1);
    (min, nat)
}

fn heights(widget: &gtk::Widget, width: i32) -> (i32, i32) {
    let (min, nat, _, _) = widget.measure(gtk::Orientation::Vertical, width);
    (min, nat)
}

/// The text's and the control's widths side by side in `width`, or None when the control goes
/// below.
fn beside(text: &gtk::Widget, control: &gtk::Widget, width: i32) -> Option<(i32, i32)> {
    let size = |widget: &gtk::Widget| {
        let (min, nat) = widths(widget);
        Size {
            min,
            nat,
            expands: widget.compute_expand(gtk::Orientation::Horizontal),
        }
    };
    share(width, size(text), size(control))
}

/// A child's minimum and natural widths, and whether it takes extra room.
#[derive(Clone, Copy)]
struct Size {
    min: i32,
    nat: i32,
    expands: bool,
}

/// Shares `width` between the text and the control as a horizontal box would, or None when that
/// leaves the text less than half the row and less than it needs.
fn share(width: i32, text: Size, control: Size) -> Option<(i32, i32)> {
    let room = width - BESIDE;
    let extra = room - text.min - control.min;
    if extra < 0 {
        return None;
    }
    // Each grows towards its natural width, the one needing less filled first, and what's left
    // over goes to whichever expands.
    let (text_gap, control_gap) = (text.nat - text.min, control.nat - control.min);
    let first = ((extra + 1) / 2).min(text_gap.min(control_gap));
    let second = (extra - first).min(text_gap.max(control_gap));
    let (mut text_width, mut control_width) = if text_gap <= control_gap {
        (text.min + first, control.min + second)
    } else {
        (text.min + second, control.min + first)
    };
    let left = room - text_width - control_width;
    if left > 0 {
        match (text.expands, control.expands) {
            (true, true) => {
                text_width += left - left / 2;
                control_width += left / 2;
            }
            (false, true) => control_width += left,
            _ => text_width += left,
        }
    }
    (text_width >= text.nat.min(width / 2)).then_some((text_width, control_width))
}

/// The control's width on a line of its own.
fn below(control: &gtk::Widget, width: i32) -> i32 {
    if control.compute_expand(gtk::Orientation::Horizontal) {
        return width;
    }
    let (min, nat) = widths(control);
    nat.min(width).max(min)
}

fn place(child: &gtk::Widget, x: i32, y: i32, width: i32, height: i32) {
    let at = gsk::Transform::new().translate(&graphene::Point::new(x as f32, y as f32));
    child.allocate(width, height, -1, Some(at));
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RowBox;

    #[glib::object_subclass]
    impl ObjectSubclass for RowBox {
        const NAME: &'static str = "SlipstreamRowBox";
        type Type = super::RowBox;
        type ParentType = gtk::Widget;

        fn class_init(class: &mut Self::Class) {
            class.set_css_name("box");
            class.set_accessible_role(gtk::AccessibleRole::Generic);
        }
    }

    impl ObjectImpl for RowBox {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for RowBox {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some((text, control)) = self.obj().parts() else {
                return (0, 0, -1, -1);
            };
            let Some(control) = control else {
                let (min, nat, _, _) = text.measure(orientation, for_size);
                return (min, nat, -1, -1);
            };
            let ((text_min, text_nat), (control_min, control_nat)) =
                (widths(&text), widths(&control));
            if orientation == gtk::Orientation::Horizontal {
                // As narrow as the control on a line of its own allows.
                return (
                    text_min.max(control_min),
                    text_nat + BESIDE + control_nat,
                    -1,
                    -1,
                );
            }
            let width = if for_size < 0 {
                text_nat + BESIDE + control_nat
            } else {
                for_size
            };
            let (min, nat) = match beside(&text, &control, width) {
                Some((text_width, control_width)) => {
                    let (text, control) =
                        (heights(&text, text_width), heights(&control, control_width));
                    (text.0.max(control.0), text.1.max(control.1))
                }
                None => {
                    let text = heights(&text, width);
                    let control = heights(&control, below(&control, width));
                    (text.0 + BELOW + control.0, text.1 + BELOW + control.1)
                }
            };
            (min, nat, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let row = self.obj();
            let Some((text, control)) = row.parts() else {
                return;
            };
            let Some(control) = control else {
                place(&text, 0, 0, width, height);
                return;
            };
            let rtl = row.direction() == gtk::TextDirection::Rtl;
            let mirror = |x: i32, w: i32| if rtl { width - x - w } else { x };
            match beside(&text, &control, width) {
                Some((text_width, control_width)) => {
                    place(&text, mirror(0, text_width), 0, text_width, height);
                    let x = text_width + BESIDE;
                    place(&control, mirror(x, control_width), 0, control_width, height);
                }
                None => {
                    let (_, text_height) = heights(&text, width);
                    place(&text, 0, 0, width, text_height);
                    let control_width = below(&control, width);
                    let (_, control_height) = heights(&control, control_width);
                    let x = width - control_width;
                    let y = text_height + BELOW;
                    place(
                        &control,
                        mirror(x, control_width),
                        y,
                        control_width,
                        control_height,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row's text: a word or two at the least, a long sentence at its natural width.
    const TEXT: Size = Size {
        min: 70,
        nat: 900,
        expands: true,
    };
    const DROPDOWN: Size = Size {
        min: 120,
        nat: 120,
        expands: false,
    };

    #[test]
    fn at_full_width_the_control_sits_beside_its_text() {
        assert_eq!(share(640, TEXT, DROPDOWN), Some((640 - BESIDE - 120, 120)));
    }

    #[test]
    fn in_a_half_screen_tile_a_wide_control_goes_below_its_text() {
        let wide = Size {
            min: 230,
            nat: 230,
            expands: false,
        };
        assert_eq!(share(360, TEXT, wide), None);
        assert!(share(360, TEXT, DROPDOWN).is_some());
    }

    #[test]
    fn a_short_title_keeps_its_control_beside_it() {
        let title = Size {
            min: 60,
            nat: 110,
            expands: true,
        };
        let slider = Size {
            min: 180,
            nat: 180,
            expands: false,
        };
        assert_eq!(share(320, title, slider), Some((320 - BESIDE - 180, 180)));
    }

    #[test]
    fn an_expanding_control_shares_the_room_left_over_like_a_box() {
        let title = Size {
            min: 60,
            nat: 100,
            expands: true,
        };
        let entry = Size {
            min: 100,
            nat: 200,
            expands: true,
        };
        assert_eq!(share(414, title, entry), Some((150, 250)));
    }
}
