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

//! Cluster / BR version checks matching `br/pkg/version/version.go`.
//!
//! PD/`metapb.Store`/engine/dbutil boundaries use local stand-ins so this crate
//! stays off the arm64 grpcio rebuild path while preserving Go check logic.
//! 集群与 BR 版本兼容性检查核心，对齐 Go `br/pkg/version/version.go`。
//! PD/Store/引擎边界使用本地桩，避开 arm64 grpcio 重建，同时保留 Go 检查逻辑。
//! 覆盖 BR/PiTR/DDL/Keyspace/Backup 多条检查路径及版本串解析规范化。
//! 线程本地 RELEASE_OVERRIDE 供单测注入，避免 Rust 并行测试互相覆盖。
//! checkpoint/PiTR 支持标志为过程副作用，由对应检查函数写入供后续查询。
//! Cloud 版本年月映射遵循 mysql 包年份窗口，窗外返回空串再回落 0.0.0。
//! 错误统一走 ErrVersionMismatch 注解，文案尽量保持与 Go 字面量一致。
//! 本文件不发起真实 PD/DB 网络调用，调用方注入 PdClient/QueryExecutor。
//! removeVAndHash 是几乎所有检查路径的前置净化步骤，顺序为 hash→dirty→v。
//! ParseServerInfo 与 FetchVersion 分工：前者解析字符串，后者负责 SQL 获取策略。
//! CheckClusterVersion 是编排入口；具体策略由 VerChecker / 具名检查函数提供。

use std::sync::{Arc, Mutex, OnceLock};

use astersql_br_pkg_errors::ErrVersionMismatch;
use astersql_br_pkg_version_build as build;
use astersql_errors::{Annotate, SharedError};
use regex::Regex;
use semver::Version;

/// Once TableInfoVersion updated. BR need to check compatibility…
/// Mirrors `model.TableInfoVersion5`.
/// 当前 BR 支持的 TableInfo 版本上限，对应 `model.TableInfoVersion5`。
/// 升级模型版本时必须同步审查备份兼容性。
pub const CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION: u16 = 5;

/// BR 支持的最低 TiKV 版本门槛。
fn min_tikv_version() -> Version {
    Version::parse("3.1.0-beta.2").unwrap()
}
/// major=3 时 BR≥3.1.0 与更旧 TiKV 不兼容的分水岭。
fn incompatible_tikv_major3() -> Version {
    Version::parse("3.1.0").unwrap()
}
/// major=4 时 BR≥4.0.0-rc.1 与更旧 TiKV 不兼容的分水岭。
fn incompatible_tikv_major4() -> Version {
    Version::parse("4.0.0-rc.1").unwrap()
}
/// TiFlash major=3 的最低兼容版本。
fn compatible_tiflash_major3() -> Version {
    Version::parse("3.1.0").unwrap()
}
/// TiFlash major=4 的最低兼容版本。
fn compatible_tiflash_major4() -> Version {
    Version::parse("4.0.0").unwrap()
}

/// 匹配 `-N-gHASH` 构建后缀，供 removeVAndHash 剥离。
fn version_hash_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"-[0-9]+-g[0-9a-f]{7,}").unwrap())
}

/// CheckVersionForBR 写入的 checkpoint 支持错误缓存；None 表示支持。
fn checkpoint_support_error() -> &'static Mutex<Option<SharedError>> {
    static E: OnceLock<Mutex<Option<SharedError>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(None))
}

/// PiTR 是否支持批量 KV 文件标志，由 CheckVersionForBRPiTR 更新。
fn pitr_support_batch_kv_files() -> &'static Mutex<bool> {
    static B: OnceLock<Mutex<bool>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(false))
}

thread_local! {
    /// Test override for `build.ReleaseVersion`.
    ///
    /// Go's package tests mutate this value without racing because none of them
    /// use `t.Parallel`. Rust runs tests on multiple threads by default, so a
    /// process-global override would let unrelated tests overwrite each other.
    /// 使用线程本地覆盖：Go 测试不并行，Rust 默认并行，全局变量会互相污染。
    static RELEASE_OVERRIDE: std::cell::RefCell<Option<String>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Set/clear BR release version used by version checks (tests).
/// 设置/清除当前线程的 BR 发布版本覆盖，仅测试使用。
pub fn SetReleaseVersionForTest(v: Option<&str>) {
    RELEASE_OVERRIDE.with(|release| {
        *release.borrow_mut() = v.map(str::to_string);
    });
}

/// 优先测试覆盖，否则读 build::ReleaseVersion。
fn effective_release_version() -> String {
    if let Some(v) = RELEASE_OVERRIDE.with(|release| release.borrow().clone()) {
        return v;
    }
    build::ReleaseVersion()
}

/// 透传构建分支；Go 在非 master 且 tikv>BR 时告警，此处保留调用位点。
fn git_branch() -> String {
    build::GitBranch()
}

/// Store label stand-in for `metapb.StoreLabel`.
/// 本地 Store 标签桩，用于识别 tiflash 引擎。
#[derive(Clone, Debug, Default)]
pub struct StoreLabel {
    /// 标签键，识别引擎时使用 `"engine"`。
    pub Key: String,
    /// 标签值，如 `tiflash` / `tiflash_compute`。
    pub Value: String,
}

/// Store stand-in for `metapb.Store` fields used by BR version checks.
/// 仅保留版本检查所需字段，避免依赖完整 metapb。
#[derive(Clone, Debug, Default)]
pub struct Store {
    /// Store 标识，错误文案可选使用。
    pub Id: u64,
    /// 节点地址，出现在 mismatch 消息中。
    pub Address: String,
    /// 节点间通信地址；TiFlash 错误文案使用 protobuf GetPeerAddress。
    pub PeerAddress: String,
    /// 原始版本串，可能含 v/hash/dirty。
    pub Version: String,
    /// 标签列表；用于 TiFlash 识别。
    pub Labels: Vec<StoreLabel>,
}

impl Store {
    /// 返回 Store Id，对齐 Go getter。
    pub fn GetId(&self) -> u64 {
        self.Id
    }
    /// 返回地址字符串。
    pub fn GetAddress(&self) -> &str {
        &self.Address
    }
    /// 返回节点间通信地址，对齐 protobuf `GetPeerAddress()`。
    pub fn GetPeerAddress(&self) -> &str {
        &self.PeerAddress
    }
    /// 返回原始版本字符串（可能含 v/hash 后缀）。
    pub fn GetVersion(&self) -> &str {
        &self.Version
    }
}

/// PdClient stand-in: only GetAllStores is required.
/// 版本检查只需拉取全部 Store；exclude_tombstone 应对齐 Go 传 true。
pub trait PdClient: Send + Sync {
    fn GetAllStores(&self, exclude_tombstone: bool) -> Result<Vec<Store>, SharedError>;
}

/// 判断是否 TiFlash 引擎节点，对齐 `engine.IsTiFlash`。
fn is_tiflash(store: &Store) -> bool {
    // Mirrors pkg/util/engine.IsTiFlash (tiflash | tiflash_compute).
    // engine 标签为 tiflash 或 tiflash_compute 即视为 TiFlash。
    store
        .Labels
        .iter()
        .any(|l| l.Key == "engine" && matches!(l.Value.as_str(), "tiflash" | "tiflash_compute"))
}

/// 包装为 ErrVersionMismatch 注解错误，统一版本不匹配错误码。
fn mismatch(msg: impl Into<String>) -> SharedError {
    Annotate(Some(SharedError::new((*ErrVersionMismatch).clone())), msg).expect("annotate")
}

/// NextMajorVersion returns the next major version.
/// 解析当前发布版本并 +1 major；不可解析时返回“无穷新” nightly。
pub fn NextMajorVersion() -> Version {
    match Version::parse(&removeVAndHash(&effective_release_version())) {
        Ok(mut v) => {
            v.major += 1;
            v.minor = 0;
            v.patch = 0;
            v.pre = semver::Prerelease::EMPTY;
            v.build = semver::BuildMetadata::EMPTY;
            v
        }
        Err(_) => {
            // infinitely-new nightly
            // 脏/不可解析版本视为无限新的 nightly，避免误杀兼容检查。
            let mut v = Version::new(i64::MAX as u64, 0, 0);
            v.pre = semver::Prerelease::new("nightly").unwrap_or_default();
            v
        }
    }
}

/// removeVAndHash sanitizes a version string.
/// 去掉 `-N-gHASH`、`-dirty` 与前缀 `v`，得到可 parse 的 semver 核心。
pub fn removeVAndHash(v: &str) -> String {
    let v = version_hash_re().replace_all(v, "");
    let v = v.strip_suffix("-dirty").unwrap_or(&v);
    v.strip_prefix('v').unwrap_or(v).to_string()
}

/// 校验 TiFlash 在 major 3/4 上的最低兼容版本。
fn check_tiflash_version(store: &Store) -> Result<(), SharedError> {
    let flash = Version::parse(&removeVAndHash(&store.Version)).map_err(|err| {
        mismatch(format!(
            "failed to parse TiFlash {} version {}, err {}",
            store.GetPeerAddress(),
            store.Version,
            err
        ))
    })?;
    // major=3 且低于 3.1.0 → 不兼容。
    if flash.major == 3 && flash < compatible_tiflash_major3() {
        return Err(mismatch(format!(
            "incompatible TiFlash {} version {}, try update it to {}",
            store.GetPeerAddress(),
            store.Version,
            compatible_tiflash_major3()
        )));
    }
    // major=4 且低于 4.0.0 → 不兼容。
    if flash.major == 4 && flash < compatible_tiflash_major4() {
        return Err(mismatch(format!(
            "incompatible TiFlash {} version {}, try update it to {}",
            store.GetPeerAddress(),
            store.Version,
            compatible_tiflash_major4()
        )));
    }
    Ok(())
}

/// VerChecker decides whether the cluster is suitable to execute restore.
/// 闭包形式的单节点版本判定器，供 CheckClusterVersion 注入不同策略。
pub type VerChecker = Arc<dyn Fn(&Store, &Version) -> Result<(), SharedError> + Send + Sync>;

/// CheckClusterVersion check TiKV version.
/// 拉取全部 Store：TiFlash 走专用检查，其余解析后交给 checker。
pub fn CheckClusterVersion(
    client: &dyn PdClient,
    checker: &dyn Fn(&Store, &Version) -> Result<(), SharedError>,
) -> Result<(), SharedError> {
    let stores = client.GetAllStores(true)?;
    for s in &stores {
        if is_tiflash(s) {
            // TiFlash 不参与 TiKV/BR 主检查路径。
            check_tiflash_version(s)?;
            continue;
        }
        let tikv_version_string = removeVAndHash(&s.Version);
        let tikv_version = Version::parse(&tikv_version_string).map_err(|get_version_err| {
            mismatch(format!(
                "{get_version_err}: TiKV node {} version {tikv_version_string} is invalid",
                s.Address
            ))
        })?;
        checker(s, &tikv_version)?;
    }
    Ok(())
}

/// CheckVersionForBackup checks the version for backup.
/// 备份集群 major 比目标高超过 1 则拒绝恢复（跨大版本风险）。
pub fn CheckVersionForBackup(backup_version: Version) -> VerChecker {
    Arc::new(move |_store: &Store, ver: &Version| {
        if backup_version.major > ver.major && backup_version.major - ver.major > 1 {
            return Err(mismatch(format!(
                "backup with cluster version {backup_version} cannot be restored at cluster of version {ver}: major version mismatches"
            )));
        }
        Ok(())
    })
}

/// CheckVersionForBRPiTR checks PiTR compatibility.
/// PiTR：要求 TiKV≥6.1；BR 6.1 必须与 TiKV 6.1 对齐；更高 BR 要求 TiKV≥6.2。
/// 同时更新 batch KV files 支持标志（TiKV≥6.5 为 true）。
pub fn CheckVersionForBRPiTR(s: &Store, tikv_version: &Version) -> Result<(), SharedError> {
    // 测试回落版本跳过真实检查，对齐 Go 对 ReleaseVersionForTest 的短路。
    if effective_release_version() == build::ReleaseVersionForTest {
        return Ok(());
    }
    let br_version = Version::parse(&removeVAndHash(&effective_release_version())).map_err(|err| {
        mismatch(format!(
            "{err}: invalid version, please recompile using `git fetch origin --tags && make build`"
        ))
    })?;

    // PiTR 最低要求 v6.1.0（文案仍推荐 v6.2.0+）。
    if tikv_version.major < 6 || (tikv_version.major == 6 && tikv_version.minor == 0) {
        return Err(mismatch(format!(
            "TiKV node {} version {tikv_version} is too low when use PiTR, please update tikv's version to at least v6.1.0(v6.2.0+ recommanded)",
            s.Address
        )));
    }
    {
        // TiKV < 6.5 不支持 PiTR 批量 KV 文件。
        let mut flag = pitr_support_batch_kv_files().lock().unwrap();
        *flag = !(tikv_version.major < 6 || (tikv_version.major == 6 && tikv_version.minor < 5));
    }

    // BR 6.1 必须与 TiKV 6.1 成对；更高 BR 则拒绝 TiKV≤6.1。
    if br_version.major == 6 && br_version.minor == 1 {
        if tikv_version.major != 6 || tikv_version.minor != 1 {
            return Err(mismatch(format!(
                "TiKV node {} version {tikv_version} and BR {} version mismatch when use PiTR v6.1.0, please use the same version of BR",
                s.Address,
                effective_release_version()
            )));
        }
    } else if tikv_version.major == 6 && tikv_version.minor <= 1 {
        return Err(mismatch(format!(
            "TiKV node {} version {tikv_version} and BR {} version mismatch when use PiTR v6.2.0+, please use the tikv with version v6.2.0+",
            s.Address,
            effective_release_version()
        )));
    }
    Ok(())
}

/// CheckVersionForDDL checks DDL queue vs table mode.
/// DDL 表模式要求集群 ≥ 6.2.0-alpha。
pub fn CheckVersionForDDL(_s: &Store, tikv_version: &Version) -> Result<(), SharedError> {
    let require = Version::parse("6.2.0-alpha").unwrap();
    if *tikv_version < require {
        return Err(astersql_errors::New(format!(
            "detected the old version of tidb cluster, require: >= 6.2.0, but got {tikv_version}"
        )));
    }
    Ok(())
}

/// CheckVersionForKeyspaceBR checks keyspace BR support.
/// Keyspace BR 要求集群 ≥ 6.6.0-alpha。
pub fn CheckVersionForKeyspaceBR(_s: &Store, tikv_version: &Version) -> Result<(), SharedError> {
    let require = Version::parse("6.6.0-alpha").unwrap();
    if *tikv_version < require {
        return Err(astersql_errors::New(format!(
            "detected the old version of tidb cluster, require: >= 6.6.0, but got {tikv_version}"
        )));
    }
    Ok(())
}

/// CheckVersionForBR checks BR/cluster compatibility.
/// 校验最低 TiKV、major 差、3.x/4.x 细粒度不兼容，并更新 checkpoint 错误缓存。
pub fn CheckVersionForBR(s: &Store, tikv_version: &Version) -> Result<(), SharedError> {
    // 测试回落版本短路，避免 nightly-dirty 触发真实不匹配。
    if effective_release_version() == build::ReleaseVersionForTest {
        return Ok(());
    }
    let br_version = Version::parse(&removeVAndHash(&effective_release_version())).map_err(|err| {
        mismatch(format!(
            "{err}: invalid version, please recompile using `git fetch origin --tags && make build`"
        ))
    })?;

    // 低于最低 TiKV 则完全不支持 BR。
    if *tikv_version < min_tikv_version() {
        return Err(mismatch(format!(
            "TiKV node {} version {tikv_version} don't support BR, please upgrade cluster to {}",
            s.Address,
            effective_release_version()
        )));
    }

    // BR major 不得落后 TiKV，且领先不得超过 2。
    if br_version.major < tikv_version.major || br_version.major - tikv_version.major > 2 {
        return Err(mismatch(format!(
            "TiKV node {} version {tikv_version} and BR {} major version mismatch, please use the same version of BR",
            s.Address,
            effective_release_version()
        )));
    }

    // 3.x：新 BR 配旧于 3.1.0 的 TiKV 不兼容。
    if tikv_version.major == 3
        && *tikv_version < incompatible_tikv_major3()
        && br_version >= incompatible_tikv_major3()
    {
        return Err(mismatch(format!(
            "TiKV node {} version {tikv_version} and BR {} version mismatch, please use the same version of BR",
            s.Address,
            effective_release_version()
        )));
    }

    // 4.x：新 BR 配旧于 4.0.0-rc.1 的 TiKV 不兼容。
    if tikv_version.major == 4
        && *tikv_version < incompatible_tikv_major4()
        && br_version >= incompatible_tikv_major4()
    {
        return Err(mismatch(format!(
            "TiKV node {} version {tikv_version} and BR {} version mismatch, please use the same version of BR",
            s.Address,
            effective_release_version()
        )));
    }

    {
        // checkpoint 要求 TiKV≥6.5；不足时缓存错误供 CheckCheckpointSupport 读取。
        let mut err = checkpoint_support_error().lock().unwrap();
        *err = None;
        if tikv_version.major < 6 || (tikv_version.major == 6 && tikv_version.minor < 5) {
            *err = Some(mismatch(format!(
                "TiKV node {} version {tikv_version} is too low when use checkpoint, please update tikv's version to at least v6.5.0",
                s.Address
            )));
        }
    }

    // Go 在非 master 且 tikv>BR 时告警；保留调用以对齐副作用位点。
    let _ = git_branch(); // Go warns when branch != master && tikv > BR; keep call for parity side-effect site.
    Ok(())
}

/// CheckVersion checks if actual is within [requiredMin, requiredMax).
/// 半开区间：过旧按完整区间报错；过新仅比较 major（含 pre-release 触达上界）。
pub fn CheckVersion(
    component: &str,
    actual: &Version,
    required_min: &Version,
    required_max: &Version,
) -> Result<(), SharedError> {
    if actual < required_min {
        return Err(mismatch(format!(
            "{component} version too old, required to be in [{required_min}, {required_max}), found '{actual}'"
        )));
    }
    // 与 Go 一致：用 major 判断上界，使 6.0.0-beta 相对 required_max=6.0.0 视为过新。
    if actual.major >= required_max.major {
        return Err(mismatch(format!(
            "{component} version too new, expected to be within [{required_min}, {}.0.0), found '{actual}'",
            required_max.major
        )));
    }
    Ok(())
}

/// ExtractTiDBVersion extracts TiDB version from `version()` outputs.
/// 从 `5.7.x-TiDB-vX.Y.Z[-hash][-dirty]` 形态提取 semver；段数不合规则报错。
pub fn ExtractTiDBVersion(version: &str) -> Result<Version, SharedError> {
    let trimmed = version.strip_suffix("-dirty").unwrap_or(version);
    let versions: Vec<&str> = trimmed.split('-').collect();
    // 3/4 段取至末尾；5/6 段去掉末尾 hash 两段；其他长度非法。
    let end = match versions.len() {
        3 | 4 => versions.len(),
        5 | 6 => versions.len() - 2,
        _ => {
            return Err(mismatch(format!("not a valid TiDB version: {version}")));
        }
    };
    let mut raw = versions[2..end].join("-");
    if let Some(stripped) = raw.strip_prefix('v') {
        raw = stripped.to_string();
    }
    Version::parse(&raw).map_err(|e| astersql_errors::New(e.to_string()))
}

/// CheckTiDBVersion is ExtractTiDBVersion + CheckVersion via ParseServerInfo.
/// 先 ParseServerInfo 确认是 TiDB，再对解析出版本做区间检查。
pub fn CheckTiDBVersion(
    version_str: &str,
    required_min: Version,
    required_max: Version,
) -> Result<(), SharedError> {
    let server_info = ParseServerInfo(version_str);
    if server_info.ServerType != ServerType::TiDB {
        return Err(astersql_errors::New(format!(
            "server with version '{version_str}' is not TiDB"
        )));
    }
    let ver = server_info
        .ServerVersion
        .ok_or_else(|| astersql_errors::New("missing server version"))?;
    CheckVersion("TiDB", &ver, &required_min, &required_max)
}

/// 对齐 Go `strconv.Unquote` 的双引号/反引号字符串解码子集。
/// 备份版本来自 PD JSON，双引号路径需完整处理 Go 支持的字符转义。
fn unquote_go_string(value: &str) -> Option<String> {
    if value.starts_with('`') && value.ends_with('`') && value.len() >= 2 {
        let raw = &value[1..value.len() - 1];
        if raw.contains('`') {
            return None;
        }
        return Some(raw.replace('\r', ""));
    }
    if !value.starts_with('"') || !value.ends_with('"') || value.len() < 2 {
        return None;
    }

    let body = &value[1..value.len() - 1];
    let mut chars = body.chars().peekable();
    let mut output = String::with_capacity(body.len());
    while let Some(ch) = chars.next() {
        if ch == '\n' || ch == '\r' || ch == '"' {
            return None;
        }
        if ch != '\\' {
            output.push(ch);
            continue;
        }

        let escape = chars.next()?;
        match escape {
            'a' => output.push('\u{7}'),
            'b' => output.push('\u{8}'),
            'f' => output.push('\u{c}'),
            'n' => output.push('\n'),
            'r' => output.push('\r'),
            't' => output.push('\t'),
            'v' => output.push('\u{b}'),
            '\\' => output.push('\\'),
            '"' => output.push('"'),
            'x' | 'u' | 'U' => {
                let digits = match escape {
                    'x' => 2,
                    'u' => 4,
                    'U' => 8,
                    _ => unreachable!(),
                };
                let mut value = 0_u32;
                for _ in 0..digits {
                    value = value
                        .checked_mul(16)?
                        .checked_add(chars.next()?.to_digit(16)?)?;
                }
                output.push(char::from_u32(value)?);
            }
            '0'..='7' => {
                let mut value = escape.to_digit(8)?;
                for _ in 0..2 {
                    value = value
                        .checked_mul(8)?
                        .checked_add(chars.next()?.to_digit(8)?)?;
                }
                if value > u8::MAX as u32 {
                    return None;
                }
                output.push(char::from_u32(value)?);
            }
            _ => return None,
        }
    }
    Some(output)
}

/// NormalizeBackupVersion normalizes the version string from backupmeta.
/// 先按 Go `strconv.Unquote` 解码，再去空白并 parse；失败返回 None。
pub fn NormalizeBackupVersion(version: &str) -> Option<Version> {
    let trimmed = version.trim();
    let unquoted = unquote_go_string(trimmed).unwrap_or_else(|| trimmed.to_owned());
    let normalized = unquoted.trim();
    Version::parse(normalized).ok()
}

/// QueryExecutor stand-in for FetchVersion.
/// 仅需 QueryRow，对应 Go dbutil 查询一行的最小接口。
pub trait QueryExecutor: Send + Sync {
    fn QueryRow(&self, sql: &str) -> Result<String, SharedError>;
}

/// 匹配 tidb_version() 完整输出中的 Release Version / CLOUD 行。
fn tidb_release_version_full_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"Release Version:\s*(v\d+\.\d+\.\d+([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?|CLOUD\.\d{6}\.\d+([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?)")
            .unwrap()
    })
}

/// FetchVersion gets version information from the database server.
/// 优先 `tidb_version()` 且需匹配 Release Version 正则；否则回退 `version()`。
pub fn FetchVersion(db: &dyn QueryExecutor) -> Result<String, SharedError> {
    const QUERY_TIDB: &str = "SELECT tidb_version();";
    match db.QueryRow(QUERY_TIDB) {
        // 仅当输出含规范 Release Version 时采纳，避免把 commit-id 误当发布版本。
        Ok(version_info) if tidb_release_version_full_re().is_match(&version_info) => {
            return Ok(version_info);
        }
        _ => {}
    }
    const QUERY: &str = "SELECT version();";
    db.QueryRow(QUERY)
        .map_err(|e| Annotate(Some(e), format!("sql: {QUERY}")).expect("annotate"))
}

/// 读取 CheckVersionForBR 缓存的 checkpoint 支持错误。
pub fn CheckCheckpointSupport() -> Result<(), SharedError> {
    match checkpoint_support_error().lock().unwrap().clone() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// 读取 PiTR 批量 KV 文件支持标志。
pub fn CheckPITRSupportBatchKVFiles() -> bool {
    *pitr_support_batch_kv_files().lock().unwrap()
}

/// 测试注入 PiTR batch KV 标志，便于用例预设前置状态。
#[cfg(test)]
pub(crate) fn SetPITRSupportBatchKVFilesForTest(supported: bool) {
    *pitr_support_batch_kv_files().lock().unwrap() = supported;
}

/// 服务器类型枚举，对齐 Go `mysql.ServerType` 取值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerType {
    Unknown = 0,
    MySQL = 1,
    MariaDB = 2,
    TiDB = 3,
    All = 4,
}

impl ServerType {
    /// 展示名；All 返回空串，对齐 Go String()。
    pub fn String(self) -> &'static str {
        match self {
            ServerType::Unknown => "Unknown",
            ServerType::MySQL => "MySQL",
            ServerType::MariaDB => "MariaDB",
            ServerType::TiDB => "TiDB",
            ServerType::All => "",
        }
    }
}

/// 解析后的服务器信息；HasTiKV 字段保留以对齐 Go 结构，本文件未填充。
#[derive(Clone, Debug)]
pub struct ServerInfo {
    /// 解析出的服务器类型。
    pub ServerType: ServerType,
    /// 解析出的 semver；失败时为 0.0.0。
    pub ServerVersion: Option<Version>,
    /// Go 结构对齐字段；本解析路径不填充。
    pub HasTiKV: bool,
}

impl Default for ServerInfo {
    /// 默认 Unknown / 无版本 / 无 TiKV 标记。
    fn default() -> Self {
        Self {
            ServerType: ServerType::Unknown,
            ServerVersion: None,
            HasTiKV: false,
        }
    }
}

/// 匹配 MySQL/通用 `X.Y.Z[...]` 版本前缀。
fn mysql_version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\d+\.\d+\.\d+([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?").unwrap())
}
/// 匹配 `version()` 中 `-vX.Y.Z` / `-X.Y.Z` TiDB 段。
fn tidb_version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"-[v]?\d+\.\d+\.\d+([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?").unwrap())
}
/// 匹配 Release Version 行中的 `vX.Y.Z...`。
fn tidb_release_version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"v\d+\.\d+\.\d+([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?").unwrap())
}
/// 匹配 `TiDB-CLOUD.YYYYMM.patch` 服务端版本串。
fn tidb_cloud_server_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"TiDB-CLOUD\.(\d{4})(\d{2})\.(\d+)([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?").unwrap()
    })
}
/// 匹配 Release Version 行中的 `CLOUD.YYYYMM.patch`。
fn tidb_cloud_release_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"Release Version:\s*CLOUD\.(\d{4})(\d{2})\.(\d+)([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?",
        )
        .unwrap()
    })
}

/// Matches `mysql.TiDBXVerMinYear` / `TiDBXVerMaxYear`.
/// TiDB Cloud/X 年份窗口；窗外版本拒绝转换为 semver。
const TIDB_X_VER_MIN_YEAR: u64 = 2025;
const TIDB_X_VER_MAX_YEAR: u64 = 2099;

/// 将 CLOUD.YYYYMM.patch 映射为 `(year-2000).month.patch[+suffix]` semver 串。
/// 年份/月份非法或未匹配正则时返回空串，由调用方回落 0.0.0。
fn parse_tidbx_version_to_semver(version_str: &str) -> String {
    let caps = tidb_cloud_server_re()
        .captures(version_str)
        .or_else(|| tidb_cloud_release_re().captures(version_str));
    let Some(caps) = caps else {
        return String::new();
    };
    if caps.len() < 5 {
        return String::new();
    }
    let year: u64 = caps[1].parse().unwrap_or(0);
    if !(TIDB_X_VER_MIN_YEAR..=TIDB_X_VER_MAX_YEAR).contains(&year) {
        return String::new();
    }
    let month: u64 = caps[2].parse().unwrap_or(0);
    if !(1..=12).contains(&month) {
        return String::new();
    }
    let patch: u64 = caps[3].parse().unwrap_or(0);
    let suffix = caps.get(4).map(|m| m.as_str()).unwrap_or("");
    // 年份映射到 major=year-2000，与 Go mysql 包一致。
    format!("{}.{}.{}{}", year - 2000, month, patch, suffix)
}

/// ParseServerInfo parses exported server type and version info from version string.
/// 按关键字判定类型；TiDB 优先 Release Version / `-vX.Y.Z` / Cloud 映射。
/// 解析失败时 ServerVersion 回落 0.0.0，与 Go 行为一致。
pub fn ParseServerInfo(src: &str) -> ServerInfo {
    let lower = src.to_lowercase();
    let mut server_info = ServerInfo::default();
    let mut is_release_version = false;
    // 类型识别顺序：Release Version → tidb → mariadb → mysql 数字 → Unknown。
    if lower.contains("release version:") {
        server_info.ServerType = ServerType::TiDB;
        is_release_version = true;
    } else if lower.contains("tidb") {
        server_info.ServerType = ServerType::TiDB;
    } else if lower.contains("mariadb") {
        server_info.ServerType = ServerType::MariaDB;
    } else if mysql_version_re().is_match(&lower) {
        server_info.ServerType = ServerType::MySQL;
    } else {
        server_info.ServerType = ServerType::Unknown;
    }

    let mut version_str = String::new();
    if server_info.ServerType == ServerType::TiDB {
        if is_release_version {
            if let Some(m) = tidb_release_version_re().find(src) {
                version_str = m.as_str().to_string();
            }
        } else if let Some(m) = tidb_version_re().find(src) {
            version_str = m.as_str().trim_start_matches('-').to_string();
        }
        // 常规正则未命中时尝试 Cloud/X 年月映射。
        if version_str.is_empty() {
            version_str = parse_tidbx_version_to_semver(src);
        }
        if let Some(stripped) = version_str.strip_prefix('v') {
            version_str = stripped.to_string();
        }
    } else if let Some(m) = mysql_version_re().find(src) {
        version_str = m.as_str().to_string();
    }

    // 无法 parse 时填 0.0.0，避免 Option 空值破坏调用方。
    server_info.ServerVersion =
        Some(Version::parse(&version_str).unwrap_or_else(|_| Version::new(0, 0, 0)));
    server_info
}
