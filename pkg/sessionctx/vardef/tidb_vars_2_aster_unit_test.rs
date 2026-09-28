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

// `tidb_vars` 核心类型与全局原子访问的 Aster 迁移单元测试。
//
// 覆盖 Exchange 压缩模式、作用域/类型 iota、聚簇索引选项、DDL 原子读写、
// 进程全局默认值，以及时间解析与 NextGen MDL/断言默认值。

use astersql_sessionctx_vardef::*;

struct DdlGlobalsRestore {
    reorg_workers: i32,
    flashback_concurrency: i32,
    reorg_batch_size: i32,
    error_count_limit: i64,
    row_format: i64,
    max_delta_schema_count: i64,
}

impl Drop for DdlGlobalsRestore {
    fn drop(&mut self) {
        SetDDLReorgWorkerCounter(self.reorg_workers);
        SetDDLFlashbackConcurrency(self.flashback_concurrency);
        SetDDLReorgBatchSize(self.reorg_batch_size);
        SetDDLErrorCountLimit(self.error_count_limit);
        SetDDLReorgRowFormat(self.row_format);
        SetMaxDeltaSchemaCount(self.max_delta_schema_count);
    }
}

struct ProcessGlobalsRestore {
    general_log: bool,
    oom_action: String,
    memory_alarm_ratio: f64,
}

impl Drop for ProcessGlobalsRestore {
    fn drop(&mut self) {
        ProcessGeneralLog.Store(self.general_log);
        OOMAction.Store(&self.oom_action);
        MemoryUsageAlarmRatio.Store(self.memory_alarm_ratio);
    }
}

struct EnableMdlRestore(bool);

impl Drop for EnableMdlRestore {
    fn drop(&mut self) {
        SetEnableMDL(self.0);
    }
}

#[test]
/// 压缩模式名称解析、tipb 映射与 Name() 字符串与 Go 一致。
fn exchange_compression_mode_matches_tipb_names() {
    let cases = [
        (
            "none",
            ExchangeCompressionModeNONE,
            CompressionMode::None,
            "NONE",
        ),
        (
            "FAST",
            ExchangeCompressionModeFast,
            CompressionMode::Fast,
            "FAST",
        ),
        (
            "high_compression",
            ExchangeCompressionModeHC,
            CompressionMode::HighCompression,
            "HIGH_COMPRESSION",
        ),
        (
            "unspecified",
            ExchangeCompressionModeUnspecified,
            CompressionMode::None,
            "UNSPECIFIED",
        ),
    ];

    for (input, expected, tipb, name) in cases {
        assert_eq!(ToExchangeCompressionMode(input), (expected, true));
        assert_eq!(expected.ToTipbCompressionMode(), tipb);
        assert_eq!(expected.Name(), name);
    }
    assert_eq!(
        ToExchangeCompressionMode("not-a-compression-mode"),
        (ExchangeCompressionModeNONE, false)
    );
}

#[test]
/// ScopeFlag 字符串化与 TypeFlag 数值顺序对齐 Go iota。
fn scope_and_type_flags_preserve_go_iota_values() {
    assert_eq!(ScopeNone.String(), "NONE");
    assert_eq!(ScopeSession.String(), "SESSION");
    assert_eq!((ScopeSession | ScopeGlobal).String(), "SESSION,GLOBAL");
    assert_eq!(
        (ScopeInstance | ScopeGlobal | ScopeSession).String(),
        "SESSION,GLOBAL,INSTANCE"
    );
    assert_eq!(
        [
            TypeStr,
            TypeBool,
            TypeInt,
            TypeEnum,
            TypeFloat,
            TypeUnsigned,
            TypeTime,
            TypeDuration
        ],
        [0, 1, 2, 3, 4, 5, 6, 7]
    );
}

#[test]
/// `TiDBOptEnableClustered` 对 ON/OFF/其它取值的分支与 Go switch 一致。
fn clustered_index_option_matches_go_switch() {
    assert_eq!(TiDBOptEnableClustered(On), ClusteredIndexDefModeOn);
    assert_eq!(TiDBOptEnableClustered(Off), ClusteredIndexDefModeOff);
    assert_eq!(
        TiDBOptEnableClustered("bogus"),
        ClusteredIndexDefModeIntOnly
    );
}

#[test]
/// DDL 相关原子计数器的 setter/getter 往返。
fn ddl_atomic_accessors_round_trip() {
    let _restore = DdlGlobalsRestore {
        reorg_workers: GetDDLReorgWorkerCounter(),
        flashback_concurrency: GetDDLFlashbackConcurrency(),
        reorg_batch_size: GetDDLReorgBatchSize(),
        error_count_limit: GetDDLErrorCountLimit(),
        row_format: GetDDLReorgRowFormat(),
        max_delta_schema_count: GetMaxDeltaSchemaCount(),
    };

    SetDDLReorgWorkerCounter(17);
    SetDDLFlashbackConcurrency(18);
    SetDDLReorgBatchSize(19);
    SetDDLErrorCountLimit(20);
    SetDDLReorgRowFormat(21);
    SetMaxDeltaSchemaCount(22);

    assert_eq!(GetDDLReorgWorkerCounter(), 17);
    assert_eq!(GetDDLFlashbackConcurrency(), 18);
    assert_eq!(GetDDLReorgBatchSize(), 19);
    assert_eq!(GetDDLErrorCountLimit(), 20);
    assert_eq!(GetDDLReorgRowFormat(), 21);
    assert_eq!(GetMaxDeltaSchemaCount(), 22);
}

#[test]
/// 进程全局状态默认值，以及若干 Atomic Store/Load 更新。
fn process_globals_keep_go_defaults_and_atomic_updates() {
    assert!(RunAutoAnalyze.Load());
    assert!(EnableAutoAnalyzePriorityQueue.Load());
    assert_eq!(AnalyzeColumnOptions.Load(), "ALL");
    assert_eq!(QueryLogMaxLen.Load(), 4096);
    assert_eq!(MemoryUsageAlarmRatio.Load(), 0.7);
    assert_eq!(LowResolutionTSOUpdateInterval.Load(), 2000);
    assert_eq!(OOMAction.Load(), "CANCEL");
    assert_eq!(TTLJobScheduleWindowStartTime.Load(), "00:00 +0000");
    assert_eq!(TTLJobScheduleWindowEndTime.Load(), "23:59 +0000");
    assert!(GlobalSlowLogRateLimiter.Allow());

    let _restore = ProcessGlobalsRestore {
        general_log: ProcessGeneralLog.Load(),
        oom_action: OOMAction.Load(),
        memory_alarm_ratio: MemoryUsageAlarmRatio.Load(),
    };

    ProcessGeneralLog.Store(true);
    OOMAction.Store("LOG");
    MemoryUsageAlarmRatio.Store(0.8);
    assert!(ProcessGeneralLog.Load());
    assert_eq!(OOMAction.Load(), "LOG");
    assert_eq!(MemoryUsageAlarmRatio.Load(), 0.8);
}

#[test]
/// `mustParseTime` 接受合法布局，非法时间应 panic。
fn time_parser_matches_go_layout_validation() {
    assert_eq!(
        mustParseTime(FullDayTimeFormat, "23:59 +0000"),
        "23:59 +0000"
    );
    assert_eq!(mustParseTime(LocalDayTimeFormat, "08:30"), "08:30");
    assert!(std::panic::catch_unwind(|| mustParseTime(FullDayTimeFormat, "25:00 +0000")).is_err());
}

#[test]
/// NextGen 下 MDL 恒开且断言默认 STRICT；Classic 可开关 MDL，默认断言 OFF。
fn kernel_specific_defaults_match_go() {
    let _restore = EnableMdlRestore(IsMDLEnabled());

    // 先尝试关闭 MDL，再按内核类型断言实际行为。
    SetEnableMDL(false);
    if kerneltype::IsNextGen() {
        assert!(IsMDLEnabled());
        assert_eq!(GetDefaultTxnAssertionLevel(), AssertionStrictStr);
    } else {
        assert!(!IsMDLEnabled());
        assert_eq!(GetDefaultTxnAssertionLevel(), AssertionOffStr);
        SetEnableMDL(true);
        assert!(IsMDLEnabled());
    }
}
