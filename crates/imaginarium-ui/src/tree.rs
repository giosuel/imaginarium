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
use imaginarium_render::kurbo::{Point, Rect, Size};
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

impl WidgetId {
    /// The first accessibility node id of this widget. A widget's own node
    /// is this; it may use the next 2³² − 1 ids for nodes of its own
    /// (text runs, canvas items).
    pub fn access_base(self) -> u64 {
        (u64::from(self.index) + 1) << 32
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

    /// Its accessibility node (`None`: transparent, its children belong to
    /// the parent's node). Extra nodes (text runs, items it draws itself)
    /// go into [`Cx::access_tree`] with ids from [`WidgetId::access_base`],
    /// listed with [`Cx::add_access_child`].
    fn accessibility(&mut self, cx: &mut Cx<'_>) -> Option<Node> {
        let _ = cx;
        None
    }

    /// A screen reader's request aimed at one of its nodes. `true` if it
    /// did something. (Focus is handled by the tree.)
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
    focus: Option<bool>,
    capture: Option<bool>,
    close_layer: bool,
    access_children: Vec<NodeId>,
}

/// A widget's view of the tree during a call.
pub struct Cx<'a> {
    id: WidgetId,
    bounds: Rect,
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
        self.layers.push(Layer {
            id,
            root: content,
            options,
            return_focus: self.focus,
            focus_pending,
            fresh: true,
        });
        self.layout_dirty = true;
        request_frame();
        id
    }

    /// Closes layer `id` and the layers opened after it (submenus), from
    /// the top: their widgets are removed, the focus goes back to where it
    /// was when they opened, and their `on_close` callbacks run. `false`
    /// if it wasn't open.
    pub fn close_layer(&mut self, fonts: &mut Fonts, id: LayerId) -> bool {
        let Some(index) = self.layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        let mut closed: Vec<Layer> = self.layers.drain(index..).collect();
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
        self.layout_layers(fonts, area);
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
            self.capture = None;
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
                    self.capture = None;
                    self.implicit_capture = false;
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
        if let Some(node) = self.node_mut(id)
            && node.bounds != bounds
        {
            node.bounds = bounds;
            node.paint_dirty = true;
        }
        for child in children {
            self.place(child, origin);
        }
    }

    /// Paints what changed and composes every widget's scene into `scene`.
    pub fn paint(&mut self, fonts: &mut Fonts, scene: &mut Scene) {
        self.stats.repainted = 0;
        self.stats.widgets = self.slots.iter().filter(|slot| slot.node.is_some()).count();
        if let Some(root) = self.root {
            self.paint_node(fonts, root, scene);
        }
        let layers: Vec<(WidgetId, LayerKind)> = self
            .layers
            .iter()
            .map(|layer| (layer.root, layer.options.kind))
            .collect();
        for (root, kind) in layers {
            if kind == LayerKind::Modal {
                let scrim = crate::theme::theme().color(crate::theme::Token::Scrim);
                scene.fill(&self.area, scrim);
            }
            self.paint_node(fonts, root, scene);
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
        node.children
            .iter()
            .rev()
            .find_map(|&child| self.hit(child, point))
            .or(inside.then_some(id))
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
                    self.set_focus(fonts, focus);
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
                if self.implicit_capture {
                    self.capture = None;
                    self.implicit_capture = false;
                }
                handled
            }
            Input::Key { .. } | Input::Ime(_) | Input::Paste(_) | Input::WindowFocus(_) => {
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

    /// The earliest time a widget wants to be woken.
    pub fn wake_at(&self) -> Option<Instant> {
        self.slots
            .iter()
            .filter_map(|slot| slot.node.as_ref()?.widget.wake_at())
            .min()
    }

    /// Wakes the widgets whose time came.
    pub fn wake(&mut self, fonts: &mut Fonts, now: Instant) {
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

    /// Routes a screen reader's request to the widget owning the node.
    pub fn access_action(&mut self, fonts: &mut Fonts, request: &ActionRequest) -> bool {
        let index = (request.target_node.0 >> 32).checked_sub(1);
        let Some(index) = index.and_then(|index| u32::try_from(index).ok()) else {
            return false;
        };
        let Some(slot) = self.slots.get(index as usize) else {
            return false;
        };
        let id = WidgetId {
            index,
            generation: slot.generation,
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
