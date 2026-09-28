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

// 系统 schema 名常量与判定。
//
// 对应 Go `pkg/util/filter/schema.go`：识别 TiDB/MySQL 内置库，以及 DM 心跳库、
// 巡检库等特殊 schema，供表过滤与元数据路径跳过系统库时使用。

// DMHeartbeatSchema is the heartbeat schema name
// DMHeartbeatSchema 对应 Go 导出的心跳库名变量；这里用常量保留字符串值。
/// DM 心跳库名（`dm_heartbeat`），用于心跳探测相关表过滤。
pub const DMHeartbeatSchema: &str = "dm_heartbeat";

// InspectionSchemaName is the `INSPECTION_SCHEMA` database name
// InspectionSchemaName 对应 Go 导出的 inspection schema 库名变量。
/// 巡检用 schema 名（`inspection_schema`）。
pub const InspectionSchemaName: &str = "inspection_schema";

// IsSystemSchema checks whether schema is system schema or not.
// case insensitive
// IsSystemSchema 保留 Go 语义：输入应已是小写，然后判断是否为 TiDB/DM 内置系统库。
/// 判断 schema 是否为系统库（含内存/系统库、DM 心跳库、巡检库）。
///
/// 调用方应传入已转小写的名字；`debug_assert` 对应 Go 的 intest 小写约束。
pub fn IsSystemSchema(schema: &str) -> bool {
    // Go 代码通过 intest.AssertFunc 校验调用方传入小写 schema；用 debug_assert 表达同一约束。
    debug_assert_eq!(schema, schema.to_lowercase());

    // 先匹配本包导出的特殊库名，再委托 metadef 判断内存库/系统库。
    schema == DMHeartbeatSchema || schema == InspectionSchemaName || metadef::IsMemOrSysDB(schema)
}
