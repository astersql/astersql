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

// 进程级全局变量初始化。
//
// 从系统库（如 `mysql.tidb`）加载时区与排序规则（Collation）参数，
// 写入进程全局状态；通过 `SchemaLoadFilter` 仅加载系统库元数据，
// 避免拉起完整业务 InfoSchema（信息模式）。

#![allow(dead_code, non_snake_case)]

/// 模式差异（Schema Diff）占位类型，对应增量加载时的变更描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDiff;

/// InfoSchema（信息模式）占位类型，描述当前可见的库表元数据快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoSchema;

/// 数据库元信息的轻量描述，此处仅保留库名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DBInfo {
    /// 数据库名称。
    pub name: String,
}

/// 控制 Domain 初始化时跳过哪些 diff / schema 的过滤器。
pub trait SchemaLoadFilter {
    /// 是否跳过加载给定的模式差异。
    fn SkipLoadDiff(&self, diff: &SchemaDiff, info_schema: &InfoSchema) -> bool;
    /// 是否跳过加载给定数据库的 schema。
    fn SkipLoadSchema(&self, db_info: &DBInfo) -> bool;
}

/// 仅加载系统库的过滤器实现。
#[derive(Debug, Default, Clone, Copy)]
#[allow(non_camel_case_types)]
pub struct systemDBFilter;

impl SchemaLoadFilter for systemDBFilter {
    fn SkipLoadDiff(&self, _diff: &SchemaDiff, _info_schema: &InfoSchema) -> bool {
        false
    }

    fn SkipLoadSchema(&self, db_info: &DBInfo) -> bool {
        // 非系统库一律跳过，只保留 mysql / information_schema 等。
        !is_system_database(&db_info.name)
    }
}

impl systemDBFilter {
    /// 委托 trait 方法：是否跳过 diff。
    pub fn SkipLoadDiff(&self, diff: &SchemaDiff, info_schema: &InfoSchema) -> bool {
        SchemaLoadFilter::SkipLoadDiff(self, diff, info_schema)
    }

    /// 委托 trait 方法：是否跳过 schema。
    pub fn SkipLoadSchema(&self, db_info: &DBInfo) -> bool {
        SchemaLoadFilter::SkipLoadSchema(self, db_info)
    }
}

/// 判断库名是否为内置系统库（大小写不敏感）。
fn is_system_database(name: &str) -> bool {
    name.eq_ignore_ascii_case("mysql")
}

/// All database and process-global effects are mandatory production boundaries.
/// Implementations must create an isolated domain; no operation has a success default.
///
/// 全局初始化运行时边界：所有库级与进程级副作用必须由实现方提供；
/// 必须创建隔离 Domain，任一操作都没有“默认成功”路径。
pub trait GlobalInitRuntime {
    /// 运行时错误类型。
    type Error;
    /// 底层存储句柄类型。
    type Store;
    /// Domain（域）类型：承载 InfoSchema、统计等共享组件。
    type Domain;
    /// 会话类型。
    type Session;

    /// 按过滤器获取或创建 Domain。
    fn get_domain_for_global_var_init(
        &mut self,
        store: &Self::Store,
        filter: systemDBFilter,
        server_info_options: &[astersql_domain_serverinfo::SyncerOption],
    ) -> Result<Self::Domain, Self::Error>;
    /// 在给定 Domain 上创建临时会话。
    fn create_session(
        &mut self,
        store: &Self::Store,
        domain: &Self::Domain,
    ) -> Result<Self::Session, Self::Error>;
    /// 读取系统表指定 key 的字符串值（如 `tidb.system_tz`）。
    fn table_value(
        &mut self,
        session: &Self::Session,
        table: &str,
        key: &str,
    ) -> Result<String, Self::Error>;
    /// 从会话加载是否启用新排序规则参数。
    fn load_collation_parameter(&mut self, session: &Self::Session) -> Result<bool, Self::Error>;
    /// 设置进程级系统时区。
    fn set_system_timezone(&mut self, timezone: String);
    /// 测试用：设置新排序规则启用开关。
    fn set_new_collation_enabled_for_test(&mut self, enabled: bool);
    /// 关闭隔离 Domain，释放资源。
    fn close_domain(&mut self, domain: Self::Domain);
}

/// 从系统库初始化进程级全局变量（时区、排序规则开关）。
pub fn initGlobalVarFromSystemDB<R: GlobalInitRuntime>(
    runtime: &mut R,
    store: &R::Store,
) -> Result<(), R::Error> {
    let domain = runtime.get_domain_for_global_var_init(
        store,
        systemDBFilter,
        &[astersql_domain_serverinfo::SyncerOption::WithoutStatusEndpointClaim],
    )?;

    // Keep the temporary session inside this scope so it is dropped before the
    // isolated domain is closed, matching Go's deferred dom.Close ordering.
    // 临时会话必须在 Domain 关闭前析构，对齐 Go 中 defer dom.Close 的顺序。
    let result = (|| {
        let session = runtime.create_session(store, &domain)?;
        let timezone = runtime.table_value(&session, "tidb", "system_tz")?;
        runtime.set_system_timezone(timezone);

        let new_collation_enabled = runtime.load_collation_parameter(&session)?;
        runtime.set_new_collation_enabled_for_test(new_collation_enabled);
        Ok(())
    })();

    runtime.close_domain(domain);
    result
}
