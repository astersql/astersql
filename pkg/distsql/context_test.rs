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

//! Test-only DistSQL context defaults.
//!
//! This is the Rust counterpart of Go's `NewDistSQLContextForTest` and
//! `DefaultDistSQLContext`. The full DistSQL dependency graph is currently
//! enabled only for the Windows migration target, matching `Cargo.toml`.

#[cfg(target_os = "windows")]
use std::sync::{Arc, LazyLock};

#[cfg(target_os = "windows")]
use astersql_distsql_context::{DistSQLContext, WarnAppenderRef, contextutil, errctx};
#[cfg(target_os = "windows")]
use astersql_sessionctx_vardef as vardef;

/// Creates the test context defaults used by DistSQL request-builder tests.
#[cfg(target_os = "windows")]
pub(crate) fn NewDistSQLContextForTest() -> DistSQLContext<'static> {
    // A zero-capacity handler has the same observable no-op warning behavior as
    // Go's callback appender and `contextutil.IgnoreWarn` error context.
    let warn_handler: WarnAppenderRef = Arc::new(contextutil::NewStaticWarnHandler(0));
    let err_context = errctx::NewContext(Arc::clone(&warn_handler));

    DistSQLContext {
        WarnHandler: warn_handler,
        TiFlashMaxThreads: vardef::DefTiFlashMaxThreads,
        TiFlashMaxBytesBeforeExternalJoin: vardef::DefTiFlashMaxBytesBeforeExternalJoin,
        TiFlashMaxBytesBeforeExternalGroupBy: vardef::DefTiFlashMaxBytesBeforeExternalGroupBy,
        TiFlashMaxBytesBeforeExternalSort: vardef::DefTiFlashMaxBytesBeforeExternalSort,
        TiFlashMaxQueryMemoryPerNode: vardef::DefTiFlashMemQuotaQueryPerNode,
        TiFlashQuerySpillRatio: vardef::DefTiFlashQuerySpillRatio,
        TiFlashHashJoinVersion: vardef::DefTiFlashHashJoinVersion.to_owned(),
        DistSQLConcurrency: vardef::DefDistSQLScanConcurrency as isize,
        MinPagingSize: vardef::DefMinPagingSize as isize,
        MaxPagingSize: vardef::DefMaxPagingSize as isize,
        ResourceGroupName: "default".to_owned(),
        ErrCtx: err_context,
        ..DistSQLContext::default()
    }
}

/// Empty test context without a client, equivalent to Go's package variable.
#[cfg(target_os = "windows")]
pub(crate) static DefaultDistSQLContext: LazyLock<DistSQLContext<'static>> =
    LazyLock::new(NewDistSQLContextForTest);

#[cfg(target_os = "windows")]
#[test]
fn test_context_factory_matches_go_defaults() {
    let context = NewDistSQLContextForTest();

    assert!(context.Client.is_none());
    assert_eq!(context.TiFlashMaxThreads, vardef::DefTiFlashMaxThreads);
    assert_eq!(
        context.TiFlashMaxBytesBeforeExternalJoin,
        vardef::DefTiFlashMaxBytesBeforeExternalJoin
    );
    assert_eq!(
        context.TiFlashMaxBytesBeforeExternalGroupBy,
        vardef::DefTiFlashMaxBytesBeforeExternalGroupBy
    );
    assert_eq!(
        context.TiFlashMaxBytesBeforeExternalSort,
        vardef::DefTiFlashMaxBytesBeforeExternalSort
    );
    assert_eq!(
        context.TiFlashMaxQueryMemoryPerNode,
        vardef::DefTiFlashMemQuotaQueryPerNode
    );
    assert_eq!(
        context.TiFlashQuerySpillRatio,
        vardef::DefTiFlashQuerySpillRatio
    );
    assert_eq!(
        context.TiFlashHashJoinVersion,
        vardef::DefTiFlashHashJoinVersion
    );
    assert_eq!(
        context.DistSQLConcurrency,
        vardef::DefDistSQLScanConcurrency as isize
    );
    assert_eq!(context.MinPagingSize, vardef::DefMinPagingSize as isize);
    assert_eq!(context.MaxPagingSize, vardef::DefMaxPagingSize as isize);
    assert_eq!(context.ResourceGroupName, "default");

    assert!(DefaultDistSQLContext.Client.is_none());
    assert_eq!(
        DefaultDistSQLContext.DistSQLConcurrency,
        vardef::DefDistSQLScanConcurrency as isize
    );
}
