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

//! Placement rule manager matching `placement_rule_manager.go`.
//! 在线恢复时把表范围钉到带 exclusive=restore 标签的 TiKV，避免与业务流量混部。
//! 离线模式或无可用 restore store 时退化为空操作管理器，保持调用方路径统一。
//! 规则 ID 形如 restore-t{table_id}，范围用 tablecodec 前缀编码后交给 SplitClient/PD。
//! loadRestoreStores 只收 Up+标签 store；TiFlash/Offline 即使带标签也排除。
//! setupPlacementRules 克隆 default 规则并提高 Index/Override，保证恢复约束优先生效。
//! checkRegions/checkRange 用 peer 所属 store 判定调度是否完成，未完成返回进度字符串。
//! Reset 时逐表 DeletePlacementRule，失败表号汇总后 Annotate，避免静默残留规则。
//! wait_ready 控制是否允许“已就绪短路”，便于单测与长轮询生产路径共存。
//! NewPlacementRuleManager 在线分支缺少 SplitClient 时显式报错，防止半初始化。
//! 分区表会把每个 Definition.ID 也纳入 restoreTables，与 Go 覆盖分区前缀一致。
//! LabelConstraints 使用 in + restoreLabelValue，限制副本只能落在 restore 节点。
//! StartKeyHex/EndKeyHex 经 bytes_to_hex，与 PD HTTP API 期望的 hex 字符串对齐。
//! ctx.Done 在等待阶段优先返回取消错误，避免恢复任务被放置等待拖死。
//! offlinePlacementRuleManager 与 online 共享 trait，调用方无需分支感知降级。
//! 本文件只管理 placement 生命周期，不负责实际 SST 导入或 checksum。
//! 日志点 Info/Warn 与 Go 对齐，便于线上对照恢复进度。

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::stubs::{
    Context, CreatedTable, Error, GetAllTiKVStoresWithRetry, PlacementRule, Result, SplitClient,
    StoreMeta, berrors, bytes_to_hex, codec, log, metapb, tablecodec,
};

/// 与 Go 常量一致：筛选 restore 专用 store 的 label key。
pub const restoreLabelKey: &str = "exclusive";
/// 与 Go 常量一致：label value 必须为 restore。
pub const restoreLabelValue: &str = "restore";

/// 放置规则生命周期：设置 → 等待调度就绪 → 恢复结束后重置。
pub trait PlacementRuleManager: Send {
    fn SetPlacementRule(&mut self, ctx: &Context, tables: &[CreatedTable]) -> Result<()>;
    fn ResetPlacementRules(&mut self, ctx: &Context) -> Result<()>;
}

/// 扫描 Up 状态且带 restore 标签的 store id 列表；Offline/TiFlash 等会被跳过。
pub fn loadRestoreStores(ctx: &Context, pd_client: &dyn StoreMeta) -> Result<Vec<u64>> {
    let mut restore_stores = Vec::new();
    let stores = GetAllTiKVStoresWithRetry(ctx, pd_client)?;
    for s in stores {
        if s.GetState() != metapb::StoreState::Up {
            // 非 Up 节点不能承接恢复流量。
            continue;
        }
        for l in s.GetLabels() {
            if l.GetKey() == restoreLabelKey && l.GetValue() == restoreLabelValue {
                restore_stores.push(s.GetId());
                break;
            }
        }
    }
    log::Info("load restore stores");
    Ok(restore_stores)
}

/// 工厂：离线或无 restore store → offline 管理器；否则要求 SplitClient 并启用在线规则。
pub fn NewPlacementRuleManager(
    ctx: &Context,
    pd_client: &dyn StoreMeta,
    tool_client: Option<Arc<dyn SplitClient>>,
    is_online: bool,
) -> Result<Box<dyn PlacementRuleManager>> {
    if !is_online {
        // 离线恢复不改写 PD placement，直接空实现。
        return Ok(Box::new(offlinePlacementRuleManager {}));
    }

    let restore_stores = loadRestoreStores(ctx, pd_client)?;
    if restore_stores.is_empty() {
        log::Warn(
            "The cluster has not any TiKV node with the specify label, so skip setting placement rules",
        );
        // 无标签节点时降级，避免误设全局约束。
        return Ok(Box::new(offlinePlacementRuleManager {}));
    }

    let tool = tool_client.ok_or_else(|| Error::new("split client required for online mode"))?;
    Ok(Box::new(onlinePlacementRuleManager {
        toolClient: tool,
        restoreStores: restore_stores,
        restoreTables: HashMap::new(),
        waitInterval: Duration::from_secs(10),
    }))
}

/// 空操作实现：Set/Reset 均成功返回，用于离线或无标签集群。
pub struct offlinePlacementRuleManager;

impl PlacementRuleManager for offlinePlacementRuleManager {
    fn SetPlacementRule(&mut self, _ctx: &Context, _tables: &[CreatedTable]) -> Result<()> {
        Ok(())
    }

    fn ResetPlacementRules(&mut self, _ctx: &Context) -> Result<()> {
        Ok(())
    }
}

/// 在线管理器：记录待恢复表/分区 ID，通过 SplitClient 读写 PD placement 规则。
pub struct onlinePlacementRuleManager {
    pub toolClient: Arc<dyn SplitClient>,
    pub restoreStores: Vec<u64>,
    pub restoreTables: HashMap<i64, ()>,
    /// Go checks placement on a ten-second ticker; tests can inject a shorter interval.
    pub waitInterval: Duration,
}

impl PlacementRuleManager for onlinePlacementRuleManager {
    fn SetPlacementRule(&mut self, ctx: &Context, tables: &[CreatedTable]) -> Result<()> {
        for tbl in tables {
            // 表与分区各自一条规则，覆盖完整 key 空间。
            self.restoreTables.insert(tbl.Table.ID, ());
            if let Some(part) = &tbl.Table.Partition {
                for def in &part.Definitions {
                    self.restoreTables.insert(def.ID, ());
                }
            }
        }

        self.setupPlacementRules(ctx)?;
        self.waitPlacementSchedule(ctx)?;
        Ok(())
    }

    fn ResetPlacementRules(&mut self, ctx: &Context) -> Result<()> {
        log::Info("start resetting placement rules");
        let mut failed_tables = Vec::new();
        for &table_id in self.restoreTables.keys() {
            if let Err(_err) = self
                .toolClient
                .DeletePlacementRule(ctx, "pd", &getRuleID(table_id))
            {
                // 单表删除失败先收集，最后统一 Annotate，避免半清理静默成功。
                log::Info("failed to delete placement rule for table");
                failed_tables.push(table_id);
            }
        }
        if !failed_tables.is_empty() {
            return Err(Error::Annotatef(
                berrors::ErrPDInvalidResponse("invalid response"),
                format!("failed to delete placement rules for tables {failed_tables:?}"),
            ));
        }
        Ok(())
    }
}

impl onlinePlacementRuleManager {
    /// 基于 default 规则克隆，覆盖 index/override，并按表前缀写入 Start/EndKeyHex。
    pub fn setupPlacementRules(&self, ctx: &Context) -> Result<()> {
        log::Info("start setting placement rules");
        let mut rule = self.toolClient.GetPlacementRule(ctx, "pd", "default")?;
        rule.Index = 100;
        rule.Override = true;
        rule.LabelConstraints.push(crate::stubs::LabelConstraint {
            Key: restoreLabelKey.to_string(),
            Op: "in".to_string(),
            Values: vec![restoreLabelValue.to_string()],
        });
        for &table_id in self.restoreTables.keys() {
            let mut rule = rule.clone();
            rule.ID = getRuleID(table_id);
            // EncodeBytes + EncodeTablePrefix 与 Go 编码路径一致，保证 PD 识别范围。
            rule.StartKeyHex = bytes_to_hex(&codec::EncodeBytes(
                Vec::new(),
                &tablecodec::EncodeTablePrefix(table_id),
            ));
            rule.EndKeyHex = bytes_to_hex(&codec::EncodeBytes(
                Vec::new(),
                &tablecodec::EncodeTablePrefix(table_id.wrapping_add(1)),
            ));
            self.toolClient.SetPlacementRule(ctx, &rule)?;
        }
        log::Info("finish setting placement rules");
        Ok(())
    }

    /// 逐表检查 region peer 是否都落在 restoreStores；未就绪返回进度文案。
    pub fn checkRegions(&self, ctx: &Context) -> Result<(bool, String)> {
        let mut progress = 0;
        let total = self.restoreTables.len();
        for &table_id in self.restoreTables.keys() {
            let start = codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(table_id));
            let end = codec::EncodeBytes(
                Vec::new(),
                &tablecodec::EncodeTablePrefix(table_id.wrapping_add(1)),
            );
            let (ok, region_progress) = self.checkRange(ctx, &start, &end)?;
            if !ok {
                return Ok((
                    false,
                    format!("table {progress}/{total}, {region_progress}"),
                ));
            }
            progress += 1;
        }
        Ok((true, String::new()))
    }

    /// 扫描 [start,end) 内所有 region，任一 peer 不在 restoreStores 即未就绪。
    pub fn checkRange(&self, ctx: &Context, start: &[u8], end: &[u8]) -> Result<(bool, String)> {
        let regions = self.toolClient.ScanRegions(ctx, start, end, -1)?;
        for (i, r) in regions.iter().enumerate() {
            for p in r.Region.GetPeers() {
                if !self.restoreStores.contains(&p.GetStoreId()) {
                    return Ok((false, format!("region {i}/{}", regions.len())));
                }
            }
        }
        Ok((true, String::new()))
    }

    /// 等待调度完成；未完成是瞬时状态，持续轮询直到就绪或上下文取消。
    pub fn waitPlacementSchedule(&self, ctx: &Context) -> Result<()> {
        log::Info("start waiting placement schedule");
        loop {
            if ctx.Done() {
                return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
            }

            let (ok, progress) = self.checkRegions(ctx)?;
            if ok {
                log::Info("finish waiting placement schedule");
                return Ok(());
            }
            log::Info(&format!("placement schedule progress: {progress}"));

            let started = Instant::now();
            while started.elapsed() < self.waitInterval {
                if ctx.Done() {
                    return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
                }
                let remaining = self.waitInterval.saturating_sub(started.elapsed());
                thread::sleep(remaining.min(Duration::from_millis(10)));
            }
        }
    }
}

/// PD 规则 ID：restore-t{table_id}，与 Go getRuleID 字符串格式一致。
pub fn getRuleID(table_id: i64) -> String {
    format!("restore-t{table_id}")
}
