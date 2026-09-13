//! What has keyboard focus: a Wayland surface, or an X11 window, which also needs the X server's
//! input focus to receive keys. The pointer aims at the same type, which is what lets a menu
//! take a popup grab: Smithay's grab moves the pointer's focus to whatever the keyboard's is.

use std::{borrow::Cow, sync::Arc};

use smithay::{
    backend::input::{InputTime, KeyState},
    desktop::{PopupKind, Window},
    input::{
        Seat,
        dnd::{DndFocus, OfferData, Source},
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            PointerTarget, RelativeMotionEvent,
        },
    },
    reexports::wayland_server::{DisplayHandle, protocol::wl_surface::WlSurface},
    utils::{IsAlive, Logical, Point, Serial},
    wayland::{seat::WaylandFocus, selection::data_device::WlOfferData},
    xwayland::{X11Surface, xwm::XwmOfferData},
};

use crate::Slipstream;

// X11Surface is large, but focus changes rarely and is cloned rarely; boxing buys nothing here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum KeyboardFocus {
    Wayland(WlSurface),
    X11(X11Surface),
}

impl KeyboardFocus {
    pub fn for_window(window: &Window) -> Option<Self> {
        if let Some(toplevel) = window.toplevel() {
            return Some(Self::Wayland(toplevel.wl_surface().clone()));
        }
        window
            .x11_surface()
            .map(|surface| Self::X11(surface.clone()))
    }

    pub fn is_window(&self, window: &Window) -> bool {
        match self {
            Self::Wayland(surface) => window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == surface),
            Self::X11(surface) => window.x11_surface() == Some(surface),
        }
    }
}

impl From<PopupKind> for KeyboardFocus {
    /// A popup grabbing the keyboard focuses its own surface.
    fn from(popup: PopupKind) -> Self {
        Self::Wayland(popup.wl_surface().clone())
    }
}

impl From<WlSurface> for KeyboardFocus {
    fn from(surface: WlSurface) -> Self {
        Self::Wayland(surface)
    }
}

impl IsAlive for KeyboardFocus {
    fn alive(&self) -> bool {
        match self {
            Self::Wayland(surface) => surface.alive(),
            Self::X11(surface) => surface.alive(),
        }
    }
}

impl WaylandFocus for KeyboardFocus {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Wayland(surface) => Some(Cow::Borrowed(surface)),
            Self::X11(surface) => surface.wl_surface().map(Cow::Owned),
        }
    }
}

impl KeyboardTarget<Slipstream> for KeyboardFocus {
    fn enter(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::enter(surface, seat, data, keys, serial),
            Self::X11(surface) => KeyboardTarget::enter(surface, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<Slipstream>, data: &mut Slipstream, serial: Serial) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::leave(surface, seat, data, serial),
            Self::X11(surface) => KeyboardTarget::leave(surface, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: InputTime,
    ) {
        match self {
            Self::Wayland(surface) => {
                KeyboardTarget::key(surface, seat, data, key, state, serial, time)
            }
            Self::X11(surface) => {
                KeyboardTarget::key(surface, seat, data, key, state, serial, time)
            }
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => {
                KeyboardTarget::modifiers(surface, seat, data, modifiers, serial)
            }
            Self::X11(surface) => KeyboardTarget::modifiers(surface, seat, data, modifiers, serial),
        }
    }
}

impl KeyboardFocus {
    /// The surface the pointer's events really go to. Delegating through one `&dyn` keeps the
    /// sixteen methods below to one line each.
    fn pointer_target(&self) -> &dyn PointerTarget<Slipstream> {
        match self {
            Self::Wayland(surface) => surface,
            Self::X11(surface) => surface,
        }
    }
}

/// Adapted from Smithay's `anvil` example (MIT), which aims the pointer at the same focus type.
impl PointerTarget<Slipstream> for KeyboardFocus {
    fn enter(&self, seat: &Seat<Slipstream>, data: &mut Slipstream, event: &MotionEvent) {
        self.pointer_target().enter(seat, data, event)
    }

    fn motion(&self, seat: &Seat<Slipstream>, data: &mut Slipstream, event: &MotionEvent) {
        self.pointer_target().motion(seat, data, event)
    }

    fn relative_motion(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        event: &RelativeMotionEvent,
    ) {
        self.pointer_target().relative_motion(seat, data, event)
    }

    fn button(&self, seat: &Seat<Slipstream>, data: &mut Slipstream, event: &ButtonEvent) {
        self.pointer_target().button(seat, data, event)
    }

    fn axis(&self, seat: &Seat<Slipstream>, data: &mut Slipstream, frame: AxisFrame) {
        self.pointer_target().axis(seat, data, frame)
    }

    fn frame(&self, seat: &Seat<Slipstream>, data: &mut Slipstream) {
        self.pointer_target().frame(seat, data)
    }

    fn gesture_swipe_begin(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        event: &GestureSwipeBeginEvent,
    ) {
        self.pointer_target().gesture_swipe_begin(seat, data, event)
    }

    fn gesture_swipe_update(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        event: &GestureSwipeUpdateEvent,
    ) {
        self.pointer_target()
            .gesture_swipe_update(seat, data, event)
    }

    fn gesture_swipe_end(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        event: &GestureSwipeEndEvent,
    ) {
        self.pointer_target().gesture_swipe_end(seat, data, event)
    }

    fn gesture_pinch_begin(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        event: &GesturePinchBeginEvent,
    ) {
        self.pointer_target().gesture_pinch_begin(seat, data, event)
    }

    fn gesture_pinch_update(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        event: &GesturePinchUpdateEvent,
    ) {
        self.pointer_target()
            .gesture_pinch_update(seat, data, event)
    }

    fn gesture_pinch_end(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        event: &GesturePinchEndEvent,
    ) {
        self.pointer_target().gesture_pinch_end(seat, data, event)
    }

    fn gesture_hold_begin(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        event: &GestureHoldBeginEvent,
    ) {
        self.pointer_target().gesture_hold_begin(seat, data, event)
    }

    fn gesture_hold_end(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        event: &GestureHoldEndEvent,
    ) {
        self.pointer_target().gesture_hold_end(seat, data, event)
    }

    fn leave(
        &self,
        seat: &Seat<Slipstream>,
        data: &mut Slipstream,
        serial: Serial,
        time: InputTime,
    ) {
        self.pointer_target().leave(seat, data, serial, time)
    }
}

/// What a drag-and-drop offer is being held by, matching the focus it was offered to.
pub enum Offer<S: Source> {
    Wayland(WlOfferData<S>),
    X11(XwmOfferData<S>),
}

impl<S: Source> OfferData for Offer<S> {
    fn disable(&self) {
        match self {
            Self::Wayland(data) => data.disable(),
            Self::X11(data) => data.disable(),
        }
    }

    fn drop(&self) {
        match self {
            Self::Wayland(data) => data.drop(),
            Self::X11(data) => data.drop(),
        }
    }

    fn validated(&self) -> bool {
        match self {
            Self::Wayland(data) => data.validated(),
            Self::X11(data) => data.validated(),
        }
    }
}

/// Dragging onto a window, once the pointer aims at this type. An offer made to one kind of
/// surface can only be answered by that kind, so a mismatched pair drops the event.
impl DndFocus<Slipstream> for KeyboardFocus {
    type OfferData<S>
        = Offer<S>
    where
        S: Source;

    fn enter<S: Source>(
        &self,
        data: &mut Slipstream,
        dh: &DisplayHandle,
        source: Arc<S>,
        seat: &Seat<Slipstream>,
        location: Point<f64, Logical>,
        serial: &Serial,
    ) -> Option<Offer<S>> {
        match self {
            Self::Wayland(surface) => {
                DndFocus::enter(surface, data, dh, source, seat, location, serial)
                    .map(Offer::Wayland)
            }
            Self::X11(surface) => {
                DndFocus::enter(surface, data, dh, source, seat, location, serial).map(Offer::X11)
            }
        }
    }

    fn motion<S: Source>(
        &self,
        data: &mut Slipstream,
        offer: Option<&mut Offer<S>>,
        seat: &Seat<Slipstream>,
        location: Point<f64, Logical>,
        time: InputTime,
    ) {
        match self {
            Self::Wayland(surface) => {
                let offer = match offer {
                    Some(Offer::Wayland(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::motion(surface, data, offer, seat, location, time)
            }
            Self::X11(surface) => {
                let offer = match offer {
                    Some(Offer::X11(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::motion(surface, data, offer, seat, location, time)
            }
        }
    }

    fn leave<S: Source>(
        &self,
        data: &mut Slipstream,
        offer: Option<&mut Offer<S>>,
        seat: &Seat<Slipstream>,
    ) {
        match self {
            Self::Wayland(surface) => {
                let offer = match offer {
                    Some(Offer::Wayland(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::leave(surface, data, offer, seat)
            }
            Self::X11(surface) => {
                let offer = match offer {
                    Some(Offer::X11(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::leave(surface, data, offer, seat)
            }
        }
    }

    fn drop<S: Source>(
        &self,
        data: &mut Slipstream,
        offer: Option<&mut Offer<S>>,
        seat: &Seat<Slipstream>,
    ) {
        match self {
            Self::Wayland(surface) => {
                let offer = match offer {
                    Some(Offer::Wayland(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::drop(surface, data, offer, seat)
            }
            Self::X11(surface) => {
                let offer = match offer {
                    Some(Offer::X11(offer)) => Some(offer),
                    None => None,
                    _ => return,
                };
                DndFocus::drop(surface, data, offer, seat)
            }
        }
    }
}
