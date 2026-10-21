// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};

use sha2::{Digest, Sha256};

use super::parser::{parse_rules, validate_identifier};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameRouteError(String);

impl NameRouteError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    pub(crate) fn context(self, context: &str) -> Self {
        Self(format!("{context}: {}", self.0))
    }
}

impl Display for NameRouteError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for NameRouteError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectName {
    pub schema: String,
    pub table: String,
}

impl ObjectName {
    pub fn schema(schema: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            table: String::new(),
        }
    }

    pub fn table(schema: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            table: table.into(),
        }
    }

    pub fn is_table(&self) -> bool {
        !self.table.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub source: ObjectName,
    pub target: ObjectName,
}

type ObjectKey = (String, String);

#[derive(Clone, Debug)]
pub struct Router {
    rules: Vec<Rule>,
    schema_rules: HashMap<String, ObjectName>,
    table_rules: HashMap<ObjectKey, ObjectName>,
    canonical_rules: Vec<String>,
    fingerprint: [u8; 32],
}

impl Router {
    pub fn parse(specs: &[String]) -> Result<Self, NameRouteError> {
        Self::new(parse_rules(specs)?)
    }

    pub fn new(rules: Vec<Rule>) -> Result<Self, NameRouteError> {
        let mut normalized = Vec::with_capacity(rules.len());
        let mut sources = HashMap::with_capacity(rules.len());
        let mut targets = HashMap::with_capacity(rules.len());

        for (index, rule) in rules.into_iter().enumerate() {
            let rule = normalize_rule(rule).map_err(|error| {
                NameRouteError::new(format!("invalid rename rule {}: {error}", index + 1))
            })?;
            let source_key = key_of(&rule.source);
            if let Some(previous) = sources.insert(source_key.clone(), index) {
                return Err(NameRouteError::new(format!(
                    "rename rules {} and {} have duplicate source {}",
                    previous + 1,
                    index + 1,
                    format_object(&rule.source)
                )));
            }
            let target_key = key_of(&rule.target);
            if let Some(previous) = targets.insert(target_key.clone(), index) {
                return Err(NameRouteError::new(format!(
                    "rename rules {} and {} have duplicate target {}",
                    previous + 1,
                    index + 1,
                    format_object(&rule.target)
                )));
            }
            if source_key == target_key {
                return Err(NameRouteError::new(format!(
                    "rename rule {} maps {} to itself",
                    index + 1,
                    format_object(&rule.source)
                )));
            }
            normalized.push(rule);
        }

        let mut schema_rules = HashMap::new();
        let mut table_rules = HashMap::new();
        for rule in &normalized {
            if rule.source.is_table() {
                table_rules.insert(key_of(&rule.source), rule.target.clone());
            } else {
                schema_rules.insert(rule.source.schema.to_lowercase(), rule.target.clone());
            }
        }
        let mut canonical_rules: Vec<_> = normalized
            .iter()
            .map(|rule| {
                format!(
                    "{}:{}",
                    format_object(&rule.source),
                    format_object(&rule.target)
                )
            })
            .collect();
        canonical_rules.sort();
        let fingerprint = fingerprint(&canonical_rules);
        Ok(Self {
            rules: normalized,
            schema_rules,
            table_rules,
            canonical_rules,
            fingerprint,
        })
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn route(&self, schema: &str, table: &str) -> (String, String, bool) {
        if !table.is_empty() {
            let key = (schema.to_lowercase(), table.to_lowercase());
            if let Some(target) = self.table_rules.get(&key) {
                return (target.schema.clone(), target.table.clone(), true);
            }
        }
        if let Some(target) = self.schema_rules.get(&schema.to_lowercase()) {
            return (target.schema.clone(), table.to_string(), true);
        }
        (schema.to_string(), table.to_string(), false)
    }

    pub fn validate_targets(&self, objects: &[ObjectName]) -> Result<(), NameRouteError> {
        let mut targets: HashMap<ObjectKey, ObjectName> = HashMap::new();
        let mut seen_sources = HashSet::new();
        for object in objects {
            let normalized = normalize_object(object.clone())
                .map_err(|error| error.context("invalid restore object"))?;
            let source_key = key_of(&normalized);
            if !seen_sources.insert(source_key) {
                continue;
            }
            let (schema, table, _) = self.route(&normalized.schema, &normalized.table);
            let target = if table.is_empty() {
                ObjectName::schema(schema)
            } else {
                ObjectName::table(schema, table)
            };
            if let Some(previous) = targets.insert(key_of(&target), normalized.clone()) {
                return Err(NameRouteError::new(format!(
                    "source objects {} and {} conflict at target {}",
                    format_object(&previous),
                    format_object(&normalized),
                    format_object(&target)
                )));
            }
        }
        Ok(())
    }

    pub fn canonical_rules(&self) -> &[String] {
        &self.canonical_rules
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
}

fn normalize_rule(rule: Rule) -> Result<Rule, NameRouteError> {
    let source = normalize_object(rule.source).map_err(|error| error.context("invalid source"))?;
    let target = normalize_object(rule.target).map_err(|error| error.context("invalid target"))?;
    if source.is_table() != target.is_table() {
        return Err(NameRouteError::new(
            "source and target must both name a schema or both name a table",
        ));
    }
    Ok(Rule { source, target })
}

fn normalize_object(object: ObjectName) -> Result<ObjectName, NameRouteError> {
    validate_identifier(&object.schema, 64)
        .map_err(|error| error.context("invalid schema name"))?;
    if object.table.is_empty() {
        return Ok(ObjectName::schema(object.schema));
    }
    validate_identifier(&object.table, 64).map_err(|error| error.context("invalid table name"))?;
    Ok(object)
}

fn key_of(object: &ObjectName) -> ObjectKey {
    (object.schema.to_lowercase(), object.table.to_lowercase())
}

fn format_object(object: &ObjectName) -> String {
    let mut formatted = quote_identifier(&object.schema);
    if object.is_table() {
        formatted.push('.');
        formatted.push_str(&quote_identifier(&object.table));
    }
    formatted
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

fn fingerprint(canonical_rules: &[String]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for rule in canonical_rules {
        hasher.update((rule.len() as u64).to_be_bytes());
        hasher.update(rule.as_bytes());
    }
    hasher.finalize().into()
}
