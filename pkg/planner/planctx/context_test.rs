// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// BuildPBContext::Detach 行为测试，对应 Go `TestContextDetach`。
//
// 断言浅拷贝后标量字段相等，而 ExprCtx / Client / WarnHandler 等路径
// 保持接口/指针身份（与 Go AssertDeepClonedEqual 的忽略列表一致）。

use std::sync::Arc;

use contextutil_crate::NewStaticWarnHandler;
use exprstatic_crate::NewExprContext;
use planctx_crate::{BuildContextRef, BuildPBContext, WarnAppenderRef};

// test_context_detach corresponds to Go's TestContextDetach.
/// 验证 Detach 浅拷贝语义：非忽略字段相等，忽略路径保持指针身份。
#[test]
fn test_context_detach() {
    let expr_ctx: BuildContextRef = Arc::new(NewExprContext(Vec::new()));
    let warn_handler: WarnAppenderRef = Arc::new(NewStaticWarnHandler(5));
    let obj = BuildPBContext {
        ExprCtx: Arc::clone(&expr_ctx),
        Client: None,
        TiFlashFastScan: true,
        TiFlashFineGrainedShuffleBatchSize: 1,
        GroupConcatMaxLen: 1,
        InExplainStmt: true,
        WarnHandler: Some(Arc::clone(&warn_handler)),
        ExtraWarnghandler: Some(Arc::clone(&warn_handler)),
    };

    // These are exactly the non-ignored fields in Go's recursively-not-equal
    // assertion against an empty BuildPBContext.
    // 以下为 Go 与空 BuildPBContext 做递归不等断言时的非忽略字段。
    assert!(obj.TiFlashFastScan);
    assert_ne!(obj.TiFlashFineGrainedShuffleBatchSize, 0);
    assert_ne!(obj.GroupConcatMaxLen, 0);
    assert!(obj.InExplainStmt);

    let static_obj = obj.Detach(Arc::clone(&obj.ExprCtx));

    // Detach returns a distinct struct whose non-ignored fields equal the
    // original, matching AssertDeepClonedEqual in the Go test.
    // Detach 返回独立结构体，非忽略字段与原对象相等。
    assert!(!std::ptr::eq(&obj, static_obj.as_ref()));
    assert_eq!(obj.TiFlashFastScan, static_obj.TiFlashFastScan);
    assert_eq!(
        obj.TiFlashFineGrainedShuffleBatchSize,
        static_obj.TiFlashFineGrainedShuffleBatchSize
    );
    assert_eq!(obj.GroupConcatMaxLen, static_obj.GroupConcatMaxLen);
    assert_eq!(obj.InExplainStmt, static_obj.InExplainStmt);

    // The four ignored paths retain Go interface/pointer identity.
    // 四条忽略路径保留 Go 接口/指针身份。
    assert!(Arc::ptr_eq(&obj.ExprCtx, &static_obj.ExprCtx));
    assert!(obj.Client.is_none());
    assert!(static_obj.Client.is_none());
    assert!(Arc::ptr_eq(
        obj.WarnHandler.as_ref().expect("warn handler"),
        static_obj
            .WarnHandler
            .as_ref()
            .expect("detached warn handler")
    ));
    assert!(Arc::ptr_eq(
        obj.ExtraWarnghandler.as_ref().expect("extra warn handler"),
        static_obj
            .ExtraWarnghandler
            .as_ref()
            .expect("detached extra warn handler")
    ));
}
