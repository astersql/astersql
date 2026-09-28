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

// Package ddl is the core of TiDB DDL layer. It is used to manage the schema of
// TiDB Cluster.
//
// TiDB executes using the Online DDL algorithm, see docs/design/2018-10-08-online-DDL.md
// for more details.
//
// DDL maintains the following invariant:
//
// At any time, for each schema object, such as a table, there are at most 2 versions
// can exist for it, current version N loaded by all TiDBs and version N+1 pushed
// forward by DDL, before we can finish the DDL or continue to next operation, we
// need to make sure all TiDBs have synchronized to version N+1.
// Note that we are using a global version number for all schema objects, so the
// versions related some table might not be continuous, as DDLs are executed in parallel.
//
// Package invariants retained from Go doc.go.
//
//
//
// 本模块（ddl 包）是 TiDB DDL（Data Definition Language，数据定义语言）层的核心，
// 负责管理整个 TiDB 集群的 schema（模式，即数据库、表、索引等元数据对象的结构定义）。
// DDL 泛指 `CREATE TABLE`、`ALTER TABLE`、`DROP INDEX` 等改变表结构的语句。
//
// TiDB 采用 Online DDL（在线 DDL）算法执行结构变更：变更过程中不长时间锁表，
// 读写请求可以继续进行。该算法源自 Google F1 的在线异步 schema 变更论文，
// 详细设计见 docs/design/2018-10-08-online-DDL.md。
//
// DDL 层维护如下关键不变量（invariant）：
//
// 在任意时刻，对每个 schema 对象（例如一张表），最多只允许同时存在 2 个版本：
// 所有 TiDB 节点已加载的当前版本 N，以及由 DDL 推进产生的下一版本 N+1。
// 在完成本次 DDL 或继续下一步操作之前，必须确认集群中所有 TiDB 节点
// 都已同步到版本 N+1（即 schema lease 同步机制）。
// 这样可以保证集群中并存的 schema 版本差不超过 1，从而使各节点在
// 中间状态（如 delete-only、write-only 等）之间安全过渡，避免数据不一致。
//
// 注意：schema 版本号是全局单调递增的，所有 schema 对象共用同一个版本序列；
// 由于多条 DDL 可并行执行，某张表相关的版本号可能并不连续。
//
// 上述不变量说明保留自 Go 版本的 doc.go。
