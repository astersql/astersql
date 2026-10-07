// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

#![allow(non_snake_case)]

/// Cancel using the DXF manager and the job's keyspace manager. The initial
/// probe follows the later Go race fix; a missed task is never waited for.
pub fn cancelAndWaitImportJobInStorage(
    context: &astersql_dxf_framework_handle::Context,
    job_id: i64,
    task_manager: &astersql_dxf_framework_storage::TaskManager,
    job_manager: &astersql_dxf_framework_storage::TaskManager,
) -> Result<(), astersql_dxf_framework_storage::Error> {
    cancelImportJobWithFallbackHook(context, job_id, task_manager, job_manager, || {})
}

pub(crate) fn cancelImportJobWithFallbackHook(
    context: &astersql_dxf_framework_handle::Context,
    job_id: i64,
    task_manager: &astersql_dxf_framework_storage::TaskManager,
    job_manager: &astersql_dxf_framework_storage::TaskManager,
    before_fallback: impl FnOnce(),
) -> Result<(), astersql_dxf_framework_storage::Error> {
    use astersql_dxf_framework_storage as storage;
    let key = astersql_dxf_importinto::TaskKey(job_id);
    match task_manager.GetTaskBaseByKeyWithHistory((), key.clone()) {
        Ok(_) => {
            task_manager.WithNewTxn((), |session| {
                task_manager.CancelTaskByKeySession((), session, key.clone())
            })?;
            astersql_dxf_framework_handle::WaitTaskDoneByKeyWithManager(context, &key, task_manager)
        }
        Err(error) if error == storage::ErrTaskNotFound => {
            before_fallback();
            cancelDanglingImportJob(job_manager, job_id)
        }
        Err(error) => Err(error),
    }
}

/// Atomically cancel only a pending import job in its own keyspace.
pub fn cancelDanglingImportJob(
    manager: &astersql_dxf_framework_storage::TaskManager,
    job_id: i64,
) -> Result<(), astersql_dxf_framework_storage::Error> {
    use astersql_dxf_framework_storage as storage;
    manager.WithNewSession(|session| {
        let mut executor = astersql_dxf_importinto::scheduler::ImportJobStorageSession {
            executor: session.GetSQLExecutor(),
        };
        astersql_executor_importer::CancelPendingJob(&mut executor, job_id)
            .map_err(storage::Error::new)?;
        if session.GetSessionVars().StmtCtx.AffectedRows() == 0 {
            return Err(storage::Error::new(
                "job state changed during cancel, please try again later",
            ));
        }
        Ok(())
    })
}
