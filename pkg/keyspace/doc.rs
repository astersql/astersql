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

// Package keyspace provides utilities for keyspace for nextgen TiDB.
//
// Keyspace are used to isolate data and operations, allowing for multi-tenancy
// in next generation TiDB. Each keyspace represents a logical cluster on top of
// the underlying physical cluster.
//
// Keyspace（键空间）：在同一物理集群上隔离租户数据与操作的逻辑命名空间；
// 每个 keyspace 相当于一层逻辑集群，实现多租户（multi-tenancy）。
//
// Keyspace 分为用户 keyspace 与保留的内部 keyspace；目前只有 SYSTEM 被保留给内部用途。
//
// SYSTEM keyspace is reserved for system-level services and data, currently,
// only the DXF service uses this keyspace. As user keyspace depends on SYSTEM
// keyspace, we need to make sure SYSTEM keyspace exist before user keyspace
// start serving any user traffic.
//
// SYSTEM：系统级保留键空间，承载 DXF 等内部服务；用户 keyspace 依赖其已就绪，
// 因此必须先于用户流量完成引导（bootstrap）。
//
// nextgen 集群部署必须保持以下先后关系：
// - 先部署 PD、TiKV 等底层组件，并等待它们可以承接 TiDB 访问；
// - 再部署 SYSTEM keyspace，并等待其完整引导；
// - 最后部署其他用户 keyspace，这些用户 keyspace 可以并发部署。
//
// PD（Placement Driver）负责调度与元数据；TiKV 为分布式 KV 存储引擎。
//
// 升级同样必须先处理 SYSTEM keyspace，再升级用户 keyspace，避免破坏用户 keyspace 对系统服务的依赖。
//
// 注意：serverless 也使用 keyspace，并额外具有特殊的 NULL 和 DEFAULT keyspace；nextgen 当前没有这两类。
//
// 本文件为包级文档模块（对应 Go `doc.go`），不含可执行逻辑。
//
