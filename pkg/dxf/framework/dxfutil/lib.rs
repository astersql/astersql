// Copyright 2026 AsterSQL.

// DXF 工具层（dxfutil）：会话上下文、任务管理器与跨 keyspace 运行时获取。
//
// Keyspace 是多租户隔离单元；本 crate 提供获取/校验任务运行时（Runtime）的能力，
// 并桥接 KV 存储、SQL Server API、Owner Manager 等依赖。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::any::Any;
use std::sync::Arc;

/// 再导出 KV 存储依赖（事务、快照等底层接口）。
pub use kv_dependency as kv;

/// 元数据模型命名空间（如 AlterTableMode 目标等）。
pub mod meta {
    pub mod model {
        pub use model_dependency::group_3::*;
    }
}

/// DDL Owner 管理器再导出；Owner 负责集群内唯一调度权威节点选举。
pub mod owner {
    pub use owner_dependency::Manager;
}

/// SQL Server API：获取 Runtime、按 keyspace 申请 KSRuntimeHandle 等。
pub mod sqlsvrapi {
    pub use sqlsvrapi_dependency::server::*;
}

/// 会话池（Session Pool）工具，供任务管理器借还 session。
pub mod util {
    pub use util_dependency::session_pool::*;
}

/// 会话上下文：绑定 KV Storage 与可选 SQL Server，实现池化资源接口。
pub mod sessionctx {
    use super::*;

    #[derive(Clone)]
    /// 一次会话的上下文：持有存储句柄，可选挂接 SQL Server 以跨 keyspace 取 Runtime。
    pub struct Context {
        store: Arc<dyn kv::Storage + Send + Sync>,
        sql_server: Option<Arc<dyn sqlsvrapi::Server>>,
    }

    impl Context {
        /// 用给定存储与可选 SQL Server 构造会话上下文。
        pub fn new(
            store: Arc<dyn kv::Storage + Send + Sync>,
            sql_server: Option<Arc<dyn sqlsvrapi::Server>>,
        ) -> Self {
            Self { store, sql_server }
        }

        /// 返回会话绑定的 KV 存储（用于读取当前 keyspace 等）。
        pub fn GetStore(&self) -> Arc<dyn kv::Storage + Send + Sync> {
            Arc::clone(&self.store)
        }

        /// 返回 SQL Server；未配置时 panic（跨 keyspace 路径必须具备该依赖）。
        pub fn GetSQLServer(&self) -> Arc<dyn sqlsvrapi::Server> {
            Arc::clone(
                self.sql_server
                    .as_ref()
                    .expect("session does not provide a SQL server"),
            )
        }
    }

    /// 作为会话池资源：关闭为空操作，`as_any` 供下转型。
    impl util::Resource for Context {
        fn close(&self) {}

        fn as_any(&self) -> &dyn Any {
            self
        }
    }
}

/// 任务管理侧存储适配：通过会话池借 session 执行回调。
pub mod storage {
    use super::*;

    /// 任务管理器：持有可销毁会话池，在回调内执行需 session 的逻辑。
    pub struct TaskManager {
        pool: Arc<dyn util::DestroyableSessionPool>,
    }

    /// 用给定会话池构造 TaskManager。
    pub fn NewTaskManager(pool: Arc<dyn util::DestroyableSessionPool>) -> TaskManager {
        TaskManager { pool }
    }

    impl TaskManager {
        /// 从池中取出 session，下转型为 Context 后调用 callback，最后归还资源。
        pub fn WithNewSession<F>(&self, callback: F) -> Result<(), sqlsvrapi::SqlSvrError>
        where
            F: FnOnce(sessionctx::Context) -> Result<(), sqlsvrapi::SqlSvrError>,
        {
            let resource = self.pool.Get().map_err(|error| -> sqlsvrapi::SqlSvrError {
                std::io::Error::other(error.to_string()).into()
            })?;
            let session = resource
                .as_any()
                .downcast_ref::<sessionctx::Context>()
                .cloned()
                .ok_or_else(|| -> sqlsvrapi::SqlSvrError {
                    std::io::Error::other("session pool returned a non-session resource").into()
                })?;
            let result = callback(session);
            self.pool.Put(resource);
            result
        }
    }
}

/// 核心工具实现（AcquireTaskRuntime / CheckTaskRuntime / GenHolderID）。
#[path = "util.rs"]
mod dxfutil_impl;
pub use dxfutil_impl::*;

/// 迁移对齐测试：覆盖跨 keyspace 获取 Runtime 与校验分支。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// 基于 mock 的 util 单元测试。
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
