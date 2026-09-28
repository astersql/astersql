// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 遥测滑动窗口统计：子窗口轮转、原子计数与内置函数使用聚合。
//
// 每分钟（SubWindow）将当前原子计数 swap 归零并入队；最多保留 360 个子窗口
//（约 6 小时）。读取时按 60 个子窗口合并为一个小时级窗口上报。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// 批量声明当前子窗口内的原子计数器。
macro_rules! counters{($($n:ident),*)=>{$(pub static $n:AtomicU64=AtomicU64::new(0);)*}}
counters!(
    CurrentExecuteCount,
    CurrentTiFlashPushDownCount,
    CurrentTiFlashExchangePushDownCount,
    CurrentCoprCacheHitRatioGTE0Count,
    CurrentCoprCacheHitRatioGTE1Count,
    CurrentCoprCacheHitRatioGTE10Count,
    CurrentCoprCacheHitRatioGTE20Count,
    CurrentCoprCacheHitRatioGTE40Count,
    CurrentCoprCacheHitRatioGTE80Count,
    CurrentCoprCacheHitRatioGTE100Count,
    CurrentTiflashTableScanCount,
    CurrentTiflashTableScanWithFastScanCount
);
/// 上报用小时窗口大小（1 小时）。
pub const WindowSize: Duration = Duration::from_secs(3600);
/// 底层子窗口大小（1 分钟）。
pub const SubWindowSize: Duration = Duration::from_secs(60);
/// 内存中最多保留的子窗口个数（360 分钟 ≈ 6 小时）。
const MAX_SUB_WINDOWS: usize = 360;
/// 合并上报时每个小时窗口包含的子窗口数。
const IN_WINDOW: usize = 60;
/// 内置函数名到调用次数的映射。
pub type BuiltinFunctionsUsage = HashMap<String, u32>;
/// 对内置函数使用表的自增与合并扩展。
pub trait BuiltinUsageExt {
    /// 指定函数调用次数 +1。
    fn Inc(&mut self, name: &str);
    /// 将另一张表的计数累加到自身。
    fn Merge(&mut self, other: &BuiltinFunctionsUsage);
}
impl BuiltinUsageExt for BuiltinFunctionsUsage {
    fn Inc(&mut self, name: &str) {
        let count = self.entry(name.into()).or_default();
        *count = count.wrapping_add(1);
    }
    fn Merge(&mut self, other: &BuiltinFunctionsUsage) {
        for (k, v) in other {
            let count = self.entry(k.clone()).or_default();
            *count = count.wrapping_add(*v);
        }
    }
}
/// 全局内置函数使用收集器，线程安全累加后可 Dump 取出。
pub struct builtinFunctionsUsageCollector(Mutex<BuiltinFunctionsUsage>);
impl builtinFunctionsUsageCollector {
    /// 合并一批函数使用计数到全局表。
    pub fn Collect(&self, data: BuiltinFunctionsUsage) {
        self.0.lock().expect("builtin lock poisoned").Merge(&data)
    }
    /// 取出并清空当前累计，供子窗口快照使用。
    pub fn Dump(&self) -> BuiltinFunctionsUsage {
        std::mem::take(&mut *self.0.lock().expect("builtin lock poisoned"))
    }
}
/// 进程级全局内置函数使用收集器。
pub static GlobalBuiltinFunctionsUsage: LazyLock<builtinFunctionsUsageCollector> =
    LazyLock::new(|| builtinFunctionsUsageCollector(Mutex::new(HashMap::new())));
fn builtin() -> &'static builtinFunctionsUsageCollector {
    &GlobalBuiltinFunctionsUsage
}
/// Coprocessor 缓存命中率分桶计数（命中率 ≥ 各阈值的次数）。
#[derive(Clone, Debug, Default)]
pub struct coprCacheUsageData {
    pub GTE0: u64,
    pub GTE1: u64,
    pub GTE10: u64,
    pub GTE20: u64,
    pub GTE40: u64,
    pub GTE80: u64,
    pub GTE100: u64,
}
/// TiFlash（列存加速引擎）下推与扫描相关计数。
#[derive(Clone, Debug, Default)]
pub struct tiFlashUsageData {
    /// 下推到 TiFlash 的次数。
    pub PushDown: u64,
    /// Exchange 算子下推次数。
    pub ExchangePushDown: u64,
    /// TiFlash TableScan 次数。
    pub TableScan: u64,
    /// 启用 FastScan 的 TableScan 次数。
    pub TableScanWithFastScan: u64,
}
/// 单个子窗口（或合并后小时窗口）的统计快照。
#[derive(Clone, Debug)]
pub struct windowData {
    /// 子窗口起始时间。
    pub BeginAt: SystemTime,
    /// 该窗口内语句执行次数。
    pub ExecuteCount: u64,
    /// TiFlash 相关使用。
    pub TiFlashUsage: tiFlashUsageData,
    /// Coprocessor 缓存命中分桶。
    pub CoprCacheUsage: coprCacheUsageData,
    /// 内置函数调用统计。
    pub BuiltinFunctionsUsage: BuiltinFunctionsUsage,
}
impl windowData {
    /// 将时间编码为 Go `time.Time.MarshalJSON` 兼容的 RFC3339Nano UTC 字符串。
    fn format_time(time: SystemTime) -> String {
        let elapsed = time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let days = (elapsed.as_secs() / 86_400) as i64;
        let seconds = elapsed.as_secs() % 86_400;
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = mp + if mp < 10 { 3 } else { -9 };
        let year = y + (month <= 2) as i64;
        let hour = seconds / 3_600;
        let minute = (seconds % 3_600) / 60;
        let second = seconds % 60;
        let nanos = elapsed.subsec_nanos();
        if nanos == 0 {
            format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
        } else {
            let fraction = format!("{nanos:09}").trim_end_matches('0').to_owned();
            format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{fraction}Z")
        }
    }

    /// 序列化为与 Go `encoding/json` tags 一致的完整 JSON。
    pub fn Marshal(&self) -> String {
        let mut builtin = self.BuiltinFunctionsUsage.iter().collect::<Vec<_>>();
        // encoding/json sorts string map keys, making reports deterministic.
        builtin.sort_by(|(left, _), (right, _)| left.cmp(right));
        let builtin = builtin
            .into_iter()
            .map(|(name, count)| format!("{}:{}", json_quote(name), count))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"beginAt\":\"{}\",\"executeCount\":{},\"tiFlashUsage\":{{\"pushDown\":{},\"exchangePushDown\":{},\"tableScan\":{},\"tableScanWithFastScan\":{}}},\"coprCacheUsage\":{{\"gte0\":{},\"gte1\":{},\"gte10\":{},\"gte20\":{},\"gte40\":{},\"gte80\":{},\"gte100\":{}}},\"builtinFunctionsUsage\":{{{}}}}}",
            Self::format_time(self.BeginAt),
            self.ExecuteCount,
            self.TiFlashUsage.PushDown,
            self.TiFlashUsage.ExchangePushDown,
            self.TiFlashUsage.TableScan,
            self.TiFlashUsage.TableScanWithFastScan,
            self.CoprCacheUsage.GTE0,
            self.CoprCacheUsage.GTE1,
            self.CoprCacheUsage.GTE10,
            self.CoprCacheUsage.GTE20,
            self.CoprCacheUsage.GTE40,
            self.CoprCacheUsage.GTE80,
            self.CoprCacheUsage.GTE100,
            builtin,
        )
    }
}

fn json_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
/// 返回全局子窗口环形缓冲的互斥访问入口。
fn windows() -> &'static Mutex<Vec<windowData>> {
    static W: OnceLock<Mutex<Vec<windowData>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(vec![]))
}

/// 轮转当前子窗口：原子计数 swap 归零入库，超出上限时丢弃最旧条目。
pub fn RotateSubWindow() {
    // 用 AcqRel 保证并发递增与本次快照之间的可见性。
    let w = windowData {
        BeginAt: SystemTime::now(),
        ExecuteCount: CurrentExecuteCount.swap(0, Ordering::AcqRel),
        TiFlashUsage: tiFlashUsageData {
            PushDown: CurrentTiFlashPushDownCount.swap(0, Ordering::AcqRel),
            ExchangePushDown: CurrentTiFlashExchangePushDownCount.swap(0, Ordering::AcqRel),
            TableScan: CurrentTiflashTableScanCount.swap(0, Ordering::AcqRel),
            TableScanWithFastScan: CurrentTiflashTableScanWithFastScanCount
                .swap(0, Ordering::AcqRel),
        },
        CoprCacheUsage: coprCacheUsageData {
            GTE0: CurrentCoprCacheHitRatioGTE0Count.swap(0, Ordering::AcqRel),
            GTE1: CurrentCoprCacheHitRatioGTE1Count.swap(0, Ordering::AcqRel),
            GTE10: CurrentCoprCacheHitRatioGTE10Count.swap(0, Ordering::AcqRel),
            GTE20: CurrentCoprCacheHitRatioGTE20Count.swap(0, Ordering::AcqRel),
            GTE40: CurrentCoprCacheHitRatioGTE40Count.swap(0, Ordering::AcqRel),
            GTE80: CurrentCoprCacheHitRatioGTE80Count.swap(0, Ordering::AcqRel),
            GTE100: CurrentCoprCacheHitRatioGTE100Count.swap(0, Ordering::AcqRel),
        },
        BuiltinFunctionsUsage: builtin().Dump(),
    };
    let mut all = windows().lock().expect("windows lock poisoned");
    all.push(w);
    // 保留最近 MAX_SUB_WINDOWS 个子窗口，丢弃更早的数据。
    let excess = all.len().saturating_sub(MAX_SUB_WINDOWS);
    if excess > 0 {
        all.drain(..excess);
    }
}

/// 将源子窗口的计数字段累加到目标窗口（用于小时级合并）。
pub(crate) fn merge(a: &mut windowData, b: &windowData) {
    a.ExecuteCount = a.ExecuteCount.wrapping_add(b.ExecuteCount);
    a.TiFlashUsage.PushDown = a
        .TiFlashUsage
        .PushDown
        .wrapping_add(b.TiFlashUsage.PushDown);
    a.TiFlashUsage.ExchangePushDown = a
        .TiFlashUsage
        .ExchangePushDown
        .wrapping_add(b.TiFlashUsage.ExchangePushDown);
    a.TiFlashUsage.TableScan = a
        .TiFlashUsage
        .TableScan
        .wrapping_add(b.TiFlashUsage.TableScan);
    a.TiFlashUsage.TableScanWithFastScan = a
        .TiFlashUsage
        .TableScanWithFastScan
        .wrapping_add(b.TiFlashUsage.TableScanWithFastScan);
    a.CoprCacheUsage.GTE0 = a.CoprCacheUsage.GTE0.wrapping_add(b.CoprCacheUsage.GTE0);
    a.CoprCacheUsage.GTE1 = a.CoprCacheUsage.GTE1.wrapping_add(b.CoprCacheUsage.GTE1);
    a.CoprCacheUsage.GTE10 = a.CoprCacheUsage.GTE10.wrapping_add(b.CoprCacheUsage.GTE10);
    a.CoprCacheUsage.GTE20 = a.CoprCacheUsage.GTE20.wrapping_add(b.CoprCacheUsage.GTE20);
    a.CoprCacheUsage.GTE40 = a.CoprCacheUsage.GTE40.wrapping_add(b.CoprCacheUsage.GTE40);
    a.CoprCacheUsage.GTE80 = a.CoprCacheUsage.GTE80.wrapping_add(b.CoprCacheUsage.GTE80);
    a.CoprCacheUsage.GTE100 = a
        .CoprCacheUsage
        .GTE100
        .wrapping_add(b.CoprCacheUsage.GTE100);
    a.BuiltinFunctionsUsage.Merge(&b.BuiltinFunctionsUsage)
}

/// 按 IN_WINDOW 个子窗口合并，返回小时级窗口数据列表供上报。
pub fn getWindowData() -> Vec<windowData> {
    windows()
        .lock()
        .expect("windows lock poisoned")
        .chunks(IN_WINDOW)
        .map(|chunk| {
            // 以首个子窗口为基底，累加同组其余子窗口。
            let mut out = chunk[0].clone();
            for item in &chunk[1..] {
                merge(&mut out, item)
            }
            out
        })
        .collect()
}
