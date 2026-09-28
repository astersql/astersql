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

// metrics 包迁移对照单测（Aster 补充）。
//
// 校验 CmdToString 标签、稀疏查询计数器索引/标签，以及断连、空闲与包 IO 指标行为。

#![cfg(test)]

use crate::*;
use prometheus::core::Collector;

fn has_label(collector: &dyn Collector, name: &str, value: &str) -> bool {
    collector.collect().iter().any(|family| {
        family.get_metric().iter().any(|metric| {
            metric
                .get_label()
                .iter()
                .any(|label| label.get_name() == name && label.get_value() == value)
        })
    })
}

/// 已知命令映射为 Go 同名标签；未知命令回退为数字字符串。
#[test]
fn cmd_to_string_matches_go_for_named_and_numeric_commands() {
    let named = [
        (COM_SLEEP, "Sleep"),
        (COM_QUIT, "Quit"),
        (COM_INIT_DB, "InitDB"),
        (COM_QUERY, "Query"),
        (COM_PING, "Ping"),
        (COM_FIELD_LIST, "FieldList"),
        (COM_STMT_PREPARE, "StmtPrepare"),
        (COM_STMT_EXECUTE, "StmtExecute"),
        (COM_STMT_FETCH, "StmtFetch"),
        (COM_STMT_CLOSE, "StmtClose"),
        (COM_STMT_SEND_LONG_DATA, "StmtSendLongData"),
        (COM_STMT_RESET, "StmtReset"),
        (COM_SET_OPTION, "SetOption"),
    ];

    for (command, expected) in named {
        assert_eq!(CmdToString(command), expected);
    }
    assert_eq!(CmdToString(5), "5");
    assert_eq!(CmdToString(u8::MAX), "255");
}

/// 初始化后向量长度与稀疏槽位（如 COM_CREATE_DB）对齐 Go，并校验 Query 标签组合。
#[test]
fn initialized_query_counters_preserve_go_sparse_indices_and_labels() {
    InitMetricsVars();

    assert_eq!(QueryTotalCountOk.len(), COM_STMT_FETCH as usize + 1);
    assert_eq!(QueryTotalCountErr.len(), COM_STMT_FETCH as usize + 1);
    assert!(QueryTotalCountOk[COM_CREATE_DB as usize].is_none());
    assert!(QueryTotalCountErr[COM_CREATE_DB as usize].is_none());

    QueryTotalCountOk[COM_QUERY as usize]
        .as_ref()
        .unwrap()
        .inc();
    QueryTotalCountErr[COM_QUERY as usize]
        .as_ref()
        .unwrap()
        .inc();

    // 从 Collector 取出标签，确认 resource_group/result/type 与 Go 一致。
    let families = QUERY_TOTAL_COUNTER.collect();
    let labels: Vec<Vec<(String, String)>> = families[0]
        .get_metric()
        .iter()
        .map(|metric| {
            metric
                .get_label()
                .iter()
                .map(|label| (label.get_name().to_owned(), label.get_value().to_owned()))
                .collect()
        })
        .collect();
    assert!(labels.iter().any(|labels| labels
        == &[
            ("resource_group".into(), DEFAULT_RESOURCE_GROUP_NAME.into()),
            ("result".into(), "OK".into()),
            ("type".into(), "Query".into()),
        ]));
    assert!(labels.iter().any(|labels| labels
        == &[
            ("resource_group".into(), DEFAULT_RESOURCE_GROUP_NAME.into()),
            ("result".into(), "Error".into()),
            ("type".into(), "Query".into()),
        ]));
}

/// 校验断连、事务内外空闲直方图与入出站包字节计数器可观察。
#[test]
fn initialized_disconnect_idle_and_packet_metrics_match_go_labels() {
    init();

    assert!(has_label(&*DisconnectNormal, "result", "ok"));
    assert!(has_label(&*DisconnectByClientWithError, "result", "error"));
    assert!(has_label(
        &*DisconnectErrorUndetermined,
        "result",
        "undetermined"
    ));
    assert!(has_label(
        &*ConnIdleDurationHistogramNotInTxn,
        "in_txn",
        "0"
    ));
    assert!(has_label(&*ConnIdleDurationHistogramInTxn, "in_txn", "1"));
    assert!(has_label(&*InPacketBytes, "type", "In"));
    assert!(has_label(&*OutPacketBytes, "type", "Out"));

    DisconnectNormal.inc();
    DisconnectByClientWithError.inc();
    DisconnectErrorUndetermined.inc();
    ConnIdleDurationHistogramNotInTxn.observe(0.25);
    ConnIdleDurationHistogramInTxn.observe(0.5);
    InPacketBytes.inc_by(11.0);
    OutPacketBytes.inc_by(13.0);

    assert_eq!(DisconnectNormal.get(), 1.0);
    assert_eq!(DisconnectByClientWithError.get(), 1.0);
    assert_eq!(DisconnectErrorUndetermined.get(), 1.0);
    assert_eq!(ConnIdleDurationHistogramNotInTxn.get_sample_count(), 1);
    assert_eq!(ConnIdleDurationHistogramInTxn.get_sample_count(), 1);
    assert_eq!(InPacketBytes.get(), 11.0);
    assert_eq!(OutPacketBytes.get(), 13.0);
}
