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

//! Column family name constants matching `br/pkg/utils/consts`.
//!
//! RocksDB/TiKV 列族名常量；与 Go 侧字符串必须字节级一致，供备份恢复选 CF。

/// Default columnFamily and write columnFamily
/// 默认数据列族名（TiKV `default` CF）。
pub const DefaultCF: &str = "default";
/// Write CF：事务写意图与提交记录所在列族。
pub const WriteCF: &str = "write";
