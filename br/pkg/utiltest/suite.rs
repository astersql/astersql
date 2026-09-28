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

//! Restore-schema test suite matching `br/pkg/utiltest/suite.go`.
//! 恢复 schema 单测套件：聚合 mock 集群、MockGlue 与本地对象存储。
//! 对应 Go `TestRestoreSchemaSuite` 的构造/启动/Cleanup 生命周期。
//! `_temp_dir` 持有临时目录所有权，避免 Storage 仍指向已删除路径。
//! `Stop` 与 `Drop` 幂等，且只停止 Mock，严格对齐 Go Cleanup。
//! 工厂失败直接 panic，对齐 Go `require.NoError` 的快速失败语义。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use astersql_br_pkg_gluetidb_mock::MockGlue;
use astersql_br_pkg_mock::{Cluster, NewCluster};
use tempfile::TempDir;

use crate::stubs::{self, NewLocalStorage, Storage};

/// TestRestoreSchemaSuite aggregates mock cluster, mock glue, and local storage.
/// Matches Go `TestRestoreSchemaSuite`.
/// 套件聚合体：Mock 集群提供 Domain/Server；Storage 为 local:// 临时目录。
pub struct TestRestoreSchemaSuite {
    pub Mock: Cluster,
    pub MockGlue: MockGlue,
    pub Storage: Arc<dyn Storage>,
    /// Holds Go `t.TempDir()` so the backing path stays alive for the suite.
    /// 必须活到套件结束，否则 LocalStorage 底层路径会失效。
    _temp_dir: TempDir,
    pub(crate) stopped: AtomicBool,
}

/// Alias used by mechanical callers (`utiltest::RestoreSchemaSuite`).
/// 机械移植调用方使用的类型别名，与 Go 导出名对齐。
pub type RestoreSchemaSuite = TestRestoreSchemaSuite;

impl TestRestoreSchemaSuite {
    /// Explicit stop matching Go `s.Mock.Stop()` (also invoked from Drop / cleanup).
    /// 仅首次成功将 stopped 置位时停止集群，后续调用为 no-op。
    pub fn Stop(&mut self) {
        if self
            .stopped
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.Mock.Stop();
        }
    }
}

impl Drop for TestRestoreSchemaSuite {
    fn drop(&mut self) {
        // Go: t.Cleanup(func() { s.Mock.Stop() })
        // 作用域结束时自动 Cleanup，与 Go t.Cleanup 等价。
        self.Stop();
    }
}

/// CreateRestoreSchemaSuite matches Go:
/// ```go
/// s.MockGlue = &gluemock.MockGlue{}
/// s.Mock, err = mock.NewCluster()
/// require.NoError(t, err)
/// base := t.TempDir()
/// s.Storage, err = objstore.NewLocalStorage(base)
/// require.NoError(t, err)
/// require.NoError(t, s.Mock.Start())
/// t.Cleanup(func() { s.Mock.Stop() })
/// ```
/// 按 Go 顺序装配：Glue → Cluster → TempDir 存储 → Start；错误以 panic 暴露。
pub fn CreateRestoreSchemaSuite() -> TestRestoreSchemaSuite {
    let MockGlue = MockGlue::default();

    let mut Mock = NewCluster().unwrap_or_else(|e| panic!("mock.NewCluster: {e}"));

    let _temp_dir = TempDir::new().unwrap_or_else(|e| panic!("t.TempDir: {e}"));
    let Storage = NewLocalStorage(_temp_dir.path())
        .unwrap_or_else(|e| panic!("objstore.NewLocalStorage: {e}"));

    Mock.Start()
        .unwrap_or_else(|e| panic!("mock.Cluster.Start: {e}"));

    TestRestoreSchemaSuite {
        Mock,
        MockGlue,
        Storage,
        _temp_dir,
        stopped: AtomicBool::new(false),
    }
}

/// Re-export stub Context for suite consumers / parity tests.
/// 再导出 stubs::Context，方便套件消费者无需再依赖 stubs 路径。
pub use stubs::Context;
