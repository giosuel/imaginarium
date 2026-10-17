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
    /// The file was read (only then is it written: a list that couldn't
    /// be read is never overwritten).
    pub(crate) loaded: bool,
    pub(crate) boards: Vec<RecentBoard>,
    /// Counts changes, for the start window to notice.
    pub(crate) generation: u64,
    /// Wake the windows that show it, by the id they were given.
    wakers: Vec<(u64, Box<dyn Fn() + Send>)>,
    next_waker: u64,
    /// Pictures to read, by one worker at a time.
    queue: std::collections::VecDeque<PathBuf>,
    asked: std::collections::HashSet<PathBuf>,
    working: bool,
}

impl RecentState {
    /// Something changed: the windows showing it draw again.
    pub(crate) fn changed(&mut self) {
        self.generation += 1;
        for (_, wake) in &self.wakers {
            wake();
        }
    }

    /// Wakes a window when the list changes; the id lets it go.
    pub(crate) fn add_waker(&mut self, wake: Box<dyn Fn() + Send>) -> u64 {
        self.next_waker += 1;
        self.wakers.push((self.next_waker, wake));
        self.next_waker
    }

    /// The window `id` closed.
    pub(crate) fn remove_waker(&mut self, id: u64) {
        self.wakers.retain(|(waker, _)| *waker != id);
    }

    /// A board's picture, read by now (a closing board's own, fresh).
    pub(crate) fn set_picture(&mut self, path: &Path, preview: Preview) {
        if let Some(board) = self.boards.iter_mut().find(|board| board.path == path) {
            board.preview = Some(Arc::new(preview));
            self.asked.insert(path.to_owned());
            self.changed();
        }
    }
}

/// `path` made absolute and without `.` and `..`, the same way wherever it
/// came from (the command line, a dialog), so a board is in the list once.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let mut out = PathBuf::new();
    for part in absolute.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The boards opened before the file was read (`opened`) first, then the
/// file's, each once, at most [`MAX`].
pub(crate) fn merge(opened: Vec<RecentBoard>, read: Vec<RecentBoard>) -> Vec<RecentBoard> {
    let seen: std::collections::HashSet<PathBuf> =
        opened.iter().map(|board| board.path.clone()).collect();
    let mut boards = opened;
    boards.extend(read.into_iter().filter(|board| !seen.contains(&board.path)));
    boards.truncate(MAX);
    boards
}

/// The list's file as text.
pub(crate) fn to_text(boards: &[RecentBoard]) -> String {
    boards
        .iter()
        .filter_map(|board| {
            let path = board.path.to_str().filter(|path| !path.contains('\n'))?;
            Some(format!("{}\t{path}\n", board.at_ms))
        })
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
/// colors in both themes (dark first), so it follows a theme switch.
pub(crate) struct Preview {
    shapes: Vec<PreviewShape>,
    bounds: Rect,
}

struct PreviewShape {
    path: BezPath,
    /// The frames it is inside, which clip it (board space).
    clips: Vec<BezPath>,
    /// The fill and outline in the dark theme, then the light one.
    fill: [Option<Color>; 2],
    stroke: [Option<Color>; 2],
}

impl Preview {
    /// The picture of `view` (on any thread), walked as the board draws
    /// it: hidden subtrees left out, rotations and frames' clips kept.
    pub(crate) fn of(view: &DocumentView) -> Self {
        let themes = [
            imaginarium_ui::Theme::dark(imaginarium_ui::DEFAULT_ACCENT),
            imaginarium_ui::Theme::light(imaginarium_ui::DEFAULT_ACCENT),
        ];
        let mut shapes = Vec::new();
        for id in view.children(imaginarium_model::Parent::Root) {
            walk(view, id, Affine::IDENTITY, &[], &themes, &mut shapes);
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

/// The transform from a node's box to its parent's space: to its corner,
/// then rotated around its center (schema-v1.md §5.2, as the board).
fn local_transform(x: f64, y: f64, w: f64, h: f64, degrees: f64) -> Affine {
    let mut local = Affine::translate((x, y));
    if degrees != 0.0 && degrees.is_finite() {
        local *= Affine::rotate_about(
            degrees.to_radians(),
            imaginarium_render::kurbo::Point::new(w / 2.0, h / 2.0),
        );
    }
    local
}

/// Adds `id` and what is inside it.
fn walk(
    view: &DocumentView,
    id: imaginarium_model::NodeId,
    parent: Affine,
    clips: &[BezPath],
    themes: &[imaginarium_ui::Theme; 2],
    shapes: &mut Vec<PreviewShape>,
) {
    // Deep enough for any real board, and a stop for a broken one.
    if clips.len() > 32 || shapes.len() > MAX_SHAPES * 4 {
        return;
    }
    let Some(node) = view.node(id) else {
        return;
    };
    if node.hidden() {
        return;
    }
    let geometry = node.geometry().unwrap_or_default();
    let size = Size::new(geometry.w.max(0.0), geometry.h.max(0.0));
    let world =
        parent * local_transform(geometry.x, geometry.y, size.width, size.height, geometry.r);
    let looks = themes
        .each_ref()
        .map(|theme| imaginarium_canvas::node_look(view, id, theme));
    let mut inner = clips.to_vec();
    match node.kind() {
        imaginarium_model::NodeKind::Shape => {
            let outline = imaginarium_canvas::shape_path(node, size, looks[0].radius);
            shapes.push(PreviewShape {
                path: world * outline.path,
                clips: clips.to_vec(),
                fill: looks.map(|look| look.fill.filter(|_| outline.closed)),
                stroke: looks.map(|look| look.stroke),
            });
        }
        imaginarium_model::NodeKind::Frame => {
            let rect =
                RoundedRect::from_rect(Rect::from_origin_size((0.0, 0.0), size), looks[0].radius);
            let path = world * rect.to_path(0.5);
            shapes.push(PreviewShape {
                path: path.clone(),
                clips: clips.to_vec(),
                fill: looks.map(|look| look.fill),
                stroke: looks.map(|look| look.stroke),
            });
            inner.push(path);
        }
        // Text shows as a bar, as the mockups draw it.
        imaginarium_model::NodeKind::Text => {
            let bar = Rect::new(0.0, 0.0, size.width.max(1.0), size.height.clamp(1.0, 14.0));
            shapes.push(PreviewShape {
                path: world * RoundedRect::from_rect(bar, 3.0).to_path(0.5),
                clips: clips.to_vec(),
                fill: themes
                    .each_ref()
                    .map(|theme| Some(theme.color(Token::TextSubtle).with_alpha(0.6))),
                stroke: [None, None],
            });
        }
        _ => {}
    }
    for child in view.children(imaginarium_model::Parent::Node(id)) {
        walk(view, child, world, &inner, themes, shapes);
    }
}

fn area(path: &BezPath) -> f64 {
    let bounds = path.bounding_box();
    bounds.width() * bounds.height()
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
            let which = usize::from(theme.kind() != imaginarium_ui::ThemeKind::Dark);
            scene.push_clip(&tile.to_path(0.1));
            for shape in &preview.shapes {
                for clip in &shape.clips {
                    scene.push_clip(&(fit * clip.clone()));
                }
                let path = fit * shape.path.clone();
                if let Some(fill) = shape.fill[which] {
                    scene.fill(&path, fill);
                }
                if let Some(stroke) = shape.stroke[which] {
                    scene.stroke_with(&path, &Stroke::new(1.0), stroke);
                }
                for _ in &shape.clips {
                    scene.pop();
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

/// Reads the list at `file` on a thread of its own (the pictures come when
/// a start window shows them: [`want_pictures`]). Boards whose file isn't
/// there stay (a drive may be away); a click on one that is gone
/// forgets it. Only a file that isn't there counts as an empty list:
/// one that can't be read is never overwritten.
///
/// `write` saves the list when it differs from the file (boards opened
/// before it was read).
pub(crate) fn load(
    file: PathBuf,
    shared: SharedRecent,
    mut write: impl FnMut(String) + Send + 'static,
) {
    let reading = shared.clone();
    let thread = std::thread::Builder::new()
        .name("recent-boards".into())
        .spawn(move || {
            let text = match std::fs::read_to_string(&file) {
                Ok(text) => text,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(_) => return,
            };
            if let Ok(mut state) = reading.lock() {
                let opened = std::mem::take(&mut state.boards);
                state.boards = merge(opened, from_text(&text));
                state.loaded = true;
                let merged = to_text(&state.boards);
                state.changed();
                if merged != text {
                    write(merged);
                }
            }
        });
    drop(thread);
}

/// Reads the pictures of `paths` not read or asked for yet, one at a time
/// on a worker thread.
pub(crate) fn want_pictures(shared: &SharedRecent, paths: &[PathBuf]) {
    let start = {
        let Ok(mut state) = shared.lock() else {
            return;
        };
        for path in paths {
            if state.asked.insert(path.clone()) {
                state.queue.push_back(path.clone());
            }
        }
        let start = !state.working && !state.queue.is_empty();
        state.working |= start;
        start
    };
    if !start {
        return;
    }
    let shared = shared.clone();
    let worker = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("recent-pictures".into())
        .spawn(move || {
            loop {
                let next = worker.lock().ok().and_then(|mut state| {
                    let next = state.queue.pop_front();
                    if next.is_none() {
                        state.working = false;
                    }
                    next
                });
                let Some(path) = next else {
                    break;
                };
                picture(&worker, &path);
            }
        });
    if spawned.is_err()
        && let Ok(mut state) = shared.lock()
    {
        state.working = false;
    }
}

/// Reads the picture of the board at `path` (on the calling thread) into
/// the list.
fn picture(shared: &SharedRecent, path: &Path) {
    let Ok(opened) =
        imaginarium_io_native::open_path(path, imaginarium_io_native::Credentials::None)
    else {
        return;
    };
    let preview = Preview::of(opened.document.view());
    if let Ok(mut state) = shared.lock() {
        state.set_picture(path, preview);
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
    fn boards_opened_before_the_file_was_read_come_first() {
        let board = |path: &str, at_ms| RecentBoard {
            path: PathBuf::from(path),
            at_ms,
            preview: None,
        };
        let merged = merge(
            vec![board("/a/New.imag", 900)],
            vec![board("/a/Old.imag", 100), board("/a/New.imag", 50)],
        );
        let paths: Vec<_> = merged.iter().map(|board| board.path.clone()).collect();
        assert_eq!(
            paths,
            [PathBuf::from("/a/New.imag"), PathBuf::from("/a/Old.imag")]
        );
        assert_eq!(merged[0].at_ms, 900);
    }

    #[test]
    fn paths_are_kept_the_same_way_wherever_they_come_from() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./Plan.imag")),
            PathBuf::from("/a/c/Plan.imag")
        );
        // A newline would break the file's lines: such a path is left out.
        let odd = RecentBoard {
            path: PathBuf::from("/a/two\nlines.imag"),
            at_ms: 1,
            preview: None,
        };
        assert_eq!(to_text(&[odd]), "");
    }

    #[test]
    fn pictures_leave_hidden_things_out_and_turn_with_their_shapes() {
        use imaginarium_model::{Actor, Document, DocumentKind, Geometry, NodeKind, Parent};
        let mut document = Document::new(DocumentKind::Board).unwrap();
        document
            .transact(&Actor::User, |t| {
                let bar = t.create_node(Parent::Root, None, &NodeKind::Shape)?;
                t.set_geometry(
                    bar,
                    Geometry {
                        x: 0.0,
                        y: 0.0,
                        w: 200.0,
                        h: 10.0,
                        r: 90.0,
                    },
                )?;
                let hidden = t.create_node(Parent::Root, None, &NodeKind::Frame)?;
                t.set_geometry(hidden, Geometry::new(500.0, 500.0, 100.0, 100.0))?;
                t.set_prop(hidden, "hidden", imaginarium_model::Value::Bool(true))?;
                let inside = t.create_node(Parent::Node(hidden), None, &NodeKind::Shape)?;
                t.set_geometry(inside, Geometry::new(10.0, 10.0, 20.0, 20.0))
            })
            .unwrap();
        let preview = Preview::of(document.view());
        // Only the bar, standing up: 10 wide, 200 high.
        assert_eq!(preview.shapes.len(), 1);
        assert!(
            (preview.bounds.width() - 10.0).abs() < 0.5,
            "{:?}",
            preview.bounds
        );
        assert!(
            (preview.bounds.height() - 200.0).abs() < 0.5,
            "{:?}",
            preview.bounds
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
