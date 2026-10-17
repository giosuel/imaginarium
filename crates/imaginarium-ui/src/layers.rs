// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layers above the window's root: popovers and menus, modal dialogs and
//! tooltips (`docs/design/shell.md` §6). The widget tree keeps them as a
//! stack ([`Ui::open_layer`](crate::Ui::open_layer)); this module has
//! their options and where they go on screen.

use imaginarium_render::kurbo::{Point, Rect, Size};

use crate::tree::WidgetId;

/// A layer of the window, from [`Ui::open_layer`](crate::Ui::open_layer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayerId(pub(crate) u64);

/// What kind of layer it is, which decides how it takes input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    /// A popover or menu: a click outside it or `Esc` closes it.
    Popover,
    /// A dialog: takes all input until it closes, over a dimmed window.
    /// `Esc` closes it unless it is opened with
    /// [`LayerOptions::dismissable`] off.
    Modal,
    /// A tooltip: shown, never takes input.
    Tooltip,
}

/// Which side of its anchor a layer opens on. It flips to the other side
/// when there isn't room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Below the anchor (menus, toolbar popovers).
    Below,
    /// Above it (a contextual toolbar).
    Above,
    /// To its left.
    Left,
    /// To its right (submenus, a tool bar's popovers when docked left).
    Right,
}

/// How a layer lines up with its anchor along the anchor's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// The layer's start edge on the anchor's (left or top).
    Start,
    /// Centered on the anchor.
    Center,
    /// The layer's end edge on the anchor's (right or bottom).
    End,
}

/// What a layer is placed next to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// A widget's box, followed as it moves; the layer closes when the
    /// widget goes away.
    Widget(WidgetId),
    /// A fixed rectangle in window coordinates (a selection's box).
    Rect(Rect),
    /// A point (a right-click).
    Point(Point),
    /// The window's center (dialogs).
    Center,
}

/// How a layer behaves and where it goes.
pub struct LayerOptions {
    pub(crate) kind: LayerKind,
    pub(crate) anchor: Anchor,
    pub(crate) placement: Placement,
    pub(crate) align: Align,
    pub(crate) gap: f64,
    pub(crate) focus_first: bool,
    pub(crate) initial_focus: Option<WidgetId>,
    pub(crate) dismissable: bool,
    pub(crate) on_close: Option<Box<dyn FnMut()>>,
    /// A dialog's dimming, if not the theme's ([`LayerOptions::scrim`]).
    pub(crate) scrim: Option<f32>,
}

impl LayerOptions {
    /// A popover below `anchor`, which takes the focus (its first focusable
    /// widget).
    pub fn popover(anchor: Anchor) -> Self {
        Self {
            kind: LayerKind::Popover,
            anchor,
            placement: Placement::Below,
            align: Align::Start,
            gap: 8.0,
            focus_first: true,
            initial_focus: None,
            dismissable: true,
            on_close: None,
            scrim: None,
        }
    }

    /// A modal dialog in the window's center.
    pub fn modal() -> Self {
        Self {
            kind: LayerKind::Modal,
            anchor: Anchor::Center,
            placement: Placement::Below,
            align: Align::Center,
            gap: 0.0,
            focus_first: true,
            initial_focus: None,
            dismissable: true,
            on_close: None,
            scrim: None,
        }
    }

    /// A tooltip below `anchor`, centered on it.
    pub fn tooltip(anchor: Anchor) -> Self {
        Self {
            kind: LayerKind::Tooltip,
            anchor,
            placement: Placement::Below,
            align: Align::Center,
            gap: 6.0,
            focus_first: false,
            initial_focus: None,
            dismissable: false,
            on_close: None,
            scrim: None,
        }
    }

    /// Which side of the anchor it opens on.
    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// How it lines up with the anchor.
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// The distance from the anchor.
    pub fn gap(mut self, gap: f64) -> Self {
        self.gap = gap;
        self
    }

    /// Whether opening moves the focus to its first focusable widget
    /// (default for popovers and dialogs). The focus goes back to where it
    /// was when the layer closes.
    pub fn focus_first(mut self, focus: bool) -> Self {
        self.focus_first = focus;
        self
    }

    /// Which widget in it takes the focus when it opens, instead of the
    /// first one (a dialog's safe default: Save, not Don't save).
    pub fn focus(mut self, widget: WidgetId) -> Self {
        self.initial_focus = Some(widget);
        self.focus_first = true;
        self
    }

    /// Whether `Esc` (and, for popovers, a click outside) closes it.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Called after it closed, however it closed (a toggle button resets
    /// its state here).
    pub fn on_close(mut self, on_close: impl FnMut() + 'static) -> Self {
        self.on_close = Some(Box::new(on_close));
        self
    }

    /// Places it at `anchor` instead (a dialog near the top, like the
    /// command palette).
    pub fn anchored(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// Dims the window behind a dialog this much (0 to 1) instead of the
    /// theme's [`Token::Scrim`](crate::Token::Scrim): the command palette
    /// dims less than a question does.
    pub fn scrim(mut self, alpha: f32) -> Self {
        self.scrim = Some(alpha.clamp(0.0, 1.0));
        self
    }
}

/// Space kept between a layer and the window's edge.
pub(crate) const EDGE: f64 = 8.0;

/// Where a layer of `size` goes next to `anchor` in `area` (the window):
/// on the asked side if it fits, else on the other side if that fits
/// better, then shifted to stay inside the window.
pub(crate) fn place(
    anchor: Rect,
    size: Size,
    area: Rect,
    placement: Placement,
    align: Align,
    gap: f64,
) -> Point {
    let inner = area.inset(-EDGE);
    let along = |start: f64, end: f64, length: f64| match align {
        Align::Start => start,
        Align::Center => (start + end - length) / 2.0,
        Align::End => end - length,
    };
    // The asked side if it fits, else the other side if that fits, else
    // whichever side has more room.
    let choose = |asked: f64,
                  other: f64,
                  asked_fits: bool,
                  other_fits: bool,
                  asked_room: f64,
                  other_room: f64| {
        if asked_fits || (!other_fits && asked_room >= other_room) {
            asked
        } else {
            other
        }
    };
    let (x, y) = match placement {
        Placement::Below | Placement::Above => {
            let below = anchor.y1 + gap;
            let above = anchor.y0 - gap - size.height;
            let (fits_below, fits_above) = (below + size.height <= inner.y1, above >= inner.y0);
            let (room_below, room_above) = (inner.y1 - below, anchor.y0 - gap - inner.y0);
            let y = if placement == Placement::Below {
                choose(below, above, fits_below, fits_above, room_below, room_above)
            } else {
                choose(above, below, fits_above, fits_below, room_above, room_below)
            };
            (along(anchor.x0, anchor.x1, size.width), y)
        }
        Placement::Left | Placement::Right => {
            let right = anchor.x1 + gap;
            let left = anchor.x0 - gap - size.width;
            let (fits_right, fits_left) = (right + size.width <= inner.x1, left >= inner.x0);
            let (room_right, room_left) = (inner.x1 - right, anchor.x0 - gap - inner.x0);
            let x = if placement == Placement::Right {
                choose(right, left, fits_right, fits_left, room_right, room_left)
            } else {
                choose(left, right, fits_left, fits_right, room_left, room_right)
            };
            (x, along(anchor.y0, anchor.y1, size.height))
        }
    };
    keep_inside(Point::new(x, y), size, area)
}

/// Moves a layer of `size` at `origin` inside `area`, `EDGE` from its
/// edges where it fits; pinned to the start edge where it doesn't (its
/// start is what people read first).
pub(crate) fn keep_inside(origin: Point, size: Size, area: Rect) -> Point {
    let inner = area.inset(-EDGE);
    let clamp = |value: f64, length: f64, lo: f64, hi: f64| value.min(hi - length).max(lo);
    Point::new(
        clamp(origin.x, size.width, inner.x0, inner.x1),
        clamp(origin.y, size.height, inner.y0, inner.y1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);
    const MENU: Size = Size::new(200.0, 160.0);

    fn at(anchor: Rect, placement: Placement, align: Align) -> Point {
        place(anchor, MENU, WINDOW, placement, align, 8.0)
    }

    #[test]
    fn opens_on_the_asked_side_when_it_fits() {
        let button = Rect::new(100.0, 40.0, 140.0, 76.0);
        assert_eq!(
            at(button, Placement::Below, Align::Start),
            Point::new(100.0, 84.0)
        );
        assert_eq!(
            at(button, Placement::Right, Align::Start),
            Point::new(148.0, 40.0)
        );
        assert_eq!(
            at(button, Placement::Below, Align::Center),
            Point::new(20.0, 84.0)
        );
        assert_eq!(
            at(button, Placement::Below, Align::End),
            Point::new(8.0, 84.0)
        );
    }

    #[test]
    fn flips_when_the_asked_side_has_no_room() {
        // A button near the bottom: the menu opens above it.
        let low = Rect::new(100.0, 540.0, 140.0, 576.0);
        assert_eq!(
            at(low, Placement::Below, Align::Start),
            Point::new(100.0, 372.0)
        );
        // Near the right edge, a submenu opens to the left.
        let right = Rect::new(700.0, 100.0, 760.0, 130.0);
        assert_eq!(
            at(right, Placement::Right, Align::Start),
            Point::new(492.0, 100.0)
        );
        // Above near the top flips below.
        let top = Rect::new(100.0, 20.0, 140.0, 50.0);
        assert_eq!(
            at(top, Placement::Above, Align::Start),
            Point::new(100.0, 58.0)
        );
    }

    #[test]
    fn stays_inside_the_window() {
        let corner = Rect::new(760.0, 10.0, 790.0, 40.0);
        let point = at(corner, Placement::Below, Align::Start);
        assert_eq!(point.x, 800.0 - EDGE - MENU.width);
        // Taller than the window: pinned to the top edge.
        let tall = place(
            Rect::new(10.0, 10.0, 20.0, 20.0),
            Size::new(100.0, 900.0),
            WINDOW,
            Placement::Below,
            Align::Start,
            8.0,
        );
        assert_eq!(tall.y, EDGE);
    }
}
