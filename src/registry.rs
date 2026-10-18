//! Internal bundled-rule contracts, registration, and typed option validation.

use crate::model::{ByteSpan, Level};
use crate::source::SourceContext;
use std::collections::{BTreeMap, BTreeSet};

/// A supported type for a rule-defined configuration option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionKind {
    /// A true or false setting.
    Boolean,
    /// A signed TOML integer.
    Integer,
    /// A TOML string.
    String,
    /// An array whose every member is a string.
    StringList,
}

impl OptionKind {
    /// Check a configuration value without coercing its type.
    pub fn accepts(self, value: &toml::Value) -> bool {
        match self {
            Self::Boolean => value.is_bool(),
            Self::Integer => value.is_integer(),
            Self::String => value.is_str(),
            Self::StringList => value
                .as_array()
                .is_some_and(|items| items.iter().all(toml::Value::is_str)),
        }
    }

    /// Describe the accepted TOML type in rule help.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::String => "string",
            Self::StringList => "array of strings",
        }
    }
}

/// A named and documented option accepted by one rule.
#[derive(Clone, Debug)]
pub struct OptionSpec {
    /// Configuration key below the rule's options table.
    pub name: &'static str,
    /// Required type when the setting is supplied.
    pub kind: OptionKind,
    /// User-facing explanation of the option.
    pub description: &'static str,
}

/// Metadata shared by configuration, help, and the analysis dispatcher.
#[derive(Clone, Debug)]
pub struct RuleDefinition {
    /// Unique stable rule ID.
    pub id: &'static str,
    /// Short rule name.
    pub name: &'static str,
    /// Explanation of the rule's purpose.
    pub description: &'static str,
    /// Enforcement used when the project omits the rule from configuration.
    pub default_level: Level,
    /// Typed options recognized by this rule.
    pub options: Vec<OptionSpec>,
}

/// A policy-free finding with an exact local suppression target.
#[derive(Clone, Debug)]
pub struct Finding {
    /// Actionable diagnostic text.
    pub message: String,
    /// Original source range highlighted by the diagnostic.
    pub span: ByteSpan,
    /// Identity of the construct to which this rule finding belongs.
    pub target: ByteSpan,
    /// Exact comment spans that may annotate this target for this rule.
    pub annotations: Vec<ByteSpan>,
}

/// The internal interface implemented by bundled structural lint rules.
pub trait Rule {
    /// Describe this rule and its configuration contract.
    fn definition(&self) -> RuleDefinition;

    /// Inspect source and return findings without performing IO or enforcement.
    ///
    /// # Errors
    /// Returns an explanation when a rule cannot finish its analysis.
    fn check(&self, context: &SourceContext, options: &toml::Table)
    -> Result<Vec<Finding>, String>;
}

/// Deterministically ordered rule collection shared by all entry points.
pub struct Registry {
    /// Rules indexed by their validated stable IDs.
    rules: BTreeMap<&'static str, Box<dyn Rule>>,
}

impl Registry {
    /// Validate and return the production rule catalog.
    ///
    /// # Errors
    /// Returns an error if bundled rule IDs or option schemas conflict.
    pub fn bundled() -> Result<Self, String> {
        Self::new(vec![Box::new(crate::flow::FlowCommentRule)])
    }

    /// Validate an internal rule collection before registering it.
    ///
    /// # Errors
    /// Returns an error for invalid or duplicate IDs and duplicate option names.
    pub fn new(rules: Vec<Box<dyn Rule>>) -> Result<Self, String> {
        let mut registered = BTreeMap::new();
        for rule in rules {
            // Validate the complete catalog before dispatch so conflicting rule
            // identities cannot produce configuration-dependent results.
            let definition = rule.definition();
            if !valid_id(definition.id) || definition.id.starts_with("EF") {
                return Err("rule registry contains an invalid or reserved ID".into());
            }
            if registered.contains_key(definition.id) {
                return Err(format!("duplicate rule ID {}", definition.id));
            }
            let mut names = BTreeSet::new();
            for option in &definition.options {
                if option.name.is_empty() || !names.insert(option.name) {
                    return Err(format!("invalid option schema for {}", definition.id));
                }
            }
            registered.insert(definition.id, rule);
        }
        Ok(Self { rules: registered })
    }

    /// Return rule definitions in stable ID order.
    pub fn definitions(&self) -> Vec<RuleDefinition> {
        self.rules.values().map(|rule| rule.definition()).collect()
    }

    /// Find a registered rule without enabling it.
    pub fn get(&self, id: &str) -> Option<&dyn Rule> {
        self.rules.get(id).map(|rule| rule.as_ref())
    }

    /// Iterate registered rule implementations in stable ID order.
    pub fn rules(&self) -> impl Iterator<Item = &dyn Rule> {
        self.rules.values().map(|rule| rule.as_ref())
    }

    /// Check every rule selection and supplied option, including disabled rules.
    ///
    /// # Errors
    /// Returns an error for unknown rules, options, or mismatched option types.
    pub fn validate_settings(
        &self,
        levels: &BTreeMap<String, Level>,
        options: &BTreeMap<String, toml::Table>,
    ) -> Result<(), String> {
        for id in levels.keys().chain(options.keys()) {
            if self.get(id).is_none() {
                return Err(format!("unknown rule ID {id}"));
            }
        }
        for (id, table) in options {
            // Validate rule-local schemas centrally, keeping configuration parsing
            // unchanged when a bundled rule adds an option.
            let Some(rule) = self.get(id) else {
                continue;
            };
            let definition = rule.definition();
            for (name, value) in table {
                let Some(specification) = definition.options.iter().find(|item| item.name == name)
                else {
                    return Err(format!("unknown option {name} for rule {id}"));
                };
                if !specification.kind.accepts(value) {
                    return Err(format!("invalid type for option {name} of rule {id}"));
                }
            }
        }
        Ok(())
    }
}

/// Recognize uppercase rule families followed by at least three digits.
fn valid_id(id: &str) -> bool {
    let prefix_length = id.bytes().take_while(u8::is_ascii_uppercase).count();
    prefix_length > 0
        && id.len().saturating_sub(prefix_length) >= 3
        && id[prefix_length..]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
}
