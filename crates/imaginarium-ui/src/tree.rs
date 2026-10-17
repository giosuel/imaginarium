// SPDX-License-Identifier: AGPL-3.0-or-later

//! The retained widget tree: widgets in an arena, laid out by Taffy, each
//! with a cached scene, plus hit testing, hover, pointer capture, focus and
//! the accessibility tree.

use std::any::Any;
use std::cell::RefCell;
use std::time::{Duration, Instant};

use accesskit::{Action, ActionRequest, Node, NodeId};
use imaginarium_reactive::Effect;
use imaginarium_render::Scene;
use imaginarium_render::kurbo::{Point, Rect, Size, Vec2};
use imaginarium_text::Fonts;
use taffy::{AvailableSpace, TaffyTree};

use crate::access::AccessTree;
use crate::clipboard::Clipboard;
use crate::layers::{Anchor, LayerId, LayerKind, LayerOptions, place as place_layer};
use crate::window::{Input, Key, request_frame};

/// A widget in a [`Ui`]. Ids of removed widgets are never reused for
/// another widget (generations).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WidgetId {
    index: u32,
    generation: u32,
}

/// How a widget's accessibility node ids are laid out in 64 bits: the
/// slot (index + 1) in the top 24 bits, the low 16 bits of its generation
/// below that, and 24 bits for the widget's own extra nodes.
const ACCESS_INDEX_SHIFT: u32 = 40;
const ACCESS_GENERATION_SHIFT: u32 = 24;
const ACCESS_GENERATION_MASK: u64 = 0xffff;
/// Extra nodes a widget may number below its base (text runs, canvas
/// items): 2²⁴ − 1.
pub const ACCESS_LOCAL_IDS: u64 = (1 << ACCESS_GENERATION_SHIFT) - 1;

impl WidgetId {
    /// The first accessibility node id of this widget. A widget's own node
    /// is this; it may use the next [`ACCESS_LOCAL_IDS`] ids for nodes of
    /// its own (text runs, canvas items). The id carries the widget's
    /// generation, so a screen reader's late request about a removed
    /// widget can't reach the widget that took its slot.
    pub fn access_base(self) -> u64 {
        debug_assert!(u64::from(self.index) + 1 < 1 << (64 - ACCESS_INDEX_SHIFT));
        ((u64::from(self.index) + 1) << ACCESS_INDEX_SHIFT)
            | ((u64::from(self.generation) & ACCESS_GENERATION_MASK) << ACCESS_GENERATION_SHIFT)
    }

    /// The widget an accessibility node id belongs to, as far as the id
    /// says (its slot and generation bits).
    fn from_access(node: u64) -> Option<(u32, u64)> {
        let index = (node >> ACCESS_INDEX_SHIFT).checked_sub(1)?;
        let generation = (node >> ACCESS_GENERATION_SHIFT) & ACCESS_GENERATION_MASK;
        Some((u32::try_from(index).ok()?, generation))
    }
}

/// A piece of UI. Everything has a default except painting: a widget that
/// only lays out its children is a [`Flex`](crate::Flex).
///
/// Coordinates are window coordinates in logical pixels; [`Cx::bounds`]
/// is the widget's box from the last layout.
pub trait Widget: Any {
    /// How the widget is laid out (Taffy's CSS-like style: size, flex,
    /// padding, gap, position).
    fn style(&self) -> taffy::Style {
        taffy::Style::default()
    }

    /// The size of a leaf (a widget without children) for the space
    /// available, e.g. its text's size.
    fn measure(
        &mut self,
        cx: &mut Cx<'_>,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<AvailableSpace>,
    ) -> Size {
        let _ = (cx, known, available);
        Size::ZERO
    }

    /// Draws the widget itself (its children draw after it, on top).
    fn paint(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let _ = (cx, scene);
    }

    /// Handles an input: pointer input aimed at it (or captured by it),
    /// keys while it or a descendant has the focus. Return `true` if
    /// handled; unhandled input bubbles to the parent.
    fn event(&mut self, cx: &mut Cx<'_>, input: &Input) -> bool {
        let _ = (cx, input);
        false
    }

    /// Whether a click or Tab can give it the keyboard focus.
    fn focusable(&self) -> bool {
        false
    }

    /// Whether a click gives it the focus (when [`Widget::focusable`]).
    /// Tool bar buttons say no: a click on a tool leaves the keyboard
    /// where it was (the board), while Tab still reaches them.
    fn focus_on_click(&self) -> bool {
        true
    }

    /// Whether the pointer goes through it to what is below, where none
    /// of its children is: an overlay laying out panels over the board.
    fn passes_pointer(&self) -> bool {
        false
    }

    /// It gained or lost the focus.
    fn focus_changed(&mut self, cx: &mut Cx<'_>, focused: bool) {
        let _ = (cx, focused);
    }

    /// The pointer entered or left it.
    fn hover_changed(&mut self, cx: &mut Cx<'_>, hovered: bool) {
        let _ = (cx, hovered);
    }

    /// Whether children are clipped to its bounds.
    fn clips(&self) -> bool {
        false
    }

    /// How far its children are moved up and left (a scroll container's
    /// offset). Their boxes are placed shifted by it.
    fn scroll_offset(&self) -> Vec2 {
        Vec2::ZERO
    }

    /// Whether it takes pointer input at `point` before its children (a
    /// scroll bar drawn over its content). `bounds` and `content` are its
    /// box and [`Cx::content_size`].
    fn claims(&self, bounds: Rect, content: Size, point: Point) -> bool {
        let _ = (bounds, content, point);
        false
    }

    /// Whether it draws over its children ([`Widget::paint_over`]).
    fn paints_over(&self) -> bool {
        false
    }

    /// Draws over its children (a scroll bar), every frame, after them.
    fn paint_over(&mut self, cx: &mut Cx<'_>, scene: &mut Scene) {
        let _ = (cx, scene);
    }

    /// Its box and content size after a layout, before its children are
    /// placed (a scroll container keeps its offset in range here).
    fn placed(&mut self, bounds: Rect, content: Size) {
        let _ = (bounds, content);
    }

    /// The tree took the pointer capture away (the window lost the focus,
    /// a dialog opened): a drag in progress ends.
    fn capture_lost(&mut self, cx: &mut Cx<'_>) {
        let _ = cx;
    }

    /// Asked to bring `target` (window coordinates, a descendant's box)
    /// into view, as when the focus moves to it. `true` if it moved.
    fn reveal(&mut self, cx: &mut Cx<'_>, target: Rect) -> bool {
        let _ = (cx, target);
        false
    }

    /// Its accessibility node (`None`: transparent, its children belong to
    /// the parent's node). Extra nodes (text runs, items it draws itself)
    /// go into [`Cx::access_tree`] with ids from [`WidgetId::access_base`],
    /// listed with [`Cx::add_access_child`].
    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let _ = cx;
        None
    }

    /// What a tooltip says when the pointer rests on it (its name and its
    /// key, "Rectangle · R"). The tree shows it after a moment
    /// ([`TOOLTIP_DELAY`]) and hides it when the pointer moves on or
    /// presses.
    fn tooltip(&self) -> Option<String> {
        None
    }

    /// Which side of it its tooltip shows on (below by default; above for
    /// controls at the window's bottom).
    fn tooltip_placement(&self) -> crate::layers::Placement {
        crate::layers::Placement::Below
    }

    /// A screen reader's request aimed at one of its nodes. `true` if it
    /// did something. (Focus is handled by the tree, which gives the widget
    /// the keyboard focus; a focus on one of the widget's own nodes then
    /// comes here too, so a board can select the item.)
    fn access_action(&mut self, cx: &mut Cx<'_>, request: &ActionRequest) -> bool {
        let _ = (cx, request);
        false
    }

    /// Where the input method's window goes while it has the focus (`None`:
    /// no text input).
    fn ime_area(&mut self, cx: &mut Cx<'_>) -> Option<Rect> {
        let _ = cx;
        None
    }

    /// When it wants to be woken without input (a caret blink).
    fn wake_at(&self) -> Option<Instant> {
        None
    }

    /// The time from [`Widget::wake_at`] came.
    fn wake(&mut self, cx: &mut Cx<'_>, now: Instant) {
        let _ = (cx, now);
    }
}

/// What a widget asked for during a call.
#[derive(Debug, Default)]
struct Requests {
    paint: bool,
    layout: bool,
    place: bool,
    focus: Option<bool>,
    capture: Option<bool>,
    close_layer: bool,
    access_children: Vec<NodeId>,
}

/// A widget's view of the tree during a call.
pub struct Cx<'a> {
    id: WidgetId,
    bounds: Rect,
    content: Size,
    focused: bool,
    focus_visible: bool,
    hovered: bool,
    fonts: &'a mut Fonts,
    clipboard: &'a mut Clipboard,
    access: Option<&'a mut AccessTree>,
    requests: Requests,
}

impl Cx<'_> {
    /// This widget.
    pub fn id(&self) -> WidgetId {
        self.id
    }

    /// Its box from the last layout, in window coordinates.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// How far its children reach from its top left corner, before any
    /// scroll offset: the size a scroll container scrolls through.
    pub fn content_size(&self) -> Size {
        self.content
    }

    /// Whether it has the keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Whether it has the focus and the keyboard is in use, so a focus
    /// ring should show (CSS's `:focus-visible`). A click focuses without
    /// a ring; the next key press brings it.
    pub fn is_focus_visible(&self) -> bool {
        self.focus_visible
    }

    /// Whether the pointer is over it.
    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// The fonts, for text layout.
    pub fn fonts(&mut self) -> &mut Fonts {
        self.fonts
    }

    /// The system clipboard.
    pub fn clipboard(&mut self) -> &mut Clipboard {
        self.clipboard
    }

    /// The fonts and the clipboard at once (a text field needs both).
    pub fn fonts_and_clipboard(&mut self) -> (&mut Fonts, &mut Clipboard) {
        (self.fonts, self.clipboard)
    }

    /// The fonts and the accessibility tree being built at once.
    pub fn fonts_and_access(&mut self) -> (&mut Fonts, Option<&mut AccessTree>) {
        (self.fonts, self.access.as_deref_mut())
    }

    /// Its picture changed: paint it again.
    pub fn request_paint(&mut self) {
        self.requests.paint = true;
    }

    /// Its size or style changed: lay out (and paint) again.
    pub fn request_layout(&mut self) {
        self.requests.layout = true;
        self.requests.paint = true;
    }

    /// Its children moved without changing size (a scroll offset): place
    /// the boxes again, which reuses the layout.
    pub fn request_place(&mut self) {
        self.requests.place = true;
    }

    /// Takes the keyboard focus.
    pub fn request_focus(&mut self) {
        self.requests.focus = Some(true);
    }

    /// Gives the focus up (to nobody).
    pub fn resign_focus(&mut self) {
        self.requests.focus = Some(false);
    }

    /// Receives all pointer input until [`Cx::release_pointer`] (a drag
    /// leaving its bounds). A press captures automatically until release.
    pub fn capture_pointer(&mut self) {
        self.requests.capture = Some(true);
    }

    /// Ends [`Cx::capture_pointer`].
    pub fn release_pointer(&mut self) {
        self.requests.capture = Some(false);
    }

    /// Closes the layer this widget is in (a menu item that ran, a dialog's
    /// Cancel), and the layers opened after it.
    pub fn close_layer(&mut self) {
        self.requests.close_layer = true;
    }

    /// The accessibility tree being built (only in
    /// [`Widget::accessibility`]).
    pub fn access_tree(&mut self) -> Option<&mut AccessTree> {
        self.access.as_deref_mut()
    }

    /// Lists an extra node (added to [`Cx::access_tree`]) as a child of this
    /// widget's node, before its child widgets.
    pub fn add_access_child(&mut self, id: NodeId) {
        self.requests.access_children.push(id);
    }
}

thread_local! {
    /// Widgets whose signals changed: (widget, needs layout).
    static PENDING: RefCell<Vec<(WidgetId, bool)>> = const { RefCell::new(Vec::new()) };
}

/// How long the pointer rests on a widget before its tooltip shows
/// (`shell.md` §3.3).
pub const TOOLTIP_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// Marks `id` for repainting (and re-layout if `layout`), and asks for a
/// frame. Effects call this; it is safe while the tree is borrowed.
pub fn invalidate(id: WidgetId, layout: bool) {
    PENDING.with(|pending| pending.borrow_mut().push((id, layout)));
    request_frame();
}

struct NodeData {
    widget: Box<dyn Widget>,
    parent: Option<WidgetId>,
    children: Vec<WidgetId>,
    layout: taffy::NodeId,
    bounds: Rect,
    /// How far its children reach (see [`Cx::content_size`]).
    content: Size,
    /// The widget's own drawing, kept until it is invalidated.
    scene: Scene,
    paint_dirty: bool,
    /// Effects that invalidate this widget, disposed with it.
    effects: Vec<Effect>,
}

struct Slot {
    generation: u32,
    node: Option<NodeData>,
}

/// A [`VirtualList`](crate::VirtualList)'s rows and how to make more.
struct ListState {
    scroll: WidgetId,
    body: WidgetId,
    spacer: WidgetId,
    row_height: f64,
    overscan: usize,
    count: std::rc::Rc<dyn Fn() -> usize>,
    /// Taken out while it builds a row (it gets the tree).
    build: Option<crate::scroll::RowBuilder>,
    /// Built rows: index → their slot.
    rows: std::collections::BTreeMap<usize, WidgetId>,
    /// Something the count closure reads changed: rebuild the rows, whose
    /// content may come from the same data.
    stale: std::rc::Rc<std::cell::Cell<bool>>,
    /// A row to bring into view once the view has a height.
    pending_row: Option<usize>,
}

/// An open layer: its content's root and how it behaves.
struct Layer {
    id: LayerId,
    root: WidgetId,
    options: LayerOptions,
    /// The focus when it opened, which gets it back when it closes.
    return_focus: Option<WidgetId>,
    /// Moves the focus into it after its first layout.
    focus_pending: bool,
    /// Not laid out yet (a dialog ends drags and hover below it then).
    fresh: bool,
    /// Fades in from 0 to 1 as it opens.
    appear: crate::animation::Transition,
}

/// What the last frame cost the tree, for the gate report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TreeStats {
    /// Taffy layout, when it ran.
    pub layout: Option<Duration>,
    /// Widgets that painted again (the others reused their scenes).
    pub repainted: usize,
    /// Widgets in the tree.
    pub widgets: usize,
}

/// A tree of widgets: the toolkit's retained UI.
pub struct Ui {
    slots: Vec<Slot>,
    free: Vec<u32>,
    taffy: TaffyTree<WidgetId>,
    root: Option<WidgetId>,
    focus: Option<WidgetId>,
    hover: Option<WidgetId>,
    /// A widget with a tooltip under the pointer since then.
    tip_wait: Option<(WidgetId, Instant)>,
    /// The tooltip shown, and the widget it is for.
    tip: Option<(WidgetId, LayerId)>,
    capture: Option<WidgetId>,
    /// The press that started a capture (implicit capture ends once every
    /// button is up).
    implicit_capture: bool,
    /// Pointer buttons held down.
    buttons_held: u8,
    area: Rect,
    layout_dirty: bool,
    /// Widgets changed through [`Ui::widget_mut`], whose style is read
    /// again at the next layout (after the change).
    restyle: Vec<WidgetId>,
    clipboard: Clipboard,
    stats: TreeStats,
    /// The theme generation the widgets last painted with.
    theme_seen: u64,
    /// Popovers, dialogs and tooltips above the root, bottom to top.
    layers: Vec<Layer>,
    next_layer: u64,
    /// Inside `set_focus`: a widget's focus callback that closes its layer
    /// waits until the focus change is done.
    focusing: bool,
    deferred_closes: Vec<WidgetId>,
    /// Where the focus goes after a layer was removed from outside.
    pending_return: Option<WidgetId>,
    /// Lists whose rows the tree builds as they come into view.
    lists: Vec<ListState>,
    /// Whether the last input was the keyboard (focus rings show) rather
    /// than the pointer.
    keyboard: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    /// An empty tree.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            taffy: TaffyTree::new(),
            root: None,
            focus: None,
            hover: None,
            tip_wait: None,
            tip: None,
            capture: None,
            implicit_capture: false,
            buttons_held: 0,
            area: Rect::ZERO,
            layout_dirty: true,
            restyle: Vec::new(),
            clipboard: Clipboard::default(),
            stats: TreeStats::default(),
            theme_seen: crate::theme::generation(),
            layers: Vec::new(),
            next_layer: 0,
            focusing: false,
            deferred_closes: Vec::new(),
            pending_return: None,
            lists: Vec::new(),
            keyboard: true,
        }
    }

    fn node(&self, id: WidgetId) -> Option<&NodeData> {
        let slot = self.slots.get(id.index as usize)?;
        (slot.generation == id.generation)
            .then_some(slot.node.as_ref())
            .flatten()
    }

    fn node_mut(&mut self, id: WidgetId) -> Option<&mut NodeData> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation == id.generation {
            slot.node.as_mut()
        } else {
            None
        }
    }

    /// Adds a widget, without a parent yet.
    pub fn add(&mut self, widget: impl Widget) -> WidgetId {
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    node: None,
                });
                (self.slots.len() - 1) as u32
            }
        };
        let generation = self.slots[index as usize].generation;
        let id = WidgetId { index, generation };
        let style = widget.style();
        // Taffy only fails for ids it doesn't know; ours are fresh.
        let layout = self
            .taffy
            .new_leaf_with_context(style, id)
            .unwrap_or_else(|_| taffy::NodeId::from(u64::MAX));
        self.slots[index as usize].node = Some(NodeData {
            widget: Box::new(widget),
            parent: None,
            children: Vec::new(),
            layout,
            bounds: Rect::ZERO,
            content: Size::ZERO,
            scene: Scene::new(),
            paint_dirty: true,
            effects: Vec::new(),
        });
        self.layout_dirty = true;
        id
    }

    /// Adds a widget with `children` (already added, without a parent).
    pub fn add_with_children(&mut self, widget: impl Widget, children: &[WidgetId]) -> WidgetId {
        let id = self.add(widget);
        self.set_children(id, children);
        id
    }

    /// Replaces `parent`'s children. Children it had before that aren't in
    /// `children` are detached, not removed.
    pub fn set_children(&mut self, parent: WidgetId, children: &[WidgetId]) {
        let Some(node) = self.node(parent) else {
            return;
        };
        let old = node.children.clone();
        for child in old {
            if let Some(child) = self.node_mut(child) {
                child.parent = None;
            }
        }
        let mut layouts = Vec::with_capacity(children.len());
        for &child in children {
            if let Some(node) = self.node_mut(child) {
                node.parent = Some(parent);
                layouts.push(node.layout);
            }
        }
        if let Some(node) = self.node_mut(parent) {
            node.children = children.to_vec();
            let layout = node.layout;
            let _ = self.taffy.set_children(layout, &layouts);
        }
        self.layout_dirty = true;
    }

    /// Makes `id` the root, laid out over the whole window.
    pub fn set_root(&mut self, id: WidgetId) {
        self.root = Some(id);
        self.layout_dirty = true;
    }

    /// Opens `content` (added, without a parent) as a layer above the
    /// window: a popover, a dialog or a tooltip (`docs/design/shell.md`
    /// §6). It is laid out at its own size and placed next to its anchor;
    /// the newest layer is on top.
    pub fn open_layer(&mut self, content: WidgetId, options: LayerOptions) -> LayerId {
        debug_assert!(
            self.node(content).is_some_and(|node| node.parent.is_none()),
            "a layer's content must be a widget without a parent"
        );
        let id = LayerId(self.next_layer);
        self.next_layer += 1;
        let focus_pending = options.focus_first;
        let mut appear = crate::animation::Transition::new(0.0);
        let duration = match options.kind {
            LayerKind::Modal => crate::animation::durations::DIALOG,
            _ => crate::animation::durations::POPOVER,
        };
        appear.to(1.0, duration, crate::animation::Easing::EaseOut);
        self.layers.push(Layer {
            id,
            root: content,
            options,
            return_focus: self.focus,
            focus_pending,
            fresh: true,
            appear,
        });
        self.layout_dirty = true;
        request_frame();
        id
    }

    /// Puts the open layer `id` at another anchor (a dialog placed for
    /// the window's size, after a resize). `false` if it isn't open.
    pub fn move_layer(&mut self, id: LayerId, anchor: crate::layers::Anchor) -> bool {
        let Some(layer) = self.layers.iter_mut().find(|layer| layer.id == id) else {
            return false;
        };
        layer.options.anchor = anchor;
        self.layout_dirty = true;
        request_frame();
        true
    }

    /// Closes layer `id` and the layers opened after it (submenus), from
    /// the top: their widgets are removed, the focus goes back to where it
    /// was when they opened, and their `on_close` callbacks run. `false`
    /// if it wasn't open.
    pub fn close_layer(&mut self, fonts: &mut Fonts, id: LayerId) -> bool {
        let Some(index) = self.layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        // A tooltip closes alone: what opened after it (a popover from a
        // shortcut) isn't its submenu.
        let end = if self.layers[index].options.kind == LayerKind::Tooltip {
            index + 1
        } else {
            self.layers.len()
        };
        let mut closed: Vec<Layer> = self.layers.drain(index..end).collect();
        if self
            .tip
            .is_some_and(|(_, tip)| closed.iter().any(|layer| layer.id == tip))
        {
            self.tip = None;
        }
        closed.reverse();
        let mut callbacks = Vec::new();
        for layer in closed {
            let had_focus = self
                .focus
                .is_some_and(|focus| self.top_of(focus) == layer.root);
            self.remove_subtree(layer.root);
            if had_focus || self.focus.is_none() {
                let back = layer.return_focus.filter(|&id| self.is_shown(id));
                self.set_focus(fonts, back);
            }
            callbacks.extend(layer.options.on_close);
        }
        self.layout_dirty = true;
        request_frame();
        for mut callback in callbacks {
            callback();
        }
        true
    }

    /// Tells the tree the keyboard was used, for keys the view handles
    /// itself before the tree (app shortcuts like `Ctrl+K`), so the focus
    /// ring shows in what they open.
    pub fn note_keyboard(&mut self) {
        self.set_keyboard(true);
    }

    /// Records whether the keyboard or the pointer was used last; the
    /// focused widget repaints when its ring comes or goes.
    fn set_keyboard(&mut self, keyboard: bool) {
        if self.keyboard != keyboard {
            self.keyboard = keyboard;
            if let Some(focus) = self.focus {
                self.mark(focus, false);
                request_frame();
            }
        }
    }

    /// Brings `id` into view: every scroll container around it scrolls
    /// as far as needed (the focus does this on its own).
    pub fn reveal(&mut self, fonts: &mut Fonts, id: WidgetId) {
        let Some(mut target) = self.node(id).map(|node| node.bounds) else {
            return;
        };
        let mut current = self.node(id).and_then(|node| node.parent);
        while let Some(ancestor) = current {
            let before = self.node(ancestor).map(|node| node.widget.scroll_offset());
            self.call(fonts, ancestor, |widget, cx| widget.reveal(cx, target));
            let after = self.node(ancestor).map(|node| node.widget.scroll_offset());
            if let (Some(before), Some(after)) = (before, after) {
                // Seen from further out, the target moved with the scroll.
                target = target - (after - before);
            }
            current = self.node(ancestor).and_then(|node| node.parent);
        }
    }

    /// Adds a [`VirtualList`](crate::VirtualList) in `scroll` (its style: height, growth) and
    /// returns the scroll container. Its rows are built during layout, as
    /// they come into view.
    pub fn add_virtual_list(
        &mut self,
        list: crate::scroll::VirtualList,
        scroll: crate::scroll::Scroll,
    ) -> WidgetId {
        let count = (list.count)();
        let spacer = self.add(crate::scroll::ListSpacer { height: 0.0 });
        let body = self.add_with_children(
            crate::scroll::ListBody {
                height: list.row_height * count as f64,
                count,
                label: list.label.clone(),
            },
            &[spacer],
        );
        let id = self.add_with_children(scroll, &[body]);
        let watched = list.count.clone();
        let stale = std::rc::Rc::new(std::cell::Cell::new(false));
        let mark = stale.clone();
        self.watch(id, true, move || {
            watched();
            mark.set(true);
        });
        self.lists.push(ListState {
            scroll: id,
            body,
            spacer,
            row_height: list.row_height,
            overscan: list.overscan,
            count: list.count,
            build: Some(list.build),
            rows: std::collections::BTreeMap::new(),
            stale,
            pending_row: None,
        });
        self.layout_dirty = true;
        id
    }

    /// Scrolls the list in `scroll` (from [`Ui::add_virtual_list`]) so its
    /// row `index` is in view (the palette's selection moving with ↑↓), at
    /// the next layout, once the list's height is known.
    pub fn scroll_to_row(&mut self, scroll: WidgetId, index: usize) {
        if let Some(list) = self.lists.iter_mut().find(|list| list.scroll == scroll) {
            list.pending_row = Some(index);
            self.layout_dirty = true;
            request_frame();
        }
    }

    fn scroll_widget(&mut self, scroll: WidgetId) -> Option<&mut crate::scroll::Scroll> {
        self.node_mut(scroll).and_then(|node| {
            (&mut *node.widget as &mut dyn Any).downcast_mut::<crate::scroll::Scroll>()
        })
    }

    /// Whether `ancestor` is `id` or one of its ancestors.
    fn contains(&self, ancestor: WidgetId, id: WidgetId) -> bool {
        let mut current = Some(id);
        while let Some(at) = current {
            if at == ancestor {
                return true;
            }
            current = self.node(at).and_then(|node| node.parent);
        }
        false
    }

    /// Takes the pointer capture away, telling its holder.
    fn drop_capture(&mut self, fonts: &mut Fonts) {
        self.implicit_capture = false;
        if let Some(held) = self.capture.take() {
            self.call(fonts, held, |widget, cx| widget.capture_lost(cx));
        }
    }

    /// Builds the rows that came into view and removes those far out of it,
    /// in every list. `true` if any row came or went.
    fn update_lists(&mut self, fonts: &mut Fonts) -> bool {
        let mut any = false;
        let scrolls: Vec<WidgetId> = self.lists.iter().map(|list| list.scroll).collect();
        for scroll in scrolls {
            any |= self.update_list(fonts, scroll);
        }
        any
    }

    fn list(&self, scroll: WidgetId) -> Option<&ListState> {
        self.lists.iter().find(|list| list.scroll == scroll)
    }

    fn list_mut(&mut self, scroll: WidgetId) -> Option<&mut ListState> {
        self.lists.iter_mut().find(|list| list.scroll == scroll)
    }

    fn built_rows(&self, scroll: WidgetId) -> Vec<WidgetId> {
        self.list(scroll)
            .map(|list| list.rows.values().copied().collect())
            .unwrap_or_default()
    }

    /// One list's rows. The list is looked up by its scroll id each time:
    /// its builder gets the tree and may add or remove lists.
    fn update_list(&mut self, fonts: &mut Fonts, scroll: WidgetId) -> bool {
        let Some((body, spacer, row_height, overscan, count_of, stale)) =
            self.list(scroll).map(|list| {
                (
                    list.body,
                    list.spacer,
                    list.row_height,
                    list.overscan,
                    list.count.clone(),
                    list.stale.clone(),
                )
            })
        else {
            return false;
        };
        let count = imaginarium_reactive::untrack(|| count_of());
        let Some(view) = self.node(scroll).map(|node| node.bounds.height()) else {
            return false;
        };
        let mut changed = false;
        // A row asked for comes into view once the view has a height.
        if view > 0.0
            && let Some(row) = self
                .list_mut(scroll)
                .and_then(|list| list.pending_row.take())
            && let Some(widget) = self.scroll_widget(scroll)
        {
            let (top, bottom) = (row as f64 * row_height, (row + 1) as f64 * row_height);
            let offset = widget.offset();
            if top < offset {
                widget.set_offset(top);
            } else if bottom > offset + view {
                widget.set_offset(bottom - view);
            }
        }
        // In range before the rows are chosen, so a list that shrank keeps
        // rows in view.
        let max = (count as f64 * row_height - view).max(0.0);
        let offset = self.scroll_widget(scroll).map_or(0.0, |widget| {
            let clamped = widget.offset().min(max);
            // Only when out of range: setting the offset ends a glide.
            if clamped != widget.offset() {
                widget.set_offset(clamped);
            }
            clamped
        });
        let first = ((offset / row_height).floor() as usize).saturating_sub(overscan);
        let end = ((offset + view) / row_height)
            .ceil()
            .clamp(0.0, count as f64) as usize;
        let last = end.saturating_add(overscan).min(count);
        let wanted = first.min(last)..last;
        // The body's height and the rows' set size follow the count.
        if let Some(body_widget) = self.node_mut(body).and_then(|node| {
            (&mut *node.widget as &mut dyn Any).downcast_mut::<crate::scroll::ListBody>()
        }) && body_widget.count != count
        {
            body_widget.count = count;
            body_widget.height = row_height * count as f64;
            self.mark(body, true);
            for slot in self.built_rows(scroll) {
                if let Some(row) = self.node_mut(slot).and_then(|node| {
                    (&mut *node.widget as &mut dyn Any).downcast_mut::<crate::scroll::RowSlot>()
                }) {
                    row.count = count;
                }
            }
        }
        // New data: every row is built again from it.
        let rebuild = stale.replace(false);
        let gone: Vec<usize> = self
            .list(scroll)
            .map(|list| {
                list.rows
                    .keys()
                    .copied()
                    .filter(|row| rebuild || !wanted.contains(row))
                    .collect()
            })
            .unwrap_or_default();
        if !gone.is_empty() {
            let removed: Vec<WidgetId> = match self.list_mut(scroll) {
                Some(list) => gone
                    .iter()
                    .filter_map(|row| list.rows.remove(row))
                    .collect(),
                None => Vec::new(),
            };
            // Detached from the body first: removing a subtree assumes its
            // parent is going too.
            let kept: Vec<WidgetId> = std::iter::once(spacer)
                .chain(self.built_rows(scroll))
                .collect();
            self.set_children(body, &kept);
            for slot in removed {
                // A row with the focus or a drag in it hands them to the
                // list, rather than dropping them where keys can't follow.
                if self.capture.is_some_and(|held| self.contains(slot, held)) {
                    self.drop_capture(fonts);
                }
                if self.focus.is_some_and(|focus| self.contains(slot, focus)) {
                    self.set_focus(fonts, Some(scroll));
                }
                self.remove_subtree(slot);
            }
            changed = true;
        }
        let missing: Vec<usize> = self
            .list(scroll)
            .map(|list| wanted.filter(|row| !list.rows.contains_key(row)).collect())
            .unwrap_or_default();
        if !missing.is_empty()
            && let Some(mut build) = self.list_mut(scroll).and_then(|list| list.build.take())
        {
            for row in missing {
                let content = build(self, row);
                let slot = self.add_with_children(
                    crate::scroll::RowSlot {
                        index: row,
                        count,
                        height: row_height,
                    },
                    &[content],
                );
                match self.list_mut(scroll) {
                    Some(list) => {
                        list.rows.insert(row, slot);
                    }
                    // The builder removed its own list.
                    None => self.remove_subtree(slot),
                }
            }
            if let Some(list) = self.list_mut(scroll) {
                list.build = Some(build);
            }
            changed = true;
        }
        if changed && self.list(scroll).is_some() {
            let first_built = self
                .list(scroll)
                .and_then(|list| list.rows.keys().next().copied())
                .unwrap_or(0);
            if let Some(space) = self.node_mut(spacer).and_then(|node| {
                (&mut *node.widget as &mut dyn Any).downcast_mut::<crate::scroll::ListSpacer>()
            }) {
                space.height = first_built as f64 * row_height;
            }
            self.mark(spacer, true);
            let children: Vec<WidgetId> = std::iter::once(spacer)
                .chain(self.built_rows(scroll))
                .collect();
            self.set_children(body, &children);
        }
        changed
    }

    /// Whether layer `id` is open.
    pub fn is_layer_open(&self, id: LayerId) -> bool {
        self.layers.iter().any(|layer| layer.id == id)
    }

    /// The topmost ancestor of `id` (the root, a layer's content, or a
    /// detached widget).
    fn top_of(&self, id: WidgetId) -> WidgetId {
        let mut current = id;
        while let Some(parent) = self.node(current).and_then(|node| node.parent) {
            current = parent;
        }
        current
    }

    /// The topmost modal layer, which takes all input.
    fn modal(&self) -> Option<&Layer> {
        self.layers
            .iter()
            .rev()
            .find(|layer| layer.options.kind == LayerKind::Modal)
    }

    /// Whether `id` may take input now: everything, unless a dialog is
    /// open; then only the dialog and the layers above it.
    fn reachable(&self, id: WidgetId) -> bool {
        let Some(modal) = self
            .layers
            .iter()
            .rposition(|layer| layer.options.kind == LayerKind::Modal)
        else {
            return true;
        };
        let top = self.top_of(id);
        self.layers[modal..].iter().any(|layer| layer.root == top)
    }

    /// Removes `id` and everything below it, disposing their effects.
    pub fn remove(&mut self, id: WidgetId) {
        let Some(parent) = self.node(id).map(|node| node.parent) else {
            return;
        };
        if let Some(parent) = parent
            && let Some(node) = self.node(parent)
        {
            let rest: Vec<WidgetId> = node.children.iter().copied().filter(|&c| c != id).collect();
            self.set_children(parent, &rest);
        }
        self.remove_subtree(id);
    }

    /// Removes `id` and its descendants, without detaching it from its
    /// parent (the parent is going too, or did that already): detaching
    /// each child from a dying parent would be quadratic.
    fn remove_subtree(&mut self, id: WidgetId) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        let children = std::mem::take(&mut node.children);
        // Taffy drops each removed child from its parent's list, which is
        // quadratic for a wide parent; emptying the list first keeps it
        // linear.
        let layout = node.layout;
        if !children.is_empty() {
            let _ = self.taffy.set_children(layout, &[]);
        }
        for child in children {
            self.remove_subtree(child);
        }
        let slot = &mut self.slots[id.index as usize];
        if let Some(node) = slot.node.take() {
            let _ = self.taffy.remove(node.layout);
            for effect in node.effects {
                effect.dispose();
            }
        }
        slot.generation = slot.generation.wrapping_add(1);
        // A wrapped generation could alias an old id: retire the slot.
        if slot.generation != 0 {
            self.free.push(id.index);
        }
        for held in [&mut self.focus, &mut self.hover, &mut self.capture] {
            if *held == Some(id) {
                *held = None;
            }
        }
        if self.root == Some(id) {
            self.root = None;
        }
        self.lists.retain(|list| list.scroll != id);
        // Removed from outside: the layer goes without its callback, and
        // the focus goes back at the next layout.
        if let Some(index) = self.layers.iter().position(|layer| layer.root == id) {
            let layer = self.layers.remove(index);
            if self.focus.is_none() {
                self.pending_return = layer.return_focus;
            }
        }
        self.layout_dirty = true;
    }

    /// The widget `id` as its concrete type, to change it. Marks it for
    /// layout and paint; the next layout reads its style again.
    pub fn widget_mut<W: Widget>(&mut self, id: WidgetId) -> Option<&mut W> {
        let node = self.node_mut(id)?;
        node.paint_dirty = true;
        self.layout_dirty = true;
        self.restyle.push(id);
        let node = self.node_mut(id)?;
        (&mut *node.widget as &mut dyn Any).downcast_mut::<W>()
    }

    /// The widget `id` as its concrete type, to read it.
    pub fn widget<W: Widget>(&self, id: WidgetId) -> Option<&W> {
        (&*self.node(id)?.widget as &dyn Any).downcast_ref::<W>()
    }

    /// Re-lays out (`layout`) or repaints `id` whenever something `watch`
    /// reads changes (signals, memos). The effect lives as long as the
    /// widget.
    pub fn watch(&mut self, id: WidgetId, layout: bool, mut watch: impl FnMut() + 'static) {
        let mut first = true;
        let effect = imaginarium_reactive::effect(move || {
            watch();
            if !std::mem::take(&mut first) {
                invalidate(id, layout);
            }
        });
        if let Some(node) = self.node_mut(id) {
            node.effects.push(effect);
        } else {
            effect.dispose();
        }
    }

    /// Marks `id` for layout and paint in the frame being drawn, without
    /// asking for a frame (content that shows per-frame data, like timings).
    pub fn refresh(&mut self, id: WidgetId) {
        self.mark(id, true);
    }

    /// The widget whose tooltip shows, and the tooltip's text.
    #[cfg(test)]
    pub(crate) fn tip_for_tests(&self) -> Option<(WidgetId, String)> {
        let (widget, layer) = self.tip?;
        if !self.is_layer_open(layer) {
            return None;
        }
        let text = self.node(widget)?.widget.tooltip()?;
        Some((widget, text))
    }

    /// The children of `id`, for tests.
    #[cfg(test)]
    pub(crate) fn widget_children_for_tests(&self, id: WidgetId) -> Vec<WidgetId> {
        self.node(id)
            .map(|node| node.children.clone())
            .unwrap_or_default()
    }

    /// The widget's box from the last layout.
    pub fn bounds(&self, id: WidgetId) -> Option<Rect> {
        self.node(id).map(|node| node.bounds)
    }

    /// The focused widget.
    pub fn focused(&self) -> Option<WidgetId> {
        self.focus
    }

    /// What the last frame cost.
    pub fn stats(&self) -> TreeStats {
        self.stats
    }

    fn apply_pending(&mut self) {
        let pending = PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
        for (id, layout) in pending {
            self.mark(id, layout);
        }
        // A theme switch touches every widget: colors, and the text that
        // labels shaped in their old color. It is rare, so no finer
        // tracking.
        let theme = crate::theme::generation();
        if theme != self.theme_seen {
            self.theme_seen = theme;
            let live: Vec<WidgetId> = self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.node.is_some())
                .map(|(index, slot)| WidgetId {
                    index: index as u32,
                    generation: slot.generation,
                })
                .collect();
            for id in live {
                self.mark(id, true);
            }
        }
    }

    fn mark(&mut self, id: WidgetId, layout: bool) {
        let Some(node) = self.node_mut(id) else {
            return;
        };
        node.paint_dirty = true;
        if layout {
            let layout = node.layout;
            let style = node.widget.style();
            let _ = self.taffy.set_style(layout, style);
            let _ = self.taffy.mark_dirty(layout);
            self.layout_dirty = true;
        }
    }

    /// Calls `f` on widget `id` with a context, then applies what it asked
    /// for.
    fn call<R>(
        &mut self,
        fonts: &mut Fonts,
        id: WidgetId,
        f: impl FnOnce(&mut dyn Widget, &mut Cx<'_>) -> R,
    ) -> Option<R> {
        let (focused, hovered) = (self.focus == Some(id), self.hover == Some(id));
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        let node = slot.node.as_mut()?;
        let mut cx = Cx {
            id,
            bounds: node.bounds,
            content: node.content,
            focused,
            focus_visible: focused && self.keyboard,
            hovered,
            fonts,
            clipboard: &mut self.clipboard,
            access: None,
            requests: Requests::default(),
        };
        let result = f(&mut *node.widget, &mut cx);
        let requests = cx.requests;
        self.apply(fonts, id, requests);
        Some(result)
    }

    fn apply(&mut self, fonts: &mut Fonts, id: WidgetId, requests: Requests) {
        if requests.paint || requests.layout {
            self.mark(id, requests.layout);
            request_frame();
        }
        if requests.place {
            self.layout_dirty = true;
            request_frame();
        }
        match requests.capture {
            Some(true) => {
                self.capture = Some(id);
                self.implicit_capture = false;
            }
            Some(false) if self.capture == Some(id) => self.capture = None,
            _ => {}
        }
        match requests.focus {
            Some(true) => self.set_focus(fonts, Some(id)),
            Some(false) if self.focus == Some(id) => self.set_focus(fonts, None),
            _ => {}
        }
        if requests.close_layer {
            if self.focusing {
                self.deferred_closes.push(id);
            } else {
                self.close_layer_of(fonts, id);
            }
        }
    }

    /// Moves the keyboard focus to `id` (or nowhere).
    pub fn set_focus(&mut self, fonts: &mut Fonts, id: Option<WidgetId>) {
        if self.focus == id {
            return;
        }
        let outer = !std::mem::replace(&mut self.focusing, true);
        if let Some(old) = self.focus.take() {
            self.call(fonts, old, |widget, cx| widget.focus_changed(cx, false));
            self.mark(old, false);
        }
        self.focus = id;
        if let Some(new) = id {
            self.call(fonts, new, |widget, cx| widget.focus_changed(cx, true));
            self.mark(new, false);
            self.reveal(fonts, new);
        }
        crate::window::reset_ime();
        request_frame();
        if outer {
            self.focusing = false;
            for widget in std::mem::take(&mut self.deferred_closes) {
                self.close_layer_of(fonts, widget);
            }
        }
    }

    /// Closes the layer `widget` is in, if any.
    fn close_layer_of(&mut self, fonts: &mut Fonts, widget: WidgetId) {
        let top = self.top_of(widget);
        if let Some(layer) = self.layers.iter().find(|layer| layer.root == top) {
            let layer = layer.id;
            self.close_layer(fonts, layer);
        }
    }

    /// Lays the tree out in `area` (the window, in logical pixels) if
    /// anything changed.
    pub fn layout(&mut self, fonts: &mut Fonts, area: Rect) {
        self.apply_pending();
        if area != self.area {
            self.area = area;
            self.layout_dirty = true;
        }
        if self.root.is_none() && self.layers.is_empty() {
            return;
        }
        if !self.layout_dirty {
            self.stats.layout = None;
            return;
        }
        let start = Instant::now();
        for id in std::mem::take(&mut self.restyle) {
            self.mark(id, true);
        }
        // Lists build the rows in view; a first layout (or a resize) can
        // show other rows than the last one did, so they look again after
        // placing, at most once more.
        for pass in 0..2 {
            let changed = self.update_lists(fonts);
            if pass == 1 && !changed {
                break;
            }
            if let Some(root) = self.root
                && let Some(root_layout) = self.node(root).map(|node| node.layout)
            {
                let available = taffy::Size {
                    width: AvailableSpace::Definite(area.width() as f32),
                    height: AvailableSpace::Definite(area.height() as f32),
                };
                self.compute(fonts, root_layout, available);
                self.place(root, area.origin());
            }
            // Lists in layers too: they are laid out in every pass.
            self.layout_layers(fonts, area);
            if self.lists.is_empty() {
                break;
            }
        }

        if self.focus.is_none()
            && let Some(back) = self.pending_return.take()
            && self.is_shown(back)
        {
            self.set_focus(fonts, Some(back));
        }
        self.layout_dirty = false;
        self.stats.layout = Some(start.elapsed());
        // Keys must not go to a widget nobody sees, and a screen reader's
        // focus must be on a node that exists.
        if let Some(focus) = self.focus
            && !self.is_shown(focus)
        {
            self.set_focus(fonts, None);
        }
        if let Some(capture) = self.capture
            && !self.is_shown(capture)
        {
            self.drop_capture(fonts);
        }
    }

    /// Runs Taffy on the tree under `layout`, measuring leaves with their
    /// widgets.
    fn compute(
        &mut self,
        fonts: &mut Fonts,
        layout: taffy::NodeId,
        available: taffy::Size<AvailableSpace>,
    ) {
        let root_layout = layout;
        let slots = &mut self.slots;
        let clipboard = &mut self.clipboard;
        let _ = self.taffy.compute_layout_with_measure(
            root_layout,
            available,
            |inputs, _node, context, style| {
                let id = context.copied();
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, available| {
                        let Some(id) = id else {
                            return taffy::Size::ZERO;
                        };
                        let Some(node) = slots
                            .get_mut(id.index as usize)
                            .and_then(|slot| slot.node.as_mut())
                        else {
                            return taffy::Size::ZERO;
                        };
                        let mut cx = Cx {
                            id,
                            bounds: node.bounds,
                            content: node.content,
                            focused: false,
                            focus_visible: false,
                            hovered: false,
                            fonts: &mut *fonts,
                            clipboard: &mut *clipboard,
                            access: None,
                            requests: Requests::default(),
                        };
                        let size = node.widget.measure(&mut cx, known, available);
                        taffy::Size {
                            width: size.width as f32,
                            height: size.height as f32,
                        }
                    },
                )
            },
        );
    }

    /// Lays out and places every layer: at its content's own size, next to
    /// its anchor. Layers whose anchor widget went away close.
    fn layout_layers(&mut self, fonts: &mut Fonts, area: Rect) {
        let content_sized = taffy::Size {
            width: AvailableSpace::MaxContent,
            height: AvailableSpace::MaxContent,
        };
        let mut orphans = Vec::new();
        for index in 0..self.layers.len() {
            let (root, anchor, placement, align, gap) = {
                let layer = &self.layers[index];
                let o = &layer.options;
                (layer.root, o.anchor, o.placement, o.align, o.gap)
            };
            let Some(layout) = self.node(root).map(|node| node.layout) else {
                continue;
            };
            self.compute(fonts, layout, content_sized);
            let size_of = |taffy: &TaffyTree<WidgetId>| {
                taffy
                    .layout(layout)
                    .map(|l| Size::new(f64::from(l.size.width), f64::from(l.size.height)))
                    .unwrap_or(Size::ZERO)
            };
            let mut size = size_of(&self.taffy);
            // Wider than the window: laid out again at the window's width,
            // so its text wraps. (Taller content needs a scroll container.)
            let max_width = (area.width() - 2.0 * crate::layers::EDGE).max(0.0);
            if size.width > max_width {
                self.compute(
                    fonts,
                    layout,
                    taffy::Size {
                        width: AvailableSpace::Definite(max_width as f32),
                        height: AvailableSpace::MaxContent,
                    },
                );
                size = size_of(&self.taffy);
            }
            let anchor = match anchor {
                Anchor::Widget(widget) => {
                    if !self.is_shown(widget) {
                        orphans.push(self.layers[index].id);
                        continue;
                    }
                    self.node(widget).map(|node| node.bounds)
                }
                Anchor::Rect(rect) => Some(rect),
                Anchor::Point(point) => Some(Rect::from_points(point, point)),
                Anchor::Center => None,
            };
            let origin = match anchor {
                Some(anchor) => place_layer(anchor, size, area, placement, align, gap),
                None => {
                    crate::layers::keep_inside(area.center() - size.to_vec2() / 2.0, size, area)
                }
            };
            self.place(root, origin);
        }
        for id in orphans {
            self.close_layer(fonts, id);
        }
        // The focus moves into a layer once it has boxes to focus. Layers
        // opened together (a menu and its submenu) give it back to the one
        // below, not all to the window.
        let mut below_was_pending = false;
        for index in 0..self.layers.len() {
            let pending = std::mem::take(&mut self.layers[index].focus_pending);
            if pending {
                if below_was_pending {
                    self.layers[index].return_focus = self.focus;
                }
                let mut order = Vec::new();
                self.focusables(self.layers[index].root, &mut order);
                let wanted = self.layers[index]
                    .options
                    .initial_focus
                    .filter(|wanted| order.contains(wanted));
                if let Some(first) = wanted.or_else(|| order.first().copied()) {
                    self.set_focus(fonts, Some(first));
                }
            }
            below_was_pending = pending;
            // A new dialog ends drags and hover below it.
            if std::mem::take(&mut self.layers[index].fresh)
                && self.layers[index].options.kind == LayerKind::Modal
            {
                if self.capture.is_some_and(|id| !self.reachable(id)) {
                    self.drop_capture(fonts);
                }
                if let Some(hover) = self.hover
                    && !self.reachable(hover)
                {
                    self.hover = None;
                    self.call(fonts, hover, |widget, cx| widget.hover_changed(cx, false));
                }
                // Nothing behind it keeps the keys.
                if let Some(focus) = self.focus
                    && !self.reachable(focus)
                {
                    self.set_focus(fonts, None);
                }
            }
        }
    }

    /// Whether `id` is in the tree under the root and has a box (it and
    /// its ancestors aren't hidden). Detached widgets keep their old
    /// bounds, so the walk has to reach the root.
    fn is_shown(&self, id: WidgetId) -> bool {
        let mut current = id;
        loop {
            let Some(node) = self.node(current) else {
                return false;
            };
            if node.bounds.width() <= 0.0 || node.bounds.height() <= 0.0 {
                return false;
            }
            match node.parent {
                Some(parent) => current = parent,
                None => {
                    return self.root == Some(current)
                        || self.layers.iter().any(|layer| layer.root == current);
                }
            }
        }
    }

    /// Computes window coordinates from Taffy's parent-relative ones.
    fn place(&mut self, id: WidgetId, parent_origin: Point) {
        let Some(node) = self.node(id) else {
            return;
        };
        let Ok(layout) = self.taffy.layout(node.layout) else {
            return;
        };
        let origin = parent_origin
            + imaginarium_render::kurbo::Vec2::new(
                f64::from(layout.location.x),
                f64::from(layout.location.y),
            );
        let bounds = Rect::from_origin_size(
            origin,
            Size::new(f64::from(layout.size.width), f64::from(layout.size.height)),
        );
        let children = node.children.clone();
        // How far the children reach, from their Taffy boxes, with their
        // margins and this box's padding and border at the far edges.
        let (pad_right, pad_bottom) = (
            f64::from(layout.padding.right + layout.border.right),
            f64::from(layout.padding.bottom + layout.border.bottom),
        );
        let mut content = Size::ZERO;
        for &child in &children {
            if let Some(child) = self.node(child)
                && let Ok(child) = self.taffy.layout(child.layout)
            {
                let right = child.location.x + child.size.width + child.margin.right;
                let bottom = child.location.y + child.size.height + child.margin.bottom;
                content.width = content.width.max(f64::from(right) + pad_right);
                content.height = content.height.max(f64::from(bottom) + pad_bottom);
            }
        }
        let mut offset = Vec2::ZERO;
        if let Some(node) = self.node_mut(id) {
            node.content = content;
            if node.bounds != bounds {
                node.bounds = bounds;
                node.paint_dirty = true;
            }
            node.widget.placed(bounds, content);
            offset = node.widget.scroll_offset();
        }
        for child in children {
            self.place(child, origin - offset);
        }
    }

    /// Paints what changed and composes every widget's scene into `scene`.
    pub fn paint(&mut self, fonts: &mut Fonts, scene: &mut Scene) {
        self.stats.repainted = 0;
        self.stats.widgets = self.slots.iter().filter(|slot| slot.node.is_some()).count();
        if let Some(root) = self.root {
            self.paint_node(fonts, root, scene);
        }
        let layers: Vec<(WidgetId, LayerKind, f64, Option<f32>)> = self
            .layers
            .iter()
            .map(|layer| {
                let appear = layer.appear.value();
                (layer.root, layer.options.kind, appear, layer.options.scrim)
            })
            .collect();
        for (root, kind, appear, dim) in layers {
            // Layers fade in as they open; the scrim with its dialog.
            let alpha = appear.clamp(0.0, 1.0) as f32;
            if kind == LayerKind::Modal {
                let scrim = crate::theme::theme().color(crate::theme::Token::Scrim);
                let strength = dim.unwrap_or(scrim.components[3]);
                let scrim = scrim.with_alpha(strength * alpha);
                scene.fill(&self.area, scrim);
            }
            if alpha < 1.0 {
                scene.push_opacity(alpha);
                self.paint_node(fonts, root, scene);
                scene.pop();
            } else {
                self.paint_node(fonts, root, scene);
            }
        }
    }

    fn paint_node(&mut self, fonts: &mut Fonts, id: WidgetId, scene: &mut Scene) {
        let Some(node) = self.node(id) else {
            return;
        };
        if node.bounds.width() <= 0.0 || node.bounds.height() <= 0.0 {
            return; // hidden (display: none) or collapsed
        }
        if node.paint_dirty {
            let mut own = self
                .node_mut(id)
                .map(|node| std::mem::take(&mut node.scene))
                .unwrap_or_default();
            own.clear();
            self.call(fonts, id, |widget, cx| widget.paint(cx, &mut own));
            // Texture work happens in this frame only; the cached scene is
            // appended again in later frames.
            own.move_texture_work(scene);
            if let Some(node) = self.node_mut(id) {
                node.scene = own;
                node.paint_dirty = false;
            }
            self.stats.repainted += 1;
        }
        let Some(node) = self.node(id) else {
            return;
        };
        scene.append(&node.scene, imaginarium_render::kurbo::Affine::IDENTITY);
        let clips = node.widget.clips();
        let bounds = node.bounds;
        let children = node.children.clone();
        if clips {
            scene.push_clip(&bounds);
        }
        for child in children {
            self.paint_node(fonts, child, scene);
        }
        // Drawn every frame (cheap: a scroll bar), over the children and
        // inside the clip.
        if self.node(id).is_some_and(|node| node.widget.paints_over()) {
            self.call(fonts, id, |widget, cx| widget.paint_over(cx, scene));
        }
        if clips {
            scene.pop();
        }
    }

    /// The deepest visible widget at `point`, later siblings first (they
    /// are drawn on top). Children may stick out of a parent that doesn't
    /// clip them, as they are drawn.
    fn hit(&self, id: WidgetId, point: Point) -> Option<WidgetId> {
        let node = self.node(id)?;
        if node.bounds.width() <= 0.0 || node.bounds.height() <= 0.0 {
            return None; // hidden (display: none), and so are its children
        }
        let inside = node.bounds.contains(point);
        if !inside && node.widget.clips() {
            return None;
        }
        if inside && node.widget.claims(node.bounds, node.content, point) {
            return Some(id);
        }
        node.children
            .iter()
            .rev()
            .find_map(|&child| self.hit(child, point))
            .or((inside && !node.widget.passes_pointer()).then_some(id))
    }

    /// The widget at `point`, from the top layer down: tooltips are passed
    /// through, and nothing below a modal layer is reached.
    fn hit_top(&self, point: Point) -> Option<WidgetId> {
        for layer in self.layers.iter().rev() {
            match layer.options.kind {
                LayerKind::Tooltip => continue,
                kind => {
                    if let Some(hit) = self.hit(layer.root, point) {
                        return Some(hit);
                    }
                    if kind == LayerKind::Modal {
                        return None;
                    }
                }
            }
        }
        self.root.and_then(|root| self.hit(root, point))
    }

    /// A press outside the top popovers closes them (and only them), and
    /// is used up by that: it doesn't also click what lies below.
    fn dismiss_outside(&mut self, fonts: &mut Fonts, point: Point) -> bool {
        let mut first_closed = None;
        for (index, layer) in self.layers.iter().enumerate().rev() {
            // Tooltips take no input, so they neither shield nor close.
            if layer.options.kind == LayerKind::Tooltip {
                continue;
            }
            let inside = self.hit(layer.root, point).is_some();
            if inside || layer.options.kind != LayerKind::Popover || !layer.options.dismissable {
                break;
            }
            first_closed = Some(index);
        }
        match first_closed {
            Some(index) => {
                let id = self.layers[index].id;
                self.close_layer(fonts, id)
            }
            None => false,
        }
    }

    fn ancestors(&self, id: WidgetId) -> Vec<WidgetId> {
        let mut chain = vec![id];
        let mut current = id;
        while let Some(parent) = self.node(current).and_then(|node| node.parent) {
            chain.push(parent);
            current = parent;
        }
        chain
    }

    /// Delivers `input` to `target`, then its ancestors, until handled.
    fn bubble(&mut self, fonts: &mut Fonts, target: WidgetId, input: &Input) -> bool {
        for id in self.ancestors(target) {
            if self
                .call(fonts, id, |widget, cx| widget.event(cx, input))
                .unwrap_or(false)
            {
                return true;
            }
        }
        false
    }

    /// Handles a window input. Returns whether it was handled.
    pub fn input(&mut self, fonts: &mut Fonts, input: &Input) -> bool {
        self.apply_pending();
        let Some(root) = self
            .root
            .or_else(|| self.layers.last().map(|layer| layer.root))
        else {
            return false;
        };
        match input {
            Input::PointerMoved(position) => {
                let target = self.hit_top(*position);
                if target != self.hover {
                    if let Some(old) = self.hover {
                        self.call(fonts, old, |widget, cx| widget.hover_changed(cx, false));
                    }
                    self.hover = target;
                    if let Some(new) = target {
                        self.call(fonts, new, |widget, cx| widget.hover_changed(cx, true));
                    }
                    self.hover_tip(fonts, target);
                }
                match self.capture.or(target) {
                    Some(target) => self.bubble(fonts, target, input),
                    None => false,
                }
            }
            Input::PointerLeft => {
                if let Some(old) = self.hover.take() {
                    self.call(fonts, old, |widget, cx| widget.hover_changed(cx, false));
                }
                self.hover_tip(fonts, None);
                match self.capture.or(self.focus) {
                    Some(target) => self.bubble(fonts, target, input),
                    None => false,
                }
            }
            Input::PointerButton {
                pressed, position, ..
            } => {
                if *pressed {
                    self.set_keyboard(false);
                    // A press hides the tooltip until the pointer moves on.
                    self.close_tip(fonts);
                    self.tip_wait = None;
                }
                self.buttons_held = if *pressed {
                    self.buttons_held.saturating_add(1)
                } else {
                    self.buttons_held.saturating_sub(1)
                };
                if *pressed && self.capture.is_none() && self.dismiss_outside(fonts, *position) {
                    return true;
                }
                let target = self.capture.or_else(|| self.hit_top(*position));
                // A click beside a dialog does nothing, not even unfocus.
                let blocked = target.is_none() && self.modal().is_some();
                if *pressed && self.capture.is_none() && !blocked {
                    // Click to focus: the nearest focusable widget, or none.
                    let focus = target.and_then(|target| {
                        self.ancestors(target)
                            .into_iter()
                            .find(|&id| self.node(id).is_some_and(|node| node.widget.focusable()))
                    });
                    let keeps = focus.is_some_and(|id| {
                        self.node(id)
                            .is_some_and(|node| !node.widget.focus_on_click())
                    });
                    if !keeps {
                        self.set_focus(fonts, focus);
                    }
                    self.capture = target;
                    self.implicit_capture = true;
                }
                let handled = match target {
                    Some(target) => self.bubble(fonts, target, input),
                    None => false,
                };
                // A drag with the left button doesn't end when the right
                // one is released.
                if !*pressed && self.implicit_capture && self.buttons_held == 0 {
                    self.capture = None;
                    self.implicit_capture = false;
                }
                handled
            }
            Input::Scroll { position, .. } => match self.hit_top(*position) {
                Some(target) => self.bubble(fonts, target, input),
                None => false,
            },
            Input::WindowFocus(false) => {
                // Buttons released in another window never reach us.
                let handled = match self.focus {
                    Some(target) => self.bubble(fonts, target, input),
                    None => self.bubble(fonts, root, input),
                };
                self.buttons_held = 0;
                self.drop_capture(fonts);
                handled
            }
            Input::Key { .. } | Input::Ime(_) | Input::Paste(_) | Input::WindowFocus(_) => {
                if let Input::Key { pressed: true, .. } = input {
                    // Typing hides a tooltip; it comes back on the next rest.
                    self.close_tip(fonts);
                    self.tip_wait = None;
                }
                if let Input::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = input
                    && navigates(key, modifiers)
                {
                    self.set_keyboard(true);
                }
                // Keys go to the focus, unless it is behind a dialog (one that
                // just opened, or has nothing to focus): then to the dialog.
                let fallback = self.modal().map_or(root, |modal| modal.root);
                let target = self
                    .focus
                    .filter(|&focus| self.reachable(focus))
                    .unwrap_or(fallback);
                // Esc closes a popover that opened without taking the focus
                // (a note next to a button) before the focus sees it: the
                // board would take it to clear its selection (shell.md §6:
                // Esc closes the topmost popover).
                if let Input::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                } = input
                    && let Some(top) = self.layers.iter().rev().find(|layer| {
                        layer.options.dismissable && layer.options.kind != LayerKind::Tooltip
                    })
                    && !self.ancestors(target).contains(&top.root)
                {
                    let id = top.id;
                    return self.close_layer(fonts, id);
                }
                let handled = self.bubble(fonts, target, input);
                if !handled
                    && let Input::Key {
                        key: Key::Escape,
                        pressed: true,
                        ..
                    } = input
                    && let Some(top) = self.layers.iter().rev().find(|layer| {
                        layer.options.dismissable && layer.options.kind != LayerKind::Tooltip
                    })
                {
                    let id = top.id;
                    return self.close_layer(fonts, id);
                }
                if !handled
                    && let Input::Key {
                        key: Key::Tab,
                        pressed: true,
                        modifiers,
                        ..
                    } = input
                {
                    self.focus_next(fonts, !modifiers.shift);
                    return true;
                }
                handled
            }
        }
    }

    fn focusables(&self, id: WidgetId, out: &mut Vec<WidgetId>) {
        let Some(node) = self.node(id) else {
            return;
        };
        if node.bounds.width() <= 0.0 || node.bounds.height() <= 0.0 {
            return;
        }
        if node.widget.focusable() {
            out.push(id);
        }
        for &child in &node.children {
            self.focusables(child, out);
        }
    }

    /// Moves the focus to the next (or previous) focusable widget in tree
    /// order, wrapping around.
    pub fn focus_next(&mut self, fonts: &mut Fonts, forward: bool) {
        // Tab stays inside the top dialog, or inside the layer the focus
        // is in (a menu), or else in the window's root.
        let scope = match (self.modal(), self.focus.map(|focus| self.top_of(focus))) {
            (Some(modal), _) => Some(modal.root),
            (None, Some(top)) if self.layers.iter().any(|layer| layer.root == top) => Some(top),
            _ => self.root,
        };
        let Some(scope) = scope else {
            return;
        };
        let mut order = Vec::new();
        self.focusables(scope, &mut order);
        if order.is_empty() {
            return;
        }
        let at = self
            .focus
            .and_then(|focus| order.iter().position(|&id| id == focus));
        let next = match (at, forward) {
            (Some(at), true) => (at + 1) % order.len(),
            (Some(at), false) => (at + order.len() - 1) % order.len(),
            (None, true) => 0,
            (None, false) => order.len() - 1,
        };
        self.set_focus(fonts, Some(order[next]));
    }

    /// Where the focused widget wants the input method's window.
    pub fn ime_area(&mut self, fonts: &mut Fonts) -> Option<Rect> {
        let focus = self.focus?;
        self.call(fonts, focus, |widget, cx| widget.ime_area(cx))
            .flatten()
    }

    /// The earliest time a widget (or a layer fading in) wants to be woken.
    pub fn wake_at(&self) -> Option<Instant> {
        let widgets = self
            .slots
            .iter()
            .filter_map(|slot| slot.node.as_ref()?.widget.wake_at());
        let layers = self
            .layers
            .iter()
            .filter_map(|layer| layer.appear.wake_at());
        let tip = self.tip_wait.map(|(_, since)| since + TOOLTIP_DELAY);
        widgets.chain(layers).chain(tip).min()
    }

    /// The pointer is over `target` now: a tooltip shown for another
    /// widget goes, and one of `target`'s waits.
    fn hover_tip(&mut self, fonts: &mut Fonts, target: Option<WidgetId>) {
        // Not in the middle of a drag (drawing a shape across the tool
        // bar, dragging the bar itself).
        let dragging = self.buttons_held > 0 || self.capture.is_some();
        // The nearest widget with a tooltip: the one under the pointer, or
        // a parent whose children take the hover.
        let owner = target.filter(|_| !dragging).and_then(|target| {
            self.ancestors(target).into_iter().find(|&id| {
                self.node(id)
                    .is_some_and(|node| node.widget.tooltip().is_some())
            })
        });
        if self.tip.is_some_and(|(widget, _)| Some(widget) != owner) {
            self.close_tip(fonts);
        }
        if self.tip.is_some() {
            return;
        }
        if self.tip_wait.map(|(widget, _)| widget) != owner {
            self.tip_wait = owner.map(|id| (id, crate::animation::now()));
        }
    }

    /// Hides the tooltip, and only it ([`Ui::close_layer`] closes a
    /// tooltip alone).
    fn close_tip(&mut self, fonts: &mut Fonts) {
        if let Some((_, layer)) = self.tip.take() {
            self.close_layer(fonts, layer);
        }
    }

    /// Shows the waiting tooltip when its time came and the pointer is
    /// still there.
    fn show_tip(&mut self, now: Instant) {
        let Some((widget, since)) = self.tip_wait else {
            return;
        };
        if since + TOOLTIP_DELAY > now {
            return;
        }
        self.tip_wait = None;
        let still_over = self
            .hover
            .is_some_and(|hover| self.ancestors(hover).contains(&widget));
        if !still_over || self.buttons_held > 0 {
            return;
        }
        let Some(text) = self.node(widget).and_then(|node| node.widget.tooltip()) else {
            return;
        };
        let label = self.add(
            crate::widgets::Label::new(text.clone(), imaginarium_text::TextStyle::new(12.0))
                .color(crate::theme::Token::OnAccent)
                .single_line(),
        );
        let content = self.add_with_children(
            crate::widgets::Flex::row()
                .padding(10.0, 5.0)
                .background(crate::theme::Token::TooltipBg)
                .radius(7.0)
                .role(accesskit::Role::Tooltip, &text),
            &[label],
        );
        let placement = self
            .node(widget)
            .map_or(crate::layers::Placement::Below, |node| {
                node.widget.tooltip_placement()
            });
        let layer = self.open_layer(
            content,
            crate::layers::LayerOptions::tooltip(crate::layers::Anchor::Widget(widget))
                .placement(placement),
        );
        self.tip = Some((widget, layer));
        request_frame();
    }

    /// Wakes the widgets whose time came, and draws the next frame of
    /// layers fading in.
    pub fn wake(&mut self, fonts: &mut Fonts, now: Instant) {
        self.show_tip(now);
        for layer in &mut self.layers {
            if layer.appear.wake_at().is_some_and(|at| at <= now) {
                layer.appear.advance();
                request_frame();
            }
        }
        let due: Vec<WidgetId> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let node = slot.node.as_ref()?;
                (node.widget.wake_at()? <= now).then_some(WidgetId {
                    index: index as u32,
                    generation: slot.generation,
                })
            })
            .collect();
        for id in due {
            self.call(fonts, id, |widget, cx| widget.wake(cx, now));
        }
    }

    /// Adds the tree's accessibility nodes under the window.
    pub fn access(&mut self, fonts: &mut Fonts, tree: &mut AccessTree) {
        // Behind a dialog, nothing is offered to the screen reader.
        let roots: Vec<WidgetId> = self
            .root
            .into_iter()
            .chain(self.layers.iter().map(|layer| layer.root))
            .filter(|&root| self.reachable(root))
            .collect();
        for root in roots {
            let top = self.access_node(fonts, root, tree);
            for id in top {
                tree.add_top_level(id);
            }
        }
        if let Some(focus) = self.focus
            && tree.focus().is_none()
            && self.reachable(focus)
        {
            tree.set_focus(NodeId(focus.access_base()));
        }
    }

    /// Builds `id`'s node; returns the nodes its parent lists as children
    /// (its own, or its children's when it has none).
    fn access_node(
        &mut self,
        fonts: &mut Fonts,
        id: WidgetId,
        tree: &mut AccessTree,
    ) -> Vec<NodeId> {
        let Some(node) = self.node(id) else {
            return Vec::new();
        };
        if node.bounds.width() <= 0.0 || node.bounds.height() <= 0.0 {
            return Vec::new();
        }
        let children = node.children.clone();
        let (focused, hovered) = (self.focus == Some(id), self.hover == Some(id));
        let slot = &mut self.slots[id.index as usize];
        let Some(data) = slot.node.as_mut() else {
            return Vec::new();
        };
        let mut cx = Cx {
            id,
            bounds: data.bounds,
            content: data.content,
            focused,
            focus_visible: focused && self.keyboard,
            hovered,
            fonts: &mut *fonts,
            clipboard: &mut self.clipboard,
            access: Some(&mut *tree),
            requests: Requests::default(),
        };
        let own = data.widget.accessibility(&mut cx);
        let extra = std::mem::take(&mut cx.requests.access_children);
        let mut child_nodes = extra;
        for child in children {
            child_nodes.extend(self.access_node(fonts, child, tree));
        }
        match own {
            Some(mut node) => {
                let node_id = NodeId(id.access_base());
                if self.node(id).is_some_and(|data| data.widget.focusable()) {
                    node.add_action(Action::Focus);
                }
                node.set_children(child_nodes);
                tree.push(node_id, node);
                vec![node_id]
            }
            None => child_nodes,
        }
    }

    /// The widget whose accessibility node `node` is, if it is still there.
    fn widget_of_access_node(&self, node: NodeId) -> Option<WidgetId> {
        let (index, generation) = WidgetId::from_access(node.0)?;
        let slot = self.slots.get(index as usize)?;
        // A late request about a removed widget names its old generation;
        // the slot's new widget doesn't answer it.
        if u64::from(slot.generation) & ACCESS_GENERATION_MASK != generation {
            return None;
        }
        Some(WidgetId {
            index,
            generation: slot.generation,
        })
    }

    /// The widget the pointer would reach at `point` (window space), from
    /// the top layer down, as a click there would: for a view deciding
    /// what a right-click is about.
    pub fn widget_at(&self, point: Point) -> Option<WidgetId> {
        self.hit_top(point)
    }

    /// The widget a screen reader knows as `label`: the first in the tree
    /// whose own node has that name. For scripted runs and tests of whole
    /// windows, which find controls the way people with a screen reader
    /// do.
    pub fn find_by_label(&mut self, fonts: &mut Fonts, label: &str) -> Option<WidgetId> {
        let mut tree = crate::access::AccessTree::default();
        self.access(fonts, &mut tree);
        let (node, _) = tree
            .nodes()
            .iter()
            .find(|(_, node)| node.label() == Some(label))?;
        let id = self.widget_of_access_node(*node)?;
        self.node(id).is_some().then_some(id)
    }

    /// Routes a screen reader's request to the widget owning the node.
    pub fn access_action(&mut self, fonts: &mut Fonts, request: &ActionRequest) -> bool {
        let Some(id) = self.widget_of_access_node(request.target_node) else {
            return false;
        };
        // A screen reader can't click or focus what a dialog covers.
        if self.node(id).is_none() || !self.reachable(id) {
            return false;
        }
        if request.action == Action::Focus {
            // A request about a removed widget can arrive late, and its
            // slot may be empty or hold another widget by then.
            if !self.node(id).is_some_and(|node| node.widget.focusable()) {
                return false;
            }
            // A screen reader moves the focus the way the keyboard does.
            self.set_keyboard(true);
            self.set_focus(fonts, Some(id));
            // Focus on one of the widget's own nodes (an item on a board)
            // is the widget's to follow too.
            if request.target_node.0 != id.access_base() {
                self.call(fonts, id, |widget, cx| widget.access_action(cx, request));
            }
            return true;
        }
        self.call(fonts, id, |widget, cx| widget.access_action(cx, request))
            .unwrap_or(false)
    }
}

/// Whether a key press is keyboard navigation, which shows focus rings: not
/// a modifier on its own, and not a pointer chord like Ctrl+wheel or
/// Shift+click's Shift (as CSS's `:focus-visible` heuristics do).
fn navigates(key: &Key, modifiers: &crate::window::Modifiers) -> bool {
    let modifier_alone = matches!(key, Key::Other(name)
        if ["Control", "Shift", "Alt", "AltGraph", "Super", "Meta", "Hyper"].contains(&name.as_str()));
    (!modifier_alone && !(modifiers.ctrl || modifiers.alt || modifiers.meta))
        || matches!(key, Key::Tab)
}
