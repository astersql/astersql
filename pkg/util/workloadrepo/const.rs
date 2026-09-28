// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载仓库（workload repository）常量。
//
// 定义 etcd 键、默认采样/快照/保留参数、schema/表名、系统变量名及错误文案，
// 供采样、快照与 housekeeper 共用。

/// etcd 上工作负载仓库 owner 竞选键。
pub const ownerKey: &str = "/tidb/workloadrepo/owner";
/// 日志/提示用短标识。
pub const promptKey: &str = "workloadrepo";
/// etcd 上当前快照 ID 键。
pub const snapIDKey: &str = "/tidb/workloadrepo/snap_id";
/// 快照写入失败时的最大重试次数。
pub const snapshotRetries: usize = 5;
/// 默认主动采样间隔（秒）。
pub const defSamplingInterval: i32 = 5;
/// 默认全量快照间隔（秒）。
pub const defSnapshotInterval: i32 = 3600;
/// 默认历史分区保留天数。
pub const defRententionDays: i32 = 7;
/// 历史快照元数据表名。
pub const histSnapshotsTable: &str = "HIST_SNAPSHOTS";
/// 工作负载仓库所在 schema 名。
pub const workloadSchema: &str = "WORKLOAD_SCHEMA";
/// 仓库目标库系统变量名。
pub const repositoryDest: &str = "tidb_workload_repository_dest";
/// 保留天数系统变量名。
pub const repositoryRetentionDays: &str = "tidb_workload_repository_retention_days";
/// 主动采样间隔系统变量名。
pub const repositorySamplingInterval: &str = "tidb_workload_repository_active_sampling_interval";
/// 快照间隔系统变量名。
pub const repositorySnapshotInterval: &str = "tidb_workload_repository_snapshot_interval";
/// etcd key 未找到时的错误文案。
pub const errKeyNotFound: &str = "key not found";
/// 仓库未启用时的错误文案。
pub const errWorkloadNotStarted: &str = "Workload repository is not enabled";
/// 缺少 etcd 客户端时的错误文案。
pub const errUnsupportedEtcdRequired: &str = "etcd client required for workload repository";
