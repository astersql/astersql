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

// 本文件由 pkg/config/deploymode/doc.go 迁移而来，保留 Go 文档结构。

// deploymode 保存 TiDB X（NextGen）部署模式的进程级配置语义。
//
// 术语说明：
// - TiKV：分布式键值存储层，负责数据的持久化与多副本一致性（基于 Raft 协议）。
// - coprocessor（协处理器）：下推到存储层执行的计算组件，可在靠近数据处执行
//   过滤、聚合等算子，减少网络传输。
// - keyspace（键空间）：对键值数据的逻辑隔离单元，多租户场景下每个租户可拥有
//   独立 keyspace；SYSTEM keyspace 是系统保留的键空间。
// - worker（工作节点）：承担后台任务或弹性计算的进程实例，可按需扩缩容。
//
// TiDB X Premium Reserved 保留 Premium 产品能力集，但采用固定资源部署形态，
// 而不是标准 Premium 的弹性形态。资源范围在集群启动时确定，TiDB-worker、
// TiKV-worker 和 coprocessor-worker 不会按需扩缩，因此后台任务不能假设需求增加时
// 会自动创建更多 worker 资源。
//
// Premium Reserved 会把 Premium 行为调整到固定资源形态：避免部署 TiKV-worker 和
// coprocessor-worker，并合并 TiDB 与 TiDB-worker 行为。用户流量和分布式任务直接
// 在 TiDB 节点运行，且都运行在 SYSTEM keyspace。
//
// Starter 是用于支持大量小租户的部署模式。
//
// 部署模式在 TiDB 启动期间初始化，设置后不可更改。它存放在 TiDB 组件配置中，
// 如果不同 TiDB 实例用不同配置启动，部署模式可能不一致。这个取舍是有意的：
// 保持在 TiDB 配置中可以避免修改其它组件，也避免维护单独二进制。Premium Reserved
// 主要用于云上部署，同一组 TiDB 实例通常用相同配置启动，因此一致性风险可接受。
