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

// DXF 运行时工具：按任务 keyspace 获取/释放 Runtime，并校验 store/session 一致性。
//
// 当任务属于其他 keyspace 时，通过 SQL Server 的 AcquireKSRuntime 借出句柄；
// 调用方必须在用完后执行返回的 release 闭包，避免泄漏持有者（holder）。

use std::sync::Arc;

use crate::{sessionctx, sqlsvrapi, storage};

// sessionProvider 提供可访问 SQL Server Runtime 的会话。
// sessionProvider provides a session used to access the SQL server runtime.
/// 会话提供者：在回调中借出 `sessionctx::Context`。
pub trait sessionProvider {
    fn WithNewSession<F>(&self, callback: F) -> Result<(), sqlsvrapi::SqlSvrError>
    where
        F: FnOnce(sessionctx::Context) -> Result<(), sqlsvrapi::SqlSvrError>;
}

// AcquireTaskRuntime 返回任务 keyspace 对应的 Runtime 视图及释放函数。
// sessionProvider 提供的会话，其 store keyspace 必须是当前节点 keyspace，
// 据此判断任务是否属于另一 keyspace。
// 调用方在不再使用 Runtime 时必须调用释放函数。
// AcquireTaskRuntime returns a runtime view for the task keyspace and a release function.
// The sessionProvider must supply sessions whose store keyspace is the current node's keyspace;
// this is used to detect whether the task belongs to a different keyspace.
// Callers must call the release function when the returned runtime is no longer used.
/// 获取任务 keyspace 的 Runtime；跨 keyspace 时申请 KSRuntimeHandle，同 keyspace 则用本机 GetRuntime。
pub fn AcquireTaskRuntime<P>(
    sessionProvider: P,
    taskKS: String,
    holderID: String,
) -> Result<
    (
        Arc<dyn sqlsvrapi::Runtime>,
        Box<dyn FnOnce() + Send + 'static>,
    ),
    sqlsvrapi::SqlSvrError,
>
where
    P: sessionProvider,
{
    let mut taskRuntime: Option<Arc<dyn sqlsvrapi::Runtime>> = None;
    let mut acquiredHandle: Option<Arc<dyn sqlsvrapi::KSRuntimeHandle>> = None;

    // 比较任务 keyspace 与当前会话 store keyspace，决定 Acquire 或直取本地 Runtime。
    sessionProvider.WithNewSession(|se: sessionctx::Context| {
        let currentKS = se.GetStore().GetKeyspace();
        let sqlServer = se.GetSQLServer();
        if taskKS != currentKS {
            let handle = sqlServer.AcquireKSRuntime(taskKS.clone(), holderID.clone())?;
            let runtime: Arc<dyn sqlsvrapi::Runtime> = handle.clone();
            taskRuntime = Some(runtime);
            acquiredHandle = Some(handle);
            return Ok(());
        }
        taskRuntime = Some(sqlServer.GetRuntime());
        Ok(())
    })?;

    let runtime = taskRuntime.expect("WithNewSession returned without invoking its callback");
    Ok((
        runtime,
        Box::new(move || releaseTaskRuntime(acquiredHandle)),
    ))
}

/// 若曾跨 keyspace 获取句柄，则调用 Release；同 keyspace 路径无句柄，为空操作。
fn releaseTaskRuntime(runtimeHandle: Option<Arc<dyn sqlsvrapi::KSRuntimeHandle>>) {
    if let Some(handle) = runtimeHandle {
        handle.Release();
    }
}

/// 将普通错误消息包装为 SqlSvrError。
fn taskRuntimeError(message: String) -> sqlsvrapi::SqlSvrError {
    std::io::Error::other(message).into()
}

// CheckTaskRuntime 检查 Runtime 是否与目标任务 keyspace 匹配。
// CheckTaskRuntime checks if the runtime is valid for the task with the target keyspace.
/// 校验 Runtime 的 store keyspace、以及会话池内 session 的 keyspace 均与 taskKS 一致。
pub fn CheckTaskRuntime(
    runtime: Arc<dyn sqlsvrapi::Runtime>,
    taskKS: String,
) -> Result<(), sqlsvrapi::SqlSvrError> {
    let storeKS = runtime.Store().GetKeyspace();
    if storeKS != taskKS {
        // shouldn't happen normally, but since keyspace mismatch might cause
        // correctness error, we check it at runtime too.
        return Err(taskRuntimeError(format!(
            "store keyspace mismatch with task: {} vs {}",
            storeKS, taskKS
        )));
    }

    let taskMgr = storage::NewTaskManager(runtime.SysSessionPool());
    taskMgr.WithNewSession(|se: sessionctx::Context| {
        let sessKs = se.GetStore().GetKeyspace();
        if storeKS != sessKs {
            // shouldn't happen normally. we do it for the same reason as above.
            return Err(taskRuntimeError(format!(
                "invalid task runtime with mismatched keyspace: {} vs {}",
                storeKS, sessKs
            )));
        }
        Ok(())
    })
}

// GenHolderID 为指定 DXF 组件与任务 ID 生成 holder 标识。
// GenHolderID generates a holder ID for the given DXF component and task ID.
/// 生成形式为 `DXF/{component}/{taskID}` 的持有者 ID，供 AcquireKSRuntime 追踪。
pub fn GenHolderID(component: String, taskID: i64) -> String {
    format!("DXF/{}/{}", component, taskID)
}
