// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! RECOVER/FLASHBACK TABLE and FLASHBACK DATABASE execution.

use super::*;

impl ConcreteSession {
    fn runtime_gc_safe_point(&self) -> SessionResult<u64> {
        let rows = self
            .domain
            .restricted_stats_query(
                "SELECT HIGH_PRIORITY variable_value FROM mysql.tidb WHERE variable_name='tikv_gc_safe_point'",
                &[],
            )
            .map_err(|error| session_error("read GC safe point", error))?;
        if rows.len() != 1 || rows[0].len() != 1 {
            return Err(SessionError::new("can not get 'tikv_gc_safe_point'"));
        }
        let value = &rows[0][0];
        let without_zone_name = value
            .rsplit_once(' ')
            .map_or(value.as_str(), |(prefix, _)| prefix);
        let timestamp = chrono::DateTime::parse_from_str(without_zone_name, "%Y%m%d-%H:%M:%S %z")
            .or_else(|_| {
                chrono::DateTime::parse_from_str(without_zone_name, "%Y%m%d-%H:%M:%S%.f %z")
            })
            .map_err(|_| SessionError::new(format!("invalid GC safe point {value:?}")))?;
        let physical = u64::try_from(timestamp.timestamp_millis())
            .map_err(|_| SessionError::new("GC safe point predates the Unix epoch"))?;
        Ok(physical << 18)
    }

    fn runtime_recovery_job(
        &self,
        job_id: Option<i64>,
        database: &str,
        table: Option<&str>,
        kinds: &[&str],
    ) -> SessionResult<RuntimeDdlJob> {
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        RUNTIME_DDL_JOBS
            .lock()
            .expect("runtime DDL jobs lock poisoned")
            .history
            .iter()
            .rev()
            .find(|job| {
                job.domain_id == domain_id
                    && job_id.is_none_or(|id| job.id == id)
                    && kinds.iter().any(|kind| job.kind == *kind)
                    && job.database.eq_ignore_ascii_case(database)
                    && table.is_none_or(|table| {
                        job.old_tables
                            .iter()
                            .any(|(_, info)| info.Name.L.eq_ignore_ascii_case(table))
                    })
            })
            .cloned()
            .ok_or_else(|| {
                SessionError::new(job_id.map_or_else(
                    || "can't find dropped table in DDL history".to_owned(),
                    |id| format!("DDL job {id} is not a recoverable table job"),
                ))
            })
    }

    fn validate_recovery_safe_point(&self, job: &RuntimeDdlJob) -> SessionResult<()> {
        let safe_point = self.runtime_gc_safe_point()?;
        if safe_point > job.real_start_ts {
            return Err(SessionError::new(format!(
                "snapshot is older than GC safe point {safe_point}"
            )));
        }
        Ok(())
    }

    fn restore_runtime_table(
        &self,
        database: &str,
        mut table: astersql_meta_model::TableInfo,
        new_name: Option<&str>,
    ) -> SessionResult<astersql_meta_model::TableInfo> {
        if let Some(new_name) = new_name.filter(|name| !name.is_empty()) {
            table.Name = ast::NewCIStr(new_name);
        }
        // Go disables TTL scheduling on recovered/flashback tables so no
        // expiration job can run before the operator inspects the restored
        // data and explicitly re-enables TTL.
        if let Some(ttl) = table.TTLInfo.as_mut() {
            ttl.Enable = false;
        }
        if self.domain.stats_table(database, &table.Name.L).is_some() {
            return Err(SessionError::new(format!(
                "[schema:1050]Table '{}' already exists",
                table.Name.O
            )));
        }
        self.domain
            .ddl_create_table(database, table, false)
            .map_err(|error| session_error("persist recovered table metadata", error))
    }

    fn execute_runtime_table_recovery(
        &self,
        job: RuntimeDdlJob,
        table_name: Option<&str>,
        new_name: Option<&str>,
    ) -> SessionResult<()> {
        self.validate_recovery_safe_point(&job)?;
        let (database, table) = job
            .old_tables
            .iter()
            .find(|(_, info)| table_name.is_none_or(|name| info.Name.L.eq_ignore_ascii_case(name)))
            .cloned()
            .ok_or_else(|| SessionError::new("recoverable table metadata is missing"))?;
        let recovery_id = begin_runtime_ddl_job(
            &self.domain,
            &database,
            new_name.unwrap_or(&table.Name.L),
            "recover table",
        );
        let guard = RuntimeDdlJobGuard::new(recovery_id);
        astersql_testkit_testfailpoint::inject_value(
            "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
            "recover table",
        );
        // Go retries the first injected commit failure. The compact metadata
        // transaction is only attempted after that synthetic failed attempt.
        if astersql_testkit_testfailpoint::eval_bool(
            "github.com/pingcap/tidb/pkg/ddl/mockRecoverTableCommitErr",
        ) && astersql_testkit_testfailpoint::eval_bool("tikvclient/mockCommitError")
        {
            update_runtime_ddl_detail(recovery_id, "retrying recover table commit");
        }
        let result = self
            .restore_runtime_table(&database, table, new_name)
            .and_then(|restored| {
                attach_runtime_ddl_table_info(recovery_id, restored);
                self.update_self_version_with_retry()
            });
        guard.finish(&result);
        result
    }

    pub(super) fn execute_recover_table(
        &self,
        statement: &ast::RecoverTableStmt,
    ) -> SessionResult<()> {
        let current_database = self.current_database();
        let (database, table_name) = statement.Table.as_ref().map_or_else(
            || (current_database.clone(), None),
            |table| {
                (
                    if table.Schema.L.is_empty() {
                        current_database.clone()
                    } else {
                        table.Schema.L.clone()
                    },
                    Some(table.Name.L.clone()),
                )
            },
        );
        let job = if statement.JobID > 0 {
            // A job ID uniquely determines its database, so locate it first.
            let domain_id = Arc::as_ptr(&self.domain) as usize;
            RUNTIME_DDL_JOBS
                .lock()
                .expect("runtime DDL jobs lock poisoned")
                .history
                .iter()
                .rev()
                .find(|job| {
                    job.domain_id == domain_id
                        && job.id == statement.JobID
                        && matches!(job.kind.as_str(), "drop table" | "truncate table")
                })
                .cloned()
                .ok_or_else(|| {
                    SessionError::new(format!(
                        "DDL job {} is not a recoverable table job",
                        statement.JobID
                    ))
                })?
        } else {
            self.runtime_recovery_job(
                None,
                &database,
                table_name.as_deref(),
                &["drop table", "truncate table"],
            )?
        };
        self.execute_runtime_table_recovery(job, table_name.as_deref(), None)
    }

    pub(super) fn execute_flashback_table(
        &self,
        statement: &ast::FlashBackTableStmt,
    ) -> SessionResult<()> {
        let database = if statement.Table.Schema.L.is_empty() {
            self.current_database()
        } else {
            statement.Table.Schema.L.clone()
        };
        let job = self.runtime_recovery_job(
            None,
            &database,
            Some(&statement.Table.Name.L),
            &["drop table", "truncate table"],
        )?;
        self.execute_runtime_table_recovery(
            job,
            Some(&statement.Table.Name.L),
            (!statement.NewName.is_empty()).then_some(statement.NewName.as_str()),
        )
    }

    pub(super) fn execute_flashback_database(
        &self,
        statement: &ast::FlashBackDatabaseStmt,
    ) -> SessionResult<()> {
        let source = statement.DBName.L.clone();
        let destination = if statement.NewName.is_empty() {
            source.clone()
        } else {
            statement.NewName.to_ascii_lowercase()
        };
        let job = self.runtime_recovery_job(None, &source, None, &["drop schema"])?;
        self.validate_recovery_safe_point(&job)?;
        if self
            .domain
            .ddl_database_names()
            .map_err(|error| session_error("read database metadata", error))?
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&destination))
        {
            return Err(SessionError::new(format!(
                "Can't create database '{destination}'; database exists"
            )));
        }
        let recovery_id =
            begin_runtime_ddl_job(&self.domain, &destination, "", "flashback database");
        let guard = RuntimeDdlJobGuard::new(recovery_id);
        let result = (|| {
            self.domain
                .ddl_create_database(&destination, false)
                .map_err(|error| session_error("restore database metadata", error))?;
            let mut restored = Vec::with_capacity(job.old_tables.len());
            for (_, table) in job.old_tables {
                restored.push(self.restore_runtime_table(&destination, table, None)?);
            }
            if let Some(table) = restored.first().cloned() {
                attach_runtime_ddl_table_info(recovery_id, table);
            }
            let domain_id = Arc::as_ptr(&self.domain) as usize;
            RUNTIME_DATABASES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(domain_id)
                .or_default()
                .insert(destination.clone());
            self.state
                .borrow_mut()
                .databases
                .insert(destination.clone());
            self.update_self_version_with_retry()
        })();
        guard.finish(&result);
        result
    }
}
