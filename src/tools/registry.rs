use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::manifest::{Capability, Source, ToolManifest};
use super::Tool;

#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, RegisteredTool>,
}

#[derive(Clone)]
struct RegisteredTool {
    tool: Arc<dyn Tool>,
    manifest: ToolManifest,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool, validating its manifest first (T1).
    ///
    /// A malformed manifest, a manifest whose name disagrees with the tool, a
    /// second tool with the same name, and a name in the wrong namespace are
    /// all programmer errors: the composition root registers a fixed set at
    /// startup, so a panic here is the explicit failure the issue asks for, not
    /// a runtime condition.
    ///
    /// An external tool (T2) uses the same manifest shape and must live in the
    /// `ext.<server>.<tool>` namespace, which is what gives it a policy row
    /// without a compile-time entry in the table. A core tool may not squat
    /// that namespace, and an external manifest cannot escape it.
    pub fn register<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        let manifest = tool.manifest();

        if let Err(error) = manifest.conformance() {
            panic!("tool {:?} has an invalid manifest: {error}", tool.name());
        }

        if manifest.name != tool.name() {
            panic!(
                "the manifest name {:?} does not match the tool name {:?}",
                manifest.name,
                tool.name()
            );
        }

        match manifest.source {
            Source::External if !super::safety::is_external_tool(manifest.name) => {
                panic!(
                    "external tool {:?} is not named ext.<server>.<tool>",
                    manifest.name
                );
            }
            Source::Core if super::safety::is_external_tool(manifest.name) => {
                panic!(
                    "core tool {:?} uses the external ext. namespace",
                    manifest.name
                );
            }
            _ => {}
        }

        if self.tools.contains_key(manifest.name) {
            panic!("tool {:?} is registered twice", manifest.name);
        }

        self.tools.insert(
            manifest.name.to_string(),
            RegisteredTool {
                tool: Arc::new(tool),
                manifest,
            },
        );
    }

    /// Whether a name is registered, available or not. The external host uses
    /// this to refuse a duplicate name without conflating a deactivated server
    /// with an unregistered one.
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// A currently offered tool. A tool whose server was removed from the
    /// configuration (T3) is registered but not available, so it is not found
    /// and a stale call fails closed.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .get(name)
            .filter(|registered| registered.tool.is_available())
            .map(|registered| registered.tool.clone())
    }

    pub fn iter(&self) -> impl Iterator<Item = Arc<dyn Tool>> + '_ {
        let mut tools: Vec<_> = self
            .tools
            .values()
            .map(|registered| registered.tool.clone())
            .collect();
        tools.sort_by(|left, right| left.name().cmp(right.name()));
        tools.into_iter()
    }

    /// Every manifest, in name order. The version of a registered tool is
    /// therefore visible without building its tool again (T1).
    ///
    /// Test-only now that `specs` filters by availability (T3): production
    /// callers ask for the model-facing specifications, which must omit a
    /// removed server's tools.
    #[cfg(test)]
    pub fn manifests(&self) -> Vec<ToolManifest> {
        let mut manifests: Vec<_> = self
            .tools
            .values()
            .map(|registered| registered.manifest)
            .collect();
        manifests.sort_by_key(|manifest| manifest.name);
        manifests
    }

    /// The model-facing tool specifications, generated from the manifests.
    ///
    /// One source of truth: what the model is told is the declaration the
    /// registry validated, not a second copy in the tool. An unavailable tool
    /// is omitted, so a server removed from the configuration disappears from
    /// the model's list on the next turn (T3).
    pub fn specs(&self) -> Vec<serde_json::Value> {
        let mut manifests: Vec<_> = self
            .tools
            .values()
            .filter(|registered| registered.tool.is_available())
            .map(|registered| registered.manifest)
            .collect();
        manifests.sort_by_key(|manifest| manifest.name);
        manifests
            .iter()
            .map(|manifest| {
                json!({
                    "type": "function",
                    "function": {
                        "name": manifest.name,
                        "description": manifest.description,
                        "parameters": manifest.schema.build(),
                    }
                })
            })
            .collect()
    }

    /// The set-level checks the compiler cannot make: every registered tool has
    /// an approval policy row (A8), and a tool that can reach the keyboard
    /// declares the `input` capability exactly when its row declares input
    /// paths (D6). Returns the problems rather than panicking, so a test can
    /// report all of them at once.
    pub fn conformance(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();

        for registered in self.tools.values() {
            let manifest = registered.manifest;
            let name = manifest.name;

            let Some(policy) = super::safety::policy_for(name) else {
                problems.push(format!(
                    "{name} is registered without an approval policy row"
                ));
                continue;
            };

            let declares_input = manifest.has_capability(Capability::Input);
            let has_input_paths = !policy.input_paths.is_empty();

            if declares_input != has_input_paths {
                problems.push(format!(
                    "{name} declares the {} capability {declares_input} but its policy row \
                     declares input paths {has_input_paths}",
                    Capability::Input.name()
                ));
            }

            if registered.tool.name() != name {
                problems.push(format!(
                    "the registered tool reports the name {:?} but its manifest says {name:?}",
                    registered.tool.name()
                ));
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::manifest;
    use anyhow::Result;
    use async_trait::async_trait;

    struct NamedTool(ToolManifest);

    #[async_trait]
    impl Tool for NamedTool {
        fn manifest(&self) -> ToolManifest {
            self.0
        }

        async fn execute(&self, _input: &str) -> Result<super::super::ToolResult> {
            Ok(super::super::ToolResult::success("ok"))
        }
    }

    fn named(name: &'static str) -> NamedTool {
        NamedTool(manifest::stub(name))
    }

    #[test]
    fn iteration_is_sorted_by_tool_name() {
        let mut registry = ToolRegistry::new();
        registry.register(named("zeta"));
        registry.register(named("alpha"));
        registry.register(named("middle"));

        let names: Vec<_> = registry
            .iter()
            .map(|tool| tool.name().to_string())
            .collect();
        assert_eq!(names, ["alpha", "middle", "zeta"]);
    }

    #[test]
    fn the_manifest_and_its_version_are_visible() {
        let mut registry = ToolRegistry::new();
        registry.register(named("alpha"));

        let manifests = registry.manifests();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "alpha");
        assert_eq!(manifests[0].version, "0.0.0");
    }

    /// The model is told the manifest, so the spec cannot come from a second
    /// copy of the description.
    #[test]
    fn the_specs_are_generated_from_the_manifests() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool(ToolManifest {
            description: "Fixture description from the manifest.",
            ..manifest::stub("fixture")
        }));

        let specs = registry.specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0]["type"], "function");
        assert_eq!(specs[0]["function"]["name"], "fixture");
        assert_eq!(
            specs[0]["function"]["description"],
            "Fixture description from the manifest."
        );
        assert_eq!(specs[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    #[should_panic(expected = "has an invalid manifest")]
    fn a_manifest_without_a_description_is_refused() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool(ToolManifest {
            description: " ",
            ..manifest::stub("fixture")
        }));
    }

    /// A tool whose own `name` disagrees with its manifest is a bug even though
    /// the default `name` reads the manifest: an override can drift from it.
    struct RenamedTool;

    #[async_trait]
    impl Tool for RenamedTool {
        fn manifest(&self) -> ToolManifest {
            manifest::stub("other")
        }

        fn name(&self) -> &str {
            "fixture"
        }

        async fn execute(&self, _input: &str) -> Result<super::super::ToolResult> {
            Ok(super::super::ToolResult::success("ok"))
        }
    }

    #[test]
    #[should_panic(expected = "does not match the tool name")]
    fn a_manifest_with_another_name_is_refused() {
        let mut registry = ToolRegistry::new();
        registry.register(RenamedTool);
    }

    #[test]
    #[should_panic(expected = "is registered twice")]
    fn a_duplicate_name_is_refused() {
        let mut registry = ToolRegistry::new();
        registry.register(named("alpha"));
        registry.register(named("alpha"));
    }

    /// T2: an external tool uses the same manifest shape and registers in its
    /// namespace, where the gate has one always-ask policy row.
    #[test]
    fn an_external_tool_in_its_namespace_registers_and_conforms() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool(ToolManifest {
            name: "ext.fixture.echo",
            source: Source::External,
            ..manifest::stub("ext.fixture.echo")
        }));

        assert_eq!(registry.conformance(), Ok(()));
        let manifests = registry.manifests();
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "ext.fixture.echo");
        assert_eq!(manifests[0].source, Source::External);
        assert!(crate::tools::safety::requires_approval(
            "ext.fixture.echo",
            "{}"
        ));
    }

    #[test]
    #[should_panic(expected = "is not named ext.<server>.<tool>")]
    fn an_external_tool_outside_the_namespace_is_refused() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool(ToolManifest {
            source: Source::External,
            ..manifest::stub("fixture")
        }));
    }

    #[test]
    #[should_panic(expected = "uses the external ext. namespace")]
    fn a_core_tool_cannot_squat_the_external_namespace() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool(ToolManifest {
            name: "ext.fixture.echo",
            ..manifest::stub("ext.fixture.echo")
        }));
    }

    #[test]
    fn conformance_reports_a_missing_policy_row_and_an_input_mismatch() {
        let mut registry = ToolRegistry::new();
        registry.register(named("not_a_tool"));
        let problems = registry.conformance().expect_err("a policy row is missing");
        assert!(
            problems[0].contains("without an approval policy row"),
            "{problems:?}"
        );

        let mut registry = ToolRegistry::new();
        registry.register(NamedTool(ToolManifest {
            name: "computer",
            capabilities: &[],
            ..manifest::stub("computer")
        }));
        let problems = registry
            .conformance()
            .expect_err("the input capability is missing");
        assert!(
            problems[0].contains("declares the input capability false"),
            "{problems:?}"
        );
    }

    /// T3: a tool whose server left the configuration is still registered
    /// (its manifest stays validated) but is not offered or callable.
    struct UnavailableTool(ToolManifest);

    #[async_trait]
    impl Tool for UnavailableTool {
        fn manifest(&self) -> ToolManifest {
            self.0
        }

        fn is_available(&self) -> bool {
            false
        }

        async fn execute(&self, _input: &str) -> Result<super::super::ToolResult> {
            Ok(super::super::ToolResult::success("unreachable"))
        }
    }

    #[test]
    fn an_unavailable_tool_is_not_offered() {
        let mut registry = ToolRegistry::new();
        registry.register(named("alpha"));
        registry.register(UnavailableTool(manifest::stub("beta")));

        assert!(registry.get("alpha").is_some());
        assert!(
            registry.get("beta").is_none(),
            "an unavailable tool must not be found"
        );
        let specs = registry.specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0]["function"]["name"], "alpha");
        // The manifest set still carries it, so conformance keeps covering it.
        assert_eq!(registry.manifests().len(), 2);
    }

    #[test]
    fn a_core_tool_with_a_policy_row_conforms() {
        let mut registry = ToolRegistry::new();
        registry.register(named("memory"));

        assert_eq!(registry.conformance(), Ok(()));
    }
}
