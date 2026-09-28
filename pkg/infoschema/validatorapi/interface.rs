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

// Schema 校验器接口：在事务时间戳处判定所用 schema 版本是否仍在租约内有效。
//
// 对应 Go `infoschema/validator` 的对外契约。事务（transaction）提交或执行期间会
// 携带其可见的 schema 版本；校验失败意味着中间发生了影响相关表的 DDL，需要重试。
//
// 租约（lease）：schema 版本被授予的有效时间窗口；过期后校验器可能返回 Unknown。

#![allow(dead_code, non_snake_case)]

// Result represents the result of info schema validation.
// Result 对应 Go 的 int 枚举，判定一次 schema 版本校验成功、失败或未知。
/// InfoSchema 校验结果，与 Go 侧 iota 取值一致。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Result {
    // ResultSucc means schemaValidator's check is passing.
    /// 校验通过：事务可继续使用该 schema 版本。
    ResultSucc = 0,
    // ResultFail means schemaValidator's check is fail.
    /// 校验失败：相关表的 schema 已变更，事务应中止或重试。
    ResultFail = 1,
    // ResultUnknown means schemaValidator doesn't know the check would be success or fail.
    /// 结果未知：例如校验器已停止或租约信息不足，调用方需按未知路径处理。
    ResultUnknown = 2,
}

// Validator is the interface for checking the validity of schema version.
// Validator 对应 Go 接口：调用者通过它维护租约窗口，并检查事务时间戳可见的 schema 版本。
/// Schema 版本校验器：维护租约窗口并检查事务时间戳处的 schema 是否可用。
pub trait Validator {
    /// Transaction-specific schema change information.
    ///
    /// Go obtains this type from TiKV client-go. Keeping it associated with
    /// the validator preserves that boundary without coupling this API crate
    /// to a particular Rust transaction client.
    /// 与事务相关的 schema 变更摘要（表 ID、动作类型等），由事务客户端提供。
    type RelatedSchemaChange;

    // Update the schema validator, add a new item, delete the expired deltaSchemaInfos.
    // The latest schemaVer is valid within leaseGrantTime plus lease duration.
    // Add the changed table IDs to the new schema information,
    // which is produced when the oldSchemaVer is updated to the newSchemaVer.
    // Update 保留 Go 参数顺序；change 为事务客户端提供的相关 schema 变化信息。
    /// 更新校验器：登记 `oldSchemaVer -> newSchemaVer` 的增量，并清理过期条目。
    /// `leaseGrantTime` 起的租约窗口内，`newSchemaVer` 被视为有效。
    fn Update(
        &self,
        leaseGrantTime: u64,
        oldSchemaVer: i64,
        newSchemaVer: i64,
        change: Option<&Self::RelatedSchemaChange>,
    );

    // Check is it valid for a transaction to use schemaVer and related tables, at timestamp txnTS.
    // Check 返回可能需要的 RelatedSchemaChange 和 Result；是否检查表级 schema 由 needCheckSchema 控制。
    // None 保留 Go nil slice，Some(&[]) 保留非 nil 空 slice；实现会区分这两种输入。
    /// 在时间戳 `txnTS` 检查事务使用 `schemaVer` 及关联物理表是否合法。
    fn Check(
        &self,
        txnTS: u64,
        schemaVer: i64,
        relatedPhysicalTableIDs: Option<&[i64]>,
        needCheckSchema: bool,
    ) -> (Option<Self::RelatedSchemaChange>, Result);

    // Stop stops checking the valid of transaction.
    /// 停止校验（例如节点关闭或切换角色时）。
    fn Stop(&self);
    // Restart restarts the schema validator after it is stopped.
    /// 在停止后以当前 schema 版本重启校验器。
    fn Restart(&self, currSchemaVer: i64);
    // Reset resets Validator to initial state.
    /// 重置为初始状态（启动、未过期、版本清零等）。
    fn Reset(&self);
    // IsStarted indicates whether Validator is started.
    /// 校验器是否处于已启动状态。
    fn IsStarted(&self) -> bool;
    // IsLeaseExpired checks whether the current lease has expired.
    /// 当前 schema 租约是否已过期。
    fn IsLeaseExpired(&self) -> bool;
}
