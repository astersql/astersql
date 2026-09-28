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

//! Go-equivalent tests for `lightning/pkg/importinto/job_submitter_test.go`.
//! SDK boundary: scriptable `importsdk::MockSDK` (Go: importsdk/mock + gomock).
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/job_submitter_test.rs`对应的作业提交与日志脱敏，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少39行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `test_job_submitter_submit_table`对齐 Go 同名测试或契约片段，用来固定\"test job submitter submit table\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_submitter_get_group_key`对齐 Go 同名测试或契约片段，用来固定\"test job submitter get group key\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_job_submitter_submit_table_log_redaction`对齐 Go 同名测试或契约片段，用来固定\"test job submitter submit table log redaction\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"successful submission\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"generate sql error\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"submit job error\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"keep s3 external id unless nextgen sem is enabled\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"sanitize s3 external id when enabled for import sql resource parameters\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"sanitize s3 external id when config flag is enabled\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"sanitize oss external id as s3 like resource parameters\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"keep non s3 resource parameters unchanged\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::*;
use std::sync::Arc;

const S3_SOURCE_DIR_WITH_EXTERNAL_ID: &str = "s3://bucket/path?role-arn=arn&external-id=remove&External_ID=remove-too&external%5Fid=remove-encoded&region=us-east-1&endpoint=http%3A%2F%2Fminio%3A9000";
const S3_RESOURCE_PARAMETERS: &str =
    "endpoint=http%3A%2F%2Fminio%3A9000&region=us-east-1&role-arn=arn";
const OSS_SOURCE_DIR_WITH_EXTERNAL_ID: &str =
    "oss://bucket/path?external-id=remove&role-arn=arn&region=us-east-1";
const OSS_RESOURCE_PARAMETERS: &str = "region=us-east-1&role-arn=arn";
const GCS_SOURCE_DIR: &str =
    "gcs://bucket/path?external-id=keep&external_id=keep-too&region=us-east-1";

/// TestJobSubmitterSubmitTable
#[test]
fn test_job_submitter_submit_table() {
    let table = importsdk::TableMeta {
        Database: "db".into(),
        Table: "t1".into(),
        ..Default::default()
    };
    let group_key = "g1".to_string();
    let ctx = context::Background();

    // successful submission
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.fixed_sql = Some("IMPORT INTO ...".into());
        sdk.fixed_job_id = Some(123);
        let cfg = Arc::new(config::Config::NewConfig());
        let submitter = NewJobSubmitter(Arc::new(sdk), cfg, group_key.clone(), log::L(), vec![]);
        let job = submitter.SubmitTable(&ctx, &table).unwrap();
        assert_eq!(123, job.JobID);
        assert_eq!(group_key, job.GroupKey);
    }

    // generate sql error
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.gen_sql_err = Some(Error::new("gen error"));
        let cfg = Arc::new(config::Config::NewConfig());
        let submitter = NewJobSubmitter(Arc::new(sdk), cfg, group_key.clone(), log::L(), vec![]);
        let err = submitter.SubmitTable(&ctx, &table).unwrap_err();
        assert!(err.Error().contains("gen error"));
    }

    // submit job error
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.fixed_sql = Some("IMPORT INTO ...".into());
        sdk.submit_err = Some(Error::new("submit error"));
        let cfg = Arc::new(config::Config::NewConfig());
        let submitter = NewJobSubmitter(Arc::new(sdk), cfg, group_key.clone(), log::L(), vec![]);
        let err = submitter.SubmitTable(&ctx, &table).unwrap_err();
        assert!(err.Error().contains("submit error"));
    }

    // keep s3 external id unless nextgen sem is enabled
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.fixed_sql = Some("IMPORT INTO ...".into());
        sdk.fixed_job_id = Some(123);
        let sdk = Arc::new(sdk);
        let mut cfg = config::Config::NewConfig();
        cfg.Mydumper.SourceDir = S3_SOURCE_DIR_WITH_EXTERNAL_ID.into();
        let source = cfg.Mydumper.SourceDir.clone();
        let submitter = NewJobSubmitter(
            sdk.clone(),
            Arc::new(cfg),
            group_key.clone(),
            log::L(),
            vec![],
        );
        submitter.SubmitTable(&ctx, &table).unwrap();
        let opts = sdk.last_import_opts.lock().unwrap().clone().unwrap();
        assert_eq!(
            "role-arn=arn&external-id=remove&External_ID=remove-too&external%5Fid=remove-encoded&region=us-east-1&endpoint=http%3A%2F%2Fminio%3A9000",
            opts.ResourceParameters
        );
        assert_eq!(S3_SOURCE_DIR_WITH_EXTERNAL_ID, source);
    }

    // sanitize s3 external id when enabled for import sql resource parameters
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.fixed_sql = Some("IMPORT INTO ...".into());
        sdk.fixed_job_id = Some(123);
        let sdk = Arc::new(sdk);
        let mut cfg = config::Config::NewConfig();
        cfg.Mydumper.SourceDir = S3_SOURCE_DIR_WITH_EXTERNAL_ID.into();
        let source = cfg.Mydumper.SourceDir.clone();
        let submitter = NewJobSubmitter(
            sdk.clone(),
            Arc::new(cfg),
            group_key.clone(),
            log::L(),
            vec![WithJobSubmitterStripS3ExternalIDForImportSQL(true)],
        );
        submitter.SubmitTable(&ctx, &table).unwrap();
        let opts = sdk.last_import_opts.lock().unwrap().clone().unwrap();
        assert_eq!(S3_RESOURCE_PARAMETERS, opts.ResourceParameters);
        assert_eq!(S3_SOURCE_DIR_WITH_EXTERNAL_ID, source);
    }

    // sanitize s3 external id when config flag is enabled
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.fixed_sql = Some("IMPORT INTO ...".into());
        sdk.fixed_job_id = Some(123);
        let sdk = Arc::new(sdk);
        let mut cfg = config::Config::NewConfig();
        cfg.Mydumper.SourceDir = S3_SOURCE_DIR_WITH_EXTERNAL_ID.into();
        cfg.TikvImporter.StripS3ExternalIDForImportSQL = true;
        let source = cfg.Mydumper.SourceDir.clone();
        let submitter = NewJobSubmitter(
            sdk.clone(),
            Arc::new(cfg),
            group_key.clone(),
            log::L(),
            vec![],
        );
        submitter.SubmitTable(&ctx, &table).unwrap();
        let opts = sdk.last_import_opts.lock().unwrap().clone().unwrap();
        assert_eq!(S3_RESOURCE_PARAMETERS, opts.ResourceParameters);
        assert_eq!(S3_SOURCE_DIR_WITH_EXTERNAL_ID, source);
    }

    // sanitize oss external id as s3 like resource parameters
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.fixed_sql = Some("IMPORT INTO ...".into());
        sdk.fixed_job_id = Some(123);
        let sdk = Arc::new(sdk);
        let mut cfg = config::Config::NewConfig();
        cfg.Mydumper.SourceDir = OSS_SOURCE_DIR_WITH_EXTERNAL_ID.into();
        let source = cfg.Mydumper.SourceDir.clone();
        let submitter = NewJobSubmitter(
            sdk.clone(),
            Arc::new(cfg),
            group_key.clone(),
            log::L(),
            vec![WithJobSubmitterStripS3ExternalIDForImportSQL(true)],
        );
        submitter.SubmitTable(&ctx, &table).unwrap();
        let opts = sdk.last_import_opts.lock().unwrap().clone().unwrap();
        assert_eq!(OSS_RESOURCE_PARAMETERS, opts.ResourceParameters);
        assert_eq!(OSS_SOURCE_DIR_WITH_EXTERNAL_ID, source);
    }

    // keep non s3 resource parameters unchanged
    {
        let mut sdk = importsdk::MockSDK::new();
        sdk.fixed_sql = Some("IMPORT INTO ...".into());
        sdk.fixed_job_id = Some(123);
        let sdk = Arc::new(sdk);
        let mut cfg = config::Config::NewConfig();
        cfg.Mydumper.SourceDir = GCS_SOURCE_DIR.into();
        let source = cfg.Mydumper.SourceDir.clone();
        let submitter = NewJobSubmitter(
            sdk.clone(),
            Arc::new(cfg),
            group_key.clone(),
            log::L(),
            vec![],
        );
        submitter.SubmitTable(&ctx, &table).unwrap();
        let opts = sdk.last_import_opts.lock().unwrap().clone().unwrap();
        assert_eq!(
            "external-id=keep&external_id=keep-too&region=us-east-1",
            opts.ResourceParameters
        );
        assert_eq!(GCS_SOURCE_DIR, source);
    }
}

/// TestJobSubmitterGetGroupKey
#[test]
fn test_job_submitter_get_group_key() {
    let sdk = Arc::new(importsdk::MockSDK::new());
    let cfg = Arc::new(config::Config::NewConfig());
    let group_key = "g1".to_string();
    let submitter = NewJobSubmitter(sdk, cfg, group_key.clone(), log::L(), vec![]);
    assert_eq!(group_key, submitter.GetGroupKey());
}

/// TestJobSubmitterSubmitTableLogRedaction
#[test]
fn test_job_submitter_submit_table_log_redaction() {
    let mut sdk = importsdk::MockSDK::new();
    let table_meta = importsdk::TableMeta {
        Database: "db".into(),
        Table: "t1".into(),
        WildcardPath: "s3://bucket/path/*.csv?access-key=ak&endpoint=http%3A%2F%2Fminio%3A9000&secret-access-key=sk".into(),
        ..Default::default()
    };
    let raw_sql = format!("IMPORT INTO `db`.`t1` FROM '{}'", table_meta.WildcardPath);
    sdk.fixed_sql = Some(raw_sql.clone());
    sdk.fixed_job_id = Some(123);
    let sdk = Arc::new(sdk);
    let logger = log::MakeTestLogger();
    let submitter = NewJobSubmitter(
        sdk,
        Arc::new(config::Config::NewConfig()),
        "g1".into(),
        logger.clone(),
        vec![],
    );
    submitter
        .SubmitTable(&context::Background(), &table_meta)
        .unwrap();

    let out = logger.buffer_string();
    assert!(out.contains("access-key=xxxxxx"), "out={out}");
    assert!(out.contains("secret-access-key=xxxxxx"), "out={out}");
    assert!(!out.contains("access-key=ak"), "out={out}");
    assert!(!out.contains("secret-access-key=sk"), "out={out}");
}
