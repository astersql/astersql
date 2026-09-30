// Copyright 2026 AsterSQL.

//! Adapt relational SQL row mutations to the executable table MLog contract.

use super::*;

pub(super) struct RuntimeMLog<'a> {
    session: &'a ConcreteSession,
    log: astersql_meta_model::TableInfo,
    tracked_offsets: Vec<usize>,
    flags: astersql_types::Flags,
}

impl<'a> RuntimeMLog<'a> {
    pub(super) fn for_table(
        session: &'a ConcreteSession,
        _database: &str,
        base: &astersql_meta_model::TableInfo,
        flags: astersql_types::Flags,
    ) -> SessionResult<Option<Self>> {
        let Some(log_id) = base.MaterializedViewBase.as_ref().map(|info| info.MLogID) else {
            return Ok(None);
        };
        if log_id == 0 {
            return Ok(None);
        }
        let log = session
            .domain
            .info_schema()
            .TableByID(log_id)
            .ok_or_else(|| {
                SessionError::new(format!("materialized view log ID {log_id} is missing"))
            })?
            .ModelMeta()
            .map_err(|error| SessionError::new(error.to_string()))?;
        let tracked_offsets = astersql_table::mview_log::validate_meta(base, &log)
            .map_err(|error| SessionError::new(error.to_string()))?;
        Ok(Some(Self {
            session,
            log: log.as_ref().clone(),
            tracked_offsets,
            flags,
        }))
    }

    pub(super) fn tracked_changed(
        &self,
        base: &astersql_meta_model::TableInfo,
        old: &HashMap<String, Option<String>>,
        new: &HashMap<String, Option<String>>,
    ) -> bool {
        self.tracked_offsets.iter().any(|offset| {
            let name = &base.Columns[*offset].Name.L;
            old.get(name) != new.get(name)
        })
    }

    pub(super) fn append(
        &self,
        base: &astersql_meta_model::TableInfo,
        row: &HashMap<String, Option<String>>,
        dml: astersql_table::mview_log::MLogDMLType,
        old_new: i64,
        mutations: &mut Vec<(kv::Key, Option<Vec<u8>>)>,
    ) -> SessionResult<()> {
        let mut log_row = HashMap::with_capacity(self.log.Columns.len() + 1);
        let log_info = self
            .log
            .MaterializedViewLog
            .as_ref()
            .expect("validated materialized view log metadata");
        for (position, offset) in self.tracked_offsets.iter().enumerate() {
            log_row.insert(
                log_info.Columns[position].L.clone(),
                row.get(&base.Columns[*offset].Name.L).cloned().flatten(),
            );
        }
        log_row.insert(
            astersql_meta_model::MaterializedViewLogDMLTypeColumnName.to_ascii_lowercase(),
            Some(dml.as_str().to_owned()),
        );
        log_row.insert(
            astersql_meta_model::MaterializedViewLogOldNewColumnName.to_ascii_lowercase(),
            Some(old_new.to_string()),
        );
        let (row_id, _) = self
            .session
            .allocate_runtime_auto_id(self.log.ID, None, 2, 1, 1)?;
        log_row.insert("_tidb_rowid".to_owned(), Some(row_id.to_string()));
        let (key, value) = self
            .session
            .encode_relational_row_for_write(&self.log, &log_row, self.flags)?;
        mutations.extend(relational_index_mutations(
            &self.log,
            None,
            Some(&log_row),
            self.flags,
        )?);
        mutations.push((key, Some(value)));
        Ok(())
    }
}
