// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Test backup with key locked errors.
//
// This file is copied from pingcap/schrodinger-test#428 https://git.io/Je1md

//! Lock generator for key-locked backup tests — port of
//! `br/tests/br_key_locked/locker.go`.
//!
//! 本模块实现 key-locked 集成测试的加锁负载发生器，对照 Go `locker.go`。
//! 职责：解析 CLI 参数 → 经 TiDB status 取 table id → 向 TiKV 发 Prewrite 留下锁。
//! 约束：故意不 Commit/Rollback，以便备份路径观察到 key locked；非生产路径。
//! 依赖边界：真实 PD/TiKV 拨号在二进制入口被 stub，单元测试经 `run_with` 注入。
//! 数据流：Config → getTableID(HTTP) → Locker::generateLocks → lockBatch(Prewrite)。
//! 与 Go 对齐：flag 默认值、schema URL、region 定位与 Prewrite 语义保持一致。

use std::cmp::Ordering;
use std::time::Duration;

use crate::codec::CodecPDClient;
use crate::stubs::{
    self, Backoffer, BoRegionMiss, Context, Error, HttpClient, Intn, NewBackoffer, NewConfig,
    PdClient, Result, Storage, StoreGlobalConfig, StubHttpClient, TLSConfig, TableInfo, TidbConfig,
    kvrpcpb, oracle, tablecodec, tikvrpc,
};

/// CLI / runtime configuration matching Go flag defaults.
/// 运行时配置：字段与 Go `flag` 变量一一对应（ca/cert/key、tidb/pd、db/table 等）。
/// `table_size`/`timeout`/`lock_ttl` 默认值对齐 Go（10000、10s、10s）。
/// `validate` 要求 tidb/pd/db/table 非空，与 Go `main` 中 panic 前置检查一致。
#[derive(Clone, Debug)]
pub struct Config {
    /// TLS CA 路径；非空时启用集群 TLS，对照 Go `-ca`。
    pub ca: String,
    /// 客户端证书路径，对照 Go `-cert`。
    pub cert: String,
    /// 客户端私钥路径，对照 Go `-key`。
    pub key: String,
    /// TiDB status 地址，用于拉取 schema/table id。
    pub tidb_status_addr: String,
    /// PD 地址；真实拨号在 stub 边界外，测试经注入绕过。
    pub pd_addr: String,
    /// 目标库名，参与 schema URL。
    pub db_name: String,
    /// 目标表名，参与 schema URL。
    pub table_name: String,
    /// 表行数上界；`generateLocks` 用其对 row_id 取模循环。
    pub table_size: i64,
    /// 总运行超时；映射到 `Context::WithTimeout`。
    pub timeout: Duration,
    /// Prewrite 锁 TTL，写入 `LockTtl` 毫秒字段。
    pub lock_ttl: Duration,
}

impl Default for Config {
    /// 默认值对齐 Go flag 默认（空串 + table_size=10000 + 10s 超时/TTL）。
    fn default() -> Self {
        Self {
            ca: String::new(),
            cert: String::new(),
            key: String::new(),
            tidb_status_addr: String::new(),
            pd_addr: String::new(),
            db_name: String::new(),
            table_name: String::new(),
            table_size: 10000,
            timeout: Duration::from_secs(10),
            lock_ttl: Duration::from_secs(10),
        }
    }
}

impl Config {
    /// Parse argv in Go `flag` style (`-name value` / `-name=value`).
    /// 解析 argv：支持 `-name value` 与 `-name=value`，跳过 argv[0]。
    /// 未知 flag 或缺值返回错误；时长走 Go 风格 `parse_go_duration`。
    /// 不改写环境，仅构造 Config，便于 parity 单测直接喂入参数切片。
    pub fn parse_args<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut cfg = Self::default();
        let mut iter = args.into_iter().peekable();
        // skip program name if present
        if iter.peek().is_some() {
            let _ = iter.next();
        }
        while let Some(arg) = iter.next() {
            let arg = arg.as_ref();
            if arg == "--" || !arg.starts_with('-') {
                // Go's flag package stops at the first positional argument or `--`.
                break;
            }
            let (name, inline) =
                if let Some(rest) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) {
                    if let Some((n, v)) = rest.split_once('=') {
                        (n, Some(v.to_string()))
                    } else {
                        (rest, None)
                    }
                } else {
                    unreachable!();
                };
            let value = match inline {
                Some(v) => v,
                None => iter
                    .next()
                    .ok_or_else(|| Error::new(format!("missing value for -{name}")))?
                    .as_ref()
                    .to_string(),
            };
            match name {
                "ca" => cfg.ca = value,
                "cert" => cfg.cert = value,
                "key" => cfg.key = value,
                "tidb" => cfg.tidb_status_addr = value,
                "pd" => cfg.pd_addr = value,
                "db" => cfg.db_name = value,
                "table" => cfg.table_name = value,
                "table-size" => {
                    cfg.table_size = value
                        .parse()
                        .map_err(|_| Error::new("invalid -table-size"))?;
                }
                "run-timeout" => {
                    cfg.timeout = parse_go_duration(&value)?;
                }
                "lock-ttl" => {
                    cfg.lock_ttl = parse_go_duration(&value)?;
                }
                other => return Err(Error::new(format!("unknown flag -{other}"))),
            }
        }
        Ok(cfg)
    }

    /// 校验必填字段；空串错误文案与 Go `log.Panic` 前置检查一致。
    pub fn validate(&self) -> Result<()> {
        if self.tidb_status_addr.is_empty() {
            return Err(Error::new("tidb status address is empty"));
        }
        if self.pd_addr.is_empty() {
            return Err(Error::new("pd address is empty"));
        }
        if self.db_name.is_empty() {
            return Err(Error::new("database name is empty"));
        }
        if self.table_name.is_empty() {
            return Err(Error::new("table name is empty"));
        }
        Ok(())
    }
}

/// 解析 Go `time.ParseDuration` 的非负时长字面量，供 CLI flag 使用。
/// 无法识别的格式返回错误，避免静默落到零时长。
fn parse_go_duration(s: &str) -> Result<Duration> {
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    let s = s.strip_prefix('+').unwrap_or(s);
    if s.is_empty() || s.starts_with('-') {
        return Err(Error::new(format!("invalid duration {s}")));
    }

    let bytes = s.as_bytes();
    let mut pos = 0usize;
    let mut total_nanos = 0u128;
    while pos < bytes.len() {
        let number_start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        let integer_end = pos;
        let mut fraction_start = None;
        if pos < bytes.len() && bytes[pos] == b'.' {
            pos += 1;
            fraction_start = Some(pos);
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
            }
        }
        if integer_end == number_start && fraction_start == Some(pos) {
            return Err(Error::new(format!("invalid duration {s}")));
        }

        let unit_start = pos;
        while pos < bytes.len() && !bytes[pos].is_ascii_digit() && bytes[pos] != b'.' {
            pos += 1;
        }
        let unit = &s[unit_start..pos];
        let unit_nanos = match unit {
            "h" => 3_600_000_000_000u128,
            "m" => 60_000_000_000,
            "s" => 1_000_000_000,
            "ms" => 1_000_000,
            "us" | "µs" | "μs" => 1_000,
            "ns" => 1,
            _ => return Err(Error::new(format!("invalid duration {s}"))),
        };
        let whole = if integer_end == number_start {
            0
        } else {
            s[number_start..integer_end]
                .parse::<u128>()
                .map_err(|_| Error::new(format!("invalid duration {s}")))?
        };
        let mut component = whole
            .checked_mul(unit_nanos)
            .ok_or_else(|| Error::new(format!("invalid duration {s}")))?;
        if let Some(fraction_start) = fraction_start {
            let fraction = &s[fraction_start..unit_start];
            if fraction.is_empty() {
                return Err(Error::new(format!("invalid duration {s}")));
            }
            let numerator = fraction
                .parse::<u128>()
                .map_err(|_| Error::new(format!("invalid duration {s}")))?;
            let denominator = 10u128
                .checked_pow(fraction.len() as u32)
                .ok_or_else(|| Error::new(format!("invalid duration {s}")))?;
            component = component
                .checked_add(numerator.saturating_mul(unit_nanos) / denominator)
                .ok_or_else(|| Error::new(format!("invalid duration {s}")))?;
        }
        total_nanos = total_nanos
            .checked_add(component)
            .ok_or_else(|| Error::new(format!("invalid duration {s}")))?;
    }
    let nanos =
        u64::try_from(total_nanos).map_err(|_| Error::new(format!("invalid duration {s}")))?;
    Ok(Duration::from_nanos(nanos))
}

/// Build TiDB schema status URL: `https://{host}:10080/schema/{db}/{table}`.
/// 构造 schema 查询 URL：主机取自 status 地址，端口固定 10080（对齐 Go）。
/// 丢弃原端口是故意行为：status 服务与 SQL 端口分离。
pub fn build_schema_url(db_addr: &str, db_name: &str, table: &str) -> Result<String> {
    let (db_host, _) = split_host_port(db_addr)?;
    let db_status_addr = join_host_port(&db_host, "10080");
    Ok(format!("https://{db_status_addr}/schema/{db_name}/{table}"))
}

/// 拆分 host:port；优先括号 IPv6，再回退最后一个 `:`。
/// 空 host/port 或未加括号的多段 IPv6 视为非法，防止静默错连。
fn split_host_port(addr: &str) -> Result<(String, String)> {
    // Prefer last ':' so IPv6 "[::1]:4000" still works when bracketed.
    // 括号形式先切，避免 IPv6 内部冒号干扰。
    if let Some(bracket_end) = addr.find(']') {
        if addr.as_bytes().get(bracket_end + 1) == Some(&b':') {
            let host = addr[..=bracket_end].to_string();
            let port = addr[bracket_end + 2..].to_string();
            if port.is_empty() {
                return Err(Error::new(format!("missing port in address {addr}")));
            }
            return Ok((host, port));
        }
    }
    match addr.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && !port.is_empty() && !port.contains(':') => {
            Ok((host.to_string(), port.to_string()))
        }
        _ => Err(Error::new(format!("invalid address {addr}"))),
    }
}

/// 拼接 host:port；裸 IPv6（含冒号且无括号）自动加 `[]`，对齐 `net.JoinHostPort`。
fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Parse TableInfo JSON body and return `ID` (Go `json.Unmarshal` into model.TableInfo).
/// 从 schema JSON 解出表 ID；字段名 `ID` 对齐 Go `model.TableInfo`。
/// 反序列化失败经 `Error::Trace` 包装，便于上层 Annotate。
pub fn parse_table_id_from_schema_body(body: &[u8]) -> Result<i64> {
    let data: TableInfo =
        serde_json::from_slice(body).map_err(|e| Error::Trace(Error::new(e.to_string())))?;
    Ok(data.ID)
}

/// getTableID of the table with specified table name.
/// 经 HTTP GET schema URL 取 table id；非 200 带 body 报错，对齐 Go。
/// `HttpClient` 可注入，便于 parity 用假响应测 URL 与解析路径。
pub fn getTableID<H: HttpClient>(
    ctx: &Context,
    db_addr: &str,
    db_name: &str,
    table: &str,
    client: &H,
) -> Result<i64> {
    let url = build_schema_url(db_addr, db_name, table)?;
    let (status, body) = client.DoGet(ctx, &url).map_err(Error::Trace)?;
    if status != 200 {
        return Err(Error::Errorf(format!(
            "HTTP request to TiDB status reporter returns {}. Body: {}",
            status,
            String::from_utf8_lossy(&body)
        )));
    }
    parse_table_id_from_schema_body(&body).map_err(Error::Trace)
}

/// 构造 HTTP 客户端桩：若配置了 CA 则先校验 TLS 材料可解析。
/// 当前返回 `StubHttpClient`，真实网络 GET 在测试注入路径完成。
fn new_http_client(cfg: &Config) -> Result<StubHttpClient> {
    if !cfg.ca.is_empty() {
        let tls_cfg = TLSConfig {
            CA: cfg.ca.clone(),
            Cert: cfg.cert.clone(),
            Key: cfg.key.clone(),
        };
        // 仅验证配置可解析；不在此建立真实 TLS 连接。
        tls_cfg
            .ToTLSConfig()
            .map_err(|e| Error::new(format!("fail to parse TLS config: {e}")))?;
    }
    Ok(StubHttpClient)
}

/// Locker leaves locks on a table.
/// 在表上留下未提交 Prewrite 锁，制造备份时的 key locked 场景。
/// 泛型 `P`/`S` 便于注入假 PD 与 Storage，不依赖真实集群。
pub struct Locker<P: PdClient, S: Storage> {
    /// 表 ID，用于编码 record key 前缀。
    pub table_id: i64,
    /// 行号循环上界（取模）。
    pub table_size: i64,
    /// 写入 Prewrite 的锁 TTL。
    pub lock_ttl: Duration,
    /// PD 客户端（通常包一层 CodecPDClient）。
    pub pdcli: P,
    /// TiKV Storage：提供 region cache 与 SendReq。
    pub kv: S,
}

impl<P: PdClient, S: Storage> Locker<P, S> {
    /// generateLocks sends Prewrite requests to TiKV to generate locks, without
    /// committing and rolling back.
    /// 循环扫描 row_id，随机决定是否加锁，凑够一批后 `lockKeys`。
    /// `pctx.Done()` 时退出；内部另起可取消 ctx 供单次 Prewrite 使用。
    /// 事务大小随机且至少 1，避免空 Prewrite；与 Go 随机策略一致。
    pub fn generateLocks(&self, pctx: &Context) -> Result<()> {
        log_info("genLock started");

        const MAX_TXN_SIZE: i32 = 1000;

        // How many keys should be in the next transaction.
        // 下一批事务键数：`Intn(MAX)+1` 保证非零。
        let mut next_txn_size = Intn(MAX_TXN_SIZE) + 1; // 0 is not allowed.

        // How many keys has been scanned since last time sending request.
        // 自上次发送后已扫描键数（保留以对齐 Go 局部变量）。
        let mut _scanned_keys = 0i32;
        let mut batch: Vec<i64> = Vec::new();

        let (ctx, _cancel) = Context::WithCancel(&Context::Background());
        let mut row_id: i64 = 0;
        loop {
            // 父上下文超时/取消则干净结束，不强制刷剩余 batch。
            if pctx.Done() {
                log_info("genLock done");
                return Ok(());
            }

            _scanned_keys += 1;

            // Randomly decide whether to lock current key.
            // 50% 概率锁定当前行，制造稀疏锁分布。
            let lock_this = Intn(2) == 0;

            if lock_this {
                batch.push(row_id);

                if batch.len() >= next_txn_size as usize {
                    // The batch is large enough to start the transaction
                    // 批次已满：发 Prewrite，再重置随机下一批大小。
                    self.lockKeys(&ctx, &batch)
                        .map_err(|e| Error::Annotate(e, "lock keys failed"))?;

                    // Start the next loop
                    batch.clear();
                    _scanned_keys = 0;
                    next_txn_size = Intn(MAX_TXN_SIZE) + 1;
                }
            }
            // 在 [0, table_size) 环形推进，长时间运行可重复覆盖行。
            row_id = (row_id + 1) % self.table_size;
        }
    }

    /// 将 row_id 编码为 record key，以首键为 primary，分批 `lockBatch` 直至耗尽。
    /// 跨 region 时由 `lockBatch` 返回已处理前缀长度，循环推进剩余键。
    pub fn lockKeys(&self, ctx: &Context, row_ids: &[i64]) -> Result<()> {
        let mut keys: Vec<Vec<u8>> = Vec::with_capacity(row_ids.len());

        let key_prefix = tablecodec::GenTableRecordPrefix(self.table_id);
        for &row_id in row_ids {
            let key = tablecodec::EncodeRecordKey(&key_prefix, row_id);
            keys.push(key);
        }

        // primary 取首键，整批 Prewrite 共享同一 PrimaryLock。
        let primary = keys[0].clone();

        while !keys.is_empty() {
            let locked_keys = self.lockBatch(ctx, &keys, &primary)?;
            keys = keys[locked_keys..].to_vec();
        }
        Ok(())
    }

    /// 在单一 region 内组装 mutation 并发送 Prewrite；遇 region error 退避重试。
    /// 批次大小受 region 边界与 16KiB 载荷上限双重约束；对齐 Go `lockBatch`。
    /// 故意忽略 key error：本场景永不提交，不要求事务一致性。
    pub fn lockBatch(&self, ctx: &Context, keys: &[Vec<u8>], primary: &[u8]) -> Result<usize> {
        const MAX_BATCH_SIZE: usize = 16 * 1024;

        // TiKV client doesn't expose Prewrite interface directly. We need to manually
        // locate the region and send the Prewrite requests.
        // 客户端无高层 Prewrite API，需手动 LocateKey + SendReq。
        let bo = NewBackoffer(ctx.clone(), 20000);
        loop {
            let loc = self
                .kv
                .GetRegionCache()
                .LocateKey(&bo, &keys[0])
                .map_err(Error::Trace)?;

            // Get a timestamp to use as the startTs
            // 每次批次取新 TS，避免与已有锁 startTs 冲突策略纠缠。
            let (physical, logical) = self.pdcli.GetTS(ctx).map_err(Error::Trace)?;
            let start_ts = oracle::ComposeTS(physical, logical);

            // Pick a batch of keys and make up the mutations
            // 只收录落在当前 region [StartKey, EndKey) 内的键。
            let mut mutations: Vec<kvrpcpb::Mutation> = Vec::new();
            let mut batch_size = 0usize;

            for key in keys {
                // EndKey 为空表示无上界；否则键须严格小于 EndKey。
                if !loc.EndKey.is_empty() && key.cmp(&loc.EndKey) != Ordering::Less {
                    break;
                }
                if key.cmp(&loc.StartKey) == Ordering::Less {
                    break;
                }

                let value = rand_str();
                mutations.push(kvrpcpb::Mutation {
                    Op: kvrpcpb::Op::Put,
                    Key: key.clone(),
                    Value: value.into_bytes(),
                });
                batch_size += key.len() + mutations.last().unwrap().Value.len();

                if batch_size >= MAX_BATCH_SIZE {
                    break;
                }
            }

            let locked_keys = mutations.len();
            if locked_keys == 0 {
                // 无键可锁：通常因 region 边界与键序不匹配，交由上层处理。
                return Ok(0);
            }

            let prewrite = kvrpcpb::PrewriteRequest {
                Mutations: mutations,
                PrimaryLock: primary.to_vec(),
                StartVersion: start_ts,
                LockTtl: self.lock_ttl.as_millis() as u64,
            };
            let req = tikvrpc::NewRequest(tikvrpc::Cmd::Prewrite, prewrite);

            // Send the requests
            // 发送失败附带 region 与键前缀上下文，便于诊断。
            let resp = self
                .kv
                .SendReq(&bo, req, loc.Region.clone(), Duration::from_secs(20))
                .map_err(|err| {
                    Error::Annotatef(
                        err,
                        format!(
                            "send request failed. region: {:?} [{:?}, {:?}), keys: {:?}",
                            loc.Region,
                            loc.StartKey,
                            loc.EndKey,
                            &keys[0..locked_keys]
                        ),
                    )
                })?;
            let region_err = resp.GetRegionError().map_err(Error::Trace)?;
            if let Some(region_err) = region_err {
                // region miss：退避后重新 LocateKey，不消耗 keys 前缀。
                bo.Backoff(BoRegionMiss(), Error::new(region_err.String()))
                    .map_err(Error::Trace)?;
                continue;
            }

            let prewrite_resp = resp.Resp;
            if prewrite_resp.is_none() {
                return Err(Error::Errorf("response body missing"));
            }

            // Ignore key errors since we never commit the transaction and we don't
            // need to keep consistency here.
            // 忽略 key error：目标只是留下锁，不保证可提交。
            return Ok(locked_keys);
        }
    }
}

/// 生成随机数字串值，长度 `Intn(128)`，用作 Put mutation 的 Value。
fn rand_str() -> String {
    let length = Intn(128);
    let mut res = String::new();
    for _ in 0..length {
        res.push_str(&Intn(10).to_string());
    }
    res
}

/// 保留 Go 调用点的日志钩子；本 crate 日志后端为 no-op 边界。
fn log_info(msg: &str) {
    // Keep call sites; logging backend is a no-op boundary in this crate.
    let _ = msg;
}

/// Run the locker with injected PD + TiKV + HTTP dependencies.
/// 可注入依赖的编排入口：校验配置 → 取 table id → 可选写入全局 TLS → generateLocks。
/// PD 经 `CodecPDClient` 包装以对齐编码键空间；供单元/parity 测试使用。
pub fn run_with<P, S, H>(cfg: &Config, http: &H, pd: P, store: S) -> Result<()>
where
    P: PdClient,
    S: Storage,
    H: HttpClient,
{
    cfg.validate()?;
    let (pctx, _cancel) = Context::WithTimeout(&Context::Background(), cfg.timeout);

    let table_id = getTableID(
        &pctx,
        &cfg.tidb_status_addr,
        &cfg.db_name,
        &cfg.table_name,
        http,
    )
    .map_err(|e| Error::Annotate(e, "get table id failed"))?;

    // Codec 包装：对外暴露编码后的键视图，对齐 Go codecPDClient。
    let pdcli = CodecPDClient::new(pd);

    if !cfg.ca.is_empty() {
        // 非空 CA：写入全局 TiDB 集群 TLS，供后续拨号路径读取。
        let mut tidb_cfg = NewConfig();
        tidb_cfg.ClusterSSLCA = cfg.ca.clone();
        tidb_cfg.ClusterSSLCert = cfg.cert.clone();
        tidb_cfg.ClusterSSLKey = cfg.key.clone();
        StoreGlobalConfig(tidb_cfg);
    }

    let locker = Locker {
        table_id,
        table_size: cfg.table_size,
        lock_ttl: cfg.lock_ttl,
        pdcli,
        kv: store,
    };
    locker
        .generateLocks(&pctx)
        .map_err(|e| Error::Annotate(e, "generate locks failed"))
}

/// Binary entry matching Go `main` argument validation + orchestration.
/// 二进制入口：解析/校验 flag、准备 HTTP/TLS；PD/TiKV 拨号为 stub 边界。
/// 完整加锁需测试侧经 `run_with` 注入 `PdClient`+`Storage`，避免本进程直连集群。
pub fn main_with_args<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let cfg = Config::parse_args(args)?;
    cfg.validate()?;
    let http = new_http_client(&cfg)?;
    // PD/TiKV open is a network boundary: unit tests inject mocks via `run_with`.
    // The binary validates flags and HTTP/TLS setup; cluster dial is stubbed.
    // 保留对 pd_addr/全局配置/HTTP 的触达，证明参数路径可走通。
    let _ = (
        cfg.pd_addr.clone(),
        stubs::TakeGlobalConfig(),
        http,
        TidbConfig::default(),
    );
    Err(Error::new(
        "PD/TiKV boundary stub: inject PdClient+Storage via run_with for lock generation",
    ))
}

/// Binary entrypoint.
/// 进程入口：收集 `env::args`，失败打印错误并以退出码 1 结束。
pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(err) = main_with_args(args) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
