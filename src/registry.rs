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

    /// Validate rule-specific option semantics after the shared type checks.
    ///
    /// Receives an empty table when options are omitted, including for disabled rules.
    ///
    /// # Errors
    /// Returns an explanation when supplied options or their defaults are invalid.
    fn validate_options(&self, _options: &toml::Table) -> Result<(), String> {
        Ok(())
    }

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
        Self::new(vec![
            Box::new(crate::flow::FlowCommentRule),
            Box::new(crate::logging::EntryLoggingRule),
        ])
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
    /// Returns an error for unknown rules, options, mismatched option types, or
    /// rule-specific validation failures.
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
        let empty = toml::Table::new();
        for (id, rule) in &self.rules {
            rule.validate_options(options.get(*id).unwrap_or(&empty))
                .map_err(|error| format!("{id}: {error}"))?;
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

#[cfg(test)]
mod tests {
    //! Check rule-specific validation independently of source analysis and enforcement.

    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Observe which registered rules receive supplied or omitted option tables.
    type Validations = Rc<RefCell<Vec<(&'static str, toml::Table)>>>;

    /// A fixture rule whose integer option must have a positive semantic value.
    struct PositiveMinimum {
        /// Stable fixture identity used in validation errors and observations.
        id: &'static str,
        /// Shared record of every semantic validation call.
        observed: Validations,
    }

    impl Rule for PositiveMinimum {
        fn definition(&self) -> RuleDefinition {
            RuleDefinition {
                id: self.id,
                name: "positive-minimum",
                description: "Fixture rule with a positive minimum option",
                default_level: Level::Warning,
                options: vec![OptionSpec {
                    name: "minimum",
                    kind: OptionKind::Integer,
                    description: "A strictly positive minimum, defaulting to one",
                }],
            }
        }

        fn validate_options(&self, options: &toml::Table) -> Result<(), String> {
            self.observed.borrow_mut().push((self.id, options.clone()));
            let minimum = options
                .get("minimum")
                .and_then(toml::Value::as_integer)
                .unwrap_or(1);
            if minimum <= 0 {
                return Err("minimum must be positive".into());
            }
            Ok(())
        }

        fn check(
            &self,
            _context: &SourceContext,
            _options: &toml::Table,
        ) -> Result<Vec<Finding>, String> {
            Ok(Vec::new())
        }
    }

    /// Semantically invalid values remain invalid when enforcement is disabled.
    #[test]
    fn semantic_validation_rejects_invalid_values_even_for_disabled_rules() {
        let observed = Rc::new(RefCell::new(Vec::new()));
        let registry = Registry::new(vec![Box::new(PositiveMinimum {
            id: "ZZ001",
            observed: Rc::clone(&observed),
        })])
        .expect("fixture registry is valid");
        let options = BTreeMap::from([(
            "ZZ001".to_owned(),
            toml::Table::from_iter([("minimum".to_owned(), toml::Value::Integer(0))]),
        )]);
        for level in [Level::Warning, Level::Off] {
            let levels = BTreeMap::from([("ZZ001".to_owned(), level)]);
            assert_eq!(
                registry.validate_settings(&levels, &options),
                Err("ZZ001: minimum must be positive".to_owned())
            );
        }
        assert_eq!(observed.borrow().len(), 2);
    }

    /// Each rule validates its own table, including empty defaults for a disabled rule.
    #[test]
    fn semantic_validation_receives_supplied_and_default_tables() {
        let observed = Rc::new(RefCell::new(Vec::new()));
        let registry = Registry::new(vec![
            Box::new(PositiveMinimum {
                id: "ZZ002",
                observed: Rc::clone(&observed),
            }),
            Box::new(PositiveMinimum {
                id: "ZZ001",
                observed: Rc::clone(&observed),
            }),
        ])
        .expect("fixture registry is valid");
        let supplied = toml::Table::from_iter([("minimum".to_owned(), toml::Value::Integer(5))]);
        let levels = BTreeMap::from([("ZZ002".to_owned(), Level::Off)]);
        let options = BTreeMap::from([("ZZ001".to_owned(), supplied.clone())]);
        registry
            .validate_settings(&levels, &options)
            .expect("supplied and default values are valid");
        assert_eq!(
            *observed.borrow(),
            vec![("ZZ001", supplied), ("ZZ002", toml::Table::new())]
        );
    }

    /// Shared name and type errors are rejected before any rule-specific hook runs.
    #[test]
    fn shared_validation_precedes_semantic_hooks() {
        let observed = Rc::new(RefCell::new(Vec::new()));
        let registry = Registry::new(vec![Box::new(PositiveMinimum {
            id: "ZZ001",
            observed: Rc::clone(&observed),
        })])
        .expect("fixture registry is valid");
        for (table, expected) in [
            (
                toml::Table::from_iter([("unknown".to_owned(), toml::Value::Integer(1))]),
                "unknown option unknown for rule ZZ001",
            ),
            (
                toml::Table::from_iter([("minimum".to_owned(), toml::Value::String("one".into()))]),
                "invalid type for option minimum of rule ZZ001",
            ),
        ] {
            let options = BTreeMap::from([("ZZ001".to_owned(), table)]);
            assert_eq!(
                registry.validate_settings(&BTreeMap::new(), &options),
                Err(expected.to_owned())
            );
        }
        assert!(observed.borrow().is_empty());
    }
}
