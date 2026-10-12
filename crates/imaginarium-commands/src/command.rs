// SPDX-License-Identifier: AGPL-3.0-or-later

//! The [`Command`] trait and its building blocks.

use crate::args::{Args, ArgsSchema};
use crate::error::CommandError;
use crate::id::CommandId;

/// What the palette, toolbars, menus and AI models show for a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandMeta {
    /// Short title in sentence case, e.g. "Align center". It doubles as the
    /// palette label.
    pub title: &'static str,
    /// One sentence on what it does. Shown as a hint and given to AI models
    /// as the tool description, so be specific.
    pub description: &'static str,
    /// Palette section, e.g. "Text", "Arrange". Defaults to the id's first
    /// segment.
    pub category: &'static str,
    /// Extra search terms (other languages, other apps' names): "fett",
    /// "strong", "make bold".
    pub synonyms: &'static [&'static str],
    /// Icon name from the icon set, if any.
    pub icon: Option<&'static str>,
}

impl CommandMeta {
    /// Metadata with the required title and description.
    pub const fn new(title: &'static str, description: &'static str) -> Self {
        Self {
            title,
            description,
            category: "",
            synonyms: &[],
            icon: None,
        }
    }

    /// Sets the palette category.
    pub const fn category(mut self, category: &'static str) -> Self {
        self.category = category;
        self
    }

    /// Sets search synonyms.
    pub const fn synonyms(mut self, synonyms: &'static [&'static str]) -> Self {
        self.synonyms = synonyms;
        self
    }

    /// Sets the icon.
    pub const fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// Whether a command can run in the current context. The UI follows this:
/// hidden commands don't appear at all (e.g. text commands with no text
/// selected), disabled ones appear greyed out with the reason as a hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Can run.
    Enabled,
    /// Visible but can't run now. The reason is shown, e.g. "Nothing to undo".
    Disabled(&'static str),
    /// Not applicable here. Not shown.
    Hidden,
}

impl Availability {
    /// Whether the command can run.
    pub const fn is_enabled(self) -> bool {
        matches!(self, Availability::Enabled)
    }
}

/// What a successful run did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Done. The document or UI changed.
    Done,
    /// Ran but changed nothing (e.g. "align" on an already aligned selection).
    Unchanged,
    /// Started a background job (export, AI generation, …). Progress and
    /// completion are reported elsewhere.
    Started,
}

/// A user-invokable action. Everything a user can do is a command. The
/// palette, contextual toolbars, keymaps, add-ons and the AI assistant all
/// use the same [`Registry`](crate::Registry).
///
/// `Cx` is the execution context provided by higher layers (selection,
/// document access, UI services). This crate never needs to know it.
/// Commands mutate documents only through transactions obtained from `Cx`
/// (MAINTAINERS.md, invariant 2).
pub trait Command<Cx>: Send + Sync {
    /// Stable id.
    fn id(&self) -> CommandId;
    /// UI and AI metadata.
    fn meta(&self) -> &CommandMeta;
    /// Accepted arguments.
    fn args(&self) -> &ArgsSchema {
        &ArgsSchema::NONE
    }
    /// Whether it can run now. Must be cheap: the palette evaluates it for
    /// every command when it opens.
    fn availability(&self, _cx: &Cx) -> Availability {
        Availability::Enabled
    }
    /// Runs the command. The registry has already checked availability and
    /// validated `args`.
    fn run(&self, cx: &mut Cx, args: &Args) -> Result<Outcome, CommandError>;
}

/// A command from plain functions, `const`-constructible, so built-in
/// commands are declared as `static`s.
///
/// When builder calls (`.args(…)`, `.availability(…)`) follow `new`, write
/// `FnCommand::<YourCx>::new(…)` so the closures' context type is known.
///
/// ```
/// use imaginarium_commands::{Args, CommandMeta, FnCommand, Outcome, Registry};
///
/// struct Cx { bold: bool }
///
/// static TOGGLE_BOLD: FnCommand<Cx> = FnCommand::new(
///     "text.toggle_bold",
///     CommandMeta::new("Bold", "Make the selected text bold, or not bold.")
///         .synonyms(&["fett", "strong"]),
///     |cx, _args| {
///         cx.bold = !cx.bold;
///         Ok(Outcome::Done)
///     },
/// );
///
/// let mut registry = Registry::new();
/// registry.register(&TOGGLE_BOLD)?;
/// let mut cx = Cx { bold: false };
/// registry.execute(&mut cx, "text.toggle_bold", &Args::new())?;
/// assert!(cx.bold);
/// # Ok::<(), imaginarium_commands::CommandError>(())
/// ```
pub struct FnCommand<Cx> {
    id: CommandId,
    meta: CommandMeta,
    args: ArgsSchema,
    availability: fn(&Cx) -> Availability,
    run: fn(&mut Cx, &Args) -> Result<Outcome, CommandError>,
}

impl<Cx> FnCommand<Cx> {
    /// Declares a command that is always enabled and takes no arguments.
    pub const fn new(
        id: &'static str,
        meta: CommandMeta,
        run: fn(&mut Cx, &Args) -> Result<Outcome, CommandError>,
    ) -> Self {
        Self {
            id: CommandId::new(id),
            meta,
            args: ArgsSchema::NONE,
            availability: always_enabled::<Cx>,
            run,
        }
    }

    /// Declares the accepted arguments.
    pub const fn args(mut self, args: ArgsSchema) -> Self {
        self.args = args;
        self
    }

    /// Sets the availability check.
    pub const fn availability(mut self, availability: fn(&Cx) -> Availability) -> Self {
        self.availability = availability;
        self
    }
}

fn always_enabled<Cx>(_cx: &Cx) -> Availability {
    Availability::Enabled
}

impl<Cx> Command<Cx> for FnCommand<Cx> {
    fn id(&self) -> CommandId {
        self.id
    }
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }
    fn args(&self) -> &ArgsSchema {
        &self.args
    }
    fn availability(&self, cx: &Cx) -> Availability {
        (self.availability)(cx)
    }
    fn run(&self, cx: &mut Cx, args: &Args) -> Result<Outcome, CommandError> {
        (self.run)(cx, args)
    }
}

/// Lets `static` commands be registered by reference: `registry.register(&CMD)`.
impl<Cx, C: Command<Cx> + ?Sized> Command<Cx> for &'static C {
    fn id(&self) -> CommandId {
        (**self).id()
    }
    fn meta(&self) -> &CommandMeta {
        (**self).meta()
    }
    fn args(&self) -> &ArgsSchema {
        (**self).args()
    }
    fn availability(&self, cx: &Cx) -> Availability {
        (**self).availability(cx)
    }
    fn run(&self, cx: &mut Cx, args: &Args) -> Result<Outcome, CommandError> {
        (**self).run(cx, args)
    }
}
