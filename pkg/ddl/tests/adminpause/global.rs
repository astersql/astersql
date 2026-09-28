// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Admin Pause 集成测试的全局环境准备。
//
// 提供 mock Domain（领域服务容器，持有 DDL owner、schema lease 等）、
// 执行 DDL 的 TestKit，以及发送 `ADMIN PAUSE/RESUME` 的独立 TestKit。
// Schema Lease（schema 租约）控制节点缓存元数据的有效期；测试中缩短为 600ms
// 以加快状态推进。Reorg batch size / worker 数影响数据回填并行度。

/// Schema lease 时长（毫秒），对应 Go 侧 `dbTestLease`。
pub const DB_TEST_LEASE_MILLIS: u64 = 600;

/// Logger 对应 Go 包级 DDL logger；用字符串保留全局依赖形状。
// Logger 对应 Go 包级 DDL logger；用字符串保留全局依赖形状。
pub static LOGGER: &str = "logutil.DDLLogger()";

use std::sync::Arc;

use astersql_domain::Domain;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomainWithSchemaLease};

/// `prepare_domain` 的返回值：共享 mock store/domain 与两套独立会话。
// PreparedDomain 对应 prepareDomain 的三个返回值，并保留 store 生命周期。
pub struct PreparedDomain {
    pub store: Arc<AnalyzeStatsStore>,
    /// Mock Domain。
    pub domain: Arc<Domain>,
    /// 执行业务 DDL / DML 的 TestKit。
    pub stmt_kit: TestKit,
    /// 发送 admin pause/resume/cancel 命令的独立 TestKit。
    pub admin_command_kit: TestKit,
}

/// 创建 mock store/domain，并配置 DDL reorg batch/worker 参数。
// prepareDomain 创建 mock store/domain，并配置 DDL reorg batch/worker 参数。
pub fn prepare_domain(_t: &()) -> PreparedDomain {
    let (store, domain) = CreateMockStoreAndDomainWithSchemaLease(
        std::time::Duration::from_millis(DB_TEST_LEASE_MILLIS),
    );
    let mut stmt_kit = TestKit::new(store.clone());
    stmt_kit.MustExec("set @@tidb_ddl_reorg_batch_size=2", Vec::new());
    stmt_kit.MustExec("set @@tidb_ddl_reorg_worker_cnt=1", Vec::new());
    let admin_command_kit = TestKit::new(store.clone());
    // Go deliberately replaces the setup session before running statements.
    let mut stmt_kit = TestKit::new(store.clone());
    stmt_kit.MustExec("use test", Vec::new());

    PreparedDomain {
        store,
        domain,
        stmt_kit,
        admin_command_kit,
    }
}
