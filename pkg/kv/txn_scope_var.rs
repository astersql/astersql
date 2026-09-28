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

// 事务作用域变量（`@@txn_scope`）封装。
//
// 事务作用域决定向 PD（Placement Driver，集群元数据与时间戳服务）请求
// TSO（Timestamp Oracle，全局/本地时间戳）时使用的 scope：`global` 走全局
// 时间戳，`local` 可绑定具体部署区域（zone label）以降低跨区延迟。
// 对外会话变量只暴露 `global`/`local`，真实区域标签保存在内部字段。

// TxnScopeVar 对应 Go 同名结构：varValue 是 @@txn_scope 对外值，txnScope 是向 PD 请求 TSO 的真实 scope。
use crate::config;

/// 会话变量显示值与真实 TSO scope 的配对容器。
pub struct TxnScopeVar {
    // @@txn_scope 只暴露 global 或 local，不直接泄漏部署区域标签。
    varValue: String,
    // 本字段可保存具体 zone label，并由 GetTxnScope 交给后续 TSO 请求层。
    txnScope: String,
}

// NewDefaultTxnScopeVar 根据全局配置选择默认 scope。
// 配置返回非 global 值时把它作为真实区域 scope，同时把会话变量显示为 local。
/// 按全局配置构造默认 `TxnScopeVar`；非 global 配置映射为 local 显示值。
pub fn NewDefaultTxnScopeVar() -> TxnScopeVar {
    let txnScope = config::GetTxnScopeFromConfig();
    if txnScope != GlobalTxnScope {
        return NewLocalTxnScopeVar(txnScope);
    }
    NewGlobalTxnScopeVar()
}

// NewGlobalTxnScopeVar 创建变量值和真实 scope 都为 global 的实例。
/// 构造显示值与真实 scope 均为 `global` 的实例。
pub fn NewGlobalTxnScopeVar() -> TxnScopeVar {
    newTxnScopeVar(GlobalTxnScope.to_owned(), GlobalTxnScope.to_owned())
}

// NewLocalTxnScopeVar 创建对外值为 local、真实值为给定区域标签的实例。
/// 构造对外值为 `local`、真实 scope 为给定区域标签的实例。
pub fn NewLocalTxnScopeVar(txnScope: String) -> TxnScopeVar {
    newTxnScopeVar(LocalTxnScope.to_owned(), txnScope)
}

impl TxnScopeVar {
    // GetVarValue 返回 @@txn_scope 的 global/local 值。
    // Go 按值返回 string；Rust 借用字段以避免为只读访问额外复制。
    /// 返回 `@@txn_scope` 对外显示值（`global` 或 `local`）。
    pub fn GetVarValue(&self) -> &str {
        &self.varValue
    }

    // GetTxnScope 返回 tidb-server 实际用于请求 TSO 的 scope；local 场景可能是具体 zone label。
    /// 返回实际用于请求 TSO 的 scope（local 时可能是具体 zone label）。
    pub fn GetTxnScope(&self) -> &str {
        &self.txnScope
    }
}

// newTxnScopeVar 保持 Go 私有构造函数的字段赋值顺序。
/// 私有构造：按 Go 字段顺序赋值。
fn newTxnScopeVar(varValue: String, txnScope: String) -> TxnScopeVar {
    TxnScopeVar { varValue, txnScope }
}

// GlobalTxnScope 与 PD/oracle 的 global 定义保持同步；跨 crate 常量接线留给后续模块化任务。
/// 全局事务作用域常量，与 PD/oracle 的 global 定义一致。
pub const GlobalTxnScope: &str = "global";

// LocalTxnScope 表示事务应使用本地时间戳服务。
/// 本地事务作用域常量：使用本地时间戳服务。
pub const LocalTxnScope: &str = "local";
