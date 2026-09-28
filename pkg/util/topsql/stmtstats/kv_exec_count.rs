// Copyright 2021 PingCAP, Inc.
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

// KV 执行次数计数器：同一 SQL 执行内对每个 target 最多记一次。
//
// 用于 TopSQL 统计语句打到哪些 TiKV 等存储目标；对接 client-go 风格拦截链。

#![allow(non_snake_case)]

use std::collections::HashSet;
use std::sync::Mutex;

use crate::topsql_state;
use crate::{SQLPlanDigest, StatementStats};

impl StatementStats {
    /// 为指定 SQL/计划摘要创建绑定到本 StatementStats 的 KV 执行计数器。
    pub fn CreateKvExecCounter(&self, sql_digest: &[u8], plan_digest: &[u8]) -> KvExecCounter<'_> {
        KvExecCounter {
            stats: self,
            marked: Mutex::new(HashSet::new()),
            digest: SQLPlanDigest::new(sql_digest, plan_digest),
        }
    }
}

/// Counts at most one KV execution per target for a single SQL execution.
/// 单次 SQL 执行内，按 target 去重后累加 KV 执行次数。
pub struct KvExecCounter<'a> {
    /// 写入目标语句统计。
    stats: &'a StatementStats,
    /// 本执行已记过数的 target 集合。
    marked: Mutex<HashSet<String>>,
    /// SQL 与执行计划摘要键。
    digest: SQLPlanDigest,
}

impl KvExecCounter<'_> {
    /// Rust-native interceptor adapter. The next handler is always invoked and
    /// its result is returned unchanged, matching client-go's interceptor chain.
    /// 拦截适配：先记 target，再调用 next，原样返回结果。
    pub fn intercept<T, R, E>(
        &self,
        target: &str,
        request: T,
        next: impl FnOnce(&str, T) -> Result<R, E>,
    ) -> Result<R, E> {
        self.record_target(target);
        next(target, request)
    }

    /// TopSQL 开启时，对首次出现的 target 向统计写入 +1。
    pub fn record_target(&self, target: &str) {
        if !topsql_state::TopSQLEnabled() {
            return;
        }
        // insert 返回 true 表示该 target 首次出现。
        let first_mark = self
            .marked
            .lock()
            .expect("KvExecCounter mutex poisoned")
            .insert(target.to_owned());
        if first_mark {
            self.stats.add_kv_exec_count(
                self.digest.SQLDigest.as_bytes(),
                self.digest.PlanDigest.as_bytes(),
                target,
                1,
            );
        }
    }
}
