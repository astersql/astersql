// Copyright 2026 AsterSQL.

//!
//! `logutil` Go/Rust 公开契约对照测试。
//! 将缩略数组、Field JSON、上下文 logger、速率与脱敏等关键路径集中冒烟。
//! 断言字符串与 `logging_test` / Go 用例同源，防止迁移漂移。

use std::time::{Duration, Instant};

use astersql_br_pkg_errors::ErrInvalidArgument;
use astersql_errors::{Annotate, SharedError};
use prometheus::Counter;

use crate::kvproto::brpb;
use crate::kvproto::import_sstpb;
use crate::kvproto::metapb;
use crate::{
    AbbreviatedArrayMarshaler, ArrayMarshaler, CL, Context, ContextWithField, Field, File, Files,
    Key, Keys, Leader, Level, Logger, OverrideLevelForTest, Redact, Region, ResetGlobalLogger,
    RewriteRule, SSTMeta, ShortError, TraceRateOver, log,
};

// 与 logging_test 同源的 File fixture，保证两侧期望串一致。
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
    // CF 固定 write。
    file.set_cf("write".to_string());
    file.set_size(j as u64);
    file
}

// 相对/绝对误差判定，服务 RateAt 采样断言。
fn in_epsilon(expected: f64, actual: f64, epsilon: f64) -> bool {
    // 期望为 0：绝对误差。
    if expected == 0.0 {
        actual.abs() <= epsilon
    } else {
        ((actual - expected) / expected).abs() <= epsilon
    }
}

// Field JSON 与期望串全等比较。
fn assert_trim_equal(field: Field, expect: &str) {
    assert_eq!(expect, field.encode_json(), "encoded field mismatch");
}

#[test]
/// 汇总对照：公开 Field/API 与 Go 行为一致。
fn go_rust_public_contract_matches() {
    // 正常路径：缩略数组 marshal + 单 File JSON。
    // normal: abbreviated array marshaler and file field encoding
    // 5 元应缩成 3 项（首/skip/尾）。
    let abbreviated = AbbreviatedArrayMarshaler(vec![
        "0".to_string(),
        "1".to_string(),
        "2".to_string(),
        "3".to_string(),
        "4".to_string(),
    ]);
    let mut array_enc = crate::logging::ArrayEncoder::default();
    // 直接测 Marshaler，不经过 Field 包装。
    abbreviated.marshal_array(&mut array_enc);
    // 缩略后长度固定为 3。
    assert_eq!(array_enc.items.len(), 3);
    assert_trim_equal(
        // 单文件契约串。
        File(new_file(1)),
        r#"{"file": {"name": "1", "CF": "write", "sha256": "31", "startKey": "31", "endKey": "32", "startVersion": 1, "endVersion": 2, "totalKvs": 1, "totalBytes": 1, "CRC64Xor": 1}}"#,
    );

    // 边界：空 Files 与触发缩略的 5 文件/Keys。
    // boundary: empty and abbreviated collections
    assert_trim_equal(
        // 空聚合全零。
        Files(vec![]),
        r#"{"files": {"total": 0, "files": [], "totalKVs": 0, "totalBytes": 0, "totalSize": 0}}"#,
    );
    let mut many_files = Vec::new();
    // 构造 5 文件以触发 `(skip 3)`。
    for j in 0..5 {
        many_files.push(new_file(j));
    }
    assert_trim_equal(
        // 缩略文件名列表 + 合计。
        Files(many_files),
        r#"{"files": {"total": 5, "files": ["0", "(skip 3)", "4"], "totalKVs": 10, "totalBytes": 10, "totalSize": 10}}"#,
    );

    let mut keys = Vec::new();
    // 5 个四位 key。
    for j in 0..5 {
        keys.push(format!("{j:04}").into_bytes());
    }
    assert_trim_equal(
        // Keys 缩略契约。
        Keys(keys),
        r#"{"keys": {"total": 5, "keys": ["30303030", "(skip 3)", "30303034"]}}"#,
    );

    // 错误：短文本格式；None 应 skip。
    // error: short error formatting and nil skip
    let err = Annotate(
        // BR ErrInvalidArgument + Annotate 前缀。
        Some(SharedError::new((*ErrInvalidArgument).clone())),
        "test",
    )
    .expect("annotate keeps error");
    assert_trim_equal(
        ShortError(Some(&err)),
        r#"{"error": "test: [BR:Common:ErrInvalidArgument]invalid argument"}"#,
    );
    // nil 错误不产出字段。
    assert!(ShortError(None).is_skip());

    // 生命周期：上下文 logger 字段继承；结束时重置全局。
    // resource / lifecycle: contextual logger preserves prior contexts after global reset
    let (capture_logger, logs) = Logger::capture();
    // 注入 Capture 供 CL 回退。
    ResetGlobalLogger(Some(capture_logger));
    let ctx = Context::Background();
    // CL 等价 LoggerFromContext。
    let l0 = CL(&ctx);
    l0.Info(
        "going to take an adventure?",
        [
            // 首条日志多字段。
            Field::int("HP", 50),
            Field::int("HP-MAX", 50),
            Field::string("character", "solte"),
        ],
    );
    // 绑定 friends 数组到新上下文。
    let ctx_with_fields = ContextWithField(
        ctx,
        [Field::array(
            "friends",
            vec![
                crate::logging::EncodedValue::String("firo".to_string()),
                crate::logging::EncodedValue::String("seren".to_string()),
                crate::logging::EncodedValue::String("black".to_string()),
            ],
        )],
    );
    // 取带字段的上下文 logger。
    let l1 = CL(&ctx_with_fields);
    // 应同时看到继承字段与本次字段。
    l1.Info("let's go!", [Field::string("character", "solte")]);

    // 读取两条 Capture 记录做轻量断言。
    let captured = logs.lock().expect("capture lock").clone();
    // 两条 Info。
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].message, "going to take an adventure?");
    assert_eq!(captured[1].message, "let's go!");
    assert_eq!(captured[1].fields.len(), 2);
    // 继承字段排在 With 合并后的前列。
    assert_eq!(captured[1].fields[0].key, "friends");
    // 清理全局 logger。
    ResetGlobalLogger(None);

    // 速率：与 Go TestRater 相同的时间点采样。
    // rate tracer math aligned with Go TestRater
    let counter = Counter::with_opts(
        prometheus::Opts::new("rater", "A testing counter for the rater").namespace("testing"),
    )
    .expect("counter");
    // 基线 42，不计入后续速率。
    counter.inc_by(42.0);
    // RateAt 时间原点。
    let time_pass = Instant::now();
    let rater = crate::rate::RateTracer {
        start: time_pass,
        base: astersql_lightning_metric::read_counter(&counter),
        counter: Some(counter),
    };
    // 第一次 Inc → ~10 ops/s @100ms。
    rater.Inc();
    assert!(
        in_epsilon(
            10.0,
            rater.RateAt(time_pass + Duration::from_millis(100)),
            0.1
        ),
        // 1/0.1。
        "first rate sample"
    );
    rater.Inc();
    assert!(
        in_epsilon(
            13.0,
            rater.RateAt(time_pass + Duration::from_millis(150)),
            0.1
        ),
        // 2/0.15。
        "second rate sample"
    );
    rater.Add(18.0);
    assert!(
        in_epsilon(
            100.0,
            rater.RateAt(time_pass + Duration::from_millis(200)),
            0.1
        ),
        // 20/0.2。
        "third rate sample"
    );
    assert!(
        // 构造函数冒烟：counter 非空。
        TraceRateOver(
            Counter::with_opts(prometheus::Opts::new("rater_smoke", "smoke").namespace("testing"),)
                .expect("counter")
        )
        .counter
        .is_some()
    );

    // 脱敏与测试级别覆盖：守卫退出后级别还原。
    // redact and override-level helpers
    // 临时降到 Debug。
    let _guard = OverrideLevelForTest(Level::Debug);
    // 覆盖生效。
    assert_eq!(log::GetLevel(), Level::Debug);
    // 未开 NeedRedact 时内容保持（此处只查 key）。
    let redacted = Redact(Field::string("sql", "secret"));
    assert_eq!(redacted.key, "sql");
    // 显式 drop 触发 LevelGuard 还原。
    drop(_guard);
    // 恢复默认 Info。
    assert_eq!(log::GetLevel(), Level::Info);

    // restore 常用 protobuf 字段编码对照。
    // protobuf helpers used by restore logging
    let mut rule = import_sstpb::RewriteRule::new();
    rule.set_old_key_prefix(b"old".to_vec());
    rule.set_new_key_prefix(b"new".to_vec());
    // 时间戳十进制期望 5592405。
    rule.set_new_timestamp(0x555555);
    assert_trim_equal(
        // RewriteRule JSON 契约。
        RewriteRule(rule),
        r#"{"rewriteRule": {"oldKeyPrefix": "6f6c64", "newKeyPrefix": "6e6577", "newTimestamp": 5592405}}"#,
    );

    let mut epoch = metapb::RegionEpoch::new();
    epoch.set_conf_ver(1);
    epoch.set_version(1);
    let mut peer_a = metapb::Peer::new();
    peer_a.set_id(2);
    peer_a.set_store_id(3);
    let mut peer_b = metapb::Peer::new();
    peer_b.set_id(4);
    peer_b.set_store_id(5);
    let mut region = metapb::Region::new();
    // 组装双 peer Region。
    region.set_id(1);
    region.set_start_key(vec![0x00, 0x01]);
    region.set_end_key(vec![0x00, 0x02]);
    region.set_region_epoch(epoch);
    region.mut_peers().push(peer_a);
    region.mut_peers().push(peer_b);
    assert_trim_equal(
        // Region 紧凑 epoch/peers 串。
        Region(region),
        r#"{"region": {"ID": 1, "startKey": "0001", "endKey": "0002", "epoch": "conf_ver:1 version:1 ", "peers": "id:2 store_id:3 ,id:4 store_id:5 "}}"#,
    );
    assert_trim_equal(
        // Leader 紧凑 peer 串。
        Leader({
            let mut leader = metapb::Peer::new();
            leader.set_id(2);
            leader.set_store_id(3);
            leader
        }),
        r#"{"leader": "id:2 store_id:3 "}"#,
    );

    let mut range = import_sstpb::Range::new();
    range.set_start(vec![0x00, 0x01]);
    range.set_end(vec![0x00, 0x02]);
    let mut meta = import_sstpb::SstMeta::new();
    // 非法 UUID 显示分支。
    meta.set_uuid(b"mock uuid".to_vec());
    meta.set_range(range);
    meta.set_crc32(0x555555);
    meta.set_length(1);
    meta.set_cf_name("default".to_string());
    meta.set_region_id(1);
    meta.set_region_epoch({
        let mut epoch = metapb::RegionEpoch::new();
        epoch.set_conf_ver(1);
        epoch.set_version(1);
        epoch
    });
    assert_trim_equal(
        // SSTMeta 全字段契约。
        SSTMeta(meta),
        r#"{"sstMeta": {"CF": "default", "endKeyExclusive": false, "CRC32": 5592405, "length": 1, "regionID": 1, "regionEpoch": "conf_ver:1 version:1 ", "startKey": "0001", "endKey": "0002", "UUID": "invalid UUID 6d6f636b2075756964"}}"#,
    );

    // 单 Key hex 契约收尾。
    assert_trim_equal(Key("test", vec![0, 1, 2, 3]), r#"{"test": "00010203"}"#);
}
