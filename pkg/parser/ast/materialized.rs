// Copyright 2026 AsterSQL.

//! SQL restoration for materialized-view AST nodes.

use crate::sql_restore::{restore_expr as expression, restore_node as restore_select};
use crate::{
    AlterMaterializedViewAction, AlterMaterializedViewActionType, AlterMaterializedViewLogAction,
    AlterMaterializedViewLogActionType, AlterMaterializedViewLogStmt, AlterMaterializedViewStmt,
    CancelMaterializedViewJobStmt, CancelMaterializedViewJobType, CreateMaterializedViewLogStmt,
    CreateMaterializedViewStmt, DropMaterializedViewLogStmt, DropMaterializedViewStmt,
    MLogAccumulationAlertClause, MLogPurgeClause, MViewRefreshClause, PurgeMaterializedViewLogStmt,
    RefreshMaterializedViewCompleteType, RefreshMaterializedViewImplementStmt,
    RefreshMaterializedViewMode, RefreshMaterializedViewObserveType, RefreshMaterializedViewStmt,
    RefreshMaterializedViewType, TableName, TableOption, TableOptionType,
};

impl std::fmt::Display for RefreshMaterializedViewType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Fast => "FAST",
            Self::Complete => "COMPLETE",
            Self::Unknown(_) => "UNKNOWN",
        })
    }
}

impl std::fmt::Display for RefreshMaterializedViewCompleteType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InPlace => "IN PLACE",
            Self::OutOfPlace => "OUT OF PLACE",
            Self::DeltaApply => "DELTA APPLY",
            Self::Unknown(_) => "UNKNOWN",
        })
    }
}

impl std::fmt::Display for RefreshMaterializedViewMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Fast => "FAST",
            Self::CompleteInPlace => "COMPLETE IN PLACE",
            Self::CompleteOutOfPlace => "COMPLETE OUT OF PLACE",
            Self::CompleteDeltaApply => "COMPLETE DELTA APPLY",
        })
    }
}

impl PurgeMaterializedViewLogStmt {
    pub fn restore(&self) -> Result<String, String> {
        Ok(format!(
            "PURGE MATERIALIZED VIEW LOG ON {}",
            required_name(&self.Table, "PurgeMaterializedViewLogStmt.Table")?
        ))
    }
}

impl CancelMaterializedViewJobStmt {
    pub fn restore(&self) -> Result<String, String> {
        let prefix = match self.Tp {
            CancelMaterializedViewJobType::Refresh => "CANCEL MATERIALIZED VIEW REFRESH JOB ",
            CancelMaterializedViewJobType::LogPurge => "CANCEL MATERIALIZED VIEW LOG PURGE JOB ",
            CancelMaterializedViewJobType::Unknown(value) => {
                return Err(format!(
                    "invalid materialized view job cancel type: {value}"
                ));
            }
        };
        Ok(format!("{prefix}{}", self.JobID))
    }
}

impl RefreshMaterializedViewStmt {
    pub fn mode(&self) -> Result<RefreshMaterializedViewMode, String> {
        match self.Type {
            RefreshMaterializedViewType::Fast => Ok(RefreshMaterializedViewMode::Fast),
            RefreshMaterializedViewType::Complete => match self.CompleteType {
                RefreshMaterializedViewCompleteType::InPlace => Ok(RefreshMaterializedViewMode::CompleteInPlace),
                RefreshMaterializedViewCompleteType::OutOfPlace => Ok(RefreshMaterializedViewMode::CompleteOutOfPlace),
                RefreshMaterializedViewCompleteType::DeltaApply => Ok(RefreshMaterializedViewMode::CompleteDeltaApply),
                RefreshMaterializedViewCompleteType::Unknown(_) => Err("RefreshMaterializedViewStmt: COMPLETE refresh mode must be specified explicitly".into()),
            },
            RefreshMaterializedViewType::Unknown(_) => Err("RefreshMaterializedViewStmt: unknown REFRESH MATERIALIZED VIEW type".into()),
        }
    }

    pub fn restore(&self) -> Result<String, String> {
        let mut sql = format!(
            "REFRESH MATERIALIZED VIEW {}",
            required_name(&self.ViewName, "RefreshMaterializedViewStmt.ViewName")?
        );
        if self.WithAsyncMode {
            sql.push_str(" WITH ASYNC MODE");
        }
        match self.Type {
            RefreshMaterializedViewType::Fast => sql.push_str(" FAST"),
            RefreshMaterializedViewType::Complete => {
                sql.push_str(" COMPLETE");
                sql.push_str(match self.CompleteType {
                    RefreshMaterializedViewCompleteType::InPlace => " IN PLACE",
                    RefreshMaterializedViewCompleteType::OutOfPlace => " OUT OF PLACE",
                    RefreshMaterializedViewCompleteType::DeltaApply => " DELTA APPLY",
                    RefreshMaterializedViewCompleteType::Unknown(_) => return Err("RefreshMaterializedViewStmt: COMPLETE refresh mode must be specified explicitly".into()),
                });
            }
            RefreshMaterializedViewType::Unknown(_) => sql.push_str(" UNKNOWN"),
        }
        if let Some(as_of) = &self.AsOf {
            sql.push_str(" AS OF TIMESTAMP ");
            sql.push_str(
                &expression(&as_of.TsExpr)
                    .map_err(|e| format!("RefreshMaterializedViewStmt.AsOf: {e}"))?,
            );
        }
        match self.ObserveType {
            RefreshMaterializedViewObserveType::DryRun => sql.push_str(" DRY RUN"),
            RefreshMaterializedViewObserveType::Profile => sql.push_str(" WITH PROFILE"),
            RefreshMaterializedViewObserveType::None => {}
        }
        Ok(sql)
    }
}

impl RefreshMaterializedViewImplementStmt {
    pub fn restore(&self) -> Result<String, String> {
        let refresh = self
            .RefreshStmt
            .as_ref()
            .ok_or("RefreshMaterializedViewImplementStmt: missing RefreshStmt")?;
        let mut sql = format!("IMPLEMENT FOR {} USING TIMESTAMP {}", refresh.restore().map_err(|e| format!("An error occurred while restore RefreshMaterializedViewImplementStmt.RefreshStmt: {e}"))?, self.LastSuccessfulRefreshReadTSO);
        if self.TargetRefreshReadTSO > 0 {
            sql.push_str(&format!(" UP TO TIMESTAMP {}", self.TargetRefreshReadTSO));
        }
        if self.MLogRetainedLowerTSO > 0 {
            sql.push_str(&format!(
                " MLOG RETAINED LOWER TIMESTAMP {}",
                self.MLogRetainedLowerTSO
            ));
        }
        Ok(sql)
    }
}

fn quote_name(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

fn quote_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

fn table_name(name: &TableName) -> String {
    if name.Schema.O.is_empty() {
        quote_name(&name.Name.O)
    } else {
        format!(
            "{}.{}",
            quote_name(&name.Schema.O),
            quote_name(&name.Name.O)
        )
    }
}

fn required_name(name: &Option<TableName>, context: &str) -> Result<String, String> {
    name.as_ref()
        .map(table_name)
        .ok_or_else(|| format!("{context} is missing"))
}

impl TableOption {
    pub fn restore(&self) -> Result<String, String> {
        let sql = match self.Tp {
            TableOptionType::ShardRowID => format!("SHARD_ROW_ID_BITS = {}", self.UintValue),
            TableOptionType::PreSplitRegion => format!("PRE_SPLIT_REGIONS = {}", self.UintValue),
            TableOptionType::EngineAttribute => {
                format!("ENGINE_ATTRIBUTE = {}", quote_string(&self.StrValue))
            }
            TableOptionType::StorageClass => {
                format!("STORAGE_CLASS = {}", quote_string(&self.StrValue))
            }
            TableOptionType::StartTransaction => "START TRANSACTION".to_owned(),
            _ => return Err(format!("table option {:?} has no Rust restore", self.Tp)),
        };
        Ok(sql)
    }
}

impl MViewRefreshClause {
    pub fn restore(&self) -> Result<String, String> {
        let mut sql = self.Method.to_string();
        if let Some(expr) = &self.StartWith {
            sql.push_str(" START WITH ");
            sql.push_str(
                &expression(expr).map_err(|e| format!("MViewRefreshClause.StartWith: {e}"))?,
            );
        }
        if let Some(expr) = &self.Next {
            sql.push_str(" NEXT ");
            sql.push_str(&expression(expr).map_err(|e| format!("MViewRefreshClause.Next: {e}"))?);
        }
        Ok(sql)
    }
}

impl MLogPurgeClause {
    pub fn restore(&self) -> Result<String, String> {
        let mut sql = "PURGE".to_owned();
        if self.Immediate {
            sql.push_str(" IMMEDIATE");
            return Ok(sql);
        }
        if let Some(expr) = &self.StartWith {
            sql.push_str(" START WITH ");
            sql.push_str(&expression(expr).map_err(|e| format!("MLogPurgeClause.StartWith: {e}"))?);
        }
        if let Some(expr) = &self.Next {
            sql.push_str(" NEXT ");
            sql.push_str(&expression(expr).map_err(|e| format!("MLogPurgeClause.Next: {e}"))?);
        }
        Ok(sql)
    }
}

impl MLogAccumulationAlertClause {
    pub fn restore(&self) -> String {
        format!("ALERT ROWS {}", self.Rows)
    }
}

impl CreateMaterializedViewStmt {
    pub fn restore(&self) -> Result<String, String> {
        let mut sql = format!(
            "CREATE MATERIALIZED VIEW {} (",
            required_name(&self.ViewName, "CreateMaterializedViewStmt.ViewName")?
        );
        sql.push_str(
            &self
                .Cols
                .iter()
                .map(|c| quote_name(&c.O))
                .collect::<Vec<_>>()
                .join(", "),
        );
        sql.push(')');
        if !self.Comment.is_empty() {
            sql.push_str(&format!(" COMMENT = {}", quote_string(&self.Comment)));
        }
        for (i, value) in self.Options.iter().enumerate() {
            sql.push(' ');
            sql.push_str(
                &value
                    .restore()
                    .map_err(|e| format!("CreateMaterializedViewStmt.TableOption[{i}]: {e}"))?,
            );
        }
        if let Some(refresh) = &self.Refresh {
            sql.push(' ');
            sql.push_str(
                &refresh
                    .restore()
                    .map_err(|e| format!("CreateMaterializedViewStmt.Refresh: {e}"))?,
            );
        }
        if !self.Attributes.is_empty() {
            sql.push_str(&format!(" ATTRIBUTES = {}", quote_string(&self.Attributes)));
        }
        sql.push_str(" AS ");
        let select = self
            .Select
            .as_ref()
            .ok_or("CreateMaterializedViewStmt.Select is missing")?;
        let text = restore_select(select.as_ref())
            .map_err(|e| format!("CreateMaterializedViewStmt.{e}"))?;
        sql.push_str(&text);
        Ok(sql)
    }
}

impl CreateMaterializedViewLogStmt {
    pub fn restore(&self) -> Result<String, String> {
        let mut sql = format!(
            "CREATE MATERIALIZED VIEW LOG ON {} (",
            required_name(&self.Table, "CreateMaterializedViewLogStmt.Table")?
        );
        sql.push_str(
            &self
                .Cols
                .iter()
                .map(|c| quote_name(&c.O))
                .collect::<Vec<_>>()
                .join(", "),
        );
        sql.push(')');
        for (i, value) in self.Options.iter().enumerate() {
            sql.push(' ');
            sql.push_str(
                &value
                    .restore()
                    .map_err(|e| format!("CreateMaterializedViewLogStmt.Options[{i}]: {e}"))?,
            );
        }
        if let Some(purge) = &self.Purge {
            sql.push(' ');
            sql.push_str(
                &purge
                    .restore()
                    .map_err(|e| format!("CreateMaterializedViewLogStmt.Purge: {e}"))?,
            );
        }
        if let Some(alert) = &self.AccumulationAlert {
            sql.push(' ');
            sql.push_str(&alert.restore());
        }
        Ok(sql)
    }
}

impl AlterMaterializedViewAction {
    pub fn restore(&self) -> Result<String, String> {
        match self.Tp {
            AlterMaterializedViewActionType::Comment => {
                Ok(format!("COMMENT = {}", quote_string(&self.Comment)))
            }
            AlterMaterializedViewActionType::Attributes => {
                Ok(format!("ATTRIBUTES = {}", quote_string(&self.Attributes)))
            }
            AlterMaterializedViewActionType::Refresh => {
                let mut sql = "REFRESH".to_owned();
                if let Some(refresh) = &self.Refresh {
                    if let Some(expr) = &refresh.StartWith {
                        sql.push_str(" START WITH ");
                        sql.push_str(&expression(expr).map_err(|e| {
                            format!("AlterMaterializedViewAction.Refresh.StartWith: {e}")
                        })?);
                    }
                    if let Some(expr) = &refresh.Next {
                        sql.push_str(" NEXT ");
                        sql.push_str(&expression(expr).map_err(|e| {
                            format!("AlterMaterializedViewAction.Refresh.Next: {e}")
                        })?);
                    }
                }
                Ok(sql)
            }
        }
    }
}

impl AlterMaterializedViewStmt {
    pub fn restore(&self) -> Result<String, String> {
        let actions = self
            .Actions
            .iter()
            .enumerate()
            .map(|(i, action)| {
                action
                    .restore()
                    .map_err(|e| format!("AlterMaterializedViewStmt.Actions[{i}]: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(format!(
            "ALTER MATERIALIZED VIEW {} {}",
            required_name(&self.ViewName, "AlterMaterializedViewStmt.ViewName")?,
            actions.join(", ")
        ))
    }
}

impl AlterMaterializedViewLogAction {
    pub fn restore(&self) -> Result<String, String> {
        match self.Tp {
            AlterMaterializedViewLogActionType::Purge => self
                .Purge
                .as_ref()
                .map(MLogPurgeClause::restore)
                .transpose()
                .map(|x| x.unwrap_or_default()),
            AlterMaterializedViewLogActionType::AddColumn => Ok(format!(
                "ADD COLUMN ({})",
                self.Cols
                    .iter()
                    .map(|c| quote_name(&c.O))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

impl AlterMaterializedViewLogStmt {
    pub fn restore(&self) -> Result<String, String> {
        let actions = self
            .Actions
            .iter()
            .enumerate()
            .map(|(i, action)| {
                action
                    .restore()
                    .map_err(|e| format!("AlterMaterializedViewLogStmt.Actions[{i}]: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(format!(
            "ALTER MATERIALIZED VIEW LOG ON {} {}",
            required_name(&self.Table, "AlterMaterializedViewLogStmt.Table")?,
            actions.join(", ")
        ))
    }
}

impl DropMaterializedViewStmt {
    pub fn restore(&self) -> Result<String, String> {
        Ok(format!(
            "DROP MATERIALIZED VIEW {}{}",
            if self.IfExists { "IF EXISTS " } else { "" },
            required_name(&self.ViewName, "DropMaterializedViewStmt.ViewName")?
        ))
    }
}

impl DropMaterializedViewLogStmt {
    pub fn restore(&self) -> Result<String, String> {
        Ok(format!(
            "DROP MATERIALIZED VIEW LOG {}ON {}",
            if self.IfExists { "IF EXISTS " } else { "" },
            required_name(&self.Table, "DropMaterializedViewLogStmt.Table")?
        ))
    }
}
