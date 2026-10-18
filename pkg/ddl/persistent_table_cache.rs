// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Go table.go onAlterCacheTable/onAlterNoCacheTable, in the owner transaction.
use astersql_meta_model::group_3::{Job, JobState};
use astersql_meta_model::{
    SchemaState, TableCacheStatusDisable, TableCacheStatusEnable, TableCacheStatusSwitching,
    TempTableNone,
};

pub(crate) fn step(txn: &mut dyn astersql_kv::Transaction, job: &mut Job) -> Result<i64, String> {
    let mut meta = astersql_meta::TransactionMutator::new(txn);
    let mut table = crate::persistent_actions::public_table(&meta, job)?;
    let enabling = job.tp == 57;
    let target = if enabling {
        TableCacheStatusEnable
    } else {
        TableCacheStatusDisable
    };
    if table.TableCacheStatusType == target {
        job.finish_table_job(
            JobState::Done,
            SchemaState::Public,
            0,
            std::sync::Arc::new(table),
        );
        return Ok(0);
    }
    if enabling && (table.TempTableType != TempTableNone || table.Partition.is_some()) {
        job.state = JobState::Cancelled;
        return Err("[ddl:8200]cached tables do not support temporary tables or partitions".into());
    }
    let expected_start = if enabling {
        TableCacheStatusDisable
    } else {
        TableCacheStatusEnable
    };
    if table.TableCacheStatusType != expected_start
        && table.TableCacheStatusType != TableCacheStatusSwitching
    {
        job.state = JobState::Cancelled;
        return Err(format!(
            "invalid cached-table DDL state {}",
            table.TableCacheStatusType.0
        ));
    }
    let finishing = table.TableCacheStatusType == TableCacheStatusSwitching;
    table.TableCacheStatusType = if finishing {
        target
    } else {
        TableCacheStatusSwitching
    };
    let version =
        crate::persistent_actions::update_version_and_table_with_check(&mut meta, job, &mut table)?;
    if finishing {
        job.finish_table_job(
            JobState::Done,
            SchemaState::Public,
            version,
            std::sync::Arc::new(table),
        );
    }
    Ok(version)
}
