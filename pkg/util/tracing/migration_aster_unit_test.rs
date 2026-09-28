// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// tracing 迁移单元测试。
//
// 验证 CE 去重与 JSON 字段形态、TraceCategory 原子开关、Span 父子链、
// TraceInfo 透传，以及 Region begin/end 事件，对齐 Go 迁移基线。

use crate::CATEGORY_TEST_LOCK;
use crate::opt_trace::{CETraceRecord, DedupCETrace};
use crate::util::*;
use std::sync::{Arc, Mutex};

/// CE 去重保留首次记录，且序列化键名与 Go（snake_case）一致、不暴露 TableID。
#[test]
fn ce_trace_dedup_preserves_first_value_and_go_json_shape() {
    let first = CETraceRecord {
        TableName: "t".into(),
        Type: "table".into(),
        Expr: "a > 1".into(),
        TableID: 42,
        RowCount: 7,
    };
    let duplicate = first.clone();
    let distinct = CETraceRecord {
        RowCount: 8,
        ..first.clone()
    };
    let distinct_table_id = CETraceRecord {
        TableID: 43,
        ..first.clone()
    };

    let records = DedupCETrace(vec![
        Box::new(first),
        Box::new(duplicate),
        Box::new(distinct),
        Box::new(distinct_table_id),
    ]);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].RowCount, 7);
    assert_eq!(records[1].RowCount, 8);
    assert_eq!(records[2].TableID, 43);

    let json = serde_json::to_value(&records[0]).unwrap();
    assert_eq!(json["table_name"], "t");
    assert_eq!(json["type"], "table");
    assert_eq!(json["expr"], "a > 1");
    assert_eq!(json["row_count"], 7);
    assert!(json.get("TableID").is_none());
    assert!(json.get("table_id").is_none());
}

/// 类别名称解析与 Enable/Disable/SetCategories 位图操作与 Go 一致。
#[test]
fn categories_match_go_names_and_atomic_mask_operations() {
    let _guard = CATEGORY_TEST_LOCK.lock().unwrap();
    SetCategories(TraceCategory::NONE);
    for category in TraceCategory::KNOWN {
        assert_eq!(ParseTraceCategory(category.as_str()), category);
    }
    assert_eq!(ParseTraceCategory("missing"), TraceCategory::NONE);
    assert_eq!(TraceCategory(3).to_string(), "unknown(3)");

    Enable(TxnLifecycle | General);
    assert!(IsEnabled(TxnLifecycle));
    assert!(IsEnabled(General));
    Disable(TxnLifecycle);
    assert!(!IsEnabled(TxnLifecycle));
    assert_eq!(GetEnabledCategories(), General);
    SetCategories(AllCategories);
    assert_eq!(GetEnabledCategories(), AllCategories);
    SetCategories(TraceCategory::NONE);
}

/// 记录型 Span 保留 baggage（TiDBTrace）并维护正确的 parent_span_id 链。
#[test]
fn recorded_spans_preserve_baggage_and_parent_chain() {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let output = recorded.clone();
    let root = NewRecordedTrace("root", move |span| output.lock().unwrap().push(span));
    assert_eq!(root.baggage_item(TiDBTrace).as_deref(), Some("1"));

    let ctx = Context::background().with_span(root.clone());
    let (parent, ctx) = ChildSpanFromContxt(ctx, "parent");
    let (child, _) = ChildSpanFromContxt(ctx, "child");
    root.finish();
    parent.finish();
    child.finish();

    let spans = recorded.lock().unwrap();
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].operation, "root");
    assert_eq!(spans[1].operation, "parent");
    assert_eq!(spans[2].operation, "child");
    assert_eq!(spans[0].parent_span_id, 0);
    assert_eq!(spans[1].parent_span_id, spans[0].span_id);
    assert_eq!(spans[2].parent_span_id, spans[1].span_id);
}

/// 无 Span 的 Context 上取子 Span 应得到 noop，且返回同一 Context 实例。
#[test]
fn missing_context_span_returns_a_noop_span() {
    let ctx = Context::background();
    assert!(SpanFromContext(&ctx).is_noop());
    let (child, returned) = ChildSpanFromContxt(ctx.clone(), "unused");
    assert!(child.is_noop());
    assert!(returned.same_instance(&ctx));
}

/// TraceInfo 挂载后可回读；ExtractTraceID 仍取 Context 上原有的 trace_id 字节。
#[test]
fn trace_info_and_trace_id_are_carried_without_losing_context() {
    let ctx = Context::background().with_trace_id(vec![1, 2, 3]);
    assert!(TraceInfoFromContext(&ctx).is_none());
    assert!(ContextWithTraceInfo(ctx.clone(), None).same_instance(&ctx));

    let ctx = ContextWithTraceInfo(
        ctx,
        Some(TraceInfo {
            SessionAlias: "alias1".into(),
            TraceID: vec![9],
            ConnectionID: 12345,
        }),
    );
    let info = TraceInfoFromContext(&ctx).unwrap();
    assert_eq!(info.ConnectionID, 12345);
    assert_eq!(info.SessionAlias, "alias1");
    assert_eq!(info.TraceID, vec![9]);
    assert_eq!(ExtractTraceID(&ctx), vec![1, 2, 3]);
}

/// 测试用 Sink：把收到的 Event 收集到互斥向量，兼作 FlightRecorder。
#[derive(Default)]
struct RecordingSink(Mutex<Vec<Event>>);

impl Sink for RecordingSink {
    fn record(&self, _ctx: &Context, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

impl FlightRecorder for RecordingSink {}

/// StartRegion 在启用 General 类别时应写出配对的 begin/end，并带上 Context 的 TraceID。
#[test]
fn region_records_matching_begin_and_end_events() {
    let _guard = CATEGORY_TEST_LOCK.lock().unwrap();
    SetCategories(General);
    let sink = Arc::new(RecordingSink::default());
    let ctx = WithFlightRecorder(
        Context::background().with_trace_id(vec![4, 2]),
        sink.clone(),
    );
    let mut region = StartRegion(ctx, "compile");
    region.end();

    let events = sink.0.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].Name, "compile");
    assert_eq!(events[0].Phase, PhaseBegin);
    assert_eq!(events[0].Category, General);
    assert_eq!(events[1].Phase, PhaseEnd);
    assert_eq!(events[0].TraceID, vec![4, 2]);
    assert_eq!(events[1].Name, "compile");
    assert_eq!(events[1].Category, General);
    assert_eq!(events[1].TraceID, vec![4, 2]);
    SetCategories(TraceCategory::NONE);
}
