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

// uni-store（单机嵌入式存储）运行标志。
//
// 标记当前实例是否以 uni-store 作为存储引擎；主要用于 nextgen 场景下
// 判断是否需要保留 keyspace 前缀（uni-store 不会从 key 中剥离该前缀）。

// StandAloneTiDB 对应 Go 的可变 bool：为 true 时表示实例以 uni-store 作为存储引擎运行。
// 该标志仅服务 nextgen，因为 uni-store 不会从 key 中移除 keyspace 前缀。
// Go 全局变量可被直接读写；Rust 使用原子布尔值明确跨线程可见性，读写方应采用 Relaxed 顺序。
/// 是否以独立/单机 TiDB + uni-store 模式运行的全局原子标志。
pub static StandAloneTiDB: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
