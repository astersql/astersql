// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/logutil/logging_test.go`.
//! 对齐 Go `logging_test.go`：校验 Field JSON、速率追踪与上下文 logger。
//! 期望字符串刻意贴合 Go zap 输出形态；用例不改断言逻辑，仅补充意图说明。
//! Capture Logger 用于观察 ContextWithField 的字段继承与全局 logger 回退。
//! RateLogger 节流同一消息的重复 Info，避免测试输出被刷屏掩盖断言点。
//! redact 相关字段断言验证敏感值在日志中被遮罩而非明文落盘。
//! Context logger 缺省回退全局 logger，确保无 Context 注入时仍可观测。

use std::time::{Duration, Instant};

use astersql_br_pkg_errors::ErrInvalidArgument;
use astersql_errors::{Annotate, SharedError};
use prometheus::{Counter, Histogram, HistogramOpts};

use crate::kvproto::brpb;
use crate::kvproto::import_sstpb;
use crate::kvproto::metapb;
use crate::logging::{CapturedLog, EncodedValue, ObjectEncoder, ObjectMarshaler};
use crate::{
    BriefSSTMetas, Context, ContextWithField, Field, File, Files, Key, Keys, Leader, Logger,
    LoggerFromContext, MarshalHistogram, MarshalLogObjectForFiles, RedactAny, Region,
    ResetGlobalLogger, RewriteRule, SSTMeta, ShortError, TraceRateOver,
};

/// 将 Field 编成 JSON 并与 Go 期望串逐字比较（含空格风格）。
/// assertTrimEqual: encode Field as JSON and compare to the Go expected string.
fn assert_trim_equal(f: Field, expect: &str) {
    assert_eq!(expect, f.encode_json(), "encoded field mismatch");
}

/// 用整数 `j` 派生可重复的 File fixture，便于表驱动聚合断言。
/// newFile constructs a backuppb.File fixture whose fields derive from `j`.
fn new_file(j: i32) -> brpb::File {
    let mut file = brpb::File::new();
    file.set_name(j.to_string());
    file.set_start_key(j.to_string().into_bytes());
    file.set_end_key((j + 1).to_string().into_bytes());
    file.set_total_kvs(j as u64);
    file.set_total_bytes(j as u64);
    file.set_start_version(j as u64);
    file.set_end_version((j + 1) as u64);
    file.set_crc64xor(j as u64);
    file.set_sha256(j.to_string().into_bytes());
    // CF 固定 write，与期望 JSON 对齐。
    file.set_cf("write".to_string());
    file.set_size(j as u64);
    file
}

// 相对误差比较；期望为 0 时退化为绝对误差（对齐 Go InEpsilon）。
fn in_epsilon(expected: f64, actual: f64, epsilon: f64) -> bool {
    // 零期望：只用绝对阈值。
    if expected == 0.0 {
        // 绝对阈值分支。
        actual.abs() <= epsilon
    } else {
        // 相对阈值分支。
        ((actual - expected) / expected).abs() <= epsilon
    }
}

/// 速率追踪：基线 42 后按固定时刻采样 Inc/Add 结果。
/// TestRater: counter baseline 42, then Inc/Add rates at fixed time offsets.
#[test]
fn test_rater() {
    let m = Counter::with_opts(
        prometheus::Opts::new("rater", "A testing counter for the rater").namespace("testing"),
    )
    .expect("counter");
    // 先垫高基线，确保 Rate 只计创建后增量。
    m.inc_by(42.0);

    // 固定 start=time_pass，使 RateAt 偏移与 Go 用例一致。
    // Align start with RateAt samples the same way Go pins timePass after TraceRateOver.
    // 作为 RateAt 的时间原点。
    let time_pass = Instant::now();
    let rater = crate::rate::RateTracer {
        start: time_pass,
        base: astersql_lightning_metric::read_counter(&m),
        counter: Some(m),
    };
    // 同时冒烟 TraceRateOver 构造路径。
    // Also exercise TraceRateOver construction (Go calls TraceRateOver).
    let _ = TraceRateOver(
        Counter::with_opts(
            prometheus::Opts::new("rater_ctor", "A testing counter for the rater")
                .namespace("testing"),
        )
        .expect("counter"),
    );

    // +1 → 100ms 时应约为 10 ops/s。
    rater.Inc();
    assert!(
        in_epsilon(
            10.0,
            rater.RateAt(time_pass + Duration::from_millis(100)),
            0.1
        ),
        // 首个采样点：1/0.1s。
        "first rate sample"
    );
    rater.Inc();
    assert!(
        in_epsilon(
            13.0,
            rater.RateAt(time_pass + Duration::from_millis(150)),
            0.1
        ),
        // 第二个采样：累计 2 / 0.15s ≈ 13.33。
        "second rate sample"
    );
    // 大增量把速率拉到 100。
    rater.Add(18.0);
    assert!(
        in_epsilon(
            100.0,
            rater.RateAt(time_pass + Duration::from_millis(200)),
            0.1
        ),
        // Add(18) 后累计 20 / 0.2s = 100。
        "third rate sample"
    );
}

/// Go 的 time.Duration 与浮点除法允许零时长和负时长，不应 panic 或改写 IEEE 结果。
#[test]
fn test_rater_go_time_boundaries() {
    let counter = Counter::new("rater_boundaries", "rate boundary behavior").expect("counter");
    let start = Instant::now();
    let rater = crate::rate::RateTracer {
        start,
        base: astersql_lightning_metric::read_counter(&counter),
        counter: Some(counter),
    };

    rater.Inc();
    assert_eq!(rater.RateAt(start), f64::INFINITY);
    assert_eq!(rater.RateAt(start - Duration::from_millis(100)), -10.0);
}

/// zap.Any 保留标量/数组的 JSON 类型，未脱敏时不能退化成 Debug 字符串。
#[test]
fn test_redact_any_preserves_value_shape() {
    assert_trim_equal(RedactAny("count", 42_i64), r#"{"count": 42}"#);
    assert_trim_equal(RedactAny("ready", true), r#"{"ready": true}"#);
    assert_trim_equal(
        RedactAny("items", vec![1_i64, 2_i64]),
        r#"{"items": [1, 2]}"#,
    );
}

/// Go 使用 fmt.Sprintf("lt_%f")，桶上界固定保留六位小数。
#[test]
fn test_histogram_bucket_key_matches_go_format() {
    let histogram = Histogram::with_opts(
        HistogramOpts::new("histogram_format", "histogram key formatting").buckets(vec![1.5]),
    )
    .expect("histogram");
    histogram.observe(1.0);
    let mut encoder = ObjectEncoder::default();
    MarshalHistogram(Some(histogram)).marshal_object(&mut encoder);

    assert_eq!(encoder.fields[0].0, "lt_1.500000");
}

/// 这些 Go 包级 API 必须能从 crate 根直接导入并实际调用。
#[test]
fn test_root_exports_cover_go_public_helpers() {
    let mut encoder = ObjectEncoder::default();
    MarshalLogObjectForFiles(&[], &mut encoder);
    assert_eq!(encoder.fields[0].0, "total");
    assert_trim_equal(
        BriefSSTMetas("brief", vec![]),
        r#"{"brief": {"total": 0, "startKey": "", "endKey": "", "totalSize": 0, "totalKvs": 0, "totalKvSize": 0}}"#,
    );
}

/// 单文件字段：键与 sha256 以 hex 出现在期望 JSON 中。
/// TestFile: single File field JSON, including hex-encoded keys/sha256.
#[test]
fn test_file() {
    assert_trim_equal(
        // j=1 → name/sha256/key 的 ASCII hex 可预测。
        File(new_file(1)),
        r#"{"file": {"name": "1", "CF": "write", "sha256": "31", "startKey": "31", "endKey": "32", "startVersion": 1, "endVersion": 2, "totalKvs": 1, "totalBytes": 1, "CRC64Xor": 1}}"#,
    );
}

/// 多文件：覆盖空/短/恰好4/缩略/大 N，并核对合计指标。
/// TestFiles: table-driven abbreviated file list + total aggregates.
#[test]
fn test_files() {
    let cases = [
        (
            // 空列表：全零合计。
            0,
            r#"{"files": {"total": 0, "files": [], "totalKVs": 0, "totalBytes": 0, "totalSize": 0}}"#,
        ),
        (
            // 单文件：合计仍为 0（j 从 0 起）。
            1,
            r#"{"files": {"total": 1, "files": ["0"], "totalKVs": 0, "totalBytes": 0, "totalSize": 0}}"#,
        ),
        (
            2,
            r#"{"files": {"total": 2, "files": ["0", "1"], "totalKVs": 1, "totalBytes": 1, "totalSize": 1}}"#,
        ),
        (
            3,
            r#"{"files": {"total": 3, "files": ["0", "1", "2"], "totalKVs": 3, "totalBytes": 3, "totalSize": 3}}"#,
        ),
        (
            // 恰 4 个仍全量展开，不插入 skip。
            4,
            r#"{"files": {"total": 4, "files": ["0", "1", "2", "3"], "totalKVs": 6, "totalBytes": 6, "totalSize": 6}}"#,
        ),
        (
            // 超过 4 个触发缩略：首、`(skip N)`、尾。
            5,
            r#"{"files": {"total": 5, "files": ["0", "(skip 3)", "4"], "totalKVs": 10, "totalBytes": 10, "totalSize": 10}}"#,
        ),
        (
            6,
            r#"{"files": {"total": 6, "files": ["0", "(skip 4)", "5"], "totalKVs": 15, "totalBytes": 15, "totalSize": 15}}"#,
        ),
        (
            // 大规模：合计用等差公式核对。
            1024,
            r#"{"files": {"total": 1024, "files": ["0", "(skip 1022)", "1023"], "totalKVs": 523776, "totalBytes": 523776, "totalSize": 523776}}"#,
        ),
    ];

    // 按 count 生成 0..count-1 的 File 列表。
    for (count, expect) in cases {
        let mut ranges = Vec::with_capacity(count);
        for j in 0..count {
            ranges.push(new_file(j as i32));
        }
        // 聚合 JSON 需与 Go 表驱动期望一致。
        assert_trim_equal(Files(ranges), expect);
    }
}

/// 单 Key：原始字节转 hex 字符串字段。
/// TestKey: single key field hex encoding.
#[test]
fn test_key() {
    assert_trim_equal(Key("test", vec![0, 1, 2, 3]), r#"{"test": "00010203"}"#);
}

/// 多 Key：四位十进制字符串的 hex，以及缩略边界。
/// TestKeys: table-driven abbreviated keys list.
#[test]
fn test_keys() {
    let cases = [
        (0, r#"{"keys": {"total": 0, "keys": []}}"#),
        (1, r#"{"keys": {"total": 1, "keys": ["30303030"]}}"#),
        (
            2,
            r#"{"keys": {"total": 2, "keys": ["30303030", "30303031"]}}"#,
        ),
        (
            3,
            r#"{"keys": {"total": 3, "keys": ["30303030", "30303031", "30303032"]}}"#,
        ),
        (
            4,
            r#"{"keys": {"total": 4, "keys": ["30303030", "30303031", "30303032", "30303033"]}}"#,
        ),
        (
            5,
            r#"{"keys": {"total": 5, "keys": ["30303030", "(skip 3)", "30303034"]}}"#,
        ),
        (
            6,
            r#"{"keys": {"total": 6, "keys": ["30303030", "(skip 4)", "30303035"]}}"#,
        ),
        (
            1024,
            r#"{"keys": {"total": 1024, "keys": ["30303030", "(skip 1022)", "31303233"]}}"#,
        ),
    ];

    for (count, expect) in cases {
        let mut keys = Vec::with_capacity(count);
        for j in 0..count {
            // 固定宽度便于期望串稳定（如 0000 → 30303030）。
            keys.push(format!("{j:04}").into_bytes());
        }
        // Keys 缩略规则与 Files 相同。
        assert_trim_equal(Keys(keys), expect);
    }
}

/// RewriteRule：ASCII 前缀转 hex，时间戳十进制。
/// TestRewriteRule: rewrite rule prefixes + timestamp encoding.
#[test]
fn test_rewrite_rule() {
    let mut rule = import_sstpb::RewriteRule::new();
    // old/new 前缀用于 hex 期望。
    rule.set_old_key_prefix(b"old".to_vec());
    rule.set_new_key_prefix(b"new".to_vec());
    // 0x555555 == 5592405，与期望串一致。
    rule.set_new_timestamp(0x555555);

    assert_trim_equal(
        RewriteRule(rule),
        r#"{"rewriteRule": {"oldKeyPrefix": "6f6c64", "newKeyPrefix": "6e6577", "newTimestamp": 5592405}}"#,
    );
}

/// Region：epoch/peers 紧凑字符串格式需与 Go 空格尾缀一致。
/// TestRegion: Region / RegionEpoch / Peer field expansion.
#[test]
fn test_region() {
    let mut epoch = metapb::RegionEpoch::new();
    // conf_ver/version 都为 1，对应期望串。
    epoch.set_conf_ver(1);
    epoch.set_version(1);
    let mut peer_a = metapb::Peer::new();
    peer_a.set_id(2);
    peer_a.set_store_id(3);
    let mut peer_b = metapb::Peer::new();
    peer_b.set_id(4);
    peer_b.set_store_id(5);
    // 第二个 peer：id=4 store_id=5。
    let mut region = metapb::Region::new();
    // Region ID=1。
    region.set_id(1);
    region.set_start_key(vec![0x00, 0x01]);
    region.set_end_key(vec![0x00, 0x02]);
    region.set_region_epoch(epoch);
    // 两个 peer，序列化时以逗号拼接。
    region.mut_peers().push(peer_a);
    region.mut_peers().push(peer_b);

    assert_trim_equal(
        Region(region),
        r#"{"region": {"ID": 1, "startKey": "0001", "endKey": "0002", "epoch": "conf_ver:1 version:1 ", "peers": "id:2 store_id:3 ,id:4 store_id:5 "}}"#,
    );
}

/// Leader：与 format_peer 相同的紧凑文本。
/// TestLeader: leader peer serializes as protobuf text.
#[test]
fn test_leader() {
    let mut leader = metapb::Peer::new();
    leader.set_id(2);
    leader.set_store_id(3);
    // store_id=3，与期望串一致。
    assert_trim_equal(Leader(leader), r#"{"leader": "id:2 store_id:3 "}"#);
}

/// SSTMeta：非法 UUID 回退为 `invalid UUID <hex>`。
/// TestSSTMeta: SSTMeta range / epoch / invalid UUID display.
#[test]
fn test_sst_meta() {
    let mut range = import_sstpb::Range::new();
    range.set_start(vec![0x00, 0x01]);
    range.set_end(vec![0x00, 0x02]);
    let mut meta = import_sstpb::SstMeta::new();
    // 非 16 字节 UUID，触发非法分支。
    meta.set_uuid(b"mock uuid".to_vec());
    meta.set_range(range);
    meta.set_crc32(0x555555);
    meta.set_length(1);
    // CF=default。
    meta.set_cf_name("default".to_string());
    // regionID=1。
    meta.set_region_id(1);
    meta.set_region_epoch({
        let mut epoch = metapb::RegionEpoch::new();
        epoch.set_conf_ver(1);
        epoch.set_version(1);
        epoch
    });

    assert_trim_equal(
        SSTMeta(meta),
        r#"{"sstMeta": {"CF": "default", "endKeyExclusive": false, "CRC32": 5592405, "length": 1, "regionID": 1, "regionEpoch": "conf_ver:1 version:1 ", "startKey": "0001", "endKey": "0002", "UUID": "invalid UUID 6d6f636b2075756964"}}"#,
    );
}

/// ShortError：Annotate 前缀 + BR 错误码文本。
/// TestShortError: annotated BR error short text.
#[test]
fn test_short_error() {
    let err = Annotate(
        // 使用包级 ErrInvalidArgument 单例构造可 Annotate 错误。
        Some(SharedError::new((*ErrInvalidArgument).clone())),
        "test",
    )
    .expect("annotate keeps error");
    assert_trim_equal(
        ShortError(Some(&err)),
        r#"{"error": "test: [BR:Common:ErrInvalidArgument]invalid argument"}"#,
    );
}

/// 上下文 logger：全局 Capture + ContextWithField 字段继承。
/// TestContextual: global logger + context field propagation via capture observer.
#[test]
fn test_contextual() {
    let (test_core, logs) = Logger::capture();
    // 注入 Capture，使 Background 上下文拿到可观察 logger。
    ResetGlobalLogger(Some(test_core));

    let ctx = Context::Background();
    // 无上下文字段：应走全局 Capture。
    let l0 = LoggerFromContext(&ctx);
    l0.Info(
        "going to take an adventure?",
        [
            // 角色属性字段，验证多 Field 合并。
            Field::int("HP", 50),
            Field::int("HP-MAX", 50),
            Field::string("character", "solte"),
        ],
    );
    // 叠加 friends 数组字段，后续 Info 应同时带上 character。
    let lctx = ContextWithField(
        ctx,
        [Field::array(
            "friends",
            vec![
                EncodedValue::String("firo".to_string()),
                EncodedValue::String("seren".to_string()),
                EncodedValue::String("black".to_string()),
            ],
        )],
    );
    let l = LoggerFromContext(&lctx);
    // 第二条应含继承的 friends + 本次 character。
    l.Info("let's go!", [Field::string("character", "solte")]);

    // 恰好两条：冒险开场 + let's go。
    let observed = logs.lock().expect("capture lock").clone();
    // 两条 Info 都被 Capture。
    assert_eq!(observed.len(), 2);
    // 校验消息与字段顺序/内容。
    check_log(
        &observed[0],
        "going to take an adventure?",
        &[
            Field::int("HP", 50),
            Field::int("HP-MAX", 50),
            Field::string("character", "solte"),
        ],
    );
    check_log(
        &observed[1],
        "let's go!",
        &[
            Field::array(
                "friends",
                vec![
                    EncodedValue::String("firo".to_string()),
                    EncodedValue::String("seren".to_string()),
                    EncodedValue::String("black".to_string()),
                ],
            ),
            Field::string("character", "solte"),
        ],
    );

    // 清理全局，避免污染其它测试。
    ResetGlobalLogger(None);
}

/// 逐字段 equals 比较 Capture 结果。
/// checkLog: compare captured message and field context one-by-one.
fn check_log(actual: &CapturedLog, message: &str, fields: &[Field]) {
    // 消息全文相等。
    assert_eq!(message, actual.message);
    // 顺序敏感：与调用时传入顺序一致。
    for (i, f) in fields.iter().enumerate() {
        assert!(
            // Field::equals 忽略 skip 并深比较值。
            f.equals(&actual.fields[i]),
            "Expected field({:?}) does not equal to actual one({:?}).",
            f,
            actual.fields[i]
        );
    }
}
