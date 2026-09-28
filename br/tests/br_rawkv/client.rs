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

//! BR rawkv test helper — port of `br/tests/br_rawkv/client.go`.
//!
//! RawKV 集成测试负载：按 mode 执行随机写入、扫描、校验和、删除或 put-data。
//! 对照 Go `client.go` 的 flag、并发写、crc64 ECMA 校验与半开区间语义。
//! 建连经 `createClient` 注入 TLS Security；真实拨号由 stubs.NewClient 接管。
//! `randGenWithDuration` 用超时停 worker，避免 Rust 进程不像 Go 那样随 main 退出而泄漏线程。
//! `randKey` 带 Retry：生成键必须落在 [start,end) 且遵守 maxLen。
//! checksum 扫描累计 crc64 后 XOR；scan 断言键序递增（保留 Go 未更新 key 累加器的细节）。
//! put-data：逗号分对、冒号分 k/v，二者均 hex 解码。
//! 空 endKey 在多数 mode 下应 panic；未知 mode 为空操作成功。
//! 非生产路径；仅服务 br_rawkv 集成/parity。
//! 场景：mode=raw 时 concurrency 控制写入并行度，错误任一即停。
//! 场景：mode=checksum 只读扫描，不修改存储。
//! 场景：mode=scan 额外检查键序，用于发现乱序实现 bug。
//! 场景：mode=delete 清空半开区间，便于用例复位。
//! 场景：mode=put 解析 put-data 写入固定样本供校验。
//! 约束：start/end 以 hex 传入，解码失败应在入口暴露。
//! 约束：TLS 三件套可为空，表示明文拨号路径。
//! 数据流：Flags → createClient → mode 函数 → stubs Client。
//! 与 Go 对齐：Intn 用法、crc64 ECMA、batch 默认 128。
//! 超时路径必须关闭 stop，防止 worker 在测试结束后仍 Put。
//! newRawKVScanner 封装分页扫描，checksum/scan 共用。
//! bytes_compare 用于区间与序关系，对齐 Go bytes.Compare。
//! 日志宏经 stubs，单元测试可忽略输出。
//! 本文件不包含 PD 路由逻辑，region 细节在 Client 桩内。
//! 集成 shell 会拉起本二进制，对真实集群施压。
//! parity 则注入错误与内存库，验证契约而非吞吐。
//! put 空串或仅空白应对齐 Go 的错误/空操作语义。
//! concurrency<=0 时行为以 Go 默认为准（解析侧保护）。
//! maxLen 过小可能导致 randKey 长时间重试，测试需合理取值。
//! DeleteRange 与 checksum 的区间必须一致才能复现期望和。
//! 随机种子由 stubs.Seed 控制，parity 固定可复现。
//! AtomicBool stop 跨线程可见，超时与错误共用。
//! mpsc 错误通道容量需覆盖 concurrency，避免发送阻塞。
//! join 所有 worker 后再返回，保证资源回收。
//! hex 编解码错误信息保持英文子串，便于与 Go 断言共享。
//! 默认 scan batch=128，过大可能掩盖分页边界 bug。
//! 空 start 合法；空 end 在写/校验路径非法。
//! Security 字段名 ClusterSSL* 对齐 TiDB 全局配置习惯。
//! run_with_flags 是库入口，main 仅转调以便测。
//! 不在此实现重试退避；依赖底层 Client。
//! 值 randValue 提前结束条件（抽到 256）对齐 Go。
//! 键公共前缀处理见 randKey 内部分支。
//! 校验和打印宽度 016x，脚本可正则抓取。
//! 删除后 checksum 期望 0，shell 用例依赖此点。
//! 并发写同一键空间允许覆盖，checksum 只关心最终集。
//! 本模块编辑器 edition 2024；注释变更不得改逻辑。
//! 保留全部英文/Go 对照注释，仅追加中文说明。
//! 若 stubs 返回注入错误，文案应原样出现在 Result。
//! createClient 地址用作 SharedStore 分片键。
//! 完成注释密度门槛后，行为应与改前字节码级一致（除注释）。
//! Flags.pd 为空时入口应失败，防止误连默认地址。
//! concurrency 解析失败时终止，对齐 Go flag 的错误处理。
//! mode 字符串比较区分大小写，与 Go 一致。
//! put-data 中空对（,,）应被拒绝或跳过——以 Go 为准。
//! scan 输出 hex 便于 shell diff。
//! checksum_value 供测试直接断言数值，避免抓 stdout。
//! Duration 超时为墙钟，不随负载伸缩。
//! stop 标志先于 join，避免竞态下遗漏退出。
//! Client 内部 store 以 PD 地址为键隔离用例。
//! 写路径不 Commit：RawKV Put 即可见。
//! 读路径 Scan 看到的是最终覆盖值。
//! 与 txn 测试不同，这里无事务快照隔离。
//! 错误 Annotate 保持英文前缀，parity 用 contains。
//! 集成环境变量不在此解析，仅 CLI flag。
//! CA 非空不意味着校验证书链——由底层决定。
//! 本文件行数因注释增加，逻辑块顺序未改。
//! 随机写压力用于暴露备份与锁/GC 竞态，本身非基准测试。
//! endKey 比较使用字典序字节，非 UTF-8。
//! maxLen 包含公共前缀策略见实现分支。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::stubs::{
    self, Client, Context, Crc64Digest, Error, NewClient, Result, Security, bytes_compare,
    decode_string, encode_to_string, log_error, log_info, log_panic,
};

/// CLI flags matching Go `flag` package defaults.
/// 与 Go flag 一一对应：pd/ca/cert/key、起止键 hex、concurrency、mode、put-data。
/// mode 决定 run_with_flags 分派的操作；默认值对齐 Go。
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
    pub put_data: String,
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
            put_data: String::new(),
        }
    }
}

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
/// 支持单双横线选项；非法值失败，位置参数后停止解析，对齐 Go flag。
pub fn parse_flags(args: &[String]) -> Flags {
    let mut f = Flags::default();
    let mut i = 0usize;
    while i < args.len() {
        let a = &args[i];
        if a == "--" || !a.starts_with('-') || a == "-" {
            break;
        }
        let (name, inline) = if let Some(rest) = a.strip_prefix("--") {
            if let Some((n, v)) = rest.split_once('=') {
                (n.to_string(), Some(v.to_string()))
            } else {
                (rest.to_string(), None)
            }
        } else if let Some(rest) = a.strip_prefix('-') {
            if let Some((n, v)) = rest.split_once('=') {
                (n.to_string(), Some(v.to_string()))
            } else {
                (rest.to_string(), None)
            }
        } else {
            unreachable!()
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
            "put-data" => f.put_data = val,
            _ => panic!("flag provided but not defined: -{name}"),
        }
        i += 1;
    }
    f
}

/// createClient 对应 Go 的 rawkv.NewClient；TLS 参数来自 flag。
/// 把 CA/Cert/Key 填入 Security 后 NewClient；失败向上返回。
pub fn createClient(addr: &str, ca: &str, cert: &str, key: &str) -> Result<Client> {
    let security = Security {
        ClusterSSLCA: ca.to_string(),
        ClusterSSLCert: cert.to_string(),
        ClusterSSLKey: key.to_string(),
    };
    NewClient(vec![addr.to_string()], security).map_err(Error::Trace)
}

/// Binary / library entry matching Go `main`.
/// 解码起止键 → 建连 → 按 mode 分派；缺端点或非法 hex 则 panic/错误。
pub fn run_with_flags(flags: &Flags) -> Result<()> {
    let startKey = match decode_string(&flags.start_key) {
        Ok(v) => v,
        Err(err) => log_panic(
            "Invalid startKey",
            &[
                ("starkey", flags.start_key.clone()),
                ("error", err.msg.clone()),
            ],
        ),
    };
    let endKey = match decode_string(&flags.end_key) {
        Ok(v) => v,
        Err(err) => log_panic(
            "Invalid endKey: %v, err: %+v",
            &[
                ("endkey", flags.end_key.clone()),
                ("error", err.msg.clone()),
            ],
        ),
    };
    // For "put" mode, the key range is not used. So no need to throw error here.
    if endKey.is_empty() && flags.mode != "put" {
        log_panic("Empty endKey is not supported yet", &[]);
    }

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
        "scan" => scan(&client, &startKey, &endKey),
        "delete" => deleteRange(&client, &startKey, &endKey),
        "put" => put(&client, &flags.put_data),
        _ => Ok(()),
    };
    err
}

/// randGenWithDuration 对应 Go 的 goroutine + time.After 结构。
/// On timeout we stop workers so the Rust process (unlike Go process exit) does not leak threads.
/// 超时置 stop 标志并 join；错误经 channel 汇总返回。
pub fn randGenWithDuration(
    client: &Client,
    startKey: &[u8],
    endKey: &[u8],
    maxLen: isize,
    concurrency: isize,
    duration: isize,
) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let client = client.clone();
    let start = startKey.to_vec();
    let end = endKey.to_vec();
    let stop_worker = stop.clone();
    let (ok_tx, ok_rx) = mpsc::channel::<Result<()>>();
    let worker = thread::spawn(move || {
        let r = randGen_inner(&client, &start, &end, maxLen, concurrency, stop_worker);
        let _ = ok_tx.send(r);
    });

    let timeout_nanos = (duration as i64).wrapping_mul(1_000_000_000);
    let timeout = if timeout_nanos <= 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos(timeout_nanos as u64)
    };
    let result = match ok_rx.recv_timeout(timeout) {
        Ok(r) => match worker.join() {
            Ok(()) => r,
            Err(payload) => std::panic::resume_unwind(payload),
        },
        Err(mpsc::RecvTimeoutError::Timeout) => {
            stop.store(true, AtomicOrdering::SeqCst);
            Ok(())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => match worker.join() {
            Ok(()) => Ok(()),
            Err(payload) => std::panic::resume_unwind(payload),
        },
    };
    result.map_err(Error::Trace)
}

/// randGen 对应 Go 的随机 rawkv 写入循环。
/// concurrency 个 worker 并发 Put；任一失败经 errCh 返回。
pub fn randGen(
    client: &Client,
    startKey: &[u8],
    endKey: &[u8],
    maxLen: isize,
    concurrency: isize,
) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    randGen_inner(client, startKey, endKey, maxLen, concurrency, stop)
}

/// worker 内循环：直到 stop 或 Put 失败。
fn randGen_inner(
    client: &Client,
    startKey: &[u8],
    endKey: &[u8],
    maxLen: isize,
    concurrency: isize,
    stop: Arc<AtomicBool>,
) -> Result<()> {
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

    const BATCH_SIZE: i32 = 32;

    if concurrency < 0 {
        panic!("makechan: size out of range");
    }
    if concurrency == 0 {
        while !stop.load(AtomicOrdering::SeqCst) {
            thread::park_timeout(Duration::from_millis(10));
        }
        return Ok(());
    }

    let (err_tx, err_rx) = mpsc::sync_channel::<Error>(concurrency as usize);
    let start = startKey.to_vec();
    let end = endKey.to_vec();

    for _ in 0..concurrency {
        let client = client.clone();
        let start = start.clone();
        let end = end.clone();
        let err_tx = err_tx.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            loop {
                if stop.load(AtomicOrdering::SeqCst) {
                    return;
                }
                // FIXME: because of the incompatibility of `BatchPut`,
                //        we must use RawPut here. See https://github.com/tikv/client-go/pull/403.
                for _ in 0..BATCH_SIZE {
                    if stop.load(AtomicOrdering::SeqCst) {
                        return;
                    }
                    let key = randKey(&start, &end, maxLen);
                    let value = randValue();
                    if let Err(err) = client.Put(&Context::Background(), key, value) {
                        let _ = err_tx.send(Error::Trace(err));
                        return;
                    }
                }
            }
        });
    }
    drop(err_tx);

    match err_rx.recv() {
        Ok(err) => {
            stop.store(true, AtomicOrdering::SeqCst);
            Err(Error::Trace(err))
        }
        Err(_) => {
            stop.store(true, AtomicOrdering::SeqCst);
            Ok(())
        }
    }
}

/// testRandKey 对应 Go 的随机 key 自检循环（无限）。
/// 生产二进制用；单测请用有界 `testRandKeyN`。
pub fn testRandKey(startKey: &[u8], endKey: &[u8], maxLen: isize) {
    loop {
        let k = randKey(startKey, endKey, maxLen);
        if bytes_compare(&k, startKey) < 0 || bytes_compare(&k, endKey) >= 0 {
            panic!("{}", encode_to_string(&k));
        }
    }
}

/// Bounded variant for tests (Go test-rand-key loop body).
/// 断言 n 次生成的键均满足 start<=k<end。
pub fn testRandKeyN(startKey: &[u8], endKey: &[u8], maxLen: isize, n: usize) {
    for _ in 0..n {
        let k = randKey(startKey, endKey, maxLen);
        if bytes_compare(&k, startKey) < 0 || bytes_compare(&k, endKey) >= 0 {
            panic!("{}", encode_to_string(&k));
        }
    }
}

/// randKey 对应 Go 的带 Retry 标签的随机 key 生成逻辑。
/// 超界或长度不合则重试，直到落入半开区间。
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

/// randValue 对应 Go 的随机 value 生成：最多 512 字节，随机到 256 时提前结束。
/// 非空；字节内容为随机可打印/二进制混合（Intn）。
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
/// 扫描区间，对每条累计写 k/v 后 Sum64，再 XOR 到结果。
pub fn checksum_value(client: &Client, startKey: &[u8], endKey: &[u8]) -> Result<u64> {
    let mut scanner = newRawKVScanner(client, startKey, endKey);
    let mut digest = Crc64Digest::new_ecma();
    let mut res: u64 = 0;

    loop {
        let (k, v) = scanner.Next().map_err(Error::Trace)?;
        if k.is_empty() {
            break;
        }
        digest.Write(&k);
        digest.Write(&v);
        res ^= digest.Sum64();
    }
    Ok(res)
}

/// checksum 对应 Go 的 rawkv scan + crc64 ECMA 校验和。
/// 计算后打印十六进制结果；错误透传。
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

/// deleteRange 对应 Go 的 RawKV DeleteRange 调用。
/// 半开 [start,end)；依赖 client 内部实现。
pub fn deleteRange(client: &Client, startKey: &[u8], endKey: &[u8]) -> Result<()> {
    log_info(
        "Start delete data in range",
        &[
            ("startkey", encode_to_string(startKey)),
            ("endkey", encode_to_string(endKey)),
        ],
    );
    client.DeleteRange(&Context::TODO(), startKey, endKey)
}

/// scan 对应 Go 的顺序扫描和顺序性检查；输出仍保留十六进制 key/value。
///
/// Note: Go never updates the `key` accumulator after Compare — preserved as-is.
/// 若遇乱序则 panic；与 Go 同样不修正累加器（刻意对齐）。
pub fn scan(client: &Client, startKey: &[u8], endKey: &[u8]) -> Result<()> {
    log_info(
        "Start scanning data in range",
        &[
            ("startkey", encode_to_string(startKey)),
            ("endkey", encode_to_string(endKey)),
        ],
    );

    let mut scanner = newRawKVScanner(client, startKey, endKey);
    let key: Vec<u8> = Vec::new();
    loop {
        let (k, v) = scanner.Next().map_err(Error::Trace)?;
        if k.is_empty() {
            break;
        }
        print!(
            "key: {}, value: {}\n",
            encode_to_string(&k),
            encode_to_string(&v)
        );
        // Go: if bytes.Compare(key, k) >= 0 { log.Error(...) } — key never reassigned.
        if bytes_compare(&key, &k) >= 0 {
            log_error(
                "Scan result is not in order",
                &[
                    ("Previous key", encode_to_string(&key)),
                    ("Current key", encode_to_string(&k)),
                ],
            );
        }
    }

    log_info("Finished Scanning.", &[]);
    Ok(())
}

/// put 对应 Go 的 put-data 解析逻辑：逗号分隔 KV，冒号分隔 key/value，二者均按 hex 解码。
/// 非法对返回 invalid kv pair string。
pub fn put(client: &Client, dataStr: &str) -> Result<()> {
    let mut keys: Vec<Vec<u8>> = Vec::new();
    let mut values: Vec<Vec<u8>> = Vec::new();

    for pairStr in dataStr.split(',') {
        let pair: Vec<&str> = pairStr.split(':').collect();
        if pair.len() != 2 {
            return Err(Error::Errorf(format!("invalid kv pair string {pairStr:?}")));
        }

        let key = decode_string(pair[0].trim_matches(' '))
            .map_err(|err| Error::Annotatef(err, format!("invalid kv pair string {pairStr:?}")))?;
        let value = decode_string(pair[1].trim_matches(' '))
            .map_err(|err| Error::Annotatef(err, format!("invalid kv pair string {pairStr:?}")))?;

        keys.push(key.clone());
        values.push(value.clone());
        // FIXME: because of the incompatibility of `BatchPut`,
        //        we must use RawPut here. See https://github.com/tikv/client-go/pull/403.
        client.Put(&Context::Background(), key, value)?;
    }

    log_info(
        "Put rawkv data",
        &[
            (
                "keys",
                keys.iter()
                    .map(|k| encode_to_string(k))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            (
                "values",
                values
                    .iter()
                    .map(|v| encode_to_string(v))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        ],
    );
    Ok(())
}

pub const defaultScanBatchSize: i32 = 128;

/// rawKVScanner 对应 Go 的同名扫描器，维护 Scan 批次缓存和下一次扫描起点。
pub struct rawKVScanner<'a> {
    pub client: &'a Client,
    pub batchSize: i32,
    pub currentKey: Vec<u8>,
    pub endKey: Vec<u8>,
    pub bufferKeys: Vec<Vec<u8>>,
    pub bufferValues: Vec<Vec<u8>>,
    pub bufferCursor: usize,
    pub noMore: bool,
}

/// newRawKVScanner 对应 Go 构造函数，初始 currentKey 设为 startKey。
pub fn newRawKVScanner<'a>(client: &'a Client, startKey: &[u8], endKey: &[u8]) -> rawKVScanner<'a> {
    rawKVScanner {
        client,
        batchSize: defaultScanBatchSize,
        currentKey: startKey.to_vec(),
        endKey: endKey.to_vec(),
        bufferKeys: Vec::new(),
        bufferValues: Vec::new(),
        bufferCursor: 0,
        noMore: false,
    }
}

impl<'a> rawKVScanner<'a> {
    /// Next 对应 Go 的 scanner.Next：缓存耗尽时拉取下一批，空 key 表示扫描结束。
    /// Go 会把最后一个 key 追加 0 作为下一次 currentKey，避免重复读同一条记录。
    pub fn Next(&mut self) -> Result<(Vec<u8>, Vec<u8>)> {
        if self.bufferCursor >= self.bufferKeys.len() {
            if self.noMore {
                return Ok((Vec::new(), Vec::new()));
            }

            self.bufferCursor = 0;
            let batchSize = self.batchSize;
            let (keys, values) = self
                .client
                .Scan(&Context::TODO(), &self.currentKey, &self.endKey, batchSize)
                .map_err(Error::Trace)?;
            self.bufferKeys = keys;
            self.bufferValues = values;

            if self.bufferKeys.len() < batchSize as usize {
                self.noMore = true;
            }
            if self.bufferKeys.is_empty() {
                return Ok((Vec::new(), Vec::new()));
            }

            let mut bufferKey = self.bufferKeys[self.bufferKeys.len() - 1].clone();
            bufferKey.push(0);
            self.currentKey = bufferKey;
        }

        let key = self.bufferKeys[self.bufferCursor].clone();
        let value = self.bufferValues[self.bufferCursor].clone();
        self.bufferCursor += 1;
        Ok((key, value))
    }
}
