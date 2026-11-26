//! Tool manifests (T1).
//!
//! One declaration per tool: name, version, description, JSON schema, risk
//! classes, capability needs, and source. The registry validates a manifest
//! when its tool registers, generates the model-facing specs from it, and ties
//! it to the approval policy table in [`crate::tools::safety`], so the three
//! views of one tool - what the model is told, what the gate decides, and what
//! the code declares - cannot drift apart.
//!
//! A manifest is a property of the code, so a tool without one does not
//! compile: [`super::Tool::manifest`] has no default. The checks here are the
//! ones the compiler cannot make - that the declaration is usable as a tool
//! schema and that the policy table agrees with it.

use std::fmt;

use serde_json::Value;

/// The manifest version every built-in tool declares.
///
/// Bump it when a tool's declaration changes in a way an external consumer
/// (T2's host, a future plugin) would notice: a renamed field, a removed
/// property, a narrower enum.
pub const CORE_VERSION: &str = "1.0.0";

/// The longest model-facing description a manifest may carry.
///
/// Descriptions go into every request, so an accidental paste of a page of
/// prose is a cost, not a feature. The current tools are well under this.
pub const MAX_DESCRIPTION_CHARS: usize = 2_000;

/// The longest summary a manifest may carry.
///
/// The summary is the one line the system prompt repeats for every offered
/// tool on every turn, so it stays a line: one sentence naming what the tool is
/// for, not a second description.
pub const MAX_SUMMARY_CHARS: usize = 200;

/// Where a tool comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Compiled into this binary and covered by the approval policy table.
    Core,
    /// A tool an external host registers (T2). It uses the same manifest shape;
    /// the registry refuses it until that host exists, because it would have no
    /// policy row.
    External,
}

/// What a tool can do, so the policy table and the documentation can be
/// checked against one declaration rather than a comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskClass {
    /// Observes state without changing it.
    Read,
    /// Changes user data, files, or application state.
    Mutate,
    /// Types text or presses keys.
    Input,
    /// Runs a program, command, or remote model call.
    Execute,
    /// Talks to a network service.
    Network,
    /// Changes operating-system state such as the clipboard, windows, or input
    /// devices.
    System,
}

/// An operating-system capability a tool needs, matching the D3 vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    /// Synthesises keyboard or pointer input.
    Input,
    /// Captures the screen.
    Screenshot,
    /// Reads or writes another application's accessibility tree.
    Accessibility,
}

impl Capability {
    pub fn name(self) -> &'static str {
        match self {
            Capability::Input => "input",
            Capability::Screenshot => "screenshot",
            Capability::Accessibility => "accessibility",
        }
    }
}

/// Where a manifest's argument schema comes from.
///
/// A core tool names a function beside its code, so its schema stays where it
/// is written. An external tool receives its schema in the host handshake
/// (T2), so it keeps the parsed value instead; the host leaks that value for
/// the process lifetime, because the server set is fixed at startup.
#[derive(Clone, Copy, Debug)]
pub enum Schema {
    /// A core tool's schema, built on demand from a function.
    Fn(fn() -> Value),
    /// A schema received at run time, valid for the process lifetime.
    Owned(&'static Value),
}

impl Schema {
    /// The schema as a JSON value.
    pub fn build(self) -> Value {
        match self {
            Schema::Fn(build) => build(),
            Schema::Owned(schema) => schema.clone(),
        }
    }
}

/// One action a dispatching tool accepts, with the arguments it needs (T1/W2).
///
/// The table is the single place that says which arguments belong to which
/// action; the schema cannot express it without a branch per action, and the
/// descriptions are prose the model has to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolAction {
    /// The value a call puts in `action`.
    pub name: &'static str,
    /// Arguments the action cannot run without.
    pub required: &'static [&'static str],
    /// Arguments the action reads when they are present.
    pub optional: &'static [&'static str],
}

/// One tool's declaration.
///
/// `Copy` because every field is a constant, so the registry can hand a
/// manifest back without borrowing the tool behind a lock.
#[derive(Clone, Copy, Debug)]
pub struct ToolManifest {
    /// The name a model call uses; must equal [`super::Tool::name`].
    pub name: &'static str,
    /// A `major.minor.patch` version for consumers of this declaration.
    pub version: &'static str,
    /// The model-facing description.
    pub description: &'static str,
    /// The one line the prompt's tool guide shows for this tool.
    ///
    /// Written by hand rather than cut out of the description: the guide used
    /// to take everything before the first `". "`, which mis-splits a
    /// description containing "e.g. " and silently drops the rest of the
    /// sentence. A field makes the intended one-liner explicit, and
    /// [`conformance`](ToolManifest::conformance) checks it.
    pub summary: &'static str,
    /// The JSON Schema for the arguments, either a core function or a value
    /// received in the T2 handshake.
    pub schema: Schema,
    /// The actions a dispatching tool accepts, with their arguments.
    ///
    /// `None` for a tool with one shape (`shell`, `process`, `web_search`), so
    /// the table never has to describe a tool that has nothing to dispatch on.
    pub actions: Option<&'static [ToolAction]>,
    /// At least one class; an unclassified tool is a bug.
    pub risk: &'static [RiskClass],
    /// The OS capabilities the tool needs, from the D3 vocabulary.
    pub capabilities: &'static [Capability],
    /// Core tools live here; external ones arrive with T2.
    pub source: Source,
}

/// Why a manifest cannot be used as a tool declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestError {
    EmptyName,
    InvalidVersion(&'static str),
    EmptyDescription,
    LongDescription(usize),
    EmptySummary,
    LongSummary(usize),
    SchemaNotObject,
    SchemaTypeNotObject,
    SchemaPropertiesNotObject,
    RequiredNotAString,
    RequiredNotDeclared(String),
    NoRiskClass,
    ActionWithoutRow(String),
    MissingActionTable,
    ArgumentNotInSchema { action: String, argument: String },
    SchemaNotClosed,
    OrphanProperty(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::EmptyName => write!(formatter, "the name is empty"),
            ManifestError::InvalidVersion(version) => write!(
                formatter,
                "the version {version:?} is not major.minor.patch"
            ),
            ManifestError::EmptyDescription => write!(formatter, "the description is empty"),
            ManifestError::LongDescription(length) => write!(
                formatter,
                "the description is {length} characters, over the {MAX_DESCRIPTION_CHARS} limit"
            ),
            ManifestError::EmptySummary => write!(formatter, "the summary is empty"),
            ManifestError::LongSummary(length) => write!(
                formatter,
                "the summary is {length} characters, over the {MAX_SUMMARY_CHARS} limit"
            ),
            ManifestError::SchemaNotObject => write!(formatter, "the schema is not an object"),
            ManifestError::SchemaTypeNotObject => {
                write!(formatter, "the schema's type is not \"object\"")
            }
            ManifestError::SchemaPropertiesNotObject => {
                write!(formatter, "the schema's properties are not an object")
            }
            ManifestError::RequiredNotAString => {
                write!(formatter, "a required entry is not a property name")
            }
            ManifestError::RequiredNotDeclared(name) => write!(
                formatter,
                "the schema requires {name:?} but does not declare it in properties"
            ),
            ManifestError::NoRiskClass => write!(formatter, "no risk class is declared"),
            ManifestError::OrphanProperty(name) => write!(
                formatter,
                "the schema declares {name:?} but no action accepts it"
            ),
            ManifestError::SchemaNotClosed => write!(
                formatter,
                "the schema does not set additionalProperties to false"
            ),
            ManifestError::MissingActionTable => write!(
                formatter,
                "the schema declares actions but the manifest has no action table"
            ),
            ManifestError::ActionWithoutRow(name) => write!(
                formatter,
                "the schema declares the action {name:?} but the action table has no row for it"
            ),
            ManifestError::ArgumentNotInSchema { action, argument } => write!(
                formatter,
                "the table's {action:?} row names {argument:?}, which the schema does not declare"
            ),
        }
    }
}

/// `major.minor.patch`, all numeric. Deliberately narrow: the registry only
/// needs to tell versions apart, and accepting more would let a typo through.
/// The external host (T2) reuses it on a version a server declares.
pub(crate) fn valid_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();

    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}

impl ToolManifest {
    /// Check the declaration against what the model-facing contract needs.
    ///
    /// Run by [`super::ToolRegistry::register`] for every tool, and available
    /// to a tool's own tests so a schema change can be checked where it is
    /// written.
    pub fn conformance(&self) -> Result<(), ManifestError> {
        if self.name.trim().is_empty() {
            return Err(ManifestError::EmptyName);
        }

        if !valid_version(self.version) {
            return Err(ManifestError::InvalidVersion(self.version));
        }

        let description = self.description.trim();

        if description.is_empty() {
            return Err(ManifestError::EmptyDescription);
        }

        let length = description.chars().count();

        if length > MAX_DESCRIPTION_CHARS {
            return Err(ManifestError::LongDescription(length));
        }

        let summary = self.summary.trim();

        if summary.is_empty() {
            return Err(ManifestError::EmptySummary);
        }

        let length = summary.chars().count();

        if length > MAX_SUMMARY_CHARS {
            return Err(ManifestError::LongSummary(length));
        }

        if self.risk.is_empty() {
            return Err(ManifestError::NoRiskClass);
        }

        let schema = self.schema.build();
        let Some(object) = schema.as_object() else {
            return Err(ManifestError::SchemaNotObject);
        };

        if object.get("type").and_then(Value::as_str) != Some("object") {
            return Err(ManifestError::SchemaTypeNotObject);
        }

        if self.source == Source::Core
            && object.get("additionalProperties").and_then(Value::as_bool) != Some(false)
        {
            return Err(ManifestError::SchemaNotClosed);
        }

        let properties = match object.get("properties") {
            Some(properties) => match properties.as_object() {
                Some(properties) => Some(properties),
                None => return Err(ManifestError::SchemaPropertiesNotObject),
            },
            None => None,
        };

        if let Some(properties) = properties {
            let declared = properties
                .get("action")
                .and_then(|action| action.get("enum"))
                .and_then(Value::as_array);

            if declared.is_some() && self.actions.is_none() && self.source == Source::Core {
                return Err(ManifestError::MissingActionTable);
            }

            if let (Some(actions), Some(declared)) = (self.actions, declared) {
                for entry in declared {
                    if let Some(name) = entry.as_str() {
                        if !actions.iter().any(|row| row.name == name) {
                            return Err(ManifestError::ActionWithoutRow(name.to_string()));
                        }
                    }
                }

                for row in actions {
                    for argument in row.required.iter().chain(row.optional.iter()) {
                        if !properties.contains_key(*argument) {
                            return Err(ManifestError::ArgumentNotInSchema {
                                action: row.name.to_string(),
                                argument: (*argument).to_string(),
                            });
                        }
                    }
                }

                for key in properties.keys() {
                    let named = key == "action"
                        || actions.iter().any(|row| {
                            row.required.contains(&key.as_str())
                                || row.optional.contains(&key.as_str())
                        });

                    if !named {
                        return Err(ManifestError::OrphanProperty(key.clone()));
                    }
                }
            }
        }

        let Some(required) = object.get("required") else {
            return Ok(());
        };

        let Some(required) = required.as_array() else {
            return Err(ManifestError::RequiredNotAString);
        };

        for entry in required {
            let Some(name) = entry.as_str() else {
                return Err(ManifestError::RequiredNotAString);
            };

            if !properties.is_some_and(|properties| properties.contains_key(name)) {
                return Err(ManifestError::RequiredNotDeclared(name.to_string()));
            }
        }

        Ok(())
    }

    /// True when the manifest declares this capability.
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// Why a call cannot run, or `None` when it can.
    ///
    /// Checked once before dispatch (W2), so a model's missing argument fails
    /// with one message naming the action and what it accepts, instead of
    /// reaching the tool and coming back with tool-specific wording after a
    /// whole round trip.
    pub fn argument_error(&self, arguments: &Value) -> Option<String> {
        let Some(actions) = self.actions else {
            return self.undeclared_key_error(arguments);
        };
        let action = arguments.get("action")?.as_str()?;
        let Some(row) = actions.iter().find(|row| row.name == action) else {
            let names: Vec<&str> = actions.iter().map(|row| row.name).collect();

            return Some(format!(
                "unknown action '{action}'; this tool accepts: {}",
                names.join(", ")
            ));
        };

        let accepted: Vec<&str> = row
            .required
            .iter()
            .chain(row.optional.iter())
            .copied()
            .collect();

        if let Some(map) = arguments.as_object() {
            let mut unknown: Vec<&str> = map
                .keys()
                .map(String::as_str)
                .filter(|key| *key != "action" && !accepted.contains(key))
                .collect();

            if !unknown.is_empty() {
                unknown.sort_unstable();

                return Some(format!(
                    "action '{action}' does not accept {}; it accepts: {}",
                    unknown.join(", "),
                    accepted.join(", ")
                ));
            }
        }

        let missing: Vec<&str> = row
            .required
            .iter()
            .copied()
            .filter(|name| arguments.get(*name).is_none())
            .collect();

        if missing.is_empty() {
            return None;
        }

        Some(format!(
            "action '{action}' requires {}; it accepts: {}",
            missing.join(", "),
            accepted.join(", ")
        ))
    }

    /// The action list the model reads in the schema, with required arguments.
    ///
    /// Generated from the same table the gate decides with (#6), so the text
    /// cannot describe a different set of actions from the one that runs.
    pub fn action_hint(&self) -> Option<String> {
        let actions = self.actions?;

        let listed: Vec<String> = actions
            .iter()
            .map(|row| {
                if row.required.is_empty() {
                    row.name.to_string()
                } else {
                    format!("{}({})", row.name, row.required.join(", "))
                }
            })
            .collect();

        Some(format!("Actions: {}.", listed.join(", ")))
    }

    /// The tool's actions as text, for the free `help` action (#6).
    ///
    /// Generated from the same table the gate decides with, so the answer
    /// cannot drift from the validation.
    pub fn help_text(&self) -> Option<String> {
        let actions = self.actions?;
        let listed = |names: &'static [&'static str]| {
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        };

        let mut text = format!("{} actions:\n", self.name);

        for row in actions {
            text.push_str(&format!(
                "- {}: required {}; optional {}\n",
                row.name,
                listed(row.required),
                listed(row.optional)
            ));
        }

        Some(text)
    }

    /// A single-shape tool has no action row, so its closure is checked against
    /// the schema's own properties.
    fn undeclared_key_error(&self, arguments: &Value) -> Option<String> {
        let schema = self.schema.build();
        let properties = schema.get("properties")?.as_object()?;
        let supplied = arguments.as_object()?;

        let mut unknown: Vec<&str> = supplied
            .keys()
            .map(String::as_str)
            .filter(|key| !properties.contains_key(*key))
            .collect();

        if unknown.is_empty() {
            return None;
        }

        unknown.sort_unstable();

        let mut accepted: Vec<&str> = properties.keys().map(String::as_str).collect();
        accepted.sort_unstable();

        Some(format!(
            "this tool does not accept {}; it accepts: {}",
            unknown.join(", "),
            accepted.join(", ")
        ))
    }
}
/// A minimal valid manifest for a test double.
#[cfg(test)]
pub fn stub(name: &'static str) -> ToolManifest {
    ToolManifest {
        name,
        version: "0.0.0",
        description: "test tool",
        summary: "test tool",
        schema: Schema::Fn(stub_schema),
        actions: None,
        risk: &[RiskClass::Read],
        capabilities: &[],
        source: Source::Core,
    }
}

/// The empty object schema a test double accepts.
#[cfg(test)]
pub fn stub_schema() -> Value {
    serde_json::json!({ "type": "object", "additionalProperties": false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string" },
                "text": { "type": "string" }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    /// The same shape as [`schema`], left open to unknown keys.
    fn open_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string" },
                "text": { "type": "string" }
            },
            "required": ["action"]
        })
    }

    /// A schema that declares two actions, for the table-coverage checks.
    fn two_action_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["scroll", "drag"] },
                "amount": { "type": "integer" },
                "from": { "type": "object" },
                "to": { "type": "object" }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    fn manifest() -> ToolManifest {
        ToolManifest {
            name: "fixture",
            version: CORE_VERSION,
            description: "A fixture tool.",
            summary: "A fixture tool.",
            schema: Schema::Fn(schema),
            actions: None,
            risk: &[RiskClass::Read, RiskClass::Mutate],
            capabilities: &[Capability::Input],
            source: Source::Core,
        }
    }

    #[test]
    fn a_complete_manifest_passes() {
        assert_eq!(manifest().conformance(), Ok(()));
        assert!(manifest().has_capability(Capability::Input));
        assert!(!manifest().has_capability(Capability::Screenshot));
    }

    #[test]
    fn the_version_is_major_minor_patch() {
        for version in ["0.0.0", "1.0.0", "10.20.30"] {
            assert!(valid_version(version), "{version} must be accepted");
        }
        for version in [
            "1", "1.0", "1.0.0.0", "1.0.x", "v1.0.0", "", " 1.0.0", "1.0.0 ",
        ] {
            assert!(!valid_version(version), "{version} must be refused");
        }
    }

    #[test]
    fn every_structural_problem_is_named() {
        let cases = [
            (
                ToolManifest {
                    name: "   ",
                    ..manifest()
                },
                ManifestError::EmptyName,
            ),
            (
                ToolManifest {
                    version: "1.0",
                    ..manifest()
                },
                ManifestError::InvalidVersion("1.0"),
            ),
            (
                ToolManifest {
                    description: "  ",
                    ..manifest()
                },
                ManifestError::EmptyDescription,
            ),
            (
                ToolManifest {
                    risk: &[],
                    ..manifest()
                },
                ManifestError::NoRiskClass,
            ),
        ];

        for (manifest, expected) in cases {
            assert_eq!(manifest.conformance(), Err(expected));
        }
    }

    const SCROLL_TABLE: &[ToolAction] = &[ToolAction {
        name: "scroll",
        required: &["amount"],
        optional: &["axis"],
    }];

    fn dispatching_manifest() -> ToolManifest {
        ToolManifest {
            actions: Some(SCROLL_TABLE),
            ..manifest()
        }
    }

    /// W2: a missing argument is answered once, by the gate, naming the action
    /// and what that action accepts.
    #[test]
    fn a_missing_required_argument_names_the_action_and_what_it_accepts() {
        let error = dispatching_manifest()
            .argument_error(&json!({"action": "scroll"}))
            .expect("a scroll without `amount` must be refused");

        assert!(error.contains("scroll"), "{error}");
        assert!(error.contains("amount"), "{error}");
        assert!(error.contains("axis"), "{error}");
    }

    /// W2: a call naming an action outside the table is refused by the gate,
    /// not carried to the tool and rejected there with different wording.
    #[test]
    fn an_unknown_action_names_the_actions_that_exist() {
        let error = dispatching_manifest()
            .argument_error(&json!({"action": "scroll_to"}))
            .expect("an action outside the table must be refused");

        assert!(error.contains("scroll_to"), "{error}");
        assert!(error.contains("scroll"), "{error}");
    }

    /// #7: a key the action does not accept is refused, so a misspelled
    /// argument cannot be dropped silently and leave the action on a default.
    #[test]
    fn an_argument_the_action_does_not_accept_is_refused() {
        let error = dispatching_manifest()
            .argument_error(&json!({"action": "scroll", "amount": 3, "amout": 4}))
            .expect("a misspelled argument must be refused");

        assert!(error.contains("amout"), "{error}");
        assert!(error.contains("amount"), "{error}");
    }

    /// W2: the table and the schema's action enum are the same set, so the
    /// gate can never describe an action the tool refuses, or miss one it
    /// accepts.
    #[test]
    fn a_table_must_have_a_row_for_every_declared_action() {
        let manifest = ToolManifest {
            schema: Schema::Fn(two_action_schema),
            actions: Some(SCROLL_TABLE),
            ..manifest()
        };

        assert_eq!(
            manifest.conformance(),
            Err(ManifestError::ActionWithoutRow("drag".to_string()))
        );
    }

    /// W2: the table cannot name an argument the schema hides from the model,
    /// because the model could never send it.
    #[test]
    fn a_table_argument_must_be_declared_in_the_schema() {
        const TABLE: &[ToolAction] = &[
            ToolAction {
                name: "scroll",
                required: &["amount"],
                optional: &["axis"],
            },
            ToolAction {
                name: "drag",
                required: &["from"],
                optional: &["to"],
            },
        ];

        let manifest = ToolManifest {
            schema: Schema::Fn(two_action_schema),
            actions: Some(TABLE),
            ..manifest()
        };

        assert_eq!(
            manifest.conformance(),
            Err(ManifestError::ArgumentNotInSchema {
                action: "scroll".to_string(),
                argument: "axis".to_string(),
            })
        );
    }

    /// W2: a core tool that dispatches on `action` cannot leave the gate blind.
    /// An external tool is exempt: its arguments are validated against the
    /// schema it declared in the handshake (T2).
    #[test]
    fn a_core_tool_with_actions_must_declare_a_table() {
        let manifest = ToolManifest {
            schema: Schema::Fn(two_action_schema),
            actions: None,
            ..manifest()
        };

        assert_eq!(
            manifest.conformance(),
            Err(ManifestError::MissingActionTable)
        );
    }

    /// #7: an open object lets a provider invent keys the tool then ignores.
    #[test]
    fn a_core_schema_must_close_its_object() {
        let core = ToolManifest {
            schema: Schema::Fn(open_schema),
            ..manifest()
        };
        assert_eq!(core.conformance(), Err(ManifestError::SchemaNotClosed));

        // An external tool's schema arrives in the handshake and may be open;
        // the host validates its arguments itself (T2).
        let external = ToolManifest {
            schema: Schema::Fn(open_schema),
            source: Source::External,
            name: "ext.fixture.tool",
            ..manifest()
        };
        assert_eq!(external.conformance(), Ok(()));
    }

    /// #7: the closure is enforced for a single-shape tool too, not only for a
    /// tool whose actions carry a table.
    #[test]
    fn a_single_shape_tool_refuses_a_key_its_schema_does_not_declare() {
        let error = manifest()
            .argument_error(&json!({"text": "hello", "tex": "typo"}))
            .expect("a key the schema does not declare must be refused");

        assert!(error.contains("tex"), "{error}");
        assert!(error.contains("text"), "{error}");
    }

    fn orphan_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["scroll"] },
                "amount": { "type": "integer" },
                "axis": { "type": "string" },
                "unused": { "type": "string" }
            },
            "required": ["action"],
            "additionalProperties": false
        })
    }

    /// #7: a parameter no action reads is advertised to the model for nothing.
    #[test]
    fn a_property_no_action_accepts_is_refused() {
        let manifest = ToolManifest {
            schema: Schema::Fn(orphan_schema),
            actions: Some(SCROLL_TABLE),
            ..manifest()
        };

        assert_eq!(
            manifest.conformance(),
            Err(ManifestError::OrphanProperty("unused".to_string()))
        );
    }

    /// #6: the help answer is generated from the table the gate uses.
    #[test]
    fn the_help_text_lists_each_action_with_its_arguments() {
        let help = dispatching_manifest()
            .help_text()
            .expect("a table-bearing tool has help");

        assert!(help.contains("scroll"), "{help}");
        assert!(help.contains("amount"), "{help}");
        assert!(help.contains("axis"), "{help}");
    }

    #[test]
    fn a_long_description_is_refused() {
        let long = "x".repeat(MAX_DESCRIPTION_CHARS + 1);
        let manifest = ToolManifest {
            description: Box::leak(long.into_boxed_str()),
            ..manifest()
        };

        assert_eq!(
            manifest.conformance(),
            Err(ManifestError::LongDescription(MAX_DESCRIPTION_CHARS + 1))
        );
    }

    /// The summary is the line the prompt carries for every tool, so a blank or
    /// oversized one is refused instead of reaching the model.
    #[test]
    fn the_summary_must_be_one_short_line() {
        assert_eq!(
            ToolManifest {
                summary: "   ",
                ..manifest()
            }
            .conformance(),
            Err(ManifestError::EmptySummary)
        );

        let long = "x".repeat(MAX_SUMMARY_CHARS + 1);
        assert_eq!(
            ToolManifest {
                summary: Box::leak(long.into_boxed_str()),
                ..manifest()
            }
            .conformance(),
            Err(ManifestError::LongSummary(MAX_SUMMARY_CHARS + 1))
        );
    }

    #[test]
    fn the_schema_must_be_an_object_with_declared_required_fields() {
        fn not_an_array() -> Value {
            json!({ "type": "object", "required": "action", "additionalProperties": false })
        }
        fn not_declared() -> Value {
            json!({ "type": "object", "properties": {}, "required": ["action"], "additionalProperties": false })
        }
        fn not_a_string() -> Value {
            json!({ "type": "object", "properties": { "action": {} }, "required": [7], "additionalProperties": false })
        }
        fn not_a_type() -> Value {
            json!({ "properties": {} })
        }
        fn properties_not_an_object() -> Value {
            json!({ "type": "object", "properties": [], "additionalProperties": false })
        }
        fn scalar() -> Value {
            json!(["not", "an", "object"])
        }

        let cases = [
            (scalar as fn() -> Value, ManifestError::SchemaNotObject),
            (not_a_type, ManifestError::SchemaTypeNotObject),
            (
                properties_not_an_object,
                ManifestError::SchemaPropertiesNotObject,
            ),
            (not_an_array, ManifestError::RequiredNotAString),
            (not_a_string, ManifestError::RequiredNotAString),
            (
                not_declared,
                ManifestError::RequiredNotDeclared("action".to_string()),
            ),
        ];

        for (schema, expected) in cases {
            let manifest = ToolManifest {
                schema: Schema::Fn(schema),
                actions: None,
                ..manifest()
            };
            assert_eq!(manifest.conformance(), Err(expected));
        }
    }
}
