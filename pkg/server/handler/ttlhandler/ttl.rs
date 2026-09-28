// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 手动触发表级 TTL（Time-To-Live）清理任务的 HTTP handler。
//
// 对齐 Go 侧 `TTLJobTriggerHandler`：仅接受 POST，从路径读取库表名，
// 通过运行时依赖触发新 TTL job 并写回 JSON 响应。

#![allow(dead_code, non_snake_case)]

/// TTL 客户端返回的完整触发结果。
pub type TTLResponse = astersql_ttl_client::TriggerNewTtlJobResponse;

/// TTL handler 对外依赖的运行时边界（HTTP、存储、日志）。
///
/// 将具体 HTTP 框架与存储实现解耦，便于单测注入 mock。
pub trait TTLHandlerRuntime {
    type Error;
    type Store;
    type RequestContext;
    type SessionDomain;

    /// 当前请求方法（如 POST）。
    fn request_method(&self) -> &str;
    /// 从路由路径取值（如 `db`、`table`）。
    fn path_value(&self, name: &str) -> String;
    /// 构造请求上下文，供触发 TTL job 时传播取消/超时。
    fn request_context(&self) -> Self::RequestContext;
    /// 从存储句柄取得 session domain；失败时 handler 必须立即返回。
    fn get_session_domain(
        &mut self,
        store: &Self::Store,
    ) -> Result<Self::SessionDomain, Self::Error>;
    /// 通过 domain 对指定库表触发一次新的 TTL 清理任务。
    fn trigger_new_ttl_job(
        &mut self,
        domain: &Self::SessionDomain,
        context: Self::RequestContext,
        database: &str,
        table: &str,
    ) -> Result<TTLResponse, Self::Error>;
    /// 方法不允许时构造错误（对齐 Go 的 Method Not Allowed）。
    fn method_not_allowed_error(&mut self) -> Self::Error;
    /// 将错误写出到 HTTP 响应。
    fn write_error(&mut self, error: Self::Error);
    /// 将成功响应序列化写出。Go 的 `handler.WriteData` 不向调用方返回错误。
    fn write_data(&mut self, response: &TTLResponse);
    /// 记录触发成功日志及完整响应体。
    fn log_success(&mut self, database: &str, table: &str, response: &TTLResponse);
    /// 记录触发或写出失败日志。
    fn log_failure(&mut self, message: &str, error: &Self::Error);
}

/// 持有存储句柄的 TTL job 触发 handler。
pub struct TTLJobTriggerHandler<S> {
    /// kv.Storage 或等价存储依赖。
    pub store: S,
}

/// 构造 `TTLJobTriggerHandler`。
pub fn NewTTLJobTriggerHandler<S>(store: S) -> TTLJobTriggerHandler<S> {
    TTLJobTriggerHandler { store }
}

impl<S> TTLJobTriggerHandler<S> {
    /// 处理 HTTP：校验 POST，解析库表，触发 TTL job 并写响应。
    pub fn ServeHTTP<R>(&self, runtime: &mut R)
    where
        R: TTLHandlerRuntime<Store = S>,
    {
        // 仅允许 POST，与 Go API 契约一致。
        if runtime.request_method() != "POST" {
            let error = runtime.method_not_allowed_error();
            runtime.write_error(error);
            return;
        }

        // 路径参数统一小写，避免大小写敏感库表名歧义。
        let database = runtime.path_value("db").to_lowercase();
        let table = runtime.path_value("table").to_lowercase();
        let context = runtime.request_context();
        let domain = match runtime.get_session_domain(&self.store) {
            Ok(domain) => domain,
            Err(error) => {
                runtime.log_failure("failed to get session domain", &error);
                runtime.write_error(error);
                return;
            }
        };
        match runtime.trigger_new_ttl_job(&domain, context, &database, &table) {
            Ok(response) => {
                runtime.write_data(&response);
                runtime.log_success(&database, &table, &response);
            }
            Err(error) => {
                runtime.log_failure("failed to trigger new TTL job", &error);
                runtime.write_error(error);
            }
        }
    }
}

/// 自由函数入口，等价于 `handler.ServeHTTP`。
pub fn serve_http<S, R>(handler: &TTLJobTriggerHandler<S>, runtime: &mut R)
where
    R: TTLHandlerRuntime<Store = S>,
{
    handler.ServeHTTP(runtime);
}
