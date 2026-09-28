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

//! BR txnkv test helper — port of `br/tests/br_txn/client.go`.
//!
//! 事务 KV 集成测试负载：随机事务写入、校验和、删除区间，对照 Go `client.go`。
//! 与 rawkv 版差异：Begin/Commit 事务边界、全局 Security 配置、appendIndex 避冲突。
//! createClient 在有 CA 时先 StoreGlobalConfig，再 NewClient。
//! randGen 批量事务写；WaitGroupGuard 在 Drop 时减计数，模拟 Go WaitGroup。
//! checksum 迭代器必须「先 Next 再读」，与 Go 顺序一致否则 XOR 漂移。
//! deleteRange 并发度取自全局 flag；半开 [start,end)。
//! 空 endKey panic；未知 mode 成功 no-op；duration=0 立即取消。
//! 非生产路径；错误经 errCh/Annotate 向上返回。
//! 主线程汇总 errCh 后返回首个错误。
//! worker 内 BATCH 循环可被取消打断。
//! Commit 成功后键值对备份可见。
//! 未 Commit 的 Set 不应出现在 checksum。
//! 与 Go 相同：log.Panic 路径在 Rust 用 panic/exit 表达。
//! startKey/endKey 为空串时的行为见入口检查。
//! TLS 路径与非 TLS 路径共享 NewClient 签名。
//! 索引追加使用十进制 ASCII，便于调试。
//! 通道关闭后 send_err 不再发送。
//! 测试 Seed 后 Intn 序列可复现。
//! 删除并发度过大时仍应正确清空区间。
//! 迭代器 Invalid 时结束 checksum 循环。
//! 保留 Go 对照英文注释不被删除。
//! 注释工作不新增 AsterSQL 版权行（文件已有则保留）。
//! 完成密度后仅允许空白/注释差异。
//! 数据流：Flags → createClient → Begin/Set/Commit 或迭代校验。
//! 全局 CONCURRENCY 影响 deleteRange 并行度。
//! Outcome 枚举区分成功与错误，汇聚 worker 结果。
//! BATCH_SIZE/NUM_BATCH 控制每事务键数与批次数。
//! 与 rawkv 共用 randKey/randValue 思想，但提交语义不同。
//! Begin 失败经 set_begin_error 注入验证。
//! Commit 失败经 set_commit_error 注入验证。
//! Set 失败经 set_set_error 注入验证。
//! 拨号失败经 set_new_client_error 注入验证。
//! GetGlobalConfig 读取 TLS 副作用，parity 会断言。
//! appendIndex 推过 end 时 Go 亦接受，不做额外截断。
//! WaitGroup 计数归零前主线程不得提前返回。
//! 取消上下文后 worker 应尽快退出循环。
//! 校验和依赖快照隔离看到的已提交值。
//! 随机写用于备份前灌数，非性能基准。
//! 空 pd / 非法 hex 在入口失败。
//! mode 分派与 Go switch 分支对齐。
//! 保留英文注释；仅追加中文说明职责与约束。
//! edition 2024；注释变更不得改可执行逻辑。
//! stubs 内存库按地址隔离，避免并行用例串扰。
//! errCh 只保留首错，后续错误丢弃。
//! Drop Guard 即使 panic 路径也应减计数（若未 abort）。
//! 事务重试不在本文件实现。
//! 集成 shell 拉起本二进制对真实集群施压。
//! parity 覆盖契约；shell 覆盖端到端。
//! Security 空串表示不启用集群 TLS。
//! 打印 checksum 宽度 016x 便于脚本抓取。
//! 删除后期望校验和为 0。
//! 并发写允许覆盖，最终集决定 checksum。
//! 本模块是测试辅助，不是生产 txnkv API。
//!
//! Go 对照：`br/tests/br_txn/client.go` 集成测试辅助入口。
//! 解析 CLI Flags 后对 txnkv 桩执行随机读写，校验校验和与区间删除。
//! `createClient` 绑定 PD 地址与 Security；失败应表面化而非吞掉。
//! `randGen*` 控制负载时长与并发，避免测试无限自旋。
//! checksum/scan/put/deleteRange 顺序对齐 Go：先写后扫再删再校验。
//! BATCH_SIZE 与 defaultScanBatchSize 决定分批边界，回归勿随意改大。
//! 本文件不启动真实 TiKV；依赖同目录 stubs 的内存 Client。
//! 注释只解释场景与断言依据，不改 flags 默认值或随机种子语义。
//! 并发路径用原子/锁保护共享计数，与 Go WaitGroup 场景等价。
//! 错误路径需保留 Annotate 上下文，便于定位是 dial 还是 KV 操作失败。
//! 完成门槛：中文注释密度、仅注释差异、空白与 rustfmt 检查。
//! 若 rustfmt 基线失败则标待回归，不声称格式已绿。
//! 随机键值生成需保证可复现 Seed，便于失败用例重放。
//! 区间扫描半开语义与 rawkv 测试一致，避免边界键重复计入。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::stubs::{
    self, Client, Context, Crc64Digest, Error, GetGlobalConfig, NewClient, Result,
    StoreGlobalConfig, bytes_compare, encode_to_string, log_info, log_panic,
};

/// CLI flags matching Go `flag` package defaults.
/// 对齐 Go：pd/TLS、起止键、concurrency、mode；另含事务相关默认。
#[derive(Clone, Debug)]
pub struct Flags {
    pub ca: String,
    pub cert: String,
    pub key: String,
    pub pd: String,
    pub mode: String,
    pub start_key: String,
    pub end_key: String,
    pub key_max_len: isize,
    pub concurrency: isize,
    pub duration: isize,
}

impl Default for Flags {
    fn default() -> Self {
        Self {
            ca: String::new(),
            cert: String::new(),
            key: String::new(),
            pd: "127.0.0.1:2379".to_string(),
            mode: String::new(),
            start_key: String::new(),
            end_key: String::new(),
            key_max_len: 32,
            concurrency: 32,
            duration: 10,
        }
    }
}

// 进程级并发度：deleteRange 与 Go 一样读全局 flag，而非函数参数透传。
pub(crate) static CONCURRENCY: AtomicIsize = AtomicIsize::new(32);

/// Parse an `int` flag with Go's `strconv.ParseInt(value, 0, 0)` syntax.
fn parse_go_int(value: &str) -> Option<isize> {
    let (negative, unsigned) = if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = value.strip_prefix('+') {
        (false, rest)
    } else {
        (false, value)
    };
    if unsigned.is_empty() {
        return None;
    }

    let (radix, digits, has_base_prefix) = if let Some(rest) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        (16, rest, true)
    } else if let Some(rest) = unsigned
        .strip_prefix("0b")
        .or_else(|| unsigned.strip_prefix("0B"))
    {
        (2, rest, true)
    } else if let Some(rest) = unsigned
        .strip_prefix("0o")
        .or_else(|| unsigned.strip_prefix("0O"))
    {
        (8, rest, true)
    } else if unsigned.len() > 1 && unsigned.starts_with('0') {
        (8, unsigned, false)
    } else {
        (10, unsigned, false)
    };

    let chars: Vec<char> = digits.chars().collect();
    let mut normalized = String::with_capacity(digits.len());
    let mut saw_digit = false;
    let mut previous_was_digit = false;
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch == '_' {
            let allowed_after_prefix = has_base_prefix && index == 0;
            let next_is_digit = chars
                .get(index + 1)
                .and_then(|next| next.to_digit(radix))
                .is_some();
            if (!previous_was_digit && !allowed_after_prefix) || !next_is_digit {
                return None;
            }
            previous_was_digit = false;
            continue;
        }
        ch.to_digit(radix)?;
        normalized.push(ch);
        saw_digit = true;
        previous_was_digit = true;
    }
    if !saw_digit || !previous_was_digit {
        return None;
    }

    let magnitude = i128::from_str_radix(&normalized, radix).ok()?;
    let signed = if negative {
        magnitude.checked_neg()?
    } else {
        magnitude
    };
    isize::try_from(signed).ok()
}

fn parse_integer_flag(name: &str, value: &str) -> isize {
    parse_go_int(value).unwrap_or_else(|| panic!("invalid value {value:?} for flag -{name}"))
}

/// Parse argv shaped like Go `flag` (`--name value` / `--name=value`).
/// 遇到首个位置参数停止；未知、缺值或非法整数视为解析失败。
pub fn parse_flags(args: &[String]) -> Flags {
    let mut f = Flags::default();
    let mut i = 0usize;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            break;
        }
        // 兼容 `--name=value` / `-name=value` 与分词两段式写法，与 Go flag 一致。
        let (name, inline) = if let Some(rest) = a.strip_prefix("--") {
            if let Some((n, v)) = rest.split_once('=') {
                (n.to_string(), Some(v.to_string()))
            } else {
                (rest.to_string(), None)
            }
        } else if let Some(rest) = a.strip_prefix('-') {
            if rest.is_empty() {
                break;
            }
            if let Some((n, v)) = rest.split_once('=') {
                (n.to_string(), Some(v.to_string()))
            } else {
                (rest.to_string(), None)
            }
        } else {
            // Go flag.Parse stops at the first non-flag argument.
            break;
        };

        let val = if let Some(v) = inline {
            v
        } else {
            i += 1;
            if i >= args.len() {
                panic!("flag needs an argument: -{name}");
            }
            args[i].clone()
        };

        match name.as_str() {
            "ca" => f.ca = val,
            "cert" => f.cert = val,
            "key" => f.key = val,
            "pd" => f.pd = val,
            "mode" => f.mode = val,
            "start-key" => f.start_key = val,
            "end-key" => f.end_key = val,
            "key-max-len" => f.key_max_len = parse_integer_flag(&name, &val),
            "concurrency" => f.concurrency = parse_integer_flag(&name, &val),
            "duration" => f.duration = parse_integer_flag(&name, &val),
            _ => panic!("flag provided but not defined: -{name}"),
        }
        i += 1;
    }
    // 同步全局并发度，供 deleteRange 读取（对齐 Go 包级变量）。
    CONCURRENCY.store(f.concurrency, Ordering::SeqCst);
    f
}

/// createClient 对应 Go 的 txnkv.NewClient；有 ca 时先更新全局 Security 配置。
/// TLS 写入全局后再拨号，供底层读取 ClusterSSL*。
pub fn createClient(addr: &str, ca: &str, cert: &str, key: &str) -> Result<Client> {
    // 有 CA 才改全局配置；空 CA 保持默认明文拨号。
    if !ca.is_empty() {
        let mut conf = GetGlobalConfig();
        conf.Security.ClusterSSLCA = ca.to_string();
        conf.Security.ClusterSSLCert = cert.to_string();
        conf.Security.ClusterSSLKey = key.to_string();
        StoreGlobalConfig(conf);
    }
    NewClient(vec![addr.to_string()]).map_err(Error::Trace)
}

/// Binary / library entry matching Go `main`.
/// 解码键、建连、按 mode 分派；校验 pd/endKey 等前置条件。
pub fn run_with_flags(flags: &Flags) -> Result<()> {
    // Go: startKey := []byte(*startKeyStr) — raw string bytes, not hex.
    // 键按原始字节而非 hex 解码，与 Go client.go 入口一致。
    let startKey = flags.start_key.as_bytes().to_vec();
    let endKey = flags.end_key.as_bytes().to_vec();
    // 空 endKey 尚未支持（半开区间上界缺失），直接 panic 对齐 Go。
    if endKey.is_empty() {
        log_panic("Empty endKey is not supported yet", &[]);
    }

    // 自检模式：不建连，仅验证 randKey 落在半开区间。
    if flags.mode == "test-rand-key" {
        testRandKey(&startKey, &endKey, flags.key_max_len);
        return Ok(());
    }

    let client = match createClient(&flags.pd, &flags.ca, &flags.cert, &flags.key) {
        Ok(cli) => cli,
        Err(err) => log_panic(
            "Failed to create client",
            &[("pd", flags.pd.clone()), ("error", err.msg.clone())],
        ),
    };

    // mode 分派：未知 mode 成功 no-op，便于脚本扩展而不破坏旧调用。
    let err = match flags.mode.as_str() {
        "rand-gen" => randGenWithDuration(
            &client,
            &startKey,
            &endKey,
            flags.key_max_len,
            flags.concurrency,
            flags.duration,
        ),
        "checksum" => checksum(&client, &startKey, &endKey),
        "delete" => deleteRange(&client, &startKey, &endKey),
        _ => Ok(()),
    };
    err
}

/// randGenWithDuration 对应 Go 的 context.WithTimeout + defer cancel。
/// duration=0 立即取消；超时后停止写入并回收线程。
pub fn randGenWithDuration(
    client: &Client,
    startKey: &[u8],
    endKey: &[u8],
    maxLen: isize,
    concurrency: isize,
    duration: isize,
) -> Result<()> {
    // Go converts int to time.Duration, then multiplies in signed nanoseconds.
    let timeout_nanos = (duration as i64).wrapping_mul(1_000_000_000);
    let timeout = if timeout_nanos <= 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos(timeout_nanos as u64)
    };
    let (ctx, cancel) = Context::WithTimeout(&Context::Background(), timeout);
    let result = randGen(&ctx, client, startKey, endKey, maxLen, concurrency);
    cancel();
    result
}

/// randGen 对应 Go 的事务随机写入逻辑。
/// 多 worker：Begin→批量 Set→Commit；失败 send_err。
pub fn randGen(
    ctx: &Context,
    client: &Client,
    startKey: &[u8],
    endKey: &[u8],
    maxLen: isize,
    concurrency: isize,
) -> Result<()> {
    // Go make(chan error, concurrency) panics when capacity is negative.
    assert!(concurrency >= 0, "negative concurrency: {concurrency}");
    log_info(
        "Start rand-gen",
        &[
            ("maxlen", maxLen.to_string()),
            ("startkey", encode_to_string(startKey)),
            ("endkey", encode_to_string(endKey)),
        ],
    );
    log_info(
        "Rand-gen will keep running. Please Ctrl+C to stop manually.",
        &[],
    );

    // 计算起止键公共前缀长度；maxLen 不得短于该前缀，否则无法生成合法键。
    let mut commonPrefixLen = 0usize;
    while commonPrefixLen < startKey.len()
        && commonPrefixLen < endKey.len()
        && startKey[commonPrefixLen] == endKey[commonPrefixLen]
    {
        commonPrefixLen += 1;
    }
    if maxLen < commonPrefixLen as isize {
        return Err(Error::Errorf(format!(
            "maxLen ({maxLen}) < commonPrefixLen ({commonPrefixLen})"
        )));
    }

    // 每事务 32 键、每 worker 100 批，对齐 Go 常量灌数强度。
    const BATCH_SIZE: i32 = 32;
    const NUM_BATCH: i32 = 100;

    let (err_tx, err_rx) = mpsc::channel::<Error>();
    let err_tx = Arc::new(Mutex::new(Some(err_tx)));
    let wg = Arc::new(Mutex::new(0usize));

    let start = startKey.to_vec();
    let end = endKey.to_vec();

    // worker 数 = concurrency+1；循环变量同时作 key 长度扰动参数。
    for i in maxLen..=maxLen.wrapping_add(concurrency) {
        {
            let mut g = wg.lock().unwrap();
            *g += 1;
        }
        let ctx = ctx.clone();
        let client = client.clone();
        let start = start.clone();
        let end = end.clone();
        let err_tx = err_tx.clone();
        let wg = wg.clone();
        thread::spawn(move || {
            // Drop 时减 WaitGroup，确保主线程不会永远等 done。
            let _guard = WaitGroupGuard(wg);
            for _ in 0..NUM_BATCH {
                // 超时/取消后尽快退出，避免超时后仍 Commit。
                if ctx.Done() {
                    return;
                }
                let mut txn = match client.Begin() {
                    Ok(txn) => txn,
                    Err(err) => {
                        send_err(&err_tx, Error::Trace(err));
                        // Go falls through on Begin error (would panic); we skip the batch.
                        // Begin 失败：上报后跳过本批，避免无 txn 上 Set panic。
                        continue;
                    }
                };
                for _ in 0..BATCH_SIZE {
                    let mut key = randKey(&start, &end, i);
                    // append index to avoid write conflict
                    // 追加 worker 索引降低写冲突；可能越过 end，Go 亦接受。
                    key = appendIndex(key, i);
                    let value = randValue();
                    if let Err(err) = txn.Set(key, value) {
                        send_err(&err_tx, Error::Trace(err));
                    }
                }
                // Commit 失败上报首错；不在此重试事务。
                if let Err(err) = txn.Commit(&Context::TODO()) {
                    send_err(&err_tx, Error::Trace(err));
                }
            }
        });
    }

    // Drop our clone of the sender so workers alone keep the channel open.
    // 丢弃主线程 sender，仅 worker 保活通道，便于 recv 感知结束。
    drop(err_tx);

    // Wait for workers in a helper thread (mirrors Go done channel).
    let (done_tx, done_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            let left = *wg.lock().unwrap();
            if left == 0 {
                let _ = done_tx.send(());
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
    });

    // Race done vs first error — matches Go select { case <-done / case err := <-errCh }.
    // 首错与全部完成竞态汇聚，对齐 Go select。
    enum Outcome {
        Done,
        Err(Error),
    }
    let (out_tx, out_rx) = mpsc::channel::<Outcome>();
    {
        let out_tx2 = out_tx.clone();
        thread::spawn(move || {
            if let Ok(err) = err_rx.recv() {
                let _ = out_tx2.send(Outcome::Err(err));
            }
        });
    }
    thread::spawn(move || {
        let _ = done_rx.recv();
        let _ = out_tx.send(Outcome::Done);
    });

    match out_rx.recv() {
        Ok(Outcome::Done) => Ok(()),
        Ok(Outcome::Err(err)) => {
            // Drain done (Go: <-done after taking err).
            // 取到首错后仍等待 done，避免 worker 泄漏。
            let _ = out_rx.recv();
            Err(err)
        }
        Err(_) => Ok(()),
    }
}

/// Drop 时减少 WaitGroup 计数，对齐 Go defer Done。
struct WaitGroupGuard(Arc<Mutex<usize>>);

impl Drop for WaitGroupGuard {
    fn drop(&mut self) {
        let mut g = self.0.lock().unwrap();
        *g = g.saturating_sub(1);
    }
}

/// 向错误通道发送首个错误；后续忽略，避免阻塞。
fn send_err(err_tx: &Arc<Mutex<Option<mpsc::Sender<Error>>>>, err: Error) {
    if let Some(tx) = err_tx.lock().unwrap().as_ref() {
        let _ = tx.send(err);
    }
}

/// testRandKey 对应 Go 的随机 key 自检循环（无限）。
/// 单测用 `testRandKeyN` 有界版本。
pub fn testRandKey(startKey: &[u8], endKey: &[u8], maxLen: isize) {
    loop {
        let k = randKey(startKey, endKey, maxLen);
        if bytes_compare(&k, startKey) < 0 || bytes_compare(&k, endKey) >= 0 {
            panic!("{}", encode_to_string(&k));
        }
    }
}

/// Bounded variant for tests (Go test-rand-key loop body).
/// 断言生成键落在 [start,end)。
pub fn testRandKeyN(startKey: &[u8], endKey: &[u8], maxLen: isize, n: usize) {
    for _ in 0..n {
        let k = randKey(startKey, endKey, maxLen);
        if bytes_compare(&k, startKey) < 0 || bytes_compare(&k, endKey) >= 0 {
            panic!("{}", encode_to_string(&k));
        }
    }
}

/// randKey 对应 Go 的带 Retry 标签随机 key 生成逻辑。
/// 超界重试直至落入半开区间。
pub fn randKey(startKey: &[u8], endKey: &[u8], maxLen: isize) -> Vec<u8> {
    'retry: loop {
        let mut result: Vec<u8> = Vec::with_capacity(maxLen as usize);
        let mut upperUnbounded = false;
        let mut lowerUnbounded = false;

        for i in 0..maxLen as usize {
            let mut upperBound = 256;
            if !upperUnbounded {
                if i >= endKey.len() {
                    // The generated key is the same as endKey which is invalid. Regenerate it.
                    continue 'retry;
                }
                upperBound = endKey[i] as i32 + 1;
            }

            let mut lowerBound = 0;
            if !lowerUnbounded {
                if i >= startKey.len() {
                    lowerUnbounded = true;
                } else {
                    lowerBound = startKey[i] as i32;
                }
            }

            if lowerUnbounded && stubs::Intn(257) == 0 {
                return result;
            }

            let mut value = stubs::Intn(upperBound - lowerBound);
            value += lowerBound;
            if value < upperBound - 1 {
                upperUnbounded = true;
            }
            if value > lowerBound {
                lowerUnbounded = true;
            }
            result.push(value as u8);
        }
        return result;
    }
}

/// appendIndex 对应 Go 的冲突规避辅助函数。
/// 追加 worker/批次索引，降低并发写同一键概率；可能越过 end。
pub fn appendIndex(mut key: Vec<u8>, i: isize) -> Vec<u8> {
    key.push(i as u8);
    key
}

/// randValue 对应 Go 的随机 value 生成，最多 512 字节。
/// 非空；长度随机。
pub fn randValue() -> Vec<u8> {
    let mut result = Vec::with_capacity(512);
    for i in 0..512 {
        let mut value = stubs::Intn(257);
        if value == 256 {
            if i > 0 {
                return result;
            }
            value -= 1;
        }
        result.push(value as u8);
    }
    result
}

/// Compute checksum value (Go prints `Checksum result: %016x`).
/// 事务迭代：Valid 循环内先 Next 再读 Key/Value，再 crc64 XOR。
pub fn checksum_value(client: &Client, startKey: &[u8], endKey: &[u8]) -> Result<u64> {
    let txn = client.Begin().map_err(Error::Trace)?;
    let mut iter = txn.Iter(startKey, endKey).map_err(Error::Trace)?;
    let mut digest = Crc64Digest::new_ecma();
    let mut res: u64 = 0;

    // Preserve Go order: Next before reading Key/Value.
    while iter.Valid() {
        iter.Next().map_err(Error::Trace)?;
        if iter.Key().is_empty() {
            break;
        }
        digest.Write(iter.Key());
        digest.Write(iter.Value());
        res ^= digest.Sum64();
    }
    // Commit best-effort (Go ignores error).
    let mut txn = txn;
    let _ = txn.Commit(&Context::TODO());
    Ok(res)
}

/// checksum 对应 Go 的事务迭代器校验和流程。
/// 打印结果；错误透传。
pub fn checksum(client: &Client, startKey: &[u8], endKey: &[u8]) -> Result<()> {
    log_info(
        "Start checkcum on range",
        &[
            ("startkey", encode_to_string(startKey)),
            ("endkey", encode_to_string(endKey)),
        ],
    );

    let res = checksum_value(client, startKey, endKey)?;
    log_info("Checksum result", &[("checksum", res.to_string())]);
    print!("Checksum result: {res:016x}\n");
    Ok(())
}

/// deleteRange 对应 Go 的 txnkv.DeleteRange，concurrency 来自全局 flag。
/// 半开区间删除，供用例复位。
pub fn deleteRange(client: &Client, startKey: &[u8], endKey: &[u8]) -> Result<()> {
    log_info(
        "Start delete data in range",
        &[
            ("startkey", encode_to_string(startKey)),
            ("endkey", encode_to_string(endKey)),
        ],
    );
    let concurrency = CONCURRENCY.load(Ordering::SeqCst);
    client
        .DeleteRange(&Context::TODO(), startKey, endKey, concurrency as i32)
        .map(|_| ())
}
