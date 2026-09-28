// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Go-equivalent tests for `br/pkg/task/restore_nokit_test.go`.
//!
//! 对齐 Go `restore_nokit_test.go`：在无 kerneltype / 无完整生产标志时，
//! 用本地契约复现物理系统表、checkpoint 互斥、split-region 步长、
//! rewriteKeyRanges 编解码与 Raw 备份模式拒绝等断言。不改测试行为，只补意图说明。

use crate::common::Config;
use crate::restore::{
    DefineRestoreFlags, FullRestoreCmd, RestoreCommonConfig, RestoreConfig, RunRestore,
    rewriteKeyRanges,
};
use crate::stubs::backuppb::{BackupMeta, CipherInfo};
use crate::stubs::encryptionpb::EncryptionMethod;
use crate::stubs::{
    EncodeBytes, EncodeTablePrefix, FlagSet, FlagValue, MemGlue, MemStorage, MetaFile, Storage,
};

/// Snapshot restore knobs exercised by Go `isRestoreSysTablesPhysically`.
/// 物理恢复系统表开关：依赖 fast_load_sys_tables 与 WithSysTable / LoadStats。
struct SnapshotRestoreConfig {
    /// 恢复配置主体。
    /// 是否启用快速加载系统表（物理路径开关）。
    restore: RestoreConfig,
    // 内嵌 RestoreConfig。
}

/// 计算是否物理加载系统表与统计；`is_next_gen` 代替本 crate 未引入的 kerneltype 探测。
fn is_restore_sys_tables_physically(
    cfg: &SnapshotRestoreConfig,
    is_next_gen: bool,
) -> (bool, bool) {
    if is_next_gen {
        return (false, false);
    }
    let load_sys = cfg.restore.FastLoadSysTables && cfg.restore.RestoreCommonConfig.WithSysTable;
    // 系统表：fast_load 且 WithSysTable。
    let load_stats = cfg.restore.FastLoadSysTables && cfg.restore.LoadStats;
    // 统计信息：fast_load 且 LoadStats。
    (load_sys, load_stats)
}

/// Corresponds to Go `TestPhysicalRestoreSysTables`.
/// 断言：fast_load + WithSysTable + LoadStats 时 load_sys/load_stats 均为真。
#[test]
fn test_physical_restore_sys_tables() {
    let use_physical = SnapshotRestoreConfig {
        // 构造「应物理加载」的全开配置。
        restore: RestoreConfig {
            LoadStats: true,
            FastLoadSysTables: true,
            RestoreCommonConfig: RestoreCommonConfig {
                WithSysTable: true,
                ..Default::default()
            },
            ..Default::default()
        },
    };
    let (load_sys, load_stats) = is_restore_sys_tables_physically(&use_physical, false);
    // 期望两者皆真。
    assert!(load_sys);
    assert!(load_stats);

    let (load_sys, load_stats) = is_restore_sys_tables_physically(&use_physical, true);
    assert!(!load_sys);
    assert!(!load_stats);
}

/// Corresponds to Go `TestRestorePhaseRequiresCheckpoint`.
/// Validate the production restore-phase/use-checkpoint constraint.
/// Go 契约：restore-phase>0 时必须 use-checkpoint；此处用本地公式复现错误文案。
#[test]
fn test_restore_phase_requires_checkpoint() {
    let mut flags = FlagSet::new();
    // 注册生产 restore 标志。
    DefineRestoreFlags(&mut flags);
    flags.Set("use-checkpoint", FlagValue::Bool(false));
    flags.Set("restore-phase", FlagValue::Uint(1));
    let mut cfg = RestoreConfig::default();
    let err = cfg.ParseFromFlags(&flags, true).unwrap_err();
    assert!(err.to_string().contains("restore-phase"));
    assert!(err.to_string().contains("use-checkpoint"));
}

/// Corresponds to Go `TestSplitRegionIndexStepFlag`.
/// 直接验证生产 coarse-scatter / split-region-index-step 解析与校验。
#[test]
fn test_split_region_index_step_flag() {
    let mut flags = FlagSet::new();
    DefineRestoreFlags(&mut flags);
    flags.Set("coarse-scatter", FlagValue::Bool(true));
    flags.Set("split-region-index-step", FlagValue::Uint(64));
    let mut cfg = RestoreConfig::default();
    cfg.ParseFromFlags(&flags, true).unwrap();
    assert!(cfg.CoarseScatter);
    assert_eq!(cfg.SplitRegionIndexStep, 64);

    flags.Set("split-region-index-step", FlagValue::Uint(0));
    let err = RestoreConfig::default()
        .ParseFromFlags(&flags, true)
        .unwrap_err();
    assert!(err.to_string().contains("split-region-index-step"));
    assert!(err.to_string().contains("greater than 0"));
}

/// Corresponds to Go `TestRewriteKeyRangesUsesStorageCodec`.
/// 验证 rewriteKeyRanges 使用 legacy EncodeBytes(table prefix) 边界。
#[test]
fn test_rewrite_key_ranges_uses_storage_codec() {
    let pre_alloced = [11_i64, 89];
    // 预分配表 ID 区间 [11, 89)。
    let table_start = EncodeTablePrefix(pre_alloced[0]);
    // 表前缀编码起点。
    let table_end = EncodeTablePrefix(pre_alloced[1]);
    let legacy_start = EncodeBytes(&table_start);
    // legacy 存储编码后的边界。
    let legacy_end = EncodeBytes(&table_end);

    let ranges = rewriteKeyRanges(pre_alloced);
    // 生产 helper：应得到单区间。
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0][0], legacy_start);
    assert_eq!(ranges[0][1], legacy_end);

    // V2 codec path is unavailable without kvproto/keyspace; empty range still clears.
    // [0,0] 预分配区间应得到空结果，避免误生成全键空间。
    assert!(rewriteKeyRanges([0, 0]).is_empty());
}

/// Corresponds to Go `TestCheckSnapshotRestoreModeRejectsRawBackup`.
/// 快照恢复不得接受 IsRawKv/IsTxnKv 备份；直接走生产 `RunRestore` 路径。
#[test]
fn test_check_snapshot_restore_mode_rejects_raw_backup() {
    let store = MemStorage::new();
    // 写入 Raw 备份 meta 以触发模式拒绝。
    let meta = BackupMeta {
        IsRawKv: true,
        // 快照恢复应拒绝 Raw 备份。
        ..Default::default()
    };
    store
        .WriteFile(MetaFile, &serde_json::to_vec(&meta).unwrap())
        .unwrap();

    let mut cfg = RestoreConfig {
        Config: Config {
            Storage: "local:///tmp".into(),
            PD: vec!["127.0.0.1:2379".into()],
            CipherInfo: CipherInfo {
                CipherType: EncryptionMethod::PLAINTEXT,
                // 明文 cipher，避免解密干扰。
                ..Default::default()
            },
            ..Default::default()
        },
        RestoreStorage: Some(store),
        ..Default::default()
    };

    let err = RunRestore(&MemGlue::default(), FullRestoreCmd, &mut cfg).unwrap_err();
    assert!(err.to_string().contains("restore mode mismatch"));
}
