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

// 导入 SDK 门面模块。
//
// 将文件扫描（FileScanner）、导入作业管理（JobManager）与 IMPORT INTO SQL 生成
// （SQLGenerator）组合为统一的 `ImportSDK`，对应 Go 包 `importsdk` 的对外入口。

use crate::{
    FileScanner, GroupStatus, ImportDataSizeEstimate, ImportOptions, JobDatabase, JobManager,
    JobManagerImpl, JobStatus, NewFileScanner, NewJobManager, NewSQLGenerator, SDKOption,
    SQLGenerator, TableMeta, defaultSDKConfig,
};
use astersql_errors as errors;
use std::any::Any;
use std::sync::Arc;

/// 导入 SDK 统一接口：同时具备扫描、作业与 SQL 生成能力，并支持关闭资源。
pub trait SDK: FileScanner + JobManager + SQLGenerator {
    /// 关闭底层扫描器等资源；对应 Go `SDK.Close`。
    fn Close(&mut self) -> Result<(), errors::SharedError>;
}

/// `ImportSDK` 具体实现：分别持有扫描器、作业管理器与 SQL 生成器。
pub struct ImportSDK {
    /// 数据源文件扫描与库表元数据发现。
    file_scanner: Box<dyn FileScanner>,
    /// 导入作业的提交、查询与取消。
    job_manager: JobManagerImpl,
    /// 根据表元数据与选项生成 `IMPORT INTO` SQL。
    sql_generator: Box<dyn SQLGenerator + Send + Sync>,
}

/// 构造 `ImportSDK`：应用选项覆盖默认配置，再创建扫描器、作业管理器与 SQL 生成器。
pub fn NewImportSDK(
    ctx: &(dyn Any + Send + Sync),
    source_path: &str,
    db: Arc<dyn JobDatabase>,
    options: Vec<SDKOption>,
) -> Result<ImportSDK, errors::SharedError> {
    // 先取默认配置，再依次应用调用方传入的 SDKOption。
    let mut config = defaultSDKConfig();
    for option in options {
        option(&mut config);
    }
    let file_scanner = NewFileScanner(ctx, source_path, Arc::clone(&db), config)?;
    Ok(ImportSDK {
        file_scanner,
        job_manager: NewJobManager(db),
        sql_generator: NewSQLGenerator(),
    })
}

// 文件扫描相关方法全部委托给内部 `file_scanner`。
impl FileScanner for ImportSDK {
    fn CreateSchemasAndTables(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> Result<(), errors::SharedError> {
        self.file_scanner.CreateSchemasAndTables(ctx)
    }

    fn CreateSchemaAndTableByName(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
        schema: &str,
        table: &str,
    ) -> Result<(), errors::SharedError> {
        self.file_scanner
            .CreateSchemaAndTableByName(ctx, schema, table)
    }

    fn GetTableMetas(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> Result<Vec<TableMeta>, errors::SharedError> {
        self.file_scanner.GetTableMetas(ctx)
    }

    fn GetTableMetasParts(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> (Option<Vec<TableMeta>>, Option<errors::SharedError>) {
        self.file_scanner.GetTableMetasParts(ctx)
    }

    fn GetTableMetaByName(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
        database: &str,
        table: &str,
    ) -> Result<TableMeta, errors::SharedError> {
        self.file_scanner.GetTableMetaByName(ctx, database, table)
    }

    fn GetTableMetaByNameParts(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
        database: &str,
        table: &str,
    ) -> (Option<TableMeta>, Option<errors::SharedError>) {
        self.file_scanner
            .GetTableMetaByNameParts(ctx, database, table)
    }

    fn GetTotalSize(&self, ctx: &(dyn Any + Send + Sync)) -> i64 {
        self.file_scanner.GetTotalSize(ctx)
    }

    fn EstimateImportDataSize(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> Result<ImportDataSizeEstimate, errors::SharedError> {
        self.file_scanner.EstimateImportDataSize(ctx)
    }

    fn EstimateImportDataSizeParts(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> (Option<ImportDataSizeEstimate>, Option<errors::SharedError>) {
        self.file_scanner.EstimateImportDataSizeParts(ctx)
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        self.file_scanner.Close()
    }
}

// 作业管理相关方法全部委托给内部 `job_manager`。
impl JobManager for ImportSDK {
    fn SubmitJob(
        &self,
        ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> Result<i64, errors::SharedError> {
        self.job_manager.SubmitJob(ctx, query)
    }

    fn SubmitJobParts(
        &self,
        ctx: &(dyn Any + Send + Sync),
        query: &str,
    ) -> (i64, Option<errors::SharedError>) {
        self.job_manager.SubmitJobParts(ctx, query)
    }

    fn GetJobStatus(
        &self,
        ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<JobStatus, errors::SharedError> {
        self.job_manager.GetJobStatus(ctx, job_id)
    }

    fn GetJobStatusParts(
        &self,
        ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> (Option<JobStatus>, Option<errors::SharedError>) {
        self.job_manager.GetJobStatusParts(ctx, job_id)
    }

    fn CancelJob(
        &self,
        ctx: &(dyn Any + Send + Sync),
        job_id: i64,
    ) -> Result<(), errors::SharedError> {
        self.job_manager.CancelJob(ctx, job_id)
    }

    fn GetGroupSummary(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<GroupStatus, errors::SharedError> {
        self.job_manager.GetGroupSummary(ctx, group_key)
    }

    fn GetGroupSummaryParts(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> (Option<GroupStatus>, Option<errors::SharedError>) {
        self.job_manager.GetGroupSummaryParts(ctx, group_key)
    }

    fn GetJobsByGroup(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> Result<Vec<JobStatus>, errors::SharedError> {
        self.job_manager.GetJobsByGroup(ctx, group_key)
    }

    fn GetJobsByGroupParts(
        &self,
        ctx: &(dyn Any + Send + Sync),
        group_key: &str,
    ) -> (Option<Vec<JobStatus>>, Option<errors::SharedError>) {
        self.job_manager.GetJobsByGroupParts(ctx, group_key)
    }
}

// SQL 生成委托给内部 `sql_generator`。
impl SQLGenerator for ImportSDK {
    fn GenerateImportSQL(
        &self,
        table_meta: &TableMeta,
        options: &ImportOptions,
    ) -> Result<String, errors::SharedError> {
        self.sql_generator.GenerateImportSQL(table_meta, options)
    }

    fn GenerateImportSQLParts(
        &self,
        table_meta: &TableMeta,
        options: &ImportOptions,
    ) -> (String, Option<errors::SharedError>) {
        self.sql_generator
            .GenerateImportSQLParts(table_meta, options)
    }
}

// SDK::Close 走 FileScanner::Close，避免与 JobManager 等同名方法混淆。
impl SDK for ImportSDK {
    fn Close(&mut self) -> Result<(), errors::SharedError> {
        FileScanner::Close(self)
    }
}
