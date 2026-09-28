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

//! Common helpers ported from `br/pkg/utils/common.go`.
//!
//! BR 通用元数据辅助：全局 ID 分配与 Next-Gen keyspace 兼容校验。
//! 对齐 Go `br/pkg/utils/common.go`；恢复/分配表 ID 等路径会调用这些入口。
//! `NewMutator`/`GenGlobalIDs` 走内部 BR 事务源标记，避免污染用户事务语义。
//! `CheckNextGenCompatibility` 按内核类型（classic/next-gen）决定 panic 或跳过。

use crate::stubs::{
    InternalTxnBR, KvContext as Context, RunInNewTxn, Storage, Transaction, WithInternalSourceType,
};
use astersql_br_pkg_logutil::log;
use astersql_config_kerneltype::{IsClassic, IsNextGen};
use astersql_errors::SharedError;
use astersql_meta::{self, new_mutator};

// 将 meta 包错误收敛为 SharedError，供 RunInNewTxn 回调统一返回。
fn map_meta_error(err: astersql_meta::errors::Error) -> SharedError {
    SharedError::new(std::io::Error::new(
        std::io::ErrorKind::Other,
        err.to_string(),
    ))
}

/// Creates a meta mutator from a KV transaction.
/// 从当前 KV 事务构造 meta Mutator，并由 meta 层设置高优先级与近满盘允许写入。
pub fn NewMutator(txn: &mut dyn Transaction) -> astersql_meta::Mutator {
    new_mutator(txn.meta_transaction(), vec![])
}

/// Generates several global ids inside an internal BR transaction.
/// 在 InternalTxnBR 标记的新事务中批量申请全局 ID，语义对齐 Go GenGlobalIDs。
pub fn GenGlobalIDs(ctx: Context, n: i32, storage: &dyn Storage) -> Result<Vec<i64>, SharedError> {
    let mut ids = Vec::new();
    // 标记为 BR 内部事务，避免与业务事务源混淆。
    let ctx = WithInternalSourceType(ctx, InternalTxnBR);
    RunInNewTxn(&ctx, storage, true, |_ctx, txn: &mut dyn Transaction| {
        ids = NewMutator(txn).gen_global_ids(n).map_err(map_meta_error)?;
        Ok(())
    })?;
    Ok(ids)
}

/// Validates Next-Gen restore compatibility and returns whether the restore targets Next-Gen.
/// 校验 classic/next-gen 与 keyspace 是否匹配；严格模式下不兼容则 panic，否则仅告警。
pub fn CheckNextGenCompatibility(keyspace_name: &str, check_requirements: bool) -> bool {
    // classic 内核不应带 keyspace 做恢复，否则可能撑爆磁盘。
    if IsClassic() && !keyspace_name.is_empty() {
        let msg = concat!(
            "classic kernel does not support keyspace restore; ",
            "it may cause high disk usage. If you are certain this can be ignored, ",
            "set --check-requirements=false or better use the next-gen build instead."
        );
        if check_requirements {
            panic!("{msg}");
        }
        log::Warn(
            &format!("{msg} Skipping check due to --check-requirements=false."),
            [],
        );
    } else if IsNextGen() {
        // next-gen 必须提供 keyspace，否则后续 SST ingest 可能直接 panic。
        if keyspace_name.is_empty() {
            panic!(
                "next-gen restore requires keyspaceName; missing value may cause SST ingest panic."
            );
        }
        return true;
    }
    false
}
