// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vendored Lucide icons (`docs/design/shell.md` §8): the names in
//! `icons.txt`, as path data that [`Icon`](crate::Icon) strokes in the
//! theme's colors. `cargo xtask icons` regenerates `lucide.rs`.

mod lucide;

pub use lucide::LUCIDE_VERSION;

/// The path data of the icon `name` (Lucide's name, e.g. `"hand"`), if it
/// is vendored.
pub fn icon_paths(name: &str) -> Option<&'static [&'static str]> {
    lucide::ICONS
        .binary_search_by(|(known, _)| (*known).cmp(name))
        .ok()
        .map(|index| lucide::ICONS[index].1)
}

/// Every vendored icon's name, sorted.
pub fn icon_names() -> impl Iterator<Item = &'static str> {
    lucide::ICONS.iter().map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use imaginarium_render::kurbo::{BezPath, Rect, Shape};

    #[test]
    fn every_listed_icon_is_vendored_and_parses() {
        let listed: Vec<&str> = include_str!("icons.txt")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        assert_eq!(
            listed.len(),
            icon_names().count(),
            "run `cargo xtask icons`"
        );
        for name in listed {
            let paths = icon_paths(name).unwrap_or_else(|| panic!("{name} isn't vendored"));
            for data in paths {
                let path = BezPath::from_svg(data).unwrap_or_else(|err| panic!("{name}: {err:?}"));
                // On Lucide's grid, with room for the stroke.
                let bounds = path.bounding_box();
                assert!(
                    Rect::new(-1.0, -1.0, 25.0, 25.0).contains_rect(bounds),
                    "{name}: {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn the_names_are_sorted_for_lookup() {
        let names: Vec<&str> = icon_names().collect();
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(icon_paths("hand").is_some());
        assert!(icon_paths("no-such-icon").is_none());
    }
}
