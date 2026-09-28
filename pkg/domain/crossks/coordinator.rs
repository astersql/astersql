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

// 跨 Keyspace（crossks）Schema 协调器。
//
// 负责登记内部 Session，并在 DDL（数据定义语言）推进时，通知各 Session
// 释放过期的 MDL（Metadata Lock，元数据锁）相关锁，避免旧事务长期占用元数据。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// 单个 DDL Job 对应的 MDL 版本信息。
#[derive(Clone, Debug, Default)]
pub struct JobMdl {
    /// 当前实例应已加载的 schema 版本号。
    pub version: i64,
    /// 该 Job 涉及的表；相关 Session 的 schema 版本必须不低于 `version`。
    pub table_ids: HashSet<i64>,
}
/// 可被协调器管理的内部 Session 抽象。
pub trait InternalSession: Send + Sync {
    /// 返回 Session 唯一标识。
    fn id(&self) -> u64;
    /// 根据当前 Job 集合移除本 Session 持有的过期 DDL 锁。
    fn remove_lock_ddl_jobs(&self, jobs: &HashMap<i64, JobMdl>, print_log: bool);
}

/// Schema 协调器：维护内部 Session 集合并驱动 MDL 清理。
pub struct SchemaCoordinator {
    /// 上次打印 MDL 相关日志的时间，用于限流。
    print_mdl_log_time: Mutex<Instant>,
    /// 已登记的内部 Session，按 id 索引。
    sessions: RwLock<HashMap<u64, Arc<dyn InternalSession>>>,
}
/// 创建空的 Schema 协调器实例。
pub fn new_schema_coordinator() -> SchemaCoordinator {
    SchemaCoordinator {
        print_mdl_log_time: Mutex::new(Instant::now()),
        sessions: RwLock::new(HashMap::new()),
    }
}
impl SchemaCoordinator {
    /// 登记一个内部 Session，供后续 MDL 检查使用。
    pub fn store_internal_session(&self, session: Arc<dyn InternalSession>) {
        self.sessions
            .write()
            .expect("coordinator lock poisoned")
            .insert(session.id(), session);
    }
    /// 按 id 移除已登记的内部 Session。
    pub fn delete_internal_session(&self, id: u64) {
        self.sessions
            .write()
            .expect("coordinator lock poisoned")
            .remove(&id);
    }
    /// 判断指定 id 的内部 Session 是否仍在登记表中。
    pub fn contains_internal_session(&self, id: u64) -> bool {
        self.sessions
            .read()
            .expect("coordinator lock poisoned")
            .contains_key(&id)
    }
    /// 返回当前登记的内部 Session 数量。
    pub fn internal_session_count(&self) -> usize {
        self.sessions
            .read()
            .expect("coordinator lock poisoned")
            .len()
    }
    /// 检查仍在运行的旧事务，并请求各 Session 释放过期 DDL 锁。
    ///
    /// 日志打印间隔至少 10 秒，避免高频刷屏；`print` 为 true 时要求 Session 输出日志。
    pub fn check_old_running_transaction(&self, jobs: &HashMap<i64, JobMdl>) {
        // 与 Go 一致，在遍历和回调期间持续持有 Session 读锁，阻止并发增删。
        let sessions = self.sessions.read().expect("coordinator lock poisoned");
        let print = {
            let mut print_mdl_log_time = self
                .print_mdl_log_time
                .lock()
                .expect("coordinator log time lock poisoned");
            let print = print_mdl_log_time.elapsed() > Duration::from_secs(10);
            if print {
                *print_mdl_log_time = Instant::now();
            }
            print
        };
        for session in sessions.values() {
            session.remove_lock_ddl_jobs(jobs, print);
        }
    }
    /// 杀死非 flashback 集群相关连接（当前为占位，对齐 Go 接口）。
    pub fn kill_non_flashback_cluster_connections(&self) {}
}
