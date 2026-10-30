// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `util` 模块的基础单元测试。
//
// 验证 `Filter`/`FilterOutInPlace` 的顺序契约，以及数值前缀、二进制时间、
// 容量格式化、时区与校对定位等与 Go 对齐的边界行为。

use crate::util_kernel::*;
use crate::*;

/// 判断表达式是否为值为 NULL 的常量节点。
fn is_null_constant(expression: &dyn Expression) -> bool {
    expression
        .as_any()
        .downcast_ref::<Constant>()
        .is_some_and(|constant| constant.Value.IsNull())
}

/// 校验 Filter 保序、FilterOutInPlace 逆序移除并拆分 kept/removed。
#[test]
fn filter_and_filter_out_preserve_go_ordering_contract() {
    let input: Vec<ExprBox> = vec![
        Box::new(NewOne()),
        Box::new(NewNull()),
        Box::new(NewSignedZero()),
        Box::new(NewNull()),
    ];
    let filtered = Filter(Vec::new(), &input, is_null_constant);
    assert_eq!(filtered.len(), 2);
    assert!(
        filtered
            .iter()
            .all(|expression| is_null_constant(expression.as_ref()))
    );

    let (kept, removed) = FilterOutInPlace(input, is_null_constant);
    assert_eq!(kept.len(), 2);
    assert_eq!(removed.len(), 2);
    assert!(
        kept.iter()
            .all(|expression| !is_null_constant(expression.as_ref()))
    );
    assert!(
        removed
            .iter()
            .all(|expression| is_null_constant(expression.as_ref()))
    );
}

/// 校验进制前缀截取：符号、非法进制与仅符号输入。
#[test]
fn valid_numeric_prefix_handles_sign_radix_and_invalid_radix_boundaries() {
    assert_eq!(getValidPrefix("+1fZ", 16), "1f");
    assert_eq!(getValidPrefix("-1012", 2), "-101");
    assert_eq!(getValidPrefix("z!", 36), "z");
    assert_eq!(getValidPrefix("-", 10), "");
    assert_eq!(getValidPrefix("123", 1), "");
    assert_eq!(getValidPrefix("123", 37), "");
}

/// 校验日期/带时区时间戳/负时长的二进制解码偏移与文本。
#[test]
fn binary_temporal_helpers_preserve_offsets_fraction_and_negative_duration() {
    let date = [0xe8, 0x07, 0x0c, 0x1f];
    assert_eq!(binaryDate(0, &date), (4, "2024-12-31".to_owned()));

    let timestamp_with_tz = [
        0xe8, 0x07, 0x0c, 0x1f, 0x17, 0x3b, 0x3a, 0x40, 0xe2, 0x01, 0x00, 0x3e, 0xfe,
    ];
    assert_eq!(
        binaryTimestampWithTZ(0, &timestamp_with_tz),
        (13, "2024-12-31 23:59:58.123456-7:30".to_owned())
    );

    let duration = [1, 2, 0, 0, 0, 3, 4, 5, 0x40, 0xe2, 0x01, 0x00];
    assert_eq!(
        binaryDurationWithMS(1, &duration, duration[0]),
        (12, "-2 03:04:05.123456".to_owned())
    );
}

/// 校验格式化阈值、时区秒偏移与空 needle 定位。
#[test]
fn format_timezone_and_collation_helpers_cover_thresholds_and_empty_needles() {
    assert_eq!(GetFormatBytes(0.0), "0 bytes");
    assert_eq!(GetFormatBytes(1024.0), "1.00 KiB");
    assert_eq!(GetFormatNanoTime(1_000.0), "1.00 us");
    assert_eq!(GetFormatNanoTime(60_000_000_000.0), "1.00 min");

    assert_eq!(timeZone2int("+13:00"), 46_800);
    assert_eq!(timeZone2int("-05:30"), -19_800);
    assert_eq!(locateStringWithCollation("abc", "", "utf8mb4_bin"), 1);
}

/// Go strconv.FormatFloat preserves a signed, two-digit exponent and spells infinities as Inf.
#[test]
fn format_helpers_match_go_scientific_notation_and_special_values() {
    assert_eq!(GetFormatBytes(100_000.0 * eib), "1.00e+05 EiB");
    assert_eq!(GetFormatNanoTime(-100_000.0 * dayTime), "-1.00e+05 d");
    assert_eq!(GetFormatBytes(f64::INFINITY), "+Inf EiB");
    assert_eq!(GetFormatNanoTime(f64::NEG_INFINITY), "-Inf d");
}

// Restricted SQL is the external boundary; exercise the real retriever and SQL builder.
#[derive(Default)]
struct PortDigestExecutor {
    calls: std::cell::RefCell<Vec<(bool, bool, Vec<String>)>>,
    data: [std::collections::HashMap<String, String>; 4],
    fail: Option<usize>,
    ignore_filter: bool,
}
impl expropt::SQLExecutor for PortDigestExecutor {
    type Context = kv::Context;
    type OptionFuncAlias = ();
    type Row = chunk::Row;
    type ResultField = ();
    fn exec_restricted_sql(
        &self,
        ctx: &kv::Context,
        _: &[()],
        sql: &str,
        args: &[Box<dyn std::any::Any>],
    ) -> anyhow::Result<(Vec<chunk::Row>, Vec<()>)> {
        assert_eq!(
            ctx.RequestSource().unwrap().RequestSourceType,
            kv::InternalTxnOthers
        );
        let global = sql.contains("cluster_statements_summary");
        let history = sql.contains("_history");
        let values = args
            .iter()
            .map(|v| v.downcast_ref::<String>().unwrap().clone())
            .collect::<Vec<_>>();
        self.calls
            .borrow_mut()
            .push((global, history, values.clone()));
        assert!(
            sql.starts_with("select distinct digest, digest_text from "),
            "{sql}"
        );
        assert!(!sql.contains("union"), "{sql}");
        assert_eq!(sql.matches("%?").count(), values.len());
        let index = usize::from(global) * 2 + usize::from(history);
        if self.fail == Some(index) {
            anyhow::bail!("summary failure {index}");
        }
        let mut rows = Vec::new();
        for (digest, text) in &self.data[index] {
            if !self.ignore_filter && !values.is_empty() && !values.contains(digest) {
                continue;
            }
            let mut row = chunk::NewChunkWithCapacity(
                vec![
                    *types::NewFieldType(mysql::TypeVarchar),
                    *types::NewFieldType(mysql::TypeVarchar),
                ],
                1,
            );
            row.AppendString(0, digest);
            row.AppendString(1, text);
            rows.push(row.GetRow(0).CopyConstruct());
        }
        Ok((rows, Vec::new()))
    }
}
fn port_digest_retriever(keys: &[&str]) -> SQLDigestTextRetriever {
    let mut r = NewSQLDigestTextRetriever();
    r.SQLDigestsMap = keys
        .iter()
        .map(|k| (k.to_string(), String::new()))
        .collect();
    r
}
fn port_digest_data(e: &mut PortDigestExecutor, index: usize, entries: &[(&str, &str)]) {
    e.data[index] = entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
}
#[test]
fn port_3f2de015d9_current_hit_skips_history() {
    let mut e = PortDigestExecutor::default();
    port_digest_data(&mut e, 0, &[("a", "current")]);
    let mut r = port_digest_retriever(&["a"]);
    r.RetrieveLocal(kv::Context::default(), &e).unwrap();
    assert_eq!(r.SQLDigestsMap["a"], "current");
    assert_eq!(e.calls.borrow().len(), 1);
}
#[test]
fn port_3f2de015d9_cluster_current_before_local_history() {
    let mut e = PortDigestExecutor::default();
    port_digest_data(&mut e, 0, &[("a", "local")]);
    port_digest_data(&mut e, 2, &[("c", "cluster")]);
    port_digest_data(&mut e, 1, &[("b", "history")]);
    let mut r = port_digest_retriever(&["c", "b", "a"]);
    r.RetrieveGlobal(kv::Context::default(), &e).unwrap();
    assert_eq!(
        *e.calls.borrow(),
        vec![
            (false, false, vec!["a".into(), "b".into(), "c".into()]),
            (true, false, vec!["b".into(), "c".into()]),
            (false, true, vec!["b".into()])
        ]
    );
    assert_eq!(r.SQLDigestsMap["c"], "cluster");
    assert_eq!(r.SQLDigestsMap["b"], "history");
}
#[test]
fn port_3f2de015d9_unrequested_and_known_are_preserved() {
    let mut e = PortDigestExecutor {
        ignore_filter: true,
        ..Default::default()
    };
    port_digest_data(&mut e, 0, &[("other", "sql")]);
    let mut r = port_digest_retriever(&["wanted"]);
    r.RetrieveLocal(kv::Context::default(), &e).unwrap();
    assert_eq!(r.SQLDigestsMap.len(), 1);
    assert_eq!(r.SQLDigestsMap["wanted"], "");
    r.SQLDigestsMap.insert("wanted".into(), "known".into());
    e.calls.borrow_mut().clear();
    r.RetrieveGlobal(kv::Context::default(), &e).unwrap();
    assert!(e.calls.borrow().is_empty());
    assert_eq!(r.SQLDigestsMap["wanted"], "known");
}
#[test]
fn port_3f2de015d9_large_history_scanned_once() {
    let e = PortDigestExecutor::default();
    let mut r = NewSQLDigestTextRetriever();
    r.SQLDigestsMap = (0..513).map(|i| (format!("d{i}"), String::new())).collect();
    r.RetrieveLocal(kv::Context::default(), &e).unwrap();
    assert_eq!(
        *e.calls.borrow(),
        vec![(false, false, vec![]), (false, true, vec![])]
    );
}
#[test]
fn port_3f2de015d9_partial_local_global_history_and_fetch_all() {
    for count in [5, 513] {
        let mut e = PortDigestExecutor::default();
        port_digest_data(&mut e, 0, &[("d1", "t1"), ("d2", "t2"), ("extra", "x")]);
        port_digest_data(&mut e, 2, &[("d3", "t3"), ("d4", "t4")]);
        port_digest_data(&mut e, 3, &[("d5", "t5")]);
        let mut r = NewSQLDigestTextRetriever();
        r.SQLDigestsMap = (1..=count)
            .map(|i| (format!("d{i}"), String::new()))
            .collect();
        r.RetrieveGlobal(kv::Context::default(), &e).unwrap();
        for i in 1..=5 {
            assert_eq!(r.SQLDigestsMap[&format!("d{i}")], format!("t{i}"));
        }
        assert_eq!(r.SQLDigestsMap.len(), count);
        assert_eq!(e.calls.borrow().len(), 4);
    }
}
#[test]
fn port_3f2de015d9_global_error_local_history_recovery() {
    for recovered in [false, true] {
        let mut e = PortDigestExecutor {
            fail: Some(2),
            ..Default::default()
        };
        if recovered {
            port_digest_data(&mut e, 1, &[("a", "recovered")]);
        }
        let mut r = port_digest_retriever(&["a"]);
        let result = r.RetrieveGlobal(kv::Context::default(), &e);
        assert_eq!(result.is_ok(), recovered);
        assert_eq!(e.calls.borrow().len(), 3);
        if !recovered {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("summary failure 2")
            );
        }
    }
}
#[test]
fn port_3f2de015d9_empty_text_and_error_precedence() {
    let mut e = PortDigestExecutor {
        fail: Some(1),
        ..Default::default()
    };
    port_digest_data(&mut e, 0, &[("a", "")]);
    let mut r = port_digest_retriever(&["a"]);
    assert!(
        r.RetrieveGlobal(kv::Context::default(), &e)
            .unwrap_err()
            .to_string()
            .contains("summary failure 1")
    );
    assert_eq!(e.calls.borrow().len(), 3);
    let e = PortDigestExecutor {
        fail: Some(0),
        ..Default::default()
    };
    let mut r = port_digest_retriever(&["a"]);
    assert!(r.RetrieveLocal(kv::Context::default(), &e).is_err());
    assert_eq!(e.calls.borrow().len(), 1);
    let mut r = NewSQLDigestTextRetriever();
    r.RetrieveGlobal(kv::Context::default(), &e).unwrap();
    assert_eq!(e.calls.borrow().len(), 1);
}

#[test]
fn port_3f2de015d9_threshold_uses_only_unresolved() {
    let mut e = PortDigestExecutor::default();
    port_digest_data(&mut e, 0, &[("d0", "current"), ("d1", "current")]);
    let mut r = NewSQLDigestTextRetriever();
    r.SQLDigestsMap = (0..513).map(|i| (format!("d{i}"), String::new())).collect();
    r.RetrieveLocal(kv::Context::default(), &e).unwrap();
    let calls = e.calls.borrow();
    assert!(calls[0].2.is_empty());
    assert_eq!(calls[1].2.len(), 511);
    assert!(!calls[1].2.contains(&"d0".into()));
    assert!(calls[1].2.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn port_3f2de015d9_skip_global_keeps_local_history() {
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            fail::remove("sqlDigestRetrieverSkipRetrieveGlobal");
        }
    }
    fail::cfg("sqlDigestRetrieverSkipRetrieveGlobal", "return").unwrap();
    let _cleanup = Cleanup;
    let mut e = PortDigestExecutor::default();
    port_digest_data(&mut e, 1, &[("a", "history")]);
    let mut r = port_digest_retriever(&["a"]);
    r.RetrieveGlobal(kv::Context::default(), &e).unwrap();
    assert_eq!(r.SQLDigestsMap["a"], "history");
    assert_eq!(
        *e.calls.borrow(),
        vec![
            (false, false, vec!["a".into()]),
            (false, true, vec!["a".into()])
        ]
    );
}
