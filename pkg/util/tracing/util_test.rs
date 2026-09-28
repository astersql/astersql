// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// tracing util 单元测试：Span 树、Context 派生与 TraceInfo。
//
// 对应 Go `util_test.go`。用内存回调收集 RawSpan，校验父子关系、
// noop 行为及 TraceInfo 挂载。

use crate::util::{
    ChildSpanFromContxt, Context, ContextWithTraceInfo, NewRecordedTrace, RawSpan, SpanFromContext,
    StartRegion, StartRegionWithNewRootSpan, TraceInfo, TraceInfoFromContext,
};
use std::sync::{Arc, Mutex};

/// 构造收集 RawSpan 的回调与共享向量。
fn collector() -> (
    Arc<Mutex<Vec<RawSpan>>>,
    impl Fn(RawSpan) + Send + Sync + 'static,
) {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let callback_spans = Arc::clone(&spans);
    (spans, move |span| callback_spans.lock().unwrap().push(span))
}

/// 空 Context 得 noop；挂上根 span 后能取回并录制。
#[test]
fn test_span_from_context() {
    let ctx = Context::background();
    assert!(SpanFromContext(&ctx).is_noop());

    let (spans, callback) = collector();
    let root = NewRecordedTrace("test", callback);
    let ctx = ctx.with_span(root.clone());
    let from_context = SpanFromContext(&ctx);
    assert_eq!(from_context.span_id(), root.span_id());
    from_context.finish();

    let spans = spans.lock().unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].operation, "test");
}

/// 无父时 Child 为 noop；有真实父时建子 span 并更新 Context。
#[test]
fn test_child_span_from_context() {
    let ctx = Context::background();
    let (noop, unchanged) = ChildSpanFromContxt(ctx.clone(), "");
    assert!(noop.is_noop());
    assert!(unchanged.same_instance(&ctx));

    let (spans, callback) = collector();
    let root = NewRecordedTrace("test", callback);
    let root_ctx = ctx.with_span(root.clone());
    let (child, child_ctx) = ChildSpanFromContxt(root_ctx, "test_child");
    assert_eq!(SpanFromContext(&child_ctx).span_id(), child.span_id());
    root.finish();
    child.finish();

    let spans = spans.lock().unwrap();
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[1].operation, "test_child");
    assert_eq!(spans[1].parent_span_id, spans[0].span_id);
}

/// 子 span 的 parent_span_id 指向根；根的 parent 为 0。
#[test]
fn test_follow_from() {
    let (spans, callback) = collector();
    let root = NewRecordedTrace("test", callback);
    let (follower, _) =
        ChildSpanFromContxt(Context::background().with_span(root.clone()), "follow_from");
    root.finish();
    follower.finish();

    let spans = spans.lock().unwrap();
    assert_eq!(spans[1].operation, "follow_from");
    assert_ne!(spans[1].parent_span_id, 0);
    assert_eq!(spans[0].parent_span_id, 0);
}

/// 全局 tracer 安装前创建的 span finish 不录制；之后的根 span 正常录制。
#[test]
fn test_create_sapn_before_setup_global_tracer() {
    let before = SpanFromContext(&Context::background());
    before.finish();

    let (spans, callback) = collector();
    let root = NewRecordedTrace("test", callback);
    root.finish();

    let spans = spans.lock().unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].operation, "test");
}

/// 三层树：test → parent → child，parent_span_id 链正确。
#[test]
fn test_tree_relationship() {
    let (spans, callback) = collector();
    let root = NewRecordedTrace("test", callback);
    let root_ctx = Context::background().with_span(root.clone());
    let (parent, parent_ctx) = ChildSpanFromContxt(root_ctx, "parent");
    let (child, _) = ChildSpanFromContxt(parent_ctx, "child");

    root.finish();
    parent.finish();
    child.finish();

    let spans = spans.lock().unwrap();
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].operation, "test");
    assert_eq!(spans[1].operation, "parent");
    assert_eq!(spans[2].operation, "child");
    assert_eq!(spans[1].parent_span_id, spans[0].span_id);
    assert_eq!(spans[2].parent_span_id, spans[1].span_id);
}

/// TraceInfo 缺省为 None；传入 None 不派生；挂载后可读 ConnectionID/SessionAlias。
#[test]
fn test_trace_info_from_context() {
    let ctx = Context::background();
    assert!(TraceInfoFromContext(&ctx).is_none());

    let unchanged = ContextWithTraceInfo(ctx.clone(), None);
    assert!(unchanged.same_instance(&ctx));

    let traced = ContextWithTraceInfo(
        ctx,
        Some(TraceInfo {
            ConnectionID: 12345,
            SessionAlias: "alias1".to_owned(),
            TraceID: Vec::new(),
        }),
    );
    let info = TraceInfoFromContext(&traced).unwrap();
    assert_eq!(info.ConnectionID, 12345);
    assert_eq!(info.SessionAlias, "alias1");
}

/// Go 直接把新根 span 放进 Region 与返回 Context，不额外创建同名子 span。
#[test]
fn test_start_region_with_new_root_span_reuses_root() {
    let (spans, callback) = collector();
    let _ = NewRecordedTrace("setup", callback);

    let (mut region, ctx) = StartRegionWithNewRootSpan(Context::background(), "new_root_region");
    let region_span = region
        .Span
        .as_ref()
        .expect("region must contain the root span");
    assert_eq!(SpanFromContext(&ctx).span_id(), region_span.span_id());
    assert_eq!(region_span.parent_span_id(), 0);

    region.end();
    let spans = spans.lock().unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].operation, "new_root_region");
    assert_eq!(spans[0].parent_span_id, 0);
}

/// Go StartRegion 对 context 中的 noop span 仍返回非空 noop Span。
#[test]
fn test_start_region_preserves_noop_span_shape() {
    let noop = SpanFromContext(&Context::background());
    let mut region = StartRegion(Context::background().with_span(noop), "noop_region");
    assert!(region.Span.as_ref().is_some_and(|span| span.is_noop()));
    region.end();
}
