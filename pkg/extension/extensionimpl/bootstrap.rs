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

//! 扩展 bootstrap 的 canonical 接线。
//!
//! extension、KV context、chunk、SQL executor 与 record set 均直接使用对应
//! crate 的公开 API；这里只保留 Domain 到系统会话的最小适配边界。

use std::sync::Arc;

use astersql_extension as extension;
use astersql_kv as kv;
use astersql_util_sqlexec as sqlexec;

/// Domain 为扩展 bootstrap 提供的最小能力。
///
/// Rust 的 canonical `Domain` 尚不拥有具体 session crate（用于避免
/// domain → session → domain 的依赖环），因此由 session 层实现此适配，
/// 并负责把池资源校验为 canonical `sessionctx::Context` 后返回其
/// `sqlexec::SQLExecutor`。
pub trait BootstrapDomain: Send + Sync {
    /// 系统会话池。
    fn SysSessionPool(&self) -> Arc<dyn extension::SessionPool>;

    /// 从借出的会话资源取得 canonical SQL executor。
    fn GetSQLExecutor<'a>(
        &self,
        resource: &'a mut dyn extension::SessionResource,
    ) -> Option<Box<dyn sqlexec::SQLExecutor + 'a>>;

    /// 会话资源的具体类型名，用于保持 Go 类型断言错误文本。
    fn SessionResourceTypeName(&self, resource: &dyn extension::SessionResource) -> String;

    /// Domain 的可选 etcd client。
    fn GetEtcdClient(&self) -> Option<Arc<extension::etcd_client::Client>>;
}

/// 扩展钩子使用的 bootstrap 上下文。
struct bootstrapContext<'a> {
    context: kv::Context,
    sql_executor: Box<dyn sqlexec::SQLExecutor + 'a>,
    etcd_client: Option<Arc<extension::etcd_client::Client>>,
    session_pool: Arc<dyn extension::SessionPool>,
}

impl extension::ExtensionContext for bootstrapContext<'_> {
    fn is_cancelled(&self) -> bool {
        self.context.is_cancelled()
    }
}

impl extension::BootstrapContext for bootstrapContext<'_> {
    fn ExecuteSQL(
        &mut self,
        sql: &str,
    ) -> Result<Vec<extension::chunk::Row>, extension::ExtensionError> {
        let context = kv::WithInternalSourceType(self.context.clone(), kv::InternalTxnBootstrap);
        let Some(mut record_set) = self
            .sql_executor
            .ExecuteInternal(&context, sql, Vec::new())
            .map_err(|error| extension::ExtensionError::new(error.to_string()))?
        else {
            return Ok(Vec::new());
        };

        let drain_result = sqlexec::DrainRecordSet(&context, record_set.as_mut(), 8)
            .map_err(|error| extension::ExtensionError::new(error.to_string()));
        let close_result = record_set
            .Close()
            .map_err(|error| extension::ExtensionError::new(error.to_string()));

        match (drain_result, close_result) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(rows), Ok(())) => Ok(rows),
        }
    }

    fn EtcdClient(&self) -> Option<&extension::etcd_client::Client> {
        self.etcd_client.as_deref()
    }

    fn SessionPool(&self) -> &dyn extension::SessionPool {
        self.session_pool.as_ref()
    }
}

/// 引导全部已注册扩展。
pub fn Bootstrap(
    ctx: &kv::Context,
    domain: &dyn BootstrapDomain,
) -> Result<(), extension::ExtensionError> {
    let Some(extensions) = extension::GetExtensions()? else {
        return Ok(());
    };

    let pool = domain.SysSessionPool();
    let mut resource = pool.Get()?;
    let resource_type_name = domain.SessionResourceTypeName(resource.as_ref());
    let result = match domain.GetSQLExecutor(resource.as_mut()) {
        None => Err(extension::ExtensionError::new(format!(
            "type '{}' cannot be casted to 'sessionctx.Context'",
            resource_type_name
        ))),
        Some(sql_executor) => {
            let mut bootstrap_context = bootstrapContext {
                context: ctx.clone(),
                sql_executor,
                etcd_client: domain.GetEtcdClient(),
                session_pool: Arc::clone(&pool),
            };
            extensions.Bootstrap(&mut bootstrap_context)
        }
    };
    pool.Put(resource);
    result
}

#[cfg(test)]
#[path = "bootstrap_test.rs"]
mod tests;
