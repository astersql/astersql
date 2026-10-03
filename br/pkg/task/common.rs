// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Common BR task configuration matching `br/pkg/task/common.go`.
//!
//! 公共 BR 任务配置：CLI flag、Config 解析、TLS/PD、加解密与 master-key、日志脱敏。
//! 备份/恢复/流任务均依赖本文件的 Config 与 Define*Flags；业务编排不在此。
//! 与 Go `common.go` 对齐：限速溢出检查、过滤器用 Enclose*、PD URL 与 TLS 开关互斥。
//! Cipher 明文密钥与 master-key 互斥；日志字段通过 flagToZapField 剥 query/打码。
//! NewMgr/GetStorage 在此提供可注入边界，测试可用 MemMgr/MemStorage。
//! ReadBackupMeta 处理 GCS 对象缺失与密文 IV 前缀，错误文案保持 Go 习惯。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use url::Url;

use crate::encryption::validateAndParseMasterKeyString;
use crate::stubs::backuppb::{CipherInfo, MasterKeyConfig, StorageBackend};
use crate::stubs::encryptionpb::EncryptionMethod;
use crate::stubs::{
    BackendOptions, CaseInsensitive, CrypterIvLen, DefChecksumTableConcurrency, EncloseDBAndTable,
    EncloseName, Error, Flag, FlagSet, FlagValue, Glue, IsEffectiveEncryptionMethod,
    KeepaliveParams, MemMgr, MetaFile, Mgr, MiB, NewOperationContext, OperationContext,
    PDSecurityOption, ParseBackend, ParseFilter, Progress, Result, Storage, StorageOptions,
    TLSConfigInner, TableFilter, VersionCheckerType, ZapField, ZapString, ZapStringer, berrors,
};

/// 是否把云存储凭据下发给 TiKV。
pub const flagSendCreds: &str = "send-credentials-to-tikv";
/// 本地不读凭据（隐藏 flag）。
pub const flagNoCreds: &str = "no-credentials";
/// 备份/恢复存储 URL。
pub const flagStorage: &str = "storage";
/// PD 地址列表。
pub const flagPD: &str = "pd";
/// TLS CA 路径。
pub const flagCA: &str = "ca";
/// TLS 客户端证书路径。
pub const flagCert: &str = "cert";
/// TLS 客户端私钥路径。
pub const flagKey: &str = "key";
/// 单库模式库名。
pub const flagDatabase: &str = "db";
/// 单表模式表名。
pub const flagTable: &str = "table";
/// 表级 checksum 并发。
pub const flagChecksumConcurrency: &str = "checksum-concurrency";
/// 限速数值（再乘 unit）。
pub const flagRateLimit: &str = "ratelimit";
/// 限速单位，默认 MiB，隐藏。
pub const flagRateLimitUnit: &str = "ratelimit-unit";
/// 任务通用并发。
pub const flagConcurrency: &str = "concurrency";
/// 是否做 checksum。
pub const flagChecksum: &str = "checksum";
/// 表过滤器表达式数组。
pub const flagFilter: &str = "filter";
/// 过滤器是否大小写敏感。
pub const flagCaseSensitive: &str = "case-sensitive";
/// 已弃用：历史上移除 TiFlash。
pub const flagRemoveTiFlash: &str = "remove-tiflash";
/// 是否检查集群版本等前提。
pub const flagCheckRequirement: &str = "check-requirements";
/// TiKV import 模式切换间隔。
pub const flagSwitchModeInterval: &str = "switch-mode-interval";
/// gRPC keepalive time（隐藏）。
pub const flagGrpcKeepaliveTime: &str = "grpc-keepalive-time";
/// gRPC keepalive timeout（隐藏）。
pub const flagGrpcKeepaliveTimeout: &str = "grpc-keepalive-timeout";
/// 是否启用 OpenTracing。
pub const flagEnableOpenTracing: &str = "enable-opentracing";
/// 跳过存储路径检查（隐藏）。
pub const flagSkipCheckPath: &str = "skip-check-path";
/// 干跑（部分子命令）。
pub const flagDryRun: &str = "dry-run";
/// 跳过 AWS API（EBS 等）。
pub const flagSkipAWS: &str = "skip-aws";
/// 是否使用 FSR。
pub const flagUseFSR: &str = "use-fsr";
/// 云 API 并发。
pub const flagCloudAPIConcurrency: &str = "cloud-api-concurrency";
/// 是否包含系统表。
pub const flagWithSysTable: &str = "with-sys-table";
/// operator 已暂停 GC/调度器。
pub const flagOperatorPausedGCAndSchedulers: &str = "operator-paused-gc-and-scheduler";

/// 默认 switch-mode 间隔 5 分钟。
pub const defaultSwitchInterval: Duration = Duration::from_secs(5 * 60);
/// 默认 gRPC keepalive time。
pub const defaultGRPCKeepaliveTime: Duration = Duration::from_secs(10);
/// 默认 gRPC keepalive timeout。
pub const defaultGRPCKeepaliveTimeout: Duration = Duration::from_secs(3);
/// 默认云 API 并发。
pub const defaultCloudAPIConcurrency: u32 = 8;

/// 全量备份加密算法。
pub const flagFullBackupCipherType: &str = "crypter.method";
/// 全量备份密钥 hex（日志脱敏）。
pub const flagFullBackupCipherKey: &str = "crypter.key";
/// 全量备份密钥文件路径。
pub const flagFullBackupCipherKeyFile: &str = "crypter.key-file";
/// 日志备份加密算法。
pub const flagLogBackupCipherType: &str = "log.crypter.method";
/// 日志备份密钥 hex（日志脱敏）。
pub const flagLogBackupCipherKey: &str = "log.crypter.key";
/// 日志备份密钥文件。
pub const flagLogBackupCipherKeyFile: &str = "log.crypter.key-file";
/// 元数据下载批大小（隐藏）。
pub const flagMetadataDownloadBatchSize: &str = "metadata-download-batch-size";
/// 元数据下载默认批大小 128。
pub const defaultMetadataDownloadBatchSize: u32 = 128;
/// 限速 0 表示不限制。
pub const unlimited: u64 = 0;
/// AES-128 密钥字节长度。
pub const crypterAES128KeyLen: usize = 16;
/// AES-192 密钥字节长度。
pub const crypterAES192KeyLen: usize = 24;
/// AES-256 密钥字节长度。
pub const crypterAES256KeyLen: usize = 32;
/// 全量备份类型（kv/aws-ebs）。
pub const flagFullBackupType: &str = "type";
/// 多 master-key URL 分隔符。
pub const masterKeysDelimiter: &str = ",";
/// master-key URL 列表（日志脱敏）。
pub const flagMasterKeyConfig: &str = "master-key";
/// master-key 包装数据密钥的算法。
pub const flagMasterKeyCipherType: &str = "master-key-crypter-method";
/// 非 hex 密钥的统一错误文案。
pub const cipherKeyNonHexErrorMsg: &str = "cipher key must be a valid hexadecimal string";

// Stream / restore flag names referenced by redaction helpers.
/// 流任务全量备份存储 flag 名。
pub const FlagStreamFullBackupStorage: &str = "full-backup-storage";
/// PiTR 加索引 SQL 存储 flag 名。
pub const FlagPiTRAddIndexSQLStorage: &str = "pitr-add-index-sql-storage";
/// checkpoint 存储 URL。
pub const flagCheckpointStorage: &str = "checkpoint-storage";
/// 是否加载统计信息。
pub const flagLoadStats: &str = "load-stats";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 全量备份类型包装；Valid 仅接受 kv/aws-ebs。
pub struct FullBackupType(pub String);

/// KV 全量备份类型字面量。
pub const FullBackupTypeKV: &str = "kv";
/// AWS EBS 全量备份类型字面量。
pub const FullBackupTypeEBS: &str = "aws-ebs";

impl FullBackupType {
    /// 仅 kv 与 aws-ebs 合法。
    pub fn Valid(&self) -> bool {
        self.0 == FullBackupTypeKV || self.0 == FullBackupTypeEBS
    }
}

#[derive(Clone, Debug, Default)]
/// PD/TiKV TLS 三元组；IsEnabled 以 CA 非空为准。
pub struct TLSConfig {
    pub CA: String,
    pub Cert: String,
    pub Key: String,
}

impl TLSConfig {
    /// CA 非空视为启用 TLS。
    pub fn IsEnabled(&self) -> bool {
        !self.CA.is_empty()
    }

    /// 启用时检查路径存在，再返回内部 TLS 标记。
    pub fn ToTLSConfig(&self) -> Result<TLSConfigInner> {
        if !self.IsEnabled() {
            return Ok(TLSConfigInner { enabled: false });
        }
        // Real TLS material loading is an OS/file boundary; validate paths exist when set.
        // 路径非空则要求文件存在，避免运行期才发现缺证书。
        for p in [&self.CA, &self.Cert, &self.Key] {
            if !p.is_empty() && !Path::new(p).exists() {
                return Err(Error::new(format!("tls file not found: {p}")));
            }
        }
        Ok(TLSConfigInner { enabled: true })
    }

    /// 转为 PD 安全选项路径三元组。
    pub fn ToPDSecurityOption(&self) -> PDSecurityOption {
        PDSecurityOption {
            CAPath: self.CA.clone(),
            CertPath: self.Cert.clone(),
            KeyPath: self.Key.clone(),
        }
    }

    /// 转为 TiKV 集群 SSL 字段。
    pub fn ToKVSecurity(&self) -> crate::stubs::KVSecurity {
        crate::stubs::KVSecurity {
            ClusterSSLCA: self.CA.clone(),
            ClusterSSLCert: self.Cert.clone(),
            ClusterSSLKey: self.Key.clone(),
        }
    }

    /// 从 FlagSet 填充自身字段（TLS 或 Config）。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        let (ca, cert, key) = ParseTLSTripleFromFlags(flags)?;
        self.CA = ca;
        self.Cert = cert;
        self.Key = key;
        Ok(())
    }
}

#[derive(Clone, Debug)]
/// BR 公共配置聚合体，ParseFromFlags 是主入口。
pub struct Config {
    pub BackendOptions: BackendOptions,
    pub Storage: String,
    pub PD: Vec<String>,
    pub TLS: TLSConfig,
    pub RateLimit: u64,
    pub ChecksumConcurrency: u32,
    pub TableConcurrency: u32,
    pub Concurrency: u32,
    pub Checksum: bool,
    pub SendCreds: bool,
    pub LogProgress: bool,
    pub OperationContext: OperationContext,
    pub CaseSensitive: bool,
    pub NoCreds: bool,
    pub CheckRequirements: bool,
    pub EnableOpenTracing: bool,
    pub SkipCheckPath: bool,
    pub FilterStr: Vec<String>,
    pub TableFilter: TableFilter,
    pub SwitchModeInterval: Duration,
    pub Schemas: HashSet<String>,
    pub Tables: HashSet<String>,
    pub GRPCKeepaliveTime: Duration,
    pub GRPCKeepaliveTimeout: Duration,
    pub CipherInfo: CipherInfo,
    pub LogBackupCipherInfo: CipherInfo,
    pub MasterKeyConfig: MasterKeyConfig,
    pub ExplicitFilter: bool,
    pub KeyspaceName: String,
    pub MetadataDownloadBatchSize: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            BackendOptions: BackendOptions::default(),
            Storage: String::new(),
            PD: Vec::new(),
            TLS: TLSConfig::default(),
            RateLimit: 0,
            ChecksumConcurrency: 0,
            TableConcurrency: 0,
            Concurrency: 0,
            Checksum: false,
            SendCreds: true,
            LogProgress: false,
            OperationContext: OperationContext::default(),
            CaseSensitive: false,
            NoCreds: false,
            CheckRequirements: true,
            EnableOpenTracing: false,
            SkipCheckPath: false,
            FilterStr: Vec::new(),
            TableFilter: TableFilter::default(),
            SwitchModeInterval: Duration::ZERO,
            Schemas: HashSet::new(),
            Tables: HashSet::new(),
            GRPCKeepaliveTime: Duration::ZERO,
            GRPCKeepaliveTimeout: Duration::ZERO,
            CipherInfo: CipherInfo::default(),
            LogBackupCipherInfo: CipherInfo::default(),
            MasterKeyConfig: MasterKeyConfig::default(),
            ExplicitFilter: false,
            KeyspaceName: String::new(),
            MetadataDownloadBatchSize: 0,
        }
    }
}

impl Config {
    /// 补齐或校验 OperationID/StartedAt 成对出现。
    pub fn EnsureOperationContext(&mut self, command: &str) -> Result<()> {
        if !self.OperationContext.OperationID.is_empty() {
            // 已有 ID 必须同时有合法 StartedAt。
            if self.OperationContext.StartedAt == std::time::SystemTime::UNIX_EPOCH {
                return Err(Error::new("operation started time is required"));
            }
            return Ok(());
        }
        if self.OperationContext.StartedAt != std::time::SystemTime::UNIX_EPOCH {
            // 只有 StartedAt 无 ID 同样非法。
            return Err(Error::new("operation ID is required"));
        }
        // 两者皆空则新建上下文。
        self.OperationContext = NewOperationContext(command)?;
        Ok(())
    }

    /// 用户是否显式指定 schema/table/filter。
    pub fn UserFiltered(&self) -> bool {
        !self.Schemas.is_empty() || !self.Tables.is_empty() || !self.FilterStr.is_empty()
    }

    /// 备份默认关闭 checksum（与 Go 一致）。
    pub fn OverrideDefaultForBackup(&mut self) {
        self.Checksum = false;
    }

    /// 填充 keepalive/checksum 并发/元数据批大小默认值。
    pub fn adjust(&mut self) {
        if self.GRPCKeepaliveTime == Duration::ZERO {
            self.GRPCKeepaliveTime = defaultGRPCKeepaliveTime;
        }
        if self.GRPCKeepaliveTimeout == Duration::ZERO {
            self.GRPCKeepaliveTimeout = defaultGRPCKeepaliveTimeout;
        }
        if self.ChecksumConcurrency == 0 {
            self.ChecksumConcurrency = DefChecksumTableConcurrency;
        }
        if self.MetadataDownloadBatchSize == 0 {
            self.MetadataDownloadBatchSize = defaultMetadataDownloadBatchSize;
        }
    }

    /// 按 TLS 开关规范化全部 PD URL。
    pub fn normalizePDURLs(&mut self) -> Result<()> {
        let use_tls = self.TLS.IsEnabled();
        for pd in &mut self.PD {
            *pd = normalizePDURL(pd, use_tls)?;
        }
        Ok(())
    }

    /// 解析全量 crypter；明文则跳过密钥。
    pub fn parseCipherInfo(&mut self, flags: &FlagSet) -> Result<()> {
        let crypterStr = flags.GetString(flagFullBackupCipherType)?;
        self.CipherInfo.CipherType = parseCipherType(&crypterStr)?;
        // 明文无需读取密钥文件。
        if self.CipherInfo.CipherType == EncryptionMethod::PLAINTEXT {
            return Ok(());
        }
        let key = flags.GetString(flagFullBackupCipherKey)?;
        let keyFilePath = flags.GetString(flagFullBackupCipherKeyFile)?;
        self.CipherInfo.CipherKey = GetCipherKeyContent(&key, &keyFilePath)?;
        checkCipherKeyMatch(&self.CipherInfo)
    }

    /// 解析日志 crypter；有效时仍校验全量 CipherInfo（Go 行为）。
    pub fn parseLogBackupCipherInfo(&mut self, flags: &FlagSet) -> Result<bool> {
        let crypterStr = flags.GetString(flagLogBackupCipherType)?;
        self.LogBackupCipherInfo.CipherType = parseCipherType(&crypterStr)?;
        if !IsEffectiveEncryptionMethod(self.LogBackupCipherInfo.CipherType) {
            return Ok(false);
        }
        let key = flags.GetString(flagLogBackupCipherKey)?;
        let keyFilePath = flags.GetString(flagLogBackupCipherKeyFile)?;
        self.LogBackupCipherInfo.CipherKey = GetCipherKeyContent(&key, &keyFilePath)?;
        // Go 校验的是全量 CipherInfo 而非日志密钥——保持该怪异行为。
        // Go checks cfg.CipherInfo (full-backup cipher), not log cipher — keep parity.
        // Go checks cfg.CipherInfo (full-backup cipher), not log cipher — keep parity.
        checkCipherKeyMatch(&self.CipherInfo)?;
        Ok(true)
    }

    /// 拆分 master-key；与明文密钥互斥并校验算法。
    pub fn parseAndValidateMasterKeyInfo(
        &mut self,
        hasPlaintextKey: bool,
        flags: &FlagSet,
    ) -> Result<()> {
        let masterKeyString = flags.GetString(flagMasterKeyConfig).map_err(|err| {
            Error::Errorf(format!(
                "master key flag '{flagMasterKeyConfig}' is not defined: {err}"
            ))
        })?;
        // 未配置 master-key 直接返回。
        if masterKeyString.is_empty() {
            return Ok(());
        }
        // 明文数据密钥与 master-key 不能同时开启。
        if hasPlaintextKey {
            return Err(Error::Errorf(
                "invalid argument: both plaintext data key encryption and master key based encryption are set at the same time",
            ));
        }
        let encryptionMethodString = flags.GetString(flagMasterKeyCipherType).map_err(|err| {
            Error::Errorf(format!(
                "encryption method flag '{flagMasterKeyCipherType}' is not defined: {err}"
            ))
        })?;
        let encryptionMethod = parseCipherType(&encryptionMethodString)
            .map_err(|err| Error::Errorf(format!("failed to parse encryption method: {err}")))?;
        // plaintext/unknown 不能作为 master-key 包装算法。
        if !IsEffectiveEncryptionMethod(encryptionMethod) {
            return Err(Error::Errorf(format!(
                "invalid encryption method: {encryptionMethodString}"
            )));
        }
        let masterKeyStrings: Vec<&str> = masterKeyString.split(masterKeysDelimiter).collect();
        self.MasterKeyConfig = MasterKeyConfig {
            EncryptionType: encryptionMethod,
            MasterKeys: Vec::with_capacity(masterKeyStrings.len()),
        };
        // 逗号分隔的多 URL；trim 后逐条 validateAndParse。
        for keyString in masterKeyStrings {
            let trimmed = keyString.trim();
            let masterKey = validateAndParseMasterKeyString(trimmed).map_err(|err| {
                Error::Wrapf(err, format!("invalid master key configuration: {trimmed}"))
            })?;
            self.MasterKeyConfig.MasterKeys.push(masterKey);
        }
        Ok(())
    }

    /// 从 FlagSet 填充自身字段（TLS 或 Config）。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.Storage = flags.GetString(flagStorage)?;
        self.SendCreds = flags.GetBool(flagSendCreds)?;
        self.NoCreds = flags.GetBool(flagNoCreds)?;
        self.Checksum = flags.GetBool(flagChecksum)?;
        self.ChecksumConcurrency = flags.GetUint(flagChecksumConcurrency)?;

        let rateLimit = flags.GetUint64(flagRateLimit)?;
        let rateLimitUnit = flags.GetUint64(flagRateLimitUnit)?;
        // 限速乘法溢出保护：避免 wrap 成小值导致误限速。
        // 提示上限约 17PB/s，与 Go 错误信息一致。
        if rateLimit > 0 && rateLimitUnit > 0 && rateLimit > u64::MAX / rateLimitUnit {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!(
                    "rate limit calculation overflow: {rateLimit} * {rateLimitUnit} exceeds uint64 max (consider max ~17PB/s)"
                ),
            ));
        }
        self.RateLimit = rateLimit.saturating_mul(rateLimitUnit);

        self.Schemas.clear();
        self.Tables.clear();
        let mut caseSensitive = false;
        // 过滤优先级：显式 filter > --db/--table > 默认 *.*。
        // ExplicitFilter 记录用户是否改过 filter，供上层决策。
        if flags.Lookup(flagFilter).is_some() {
            self.ExplicitFilter = flags.Changed(flagFilter);
            self.FilterStr = flags.GetStringArray(flagFilter)?;
            self.TableFilter = ParseFilter(self.FilterStr.clone())?;
            caseSensitive = flags.GetBool(flagCaseSensitive)?;
        } else if let Some(FlagValue::String(db)) = flags.Lookup(flagDatabase) {
            // --db 空串非法。
            if db.is_empty() {
                return Err(Error::Annotate(
                    berrors::ErrInvalidArgument,
                    "empty database name is not allowed",
                ));
            }
            self.Schemas.insert(EncloseName(db));
            if let Some(FlagValue::String(tbl)) = flags.Lookup(flagTable) {
                // --table 空串非法。
                if tbl.is_empty() {
                    return Err(Error::Annotate(
                        berrors::ErrInvalidArgument,
                        "empty table name is not allowed",
                    ));
                }
                self.Tables.insert(EncloseDBAndTable(db, tbl));
                self.TableFilter = ParseFilter(vec![format!("`{db}`.`{tbl}`")])?;
                self.FilterStr = vec![format!("`{db}`.`{tbl}`")];
            } else {
                self.TableFilter = ParseFilter(vec![format!("`{db}`.*")])?;
                self.FilterStr = vec![format!("`{db}`.*")];
            }
        } else {
            // 未指定过滤时默认全库全表。
            self.TableFilter = ParseFilter(vec!["*.*".into()])?;
            self.FilterStr = vec!["*.*".into()];
        }
        // 默认大小写不敏感，包装 CaseInsensitive 过滤器。
        if !caseSensitive {
            self.TableFilter = CaseInsensitive(self.TableFilter.clone());
        }

        self.CheckRequirements = flags.GetBool(flagCheckRequirement)?;
        self.SwitchModeInterval = flags.GetDuration(flagSwitchModeInterval)?;
        self.GRPCKeepaliveTime = flags.GetDuration(flagGrpcKeepaliveTime)?;
        self.GRPCKeepaliveTimeout = flags.GetDuration(flagGrpcKeepaliveTimeout)?;
        self.EnableOpenTracing = flags.GetBool(flagEnableOpenTracing)?;
        // 零间隔非法：TiKV 模式切换需要正周期。
        if self.SwitchModeInterval.is_zero() {
            return Err(Error::Annotatef(
                berrors::ErrInvalidArgument,
                format!(
                    "--switch-mode-interval must be positive, {:?} is not allowed",
                    self.SwitchModeInterval
                ),
            ));
        }
        self.BackendOptions.ParseFromFlags(flags)?;
        self.TLS.ParseFromFlags(flags)?;
        self.PD = flags.GetStringSlice(flagPD)?;
        // 至少一个 PD，否则后续 NewMgr 也会失败。
        if self.PD.is_empty() {
            return Err(Error::Annotate(
                berrors::ErrInvalidArgument,
                "must provide at least one PD server address",
            ));
        }
        self.SkipCheckPath = flags.GetBool(flagSkipCheckPath)?;
        self.parseCipherInfo(flags)?;
        // 日志明文密钥存在时禁止再配 master-key（互斥）。
        let hasLogBackupPlaintextKey = self.parseLogBackupCipherInfo(flags)?;
        self.parseAndValidateMasterKeyInfo(hasLogBackupPlaintextKey, flags)?;
        self.MetadataDownloadBatchSize = flags.GetUint(flagMetadataDownloadBatchSize)?;
        // keyspace flag 可能未注册（视子命令而定）。
        if flags.Lookup(crate::stubs::FlagKeyspaceName).is_some() {
            self.KeyspaceName = flags.GetString(crate::stubs::FlagKeyspaceName)?;
        }
        self.normalizePDURLs()
    }
}

/// OperationContext hint：restore_id。
pub const operationHintRestoreID: &str = "restore_id";

/// restore_id=0 时清空 hint，否则写入字符串。
pub fn setOperationContextRestoreID(operationContext: &mut OperationContext, restoreID: u64) {
    if restoreID == 0 {
        operationContext.SetHintField(operationHintRestoreID, "");
        return;
    }
    operationContext.SetHintField(operationHintRestoreID, &restoreID.to_string());
}

/// 注册备份/恢复共用 flag 与默认值/隐藏/弃用标记。
pub fn DefineCommonFlags(flags: &mut FlagSet) {
    // 默认发送凭据到 TiKV；no-creds/skip-check-path 等隐藏。
    flags.DefineBool(flagSendCreds, true);
    flags.DefineString(flagStorage, "");
    flags.DefineStringSlice(flagPD, vec!["127.0.0.1:2379".into()]);
    flags.DefineString(flagCA, "");
    flags.DefineString(flagCert, "");
    flags.DefineString(flagKey, "");
    flags.DefineUint(flagChecksumConcurrency, DefChecksumTableConcurrency as u64);
    flags.DefineUint64(flagRateLimit, unlimited);
    flags.DefineBool(flagChecksum, false);
    flags.DefineBool(flagRemoveTiFlash, true);
    flags.DefineUint64(flagRateLimitUnit, MiB);
    let _ = flags.MarkHidden(flagRateLimitUnit);
    // remove-tiflash 已无意义，保留 flag 但弃用提示。
    let _ = flags.MarkDeprecated(
        flagRemoveTiFlash,
        "TiFlash is fully supported by BR now, removing TiFlash isn't needed any more. This flag would be ignored.",
    );
    flags.DefineBool(flagCheckRequirement, true);
    flags.DefineDuration(flagSwitchModeInterval, defaultSwitchInterval);
    flags.DefineDuration(flagGrpcKeepaliveTime, defaultGRPCKeepaliveTime);
    flags.DefineDuration(flagGrpcKeepaliveTimeout, defaultGRPCKeepaliveTimeout);
    let _ = flags.MarkHidden(flagGrpcKeepaliveTime);
    let _ = flags.MarkHidden(flagGrpcKeepaliveTimeout);
    flags.DefineBool(flagEnableOpenTracing, false);
    flags.DefineBool(flagNoCreds, false);
    let _ = flags.MarkHidden(flagNoCreds);
    flags.DefineBool(flagSkipCheckPath, false);
    let _ = flags.MarkHidden(flagSkipCheckPath);
    flags.DefineString(flagFullBackupCipherType, "plaintext");
    flags.DefineString(flagFullBackupCipherKey, "");
    flags.DefineString(flagFullBackupCipherKeyFile, "");
    flags.DefineUint(
        flagMetadataDownloadBatchSize,
        defaultMetadataDownloadBatchSize as u64,
    );
    flags.DefineString(flagLogBackupCipherType, "plaintext");
    flags.DefineString(flagLogBackupCipherKey, "");
    flags.DefineString(flagLogBackupCipherKeyFile, "");
    flags.DefineString(flagMasterKeyCipherType, "plaintext");
    flags.DefineString(flagMasterKeyConfig, "");
    let _ = flags.MarkHidden(flagMetadataDownloadBatchSize);
}

/// 流任务隐藏不适用的 checksum/cipher 等 flag。
pub fn HiddenFlagsForStream(flags: &mut FlagSet) {
    // 流备份场景不适用的校验/加密相关 flag 统一隐藏。
    for name in [
        flagChecksum,
        flagLoadStats,
        flagChecksumConcurrency,
        flagRateLimit,
        flagRateLimitUnit,
        flagRemoveTiFlash,
        flagFullBackupCipherType,
        flagFullBackupCipherKey,
        flagFullBackupCipherKeyFile,
        flagLogBackupCipherType,
        flagLogBackupCipherKey,
        flagLogBackupCipherKeyFile,
        flagSwitchModeInterval,
        flagMasterKeyConfig,
        flagMasterKeyCipherType,
    ] {
        let _ = flags.MarkHidden(name);
    }
}

/// 用默认 FlagSet 解析出的标准 Config。
pub fn DefaultConfig() -> Config {
    let mut fs = FlagSet::new();
    DefineCommonFlags(&mut fs);
    let mut cfg = Config::default();
    cfg.ParseFromFlags(&fs)
        .expect("infallible operation failed");
    cfg
}

/// 注册 --db。
pub fn DefineDatabaseFlags(flags: &mut FlagSet) {
    flags.DefineString(flagDatabase, "");
}

/// 注册 --db 与 --table。
pub fn DefineTableFlags(flags: &mut FlagSet) {
    DefineDatabaseFlags(flags);
    flags.DefineString(flagTable, "");
}

/// 注册 filter/case-sensitive，可选择隐藏。
pub fn DefineFilterFlags(flags: &mut FlagSet, defaultFilter: Vec<String>, setHidden: bool) {
    flags.DefineStringArray(flagFilter, defaultFilter);
    flags.DefineBool(flagCaseSensitive, false);
    if setHidden {
        let _ = flags.MarkHidden(flagFilter);
        let _ = flags.MarkHidden(flagCaseSensitive);
    }
}

/// 从 flag 读取 CA/Cert/Key 三元组。
pub fn ParseTLSTripleFromFlags(flags: &FlagSet) -> Result<(String, String, String)> {
    Ok((
        flags.GetString(flagCA)?,
        flags.GetString(flagCert)?,
        flags.GetString(flagKey)?,
    ))
}

/// 解析 crypter.method 字符串到 EncryptionMethod。
pub fn parseCipherType(t: &str) -> Result<EncryptionMethod> {
    match t {
        "plaintext" | "PLAINTEXT" => Ok(EncryptionMethod::PLAINTEXT),
        "aes128-ctr" | "AES128-CTR" => Ok(EncryptionMethod::AES128_CTR),
        "aes192-ctr" | "AES192-CTR" => Ok(EncryptionMethod::AES192_CTR),
        "aes256-ctr" | "AES256-CTR" => Ok(EncryptionMethod::AES256_CTR),
        _ => Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!("invalid crypter method '{t}'"),
        )),
    }
}

/// 密钥与密钥文件必须二选一。
pub fn checkCipherKey(cipherKey: &str, cipherKeyFile: &str) -> Result<()> {
    if cipherKey.is_empty() == cipherKeyFile.is_empty() {
        return Err(Error::Annotate(
            berrors::ErrInvalidArgument,
            "exactly one of cipher key or keyfile path should be provided",
        ));
    }
    Ok(())
}

/// 读取 hex 密钥或文件（去尾换行）并 decode。
pub fn GetCipherKeyContent(cipherKey: &str, cipherKeyFile: &str) -> Result<Vec<u8>> {
    checkCipherKey(cipherKey, cipherKeyFile)?;
    let hexString = if !cipherKey.is_empty() {
        cipherKey.to_string()
    } else {
        let content = fs::read(cipherKeyFile)
            .map_err(|err| Error::Annotate(err.to_string(), "failed to read cipher file"))?;
        // 密钥文件常见尾换行，去掉后再按 hex 解码。
        let trimmed = if content.ends_with(b"\n") {
            &content[..content.len() - 1]
        } else {
            &content[..]
        };
        String::from_utf8_lossy(trimmed).into_owned()
    };
    // Go passes the direct value through unchanged and, for files, removes only
    // the single trailing LF above. Preserve all other whitespace for parity.
    hex::decode(hexString)
        .map_err(|_| Error::Annotate(berrors::ErrInvalidArgument, cipherKeyNonHexErrorMsg))
}

/// 按算法校验密钥长度；UNKNOWN 拒绝。
pub fn checkCipherKeyMatch(cipher: &CipherInfo) -> Result<()> {
    match cipher.CipherType {
        // 明文无需密钥长度。
        EncryptionMethod::PLAINTEXT => Ok(()),
        EncryptionMethod::AES128_CTR if cipher.CipherKey.len() == crypterAES128KeyLen => Ok(()),
        EncryptionMethod::AES192_CTR if cipher.CipherKey.len() == crypterAES192KeyLen => Ok(()),
        EncryptionMethod::AES256_CTR if cipher.CipherKey.len() == crypterAES256KeyLen => Ok(()),
        EncryptionMethod::AES128_CTR => Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!(
                "AES-128 key length mismatch: expected {crypterAES128KeyLen}, got {}",
                cipher.CipherKey.len()
            ),
        )),
        EncryptionMethod::AES192_CTR => Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!(
                "AES-192 key length mismatch: expected {crypterAES192KeyLen}, got {}",
                cipher.CipherKey.len()
            ),
        )),
        EncryptionMethod::AES256_CTR => Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!(
                "AES-256 key length mismatch: expected {crypterAES256KeyLen}, got {}",
                cipher.CipherKey.len()
            ),
        )),
        other => Err(Error::Errorf(format!(
            "Unknown encryption method: {other:?}"
        ))),
    }
}

/// Factory for Mgr used by orchestration entrypoints; tests inject `MemMgr`.
/// 构造 Mgr；空 PD 失败；当前返回 MemMgr 桩。
pub fn NewMgr(
    _g: &dyn Glue,
    keyspaceName: &str,
    pds: &[String],
    tlsConfig: &TLSConfig,
    keepalive: KeepaliveParams,
    checkRequirements: bool,
    needDomain: bool,
    _versionCheckerType: VersionCheckerType,
) -> Result<Arc<dyn Mgr>> {
    let _ = (keyspaceName, keepalive, checkRequirements, needDomain);
    // 与 ParseFromFlags 一致：空 PD 直接失败。
    if pds.is_empty() {
        return Err(Error::Annotate(
            berrors::ErrInvalidArgument,
            "pd address can not be empty",
        ));
    }
    if tlsConfig.IsEnabled() {
        let _ = tlsConfig.ToTLSConfig()?;
    }
    Ok(Arc::new(MemMgr {
        cluster_version: "mock-cluster".into(),
        region_count: 1,
        ..Default::default()
    }))
}

/// 解析 backend 并返回 MemStorage 边界。
pub fn GetStorage(storageName: &str, cfg: &Config) -> Result<(StorageBackend, Arc<dyn Storage>)> {
    // 当前实现返回内存 Storage，真实 IO 在其他层。
    let u = ParseBackend(storageName, &cfg.BackendOptions)?;
    let s: Arc<dyn Storage> = Arc::new(crate::stubs::MemStorage::new());
    Ok((u, s))
}

/// 由 Config 生成 StorageOptions。
pub fn storageOpts(cfg: &Config) -> StorageOptions {
    StorageOptions {
        NoCredentials: cfg.NoCreds,
        SendCredentials: cfg.SendCreds,
        ..Default::default()
    }
}

/// 读 backupmeta：GCS 缺失注解、密文跳过 IV。
pub fn ReadBackupMeta(
    fileName: &str,
    cfg: &Config,
    storage: &dyn Storage,
) -> Result<(StorageBackend, crate::stubs::backuppb::BackupMeta)> {
    let u = ParseBackend(&cfg.Storage, &cfg.BackendOptions)?;
    let metaData = match storage.ReadFile(fileName) {
        Ok(data) => data,
        // GCS 对象缺失单独注解，便于区分权限/网络错误。
        Err(err) if gcsObjectNotFound(&err) => {
            return Err(Error::Annotate(err.msg, "load backupmeta failed"));
        }
        Err(err) => return Err(Error::Annotate(err.msg, "load backupmeta failed")),
    };
    // 非明文时文件头携带 IV，解析前跳过 CrypterIvLen。
    let iv = if cfg.CipherInfo.CipherType != EncryptionMethod::PLAINTEXT {
        metaData.get(..CrypterIvLen).unwrap_or(&[]).to_vec()
    } else {
        Vec::new()
    };
    let body = &metaData[iv.len()..];
    // 空 body 给默认 meta，避免 JSON 解析失败。
    let backupMeta: crate::stubs::backuppb::BackupMeta = if body.is_empty() {
        crate::stubs::backuppb::BackupMeta::default()
    } else {
        serde_json::from_slice(body).map_err(|err| {
            Error::Annotate(
                err.to_string(),
                "parse backupmeta failed because of wrong aes cipher",
            )
        })?
    };
    Ok((u, backupMeta))
}

/// 日志字段：剥存储 query、打码密钥与 master-key。
pub fn flagToZapField(f: &Flag) -> ZapField {
    match f.Name.as_str() {
        flagStorage
        | FlagStreamFullBackupStorage
        | FlagPiTRAddIndexSQLStorage
        | flagCheckpointStorage => match Url::parse(&f.Value) {
            // 存储 URL 打日志时剥掉 query，避免泄露 AK/SK。
            Ok(mut hiddenQuery) => {
                hiddenQuery.set_query(None);
                ZapStringer(f.Name.clone(), hiddenQuery.to_string())
            }
            Err(_) => ZapString(f.Name.clone(), "<invalid URI>"),
        },
        // 密钥类 flag 一律输出 <redacted>。
        flagFullBackupCipherKey | flagLogBackupCipherKey | "azblob.encryption-key" => {
            ZapString(f.Name.clone(), "<redacted>")
        }
        // master-key 整串脱敏，避免 query 凭据进日志。
        flagMasterKeyConfig => ZapString(f.Name.clone(), "<redacted>"),
        _ => ZapStringer(f.Name.clone(), f.Value.clone()),
    }
}

/// 打印命令参数占位（与 Go 签名对齐）。
pub fn LogArguments(cmd_path: &str, flags: &FlagSet) {
    let _ = (cmd_path, flags);
}

/// 从 Config 提取 gRPC keepalive 参数。
pub fn GetKeepalive(cfg: &Config) -> KeepaliveParams {
    KeepaliveParams {
        Time: cfg.GRPCKeepaliveTime,
        Timeout: cfg.GRPCKeepaliveTimeout,
        PermitWithoutStream: false,
    }
}

/// 剥 http(s)://，并与 TLS 开关交叉校验。
pub fn normalizePDURL(pd: &str, useTLS: bool) -> Result<String> {
    // http 与 TLS 启用互斥；https 与 TLS 关闭互斥。
    if let Some(rest) = pd.strip_prefix("http://") {
        // TLS 开启却用 http:// 直接拒绝。
        if useTLS {
            return Err(Error::Annotate(
                berrors::ErrInvalidArgument,
                "pd url starts with http while TLS enabled",
            ));
        }
        return Ok(rest.to_string());
    }
    if let Some(rest) = pd.strip_prefix("https://") {
        // TLS 关闭却用 https:// 直接拒绝。
        if !useTLS {
            return Err(Error::Annotate(
                berrors::ErrInvalidArgument,
                "pd url starts with https while TLS disabled",
            ));
        }
        return Ok(rest.to_string());
    }
    Ok(pd.to_string())
}

/// 识别 GCS 对象不存在错误。
pub fn gcsObjectNotFound(err: &Error) -> bool {
    Error::Cause(err).contains("ErrObjectNotExist")
        || Error::Cause(err).contains("object not exist")
}

/// 写百分比进度文件；完成或取消则删除。
pub fn progressFileWriterRoutine(
    progress: &dyn Progress,
    total: i64,
    progressFile: &str,
    cancelled: bool,
) {
    // 取消或无效总量：删除进度文件，避免陈旧百分比。
    if cancelled || total <= 0 {
        let _ = fs::remove_file(progressFile);
        return;
    }
    let cur = progress.GetCurrent();
    // 进度完成删除文件，与取消路径一致。
    if cur >= total {
        let _ = fs::remove_file(progressFile);
        return;
    }
    // 写入两位小数百分比，供外部探针读取。
    let p = (cur as f64 / total as f64) * 100.0;
    let _ = fs::write(progressFile, format!("{p:.2}"));
}

/// 经 Glue 控制台输出字符串。
pub fn WriteStringToConsole(g: &dyn Glue, msg: &str) -> Result<()> {
    g.ConsoleOutWrite(msg.as_bytes())
}

/// Dial the metadata service selected by BR's keyspace, preserving proxy endpoints.
pub fn dialEtcdWithCfg(
    ctx: &astersql_metaservice::Context,
    cfg: &Config,
) -> Result<astersql_metaservice::NamespacedEtcdClient> {
    dialEtcdWithCfgAndFactory(ctx, cfg, None)
}

pub fn dialEtcdWithCfgAndFactory(
    ctx: &astersql_metaservice::Context,
    cfg: &Config,
    factory: Option<&astersql_metaservice::PdClientFactory>,
) -> Result<astersql_metaservice::NamespacedEtcdClient> {
    use astersql_metaservice::{DialEtcdClient, EtcdDialConfig, PdSecurity};
    let security = PdSecurity {
        ca: cfg.TLS.CA.clone(),
        cert: cfg.TLS.Cert.clone(),
        key: cfg.TLS.Key.clone(),
    };
    let config = EtcdDialConfig {
        tls: security
            .etcd_tls()
            .map_err(|error| Error::new(error.to_string()))?,
        keepalive_time: cfg.GRPCKeepaliveTime,
        keepalive_timeout: cfg.GRPCKeepaliveTimeout,
        ..Default::default()
    };
    DialEtcdClient(ctx, &cfg.KeyspaceName, &cfg.PD, &security, factory, config)
        .map_err(|error| Error::new(error.to_string()))
}

#[cfg(test)]
#[path = "meta_service_group_test.rs"]
mod meta_service_group_test;
