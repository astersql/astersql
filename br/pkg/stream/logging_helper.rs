// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! 流备份库表替换映射的可读日志辅助。
//! 对应 Go `LogDBReplaceMap`（zap 字段）；Rust 侧改为可选收集字符串行，便于单测断言。

use std::collections::HashMap;

use crate::stubs::{DBReplace, UpstreamID};

/// Collect log lines instead of using zap; no-op when `out` is None.
/// 遍历未过滤的库/表/分区映射，拼成一行；`out` 为 None 时等同静默（对齐生产只打日志）。
pub fn LogDBReplaceMap(
    title: &str,
    dbReplaces: &HashMap<UpstreamID, DBReplace>,
    mut out: Option<&mut Vec<String>>,
) {
    for (upstream_db_id, db_replace) in dbReplaces {
        // FilteredOut 的库不出现在恢复映射日志中。
        if db_replace.FilteredOut {
            continue;
        }
        let mut line = format!(
            "{title} dbName={} upstreamId={upstream_db_id} downstreamId={}",
            db_replace.Name, db_replace.DbID
        );
        for (upstream_table_id, table_replace) in &db_replace.TableMap {
            // 被过滤的表不进入恢复映射说明，避免噪音日志。
            if table_replace.FilteredOut {
                continue;
            }
            line.push_str(&format!(
                " table={} upstreamId={upstream_table_id} downstreamId={}",
                table_replace.Name, table_replace.TableID
            ));
            // 分区 ID 成对追加，便于对照上下游分区拓扑。
            for (up_part_id, down_part_id) in &table_replace.PartitionMap {
                line.push_str(&format!(
                    " up partition={up_part_id} down partition={down_part_id}"
                ));
            }
        }
        // 有收集器则追加行；无收集器时行构造仍执行，便于日后接真实 logger。
        if let Some(out) = out.as_mut() {
            out.push(line);
        }
    }
}
