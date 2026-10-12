// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command [`Registry`].

use std::collections::BTreeMap;

use serde_json::{Value as Json, json};

use crate::args::Args;
use crate::command::{Availability, Command, CommandMeta, Outcome};
use crate::error::CommandError;
use crate::id::CommandId;
use crate::search::SearchIndex;

/// All registered commands for a context type `Cx`.
pub struct Registry<Cx> {
    commands: BTreeMap<CommandId, Box<dyn Command<Cx>>>,
}

impl<Cx> Default for Registry<Cx> {
    fn default() -> Self {
        Self {
            commands: BTreeMap::new(),
        }
    }
}

impl<Cx> Registry<Cx> {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a command. Static commands can be passed by reference
    /// (`registry.register(&MY_COMMAND)`). Rejects malformed and duplicate ids.
    pub fn register(&mut self, command: impl Command<Cx> + 'static) -> Result<(), CommandError> {
        let id = command.id();
        id.validate()?;
        if self.commands.contains_key(&id) {
            return Err(CommandError::Duplicate(id.as_str()));
        }
        self.commands.insert(id, Box::new(command));
        Ok(())
    }

    /// Looks up a command by id string.
    pub fn get(&self, id: &str) -> Option<&dyn Command<Cx>> {
        self.commands.get(id).map(|command| command.as_ref())
    }

    /// All commands, sorted by id.
    pub fn commands(&self) -> impl Iterator<Item = &dyn Command<Cx>> {
        self.commands.values().map(|command| command.as_ref())
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether no commands are registered.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Availability of every command in `cx`. The palette takes this
    /// snapshot once when it opens, not per keystroke.
    pub fn availability(&self, cx: &Cx) -> Vec<(CommandId, Availability)> {
        self.commands
            .iter()
            .map(|(id, command)| (*id, command.availability(cx)))
            .collect()
    }

    /// Runs a command: looks it up, checks availability, validates the
    /// arguments, then runs it. This is the **single entry point** for user,
    /// keymap, add-on and AI invocations.
    pub fn execute(&self, cx: &mut Cx, id: &str, args: &Args) -> Result<Outcome, CommandError> {
        let command = self
            .get(id)
            .ok_or_else(|| CommandError::Unknown(id.to_owned()))?;
        let id = command.id();
        match command.availability(cx) {
            Availability::Enabled => {}
            Availability::Disabled(reason) => {
                return Err(CommandError::Unavailable {
                    id: id.as_str(),
                    reason: reason.to_owned(),
                });
            }
            Availability::Hidden => {
                return Err(CommandError::Unavailable {
                    id: id.as_str(),
                    reason: "not applicable here".to_owned(),
                });
            }
        }
        command.args().validate(id, args)?;
        command.run(cx, args)
    }

    /// AI tool definitions (`name`, `description`, `input_schema`) of the
    /// commands enabled in `cx` (ADR-0013). Tool names are
    /// [`CommandId::tool_name`]s.
    pub fn tool_definitions(&self, cx: &Cx) -> Vec<Json> {
        self.commands
            .values()
            .filter(|command| command.availability(cx).is_enabled())
            .map(|command| {
                let meta = command.meta();
                json!({
                    "name": command.id().tool_name(),
                    "description": format!("{}: {}", meta.title, meta.description),
                    "input_schema": command.args().to_json_schema(),
                })
            })
            .collect()
    }

    /// Runs a command from an AI tool call (`name` as returned by
    /// [`Registry::tool_definitions`], `input` as JSON). Same checks as
    /// [`Registry::execute`].
    pub fn execute_tool_call(
        &self,
        cx: &mut Cx,
        tool_name: &str,
        input: &Json,
    ) -> Result<Outcome, CommandError> {
        let id = tool_name.replace("__", ".");
        let command = self
            .get(&id)
            .ok_or_else(|| CommandError::Unknown(tool_name.to_owned()))?;
        let args = command.args().args_from_json(command.id(), input)?;
        self.execute(cx, &id, &args)
    }

    /// Builds the palette's search index over all commands.
    pub fn search_index(&self) -> SearchIndex {
        let mut index = SearchIndex::new();
        for command in self.commands.values() {
            let id = command.id();
            let meta: &CommandMeta = command.meta();
            let category = if meta.category.is_empty() {
                id.as_str().split('.').next().unwrap_or("")
            } else {
                meta.category
            };
            index.insert(id, meta.title, meta.synonyms, category);
        }
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{ArgKind, ArgSpec, ArgValue, ArgsSchema};
    use crate::command::FnCommand;
    use crate::search::UsageStats;

    #[derive(Default)]
    struct Cx {
        selection: usize,
        zoom: f64,
        log: Vec<&'static str>,
    }

    static ZOOM_TO: FnCommand<Cx> = FnCommand::<Cx>::new(
        "view.zoom_to",
        CommandMeta::new("Zoom to", "Set the zoom level.").synonyms(&["magnify"]),
        |cx, args| {
            cx.zoom = args.float("level").unwrap_or(1.0);
            Ok(Outcome::Done)
        },
    )
    .args(ArgsSchema {
        args: &[ArgSpec {
            name: "level",
            description: "Zoom factor, 1 = 100 %",
            kind: ArgKind::Float {
                min: Some(0.1),
                max: Some(8.0),
            },
            required: true,
        }],
    });

    static DELETE: FnCommand<Cx> = FnCommand::<Cx>::new(
        "edit.delete",
        CommandMeta::new("Delete", "Delete the selection.").category("Edit"),
        |cx, _| {
            cx.log.push("delete");
            Ok(Outcome::Done)
        },
    )
    .availability(|cx| {
        if cx.selection == 0 {
            Availability::Disabled("Select something first")
        } else {
            Availability::Enabled
        }
    });

    static TEXT_ONLY: FnCommand<Cx> = FnCommand::<Cx>::new(
        "text.bold",
        CommandMeta::new("Bold", "Make the selected text bold."),
        |_, _| Ok(Outcome::Unchanged),
    )
    .availability(|_| Availability::Hidden);

    fn registry() -> Registry<Cx> {
        let mut registry = Registry::new();
        registry.register(&ZOOM_TO).unwrap();
        registry.register(&DELETE).unwrap();
        registry.register(&TEXT_ONLY).unwrap();
        registry
    }

    #[test]
    fn executes_with_validated_arguments() {
        let registry = registry();
        let mut cx = Cx::default();
        let args = Args::new().with("level", ArgValue::Float(2.0));
        assert_eq!(
            registry.execute(&mut cx, "view.zoom_to", &args),
            Ok(Outcome::Done)
        );
        assert_eq!(cx.zoom, 2.0);
        let bad = Args::new().with("level", ArgValue::Float(20.0));
        assert!(matches!(
            registry.execute(&mut cx, "view.zoom_to", &bad),
            Err(CommandError::InvalidArgs { .. })
        ));
        assert_eq!(cx.zoom, 2.0, "invalid args don't run the command");
    }

    #[test]
    fn availability_is_enforced() {
        let registry = registry();
        let mut cx = Cx::default();
        assert_eq!(
            registry.execute(&mut cx, "edit.delete", &Args::new()),
            Err(CommandError::Unavailable {
                id: "edit.delete",
                reason: "Select something first".into()
            })
        );
        assert!(
            registry
                .execute(&mut cx, "text.bold", &Args::new())
                .is_err(),
            "hidden"
        );
        cx.selection = 1;
        assert!(
            registry
                .execute(&mut cx, "edit.delete", &Args::new())
                .is_ok()
        );
        assert_eq!(cx.log, ["delete"]);
        assert_eq!(
            registry.execute(&mut cx, "nope.nothing", &Args::new()),
            Err(CommandError::Unknown("nope.nothing".into()))
        );
    }

    #[test]
    fn rejects_duplicate_and_malformed_ids() {
        let mut registry = registry();
        assert_eq!(
            registry.register(&ZOOM_TO),
            Err(CommandError::Duplicate("view.zoom_to"))
        );
        static BAD: FnCommand<Cx> = FnCommand::new("Zoom", CommandMeta::new("Z", "Z."), |_, _| {
            Ok(Outcome::Done)
        });
        assert!(matches!(
            registry.register(&BAD),
            Err(CommandError::InvalidId { .. })
        ));
        assert_eq!(registry.len(), 3);
    }

    #[test]
    fn availability_snapshot_covers_all_commands() {
        let registry = registry();
        let snapshot = registry.availability(&Cx::default());
        assert_eq!(snapshot.len(), 3);
        assert!(snapshot.contains(&(CommandId::new("text.bold"), Availability::Hidden)));
    }

    #[test]
    fn tool_definitions_and_tool_calls() {
        let registry = registry();
        let mut cx = Cx::default();
        let tools = registry.tool_definitions(&cx);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["view__zoom_to"],
            "only enabled commands are offered"
        );
        assert_eq!(tools[0]["input_schema"]["required"], json!(["level"]));

        let outcome = registry.execute_tool_call(&mut cx, "view__zoom_to", &json!({ "level": 3 }));
        assert_eq!(outcome, Ok(Outcome::Done));
        assert_eq!(cx.zoom, 3.0);
        assert!(
            registry
                .execute_tool_call(&mut cx, "view__zoom_to", &json!({ "level": "huge" }))
                .is_err()
        );
        assert!(
            registry
                .execute_tool_call(&mut cx, "edit__delete", &json!({}))
                .is_err(),
            "tool calls respect availability too"
        );
    }

    #[test]
    fn search_index_includes_synonyms_and_default_categories() {
        let registry = registry();
        let index = registry.search_index();
        assert_eq!(index.len(), 3);
        let hits = index.search("magnify", &UsageStats::new(), 0, 5);
        assert_eq!(hits[0].id.as_str(), "view.zoom_to");
        let hits = index.search("edit", &UsageStats::new(), 0, 5);
        assert_eq!(hits[0].id.as_str(), "edit.delete");
    }
}
