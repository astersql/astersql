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

//! Package checkpoint computes a downstream-safe checkpoint for CRR.
//!
//! 包级文档：对齐 Go `br/pkg/stream/crr/internal/checkpoint/doc.go`。
//! 计算器按轮推进：读取上游全局检查点 `c`，扫描 `flushTS > syncedTS` 的新 meta，
//! 等待这些 meta 引用的全部文件确认已复制，再返回安全检查点 `c`。
//!
//! The calculator advances in rounds. In each round it reads the current upstream
//! global checkpoint `c`, scans new meta files with `flushTS > syncedTS`, waits
//! until every file referenced by those meta files is confirmed replicated, then
//! returns `c`.
//!
//! CRR restore safety cannot be decided from `flushTS <= c` alone. A flush batch
//! named by `flushTS` may still be required to restore a smaller checkpoint `c`,
//! because the region checkpoint carried by that batch can be strictly smaller
//! than the meta file's `flushTS`.
//!
//! 安全约束：不能仅凭 `flushTS <= c` 判定可恢复；以 `flushTS` 命名的批次仍可能
//! 服务于更小的区域检查点 `c`（批次内 region checkpoint 可严格小于 flushTS）。
//!
//! `syncedTS` is the replication-complete checkpoint. It advances only after the
//! calculator has verified that all files discovered in the round are already
//! replicated. `lastCheckpoint` is the last upstream global checkpoint returned by
//! the calculator.
//!
//! `syncedTS` 表示复制完成进度，仅在本轮发现的文件全部确认后才前进；
//! `lastCheckpoint` 是最近一次返回给调用方的上游全局检查点。
//!
//! The calculator tracks synced progress per store and publishes the global
//! `syncedTS` as the minimum synced `flushTS` across the store progress that still
//! constrains the current round. This is necessary because meta file names are
//! globally ordered by `(flushTS, storeID, extraTags)`, while `flushTS` is only
//! monotonic within each individual store.
//!
//! 按 store 追踪同步进度，全局 `syncedTS` 取仍约束本轮的各 store 已同步 flushTS 最小值。
//! 原因：meta 文件名全局按 `(flushTS, storeID, extraTags)` 排序，但 flushTS 仅在单 store 内单调。
//!
//! The alive-store set is used only as an extra blocker: if PD still reports a
//! store as alive but the calculator has not observed any flush progress for that
//! store yet, `syncedTS` must not advance. Alive stores must never make `syncedTS`
//! move faster than the minimum across the current round's synced store progress.
//! Stores that are no longer alive are pruned after a successful round has scanned
//! and waited all newly discovered files. Their pre-prune progress can still bound
//! that successful round's `syncedTS`, but it no longer blocks future rounds.
//!
//! alive store 仅作额外阻塞：PD 仍报存活但尚未观察到 flush 时，`syncedTS` 不得前进；
//! 不得因 alive 集合把 `syncedTS` 推得快于本轮已同步 store 进度的最小值。
//! 不再存活的 store 在成功轮次扫描并等待完新文件后剪枝；剪枝前进度仍可约束当轮，
//! 但不再阻塞后续轮次。
//!
//! For example:
//!
//! - `0672E0E5956C00020000000000000004-<...>.meta`
//! - `0672E0E5A00000000000000000000002-<...>.meta`
//!
//! Meta file names are ordered as `{flushTS:016X}{storeID:016X}-<...>.meta`.
//!
//! As an example:
//!
//! ```text
//! 0672E0E5956C00020000000000000004-<...>.meta
//! |flushTS ------||storeID ------|  ->  flushTS = 0x0672E0E5956C0002, storeID = 4
//! ```
//!
//! If one alive store is only known synced through `0x0672E0E5956C0002`, while
//! another is synced through `0x0672E0E5A0000000`, then the global `syncedTS`
//! must stay at `min(0x0672E0E5956C0002, 0x0672E0E5A0000000)`. If PD reports an
//! additional alive store that has not been observed yet, that missing store must
//! block advancement, but it must not raise the minimum. If an observed store is no
//! longer alive, its synced progress can bound the current round and is then removed
//! after the round verifies its discovered files.
//!
//! 示例：两 store 同步进度不同时取 min；额外未观察的 alive store 只阻塞不抬高最小值；
//! 已观察但下线的 store 可约束当轮，验证完其文件后移除。
//!
//! This algorithm relies on these invariants:
//!
//! - meta file names are ordered by `(flushTS, storeID, extraTags)`
//! - `flushTS` is the leading ordering key in the meta file name
//! - for each individual store, its own meta files have monotonically
//!   increasing `flushTS`
//!
//! 算法不变量：meta 名按 `(flushTS, storeID, extraTags)` 排序；flushTS 为前导键；
//! 单 store 内 meta 的 flushTS 单调递增。
//!
//! The calculator itself only depends on an `ObjectSyncChecker`. Wiring outside the
//! core may implement that checker by verifying downstream object existence, or by
//! consulting source-side replication metadata when the storage backend can prove
//! equivalent safety. The calculator must not read downstream object contents.
//!
//! 核心只依赖 `ObjectSyncChecker`；外部可用下游存在性或源端复制元数据证明等价安全。
//! 计算器不得读取下游对象内容。
