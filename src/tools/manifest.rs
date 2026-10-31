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
    /// Builds the JSON Schema for the arguments. A function rather than a
    /// value, so a tool keeps its schema beside its code and nothing is
    /// serialised until a spec is built.
    pub schema: fn() -> Value,
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
    SchemaNotObject,
    SchemaTypeNotObject,
    SchemaPropertiesNotObject,
    RequiredNotAString,
    RequiredNotDeclared(String),
    NoRiskClass,
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
        }
    }
}

/// `major.minor.patch`, all numeric. Deliberately narrow: the registry only
/// needs to tell versions apart, and accepting more would let a typo through.
fn valid_version(version: &str) -> bool {
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

        if self.risk.is_empty() {
            return Err(ManifestError::NoRiskClass);
        }

        let schema = (self.schema)();
        let Some(object) = schema.as_object() else {
            return Err(ManifestError::SchemaNotObject);
        };

        if object.get("type").and_then(Value::as_str) != Some("object") {
            return Err(ManifestError::SchemaTypeNotObject);
        }

        let properties = match object.get("properties") {
            Some(properties) => match properties.as_object() {
                Some(properties) => Some(properties),
                None => return Err(ManifestError::SchemaPropertiesNotObject),
            },
            None => None,
        };

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
}

/// A minimal valid manifest for a test double.
#[cfg(test)]
pub fn stub(name: &'static str) -> ToolManifest {
    ToolManifest {
        name,
        version: "0.0.0",
        description: "test tool",
        schema: stub_schema,
        risk: &[RiskClass::Read],
        capabilities: &[],
        source: Source::Core,
    }
}

/// The empty object schema a test double accepts.
#[cfg(test)]
pub fn stub_schema() -> Value {
    serde_json::json!({ "type": "object" })
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
            "required": ["action"]
        })
    }

    fn manifest() -> ToolManifest {
        ToolManifest {
            name: "fixture",
            version: CORE_VERSION,
            description: "A fixture tool.",
            schema,
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

    #[test]
    fn the_schema_must_be_an_object_with_declared_required_fields() {
        fn not_an_array() -> Value {
            json!({ "type": "object", "required": "action" })
        }
        fn not_declared() -> Value {
            json!({ "type": "object", "properties": {}, "required": ["action"] })
        }
        fn not_a_string() -> Value {
            json!({ "type": "object", "properties": { "action": {} }, "required": [7] })
        }
        fn not_a_type() -> Value {
            json!({ "properties": {} })
        }
        fn properties_not_an_object() -> Value {
            json!({ "type": "object", "properties": [] })
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
                schema,
                ..manifest()
            };
            assert_eq!(manifest.conformance(), Err(expected));
        }
    }
}
