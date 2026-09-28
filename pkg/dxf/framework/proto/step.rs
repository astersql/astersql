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

// 任务 Step（阶段）定义与字符串转换。
//
// Step 是任务流水线中的阶段编号：框架标记步（Init/Done/Prepared）为负值，
// 业务步为正值且按任务类型（ImportInto、Backfill 等）区分。
// 常量数值不可改动，否则破坏持久化兼容性。
// 成功任务有两种框架流：默认 Init→业务步→Done；prepare 模式多一步 Prepared。

use super::task::TaskType;
use super::r#type::{Backfill, ImportInto, TaskTypeExample};

/// 任务 step 类型别名（i64，对齐 Go）。
// Step is the step of task.
pub type Step = i64;

// 任务 step 常量。
// 禁止修改常量值，否则破坏向后兼容。
// 成功任务有两种框架流：
//  1. 默认：StepInit → 业务步 → StepDone。
//  2. prepare 模式：StepInit → StepPrepared → 业务步 → StepDone。
// TaskStep is the step of task.
// DO NOT change the value of the constants, will break backward compatibility.
// Successful task has two framework flows:
//  1. default flow: StepInit -> business steps -> StepDone.
//  2. prepare-mode flow: StepInit -> StepPrepared -> business steps -> StepDone.
pub const StepInit: Step = -1;
pub const StepDone: Step = -2;
// StepPrepared：框架 prepare 逻辑已完成，但任务状态仍为 pending。
// StepPrepared marks that framework prepare logic has finished while task
// state is still pending.
pub const StepPrepared: Step = -3;

/// 未知 step 字符串前缀，用于合法性判断。
const unknownStepPrefix: &str = "unknown step";

/// 将 (任务类型, step) 转为可读字符串；未知类型/step 返回 unknown 格式。
/// step 用整型定义带来扩展不便，此处保留 Go 行为。
// Step2Str converts step to string.
// it's too bad that we define step as int.
pub fn Step2Str(t: TaskType, s: Step) -> String {
    // Init/Done/Prepared 为框架特殊步，不校验任务类型。
    // StepInit, StepDone and StepPrepared are special steps, we don't check task
    // type for them.
    match s {
        StepInit => return "init".to_string(),
        StepDone => return "done".to_string(),
        StepPrepared => return "prepared".to_string(),
        _ => {}
    }
    match t {
        Backfill => backfillStep2Str(s),
        ImportInto => importIntoStep2Str(s),
        TaskTypeExample => exampleStep2Str(s),
        // Go 对未知任务类型返回 "unknown type %s"，这里保留格式化分支。
        _ => format!("unknown type {}", t),
    }
}

/// 该 step 对该任务类型是否合法（字符串不含 unknown step 前缀）。
// IsValidStep returns whether the step is valid for the task type.
pub fn IsValidStep(t: TaskType, s: Step) -> bool {
    let str = Step2Str(t, s);
    !str.contains(unknownStepPrefix)
}

/// 是否为合法业务 step；排除 Init/Done/Prepared 等框架标记步。
// IsValidBusinessStep returns whether the step is a business step valid for
// the task type. Framework marker steps are excluded.
pub fn IsValidBusinessStep(t: TaskType, s: Step) -> bool {
    if s == StepInit || s == StepDone || s == StepPrepared {
        // Go 明确排除框架标记 step，只认可业务 step。
        return false;
    }
    IsValidStep(t, s)
}

/// 示例任务类型的业务 step。
// Steps of example task type.
pub const StepOne: Step = 1;
pub const StepTwo: Step = 2;
pub const StepThree: Step = 3;

/// 示例任务 step → 字符串。
fn exampleStep2Str(s: Step) -> String {
    match s {
        StepOne => "one".to_string(),
        StepTwo => "two".to_string(),
        StepThree => "three".to_string(),
        _ => unknownStepStr(s),
    }
}

// Steps of IMPORT INTO, each step is represented by one or multiple subtasks.
// the initial step is StepInit(-1)
// steps are processed in the following order:
//
//   - local sort:
//     StepInit
//     -> ImportStepImport
//     -> ImportStepPostProcess
//     -> StepDone
//   - global sort:
//     StepInit
//     -> ImportStepEncodeAndSort
//     -> ImportStepMergeSort (optional)
//     -> ImportStepWriteAndIngest
//     -> ImportStepCollectConflicts (optional)
//     -> ImportStepConflictResolution (optional)
//     -> ImportStepPostProcess
//     -> StepDone
// ImportInto 业务 step 编码必须保持 Go 常量值，避免破坏持久化兼容性。
// ImportStepImport we sort source data and ingest it into TiKV in this step.
// IMPORT INTO 各 step；每步由一个或多个 subtask 表示。
// 初始为 StepInit(-1)。本地排序：Init→Import→PostProcess→Done；
// 全局排序：Init→EncodeAndSort→MergeSort(可选)→WriteAndIngest
// →CollectConflicts(可选)→ConflictResolution(可选)→PostProcess→Done。
// ImportStepImport：排序源数据并 ingest（导入）到 TiKV。
pub const ImportStepImport: Step = 1;
// ImportStepPostProcess：校验 checksum 并加索引。
// ImportStepPostProcess we verify checksum and add index in this step.
pub const ImportStepPostProcess: Step = 2;
// ImportStepEncodeAndSort：编码源数据，将有序 KV 写入全局存储。
// ImportStepEncodeAndSort encode source data and write sorted kv into global storage.
pub const ImportStepEncodeAndSort: Step = 3;
// ImportStepMergeSort：合并全局存储中的有序 KV，提升后续 WriteAndIngest 读性能。
// 视 KV 文件重叠程度，本步可能没有 subtask。
// ImportStepMergeSort merge sorted kv from global storage, so we can have better
// read performance during ImportStepWriteAndIngest.
// depends on how much kv files are overlapped, there's might 0 subtasks
// in this step.
pub const ImportStepMergeSort: Step = 4;
// ImportStepWriteAndIngest：将有序 KV 写入并 ingest 到 TiKV。
// ImportStepWriteAndIngest write sorted kv into TiKV and ingest it.
pub const ImportStepWriteAndIngest: Step = 5;
// ImportStepCollectConflicts：收集冲突信息；不修改下游数据，故幂等，
// 可得到冲突行正确 checksum。若与 ConflictResolution 合并，中途重试会破坏 checksum。
// 因多唯一索引需对冲突行去重，目前在内存中做；冲突过多时后续 checksum 可能跳过。
// ImportStepCollectConflicts collect conflicts info, this step won't mutate
// downstream data, so is idempotent, and we can collect a correct checksum
// for the conflicted rows. if we do this together with ImportStepConflictResolution,
// once the step retry in the middle, we can't get a correct checksum.
// this step also need to do deduplication for the conflicted rows due to
// multiple unique indexes to avoid repeated collection, currently, we do it
// in memory, so if there are too many conflicts, we will skip the later
// checksum step as we don't know the exact checksum.
pub const ImportStepCollectConflicts: Step = 6;
// ImportStepConflictResolution：解决已检测冲突。
// 全局排序其他步会把冲突记到外部存储；有冲突才在此解决，故可能 0 个 subtask。
// ImportStepConflictResolution resolve detected conflicts.
// during other steps of global sort, we will detect conflicts and record them
// in external storage, if any conflicts are detected, we will resolve them
// here. so there might be 0 subtasks in this step.
pub const ImportStepConflictResolution: Step = 7;

/// ImportInto 业务 step → 字符串。
fn importIntoStep2Str(s: Step) -> String {
    match s {
        ImportStepImport => "import".to_string(),
        ImportStepPostProcess => "post-process".to_string(),
        ImportStepEncodeAndSort => "encode".to_string(),
        ImportStepMergeSort => "merge-sort".to_string(),
        ImportStepWriteAndIngest => "ingest".to_string(),
        ImportStepCollectConflicts => "collect-conflicts".to_string(),
        ImportStepConflictResolution => "conflict-resolution".to_string(),
        _ => unknownStepStr(s),
    }
}

// Steps of Add Index, each step is represented by one or multiple subtasks.
// the initial step is StepInit(-1)
// steps are processed in the following order:
// - local sort:
// StepInit -> BackfillStepReadIndex -> StepDone
// - global sort:
// StepInit -> BackfillStepReadIndex -> BackfillStepMergeSort -> BackfillStepWriteAndIngest -> StepDone
// Backfill 业务 step 同样保持 Go 常量值，供持久化任务状态解码。
// 加索引（Add Index / Backfill）各 step；每步一个或多个 subtask。
// 本地排序：Init→ReadIndex→Done；
// 全局排序：Init→ReadIndex→MergeSort→WriteAndIngest→Done。
pub const BackfillStepReadIndex: Step = 1;
// BackfillStepMergeSort：仅全局排序使用；合并全局存储有序 KV 以提升后续 ingest 读性能。
// 视重叠程度；重叠低于 MergeSortOverlapThreshold 时无 subtask。
// BackfillStepMergeSort only used in global sort, it will merge sorted kv from global storage, so we can have better
// read performance during BackfillStepWriteAndIngest with global sort.
// depends on how much kv files are overlapped.
// When kv files overlapped less than MergeSortOverlapThreshold, there're no subtasks.
pub const BackfillStepMergeSort: Step = 2;

// BackfillStepWriteAndIngest：写入有序 KV 并 ingest 到 TiKV。
// BackfillStepWriteAndIngest write sorted kv into TiKV and ingest it.
pub const BackfillStepWriteAndIngest: Step = 3;

// BackfillStepMergeTempIndex：将临时索引合并回原索引。
// BackfillStepMergeTempIndex is the step to merge temp index into the original index.
pub const BackfillStepMergeTempIndex: Step = 4;

/// Backfill 业务 step → 字符串。
// StepStr convert proto.Step to string.
fn backfillStep2Str(s: Step) -> String {
    match s {
        BackfillStepReadIndex => "read-index".to_string(),
        BackfillStepMergeSort => "merge-sort".to_string(),
        BackfillStepWriteAndIngest => "ingest".to_string(),
        BackfillStepMergeTempIndex => "merge-temp-index".to_string(),
        _ => unknownStepStr(s),
    }
}

/// 格式化未知 step：`unknown step <id>`。
fn unknownStepStr(s: Step) -> String {
    format!("{} {}", unknownStepPrefix, s)
}
