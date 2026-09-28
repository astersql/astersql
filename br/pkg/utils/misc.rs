// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Miscellaneous helpers ported from `br/pkg/utils/misc.go`.
//! 杂项工具：列类型兼容、Store 存活、超时清理、退出信号与备份文件汇总。
//! gRPC dial 在本 crate 仅提供 Noop 桩，真实连接由上层注入。
//! 约束：不改行为，仅说明与 Go misc.go 的对齐点与桩边界。
//! IsTypeCompatible 用于增量恢复时判断列类型是否可安全拓宽。
//! CheckStoreLiveness 过滤不可用 TiKV，避免备份打到僵死节点。
//! WithCleanUp 保证 defer 清理错误不被业务错误吞掉。
//! StartExitSingleListener 对标 Go 单次信号优雅退出路径。
//! SummaryFiles 的 CRC 使用 XOR 聚合，与校验链路一致。
//! Values/FlattenValues 为调度侧收集 map 值的小工具。
//! GetPartitionByName 错误文案保留 Go 拼写 parition。
//! LabelRuleBatchSize 限制单次下发的 placement 规则条数。
//! storeDisconnectionDuration 与 PD store 心跳语义绑定。
//! GrpcDialer 抽象便于测试替换真实拨号。
//! NoopGrpcDialer 明确失败，防止静默连到空实现。
//! AllStackInfo 在退出 dump 时提供诊断线索。
//! DumpGoroutineWhenExit 由外部配置开关控制。
//! FlattenValues 预分配容量，减少大 map 展平时的重分配。
//! 分区查找使用 CIStr.L，保证大小写不敏感。
//! CollectSuccessUnit 副作用写入全局 summary，便于任务结束汇报。
//! 心跳时间戳单位为纳秒 epoch，与 metapb 一致。
//! 清理超时线程用 AtomicBool 与 cancel 协作，避免泄漏误取消。

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::kvproto::brpb::File;
use crate::kvproto::metapb::{self, Store};
use astersql_br_pkg_errors::{ErrKVStorage, ErrUnknown};
use astersql_br_pkg_logutil::{Field, log};
use astersql_br_pkg_summary::{CollectInt, CollectSuccessUnit, SummaryValue, TotalBytes, TotalKV};
use astersql_errors::{Annotate, Join, SharedError, Trace};
use astersql_meta_model::TableInfo;
use astersql_parser_ast::CIStr;
use astersql_parser_mysql::r#type::{HasNotNullFlag, HasUnsignedFlag};
use astersql_parser_mysql::util;
use astersql_parser_types::{FieldType, UnspecifiedLength};
use astersql_util::security::TLS;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::iterator::Signals;

/// Store 心跳超时阈值；超过则视为断连（对齐 Go 100s）。
pub const storeDisconnectionDuration: Duration = Duration::from_secs(100);
/// Placement Label 规则批处理大小。
pub const LabelRuleBatchSize: usize = 50;

/// 判断源列类型能否安全装入目标列。
/// 返回 `(类型兼容, collation 是否相等)`；collation 独立于类型结果。
pub fn IsTypeCompatible(src: FieldType, target: FieldType) -> (bool, bool) {
    let collate_eq = src.GetCollate() == target.GetCollate();
    // NOT NULL / UNSIGNED / EvalType 任一不一致即类型不兼容。
    if HasNotNullFlag(src.GetFlag()) != HasNotNullFlag(target.GetFlag()) {
        return (false, collate_eq);
    }
    if HasUnsignedFlag(src.GetFlag()) != HasUnsignedFlag(target.GetFlag()) {
        return (false, collate_eq);
    }
    if src.EvalType() != target.EvalType() {
        return (false, collate_eq);
    }

    // flen/decimal 经默认值补齐后，源不得大于目标。
    let (src_flen, src_decimal) = flen_and_decimal(&src);
    let (target_flen, target_decimal) = flen_and_decimal(&target);
    if src_flen > target_flen || src_decimal > target_decimal {
        return (false, collate_eq);
    }

    // enum/set：源元素集合必须是目标的子集。
    let src_elems = src.GetElems();
    let target_elems = target.GetElems();
    if src_elems.len() > target_elems.len() {
        return (false, collate_eq);
    }
    let target_set: HashSet<&str> = target_elems.iter().map(String::as_str).collect();
    for item in src_elems {
        if !target_set.contains(item.as_str()) {
            return (false, collate_eq);
        }
    }
    // 最终以 charset 是否一致决定类型兼容（collate 已在返回值第二项）。
    (src.GetCharset() == target.GetCharset(), collate_eq)
}

/// 将 UnspecifiedLength 替换为类型默认 flen/decimal，再参与比较。
fn flen_and_decimal(tp: &FieldType) -> (isize, isize) {
    let (default_flen, default_decimal) = util::GetDefaultFieldLengthAndDecimal(tp.GetType() as u8);
    let mut flen = tp.GetFlen();
    let mut decimal = tp.GetDecimal();
    if flen == UnspecifiedLength {
        flen = default_flen;
    }
    if decimal == UnspecifiedLength {
        decimal = default_decimal;
    }
    (flen, decimal)
}

/// Stub for external gRPC dial; real gRPC wiring lives outside this crate.
/// 外部 gRPC 拨号抽象；本 crate 默认 Noop，真实实现由调用方注入。
pub trait GrpcDialer: Send + Sync {
    fn dial(&self, store_addr: &str, tls: Option<&TLS>) -> Result<Arc<dyn GrpcConn>, SharedError>;
}

/// 已建立的 gRPC 连接句柄：可查询 target 与关闭。
pub trait GrpcConn: Send + Sync {
    fn target(&self) -> String;
    fn close(&self) -> Result<(), SharedError>;
}

/// 默认拨号器：始终失败，提示尚未接线。
pub struct NoopGrpcDialer;

impl GrpcDialer for NoopGrpcDialer {
    fn dial(&self, store_addr: &str, _tls: Option<&TLS>) -> Result<Arc<dyn GrpcConn>, SharedError> {
        Err(Annotate(
            Some(SharedError::new((*ErrUnknown).clone())),
            format!("GRPCConn is not available for {store_addr}"),
        )
        .expect("annotate grpc unavailable"))
    }
}

/// 兼容 Go `GRPCConn` 入口；当前固定走 NoopGrpcDialer。
pub fn GRPCConn(
    _ctx: &crate::stubs::context::Context,
    store_addr: &str,
    _tls_conf: Option<&TLS>,
) -> Result<Arc<dyn GrpcConn>, SharedError> {
    NoopGrpcDialer.dial(store_addr, _tls_conf)
}

/// 检查 Store 是否处于可服务状态：须 Up/Offline，且心跳未过期。
pub fn CheckStoreLiveness(store: &Store) -> Result<(), SharedError> {
    if store.get_state() != metapb::StoreState::Up
        && store.get_state() != metapb::StoreState::Offline
    {
        return Err(Annotate(
            Some(SharedError::new((*ErrKVStorage).clone())),
            format!("the store state isn't up, it is {:?}", store.get_state()),
        )
        .expect("annotate store state"));
    }
    // last_heartbeat==0 表示未知，跳过超时判定（与 Go 一致）。
    if store.get_last_heartbeat() > 0 {
        let last_heartbeat = UNIX_EPOCH + Duration::from_nanos(store.get_last_heartbeat() as u64);
        if let Ok(since) = SystemTime::now().duration_since(last_heartbeat) {
            if since > storeDisconnectionDuration {
                return Err(Annotate(
                    Some(SharedError::new((*ErrKVStorage).clone())),
                    format!("the store last heartbeat is too far, at {since:?}"),
                )
                .expect("annotate heartbeat"));
            }
        }
    }
    Ok(())
}

/// 在超时上下文中执行清理函数，并把清理错误与原有错误 Join。
/// Go：`context.WithTimeout` + `multierr.Combine(cleanupErr, *errOut)`，cleanup 在前。
pub fn WithCleanUp<F>(err_out: &mut Option<SharedError>, timeout: Duration, mut fn_: F)
where
    F: FnMut(&crate::stubs::context::Context) -> Result<(), SharedError>,
{
    // Mirror Go `context.WithTimeout`: cancel after timeout, but return as soon as fn completes.
    let ctx = crate::stubs::context::Context::new();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let worker_ctx = ctx.clone();
    // 后台计时：超时且清理未结束则 cancel；清理结束后 stop 阻止误 cancel。
    thread::spawn(move || {
        let start = std::time::Instant::now();
        while !stop_flag.load(Ordering::SeqCst) && start.elapsed() < timeout {
            thread::sleep(Duration::from_millis(5));
        }
        if !stop_flag.load(Ordering::SeqCst) {
            worker_ctx.cancel();
        }
    });
    let cleanup_err = fn_(&ctx).err();
    stop.store(true, Ordering::SeqCst);
    // Go: multierr.Combine(cleanupErr, *errOut) — cleanup first.
    if let Some(existing) = err_out.take() {
        *err_out = Join(&[cleanup_err, Some(existing)]);
    } else {
        *err_out = cleanup_err;
    }
}

/// 捕获当前调用栈，供退出时 goroutine/栈 dump（Rust 用 Backtrace 近似）。
pub fn AllStackInfo() -> Vec<u8> {
    std::backtrace::Backtrace::force_capture()
        .to_string()
        .into_bytes()
}

/// 为 true 时退出监听会在 process::exit 前打印栈信息。
pub static DumpGoroutineWhenExit: AtomicBool = AtomicBool::new(false);

/// 启动退出信号监听线程，返回 child context 与其取消句柄。
/// 首次收到信号时可选 dump 并取消 child；再次收到退出信号后 `exit(1)`。
pub fn StartExitSingleListener(
    ctx: crate::stubs::context::Context,
) -> (
    crate::stubs::context::Context,
    crate::stubs::context::Context,
) {
    let child = ctx.child_token();
    let cancel = child.clone();
    match Signals::new([SIGHUP, SIGINT, SIGTERM, SIGQUIT]) {
        Ok(mut signals) => {
            let signal_cancel = cancel.clone();
            thread::spawn(move || {
                let Some(signal) = signals.forever().next() else {
                    return;
                };
                let padding = "=".repeat(8);
                let print_delim = |label: &str| {
                    let _ = writeln!(io::stdout(), "{padding}[ {label} ]{padding}");
                };
                let _ = writeln!(io::stdout());
                print_delim(&format!("Got signal {signal} to exit."));
                let dump_goroutine = DumpGoroutineWhenExit.load(Ordering::Relaxed);
                print_delim(&format!("Required Goroutine Dump = {dump_goroutine}"));
                if dump_goroutine {
                    print_delim("Start Dumping Goroutine");
                    let _ = io::stdout().write_all(&AllStackInfo());
                    print_delim("End of Dumping Goroutine");
                }
                log::Warn(
                    "received signal to exit",
                    [Field::string("signal", &signal.to_string())],
                );
                signal_cancel.cancel();
                let _ = writeln!(
                    io::stderr(),
                    "gracefully shutting down, press ^C again to force exit"
                );
                if signals.forever().next().is_some() {
                    std::process::exit(1);
                }
            });
        }
        Err(err) => log::Warn(
            "failed to register exit signal listener",
            [Field::string("error", &err.to_string())],
        ),
    }
    (child, cancel)
}

/// 收集 HashMap 全部 value（顺序不确定）。
pub fn Values<K: Eq + std::hash::Hash, V: Clone>(m: &HashMap<K, V>) -> Vec<V> {
    m.values().cloned().collect()
}

/// 展平 `HashMap<K, Vec<V>>` 为单一 Vec，预分配总容量。
pub fn FlattenValues<K: Eq + std::hash::Hash, V: Clone>(m: &HashMap<K, Vec<V>>) -> Vec<V> {
    let total: usize = m.values().map(Vec::len).sum();
    let mut result = Vec::with_capacity(total);
    for values in m.values() {
        result.extend(values.iter().cloned());
    }
    result
}

/// 按分区名查找分区 ID；无分区信息或未找到时返回错误（拼写 parition 对齐 Go）。
pub fn GetPartitionByName(table_info: &TableInfo, name: CIStr) -> Result<i64, SharedError> {
    let Some(partition) = table_info.Partition.as_ref() else {
        return Err(SharedError::new(std::io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "the table {}[id={}] does not have parition",
                table_info.Name.O, table_info.ID
            ),
        )));
    };
    let part_id = partition.GetPartitionIDByName(&name.L);
    if part_id > 0 {
        return Ok(part_id);
    }
    Err(SharedError::new(std::io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "partition is not found in the table {}[id={}]",
            table_info.Name.O, table_info.ID
        ),
    )))
}

/// 汇总备份文件：crc64xor 异或、kvs/bytes 累加，并按 CF 计数写入 summary。
pub fn SummaryFiles(files: &[File]) -> (u64, u64, u64) {
    let mut crc = 0u64;
    let mut kvs = 0u64;
    let mut bytes = 0u64;
    let mut cf_count: HashMap<String, i32> = HashMap::new();
    for file in files {
        *cf_count.entry(file.get_cf().to_string()).or_default() += 1;
        CollectSuccessUnit(TotalKV, 1, SummaryValue::UInt64(file.get_total_kvs()));
        CollectSuccessUnit(TotalBytes, 1, SummaryValue::UInt64(file.get_total_bytes()));
        crc ^= file.get_crc64xor();
        kvs += file.get_total_kvs();
        bytes += file.get_total_bytes();
    }
    for (cf, count) in cf_count {
        CollectInt(&format!("{cf} CF files"), count);
    }
    (crc, kvs, bytes)
}
