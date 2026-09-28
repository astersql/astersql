// Copyright 2019-present PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// unistore 配置定义与解析（对应 Go `config` 包）。
//
// 覆盖 Server / Engine / RaftStore / Coprocessor / PessimisticTxn 各配置段，
// 以及压缩算法解析、默认配置与 duration 字符串解析。

use serde::{Deserialize, Serialize};
use std::time::Duration;

// Config contains configuration options.
// Config 对应 Go 的顶层配置结构，字段顺序和 toml 分组保持一致。
/// unistore 顶层配置：按 TOML 分组聚合各子系统选项。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(rename = "server")]
    /// 服务端监听与 Region 规模等。
    pub Server: Server, // toml:"server"
    #[serde(rename = "engine")]
    /// 存储引擎（类 Badger）相关参数。
    pub Engine: Engine, // toml:"engine"
    #[serde(rename = "raftstore")]
    /// Raft 心跳、选举与租约等。
    pub RaftStore: RaftStore, // toml:"raftstore"
    #[serde(rename = "coprocessor")]
    /// Coprocessor（下推计算）Region 切分阈值。
    pub Coprocessor: Coprocessor, // toml:"coprocessor"
    #[serde(rename = "pessimistic-txn")]
    /// 悲观事务锁等待相关。
    pub PessimisticTxn: PessimisticTxn, // toml:"pessimistic-txn"
}

// Server is the config for server.
// Server 对应 Go 的 server 配置段，只保留字段和默认值语义。
/// 服务端配置段：PD/Store/Status 地址、日志与 Region 平均大小等。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Server {
    #[serde(rename = "pd-addr")]
    /// PD（Placement Driver，集群元数据与调度中心）地址。
    pub PDAddr: String, // toml:"pd-addr"
    #[serde(rename = "store-addr")]
    /// Store gRPC 服务地址。
    pub StoreAddr: String, // toml:"store-addr"
    #[serde(rename = "status-addr")]
    /// 状态/HTTP 服务地址。
    pub StatusAddr: String, // toml:"status-addr"
    #[serde(rename = "log-level")]
    /// 日志级别字符串。
    pub LogLevel: String, // toml:"log-level"
    #[serde(rename = "region-size")]
    /// 平均 Region 大小（字节）。
    pub RegionSize: i64, // toml:"region-size"; Average region size.
    #[serde(rename = "max-procs")]
    /// 可用 CPU 核数上限，0 表示全部。
    pub MaxProcs: i32, // toml:"max-procs"; Max CPU cores to use, 0 means all cores.
    #[serde(rename = "raft")]
    /// 是否启用 Raft。
    pub Raft: bool, // toml:"raft"; Enable raft.
    #[serde(rename = "log-file")]
    /// unistore 服务日志文件路径。
    pub LogfilePath: String, // toml:"log-file"; Log file path for unistore server.
}

// RaftStore is the config for raft store.
// RaftStore 对应 Go 的 raftstore 配置段，duration 字段仍保留字符串形式。
/// RaftStore 配置：心跳/租约/选举 tick 等（duration 仍为字符串）。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RaftStore {
    #[serde(rename = "pd-heartbeat-tick-interval")]
    /// 向 PD 汇报心跳的 tick 间隔字符串。
    pub PdHeartbeatTickInterval: String, // toml:"pd-heartbeat-tick-interval"; seconds.
    #[serde(rename = "raft-store-max-leader-lease")]
    /// Leader 租约上限字符串。
    pub RaftStoreMaxLeaderLease: String, // toml:"raft-store-max-leader-lease"; milliseconds.
    #[serde(rename = "raft-base-tick-interval")]
    /// Raft 基础 tick 间隔字符串。
    pub RaftBaseTickInterval: String, // toml:"raft-base-tick-interval"; milliseconds.
    #[serde(rename = "raft-heartbeat-ticks")]
    /// 心跳间隔对应的 tick 数。
    pub RaftHeartbeatTicks: i32, // toml:"raft-heartbeat-ticks"
    #[serde(rename = "raft-election-timeout-ticks")]
    /// 选举超时对应的 tick 数。
    pub RaftElectionTimeoutTicks: i32, // toml:"raft-election-timeout-ticks"
    #[serde(rename = "custom-raft-log")]
    /// 是否使用自定义 Raft 日志实现。
    pub CustomRaftLog: bool, // toml:"custom-raft-log"
}

// Coprocessor is the config for coprocessor.
// Coprocessor 对应 Go 的 coprocessor 配置段。
/// Coprocessor 配置：Region 键数量上下限，用于触发拆分。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Coprocessor {
    #[serde(rename = "region-max-keys")]
    /// Region 允许的最大键数量。
    pub RegionMaxKeys: i64, // toml:"region-max-keys"
    #[serde(rename = "region-split-keys")]
    /// 触发 Region 拆分的键数量阈值。
    pub RegionSplitKeys: i64, // toml:"region-split-keys"
}

// Engine is the config for engine.
// Engine 对应 Go 的 badger/unistore 引擎配置，字段顺序和注释沿用来源文件。
/// 存储引擎配置：路径、MemTable/SST、压缩、缓存与 compact 行为。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Engine {
    #[serde(rename = "db-path")]
    /// 数据目录路径。
    pub DBPath: String, // toml:"db-path"; Directory to store data in.
    #[serde(rename = "value-threshold")]
    /// 大 value 阈值：超过则树中只存偏移。
    pub ValueThreshold: i32, // toml:"value-threshold"; large values store offsets in tree.
    #[serde(rename = "max-mem-table-size")]
    /// 单个 MemTable 最大字节数。
    pub MaxMemTableSize: i64, // toml:"max-mem-table-size"; Each mem table is at most this size.
    #[serde(rename = "max-table-size")]
    /// 单个 table 文件最大字节数。
    pub MaxTableSize: i64, // toml:"max-table-size"; Each table file is at most this size.
    #[serde(rename = "l1-size")]
    /// L1 层目标大小。
    pub L1Size: i64, // toml:"l1-size"
    #[serde(rename = "num-mem-tables")]
    /// 内存中最多保留的 MemTable 数。
    pub NumMemTables: i32, // toml:"num-mem-tables"; Maximum tables kept in memory.
    #[serde(rename = "num-L0-tables")]
    /// 触发 compact 前的 L0 table 数。
    pub NumL0Tables: i32, // toml:"num-L0-tables"; L0 tables before compacting.
    #[serde(rename = "num-L0-tables-stall")]
    /// 写停顿前的 L0 table 数。
    pub NumL0TablesStall: i32, // toml:"num-L0-tables-stall"; L0 tables before stalling.
    #[serde(rename = "vlog-file-size")]
    /// Value log 单文件大小。
    pub VlogFileSize: i64, // toml:"vlog-file-size"; Value log file size.

    // Sync all writes to disk. Setting this to true would slow down data loading significantly.
    #[serde(rename = "sync-write")]
    /// 是否每次写都 fsync（会显著拖慢加载）。
    pub SyncWrite: bool, // toml:"sync-write"
    #[serde(rename = "num-compactors")]
    /// 并发 compact 线程数。
    pub NumCompactors: i32, // toml:"num-compactors"
    #[serde(rename = "surf-start-level")]
    /// SuRF 索引起始层。
    pub SurfStartLevel: i32, // toml:"surf-start-level"
    #[serde(rename = "block-cache-size")]
    /// Block 缓存大小，0 表示禁用并用 mmap 访问 SST。
    pub BlockCacheSize: i64, // toml:"block-cache-size"
    #[serde(rename = "index-cache-size")]
    /// 索引缓存大小。
    pub IndexCacheSize: i64, // toml:"index-cache-size"
    #[serde(rename = "compression")]
    /// 各层压缩算法名列表。
    pub Compression: Vec<String>, // toml:"compression"; Compression types for each level.
    #[serde(rename = "ingest-compression")]
    /// Ingest 时使用的压缩算法名。
    pub IngestCompression: String, // toml:"ingest-compression"

    // Only used in tests.
    #[serde(skip)]
    /// 仅测试：易失模式开关。
    pub VolatileMode: bool,

    #[serde(rename = "compact-l0-when-close")]
    /// 关闭时是否 compact L0。
    pub CompactL0WhenClose: bool, // toml:"compact-l0-when-close"
}

// PessimisticTxn is the config for pessimistic txn.
// PessimisticTxn 对应 Go 的悲观事务等待配置。
/// 悲观事务（写时加锁、冲突时等待）锁等待相关配置。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PessimisticTxn {
    // The default and maximum delay in milliseconds before responding to TiDB when pessimistic
    // transactions encounter locks.
    #[serde(rename = "wait-for-lock-timeout")]
    /// 遇到锁时等待超时（毫秒）。
    pub WaitForLockTimeout: i64, // toml:"wait-for-lock-timeout"

    // The duration between waking up lock waiter, in milliseconds.
    #[serde(rename = "wake-up-delay-duration")]
    /// 唤醒锁等待者的间隔（毫秒）。
    pub WakeUpDelayDuration: i64, // toml:"wake-up-delay-duration"
}

/// Compression algorithms accepted by the unistore engine configuration.
///
/// 引擎配置可识别的压缩算法枚举。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompressionType {
    /// 不压缩。
    None,
    /// Snappy。
    Snappy,
    /// Zstandard。
    Zstd,
}

// ParseCompression parses the string s and returns a compression type.
// ParseCompression 对应 Go 的 switch：只识别 snappy 和 zstd，其余返回 options.None。
/// 解析压缩算法名：仅识别小写 `snappy`/`zstd`，其余为 None。
#[allow(non_snake_case)]
pub fn ParseCompression(s: &str) -> CompressionType {
    match s {
        "snappy" => CompressionType::Snappy,
        "zstd" => CompressionType::Zstd,
        _ => CompressionType::None,
    }
}

// MB represents the MB size.
/// 1 MiB 字节常量。
pub const MB: i64 = 1024 * 1024;

// DefaultConf returns the default configuration.
// Go 中 DefaultConf 是包级变量；用 LazyLock 保留“首次访问时构造 Vec”的默认配置形状。
/// 默认配置（LazyLock，首次访问时构造，形状对齐 Go 包级变量）。
#[allow(non_upper_case_globals)]
pub static DefaultConf: std::sync::LazyLock<Config> = std::sync::LazyLock::new(|| Config {
    Server: Server {
        PDAddr: "127.0.0.1:2379".to_string(),
        StoreAddr: "127.0.0.1:9191".to_string(),
        StatusAddr: "127.0.0.1:9291".to_string(),
        RegionSize: 64 * MB,
        LogLevel: "info".to_string(),
        MaxProcs: 0,
        Raft: true,
        LogfilePath: String::new(),
    },
    RaftStore: RaftStore {
        PdHeartbeatTickInterval: "20s".to_string(),
        RaftStoreMaxLeaderLease: "9s".to_string(),
        RaftBaseTickInterval: "1s".to_string(),
        RaftHeartbeatTicks: 2,
        RaftElectionTimeoutTicks: 10,
        CustomRaftLog: true,
    },
    Engine: Engine {
        DBPath: "/tmp/badger".to_string(),
        ValueThreshold: 256,
        MaxMemTableSize: 64 * MB,
        MaxTableSize: 8 * MB,
        NumMemTables: 3,
        NumL0Tables: 4,
        NumL0TablesStall: 8,
        VlogFileSize: 256 * MB,
        NumCompactors: 3,
        SurfStartLevel: 8,
        L1Size: 512 * MB,
        // Go 的 make([]string, 7) 生成 7 个空字符串；这里保留同样的层级占位。
        Compression: vec![String::new(); 7],
        BlockCacheSize: 0, // 0 means disable block cache, use mmap to access sst.
        IndexCacheSize: 0,
        CompactL0WhenClose: true,
        SyncWrite: false,
        IngestCompression: String::new(),
        VolatileMode: false,
    },
    Coprocessor: Coprocessor {
        RegionMaxKeys: 1_440_000,
        RegionSplitKeys: 960_000,
    },
    PessimisticTxn: PessimisticTxn {
        WaitForLockTimeout: 1000, // 1000ms same with tikv default value.
        WakeUpDelayDuration: 100, // 100ms same with tikv default value.
    },
});

// Mirrors Go time.ParseDuration's signed nanosecond parser.
fn parse_go_duration(input: &str) -> Result<i64, ()> {
    let bytes = input.as_bytes();
    let mut index = 0;
    let mut negative = false;
    let limit = 1_u64 << 63;

    if let Some(sign) = bytes.first() {
        if *sign == b'-' || *sign == b'+' {
            negative = *sign == b'-';
            index += 1;
        }
    }
    if &bytes[index..] == b"0" {
        return Ok(0);
    }
    if index == bytes.len() {
        return Err(());
    }

    let mut total = 0_u64;
    while index < bytes.len() {
        if bytes[index] != b'.' && !bytes[index].is_ascii_digit() {
            return Err(());
        }

        let integer_start = index;
        let (mut value, next) = parse_duration_leading_int(bytes, index)?;
        index = next;
        let has_integer = index != integer_start;

        let mut fraction = 0_u64;
        let mut scale = 1_f64;
        let mut has_fraction = false;
        if index < bytes.len() && bytes[index] == b'.' {
            index += 1;
            let fraction_start = index;
            (fraction, scale, index) = parse_duration_leading_fraction(bytes, index);
            has_fraction = index != fraction_start;
        }
        if !has_integer && !has_fraction {
            return Err(());
        }

        let unit_start = index;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'.' || byte.is_ascii_digit() {
                break;
            }
            index += 1;
        }
        if index == unit_start {
            return Err(());
        }
        let unit_name = input.get(unit_start..index).ok_or(())?;
        let unit = match unit_name {
            "ns" => 1,
            "us" | "µs" | "μs" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60_000_000_000,
            "h" => 3_600_000_000_000,
            _ => return Err(()),
        };

        if value > limit / unit {
            return Err(());
        }
        value *= unit;
        if fraction > 0 {
            value = value
                .checked_add((fraction as f64 * (unit as f64 / scale)) as u64)
                .ok_or(())?;
            if value > limit {
                return Err(());
            }
        }
        // Go accumulates into uint64 before checking the signed-duration bound.
        total = total.wrapping_add(value);
        if total > limit {
            return Err(());
        }
    }

    if negative {
        if total == limit {
            Ok(i64::MIN)
        } else {
            Ok(-(total as i64))
        }
    } else if total > i64::MAX as u64 {
        Err(())
    } else {
        Ok(total as i64)
    }
}

fn parse_duration_leading_int(bytes: &[u8], mut index: usize) -> Result<(u64, usize), ()> {
    let mut value = 0_u64;
    let limit = 1_u64 << 63;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        if value > limit / 10 {
            return Err(());
        }
        value = value * 10 + u64::from(bytes[index] - b'0');
        if value > limit {
            return Err(());
        }
        index += 1;
    }
    Ok((value, index))
}

fn parse_duration_leading_fraction(bytes: &[u8], mut index: usize) -> (u64, f64, usize) {
    let mut value = 0_u64;
    let mut scale = 1_f64;
    let mut overflow = false;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        if !overflow {
            if value > (i64::MAX as u64) / 10 {
                overflow = true;
            } else {
                let next = value * 10 + u64::from(bytes[index] - b'0');
                if next > 1_u64 << 63 {
                    overflow = true;
                } else {
                    value = next;
                    scale *= 10_f64;
                }
            }
        }
        index += 1;
    }
    (value, scale, index)
}

// ParseDuration parses duration argument string.
// Go 先按原字符串解析，失败后追加 "s" 再解析；负数或再次失败会 Fatalf 退出进程。
/// 解析 duration 字符串；失败则追加 `s` 再试，仍失败或负数则 panic。
#[allow(non_snake_case)]
pub fn ParseDuration(durationStr: &str) -> Duration {
    let mut parsed = parse_go_duration(durationStr);
    if parsed.is_err() {
        // 与 Go 一致：无单位时按秒回退解析。
        parsed = parse_go_duration(&format!("{}s", durationStr));
    }

    match parsed {
        Ok(nanos) if nanos >= 0 => Duration::from_nanos(nanos as u64),
        Err(_) => panic!("invalid duration={durationStr}"),
        Ok(_) => panic!("invalid duration={durationStr}"),
    }
}
