// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Checksum manager selection matching Go `checksum_helper.go`.
//!
//! 中文概览：这个文件负责在导入收尾阶段为表选择合适的远端 checksum 执行器。
//! 它要回答两个问题：当前是否需要做 checksum，以及应该走 TiKV 侧还是 TiDB SQL 侧执行。
//! `NewChecksumManager` 先根据 backend 和开关决定是否直接跳过，
//! 再根据 PD 主版本和 `ChecksumViaSQL` 选择 TiKV checksum manager 或 TiDB executor。
//! 这里之所以读取 PD 版本，是因为不同能力集对应不同的远端校验路径。
//! backoff weight 读取与兜底默认值则影响 TiKV 侧重试与限流行为。
//! `DoChecksum` 负责从 context 取出已经准备好的 manager，执行远端校验并上报耗时指标。
//! 也就是说，本文件更像“checksum 执行策略层”，而不是 checksum 算法本身。
//! 注释重点说明为何某些场景直接返回 `None`，以及为何 context 中必须提前挂好 manager。
//! 这能帮助维护者把“是否校验”和“如何校验”两个问题分开理解。

use crate::config;
use crate::context::Context;
use crate::errors::{self, Result};
use crate::import::Controller;
use crate::importdef;
use crate::ingestctrl::{self, ChecksumManager};
use crate::kv;
use crate::log;
use crate::logutil;
use crate::metric;
use crate::pdutil;
use crate::zap;
use std::sync::Arc;
use std::time::Duration;

pub const CHECKSUM_MANAGER_KEY: &str = "checksumManagerKey";
// context 键名固定后，controller 和执行阶段就能用同一约定传递 checksum manager。

/// NewChecksumManager creates a new checksum manager.
pub fn NewChecksumManager(
    ctx: Context,
    rc: &Controller,
    store: &kv::Storage,
) -> Result<Option<Arc<dyn ChecksumManager>>> {
    // TiDB backend 或显式关闭 checksum 时，直接返回空 manager，
    // 表达的是“此阶段无需远端校验”，而不是“构造失败”。
    if rc.cfg.TikvImporter.Backend == config::BackendTiDB
        || rc.cfg.PostRestore.Checksum == config::OpLevelOff
    {
        return Ok(None);
    }

    let pdVersion = pdutil::FetchPDVersion(ctx.clone(), rc.pdHTTPCli.clone().unwrap_or_default())
        .map_err(errors::Trace)?;

    let manager: Arc<dyn ChecksumManager> =
        if pdVersion.Major >= 4 && !rc.cfg.PostRestore.ChecksumViaSQL {
            // TiKV 校验路径会尝试读取 backoff weight，并在缺省时回退到默认值。
            let backoffWeight =
                match crate::common::GetBackoffWeightFromDB(ctx.clone(), rc.db.as_ref().unwrap()) {
                    Ok(w) if w >= ingestctrl::DefaultBackoffWeight => {
                        logutil::Logger(ctx.clone()).Info(
                            "get tidb_backoff_weight",
                            &[zap::Int("backoff_weight", w as i64)],
                        );
                        w
                    }
                    _ => {
                        logutil::Logger(ctx.clone()).Info(
                            "set tidb_backoff_weight to default",
                            &[zap::Int(
                                "backoff_weight",
                                ingestctrl::DefaultBackoffWeight as i64,
                            )],
                        );
                        ingestctrl::DefaultBackoffWeight
                    }
                };
            let _ = store.GetClient();
            Arc::new(ingestctrl::NewTiKVChecksumManager(
                (),
                (),
                rc.cfg.TiDB.DistSQLScanConcurrency as u32,
                backoffWeight,
                rc.resourceGroupName.clone(),
                rc.taskType.clone(),
            ))
        } else {
            // 能力不足或显式要求走 SQL 时，退回 TiDB checksum executor。
            Arc::new(ingestctrl::NewTiDBChecksumExecutor(
                rc.db.as_ref().unwrap().clone(),
            ))
        };

    Ok(Some(manager))
}

/// DoChecksum do checksum for tables.
pub fn DoChecksum(
    ctx: Context,
    table: &importdef::TableInfo,
) -> Result<ingestctrl::RemoteChecksum> {
    // 这里从 context 取 manager，强调执行阶段依赖的是已经选好的策略对象。
    // 如果上下文中没有 manager，说明调用链初始化顺序出了问题。
    let manager = ctx
        .Value(CHECKSUM_MANAGER_KEY)
        .and_then(|v| {
            v.downcast_ref::<ChecksumManagerHolder>()
                .map(|h| h.0.clone())
        })
        .ok_or_else(|| {
            errors::New("No gcLifeTimeManager found in context, check context initialization")
        })?;

    let task = log::Wrap(logutil::Logger(ctx.clone()).With(zap::String("table", &table.Name)))
        .Begin(zap::InfoLevel, "remote checksum");

    let result = manager.Checksum(ctx.clone(), table);
    // 无论成功还是失败，都会把耗时写入指标，便于观察远端校验成本。
    let err_ref = result.as_ref().err();
    let dur: Duration = task.End(zap::ErrorLevel, err_ref);
    if let Some(m) = metric::FromContext(ctx) {
        m.checksum_hist().Observe(dur.as_secs_f64());
    }
    result
}

pub struct ChecksumManagerHolder(pub Arc<dyn ChecksumManager>);
// 单独包一层 holder，方便以具体类型挂进 context 再安全取出。

pub fn WithChecksumManager(ctx: Context, mgr: Arc<dyn ChecksumManager>) -> Context {
    // 这个 helper 把“挑选策略”和“执行 checksum”解耦成上下文传递协议。
    crate::context::WithValue(
        ctx,
        CHECKSUM_MANAGER_KEY,
        Arc::new(ChecksumManagerHolder(mgr)),
    )
}
