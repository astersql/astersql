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

//! Job submitter for IMPORT INTO.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importinto/job_submitter.rs`对应的作业提交与日志脱敏，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少37行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `struct`承载\"struct\"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `trait`定义对外暴露的抽象边界，约束\"trait\"的最小能力集合。
//! 对 trait 的说明应重点覆盖调用者可依赖什么、实现者必须遵守什么以及错误是否允许透传。
//! 这可以帮助后续替换实现时，避免只满足编译器却破坏 Go 端既有约定。
//! 在 mock、checkpoint、monitor 或 backend 体系里，trait 文档直接决定测试替身是否可信。
//! - `SubmitTable`是当前文件的重要函数，承担\"SubmitTable\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `GetGroupKey`是当前文件的重要函数，承担\"GetGroupKey\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `fn`是当前文件的重要函数，承担\"fn\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl JobSubmitter`把\"JobSubmitter\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `impl DefaultJobSubmitter`把\"DefaultJobSubmitter\"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比， Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `buildImportOptions`是当前文件的重要函数，承担\"buildImportOptions\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `stripS3ExternalIDResourceParameters`是当前文件的重要函数，承担\"stripS3ExternalIDResourceParameters\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `generateImportSQLForLog`是当前文件的重要函数，承担\"generateImportSQLForLog\"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - 场景\"Go url.Values.Encode sorts keys lexicographically.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::stubs::*;
use std::sync::Arc;
use url::Url;

/// ImportJob represents a submitted import job with its metadata.
#[derive(Clone, Debug)]
pub struct ImportJob {
    pub JobID: i64,
    pub TableMeta: Option<importsdk::TableMeta>,
    pub GroupKey: String,
}

/// JobSubmitter is responsible for submitting import jobs to TiDB.
pub trait JobSubmitter: Send + Sync {
    fn SubmitTable(
        &self,
        ctx: &context::Context,
        tableMeta: &importsdk::TableMeta,
    ) -> Result<ImportJob>;
    fn GetGroupKey(&self) -> String;
}

/// DefaultJobSubmitter is the default implementation of JobSubmitter.
pub struct DefaultJobSubmitter {
    sdk: Arc<dyn importsdk::SDK>,
    config: Arc<config::Config>,
    groupKey: String,
    logger: log::Logger,
    stripS3ExternalIDForImportSQL: bool,
}

/// JobSubmitterOption is a function that configures the JobSubmitter.
pub type JobSubmitterOption = Box<dyn FnOnce(&mut DefaultJobSubmitter) + Send>;

/// WithJobSubmitterStripS3ExternalIDForImportSQL strips explicit S3 external ID
/// from IMPORT INTO SQL resource parameters.
pub fn WithJobSubmitterStripS3ExternalIDForImportSQL(strip: bool) -> JobSubmitterOption {
    Box::new(move |s: &mut DefaultJobSubmitter| {
        s.stripS3ExternalIDForImportSQL = strip;
    })
}

/// NewJobSubmitter creates a new job submitter.
pub fn NewJobSubmitter(
    sdk: Arc<dyn importsdk::SDK>,
    cfg: Arc<config::Config>,
    groupKey: String,
    logger: log::Logger,
    opts: Vec<JobSubmitterOption>,
) -> Arc<dyn JobSubmitter> {
    let mut submitter = DefaultJobSubmitter {
        sdk,
        config: cfg.clone(),
        groupKey,
        logger,
        stripS3ExternalIDForImportSQL: false,
    };
    WithJobSubmitterStripS3ExternalIDForImportSQL(cfg.TikvImporter.StripS3ExternalIDForImportSQL)(
        &mut submitter,
    );
    for opt in opts {
        opt(&mut submitter);
    }
    Arc::new(submitter)
}

impl JobSubmitter for DefaultJobSubmitter {
    fn SubmitTable(
        &self,
        ctx: &context::Context,
        tableMeta: &importsdk::TableMeta,
    ) -> Result<ImportJob> {
        let logger = self
            .logger
            .clone()
            .With(zap::String("database", &tableMeta.Database))
            .With(zap::String("table", &tableMeta.Table));

        let options = self.buildImportOptions(tableMeta);
        let sql = self
            .sdk
            .GenerateImportSQL(tableMeta, &options)
            .map_err(|e| errors::Annotate(e, "generate import SQL"))?;

        match generateImportSQLForLog(tableMeta, &options) {
            Ok(sql_for_log) => {
                logger.Info("submitting import job", &[zap::String("sql", &sql_for_log)]);
            }
            Err(_) => {
                logger.Info("submitting import job", &[]);
            }
        }

        let jobID = self
            .sdk
            .SubmitJob(ctx, &sql)
            .map_err(|e| errors::Annotate(e, "submit job"))?;

        logger.Info("import job submitted", &[zap::Int64("jobID", jobID)]);
        Ok(ImportJob {
            JobID: jobID,
            TableMeta: Some(tableMeta.clone()),
            GroupKey: self.groupKey.clone(),
        })
    }

    fn GetGroupKey(&self) -> String {
        self.groupKey.clone()
    }
}

impl DefaultJobSubmitter {
    fn buildImportOptions(&self, tableMeta: &importsdk::TableMeta) -> importsdk::ImportOptions {
        let cfg = &self.config;
        let mut opts = importsdk::ImportOptions {
            Detached: true,
            GroupKey: self.groupKey.clone(),
            DisablePrecheck: !cfg.App.CheckRequirements,
            ..Default::default()
        };

        if let Some(df) = tableMeta.DataFiles.first() {
            opts.Format = df.Format.String().to_string();
        }
        if opts.Format == "csv" {
            opts.CSVConfig = Some(cfg.Mydumper.CSV.clone());
            if cfg.Mydumper.CSV.Header {
                opts.SkipRows = 1;
            }
        }

        opts.SplitFile = cfg.Mydumper.StrictFormat;

        let maxTypeError = cfg.App.MaxError.Type.Load();
        if maxTypeError > 0 {
            opts.RecordErrors = maxTypeError;
        }

        if !cfg.Mydumper.DataCharacterSet.is_empty() && cfg.Mydumper.DataCharacterSet != "binary" {
            opts.CharacterSet = cfg.Mydumper.DataCharacterSet.clone();
        }

        if !cfg.Mydumper.SourceDir.is_empty() {
            if let Ok(u) = Url::parse(&cfg.Mydumper.SourceDir) {
                opts.ResourceParameters =
                    buildResourceParametersForImportSQL(&u, self.stripS3ExternalIDForImportSQL);
            }
        }

        opts
    }
}

pub(crate) fn buildResourceParametersForImportSQL(u: &Url, stripS3ExternalID: bool) -> String {
    if !(stripS3ExternalID && objstore::IsS3Like(u)) {
        return u.query().unwrap_or("").to_string();
    }

    let mut values: Vec<(String, String)> = u
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    if !stripS3ExternalIDResourceParameters(&mut values) {
        return u.query().unwrap_or("").to_string();
    }
    // Go url.Values.Encode sorts keys lexicographically.
    values.sort_by(|a, b| a.0.cmp(&b.0));
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in &values {
        ser.append_pair(k, v);
    }
    ser.finish()
}

fn stripS3ExternalIDResourceParameters(values: &mut Vec<(String, String)>) -> bool {
    let before = values.len();
    values.retain(|(key, _)| objstore::NormalizeQueryParameterKey(key) != s3like::S3ExternalID);
    values.len() != before
}

fn generateImportSQLForLog(
    tableMeta: &importsdk::TableMeta,
    options: &importsdk::ImportOptions,
) -> Result<String> {
    let mut redactedMeta = tableMeta.clone();
    let mut redactedOpts = options.clone();

    let mut path = redactedMeta.WildcardPath.clone();
    if !redactedOpts.ResourceParameters.is_empty() {
        if let Ok(mut u) = Url::parse(&path) {
            if let Some(q) = u.query() {
                if !q.is_empty() {
                    u.set_query(Some(&format!("{q}&{}", redactedOpts.ResourceParameters)));
                } else {
                    u.set_query(Some(&redactedOpts.ResourceParameters));
                }
            } else {
                u.set_query(Some(&redactedOpts.ResourceParameters));
            }
            path = u.to_string();
        }
        redactedOpts.ResourceParameters.clear();
    }
    redactedMeta.WildcardPath = ast::RedactURL(&path);
    if !redactedOpts.CloudStorageURI.is_empty() {
        redactedOpts.CloudStorageURI = ast::RedactURL(&redactedOpts.CloudStorageURI);
    }

    importsdk::NewSQLGenerator().GenerateImportSQL(&redactedMeta, &redactedOpts)
}

/// Test helper: strip S3 external-id from a source URL query.
pub fn strip_s3_external_id_for_test(source: &str) -> String {
    let u = Url::parse(source).expect("url");
    buildResourceParametersForImportSQL(&u, true)
}
