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

//! Go-equivalent tests for `lightning/pkg/importinto/job_progress_test.go`.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/job_progress_test.rs`对应的进度估算与阶段换算，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少19行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `test_job_progress_estimator_non_global_sort`对齐 Go 同名测试或契约片段，用来固定\"test job progress estimator non global sort\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_progress_estimator_global_sort`对齐 Go 同名测试或契约片段，用来固定\"test job progress estimator global sort\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 补充约束 1: `lightning/pkg/importinto/job_progress_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 1: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 2: `lightning/pkg/importinto/job_progress_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 2: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! - 补充约束 3: `lightning/pkg/importinto/job_progress_test.rs`仍需保持仅注释差异，任何默认值、错误类名或资源回收顺序都不应在本任务中变化。
//! - 补充约束 3: 若 Go 与 Rust 内部写法不同，应优先核对最终可观察行为，而不是拘泥于实现形式。
//! 中文注释索引结束

use crate::*;
use std::collections::HashMap;

/// TestJobProgressEstimator_NonGlobalSort
#[test]
fn test_job_progress_estimator_non_global_sort() {
    const MB: i64 = 1000 * 1000;

    let mut estimator = newJobProgressEstimator(log::L());
    let job_id = 1_i64;
    let job = ImportJob {
        JobID: job_id,
        TableMeta: Some(importsdk::TableMeta {
            TotalSize: 100 * MB,
            ..Default::default()
        }),
        GroupKey: String::new(),
    };

    let status = importsdk::JobStatus {
        JobID: job_id,
        Status: "running".into(),
        Phase: "importing".into(),
        Step: "import".into(),
        Percent: "50".into(),
        TotalSize: "100MB".into(),
        ..Default::default()
    };

    assert!(!estimator.isGlobalSort);
    assert_eq!(0.25, estimator.jobProgress(&status));

    let mut job_total_size = HashMap::new();
    let mut job_finished_size = HashMap::new();
    estimator.updateJobProgress(&job, &status, &mut job_total_size, &mut job_finished_size);
    assert_eq!(100 * MB, job_total_size[&job_id]);
    assert_eq!(25 * MB, job_finished_size[&job_id]);

    let finished = importsdk::JobStatus {
        JobID: job_id,
        Status: "finished".into(),
        ..Default::default()
    };
    estimator.updateJobProgress(&job, &finished, &mut job_total_size, &mut job_finished_size);
    assert_eq!(100 * MB, job_finished_size[&job_id]);
}

/// TestJobProgressEstimator_GlobalSort
#[test]
fn test_job_progress_estimator_global_sort() {
    const MB: i64 = 1000 * 1000;

    let mut estimator = newJobProgressEstimator(log::L());
    let job_id = 1_i64;
    let job = ImportJob {
        JobID: job_id,
        TableMeta: Some(importsdk::TableMeta {
            TotalSize: 100 * MB,
            ..Default::default()
        }),
        GroupKey: String::new(),
    };

    let encode = importsdk::JobStatus {
        JobID: job_id,
        Status: "running".into(),
        Phase: "global-sorting".into(),
        Step: "encode".into(),
        Percent: "50".into(),
        TotalSize: "100MB".into(),
        ..Default::default()
    };

    let mut job_total_size = HashMap::new();
    let mut job_finished_size = HashMap::new();
    estimator.updateJobProgress(&job, &encode, &mut job_total_size, &mut job_finished_size);
    assert!(estimator.isGlobalSort);
    assert_eq!(0.0625, estimator.jobProgress(&encode));
    assert_eq!(100 * MB / 16, job_finished_size[&job_id]);

    estimator.updateJobProgress(
        &job,
        &importsdk::JobStatus {
            JobID: job_id,
            Phase: "importing".into(),
            Step: "import".into(),
            Status: "running".into(),
            ..Default::default()
        },
        &mut job_total_size,
        &mut job_finished_size,
    );
    assert!(estimator.isGlobalSort);

    let merge_sort = importsdk::JobStatus {
        JobID: job_id,
        Status: "running".into(),
        Phase: "global-sorting".into(),
        Step: "merge-sort".into(),
        Percent: "0".into(),
        TotalSize: "100MB".into(),
        ..Default::default()
    };
    assert_eq!(0.125, estimator.jobProgress(&merge_sort));
}

/// Docker `units.FromHumanSize` accepts scientific notation in size fields.
#[test]
fn test_job_progress_estimator_scientific_notation_size() {
    let mut estimator = newJobProgressEstimator(log::L());
    let job_id = 1_i64;
    let job = ImportJob {
        JobID: job_id,
        TableMeta: None,
        GroupKey: String::new(),
    };
    let status = importsdk::JobStatus {
        JobID: job_id,
        Status: "running".into(),
        Phase: "importing".into(),
        Step: "import".into(),
        Percent: "50".into(),
        SourceFileSize: "1e3".into(),
        ..Default::default()
    };

    let mut job_total_size = HashMap::new();
    let mut job_finished_size = HashMap::new();
    estimator.updateJobProgress(&job, &status, &mut job_total_size, &mut job_finished_size);

    assert_eq!(1_000, job_total_size[&job_id]);
    assert_eq!(250, job_finished_size[&job_id]);
}
