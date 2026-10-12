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

// metabuild::Context 单元测试：默认值与 Option 覆盖顺序。
//
// 下方 Rust 用例验证 NewContext 与 Session 默认常量对齐，以及多个 Option 按声明顺序覆盖。

use std::convert::Infallible;

use crate::{
    NewContext, WithEnableAutoIncrementInGenerated, WithPreSplitRegions, WithPrimaryKeyRequired,
    WithShardRowIDBits, vardef,
};

/// 空 Option 列表时，关键字段应等于 vardef 中的会话默认常量。
#[test]
fn context_defaults_match_session_defaults() {
    let context = NewContext::<(), Infallible>(Vec::new());
    assert_eq!(
        context.EnableAutoIncrementInGenerated(),
        vardef::DefTiDBEnableAutoIncrementInGenerated
    );
    assert!(!context.PrimaryKeyRequired());
    assert_eq!(
        context.GetShardRowIDBits(),
        vardef::DefShardRowIDBits as u64
    );
    assert_eq!(
        context.GetPreSplitRegions(),
        vardef::DefPreSplitRegions as u64
    );
    assert!(!context.GetInfoSchema().1);
}

/// 同一字段多次 Option 时以后者为准；不同字段各自生效。
#[test]
fn context_options_override_in_declaration_order() {
    let context = NewContext::<(), Infallible>(vec![
        WithEnableAutoIncrementInGenerated(false),
        WithEnableAutoIncrementInGenerated(true),
        WithPrimaryKeyRequired(true),
        WithShardRowIDBits(8),
        WithPreSplitRegions(456),
    ]);
    assert!(context.EnableAutoIncrementInGenerated());
    assert!(context.PrimaryKeyRequired());
    assert_eq!(context.GetShardRowIDBits(), 8);
    assert_eq!(context.GetPreSplitRegions(), 456);
}
