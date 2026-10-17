// SPDX-License-Identifier: AGPL-3.0-or-later

//! Recent boards (`shell.md` §4, the Start mockups): the files boards were
//! opened from or saved to, newest first, with a small picture of each.
//!
//! The list is kept in `recent.txt` in the app's data folder, one
//! `<unix ms>\t<path>` per line, at most [`MAX`]. It is read, with each
//! board's picture, on a thread of its own at startup, and written on one
//! after every change ([`imaginarium_settings::FileWriter`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use imaginarium_model::DocumentView;
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Affine, BezPath, Rect, RoundedRect, Shape, Size, Stroke};
use imaginarium_render::peniko::Color;
use imaginarium_ui::{Cx, Token, Widget, theme};
use taffy::prelude::length;

/// The most boards the list keeps.
pub(crate) const MAX: usize = 20;

/// The most shapes a picture draws (the largest ones beyond that are
/// enough to recognize a board by).
const MAX_SHAPES: usize = 300;

/// A board in the list.
#[derive(Clone)]
pub(crate) struct RecentBoard {
    pub(crate) path: PathBuf,
    /// When it was last opened or saved (unix milliseconds).
    pub(crate) at_ms: u64,
    /// Its picture, once read (`None` until then, or if it can't be).
    pub(crate) preview: Option<Arc<Preview>>,
}

/// The list, shared with the threads that read it and the pictures.
#[derive(Default)]
pub(crate) struct RecentState {
    /// Read at startup (or never to be).
    pub(crate) loaded: bool,
    pub(crate) boards: Vec<RecentBoard>,
    /// Counts changes, for the start window to notice.
    pub(crate) generation: u64,
    /// Wake the windows that show it.
    pub(crate) wakers: Vec<Box<dyn Fn() + Send>>,
}

impl RecentState {
    /// Something changed: the windows showing it draw again.
    pub(crate) fn changed(&mut self) {
        self.generation += 1;
        for wake in &self.wakers {
            wake();
        }
    }
}

/// The list's file as text.
pub(crate) fn to_text(boards: &[RecentBoard]) -> String {
    boards
        .iter()
        .filter_map(|board| Some(format!("{}\t{}\n", board.at_ms, board.path.to_str()?)))
        .collect()
}

/// The boards in the list's file `text` (lines that don't parse are
/// left out), newest first, at most [`MAX`], each once.
pub(crate) fn from_text(text: &str) -> Vec<RecentBoard> {
    let mut boards: Vec<RecentBoard> = text
        .lines()
        .filter_map(|line| {
            let (at, path) = line.split_once('\t')?;
            Some(RecentBoard {
                path: PathBuf::from(path),
                at_ms: at.trim().parse().ok()?,
                preview: None,
            })
        })
        .filter(|board| board.path.is_absolute())
        .collect();
    boards.sort_by_key(|board| std::cmp::Reverse(board.at_ms));
    let mut seen = std::collections::HashSet::new();
    boards.retain(|board| seen.insert(board.path.clone()));
    boards.truncate(MAX);
    boards
}

/// Now, in unix milliseconds.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// When, as the Start mockup says it: "just now", "5 minutes ago",
/// "2 hours ago", "yesterday", "3 days ago", "last week", else the date
/// ("Sep 12", with the year when it isn't this one).
pub(crate) fn when_text(at_ms: u64, now_ms: u64) -> String {
    let minutes = now_ms.saturating_sub(at_ms) / 60_000;
    let hours = minutes / 60;
    let days = hours / 24;
    match (minutes, hours, days) {
        (0, _, _) => "just now".to_owned(),
        (1, _, _) => "a minute ago".to_owned(),
        (minutes, 0, _) => format!("{minutes} minutes ago"),
        (_, 1, _) => "an hour ago".to_owned(),
        (_, hours, 0) => format!("{hours} hours ago"),
        (_, _, 1) => "yesterday".to_owned(),
        (_, _, days) if days < 7 => format!("{days} days ago"),
        (_, _, days) if days < 14 => "last week".to_owned(),
        _ => {
            let (year, month, day) = civil_date(at_ms);
            let (this_year, _, _) = civil_date(now_ms);
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            let month = MONTHS[usize::try_from(month - 1).unwrap_or(0).min(11)];
            if year == this_year {
                format!("{month} {day}")
            } else {
                format!("{month} {day}, {year}")
            }
        }
    }
}

/// The (UTC) date of a unix time: year, month (1 to 12) and day (Howard
/// Hinnant's days-to-civil).
fn civil_date(at_ms: u64) -> (i64, i64, i64) {
    let days = i64::try_from(at_ms / 86_400_000).unwrap_or(0) + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The folder a board is in, as the Start mockup says it: from the home
/// folder ("Documents/Work"), "Home" for the home folder itself, else the
/// whole path.
pub(crate) fn folder_text(path: &Path, home: Option<&Path>) -> String {
    let Some(folder) = path.parent() else {
        return String::new();
    };
    match home.and_then(|home| folder.strip_prefix(home).ok()) {
        Some(inside) if inside.as_os_str().is_empty() => "Home".to_owned(),
        Some(inside) => inside
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        None => folder.display().to_string(),
    }
}

/// A board's picture: its shapes' outlines in board space, with their
/// colors in the theme they were read for.
pub(crate) struct Preview {
    shapes: Vec<PreviewShape>,
    bounds: Rect,
}

struct PreviewShape {
    path: BezPath,
    fill: Option<Color>,
    stroke: Option<Color>,
}

impl Preview {
    /// The picture of `view` in `theme` (on any thread).
    pub(crate) fn of(view: &DocumentView, theme: &imaginarium_ui::Theme) -> Self {
        let mut shapes = Vec::new();
        for id in view.walk() {
            let Some(node) = view.node(id) else {
                continue;
            };
            if node.hidden() {
                continue;
            }
            let Some(at) = world_box(view, id) else {
                continue;
            };
            let size = Size::new(at.width(), at.height());
            let path = match node.kind() {
                imaginarium_model::NodeKind::Shape => {
                    let look = imaginarium_canvas::node_look(view, id, theme);
                    let outline = imaginarium_canvas::shape_path(node, size, look.radius);
                    shapes.push(PreviewShape {
                        path: Affine::translate(at.origin().to_vec2()) * outline.path,
                        fill: look.fill.filter(|_| outline.closed),
                        stroke: look.stroke,
                    });
                    continue;
                }
                imaginarium_model::NodeKind::Frame => {
                    let look = imaginarium_canvas::node_look(view, id, theme);
                    shapes.push(PreviewShape {
                        path: RoundedRect::from_rect(at, look.radius).to_path(0.5),
                        fill: look.fill,
                        stroke: look.stroke,
                    });
                    continue;
                }
                // Text shows as a bar, as the mockups draw it.
                imaginarium_model::NodeKind::Text => {
                    let bar = Rect::new(at.x0, at.y0, at.x1, at.y0 + at.height().min(14.0));
                    RoundedRect::from_rect(bar, 3.0).to_path(0.5)
                }
                _ => continue,
            };
            shapes.push(PreviewShape {
                path,
                fill: Some(theme.color(Token::TextSubtle).with_alpha(0.6)),
                stroke: None,
            });
        }
        // Too many: the largest ones.
        if shapes.len() > MAX_SHAPES {
            shapes.sort_by(|a, b| area(&b.path).total_cmp(&area(&a.path)));
            shapes.truncate(MAX_SHAPES);
        }
        let bounds = shapes
            .iter()
            .map(|shape| shape.path.bounding_box())
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect::ZERO);
        Self { shapes, bounds }
    }

    /// Whether there is anything to draw.
    pub(crate) fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

fn area(path: &BezPath) -> f64 {
    let bounds = path.bounding_box();
    bounds.width() * bounds.height()
}

/// A node's box in board space: its geometry moved by its parents'.
fn world_box(view: &DocumentView, node: imaginarium_model::NodeId) -> Option<Rect> {
    let geometry = view.node(node)?.geometry()?;
    let (mut x, mut y) = (geometry.x, geometry.y);
    let mut parent = view.node(node)?.parent();
    // Bounded: a cycle in a broken document stops.
    for _ in 0..64 {
        let imaginarium_model::Parent::Node(id) = parent else {
            break;
        };
        let Some(up) = view.node(id) else {
            break;
        };
        if let Some(geometry) = up.geometry() {
            x += geometry.x;
            y += geometry.y;
        }
        parent = up.parent();
    }
    Some(Rect::new(
        x,
        y,
        x + geometry.w.max(1.0),
        y + geometry.h.max(1.0),
    ))
}

/// A recent board's picture in its tile (112 px high, the Start mockup's):
/// the board's shapes fitted in, on the board's color.
pub(crate) struct PreviewTile {
    pub(crate) preview: Option<Arc<Preview>>,
}

impl Widget for PreviewTile {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            size: taffy::Size {
                width: taffy::Dimension::percent(1.0),
                height: length(112.0),
            },
            flex_shrink: 0.0,
            ..taffy::Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let theme = theme();
        let bounds = cx.bounds();
        let tile = RoundedRect::from_rect(bounds, 10.0);
        scene.fill(&tile.to_path(0.1), theme.color(Token::BoardBg));
        if let Some(preview) = self.preview.as_ref().filter(|preview| !preview.is_empty()) {
            let room = bounds.inset(-14.0);
            let size = preview.bounds.size();
            let scale = (room.width() / size.width.max(1.0))
                .min(room.height() / size.height.max(1.0))
                .min(1.0);
            let drawn = size * scale;
            let at = room.center() - (drawn.to_vec2() / 2.0);
            let fit = Affine::translate(at.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-preview.bounds.origin().to_vec2());
            scene.push_clip(&tile.to_path(0.1));
            for shape in &preview.shapes {
                let path = fit * shape.path.clone();
                if let Some(fill) = shape.fill {
                    scene.fill(&path, fill);
                }
                if let Some(stroke) = shape.stroke {
                    scene.stroke_with(&path, &Stroke::new(1.0), stroke);
                }
            }
            scene.pop();
        }
        scene.stroke_with(
            &tile.to_path(0.1),
            &Stroke::new(1.0),
            theme.color(Token::PanelBorder),
        );
    }
}

/// The dashed tile that opens another board (the Start mockup's last
/// tile); its content is laid out inside it.
pub(crate) struct DashedTile;

impl Widget for DashedTile {
    fn style(&self) -> taffy::Style {
        taffy::Style {
            size: taffy::Size {
                width: taffy::Dimension::percent(1.0),
                height: length(112.0),
            },
            flex_direction: taffy::FlexDirection::Column,
            align_items: taffy::AlignItems::CENTER,
            justify_content: taffy::JustifyContent::CENTER,
            gap: taffy::Size {
                width: length(8.0),
                height: length(8.0),
            },
            flex_shrink: 0.0,
            ..taffy::Style::default()
        }
    }

    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let tile = RoundedRect::from_rect(cx.bounds().inset(-0.5), 10.0);
        scene.stroke_with(
            &tile.to_path(0.1),
            &Stroke::new(1.0).with_dashes(0.0, [4.0, 3.0]),
            theme().color(Token::ControlBorder),
        );
    }
}

/// The list as the start window shows it, and the threads keeping it.
pub(crate) type SharedRecent = Arc<Mutex<RecentState>>;

/// Reads the list at `file` and then each board's picture, on a thread of
/// its own; boards whose file is gone leave the list.
///
/// `write` saves the list when it differs from the file (boards opened
/// before it was read, files gone).
pub(crate) fn load(
    file: PathBuf,
    shared: SharedRecent,
    theme: imaginarium_ui::Theme,
    mut write: impl FnMut(String) + Send + 'static,
) {
    let reading = shared.clone();
    let thread = std::thread::Builder::new()
        .name("recent-boards".into())
        .spawn(move || {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            // Boards whose file is gone leave the list (looked at before
            // the list is locked: a disk may be slow).
            let read: Vec<RecentBoard> = from_text(&text)
                .into_iter()
                .filter(|board| board.path.exists())
                .collect();
            // Boards opened meanwhile come first, then the file's.
            if let Ok(mut state) = reading.lock() {
                let opened: std::collections::HashSet<PathBuf> = state
                    .boards
                    .iter()
                    .map(|board| board.path.clone())
                    .collect();
                let mut boards = std::mem::take(&mut state.boards);
                boards.extend(
                    read.into_iter()
                        .filter(|board| !opened.contains(&board.path)),
                );
                boards.truncate(MAX);
                let merged = to_text(&boards);
                state.boards = boards;
                state.loaded = true;
                state.changed();
                if merged != text {
                    write(merged);
                }
            }
            let paths: Vec<PathBuf> = reading
                .lock()
                .map(|state| {
                    state
                        .boards
                        .iter()
                        .map(|board| board.path.clone())
                        .collect()
                })
                .unwrap_or_default();
            for path in paths {
                picture(&reading, &path, &theme);
            }
        });
    if thread.is_err()
        && let Ok(mut state) = shared.lock()
    {
        state.loaded = true;
    }
}

/// Reads the picture of the board at `path` (on the calling thread) into
/// the list.
pub(crate) fn picture(shared: &SharedRecent, path: &Path, theme: &imaginarium_ui::Theme) {
    let Ok(opened) =
        imaginarium_io_native::open_path(path, imaginarium_io_native::Credentials::None)
    else {
        return;
    };
    let preview = Arc::new(Preview::of(opened.document.view(), theme));
    if let Ok(mut state) = shared.lock()
        && let Some(board) = state.boards.iter_mut().find(|board| board.path == path)
    {
        board.preview = Some(preview);
        state.changed();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    const MINUTE: u64 = 60_000;
    const DAY: u64 = 24 * 60 * MINUTE;

    #[test]
    fn when_reads_as_the_mockup_says_it() {
        // 2026-10-17 12:00 UTC.
        let now = 1_792_238_400_000;
        assert_eq!(when_text(now - 10_000, now), "just now");
        assert_eq!(when_text(now - MINUTE, now), "a minute ago");
        assert_eq!(when_text(now - 5 * MINUTE, now), "5 minutes ago");
        assert_eq!(when_text(now - 70 * MINUTE, now), "an hour ago");
        assert_eq!(when_text(now - 120 * MINUTE, now), "2 hours ago");
        assert_eq!(when_text(now - DAY - MINUTE, now), "yesterday");
        assert_eq!(when_text(now - 3 * DAY, now), "3 days ago");
        assert_eq!(when_text(now - 8 * DAY, now), "last week");
        assert_eq!(when_text(now - 35 * DAY, now), "Sep 12");
        assert_eq!(when_text(now - 400 * DAY, now), "Sep 12, 2025");
        // A clock that went back: just now.
        assert_eq!(when_text(now + DAY, now), "just now");
    }

    #[test]
    fn folders_read_from_home() {
        let home = Path::new("/home/me");
        assert_eq!(
            folder_text(Path::new("/home/me/Documents/Work/Plan.imag"), Some(home)),
            "Documents/Work"
        );
        assert_eq!(
            folder_text(Path::new("/home/me/Plan.imag"), Some(home)),
            "Home"
        );
        assert_eq!(
            folder_text(Path::new("/srv/boards/Plan.imag"), Some(home)),
            "/srv/boards"
        );
    }

    #[test]
    fn the_list_round_trips_newest_first_once_each() {
        let text =
            "100\t/a/One.imag\n300\t/a/Two.imag\nbroken\n200\t/a/One.imag\n5\trelative.imag\n";
        let boards = from_text(text);
        let paths: Vec<_> = boards.iter().map(|board| board.path.clone()).collect();
        assert_eq!(
            paths,
            [PathBuf::from("/a/Two.imag"), PathBuf::from("/a/One.imag")]
        );
        assert_eq!(boards[1].at_ms, 200);
        assert_eq!(from_text(&to_text(&boards)).len(), 2);
        let many: String = (0..30).map(|n| format!("{n}\t/a/{n}.imag\n")).collect();
        assert_eq!(from_text(&many).len(), MAX);
    }
}
