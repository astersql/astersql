// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
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

// 本文件对应 Go `pd.go`：封装 PD HTTP/客户端，提供调度器暂停、配置 TTL、版本门控与 ResetTS 兼容。
// 网络边界抽象为 PdHttpClient/PdClient trait，便于无 gRPC 环境下做契约测试。
// 核心闭环是 pause → 后台按 TTL/3 刷新 → Resume/Close 唤醒并恢复。
// 按键范围暂停走 region label（schedule=deny），依赖 PD >= 6.1 的 label TTL。
// 配置键写入时统一加 `schedule.` 前缀，与 PD API 约定对齐。
//! PD controller matching `br/pkg/pdutil/pd.go` public contract.
//!
// 迁移约束：逻辑必须镜像 Go，但不得在本任务中改行为或补实现缺口。
//! Network/PD gRPC boundaries are abstracted behind traits so parity tests can
//! run without rebuilding grpcio-sys on arm64. Logic (pause/resume schedulers,
//! config TTL, version gates, ResetTS Forbidden handling, label-rule pause)
//! mirrors Go.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_errors::ErrPDUpdateFailed;
use astersql_errors::{Annotate, SharedError};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// Undo 闭包类型来自 utils，与暂停恢复链路衔接。
use crate::utils::{UndoFunc, nop_undo};

// 轻量 Context：仅提供取消标志，足够驱动后台循环退出。
/// Context stand-in for Go `context.Context`.
#[derive(Clone, Debug, Default)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
}

// Context 方法集：构造、取消查询与触发。
impl Context {
    // 默认构造，等价 Background。
    pub fn new() -> Self {
        Self::default()
    }
    // 对齐 Go context.Background：默认未取消。
    pub fn Background() -> Self {
        Self::default()
    }
    // 宽松原子读，供热路径轮询。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    // 协作式取消；后台 pause/label 循环轮询该标志。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed)
    }
}

// gRPC 消息上限占位，与 Go 常量同名便于对照。
pub const maxMsgSize: i32 = 128 * 1024 * 1024;
// 默认暂停 TTL：5 分钟；SchedulerPauseTTL 未设置时回落于此。
pub const pauseTimeout: Duration = Duration::from_secs(5 * 60);
// PD 请求重试次数常量（与 Go 对齐，供上层策略引用）。
pub const PDRequestRetryTime: i32 = 120;
// 暂停时把 max-pending-peer-count 抬到 MaxInt32，避免复制阻塞。
pub const maxPendingPeerUnlimited: u64 = i32::MAX as u64;

// 依据 store 数与当前值生成暂停期配置；闭包需 Send+Sync。
/// pauseConfigGenerator generate a config value according to store count and current value.
pub type PauseConfigGenerator = Arc<dyn Fn(i32, &Value) -> Value + Send + Sync>;

// 将配置压成 0（如 merge-schedule-limit）。
pub fn zeroPauseConfig(_stores: i32, _raw: &Value) -> Value {
    Value::from(0)
}

// raw*stores，上限 40，防止调度配额爆炸。
pub fn pauseConfigMulStores(stores: i32, raw: &Value) -> Value {
    let raw_cfg = raw.as_f64().unwrap_or(0.0);
    Value::from((40.0_f64).min(raw_cfg * stores as f64))
}

// 关闭布尔型调度开关；PD 侧常用字符串 "false"。
pub fn pauseConfigFalse(_stores: i32, _raw: &Value) -> Value {
    Value::String("false".to_string())
}

// 构造恒定值 generator，用于 max-pending-peer-count 等。
pub fn constConfigGeneratorBuilder(val: Value) -> PauseConfigGenerator {
    Arc::new(move |_stores: i32, _raw: &Value| val.clone())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
// 集群调度快照：原调度器列表、配置与可选 label rule id。
pub struct ClusterConfig {
    pub Schedulers: Vec<String>,
    pub ScheduleCfg: HashMap<String, Value>,
    pub RuleID: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
// PD 暂停调度器请求体字段（Delay 秒）。
pub struct pauseSchedulerBody {
    pub Delay: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
// 区域标签；TTL/StartAt 空时省略序列化。
pub struct RegionLabel {
    pub Key: String,
    pub Value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub TTL: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub StartAt: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
// PD region label rule；Data 承载 key-range JSON。
pub struct LabelRule {
    pub ID: String,
    pub Labels: Vec<RegionLabel>,
    pub RuleType: String,
    #[serde(default)]
    pub Data: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
// 键范围规则的起止 hex，写入 LabelRule.Data。
pub struct KeyRangeRule {
    #[serde(rename = "start_key")]
    pub StartKeyHex: String,
    #[serde(rename = "end_key")]
    pub EndKeyHex: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
// 批量设置/删除 label rules 的补丁结构。
pub struct LabelRulePatch {
    #[serde(default)]
    pub SetRules: Vec<LabelRule>,
    #[serde(default)]
    pub DeleteRules: Vec<String>,
}

// 支持 pause config 的最低 PD 版本：4.0.8。
pub fn pause_config_version() -> Version {
    Version::new(4, 0, 8)
}

// 支持按键范围 label TTL 的最低版本：6.1.0。
pub fn min_version_for_region_label_ttl() -> Version {
    Version::new(6, 1, 0)
}

// 影响备份/恢复性能的调度器白名单；RemoveSchedulers 只动这些。
/// Schedulers represent region/leader schedulers which can impact on performance.
pub fn Schedulers() -> HashSet<String> {
    [
        "balance-leader-scheduler",
        "balance-hot-region-scheduler",
        "balance-region-scheduler",
        "shuffle-leader-scheduler",
        "shuffle-region-scheduler",
        "shuffle-hot-region-scheduler",
        "evict-slow-store-scheduler",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

// 默认暂停配置生成表：merge=0、若干 limit=mulStores、location=false、pending=无限。
fn expect_pd_cfg_generators() -> HashMap<String, PauseConfigGenerator> {
    let mut m: HashMap<String, PauseConfigGenerator> = HashMap::new();
    m.insert("merge-schedule-limit".into(), Arc::new(zeroPauseConfig));
    m.insert(
        "leader-schedule-limit".into(),
        Arc::new(pauseConfigMulStores),
    );
    m.insert(
        "region-schedule-limit".into(),
        Arc::new(pauseConfigMulStores),
    );
    m.insert("max-snapshot-count".into(), Arc::new(pauseConfigMulStores));
    m.insert(
        "enable-location-replacement".into(),
        Arc::new(pauseConfigFalse),
    );
    m.insert(
        "max-pending-peer-count".into(),
        constConfigGeneratorBuilder(Value::from(maxPendingPeerUnlimited)),
    );
    m
}

// UpdatePDScheduleConfig 使用的默认目标配置。
pub fn default_pd_cfg() -> HashMap<String, Value> {
    HashMap::from([
        ("merge-schedule-limit".into(), Value::from(8)),
        ("leader-schedule-limit".into(), Value::from(4)),
        ("region-schedule-limit".into(), Value::from(2048)),
        (
            "enable-location-replacement".into(),
            Value::String("true".into()),
        ),
    ])
}

// 对外导出默认 generator，供测试与 RemoveAll 扩展。
/// DefaultExpectPDCfgGenerators returns default pd config generators.
pub fn DefaultExpectPDCfgGenerators() -> HashMap<String, PauseConfigGenerator> {
    expect_pd_cfg_generators()
}

// PD HTTP 最小面：版本、调度、配置、label、ResetTS、恢复标记等。
/// Minimal PD HTTP surface used by PdController (stand-in for pdhttp.Client).
pub trait PdHttpClient: Send + Sync {
    // 集群兼容版本字符串。
    fn GetClusterVersion(&self, ctx: &Context) -> Result<String, SharedError>;
    // PD binary 版本字符串。
    fn GetPDVersion(&self, ctx: &Context) -> Result<String, SharedError>;
    // 按编码后键范围统计 region 数。
    fn GetRegionCountByKeyRange(
        &self,
        ctx: &Context,
        start: &[u8],
        end: &[u8],
    ) -> Result<i32, SharedError>;
    // 按 id 取 store。
    fn GetStore(&self, ctx: &Context, store_id: u64) -> Result<StoreInfo, SharedError>;
    // 列出调度器。
    fn GetSchedulers(&self, ctx: &Context) -> Result<Vec<String>, SharedError>;
    // 拉取 schedule 配置。
    fn GetScheduleConfig(&self, ctx: &Context) -> Result<HashMap<String, Value>, SharedError>;
    // 可选 TTL 写入配置。
    fn SetConfig(
        &self,
        ctx: &Context,
        cfg: &HashMap<String, Value>,
        ttl_seconds: Option<f64>,
    ) -> Result<(), SharedError>;
    // 设置调度器暂停 delay（秒）。
    fn SetSchedulerDelay(&self, ctx: &Context, name: &str, delay: i64) -> Result<(), SharedError>;
    // 按 id 批量查询 label rules。
    fn GetRegionLabelRulesByIDs(
        &self,
        ctx: &Context,
        ids: &[String],
    ) -> Result<Vec<LabelRule>, SharedError>;
    // 批量增删 label rules。
    fn PatchRegionLabelRules(
        &self,
        ctx: &Context,
        patch: &LabelRulePatch,
    ) -> Result<(), SharedError>;
    // 创建或刷新单条 label rule。
    fn SetRegionLabelRule(&self, ctx: &Context, rule: &LabelRule) -> Result<(), SharedError>;
    // 可选 store 子集的最小 resolved ts。
    fn GetMinResolvedTSByStoresIDs(
        &self,
        ctx: &Context,
        store_ids: Option<&[u64]>,
    ) -> Result<u64, SharedError>;
    // 重置基础 alloc id。
    fn ResetBaseAllocID(&self, ctx: &Context, id: u64) -> Result<(), SharedError>;
    // 重置 PD 时间戳；老版本可能 Forbidden。
    fn ResetTS(&self, ctx: &Context, ts: u64, force_use_larger: bool) -> Result<(), SharedError>;
    // 设置快照恢复中标记。
    fn SetSnapshotRecoveringMark(&self, ctx: &Context) -> Result<(), SharedError>;
    // 删除快照恢复标记。
    fn DeleteSnapshotRecoveringMark(&self, ctx: &Context) -> Result<(), SharedError>;
    // 释放 HTTP 资源。
    fn Close(&self);
}

// PD client 最小面：列 store、follower handle、Close。
/// Minimal PD client surface (stand-in for pd.Client).
pub trait PdClient: Send + Sync {
    // 列出全部 store。
    fn GetAllStores(&self, ctx: &Context) -> Result<Vec<StoreInfo>, SharedError>;
    // 开关 follower handle。
    fn UpdateFollowerHandle(&self, enable: bool) -> Result<(), SharedError>;
    // 释放 PD client。
    fn Close(&self);
}

#[derive(Clone, Debug, Default)]
// store 摘要信息，满足暂停配置时的计数需求。
pub struct StoreInfo {
    pub id: u64,
    pub address: String,
}

// PD 控制器：持有 client/http、版本、暂停通道与 TTL。
/// PdController manage get/update config from pd.
pub struct PdController {
    // 可选 gRPC 客户端；缺省时 store 列表为空。
    pd_client: Mutex<Option<Arc<dyn PdClient>>>,
    // HTTP 客户端，承担暂停/配置/label 主要调用。
    pd_http: Arc<dyn PdHttpClient>,
    // 解析后的 PD 版本，用于能力门控。
    version: Version,
    // 暂停循环唤醒通道；Resume/Close 通过 send/drop 结束刷新。
    /// Mirrors Go `schedulerPauseCh`: buffered(1); send wakes pause loop; close on Close.
    scheduler_pause_ch: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    // 后台暂停刷新是否仍在跑。
    pause_loop_alive: AtomicBool,
    // Close 幂等门闩。
    closed: AtomicBool,
    // 可覆盖的暂停 TTL；零表示使用 pauseTimeout。
    pub SchedulerPauseTTL: Duration,
}

impl PdController {
    // 测试友好构造：注入 client/http/version，并预建 pause channel。
    /// NewPdControllerWithPDClient mirrors Go constructor used by tests.
    pub fn NewPdControllerWithPDClient(
        pd_client: Option<Arc<dyn PdClient>>,
        pd_http: Arc<dyn PdHttpClient>,
        version: Version,
    ) -> Self {
        // 初始 channel；真正 pause 时会替换为带接收端的新通道。
        let (tx, _rx) = std::sync::mpsc::channel();
        Self {
            pd_client: Mutex::new(pd_client),
            pd_http,
            version,
            scheduler_pause_ch: Mutex::new(Some(tx)),
            pause_loop_alive: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            SchedulerPauseTTL: Duration::ZERO,
        }
    }

    // 旧别名，行为与 NewPdControllerWithPDClient 相同。
    /// Alias kept for earlier call sites / parity.
    pub fn NewPdControllerWithClients(
        pd_client: Option<Arc<dyn PdClient>>,
        pd_http: Arc<dyn PdHttpClient>,
        version: Version,
    ) -> Self {
        Self::NewPdControllerWithPDClient(pd_client, pd_http, version)
    }

    // PD>=4.0.8 才允许 pause config；否则 doRemove 直接报错。
    pub fn isPauseConfigEnabled(&self) -> bool {
        self.version >= pause_config_version()
    }

    // PD>=6.1.0 才支持 region label TTL 暂停。
    pub fn CanPauseSchedulerByKeyRange(&self) -> bool {
        self.version >= min_version_for_region_label_ttl()
    }

    // 运行时替换/注入 PD client。
    pub fn SetPDClient(&self, pd_client: Arc<dyn PdClient>) {
        *self.pd_client.lock().expect("pd client lock") = Some(pd_client);
    }

    // 取出当前 PD client 快照。
    pub fn GetPDClient(&self) -> Option<Arc<dyn PdClient>> {
        self.pd_client.lock().expect("pd client lock").clone()
    }

    // 克隆 HTTP 客户端 Arc，供外部复用。
    pub fn GetPDHTTPClient(&self) -> Arc<dyn PdHttpClient> {
        Arc::clone(&self.pd_http)
    }

    // 透传集群版本查询。
    pub fn GetClusterVersion(&self, ctx: &Context) -> Result<String, SharedError> {
        self.pd_http.GetClusterVersion(ctx)
    }

    // 键范围区域计数：先 memcomparable 编码起止键。
    pub fn GetRegionCount(
        &self,
        ctx: &Context,
        start_key: &[u8],
        end_key: &[u8],
    ) -> Result<i32, SharedError> {
        let start = crate::utils::encode_bytes(start_key);
        // 空 end 表示开放右边界，不编码。
        let end = if end_key.is_empty() {
            Vec::new()
        } else {
            crate::utils::encode_bytes(end_key)
        };
        self.pd_http.GetRegionCountByKeyRange(ctx, &start, &end)
    }

    // 查询单个 store 信息。
    pub fn GetStoreInfo(&self, ctx: &Context, store_id: u64) -> Result<StoreInfo, SharedError> {
        self.pd_http.GetStore(ctx, store_id)
    }

    // 对每个调度器设置 delay=TTL，返回已暂停列表。
    fn doPauseSchedulers(
        &self,
        ctx: &Context,
        schedulers: &[String],
    ) -> Result<Vec<String>, SharedError> {
        // 暂停时长取自 ttlOfPausing。
        let delay = self.ttlOfPausing().as_secs() as i64;
        let mut removed = Vec::with_capacity(schedulers.len());
        for scheduler in schedulers {
            // 任一调度器失败则整体失败（已成功的不回滚，对齐 Go）。
            self.pd_http.SetSchedulerDelay(ctx, scheduler, delay)?;
            removed.push(scheduler.clone());
        }
        Ok(removed)
    }

    // 带 TTL 更新调度配置。
    fn doPauseConfigs(
        &self,
        ctx: &Context,
        cfg: &HashMap<String, Value>,
    ) -> Result<(), SharedError> {
        self.doUpdatePDScheduleConfig(ctx, cfg, Some(self.ttlOfPausing().as_secs_f64()))
    }

    // 先同步暂停成功，再拉起后台按 TTL/3 刷新，直到 Resume/Close。
    /// pauseSchedulersAndConfigWith mirrors Go: first pause must succeed; then
    /// spawn a background refresher until ResumeSchedulers / Close.
    pub fn pauseSchedulersAndConfigWith(
        &self,
        ctx: &Context,
        schedulers: &[String],
        scheduler_cfg: Option<&HashMap<String, Value>>,
    ) -> Result<Vec<String>, SharedError> {
        // 调度器暂停必须先成功，再处理配置与后台循环。
        let removed = self.doPauseSchedulers(ctx, schedulers)?;
        if let Some(cfg) = scheduler_cfg {
            // 配置暂停失败会让调用失败；此时后台循环尚未启动。
            self.doPauseConfigs(ctx, cfg)?;
        }

        // 换新 channel，确保 Resume 信号打到本轮循环。
        // Replace the pause channel so Resume can signal this loop.
        let (tx, rx) = std::sync::mpsc::channel();
        *self.scheduler_pause_ch.lock().expect("pause ch") = Some(tx);
        self.pause_loop_alive.store(true, Ordering::SeqCst);

        let http = Arc::clone(&self.pd_http);
        let schedulers = schedulers.to_vec();
        let cfg = scheduler_cfg.cloned();
        let ttl = self.ttlOfPausing();
        let ctx_bg = ctx.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let alive_flag = Arc::clone(&alive);
        // Keep a reference on the controller via pause_loop_alive.
        // 后台刷新：超时则重设 delay/config；收到信号或断开则退出。
        thread::spawn(move || {
            let tick = ttl / 3;
            loop {
                // 外部取消时结束刷新。
                if ctx_bg.is_cancelled() {
                    break;
                }
                // tick=TTL/3，与 Go 刷新节奏一致。
                match rx.recv_timeout(tick) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let _ = http_set_delays(&http, &ctx_bg, &schedulers, ttl.as_secs() as i64);
                        if let Some(ref c) = cfg {
                            let _ =
                                set_schedule_with_ttl(&http, &ctx_bg, c, Some(ttl.as_secs_f64()));
                        }
                    }
                }
            }
            // 标记循环结束（当前实现未再读取该局部 Arc）。
            alive_flag.store(false, Ordering::SeqCst);
        });
        let _ = alive;
        Ok(removed)
    }

    // 对外恢复入口，委托 resumeSchedulerWith。
    pub fn ResumeSchedulers(
        &self,
        ctx: &Context,
        schedulers: &[String],
    ) -> Result<(), SharedError> {
        self.resumeSchedulerWith(ctx, schedulers)
    }

    // 若规则仍存在则 PATCH 删除；查失败/空则静默返回。
    pub fn ResumeRegionLabelRule(&self, ctx: &Context, rule_id: &str) {
        // 空 id 表示未创建规则，直接返回。
        if rule_id.is_empty() {
            return;
        }
        let rule_ret = match self
            .pd_http
            .GetRegionLabelRulesByIDs(ctx, &[rule_id.to_string()])
        {
            Ok(r) => r,
            Err(_) => return,
        };
        // 规则已不存在，无需 PATCH。
        if rule_ret.is_empty() {
            return;
        }
        let delete = LabelRulePatch {
            DeleteRules: vec![rule_id.to_string()],
            ..Default::default()
        };
        let _ = self.pd_http.PatchRegionLabelRules(ctx, &delete);
    }

    // 唤醒 pause loop，并将 delay 置 0；仿 Go 不向上返回 delay 错误。
    fn resumeSchedulerWith(&self, ctx: &Context, schedulers: &[String]) -> Result<(), SharedError> {
        // 空列表：无操作成功。
        if schedulers.is_empty() {
            return Ok(());
        }
        // 发送停止信号；通道已关则忽略。
        // Signal pause loop to exit (non-blocking; channel is buffered in Go,
        // here we try_send / ignore if already closed).
        if let Some(tx) = self.scheduler_pause_ch.lock().expect("pause ch").as_ref() {
            let _ = tx.send(());
        }
        self.pause_loop_alive.store(false, Ordering::SeqCst);

        // delay=0：通知 PD 停止对该调度器的暂停。
        // 0 means stop pause.
        for scheduler in schedulers {
            let _ = self.pd_http.SetSchedulerDelay(ctx, scheduler, 0);
        }
        // Go never returns error here — pause will timeout.
        Ok(())
    }

    // 列出当前调度器名。
    pub fn ListSchedulers(&self, ctx: &Context) -> Result<Vec<String>, SharedError> {
        self.pd_http.GetSchedulers(ctx)
    }

    // 读取 PD schedule 配置 map。
    pub fn GetPDScheduleConfig(
        &self,
        ctx: &Context,
    ) -> Result<HashMap<String, Value>, SharedError> {
        self.pd_http.GetScheduleConfig(ctx)
    }

    // 无 TTL 地写入默认调度配置。
    pub fn UpdatePDScheduleConfig(&self, ctx: &Context) -> Result<(), SharedError> {
        self.doUpdatePDScheduleConfig(ctx, &default_pd_cfg(), None)
    }

    // 给键加 schedule. 前缀后 SetConfig；失败包装 ErrPDUpdateFailed。
    fn doUpdatePDScheduleConfig(
        &self,
        ctx: &Context,
        cfg: &HashMap<String, Value>,
        ttl_seconds: Option<f64>,
    ) -> Result<(), SharedError> {
        // PD API 要求 schedule. 前缀。
        let mut new_cfg = HashMap::new();
        for (k, v) in cfg {
            new_cfg.insert(format!("schedule.{k}"), v.clone());
        }
        self.pd_http
            .SetConfig(ctx, &new_cfg, ttl_seconds)
            // 统一注解为 ErrPDUpdateFailed，便于上层识别。
            .map_err(|e| {
                Annotate(
                    Some(SharedError::new((*ErrPDUpdateFailed).clone())),
                    format!("failed to update PD schedule config: {e}"),
                )
                .expect("annotate")
            })
    }

    // 基于快照生成默认恢复闭包。
    pub fn MakeUndoFunctionByConfig(&self, config: ClusterConfig) -> UndoFunc {
        self.GenRestoreSchedulerFunc(config, expect_pd_cfg_generators())
    }

    // 细粒度 undo：先执行额外钩子，再 restore_schedulers。
    pub fn MakeFineGrainedUndoFunction(
        &self,
        config: ClusterConfig,
        undo_extra: Arc<dyn Fn() + Send + Sync>,
    ) -> UndoFunc {
        let http = Arc::clone(&self.pd_http);
        let version = self.version.clone();
        let ttl = self.SchedulerPauseTTL;
        Arc::new(move |ctx| {
            // 先跑调用方额外清理，再恢复调度/配置。
            undo_extra();
            restore_schedulers(
                &ctx,
                Arc::clone(&http),
                &version,
                ttl,
                &config,
                &expect_pd_cfg_generators(),
            )
        })
    }

    // 捕获 http/version/ttl/config/generators，生成可调用 UndoFunc。
    pub fn GenRestoreSchedulerFunc(
        &self,
        config: ClusterConfig,
        configs_need_restore: HashMap<String, PauseConfigGenerator>,
    ) -> UndoFunc {
        let http = Arc::clone(&self.pd_http);
        let version = self.version.clone();
        let ttl = self.SchedulerPauseTTL;
        Arc::new(move |ctx| {
            restore_schedulers(
                &ctx,
                Arc::clone(&http),
                &version,
                ttl,
                &config,
                &configs_need_restore,
            )
        })
    }

    // 暂停影响性能的调度器并返回可回滚函数。
    pub fn RemoveSchedulers(&self, ctx: &Context) -> Result<UndoFunc, SharedError> {
        let (origin, _, err) = self.RemoveSchedulersWithOrigin(ctx);
        err?;
        Ok(self.MakeUndoFunctionByConfig(ClusterConfig {
            Schedulers: origin.Schedulers,
            ScheduleCfg: origin.ScheduleCfg,
            RuleID: String::new(),
        }))
    }

    // 同 RemoveSchedulers，额外返回原始 ClusterConfig。
    pub fn RemoveSchedulersWithConfig(
        &self,
        ctx: &Context,
    ) -> Result<(UndoFunc, ClusterConfig), SharedError> {
        let (origin, _, err) = self.RemoveSchedulersWithOrigin(ctx);
        err?;
        let undo = self.MakeUndoFunctionByConfig(ClusterConfig {
            Schedulers: origin.Schedulers.clone(),
            ScheduleCfg: origin.ScheduleCfg.clone(),
            RuleID: String::new(),
        });
        Ok((undo, origin))
    }

    // 更激进：把更多 schedule-limit 置 0，并关闭 tikv split region。
    pub fn RemoveAllPDSchedulers(&self, ctx: &Context) -> Result<UndoFunc, SharedError> {
        // 额外关闭 TiKV split，降低恢复期抖动。
        const ENABLE_TIKV_SPLIT_REGION: &str = "enable-tikv-split-region";
        let schedule_limit_params = [
            "hot-region-schedule-limit",
            "leader-schedule-limit",
            "merge-schedule-limit",
            "region-schedule-limit",
            "replica-schedule-limit",
            ENABLE_TIKV_SPLIT_REGION,
        ];
        let mut gens = DefaultExpectPDCfgGenerators();
        for param in schedule_limit_params {
            // split region 开关用 false；其余 limit 置 0。
            if param == ENABLE_TIKV_SPLIT_REGION {
                gens.insert(param.into(), Arc::new(|_, _| Value::Bool(false)));
            } else {
                gens.insert(param.into(), Arc::new(|_, _| Value::from(0)));
            }
        }
        let (old, _, err) = self.RemoveSchedulersWithConfigGenerator(ctx, &gens);
        err?;
        Ok(self.GenRestoreSchedulerFunc(old, gens))
    }

    // 使用默认 generator 执行暂停，返回 origin/removed/error 三元组。
    pub fn RemoveSchedulersWithOrigin(
        &self,
        ctx: &Context,
    ) -> (ClusterConfig, ClusterConfig, Result<(), SharedError>) {
        self.RemoveSchedulersWithConfigGenerator(ctx, &expect_pd_cfg_generators())
    }

    // 核心：读 store/config/schedulers → 生成禁用配置 → pause。
    pub fn RemoveSchedulersWithConfigGenerator(
        &self,
        ctx: &Context,
        pd_config_generators: &HashMap<String, PauseConfigGenerator>,
    ) -> (ClusterConfig, ClusterConfig, Result<(), SharedError>) {
        // 错误路径返回空配置占位。
        let empty = ClusterConfig::default();
        // 无 client 时 store_count=0，mulStores 仍可工作。
        let stores = match self.pd_client.lock().expect("lock").as_ref() {
            Some(cli) => match cli.GetAllStores(ctx) {
                Ok(s) => s,
                Err(e) => return (empty.clone(), empty, Err(e)),
            },
            None => Vec::new(),
        };
        // 读配置失败则无法生成禁用集。
        let schedule_cfg = match self.GetPDScheduleConfig(ctx) {
            Ok(c) => c,
            Err(e) => return (empty.clone(), empty, Err(e)),
        };
        let mut disable_pd_cfg = HashMap::new();
        let mut origin_pd_cfg = HashMap::new();
        let store_count = stores.len() as i32;
        // 只处理当前 schedule_cfg 中存在的键。
        for (cfg_key, cfg_val_func) in pd_config_generators {
            let Some(value) = schedule_cfg.get(cfg_key) else {
                continue;
            };
            disable_pd_cfg.insert(cfg_key.clone(), cfg_val_func(store_count, value));
            origin_pd_cfg.insert(cfg_key.clone(), value.clone());
        }
        let mut origin_cfg = ClusterConfig {
            ScheduleCfg: origin_pd_cfg,
            ..Default::default()
        };
        let mut removed_cfg = ClusterConfig {
            ScheduleCfg: disable_pd_cfg.clone(),
            ..Default::default()
        };

        // 列调度器失败时仍带回已算好的 origin/removed cfg。
        let exist = match self.ListSchedulers(ctx) {
            Ok(s) => s,
            Err(e) => return (origin_cfg, removed_cfg, Err(e)),
        };
        let impact = Schedulers();
        // 仅移除 impact 白名单中的调度器。
        let need_remove: Vec<String> = exist.into_iter().filter(|s| impact.contains(s)).collect();

        // 成功时把 removed 调度器名写回两边配置。
        match self.doRemoveSchedulersWith(ctx, &need_remove, &disable_pd_cfg) {
            Ok(removed) => {
                origin_cfg.Schedulers = removed.clone();
                removed_cfg.Schedulers = removed;
                (origin_cfg, removed_cfg, Ok(()))
            }
            Err(e) => (origin_cfg, removed_cfg, Err(e)),
        }
    }

    // 仅抓取当前 ScheduleCfg，不改动调度器。
    pub fn GetOriginPDConfig(&self, ctx: &Context) -> Result<ClusterConfig, SharedError> {
        let schedule_cfg = self.GetPDScheduleConfig(ctx)?;
        Ok(ClusterConfig {
            ScheduleCfg: schedule_cfg,
            ..Default::default()
        })
    }

    // 按给定 ClusterConfig 再次执行暂停（用于重放）。
    pub fn RemoveSchedulersWithCfg(
        &self,
        ctx: &Context,
        remove_cfg: &ClusterConfig,
    ) -> Result<(), SharedError> {
        // 忽略返回的 removed 列表，只关心错误。
        self.doRemoveSchedulersWith(ctx, &remove_cfg.Schedulers, &remove_cfg.ScheduleCfg)
            .map(|_| ())
    }

    // 版本门控后调用 pauseSchedulersAndConfigWith。
    fn doRemoveSchedulersWith(
        &self,
        ctx: &Context,
        need_remove: &[String],
        disable_pd_cfg: &HashMap<String, Value>,
    ) -> Result<Vec<String>, SharedError> {
        // 旧 PD：明确要求升级，避免半暂停状态。
        if !self.isPauseConfigEnabled() {
            return Err(astersql_errors::New(format!(
                "pd version {} not support pause config, please upgrade",
                self.version
            )));
        }
        // Go 始终传入已创建的 disablePDCfg；即使为空也要发送带 TTL 的 SetConfig。
        self.pauseSchedulersAndConfigWith(ctx, need_remove, Some(disable_pd_cfg))
    }

    // 全 store 最小 resolved ts。
    pub fn GetMinResolvedTS(&self, ctx: &Context) -> Result<u64, SharedError> {
        self.pd_http.GetMinResolvedTSByStoresIDs(ctx, None)
    }

    // 恢复基础分配 ID，用于快照恢复场景。
    pub fn RecoverBaseAllocID(&self, ctx: &Context, id: u64) -> Result<(), SharedError> {
        self.pd_http.ResetBaseAllocID(ctx, id)
    }

    // force_use_larger=true；Forbidden 视为老版本无此 API，忽略。
    pub fn ResetTS(&self, ctx: &Context, ts: u64) -> Result<(), SharedError> {
        // 始终 force_use_larger，对齐 Go 调用。
        match self.pd_http.ResetTS(ctx, ts, true) {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                // 对齐 Go：StatusForbidden 文本包含 Forbidden。
                if msg.contains("Forbidden") {
                    // http.StatusText(http.StatusForbidden)
                    return Ok(());
                }
                Err(e)
            }
        }
    }

    // 打上快照恢复中标记。
    pub fn MarkRecovering(&self, ctx: &Context) -> Result<(), SharedError> {
        self.pd_http.SetSnapshotRecoveringMark(ctx)
    }

    // 清除快照恢复标记。
    pub fn UnmarkRecovering(&self, ctx: &Context) -> Result<(), SharedError> {
        self.pd_http.DeleteSnapshotRecoveringMark(ctx)
    }

    // 按键范围暂停：返回 rule_id 与等待清理完成的闭包。
    /// RemoveSchedulersOnRegion pauses by key range; returns rule id and cleanup wait fn.
    pub fn RemoveSchedulersOnRegion(
        &self,
        ctx: &Context,
        key_range: &[[Vec<u8>; 2]],
    ) -> Result<(String, Arc<dyn Fn() + Send + Sync>), SharedError> {
        // 克隆 ctx，供后台与 wait 闭包共享取消状态。
        let scheduler_ctx = ctx.clone();
        let (done, rule_id) = pause_scheduler_by_key_range_with_ttl(
            &scheduler_ctx,
            Arc::clone(&self.pd_http),
            key_range,
            pauseTimeout,
        )?;
        // 短暂等待规则生效，降低竞态。
        // Go waits 20ms for the rule to take effect.
        thread::sleep(Duration::from_millis(20));
        let done = Mutex::new(done);
        // cancel + recv done，确保后台退出。
        let wait: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            scheduler_ctx.cancel();
            if let Some(rx) = done.lock().expect("done").take() {
                let _ = rx.recv();
            }
        });
        Ok((rule_id, wait))
    }

    // 幂等关闭：关 client/http，丢弃 pause channel。
    pub fn Close(&self) {
        // 已关闭则直接返回。
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(cli) = self.pd_client.lock().expect("lock").as_ref() {
            cli.Close();
        }
        self.pd_http.Close();
        // 置空 sender，令后台 recv 断开退出。
        // Drop sender to wake pause loop.
        *self.scheduler_pause_ch.lock().expect("pause ch") = None;
        self.pause_loop_alive.store(false, Ordering::SeqCst);
    }

    // 优先 SchedulerPauseTTL，否则 pauseTimeout。
    pub fn ttlOfPausing(&self) -> Duration {
        if self.SchedulerPauseTTL > Duration::ZERO {
            self.SchedulerPauseTTL
        } else {
            pauseTimeout
        }
    }

    // 未设置 PD client 时返回错误。
    pub fn SetFollowerHandle(&self, val: bool) -> Result<(), SharedError> {
        let guard = self.pd_client.lock().expect("pd client lock");
        // client 未注入时拒绝设置 follower handle。
        let Some(cli) = guard.as_ref() else {
            return Err(astersql_errors::New("pd client not set"));
        };
        cli.UpdateFollowerHandle(val)
    }
}

// 批量为调度器设置 delay，供后台刷新复用。
fn http_set_delays(
    http: &Arc<dyn PdHttpClient>,
    ctx: &Context,
    schedulers: &[String],
    delay: i64,
) -> Result<(), SharedError> {
    // 顺序设置；中途失败立即返回。
    for s in schedulers {
        http.SetSchedulerDelay(ctx, s, delay)?;
    }
    Ok(())
}

// 带可选 TTL 写入 schedule.* 配置。
fn set_schedule_with_ttl(
    http: &Arc<dyn PdHttpClient>,
    ctx: &Context,
    cfg: &HashMap<String, Value>,
    ttl: Option<f64>,
) -> Result<(), SharedError> {
    let mut new_cfg = HashMap::new();
    // 组装 schedule. 前缀映射。
    for (k, v) in cfg {
        new_cfg.insert(format!("schedule.{k}"), v.clone());
    }
    http.SetConfig(ctx, &new_cfg, ttl)
}

// undo 核心：delay=0、删 label、按版本决定 TTL=0 写回原配置。
fn restore_schedulers(
    ctx: &Context,
    http: Arc<dyn PdHttpClient>,
    version: &Version,
    scheduler_pause_ttl: Duration,
    cluster_cfg: &ClusterConfig,
    configs_need_restore: &HashMap<String, PauseConfigGenerator>,
) -> Result<(), SharedError> {
    // 先恢复调度器，再处理 RuleID 与配置。
    // Resume schedulers (delay=0).
    // 恢复阶段忽略单个 delay 错误。
    for s in &cluster_cfg.Schedulers {
        let _ = http.SetSchedulerDelay(ctx, s, 0);
    }
    // 存在按键范围规则时尝试删除。
    if !cluster_cfg.RuleID.is_empty() {
        let delete = LabelRulePatch {
            DeleteRules: vec![cluster_cfg.RuleID.clone()],
            ..Default::default()
        };
        let _ = http.PatchRegionLabelRules(ctx, &delete);
    }
    let mut merge_cfg = HashMap::new();
    // 仅恢复 generator 关注且 origin 中存在的键。
    for cfg_key in configs_need_restore.keys() {
        if let Some(value) = cluster_cfg.ScheduleCfg.get(cfg_key) {
            merge_cfg.insert(cfg_key.clone(), value.clone());
        }
    }
    // 新版本用 TTL=0 立即生效；旧版本不传 TTL。
    let ttl = if *version >= pause_config_version() {
        Some(0.0)
    } else {
        let _ = scheduler_pause_ttl;
        None
    };
    // Go 对非 nil 的空 map 也调用 SetConfig，保留该可观察副作用与错误传播。
    set_schedule_with_ttl(&http, ctx, &merge_cfg, ttl).map_err(|e| {
        let update_err = Annotate(
            Some(SharedError::new((*ErrPDUpdateFailed).clone())),
            format!("failed to update PD schedule config: {e}"),
        )
        .expect("annotate");
        Annotate(Some(update_err), "fail to update PD merge config").expect("annotate")
    })?;
    Ok(())
}

// 去空白/引号/v 前缀；解析失败回落 0.0.0。
/// parseVersion mirrors Go parseVersion (trim, strip v, fallback 0.0.0).
pub fn parseVersion(versionStr: &str) -> Version {
    // 去掉空白与 JSON 引号。
    let mut v = versionStr.trim().trim_matches('"').to_string();
    // 去掉常见 v 前缀。
    if let Some(stripped) = v.strip_prefix('v') {
        v = stripped.to_string();
    }
    // 解析失败不报错，回落 0.0.0 让门控走最保守路径。
    Version::parse(&v).unwrap_or_else(|_| Version::new(0, 0, 0))
}

// 从 HTTP 拉取版本串并 parseVersion。
/// FetchPDVersion get pd version
pub fn FetchPDVersion(ctx: &Context, pd_http: &dyn PdHttpClient) -> Result<Version, SharedError> {
    // HTTP 失败直接上抛。
    let ver = pd_http.GetPDVersion(ctx)?;
    Ok(parseVersion(&ver))
}

// 单范围便捷封装：内部 TTL=pauseTimeout，并 sleep 20ms。
/// PauseSchedulersByKeyRange will pause schedulers for regions in the key range.
pub fn PauseSchedulersByKeyRange(
    ctx: &Context,
    pd_http: Arc<dyn PdHttpClient>,
    start_key: &[u8],
    end_key: &[u8],
) -> Result<Option<std::sync::mpsc::Receiver<()>>, SharedError> {
    // 包装为单元素 key_range。
    let ranges = [[start_key.to_vec(), end_key.to_vec()]];
    let (done, _) = pause_scheduler_by_key_range_with_ttl(ctx, pd_http, &ranges, pauseTimeout)?;
    thread::sleep(Duration::from_millis(20));
    Ok(done)
}

// 创建 schedule=deny 的 key-range label，并后台按 TTL/3 刷新直至取消。
/// pauseSchedulerByKeyRangeWithTTL mirrors Go helper (Arc HTTP client for background refresh).
pub fn pause_scheduler_by_key_range_with_ttl(
    ctx: &Context,
    pd_http: Arc<dyn PdHttpClient>,
    key_range: &[[Vec<u8>; 2]],
    ttl: Duration,
) -> Result<(Option<std::sync::mpsc::Receiver<()>>, String), SharedError> {
    // 空范围或首 start 为空：不建规则，返回空 id。
    // Go: no table to restore → empty rule when len==0 || keyRange[0][0] == nil.
    if key_range.is_empty() || key_range[0][0].is_empty() {
        return Ok((None, String::new()));
    }
    // 将原始字节对编码为 hex KeyRangeRule。
    let mut encoded = Vec::new();
    for pair in key_range {
        encoded.push(KeyRangeRule {
            StartKeyHex: hex::encode(&pair[0]),
            EndKeyHex: hex::encode(&pair[1]),
        });
    }
    // 随机 rule id，避免与现有规则冲突。
    let rule_id = Uuid::new_v4().to_string();
    // Labels 固定 schedule/deny，TTL 用 Go Duration 字符串格式。
    let rule = LabelRule {
        ID: rule_id.clone(),
        Labels: vec![RegionLabel {
            Key: "schedule".into(),
            Value: "deny".into(),
            TTL: format_duration(ttl),
            StartAt: String::new(),
        }],
        RuleType: "key-range".into(),
        Data: serde_json::to_value(&encoded)
            .map_err(|e| astersql_errors::New(format!("encode key range: {e}")))?,
    };

    // done 用于调用方等待后台清理结束。
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    // 先同步创建规则，失败则不启动后台。
    pd_http.SetRegionLabelRule(ctx, &rule)?;

    let ctx_bg = ctx.clone();
    let http = Arc::clone(&pd_http);
    // 刷新循环：取消后将 TTL 置 0 并 PATCH 删除，再通知 done。
    thread::spawn(move || {
        // 刷新周期为 TTL 的三分之一。
        let tick = ttl / 3;
        let mut rule = rule;
        loop {
            if ctx_bg.is_cancelled() {
                break;
            }
            // sleep 后再检查取消，减少无效刷新。
            thread::sleep(tick);
            if ctx_bg.is_cancelled() {
                break;
            }
            // 上下文取消类错误直接退出循环。
            if let Err(e) = http.SetRegionLabelRule(&ctx_bg, &rule) {
                if e.to_string().contains("canceled") {
                    break;
                }
            }
        }
        // 清理：TTL=0 + DeleteRules，使用 Background ctx 避免取消影响。
        rule.Labels[0].TTL = format_duration(Duration::ZERO);
        let delete = LabelRulePatch {
            DeleteRules: vec![rule.ID.clone()],
            ..Default::default()
        };
        // 清理不走已取消 ctx，确保 PATCH 能发出。
        let recover_ctx = Context::Background();
        let _ = http.PatchRegionLabelRules(&recover_ctx, &delete);
        // 通知等待方后台已结束。
        let _ = done_tx.send(());
    });

    Ok((Some(done_rx), rule_id))
}

// 对齐 Go time.Duration.String：保留纳秒精度并按 ns/µs/ms/s 选择单位。
pub(crate) fn format_duration(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos == 0 {
        return "0s".into();
    }

    fn decimal(value: u128, scale: u128, suffix: &str) -> String {
        let whole = value / scale;
        let remainder = value % scale;
        if remainder == 0 {
            return format!("{whole}{suffix}");
        }
        let width = scale.ilog10() as usize;
        let fraction = format!("{remainder:0width$}")
            .trim_end_matches('0')
            .to_string();
        format!("{whole}.{fraction}{suffix}")
    }

    if nanos < 1_000 {
        return format!("{nanos}ns");
    }
    if nanos < 1_000_000 {
        return decimal(nanos, 1_000, "µs");
    }
    if nanos < 1_000_000_000 {
        return decimal(nanos, 1_000_000, "ms");
    }

    let total_secs = nanos / 1_000_000_000;
    let hours = total_secs / 3_600;
    let minutes = (total_secs % 3_600) / 60;
    let second_nanos = nanos % (60 * 1_000_000_000);
    let seconds = decimal(second_nanos, 1_000_000_000, "s");
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}")
    } else {
        seconds
    }
}
