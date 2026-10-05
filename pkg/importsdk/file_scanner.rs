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

// 导入数据源文件扫描器：解析存储 URL、加载 MyDump 元数据并建库建表。
//
// [`FileScanner`] 对外提供创建 schema/表、列举 [`TableMeta`]、汇总源文件大小、
// 估算导入后 TiKV（分布式 KV 存储）占用等能力。实现侧通过 objstore 打开外部存储，
// 用 mydump loader 扫描 dump，再经 schema importer 在目标库执行 DDL。

use astersql_ddl as ddl;
use astersql_errors as errors;
use astersql_executor_importer as execimporter;
use astersql_lightning_log as log;
use astersql_lightning_mydump as mydump;
use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_objstore as objstore;
use astersql_parser as parser;
use astersql_parser_ast as ast;
use std::any::Any;
use std::collections::HashMap;
use std::convert::Infallible;
use std::io::Read;
use std::sync::Arc;
use url::Url;

use crate::pattern::generateWildcardPath;
use crate::{
    DataFileMeta, ErrCreateExternalStorage, ErrCreateLoader, ErrCreateSchema, ErrNoDatabasesFound,
    ErrParseStorageURL, ErrSchemaNotFound, ErrTableNotFound, ImportDataSizeEstimate, JobDatabase,
    SDKConfig, SQLValue, TableDataSizeEstimate, TableMeta,
};

/// 估算采样时注入的重要系统变量默认值（与 Lightning/IMPORT 行为对齐）。
const IMPORTANT_VARIABLE_DEFAULTS: &[(&str, &str)] = &[
    ("max_allowed_packet", "67108864"),
    ("div_precision_increment", "4"),
    ("time_zone", "SYSTEM"),
    ("lc_time_names", "en_US"),
    ("default_week_format", "0"),
    ("block_encryption_mode", "aes-128-ecb"),
    ("group_concat_max_len", "1024"),
    ("tidb_backoff_weight", "6"),
];
/// 导入路径额外的系统变量默认值（行格式版本等）。
const IMPORT_VARIABLE_DEFAULTS: &[(&str, &str)] = &[("tidb_row_format_version", "1")];

/// 扫描 dump 来源并驱动建库建表、元数据查询与大小估算的对外接口。
pub trait FileScanner: Send + Sync {
    /// 为 loader 发现的全部库表在目标集群创建 schema 与表。
    fn CreateSchemasAndTables(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> Result<(), errors::SharedError>;
    /// 仅按库名/表名创建单个 schema 与表。
    fn CreateSchemaAndTableByName(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
        schema: &str,
        table: &str,
    ) -> Result<(), errors::SharedError>;
    /// 返回所有可识别表的元数据（含数据文件与通配路径）。
    fn GetTableMetas(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> Result<Vec<TableMeta>, errors::SharedError>;
    fn GetTableMetasParts(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> (Option<Vec<TableMeta>>, Option<errors::SharedError>) {
        match self.GetTableMetas(ctx) {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error)),
        }
    }
    /// 按库名/表名返回单表元数据。
    fn GetTableMetaByName(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
        db: &str,
        table: &str,
    ) -> Result<TableMeta, errors::SharedError>;
    fn GetTableMetaByNameParts(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
        db: &str,
        table: &str,
    ) -> (Option<TableMeta>, Option<errors::SharedError>) {
        match self.GetTableMetaByName(ctx, db, table) {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error)),
        }
    }
    /// 汇总所有表数据文件的源端总大小（字节）。
    fn GetTotalSize(&self, ctx: &(dyn Any + Send + Sync)) -> i64;
    /// 估算导入后各表在 TiKV 上的占用及源端大小合计。
    fn EstimateImportDataSize(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> Result<ImportDataSizeEstimate, errors::SharedError>;
    fn EstimateImportDataSizeParts(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> (Option<ImportDataSizeEstimate>, Option<errors::SharedError>) {
        match self.EstimateImportDataSize(ctx) {
            Ok(value) => (Some(value), None),
            Err(error) => (None, Some(error)),
        }
    }
    /// 关闭底层外部存储客户端。
    fn Close(&mut self) -> Result<(), errors::SharedError>;
}

/// [`FileScanner`] 的具体实现：持有脱敏源路径、目标库、存储、loader 与配置。
pub struct fileScanner {
    /// 脱敏后的源路径（日志与错误 annotate 用）。
    redacted_source_path: String,
    /// 目标集群执行 DDL/查询的数据库句柄。
    db: Arc<dyn JobDatabase>,
    /// 外部对象存储；Close 后为 None。
    store: Option<objstore::storage::StorageRef>,
    /// MyDump 元数据加载器。
    loader: mydump::MDLoader,
    logger: log::Logger,
    config: SDKConfig,
    aurora_source: bool,
}

/// 无法解析且无法脱敏时使用的占位源路径文案。
const redactedInvalidSourcePath: &str = "<redacted-invalid-source>";

/// 在共享哨兵错误上附加上下文字符串。
fn annotate(base: &errors::SharedError, message: impl Into<String>) -> errors::SharedError {
    errors::Annotate(Some(base.clone()), message)
        .expect("annotating an existing error cannot return None")
}

/// 脱敏存储 URL 中的密钥查询参数（access-key、sas-token 等替换为 xxxxxx）。
fn redactURL(source: &str) -> String {
    let Ok(mut parsed) = Url::parse(source) else {
        return source.to_owned();
    };
    // 按协议选择需要遮蔽的查询键名。
    let secret_keys: &[&str] = match parsed.scheme().to_ascii_lowercase().as_str() {
        "s3" | "ks3" | "oss" => &["access-key", "secret-access-key", "session-token"],
        "azure" | "azblob" => &["account-key", "encryption-key", "sas-token"],
        _ => return source.to_owned(),
    };
    let mut pairs = parsed
        .query_pairs()
        .map(|(key, value)| {
            let normalized = key.to_ascii_lowercase().replace('_', "-");
            let value = if secret_keys.contains(&normalized.as_str()) {
                "xxxxxx".to_owned()
            } else {
                value.into_owned()
            };
            (key.into_owned(), value)
        })
        .collect::<Vec<_>>();
    // 排序保证脱敏后 URL 稳定可比较。
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    parsed.query_pairs_mut().clear().extend_pairs(pairs);
    parsed.to_string()
}

/// 将 objstore Storage 适配为 mydump::Storage。
#[derive(Clone)]
struct LoaderStorage {
    inner: objstore::storage::StorageRef,
}

/// 把 mydump 整文件压缩类型映射到 objstore 的 CompressType。
fn compressionType(
    compression: mydump::Compression,
) -> Result<objstore::objectio::CompressType, mydump::MydumpError> {
    match compression {
        mydump::Compression::None => Ok(objstore::objectio::CompressType::NoCompression),
        mydump::Compression::Gz => Ok(objstore::objectio::CompressType::Gzip),
        mydump::Compression::Snappy => Ok(objstore::objectio::CompressType::Snappy),
        mydump::Compression::Zstd => Ok(objstore::objectio::CompressType::Zstd),
        unsupported => Err(mydump::MydumpError::Configuration(format!(
            "unsupported whole-file compression {unsupported:?}"
        ))),
    }
}

impl mydump::Storage for LoaderStorage {
    fn open(
        &self,
        path: &str,
        compression: mydump::Compression,
    ) -> Result<Box<dyn Read + Send>, mydump::MydumpError> {
        let context = objstore::storage::Context::background();
        let raw: Box<dyn Read + Send> = self
            .inner
            .Open(&context, path, None)
            .map_err(|error| mydump::MydumpError::Io(error.to_string()))?;
        let compression = compressionType(compression)?;
        if compression == objstore::objectio::CompressType::NoCompression {
            return Ok(raw);
        }
        // 压缩文件包装解压 Reader，供 loader 读取 schema/数据内容。
        let reader = objstore::objectio::compressedio::new_reader(
            compression,
            objstore::objectio::compressedio::DecompressConfig::default(),
            raw,
        )
        .map_err(|error| mydump::MydumpError::Io(error.to_string()))?;
        Ok(reader.expect("compressed formats always construct a decoder"))
    }

    fn list(&self) -> Result<Vec<(String, i64)>, mydump::MydumpError> {
        let context = objstore::storage::Context::background();
        let mut files = Vec::new();
        self.inner
            .WalkDir(&context, None, &mut |path, size| {
                files.push((path.to_owned(), size));
                Ok(())
            })
            .map_err(|error| mydump::MydumpError::Io(error.to_string()))?;
        Ok(files)
    }
}

/// 为 IMPORT INTO 体量采样提供与 loader 相同的对象存储读取器。
struct ScannerKVSizeParserService {
    storage: LoaderStorage,
}

impl execimporter::KVSizeParserService for ScannerKVSizeParserService {
    fn NewParser(
        &self,
        file: &mydump::SourceFileMeta,
        config: &execimporter::KVSizeSampleConfig,
    ) -> Result<Box<dyn mydump::Parser>, String> {
        let csv_config = execimporter::generateCSVConfig(
            &config.FieldNullDef,
            &config.LineFieldsInfo,
            true,
            false,
        );
        let file_info = mydump::FileInfo {
            file_meta: mydump::FileMeta {
                path: file.path.clone(),
                file_size: file.file_size,
                real_size: file.real_size,
                source_type: file.source_type,
                compression: file.compression,
                sort_key: file.sort_key.clone(),
            },
            ..Default::default()
        };
        mydump::OpenReader(&file_info, &csv_config, &self.storage)
            .map_err(|error| error.to_string())
    }
}

/// 把 JobDatabase 适配为 mydump SchemaDatabase（执行 DDL / 查询）。
struct SchemaDatabaseAdapter {
    inner: Arc<dyn JobDatabase>,
}

/// 将 SharedError 转为 mydump Schema 错误。
fn schemaDatabaseError(error: errors::SharedError) -> mydump::MydumpError {
    mydump::MydumpError::Schema(error.to_string())
}

impl mydump::SchemaDatabase for SchemaDatabaseAdapter {
    fn execute(&self, sql: &str) -> Result<(), mydump::MydumpError> {
        // Go's schema importer only executes the CREATE statement selected from
        // a schema file.  Keep the adapter defensive as the Rust importer can
        // currently forward a preceding DROP TABLE statement as well.
        if sql
            .trim_start()
            .to_ascii_uppercase()
            .starts_with("DROP TABLE")
        {
            return Ok(());
        }
        self.inner
            .ExecContext(&(), sql)
            .map_err(schemaDatabaseError)
    }

    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, mydump::MydumpError> {
        let mut rows = self
            .inner
            .QueryContext(&(), sql)
            .map_err(schemaDatabaseError)?;
        let mut result = Vec::new();
        // 逐行拉取并把 SQLValue 展成字符串矩阵（NULL 变空串）。
        let read_result = loop {
            match rows.Next().map_err(schemaDatabaseError)? {
                Some(row) => result.push(
                    row.into_iter()
                        .map(|value| match value {
                            SQLValue::Null => String::new(),
                            SQLValue::Int64(value) => value.to_string(),
                            SQLValue::String(value) => value,
                        })
                        .collect(),
                ),
                None => break Ok(result),
            }
        };
        let close_result = rows.Close().map_err(schemaDatabaseError);
        match (read_result, close_result) {
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            (Ok(rows), Ok(())) => Ok(rows),
        }
    }
}

/// 解析源路径、创建外部存储与 MyDump loader，返回 [`FileScanner`] 实现。
pub fn NewFileScanner(
    _ctx: &(dyn Any + Send + Sync),
    source_path: &str,
    db: Arc<dyn JobDatabase>,
    cfg: SDKConfig,
) -> Result<Box<dyn FileScanner>, errors::SharedError> {
    let redacted_source_path = redactURL(source_path);
    // 解析失败且无法脱敏时，错误消息改用占位路径避免泄漏原文。
    let parse_error_source_path =
        if redacted_source_path == source_path && Url::parse(source_path).is_err() {
            redactedInvalidSourcePath.to_owned()
        } else {
            redacted_source_path.clone()
        };

    let backend = objstore::parse::ParseBackend(source_path, None).map_err(|_| {
        annotate(
            &ErrParseStorageURL,
            format!("source={parse_error_source_path}"),
        )
    })?;
    let storage_context = objstore::storage::Context::background();
    let store = objstore::storage::New(
        &storage_context,
        &backend,
        Some(&objstore::storage::Options::default()),
    )
    .map_err(|error| {
        annotate(
            &ErrCreateExternalStorage,
            format!("source={redacted_source_path}, err={error}"),
        )
    })?;

    // 组装 loader：空文件路由则用 mydump 默认规则。
    let loader_config = mydump::LoaderConfig {
        char_set: cfg.charset.clone(),
        file_routes: if cfg.file_route_rules.is_empty() {
            mydump::default_file_route_rules()
        } else {
            cfg.file_route_rules.clone()
        },
        filter: cfg.filter.clone(),
        default_file_rules: cfg.file_route_rules.is_empty(),
    };
    let mut loader_options = Vec::new();
    if let Some(limit) = cfg.max_scan_files.filter(|limit| *limit > 0) {
        loader_options.push(mydump::WithMaxScanFiles(limit as usize));
    }
    if cfg.concurrency > 0 {
        loader_options.push(mydump::WithScanFileConcurrency(cfg.concurrency as usize));
    }
    if !cfg.estimate_real_size {
        loader_options.push(mydump::WithSkipRealSizeEstimation(true));
    }
    if cfg.file_route_rules.is_empty() {
        loader_options.push(mydump::WithAuroraAutoMapping());
    }
    let loader_store: Arc<dyn mydump::Storage> = Arc::new(LoaderStorage {
        inner: Arc::clone(&store),
    });
    let loader = match mydump::NewLoaderWithStore(loader_config, loader_store, loader_options) {
        Ok(loader) => loader,
        Err(error) => {
            store.Close();
            return Err(annotate(
                &ErrCreateLoader,
                format!(
                    "source={}, charset={}, err={error}",
                    redacted_source_path, cfg.charset
                ),
            ));
        }
    };

    let aurora_source = loader.IsAuroraSource();
    Ok(Box::new(fileScanner {
        redacted_source_path,
        db,
        store: Some(store),
        loader,
        logger: cfg.logger.clone(),
        config: cfg,
        aurora_source,
    }))
}

impl fileScanner {
    /// 构造并发执行 DDL 的 SchemaImporter。
    fn schemaImporter(&self) -> mydump::SchemaImporter {
        let database: Arc<dyn mydump::SchemaDatabase> = Arc::new(SchemaDatabaseAdapter {
            inner: Arc::clone(&self.db),
        });
        mydump::NewSchemaImporter(
            database,
            self.loader.GetStore(),
            self.config.concurrency.max(1) as usize,
        )
    }
}

impl FileScanner for fileScanner {
    fn CreateSchemasAndTables(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
    ) -> Result<(), errors::SharedError> {
        let databases = self.loader.GetDatabases();
        if databases.is_empty() {
            return Err(annotate(
                &ErrNoDatabasesFound,
                format!("source={}", self.redacted_source_path),
            ));
        }
        self.schemaImporter().Run(databases).map_err(|error| {
            annotate(
                &ErrCreateSchema,
                format!(
                    "source={}, db_count={}, err={error}",
                    self.redacted_source_path,
                    databases.len()
                ),
            )
        })
    }

    fn CreateSchemaAndTableByName(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
        schema: &str,
        table: &str,
    ) -> Result<(), errors::SharedError> {
        for database in self.loader.GetDatabases() {
            if database.name != schema {
                continue;
            }
            let Some(table_meta) = database.tables.iter().find(|meta| meta.name == table) else {
                return Err(annotate(
                    &ErrTableNotFound,
                    format!("schema={schema}, table={table}"),
                ));
            };
            // 只挑选目标表，views 置空，避免误建其它对象。
            let selected = mydump::MDDatabaseMeta {
                name: database.name.clone(),
                schema_file: database.schema_file.clone(),
                tables: vec![table_meta.clone()],
                views: Vec::new(),
                char_set: database.char_set.clone(),
            };
            return self.schemaImporter().Run(&[selected]).map_err(|error| {
                annotate(
                    &ErrCreateSchema,
                    format!(
                        "source={}, schema={schema}, table={table}, err={error}",
                        self.redacted_source_path
                    ),
                )
            });
        }
        Err(annotate(&ErrSchemaNotFound, format!("schema={schema}")))
    }

    fn GetTableMetas(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
    ) -> Result<Vec<TableMeta>, errors::SharedError> {
        let mut result = Vec::new();
        for database in self.loader.GetDatabases() {
            for table in &database.tables {
                match self.buildTableMeta(database, table, self.loader.GetAllFiles()) {
                    Ok(meta) => result.push(meta),
                    // skip_invalid_files 时记 warn 并跳过坏表。
                    Err(error) if self.config.skip_invalid_files && !self.aurora_source => {
                        self.logger.Warn(
                            "skipping table due to invalid files",
                            [
                                log::Field::string("database", database.name.clone()),
                                log::Field::string("table", table.name.clone()),
                                log::Field::string("error", error.to_string()),
                            ],
                        )
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(result)
    }

    fn GetTableMetaByName(
        &mut self,
        _ctx: &(dyn Any + Send + Sync),
        db: &str,
        table: &str,
    ) -> Result<TableMeta, errors::SharedError> {
        for database in self.loader.GetDatabases() {
            if database.name != db {
                continue;
            }
            if let Some(table_meta) = database.tables.iter().find(|meta| meta.name == table) {
                return self.buildTableMeta(database, table_meta, self.loader.GetAllFiles());
            }
        }
        Err(annotate(
            &ErrTableNotFound,
            format!("table {db}.{table} not found"),
        ))
    }

    fn GetTotalSize(&self, _ctx: &(dyn Any + Send + Sync)) -> i64 {
        self.loader
            .GetDatabases()
            .iter()
            .flat_map(|database| database.tables.iter())
            .flat_map(|table| table.data_files.iter())
            .map(|file| self.dataFileSize(file))
            .sum()
    }

    fn EstimateImportDataSize(
        &mut self,
        ctx: &(dyn Any + Send + Sync),
    ) -> Result<ImportDataSizeEstimate, errors::SharedError> {
        let databases = self.loader.GetDatabases();
        if databases.is_empty() {
            return Err(annotate(
                &ErrNoDatabasesFound,
                format!("source={}", self.redacted_source_path),
            ));
        }
        let mut result = ImportDataSizeEstimate::default();
        for database in databases {
            for table in &database.tables {
                let tikv_size = match self.estimateOneTableSize(ctx, table) {
                    Ok(size) => size,
                    Err(error) if self.config.skip_invalid_files && !self.aurora_source => {
                        self.logger.Warn(
                            "skipping table during size estimation",
                            [
                                log::Field::string("database", database.name.clone()),
                                log::Field::string("table", table.name.clone()),
                                log::Field::string("error", error.to_string()),
                            ],
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let estimate = TableDataSizeEstimate {
                    Database: database.name.clone(),
                    Table: table.name.clone(),
                    SourceSize: table
                        .data_files
                        .iter()
                        .map(|file| self.dataFileSize(file))
                        .sum(),
                    TiKVSize: tikv_size,
                };
                result.TotalSourceSize += estimate.SourceSize;
                result.TotalTiKVSize += estimate.TiKVSize;
                result.Tables.push(estimate);
            }
        }
        Ok(result)
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        if let Some(store) = self.store.take() {
            store.Close();
        }
        Ok(())
    }
}

impl fileScanner {
    /// 组装单表 TableMeta：数据文件列表、总量，并生成通配路径。
    fn buildTableMeta(
        &self,
        database: &mydump::MDDatabaseMeta,
        table: &mydump::MDTableMeta,
        all_files: &HashMap<String, mydump::FileInfo>,
    ) -> Result<TableMeta, errors::SharedError> {
        let mut data_files = Vec::with_capacity(table.data_files.len());
        let mut total_size = 0;
        for file in &table.data_files {
            let mut meta = createDataFileMeta(file);
            meta.Size = self.dataFileSize(file);
            total_size += meta.Size;
            data_files.push(meta);
        }
        let mut result = TableMeta {
            Database: database.name.clone(),
            Table: table.name.clone(),
            DataFiles: data_files,
            SchemaFile: table.schema_file.file_meta.path.clone(),
            TotalSize: total_size,
            ..Default::default()
        };
        if table.data_files.is_empty() {
            self.logger.Warn(
                "table has no data files",
                [
                    log::Field::string("database", database.name.clone()),
                    log::Field::string("table", table.name.clone()),
                ],
            );
            return Ok(result);
        }
        let wildcard =
            generateWildcardPath(&table.data_files, all_files, &database.name, &table.name)?;
        let mut uri = self
            .store
            .as_ref()
            .expect("scanner storage is present before Close")
            .URI();
        if self.aurora_source && uri.contains(['*', '?', '[', ']', '\\']) {
            return Err(errors::New(
                "Aurora source prefix contains glob metacharacters",
            ));
        }
        // 本地 file:// 前缀在通配路径中剥掉，便于 IMPORT INTO 使用。
        uri = uri.strip_prefix("file://").unwrap_or(&uri).to_owned();
        result.WildcardPath = format!("{}/{}", uri.trim_end_matches('/'), wildcard);
        if self.aurora_source {
            result.WildcardPath = encodeAuroraWildcardPath(&result.WildcardPath)?;
        }
        Ok(result)
    }

    /// 返回 Go `SourceFileMeta.RealSize` 的等价值；关闭估算或未压缩文件
    /// 使用存储报告的 FileSize，压缩文件按 loader 的真实大小采样逻辑估算。
    fn dataFileSize(&self, file: &mydump::FileInfo) -> i64 {
        if !self.config.estimate_real_size
            || file.file_meta.compression == mydump::Compression::None
        {
            return file.file_meta.file_size;
        }
        let meta = mydump::SourceFileMeta {
            path: file.file_meta.path.clone(),
            source_type: file.file_meta.source_type,
            compression: file.file_meta.compression,
            sort_key: file.file_meta.sort_key.clone(),
            file_size: file.file_meta.file_size,
            ..Default::default()
        };
        mydump::EstimateRealSizeForFile(&meta, self.loader.GetStore().as_ref())
    }

    /// 估算单表导入后 TiKV 大小；当前因上游采样服务未导出而返回明确依赖错误。
    fn estimateOneTableSize(
        &self,
        _ctx: &(dyn Any + Send + Sync),
        table: &mydump::MDTableMeta,
    ) -> Result<i64, errors::SharedError> {
        if table.data_files.is_empty() {
            return Ok(0);
        }
        if table.schema_file.file_meta.path.is_empty() {
            return Err(errors::New(format!(
                "table `{}`.`{}` schema not found",
                table.db, table.name
            )));
        }
        let format = sourceTypeToImportFormat(table.data_files[0].file_meta.source_type)?;
        let mut table_info = self.buildEstimateTableInfo(table)?;
        table_info.State = model::StatePublic;
        let data_files = table
            .data_files
            .iter()
            .map(|file| mydump::SourceFileMeta {
                path: file.file_meta.path.clone(),
                file_size: file.file_meta.file_size,
                real_size: file.file_meta.real_size,
                source_type: file.file_meta.source_type,
                compression: file.file_meta.compression,
                sort_key: file.file_meta.sort_key.clone(),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let parser_service = ScannerKVSizeParserService {
            storage: LoaderStorage {
                inner: self
                    .store
                    .as_ref()
                    .expect("scanner storage is present before Close")
                    .clone(),
            },
        };
        let sampled = execimporter::SampleFileImportKVSizeWithTableInfo(
            self.buildEstimateSampleConfig(format),
            &table_info,
            &data_files,
            &[],
            &parser_service,
        )
        .map_err(errors::New)?;
        if sampled.SourceSize == 0 && sampled.TotalKVSize() == 0 {
            return Ok(0);
        }
        if sampled.SourceSize <= 0 || sampled.TotalKVSize() <= 0 {
            return Ok(table
                .data_files
                .iter()
                .map(|file| self.dataFileSize(file))
                .sum());
        }
        let total_size: i64 = table
            .data_files
            .iter()
            .map(|file| self.dataFileSize(file))
            .sum();
        Ok(
            ((total_size as f64) * (sampled.TotalKVSize() as f64) / (sampled.SourceSize as f64))
                as i64,
        )
    }

    /// 根据 CSV 配置与系统变量默认值构造 KV 大小采样配置。
    fn buildEstimateSampleConfig(&self, format: String) -> execimporter::KVSizeSampleConfig {
        let mut field_null_def = self.config.csv_config.FieldNullDefinedBy.clone();
        if field_null_def.is_empty() {
            field_null_def.push(r"\N".to_owned());
        }
        let mut fields_escaped_by = self.config.csv_config.FieldsEscapedBy.clone();
        // BackslashEscape 且未显式设置转义符时默认使用反斜杠。
        if fields_escaped_by.is_empty() && self.config.csv_config.BackslashEscape {
            fields_escaped_by = r"\".to_owned();
        }
        let mut important_sys_vars = HashMap::new();
        important_sys_vars.extend(
            IMPORTANT_VARIABLE_DEFAULTS
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );
        important_sys_vars.extend(
            IMPORT_VARIABLE_DEFAULTS
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );
        execimporter::KVSizeSampleConfig {
            Format: format.clone(),
            SQLMode: self.config.sql_mode,
            Charset: Some(self.config.data_character_set.clone()),
            ImportantSysVars: important_sys_vars,
            FieldNullDef: field_null_def,
            LineFieldsInfo: execimporter::LineFieldsInfo {
                FieldsTerminatedBy: self.config.csv_config.FieldsTerminatedBy.clone(),
                FieldsEnclosedBy: self.config.csv_config.FieldsEnclosedBy.clone(),
                FieldsEscapedBy: fields_escaped_by,
                LinesStartingBy: self.config.csv_config.LinesStartingBy.clone(),
                LinesTerminatedBy: self.config.csv_config.LinesTerminatedBy.clone(),
                ..Default::default()
            },
            // CSV 且带表头时忽略首行。
            IgnoreLines: u64::from(
                format == execimporter::DataFormatCSV && self.config.csv_config.Header,
            ),
            ..Default::default()
        }
    }

    /// 读取表 schema SQL，解析 CREATE TABLE 并构建 meta TableInfo。
    fn buildEstimateTableInfo(
        &self,
        table: &mydump::MDTableMeta,
    ) -> Result<model::TableInfo, errors::SharedError> {
        let schema = table
            .GetSchema(self.loader.GetStore().as_ref())
            .map_err(|error| errors::New(error.to_string()))?;
        let mut parser = parser::New();
        parser.SetSQLMode(self.config.sql_mode);
        let (statements, _) = parser
            .ParseSQL(&schema, &[])
            .map_err(|error| errors::New(error.to_string()))?;
        let create_statement = buildEstimateCreateTableStmt(&statements, table)?;
        let context = metabuild::NewContext::<(), Infallible>(Vec::new());
        let mut table_info = ddl::BuildTableInfoFromAST(&context, &create_statement)
            .map_err(|error| errors::New(error.to_string()))?;
        // Go's BuildTableInfoFromAST returns metadata ready for use by the
        // importer.  The Rust builder leaves newly built columns and indexes in
        // StateNone, which makes the sampler see an empty row and omit indexes.
        for column in &mut table_info.Columns {
            column.State = model::StatePublic;
        }
        for index in &mut table_info.Indices {
            index.State = model::StatePublic;
        }
        // `tables.MockTableFromMeta` in Go recognizes a PK-handle column from
        // its flag even though such a primary key has no separate IndexInfo.
        // The Rust sampling adapter currently recognizes primary-key columns
        // through IndexInfo only, so materialize the equivalent metadata at
        // this boundary without changing PKIsHandle semantics.
        if table_info.PKIsHandle && !table_info.Indices.iter().any(|index| index.Primary) {
            if let Some(column) = table_info
                .Columns
                .iter()
                .find(|column| model::mysql::HasPriKeyFlag(column.GetFlag()))
            {
                table_info.Indices.push(model::IndexInfo {
                    Name: ast::NewCIStr("PRIMARY"),
                    Columns: vec![model::IndexColumn {
                        Name: column.Name.clone(),
                        Offset: column.Offset as isize,
                        ..Default::default()
                    }],
                    State: model::StatePublic,
                    Unique: true,
                    Primary: true,
                    ..Default::default()
                });
            }
        }
        Ok(table_info)
    }
}

/// 对远端 Aurora 原始对象键恰好编码一次；本地绝对路径保持不变。
pub(crate) fn encodeAuroraWildcardPath(path: &str) -> Result<String, errors::SharedError> {
    let Some((scheme, rest)) = path.split_once("://") else {
        return Ok(path.to_owned());
    };
    let (host, raw_path) = rest.split_once('/').unwrap_or((rest, ""));
    let escaped_percent_path = raw_path.replace('%', "%25");
    Url::parse(&format!("{scheme}://{host}/{escaped_percent_path}"))
        .map(|uri| uri.to_string())
        .map_err(|error| errors::New(error.to_string()))
}

/// 将 FileInfo 列表转为 DataFileMeta，并汇总文件大小。
pub fn processDataFiles(files: &[mydump::FileInfo]) -> (Vec<DataFileMeta>, i64) {
    let data_files = files.iter().map(createDataFileMeta).collect::<Vec<_>>();
    let total_size = files.iter().map(fileRealSize).sum();
    (data_files, total_size)
}

/// 从 FileInfo 提取路径、大小、格式与压缩类型。
pub fn createDataFileMeta(file: &mydump::FileInfo) -> DataFileMeta {
    DataFileMeta {
        Path: file.file_meta.path.clone(),
        Size: fileRealSize(file),
        Format: file.file_meta.source_type,
        Compression: file.file_meta.compression,
    }
}

/// 使用 loader 已填充的真实大小；手工构造的测试元数据没有该字段时
/// 回退到存储报告大小，等价于 Go loader 对未压缩文件的初始化行为。
fn fileRealSize(file: &mydump::FileInfo) -> i64 {
    if file.file_meta.real_size > 0 {
        file.file_meta.real_size
    } else {
        file.file_meta.file_size
    }
}

/// 深拷贝 CreateTableStmt（Select 置空，避免估算路径携带查询）。
fn cloneCreateTable(statement: &ast::CreateTableStmt) -> Box<ast::CreateTableStmt> {
    Box::new(ast::CreateTableStmt {
        node_text: statement.node_text.clone(),
        IfNotExists: statement.IfNotExists,
        TemporaryKeyword: statement.TemporaryKeyword,
        OnCommitDelete: statement.OnCommitDelete,
        Table: statement.Table.clone(),
        ReferTable: statement.ReferTable.clone(),
        Cols: statement.Cols.clone(),
        Constraints: statement.Constraints.clone(),
        Options: statement.Options.clone(),
        Partition: statement.Partition.clone(),
        SplitIndex: statement.SplitIndex.clone(),
        OnDuplicate: statement.OnDuplicate,
        Select: None,
    })
}

/// 从解析出的语句中挑选与表元数据匹配的 CREATE TABLE；仅一条则直接采用。
pub fn buildEstimateCreateTableStmt(
    statements: &[Box<dyn ast::Node>],
    table: &mydump::MDTableMeta,
) -> Result<Box<ast::CreateTableStmt>, errors::SharedError> {
    let mut first = None;
    let mut count = 0;
    for statement in statements {
        let Some(create) = statement.as_any().downcast_ref::<ast::CreateTableStmt>() else {
            continue;
        };
        if first.is_none() {
            first = Some(cloneCreateTable(create));
        }
        count += 1;
        if estimateCreateTableStmtMatchesMeta(create, table) {
            return Ok(cloneCreateTable(create));
        }
    }
    if count == 1 {
        return Ok(first.expect("one CREATE TABLE statement was recorded"));
    }
    if count == 0 {
        return Err(errors::New(format!(
            "schema file {} does not contain a CREATE TABLE statement",
            table.schema_file.file_meta.path
        )));
    }
    Err(errors::New(format!(
        "schema file {} contains {} CREATE TABLE statements but none match table {}.{}",
        table.schema_file.file_meta.path, count, table.db, table.name
    )))
}

/// 判断 CREATE TABLE 的表名（及可选 schema）是否与 MDTableMeta 匹配（忽略大小写）。
pub fn estimateCreateTableStmtMatchesMeta(
    create: &ast::CreateTableStmt,
    table: &mydump::MDTableMeta,
) -> bool {
    if !create.Table.Name.O.eq_ignore_ascii_case(&table.name) {
        return false;
    }
    let schema = &create.Table.Schema.O;
    schema.is_empty() || schema.eq_ignore_ascii_case(&table.db)
}

/// 把 mydump SourceType 映射为 IMPORT 数据格式字符串（CSV/SQL/Parquet）。
pub fn sourceTypeToImportFormat(
    source_type: mydump::SourceType,
) -> Result<String, errors::SharedError> {
    match source_type {
        mydump::SourceType::Csv => Ok(execimporter::DataFormatCSV.to_owned()),
        mydump::SourceType::Sql => Ok(execimporter::DataFormatSQL.to_owned()),
        mydump::SourceType::Parquet => Ok(execimporter::DataFormatParquet.to_owned()),
        unsupported => Err(errors::New(format!(
            "unsupported source format {unsupported:?}"
        ))),
    }
}
