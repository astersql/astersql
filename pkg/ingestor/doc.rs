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

//! Package ingestor provides interfaces for ingesting SSTs directly into the
//! underlying storage layer. It also provides utilities to:
//!
//! - Sort encoded KVs locally or globally, using local disk for partial sorting
//!   or external storage for intermediate sorted files before merge sorting.
//! - Prepare the environment for writing KVs and ingesting SSTs, including
//!   pausing PD schedulers, splitting and scattering regions based on the sorted
//!   KV range, and switching TiKV to import mode.
//!
//! Most implementations currently live in `pkg/lightning/backend` and will be
//! moved into this package gradually.
//!
//! 包 ingestor：提供将 SST 直接导入底层存储的接口与辅助能力。
//!
//! 主要用途包括：
//! - 对已编码的 KV 做本地或全局排序（局部排序用本地盘，归并前中间文件可用外部存储）；
//! - 为写 KV / 导入 SST 准备环境：暂停 PD 调度、按有序键范围切分并打散 Region、
//!   将 TiKV 切到 import 模式。
//!
//! 多数实现目前仍在 `pkg/lightning/backend`，会逐步迁入本包。
//!
//! SST：有序键值表文件；Region：TiKV 的数据分片与调度单位；PD：Placement Driver，集群调度中心。
