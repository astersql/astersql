// Copyright 2026 AsterSQL.

// next-gen（下一代 TiDB）模式下若干系统变量的受限校验与会话写入。
//
// next-gen 关闭部分兼容能力（如悲观事务公平锁、bulk DML、非 leader 副本读）；
// Validate 在拒绝时仍返回规范化回落值，对齐 Go 的 `(val, err)` 约定。

#![allow(non_snake_case)]

use crate::error::{ErrNotSupportedInNextGen, ErrWrongValueForVar, ErrorDescriptor};
use crate::sysvar::{GlobalSystemVariableInitialValueWithRuntime, RuntimeEnvironment};
use crate::vardef;

/// 副本读（replica read）模式；next-gen 仅允许读 leader。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicaRead {
    Leader,
    PreferLeader,
    Follower,
    LeaderAndFollower,
    ClosestReplicas,
    ClosestAdaptive,
    Learner,
}

/// next-gen 测试用的精简会话变量视图。
#[derive(Debug, Eq, PartialEq)]
pub struct SessionVars {
    pub PessimisticTransactionFairLocking: bool,
    pub BulkDMLEnabled: bool,
    replica_read: ReplicaRead,
}

impl Default for SessionVars {
    fn default() -> Self {
        Self {
            PessimisticTransactionFairLocking: false,
            BulkDMLEnabled: false,
            replica_read: ReplicaRead::Leader,
        }
    }
}

impl SessionVars {
    /// 返回当前副本读模式。
    pub fn GetReplicaRead(&self) -> ReplicaRead {
        self.replica_read
    }
}

/// next-gen 不支持某变量取值时返回的错误，携带稳定 ErrorDescriptor 与格式化消息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NextGenError {
    descriptor: &'static ErrorDescriptor,
    message: String,
}

impl NextGenError {
    /// 构造「next-gen 不支持」错误，消息模板来自 ErrNotSupportedInNextGen。
    fn unsupported(name: &str) -> Self {
        Self {
            descriptor: &ErrNotSupportedInNextGen,
            message: ErrNotSupportedInNextGen.format(&[name]),
        }
    }

    fn wrong_value(name: &str, value: &str) -> Self {
        Self {
            descriptor: &ErrWrongValueForVar,
            message: ErrWrongValueForVar.format(&[name, value]),
        }
    }

    /// 返回错误描述符（含 MySQL 错误码等）。
    pub fn descriptor(&self) -> &'static ErrorDescriptor {
        self.descriptor
    }

    /// 返回已格式化的错误消息文本。
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// next-gen 关注的系统变量句柄（仅名称）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SysVar {
    name: &'static str,
}

/// 按名称查找 next-gen 受限系统变量；未列入则返回 None。
pub fn GetSysVar(name: &str) -> Option<SysVar> {
    match name {
        vardef::TiDBPessimisticTransactionFairLocking => Some(SysVar {
            name: vardef::TiDBPessimisticTransactionFairLocking,
        }),
        vardef::TiDBDMLType => Some(SysVar {
            name: vardef::TiDBDMLType,
        }),
        vardef::TiDBReplicaRead => Some(SysVar {
            name: vardef::TiDBReplicaRead,
        }),
        _ => None,
    }
}

impl SysVar {
    /// Returns the normalized value even when next-gen rejects the request,
    /// matching Go's `(val, err)` validation contract.
    /// 即使 next-gen 拒绝请求也返回规范化值，对齐 Go `(val, err)` 校验契约。
    pub fn Validate(&self, value: &str) -> (String, Option<NextGenError>) {
        match self.name {
            vardef::TiDBPessimisticTransactionFairLocking => {
                // Go 先按 TypeBool 规范化，再执行 next-gen 专用校验。
                let normalized = if value.eq_ignore_ascii_case(vardef::Off) || value == "0" {
                    vardef::Off
                } else if value.eq_ignore_ascii_case(vardef::On) || value == "1" {
                    vardef::On
                } else {
                    return (
                        value.to_owned(),
                        Some(NextGenError::wrong_value(self.name, value)),
                    );
                };
                if normalized == vardef::On {
                    return (
                        vardef::Off.to_owned(),
                        Some(NextGenError::unsupported(self.name)),
                    );
                }
                (vardef::Off.to_owned(), None)
            }
            vardef::TiDBDMLType => {
                // TypeStr 不做预校验；next-gen 仅禁止 bulk，其它值由 SetSession 处理。
                if value.eq_ignore_ascii_case("bulk") {
                    (
                        vardef::DefTiDBDMLType.to_owned(),
                        Some(NextGenError::unsupported(self.name)),
                    )
                } else {
                    (value.to_owned(), None)
                }
            }
            vardef::TiDBReplicaRead => {
                // Go TypeEnum 同时接受枚举文本与从 0 开始的序号。
                const VALUES: [&str; 7] = [
                    "leader",
                    "prefer-leader",
                    "follower",
                    "leader-and-follower",
                    "closest-replicas",
                    "closest-adaptive",
                    "learner",
                ];
                let normalized = VALUES
                    .iter()
                    .enumerate()
                    .find(|(index, possible)| {
                        possible.eq_ignore_ascii_case(value) || index.to_string() == value
                    })
                    .map(|(_, possible)| *possible);
                let Some(normalized) = normalized else {
                    return (
                        value.to_owned(),
                        Some(NextGenError::wrong_value(self.name, value)),
                    );
                };
                if normalized == "leader" {
                    ("leader".to_owned(), None)
                } else {
                    (
                        "leader".to_owned(),
                        Some(NextGenError::unsupported(self.name)),
                    )
                }
            }
            _ => unreachable!("GetSysVar only constructs supported next-gen variables"),
        }
    }

    /// 将已规范化的值写入会话变量结构（由 SET 钩子调用）。
    pub fn SetSessionFromHook(&self, vars: &mut SessionVars, value: &str) -> Result<(), String> {
        match self.name {
            vardef::TiDBPessimisticTransactionFairLocking => {
                vars.PessimisticTransactionFairLocking = value.eq_ignore_ascii_case(vardef::On);
            }
            vardef::TiDBDMLType => {
                if value.eq_ignore_ascii_case("standard") {
                    vars.BulkDMLEnabled = false;
                } else if value.eq_ignore_ascii_case("bulk") {
                    vars.BulkDMLEnabled = true;
                } else {
                    return Err(format!("unsupport DML type: {value}"));
                }
            }
            vardef::TiDBReplicaRead => {
                vars.replica_read = if value.eq_ignore_ascii_case("follower") {
                    ReplicaRead::Follower
                } else if value.eq_ignore_ascii_case("leader-and-follower") {
                    ReplicaRead::LeaderAndFollower
                } else if value.eq_ignore_ascii_case("leader") || value.is_empty() {
                    ReplicaRead::Leader
                } else if value.eq_ignore_ascii_case("closest-replicas") {
                    ReplicaRead::ClosestReplicas
                } else if value.eq_ignore_ascii_case("closest-adaptive") {
                    ReplicaRead::ClosestAdaptive
                } else if value.eq_ignore_ascii_case("learner") {
                    ReplicaRead::Learner
                } else if value.eq_ignore_ascii_case("prefer-leader") {
                    ReplicaRead::PreferLeader
                } else {
                    // Go 的钩子对未知值不修改状态并返回 nil；类型校验通常先行。
                    return Ok(());
                };
            }
            _ => unreachable!("GetSysVar only constructs supported next-gen variables"),
        }
        Ok(())
    }
}

/// 在 next-gen 运行时环境下计算全局系统变量的初始值。
pub fn GlobalSystemVariableInitialValue(var_name: &str, var_value: &str) -> String {
    GlobalSystemVariableInitialValueWithRuntime(
        var_name,
        var_value,
        &RuntimeEnvironment {
            store_is_tikv: false,
            in_test: true,
            next_gen: true,
            default_txn_assertion_level: vardef::GetDefaultTxnAssertionLevel().to_owned(),
        },
    )
}
