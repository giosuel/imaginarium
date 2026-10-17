// SPDX-License-Identifier: AGPL-3.0-or-later

//! The app's mark: the indigo tile with a card, a connector, a dot and two
//! lines of text cut out of it (the Start mockup).

use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Affine, BezPath, Cap, Circle, Rect, RoundedRect, Shape, Stroke};
use taffy::Style;
use taffy::prelude::length;

use crate::theme::{DEFAULT_ACCENT, Token, theme};
use crate::tree::{Cx, Widget};

/// The logo, `size` logical pixels square. Its cut-outs take the panel's
/// color, so it sits on a panel. Screen readers skip it: the app's name is
/// next to it.
///
/// ```
/// # use imaginarium_ui::{Logo, Ui};
/// let mut ui = Ui::new();
/// let logo = ui.add(Logo::new(44.0));
/// # let _ = logo;
/// ```
pub struct Logo {
    size: f64,
}

impl Logo {
    /// The logo at `size` logical pixels.
    pub fn new(size: f64) -> Self {
        Self { size }
    }
}

impl Widget for Logo {
    fn style(&self) -> Style {
        Style {
            size: taffy::Size {
                width: length(self.size as f32),
                height: length(self.size as f32),
            },
            flex_shrink: 0.0,
            ..Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let bounds = cx.bounds();
        // Drawn on the mockup's 64-unit grid.
        let to = Affine::translate(bounds.origin().to_vec2()) * Affine::scale(self.size / 64.0);
        let cut = theme().color(Token::PanelBg);
        let text = cut.with_alpha(0.7);
        scene.fill(
            &(to * RoundedRect::new(0.0, 0.0, 64.0, 64.0, 14.0).to_path(0.1)),
            DEFAULT_ACCENT,
        );
        scene.fill(
            &(to * RoundedRect::new(12.0, 14.0, 30.0, 28.0, 3.0).to_path(0.1)),
            cut,
        );
        scene.fill(&(to * Circle::new((44.0, 44.0), 8.0).to_path(0.1)), cut);
        let mut connector = BezPath::new();
        connector.move_to((30.0, 21.0));
        connector.line_to((44.0, 21.0));
        connector.line_to((44.0, 36.0));
        scene.stroke_with(
            &(to * connector),
            &Stroke::new(3.0 * self.size / 64.0).with_caps(Cap::Round),
            cut,
        );
        for line in [
            Rect::new(12.0, 38.0, 28.0, 41.0),
            Rect::new(12.0, 45.0, 23.0, 48.0),
        ] {
            scene.fill(&(to * RoundedRect::from_rect(line, 1.5).to_path(0.1)), text);
        }
    }
}
