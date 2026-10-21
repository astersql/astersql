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

//! Restore name-routing configuration shared by snapshot restore and PiTR.

use astersql_br_pkg_checkpoint::{
    CustomSSTRestoreCheckpointDatabaseName, LogRestoreCheckpointDatabaseName,
    SnapshotRestoreCheckpointDatabaseName,
};
use astersql_br_pkg_restore::nameroute::{ObjectName, Router};

use crate::common::FullBackupTypeEBS;
use crate::restore::{FlagRename, RawRestoreCmd, RestoreConfig, TxnRestoreCmd, flagNoSchema};
use crate::stubs::{Error, Result, berrors};

impl RestoreConfig {
    /// Builds a router from the current public Rename field. The router is
    /// intentionally derived on every call so programmatic updates cannot use
    /// stale cached rules.
    pub fn get_name_router(&self) -> Result<Router> {
        let router = Router::parse(&self.Rename).map_err(|error| {
            Error::Annotate(error.to_string(), "invalid restore rename configuration")
        })?;
        validate_rename_rule_schemas(&router)?;
        Ok(router)
    }

    pub fn has_name_routing(&self) -> bool {
        !self.Rename.is_empty()
    }

    pub fn is_partial_restore(&self) -> bool {
        self.Config.ExplicitFilter || self.has_name_routing()
    }

    pub fn name_route_fingerprint(&self) -> Result<String> {
        if !self.has_name_routing() {
            return Ok(String::new());
        }
        Ok(hex::encode(self.get_name_router()?.fingerprint()))
    }

    pub fn validate_name_routing(&self, cmd_name: &str) -> Result<()> {
        self.get_name_router()?;
        if !self.has_name_routing() {
            return Ok(());
        }
        if self.NoSchema {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!("--{FlagRename} cannot be used with --{flagNoSchema}"),
            ));
        }
        if self.RestoreCommonConfig.WithSysTable {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!("--{FlagRename} requires --with-sys-table=false"),
            ));
        }
        if cmd_name == RawRestoreCmd || cmd_name == TxnRestoreCmd {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!("--{FlagRename} is not supported by {cmd_name}"),
            ));
        }
        if self.FullBackupType.0 == FullBackupTypeEBS {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!("--{FlagRename} is not supported by EBS restore"),
            ));
        }
        Ok(())
    }
}

fn validate_rename_rule_schemas(router: &Router) -> Result<()> {
    for rule in router.rules() {
        for object in [&rule.source, &rule.target] {
            let schema = object.schema.to_lowercase();
            if is_system_schema(&schema)
                || schema.starts_with("__tidb_br_temporary_")
                || is_checkpoint_schema(&schema)
            {
                return Err(Error::Annotatef(
                    berrors::ErrInvalidArgument,
                    format!(
                        "restore rename does not support system, temporary system, or checkpoint schema {}",
                        object.schema
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn is_system_schema(schema: &str) -> bool {
    matches!(
        schema,
        "mysql"
            | "information_schema"
            | "performance_schema"
            | "metrics_schema"
            | "inspection_schema"
            | "sys"
    )
}

fn is_checkpoint_schema(schema: &str) -> bool {
    [
        LogRestoreCheckpointDatabaseName,
        SnapshotRestoreCheckpointDatabaseName,
        CustomSSTRestoreCheckpointDatabaseName,
    ]
    .iter()
    .any(|prefix| schema.starts_with(&prefix.to_lowercase()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreNamePlanEntry {
    pub source: ObjectName,
    pub target: ObjectName,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreNamePlan {
    pub entries: Vec<RestoreNamePlanEntry>,
}

/// Resolves immutable source names to one stable target per selected object.
/// Table rules take precedence through Router::route; all configured rules must
/// match the restore set so a typo cannot silently produce an identity restore.
pub fn build_restore_name_plan(router: &Router, objects: &[ObjectName]) -> Result<RestoreNamePlan> {
    router
        .validate_targets(objects)
        .map_err(|error| Error::Annotate(error.to_string(), "invalid restore rename targets"))?;

    let mut matched_rules = vec![false; router.rules().len()];
    let mut entries = Vec::with_capacity(objects.len());
    for source in objects {
        for (index, rule) in router.rules().iter().enumerate() {
            if rule.source.schema.eq_ignore_ascii_case(&source.schema)
                && (!rule.source.is_table()
                    || rule.source.table.eq_ignore_ascii_case(&source.table))
            {
                matched_rules[index] = true;
            }
        }
        let (schema, table, _) = router.route(&source.schema, &source.table);
        let target = if table.is_empty() {
            ObjectName::schema(schema)
        } else {
            ObjectName::table(schema, table)
        };
        entries.push(RestoreNamePlanEntry {
            source: source.clone(),
            target,
        });
    }
    if let Some((index, _)) = matched_rules
        .iter()
        .enumerate()
        .find(|(_, matched)| !**matched)
    {
        let rule = &router.rules()[index];
        return Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!(
                "restore rename source {}{} was not found in the selected backup objects",
                rule.source.schema,
                if rule.source.is_table() {
                    format!(".{}", rule.source.table)
                } else {
                    String::new()
                }
            ),
        ));
    }
    Ok(RestoreNamePlan { entries })
}
