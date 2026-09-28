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
// 分布式执行框架（DXF）任务 step 转换与校验的单元测试。
//
// 覆盖 `Step2Str`、`IsValidStep`、`IsValidBusinessStep`：框架标记 step
//（init/prepared/done）、业务 step 名称，以及未知 step/任务类型的兜底字符串。

// limitations under the License.

use super::*;

/// 校验各 TaskType 下 step 到可读字符串的映射，含未知值分支。
#[test]
fn test_step() {
    assert_eq!(Step2Str(Backfill, StepInit), "init");
    assert_eq!(Step2Str(Backfill, BackfillStepReadIndex), "read-index");
    assert_eq!(Step2Str(Backfill, BackfillStepMergeSort), "merge-sort");
    assert_eq!(Step2Str(Backfill, BackfillStepWriteAndIngest), "ingest");
    assert_eq!(Step2Str(Backfill, StepPrepared), "prepared");
    assert_eq!(Step2Str(Backfill, StepDone), "done");
    assert_eq!(Step2Str(Backfill, 111), "unknown step 111");

    assert_eq!(Step2Str(ImportInto, StepInit), "init");
    assert_eq!(Step2Str(ImportInto, ImportStepImport), "import");
    assert_eq!(Step2Str(ImportInto, ImportStepPostProcess), "post-process");
    assert_eq!(Step2Str(ImportInto, ImportStepMergeSort), "merge-sort");
    assert_eq!(Step2Str(ImportInto, ImportStepEncodeAndSort), "encode");
    assert_eq!(Step2Str(ImportInto, ImportStepWriteAndIngest), "ingest");
    assert_eq!(
        Step2Str(ImportInto, ImportStepCollectConflicts),
        "collect-conflicts"
    );
    assert_eq!(
        Step2Str(ImportInto, ImportStepConflictResolution),
        "conflict-resolution"
    );
    assert_eq!(Step2Str(ImportInto, StepPrepared), "prepared");
    assert_eq!(Step2Str(ImportInto, StepDone), "done");
    assert_eq!(Step2Str(ImportInto, 123), "unknown step 123");

    assert_eq!(Step2Str(TaskTypeExample, StepInit), "init");
    assert_eq!(Step2Str(TaskTypeExample, StepOne), "one");
    assert_eq!(Step2Str(TaskTypeExample, StepTwo), "two");
    assert_eq!(Step2Str(TaskTypeExample, StepPrepared), "prepared");
    assert_eq!(Step2Str(TaskTypeExample, StepDone), "done");
    assert_eq!(Step2Str(TaskTypeExample, 333), "unknown step 333");
    // 未知任务类型走 "unknown type" 分支，与 Go 一致。
    assert_eq!(Step2Str("123", 123), "unknown type 123");
}

/// 校验合法 step 与业务 step：框架标记 step 不算业务 step。
#[test]
fn test_is_valid_step() {
    assert!(IsValidStep(Backfill, BackfillStepReadIndex));
    assert!(!IsValidStep(Backfill, 123));
    assert!(IsValidStep(Backfill, StepPrepared));
    assert!(IsValidStep(ImportInto, ImportStepWriteAndIngest));
    assert!(!IsValidStep(ImportInto, 456));

    // 业务 step 有效；StepInit/Done/Prepared 虽可 Step2Str，但不是业务 step。
    assert!(IsValidBusinessStep(Backfill, BackfillStepReadIndex));
    assert!(!IsValidBusinessStep(Backfill, StepInit));
    assert!(!IsValidBusinessStep(Backfill, StepDone));
    assert!(!IsValidBusinessStep(Backfill, StepPrepared));
    assert!(!IsValidBusinessStep(Backfill, 123));
}
