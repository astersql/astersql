// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// MDL（Metadata Lock，元数据锁）检查所用的表集合状态。
//
// DDL 推进前需确认访问相关表的会话已切换到足够新的 schema 版本；
// 本结构缓存最近版本及待检查的 DDL 作业，供 Syncer 侧查询。

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;

/// One row from `mysql.tidb_mdl_info`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JobMDL {
    /// Schema version the job is waiting for.
    pub Ver: i64,
    /// Physical table IDs covered by the job.
    pub TableIDs: HashSet<i64>,
}

/// 参与 MDL 检查的 DDL 作业快照及最新 schema 版本。
#[derive(Debug, Default)]
pub struct mdlCheckTableInfo {
    mu: Mutex<mdlCheckTableInfoState>,
}

#[derive(Clone, Debug, Default)]
struct mdlCheckTableInfoState {
    newestVer: i64,
    jobs: HashMap<i64, JobMDL>,
}
impl mdlCheckTableInfo {
    /// Replace the snapshot read from `tidb_mdl_info`.
    pub fn replace(&self, newestVer: i64, jobs: HashMap<i64, JobMDL>) {
        let mut state = self.mu.lock().unwrap();
        state.newestVer = newestVer;
        state.jobs = jobs;
    }

    /// Return a consistent copy for the MDL checking loop.
    pub fn snapshot(&self) -> (i64, HashMap<i64, JobMDL>) {
        let state = self.mu.lock().unwrap();
        (state.newestVer, state.jobs.clone())
    }

    /// 判断给定表 ID 是否在任一待检查作业中。
    pub fn contains(&self, id: i64) -> bool {
        self.mu
            .lock()
            .unwrap()
            .jobs
            .values()
            .any(|job| job.TableIDs.contains(&id))
    }
}
/// 导出类型别名，与 Go 侧 `MDLCheckTableInfo` 命名对齐。
pub type MDLCheckTableInfo = mdlCheckTableInfo;
