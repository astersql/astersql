// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc. Licensed under Apache-2.0.

//! Operator command configs — mirrors `br/pkg/task/operator/config.go`.
//! 集中定义 operator 子命令的 flag 名、配置结构体与 ParseFromFlags。
//! 各 Config 与 Go 同名字段对齐；校验逻辑（互斥 flag、必填项）不得弱化。
//! CLI 层只调用 DefineFlags* / ParseFromFlags，不在此处执行业务 IO。
//! Rewrite 与 Upstream 的 flag 文案刻意相近但入口分离。
//! 下游存在性检查开关改变同步判定实现。

use std::time::Duration;

use regex::Regex;

use crate::stubs::{
    BackendOptions, CRRServiceConfig, Config, DefaultSchemaConcurrency, DefineBackendFlags,
    DefineCRRFlags, Error, FlagSet, RestoreConfig, Result, berrors,
};

// —— 与 Go const flag* 一一对应；改名会破坏 CLI 兼容与测试夹具 ——
pub const flagTableConcurrency: &str = "table-concurrency";
pub const flagRestoredTS: &str = "restored-ts";
pub const flagUpstreamClusterID: &str = "upstream-cluster-id";
pub const flagChecksumTS: &str = "checksum-ts";
pub const flagStorePatterns: &str = "stores";
pub const flagTaskName: &str = "task-name";
pub const flagUpstreamStorage: &str = "upstream-storage";
pub const flagDownstreamStorage: &str = "downstream-storage";
pub const flagCheckSyncedFromDownstreamStorage: &str = "check-synced-from-downstream-storage";
pub const flagTTL: &str = "ttl";
pub const flagSafePoint: &str = "safepoint";
pub const flagStorage: &str = "storage";
pub const flagLoadCreds: &str = "load-creds";
pub const flagJSON: &str = "json";
pub const flagRecent: &str = "recent";
pub const flagTo: &str = "to";
pub const flagBase: &str = "base";
pub const flagYes: &str = "yes";
pub const flagDryRun: &str = "dry-run";

/// prepare-snap / pause-GC：在 PD 上维持 GC safepoint，防止快照准备期间被回收。
pub struct PauseGcConfig {
    pub Config: Config,
    pub SafePoint: u64,
    /// SafePointID is used to identify a specific safepoint.
    /// This field is only used in ***TEST*** now, you shouldn't use it in the src codes.
    /// 仅测试注入 safepoint 标识；生产路径必须留空，避免误删他人 safepoint。
    pub SafePointID: String,
    pub TTL: Duration,
    /// 全部 store 就绪回调；生产可选，测试用于同步栅栏。
    pub OnAllReady: Option<Box<dyn Fn() + Send + Sync>>,
    /// 退出前回调，便于测试断言资源释放顺序。
    pub OnExit: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Default for PauseGcConfig {
    fn default() -> Self {
        Self {
            Config: Config::default(),
            SafePoint: 0,
            SafePointID: String::new(),
            // Go 默认 120s TTL，与 PD service safepoint 常见窗口一致。
            TTL: Duration::from_secs(120),
            OnAllReady: None,
            OnExit: None,
        }
    }
}

/// 注册 prepare-snap 的 TTL / safepoint 短选项（`-i` / `-t`）。
pub fn DefineFlagsForPrepareSnapBackup(f: &mut FlagSet) {
    f.DurationP(
        flagTTL,
        "i",
        Duration::from_secs(2 * 60),
        "The time-to-live of the safepoint.",
    );
    f.Uint64P(flagSafePoint, "t", 0, "The GC safepoint to be kept.");
}

impl PauseGcConfig {
    /// 先解析公共 Config，再覆写 SafePoint/TTL。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.Config.ParseFromFlags(flags)?;
        self.SafePoint = flags.GetUint64(flagSafePoint)?;
        self.TTL = flags.GetDuration(flagTTL)?;
        Ok(())
    }
}

/// Base64ify：外部存储 URI + 是否把环境凭证一并编码。
#[derive(Clone, Debug, Default)]
pub struct Base64ifyConfig {
    pub BackendOptions: BackendOptions,
    pub StorageURI: String,
    /// 对应 Go `LoadCerd`（历史拼写保留），真则 SendCredentials。
    pub LoadCerd: bool,
}

pub fn DefineFlagsForBase64ifyConfig(flags: &mut FlagSet) {
    DefineBackendFlags(flags);
    flags.StringP(flagStorage, "s", "", "The external storage input.");
    flags.Bool(
        flagLoadCreds,
        false,
        "whether loading the credientials from current environment and marshal them to the base64 string. [!]",
    );
}

impl Base64ifyConfig {
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.BackendOptions.ParseFromFlags(flags)?;
        self.StorageURI = flags.GetString(flagStorage)?;
        self.LoadCerd = flags.GetBool(flagLoadCreds)?;
        Ok(())
    }
}

/// list-migration：列出外部存储中的迁移记录，可选 JSON 输出。
#[derive(Clone, Debug, Default)]
// LoadCerd 拼写保留以对齐 Go 字段名。
pub struct ListMigrationConfig {
    pub BackendOptions: BackendOptions,
    pub StorageURI: String,
    pub JSONOutput: bool,
}

// JSONOutput 仅影响展示，不改变读取逻辑。
pub fn DefineFlagsForListMigrationConfig(flags: &mut FlagSet) {
    DefineBackendFlags(flags);
    flags.StringP(flagStorage, "s", "", "the external storage input.");
    flags.Bool(flagJSON, false, "output the result in json format.");
}

// MigrateTo 序号零表示未指定目标。
impl ListMigrationConfig {
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.BackendOptions.ParseFromFlags(flags)?;
        self.StorageURI = flags.GetString(flagStorage)?;
        self.JSONOutput = flags.GetBool(flagJSON)?;
        Ok(())
    }
}

/// migrate-to：在 BASE 与指定序号之间执行/演练迁移合并。
#[derive(Clone, Debug, Default)]
// 表并发默认值取自 DefaultSchemaConcurrency。
pub struct MigrateToConfig {
    pub BackendOptions: BackendOptions,
    pub StorageURI: String,
    /// 迁到最新 migration 并合并 BASE；与 `--to` 互斥。
    pub Recent: bool,
    /// 目标序号；0 表示未指定。
    pub MigrateTo: i32,
    /// 只重试 BASE 挂起操作，不合并后续 migration。
    pub Base: bool,
    /// 跳过影响评估与确认，直接执行。
    pub Yes: bool,
    /// 只打印效果不落盘。
    pub DryRun: bool,
}

// 公共 Config 解析委托给 stubs 中的实现。
pub fn DefineFlagsForMigrateToConfig(flags: &mut FlagSet) {
    DefineBackendFlags(flags);
    flags.StringP(flagStorage, "s", "", "the external storage input.");
    flags.Bool(
        flagRecent,
        true,
        "migrate to the most recent migration and BASE.",
    );
    flags.Int(
        flagTo,
        0,
        "migrate all migrations from the BASE to the specified sequence number.",
    );
    flags.Bool(
        flagBase,
        false,
        "don't merge any migrations, just retry run pending operations in BASE.",
    );
    flags.BoolP(
        flagYes,
        "y",
        false,
        "skip all effect estimating and confirming. execute directly.",
    );
    flags.Bool(
        flagDryRun,
        false,
        "do not actually perform the migration, just print the effect.",
    );
}

// 存储 URI 空串表示未配置，由各命令自行拒绝。
impl MigrateToConfig {
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.BackendOptions.ParseFromFlags(flags)?;
        self.StorageURI = flags.GetString(flagStorage)?;
        self.Recent = flags.GetBool(flagRecent)?;
        self.MigrateTo = flags.GetInt(flagTo)?;
        self.Base = flags.GetBool(flagBase)?;
        self.Yes = flags.GetBool(flagYes)?;
        self.DryRun = flags.GetBool(flagDryRun)?;
        Ok(())
    }

    /// 互斥约束与 Go `Verify` 相同：Recent↔To、Base↔(Recent|To)。
    pub fn Verify(&self) -> Result<()> {
        // 上游集群 ID 仅 PiTR checksum 需要。
        if self.Recent && self.MigrateTo != 0 {
            // SafePointID 生产路径必须留空。
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!(
                    "the --{} and --{} flag cannot be used at the same time",
                    flagRecent, flagTo
                ),
            ));
        }
        // PrepareSnap 默认 TTL 为两分钟。
        if self.Base && (self.Recent || self.MigrateTo != 0) {
            // Yes 跳过确认，仍受 DryRun 约束。
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!(
                    "the --{} and ( --{} or --{} ) flag cannot be used at the same time",
                    flagBase, flagTo, flagRecent
                ),
            ));
        }
        Ok(())
    }
}

/// force-flush：公共 Config + 匹配 TiKV 地址的正则。
pub struct ForceFlushConfig {
    pub Config: Config,
    /// StoresPattern matches the address of TiKV.
    /// The address usually looks like "<host>:20160".
    /// 默认 `.*` 匹配全部；非法正则在 ParseFromFlags 阶段失败。
    pub StoresPattern: Regex,
}

// ListMigration 的 JSON 输出开关独立于业务路径。
impl Default for ForceFlushConfig {
    // ChecksumTS 为零表示运行时取 PD 当前时间戳。
    fn default() -> Self {
        Self {
            Config: Config::default(),
            StoresPattern: Regex::new(".*").unwrap(),
        }
    }
}

// ForceFlush 默认匹配全部 TiKV 地址。
pub fn DefineFlagsForForceFlushConfig(f: &mut FlagSet) {
    f.String(
        flagStorePatterns,
        ".*",
        "The regexp to match the store peer address to be force flushed.",
    );
}

// Recent 与 To 互斥，Base 与二者也互斥。
impl ForceFlushConfig {
    /// 先编译 stores 正则，再解析公共 Config，保证坏正则尽早报错。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        let storePat = flags.GetString(flagStorePatterns)?;
        self.StoresPattern = Regex::new(&storePat).map_err(|err| {
            Error::Annotatef(
                err.to_string(),
                format!("invalid expression in --{flagStorePatterns}"),
            )
        })?;
        self.Config.ParseFromFlags(flags)
    }
}

/// CRR checkpoint：上下游日志备份存储 URI + CRR 轮询参数。
#[derive(Clone, Debug, Default)]
// TTL 与 SafePoint 短选项与 Go cobra 短名一致。
pub struct CRRCheckpointConfig {
    pub Config: Config,
    pub CRRConfig: CRRServiceConfig,
    pub UpstreamStorage: String,
    pub DownstreamStorage: String,
    /// 为真时用下游对象存在性判断同步，而非上游原生 sync API。
    pub CheckSyncedFromDownstreamStorage: bool,
}

// checksum 三类 DefineFlags 刻意拆分，避免无关 flag 污染。
pub fn DefineFlagsForCRRCheckpointConfig(flags: &mut FlagSet) {
    DefineCRRFlags(flags);
    flags.String(
        flagUpstreamStorage,
        "",
        "The upstream log backup storage URI.",
    );
    flags.String(
        flagDownstreamStorage,
        "",
        "The downstream replicated log backup storage URI.",
    );
    flags.Bool(
        flagCheckSyncedFromDownstreamStorage,
        false,
        "Check object sync by file existence on downstream storage.",
    );
}

// Base64ify 的 LoadCerd 为危险选项，CLI 有感叹号提示。
impl CRRCheckpointConfig {
    /// 解析后强制校验 task-name / upstream / downstream 非空（与 Go 一致）。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.Config.ParseFromFlags(flags)?;
        self.CRRConfig.Parse(flags)?;

        self.UpstreamStorage = flags.GetString(flagUpstreamStorage)?;
        self.DownstreamStorage = flags.GetString(flagDownstreamStorage)?;
        self.CheckSyncedFromDownstreamStorage =
            flags.GetBool(flagCheckSyncedFromDownstreamStorage)?;

        // migrate-to 的 DryRun 不得改变存储内容。
        if self.CRRConfig.TaskName.is_empty() {
            // PauseGc 的回调仅测试注入，生产保持 None。
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!("missing required flag --{flagTaskName}"),
            ));
        }
        // CRR 必填项缺失时错误消息含完整 flag 名。
        if self.UpstreamStorage.is_empty() {
            // 正则 StoresPattern 编译失败要带 flag 名上下文。
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!("missing required flag --{flagUpstreamStorage}"),
            ));
        }
        // RestoreConfig 双写 TableConcurrency 防止内外层不一致。
        if self.DownstreamStorage.is_empty() {
            // 互斥校验集中在 Verify 或 Parse 末尾，便于测试。
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!("missing required flag --{flagDownstreamStorage}"),
            ));
        }
        Ok(())
    }
}

/// checksum（rewrite-rules 路径）：复用公共 Config，表并发由专用 flag 填入。
#[derive(Clone, Debug, Default)]
// BackendOptions 复用公共后端 flag，避免重复定义。
pub struct ChecksumWithRewriteRulesConfig {
    pub Config: Config,
}

/// 注册 rewrite-rules checksum 的表并发与 restored-ts。
pub fn DefineFlagsForChecksumTableConfig(f: &mut FlagSet) {
    f.Uint(
        flagTableConcurrency,
        DefaultSchemaConcurrency,
        "The size of a BR thread pool used for backup table metas, including tableInfo/checksum and stats.",
    );
    f.Uint64(flagRestoredTS, 0, "The point time to checksum");
}

/// upstream checksum：flag 集合与 rewrite-rules 类似，语义指向上游对照。
pub fn DefineFlagsForChecksumUpstreamTableConfig(f: &mut FlagSet) {
    f.Uint(
        flagTableConcurrency,
        DefaultSchemaConcurrency,
        "The size of a BR thread pool used for backup table metas, including tableInfo/checksum and stats.",
    );
    f.Uint64(flagRestoredTS, 0, "The point time to checksum");
}

/// PiTR id-map checksum：额外要求 upstream-cluster-id 与可选 checksum-ts。
pub fn DefineFlagsForChecksumPitrTableConfig(f: &mut FlagSet) {
    f.Uint(
        flagTableConcurrency,
        DefaultSchemaConcurrency,
        "The size of a BR thread pool used for backup table metas, including tableInfo/checksum and stats.",
    );
    f.Uint64(flagRestoredTS, 0, "The restore point time");
    f.Uint64(
        flagUpstreamClusterID,
        0,
        "The upstream cluster id of used pitr id map",
    );
    f.Uint64(
        flagChecksumTS,
        0,
        "The checksum time (use current pd tso if not specified)",
    );
}

// DefineFlags 可被 CLI 与单测重复调用注册。
impl ChecksumWithRewriteRulesConfig {
    /// TableConcurrency 写入 Config，再解析其余公共 flag。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.Config.TableConcurrency = flags.GetUint(flagTableConcurrency)?;
        self.Config.ParseFromFlags(flags)
    }
}

/// PiTR 路径配置：挂在 RestoreConfig 上，并单独保留 ChecksumTS。
#[derive(Clone, Debug, Default)]
// Default 值与 Go zero-value/显式默认对齐。
pub struct ChecksumWithPitrIdMapConfig {
    pub RestoreConfig: RestoreConfig,
    /// 0 表示运行时向 PD 取当前 TSO。
    pub ChecksumTS: u64,
}

// ParseFromFlags 失败应返回 ErrInvalidArgument 语义。
impl ChecksumWithPitrIdMapConfig {
    /// 同步 TableConcurrency 到内外两层 Config，避免只写一层导致并发仍为默认。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.RestoreConfig.TableConcurrency =
            flags.GetUint(flagTableConcurrency).map_err(Error::Trace)?;
        self.RestoreConfig.Config.TableConcurrency = self.RestoreConfig.TableConcurrency;
        self.RestoreConfig.RestoreTS = flags.GetUint64(flagRestoredTS).map_err(Error::Trace)?;
        self.RestoreConfig.UpstreamClusterID = flags
            .GetUint64(flagUpstreamClusterID)
            .map_err(Error::Trace)?;
        self.ChecksumTS = flags.GetUint64(flagChecksumTS).map_err(Error::Trace)?;
        self.RestoreConfig.Config.ParseFromFlags(flags)
    }
}

/// upstream checksum：只需 RestoreConfig（含 RestoreTS）。
#[derive(Clone, Debug, Default)]
// flag 常量字符串必须与 Go/文档保持二进制兼容。
pub struct ChecksumUpstreamConfig {
    pub RestoreConfig: RestoreConfig,
}

// 配置层只解析与校验，不打开存储或拨号 PD。
impl ChecksumUpstreamConfig {
    /// 与 PiTR 解析相同地双写 TableConcurrency，再填 RestoreTS。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.RestoreConfig.TableConcurrency =
            flags.GetUint(flagTableConcurrency).map_err(Error::Trace)?;
        self.RestoreConfig.Config.TableConcurrency = self.RestoreConfig.TableConcurrency;
        self.RestoreConfig.RestoreTS = flags.GetUint64(flagRestoredTS).map_err(Error::Trace)?;
        self.RestoreConfig.Config.ParseFromFlags(flags)
    }
}
