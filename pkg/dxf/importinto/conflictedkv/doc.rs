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

// Package conflictedkv provides the functionality to handle KVs due to conflicted
// rows when importing data into TiDB using the ImportInto statement.
//
// conflicted rows are handled in the following ways:
//
// # 1. collect duplicated KVs during global sort and ingest phase
//
// in below, duplicated KVs means the key is duplicated, and the value might or
// might not be the same.
//
// When find duplicated KV in the steps of global sort, we either
//   - record, which means the KV is deleted from the encode-merge-ingest workflow,
//     and is recorded to the cloud storage for later conflict resolution. we do
//     this for duplicated data KVs and unique key (UK) KVs.
//   - or remove directly. we do this for duplicated non-unique index KVs.
//
// We record the duplicated KVs in encode and merge step too, to avoid have too
// many duplicated KVs to be recorded during ingest step, and to avoid the
// generated region job too small due to too many duplicated KVs being removed,
// then to avoid the target region to be too much undersized.
//
//   - during encode step, the duplicated data and UK KVs due to conflicted rows
//     are collected and write to the cloud storage as a separate file. there might
//     be duplicated KVs due to non-unique indices, but they are not usefully for
//     later conflict resolution, so they are removed directly without recording.
//     We only record KVs that duplicate more than 2 times, such as if a key has
//     5 duplicates, we record 3 of them, and keep other 2 in the KV files, as
//     we only have a local view of the KVs being imported at this step, we have
//     to keep 2 copies to ensure we can find other duplicated KVs later.
//   - during merge step, we also record the duplicated KVs, same as the encode step.
//   - during ingest step, we also record the duplicated KVs, but since we have
//     a global view of all conflicted KVs at this step, we will record all
//     duplicated KVs.
//
// # 2. conflict resolution
//
// this part is separated into 2 steps, collect-conflicts and resolve-conflicts.
// the former step collects all necessary info about the conflicted KVs, especially
// the checksum of original rows, we will use this checksum to get the final
// checksum of the expected data ingested into the cluster, and verify it with
// the remote checksum. And we also record all the conflicted rows into the cloud
// storage for user to check later, we record them under the 'conflicted_rows/'
// prefix.
//
// duplicated data KV and UK KV are handled differently:
//   - for duplicated data KVs, we can decode to get Datum of all columns and
//     re-encode into KVs, and we can calculate the checksum of the original rows
//   - for duplicated UK KVs, we can only get the handle of the row, we have to
//     get the data KV from the cluster, if not found, it means the data KV is
//     also duplicated and recorded before, it will be handled in previous step,
//     so we can skip it. Otherwise, we can decode and re-encode to get the
//     checksum of the original row. since there might be multiple UKs for a
//     single row, we need to avoid calculating the checksum multiple times for
//     the same row. currently, we do this deduplication in memory by maintaining
//     a set of handles processed. if the number of handles is too large to fit
//     in memory, we will skip the later checksum part, as we cannot get the exact
//     checksum in this case.
//
// the latter step resolves the conflicts by deleting all the KVs related to the
// conflicted rows from the cluster, the logic to get all those KVs is similar
// with the collect-conflicts step,
//
// # 3. checksum verification
//
// after conflict resolution, we need to minus the checksum of the conflicted rows
// from the total checksum calculated during the encode step to get the final
// checksum of the data ingested into the cluster, and verify it with the remote
// checksum.
//
// # 4. an example of the whole workflow
//
// as an example, suppose we have below table:
//
//     create table t(
//         id int primary key clustered,
//         c1 int,
//         c2 int,
//         unique(c1),
//         index(c2)
//     );
//
// and we have below rows to be imported:
//
//         id,c1,c2
//         1,1,1
//         1,1,1
//         2,1,1
//         3,3,3
//
// they will generate below KVs:
//
//     <1,1,1> -> [(handle=1 -> <1,1,1>), (c1=1 -> handle=1), (c2=1,handle=1 -> "0")]
//     <1,1,1> -> [(handle=1 -> <1,1,1>), (c1=1 -> handle=1), (c2=1,handle=1 -> "0")]
//     <2,1,1> -> [(handle=2 -> <2,1,1>), (c1=1 -> handle=2), (c2=1,handle=2 -> "0")]
//     <3,3,3> -> [(handle=3 -> <3,3,3>), (c1=3 -> handle=3), (c2=3,handle=3 -> "0")]
//
// during encode->merge->ingest workflow, below KVs will be recorded for later
// conflict resolution:
//
//   - data KVs: [(handle=1 -> <1,1,1>), (handle=1 -> <1,1,1>)]
//   - UK KVs: [(c1=1 -> handle=1), (c1=1 -> handle=1), (c1=1 -> handle=2)]
//
// and below KVs will be removed directly:
//
// - non-unique index KVs: [(c2=1,handle=1 -> "0"), (c2=1,handle=1 -> "0")]
//
// after the ingest phase, the cluster will only have below KVs:
//
//   - data KVs: [(handle=2 -> <2,1,1>), (handle=3 -> <3,3,3>)]
//   - UK KVs: [(c1=3 -> handle=3)]
//   - non-unique index KVs: [(c2=1,handle=2 -> "0"), (c2=3,handle=3 -> "0")]
//
// we can see that the KVs ingested into the cluster is not consistent.
//
// during the collect-conflicts step of conflict resolution phase, we will
// process the recorded duplicated KVs one by one, and calculated the checksum
// of the original conflicted rows, i.e. checksum of [(1,1,1), (1,1,1), (2,1,1)].
//
// during the resolve-conflicts step of conflict resolution phase, we will also
// process the recorded duplicated KVs one by one, and delete all KVs related
// to the conflicted rows from the cluster.
//
// as an example for the recorded data KV (handle=1 -> <1,1,1>), we will re-encode
// it to get all KVs for the row <1,1,1>:
//
//     1,1,1 -> [(handle=1 -> <1,1,1>), (c1=1 -> handle=1), (c2=1,handle=1 -> "0")]
//
// and delete them from the cluster, but since the all those KVs are already
// removed during encode-merge-ingest workflow, there will be no change to the
// cluster.
//
// as an example for the recorded UK KV (c1=1 -> handle=2), we will get its data
// KV from the cluster which is (handle=2 -> <2,1,1>), and re-encode it to get
// all KVs for the row <2,1,1>:
//
//     2,1,1 -> [(handle=2 -> <2,1,1>), (c1=1 -> handle=2), (c2=1,handle=2 -> "0")]
//
// and delete them from the cluster, after deleting, the cluster will only have
// below KVs:
//
//   - data KVs: [(handle=3 -> <3,3,3>)]
//   - UK KVs: [(c1=3 -> handle=3)]
//   - non-unique index KVs: [(c2=3,handle=3 -> "0")]
//
// we can see that the KVs in the cluster is consistent now.
//
// during the encode step, we have calculated the checksum of all KVs to the rows
// in the data file, and we will minus the checksum of [(1,1,1), (1,1,1), (2,1,1)]
// so only checksum of [(3,3,3)] is left, which is consistent with the data in
// the cluster.

//
// ---
//
// # 中文说明
//
// `conflictedkv` 处理 Import Into 导入时因冲突行产生的 KV。
//
// ## 1. 在 global sort 与 ingest 阶段收集重复 KV
//
// 「重复 KV」指 key 重复，value 可相同也可不同。
// 发现重复时：对 data KV 与唯一键（UK）KV **记录**（移出 encode-merge-ingest
// 工作流并写入对象存储，供后续冲突解决）；对非唯一索引 KV 则直接丢弃。
//
// 在 encode / merge 阶段也记录，是为了减少 ingest 时待记录量，并避免因大量
// 去重导致 Region（键空间分片）任务过小、目标 Region 严重偏小。
//
// - **encode**：只记录重复超过 2 次的 data/UK KV（例如 5 份重复则记录 3 份、
//   保留 2 份在 KV 文件中），因本阶段仅有局部视图。
// - **merge**：同样记录。
// - **ingest**：已有全局视图，记录全部重复 KV。
//
// ## 2. 冲突解决（collect-conflicts + resolve-conflicts）
//
// **collect-conflicts**：收集冲突行信息，尤其原行 checksum（校验和），用于
// 从 encode 总 checksum 中扣减后与远程 checksum 校验；并把冲突行写到
// `conflicted_rows/` 前缀供用户查看。
//
// - data KV：可解码全部列 Datum 并重编码，直接算原行 checksum。
// - UK KV：只能得到行的 handle（行标识），需从集群取 data KV；若不存在说明
//   data 侧已作为冲突记录过，可跳过。多 UK 时用内存 handle 集合去重；内存不足
//   则跳过后续 checksum（无法得到精确值）。
//
// **resolve-conflicts**：按类似逻辑找出冲突行相关全部 KV，从集群删除。
//
// ## 3. checksum 校验
//
// 冲突解决后，从 encode 总 checksum 减去冲突行 checksum，得到最终入库数据
// 的 checksum，再与远程 checksum 比对。
//
// ## 4. 工作流示例要点
//
// 导入含主键冲突与 UK 冲突的多行后，ingest 结束时集群 KV 可能不一致；
// collect 算出冲突行 checksum，resolve 删除相关 KV，最终只剩一致行，
// 扣减后的 checksum 与集群数据一致。
