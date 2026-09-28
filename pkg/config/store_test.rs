// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 存储引擎类型（StoreType）的集成测试。
//
// 数据库内核可以对接多种底层存储引擎（例如 TiKV 分布式 KV 存储、
// 本地 mock 存储、TiFlash 列式存储等）。配置模块通过存储类型列表
// 描述当前支持的引擎种类。本测试验证 `StoreTypeList` 返回的
// 存储类型列表的数量与合法性。

use astersql_config::StoreTypeList;

/// 验证存储类型列表的完整性：
/// - 列表应包含 3 种存储类型；
/// - 每一种类型都必须通过 `Valid()` 合法性校验。
#[test]
fn test_store_type() {
    // 获取配置模块中定义的全部存储引擎类型
    let store_types = StoreTypeList();
    // 预期支持 3 种存储类型（如 TiKV / UniStore / TiFlash）
    assert_eq!(store_types.len(), 3);
    // 每个存储类型都应是合法的已知类型
    assert!(store_types.iter().all(|store_type| store_type.Valid()));
}
