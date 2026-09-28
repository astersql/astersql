// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/checksum/executor_nokit_test.go`.
//!
//! Pure builder field coverage — no database, TiKV, or checksum execution.
//!
//! 模块职责：覆盖 ExecutorBuilder 上 RequestSource 相关 setter/getter，
//! 对齐 Go `executor_nokit_test.go` 的无套件（nokit）路径。
//! 约束：不碰库表、TiKV 与真实 checksum 执行，仅校验 builder 字段语义。

use crate::{NewExecutorBuilder, RequestSource, TableInfo};

/// Corresponds to Go `TestBuilderRequestSource`.
///
/// 验证三条路径与 Go 一致：新建零值、显式类型覆盖、整对象替换。
#[test]
fn test_builder_request_source() {
    // Go: NewExecutorBuilder(nil, 0) — empty table info stand-in.
    // Rust 用 TableInfo::default() 代替 Go 的 nil 表信息占位。
    let mut b = NewExecutorBuilder(TableInfo::default(), 0);
    // 新建 builder 的 request_source 必须保持 Go util.RequestSource{} 零值。
    assert_eq!(
        &RequestSource::default(),
        b.request_source(),
        "new builder keeps util.RequestSource{{}} zero value"
    );

    // SetExplicitRequestSourceType 只改显式类型字段，其余保持默认。
    b = b.SetExplicitRequestSourceType("aaa".to_owned());
    assert_eq!(
        &RequestSource {
            ExplicitRequestSourceType: "aaa".to_owned(),
            ..Default::default()
        },
        b.request_source()
    );

    // SetRequestSource 整对象写入后，getter 应原样返回（含 Internal/Type/Explicit）。
    let req_source = RequestSource {
        RequestSourceInternal: true,
        RequestSourceType: "type".to_owned(),
        ExplicitRequestSourceType: "bbb".to_owned(),
    };
    b = b.SetRequestSource(req_source.clone());
    assert_eq!(&req_source, b.request_source());
}
