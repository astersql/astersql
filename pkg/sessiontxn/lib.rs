// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 会话事务（sessiontxn）crate 根模块。
//
// 汇总事务上下文 Provider 接口、时间戳 Future、故障注入点等子模块，
// 供会话层按隔离级别选择乐观/悲观事务策略；测试模块仅在 `cfg(test)` 下编译。

/// 故障注入点（failpoint）相关定义与钩子。
mod failpoint;
/// 时间戳 Future 抽象（异步获取 TSO / 常量时间戳等）。
mod future;
/// 事务上下文 Provider 对外接口与公共类型。
mod interface;

pub use failpoint::*;
pub use future::*;
pub use interface::*;

/// AsterSQL 迁移期单元测试（断言记录、动作辅助、常量 Future 契约等）。
#[cfg(test)]
#[path = "sessiontxn_aster_unit_test.rs"]
mod sessiontxn_aster_unit_test;

/// 事务管理器测试（对应 Go `txn_manager_test.go`）。
#[cfg(test)]
mod txn_manager_test;

/// 事务上下文测试（对应 Go `txn_context_test.go`）。
#[cfg(test)]
mod txn_context_test;

/// Public transaction-manager interface parity tests.
#[cfg(test)]
mod interface_test;

/// Failpoint helper parity tests.
#[cfg(test)]
mod failpoint_test;

/// RC 写路径跳过向 PD 取 TSO 的优化相关测试。
#[cfg(test)]
mod txn_rc_tso_optimize_test;
