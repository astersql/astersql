// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 系统/内存数据库名称常量与库名分类判定。
//
// 区分三类特殊库：
// - 内存元数据库（INFORMATION_SCHEMA / PERFORMANCE_SCHEMA / METRICS_SCHEMA）：
//   表数据不落盘，由内核按需生成；
// - 系统相关库（mysql / sys / workload_schema）：存放权限、统计、工作负载等持久化元数据；
// - BR 临时库：备份恢复（Backup & Restore）过程中创建的带固定前缀的临时数据库。

// ast::CIStr 与 mysql 常量沿用原 Go 包语义，待跨文件模块接线时解析。

use std::sync::LazyLock;

use crate::{ast, mysql};

// Go 包级变量通过 ast.NewCIStr 同时保存原始形式和小写形式。
// LazyLock 保留运行时构造语义，避免假设 CIStr 可以在 Rust const 上下文初始化。
/// INFORMATION_SCHEMA 库名（大小写不敏感字符串，O 为大写、L 为小写）。
pub static InformationSchemaName: LazyLock<ast::CIStr> =
    LazyLock::new(|| ast::NewCIStr("INFORMATION_SCHEMA"));
/// PERFORMANCE_SCHEMA 库名。
pub static PerformanceSchemaName: LazyLock<ast::CIStr> =
    LazyLock::new(|| ast::NewCIStr("PERFORMANCE_SCHEMA"));
/// METRICS_SCHEMA 库名（TiDB 扩展的指标元数据库）。
pub static MetricSchemaName: LazyLock<ast::CIStr> =
    LazyLock::new(|| ast::NewCIStr("METRICS_SCHEMA"));

// ClusterTableInstanceColumnName 是集群表中 INSTANCE 列的固定名称。
/// 集群表（Cluster Table）结果行中标识来源实例地址的列名。
pub const ClusterTableInstanceColumnName: &str = "INSTANCE";

// temporaryDBNamePrefix 是 BR 创建临时数据库时使用的区分前缀。
/// BR（Backup & Restore）临时数据库名前缀；大小写敏感。
const temporaryDBNamePrefix: &str = "__TiDB_BR_Temporary_";

// IsMemOrSysDB 判断小写库名是否属于内存库或系统相关库，保留 Go 的短路或逻辑。
/// 判断小写库名是否为内存 schema 或系统相关库之一。
pub fn IsMemOrSysDB(dbLowerName: &str) -> bool {
    IsMemDB(dbLowerName) || IsSystemRelatedDB(dbLowerName)
}

// IsMemDB 判断库名是否为三个内存元数据库之一。
/// 判断库名是否为 INFORMATION_SCHEMA / PERFORMANCE_SCHEMA / METRICS_SCHEMA。
pub fn IsMemDB(dbLowerName: &str) -> bool {
    // 调用方按照 Go 约定传入小写名称，因此比较 CIStr 的 L 字段而不是原始 O 字段。
    matches!(
        dbLowerName,
        name if name == InformationSchemaName.L
            || name == PerformanceSchemaName.L
            || name == MetricSchemaName.L
    )
}

// IsSystemRelatedDB 包含 mysql 系统库、sys 库以及 workload schema。
/// 判断是否为 mysql / sys / workload_schema 等系统相关库。
pub fn IsSystemRelatedDB(dbLowerName: &str) -> bool {
    IsSystemDB(dbLowerName) || dbLowerName == mysql::SysDB || dbLowerName == mysql::WorkloadSchema
}

// IsSystemDB 只判断 mysql.SystemDB，不把其他系统相关库混入该分类。
/// 仅判断是否为 `mysql` 系统库（不含 sys / workload_schema）。
pub fn IsSystemDB(dbLowerName: &str) -> bool {
    dbLowerName == mysql::SystemDB
}

// IsBRRelatedDB 判断原始库名是否带有 BR 临时库前缀。
// Go 的 strings.HasPrefix 对大小写敏感，Rust starts_with 保留这一点。
/// 判断原始库名是否以 BR 临时库前缀开头（大小写敏感）。
pub fn IsBRRelatedDB(dbOriginName: &str) -> bool {
    dbOriginName.starts_with(temporaryDBNamePrefix)
}
