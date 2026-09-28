// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// KV 分组名常量与索引 ID 互转。
//
// 数据行使用固定分组名 `"data"`；二级索引以索引 ID 的十进制字符串作为分组名，
// 便于在全局排序产物中按表数据与各索引分文件组织。

/// 表数据（行）对应的 KV 分组名。
pub const DataKVGroup: &str = "data";

/// 将索引 ID 转为 KV 分组名（十进制字符串）。
pub fn IndexID2KVGroup(indexID: i64) -> String {
    indexID.to_string()
}

/// 将 KV 分组名解析回索引 ID；非数字分组名返回解析错误。
pub fn KVGroup2IndexID(kvGroup: &str) -> std::result::Result<i64, std::num::ParseIntError> {
    kvGroup.parse()
}
