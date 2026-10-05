// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// IMPORT INTO / LOAD DATA 的计划、选项与控制器。
//
// 解析导入选项、构造 `Plan`/`LoadDataController`、打开外部存储、
// 估算文件真实大小与资源参数，并生成 CSV 解析配置。
// 全局排序使用 cloud storage；本地排序直接写本地引擎后再导入 TiKV Region。

use std::collections::{HashMap, HashSet};
use std::io::{Read, SeekFrom};
use std::sync::{Arc, Mutex};

use astersql_lightning_backend_encode::Datum;
use astersql_lightning_backend_kv::{Session, litExprContext};
use astersql_lightning_mydump::{
    Compression, Parser as MydumpParser, ReadSeekCloser, SourceFileMeta, SourceType,
};
use astersql_meta_model::TableInfo;
use astersql_objstore_storeapi::{Context as StorageContext, Storage, WalkOption};
use astersql_parser_ast as ast;
use astersql_parser_mysql::r#const::SQLMode;
use astersql_table::{self as table, Table};
use astersql_util_context::SQLWarn;

/// 数据格式：CSV。
pub const DataFormatCSV: &str = "csv";
/// 数据格式：定界文本（LOAD DATA 默认）。
pub const DataFormatDelimitedData: &str = "delimited data";
/// 数据格式：SQL 转储。
pub const DataFormatSQL: &str = "sql";
/// 数据格式：Parquet。
pub const DataFormatParquet: &str = "parquet";
/// 数据格式：按文件后缀自动检测。
pub const DataFormatAuto: &str = "auto";
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 字节大小包装，用于磁盘配额与限速等选项。
pub struct ByteSize(pub i64);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 导入后置操作级别（如 checksum）：关闭 / 可选 / 必须。
pub enum PostOpLevel {
    /// 关闭后置操作。
    Off,
    /// 可选：失败仅告警。
    Optional,
    #[default]
    /// 必须：失败则导入失败。
    Required,
}

impl PostOpLevel {
    /// 从字符串解析后置操作级别。
    pub fn FromStringValue(&mut self, value: &str) -> Result<(), String> {
        *self = match value.to_ascii_lowercase().as_str() {
            "off" | "false" => Self::Off,
            "optional" => Self::Optional,
            "required" | "true" => Self::Required,
            _ => return Err(format!("invalid post-operation level {value}")),
        };
        Ok(())
    }
}

/// 默认本地磁盘配额：50 GiB。
pub const DefaultDiskQuota: ByteSize = ByteSize(50_i64 << 30);
/// 写速不限（0 表示无限制）。
pub const unlimitedWriteSpeed: ByteSize = ByteSize(0);
/// 读取块大小：64 KiB。
pub const LoadDataReadBlockSize: i64 = 64 * 1024;
/// 默认字符集 utf8mb4。
pub const defaultCharacterSet: &str = "utf8mb4";
/// 默认 NULL 字面量定义（`\N`）。
pub const defaultFieldNullDef: &[&str] = &[r"\N"];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// CSV/定界文本的字段与行分隔配置。
pub struct LineFieldsInfo {
    /// 字段分隔符。
    pub FieldsTerminatedBy: String,
    /// 字段包围符。
    pub FieldsEnclosedBy: String,
    /// 转义字符。
    pub FieldsEscapedBy: String,
    /// 是否 OPTIONALLY ENCLOSED。
    pub FieldsOptEnclosed: bool,
    /// 行起始前缀。
    pub LinesStartingBy: String,
    /// 行终止符。
    pub LinesTerminatedBy: String,
}

/// 服务器本地磁盘路径允许的文件后缀（含压缩）。
const SUPPORTED_SERVER_SUFFIXES: &[&str] = &[
    ".csv", ".sql", ".parquet", ".gz", ".gzip", ".zstd", ".zst", ".snappy",
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 遇到重复键时的处理模式。
pub enum OnDupKeyMode {
    /// 捕获冲突行以便后续处理。
    Capture,
    #[default]
    /// 遇重复键立即报错。
    Error,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 导入数据源类型：文件或查询结果。
pub enum DataSourceType {
    #[default]
    /// 从文件/对象存储导入。
    File,
    /// 从 SELECT 查询结果导入。
    Query,
}

/// 重复键模式别名：捕获冲突。
pub const OnDupKeyModeCapture: OnDupKeyMode = OnDupKeyMode::Capture;
/// 重复键模式别名：报错。
pub const OnDupKeyModeError: OnDupKeyMode = OnDupKeyMode::Error;
/// 数据源别名：文件。
pub const DataSourceTypeFile: DataSourceType = DataSourceType::File;
/// 数据源别名：查询。
pub const DataSourceTypeQuery: DataSourceType = DataSourceType::Query;

/// 将重复键模式格式化为配置字符串。
impl std::fmt::Display for OnDupKeyMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Capture => "capture",
            Self::Error => "error",
        })
    }
}

/// 将数据源类型格式化为配置字符串。
impl std::fmt::Display for DataSourceType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::File => "file",
            Self::Query => "query",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// LOAD DATA 列列表中的用户变量（如 @a）。
pub struct UserVariable {
    /// 变量名（不含 @）。
    pub Name: String,
}

#[derive(Clone)]
/// 输入字段到表列或用户变量的映射。
pub struct FieldMapping {
    /// 映射到的表列；用户变量映射时为 None。
    pub Column: Option<Arc<table::Column>>,
    /// 映射到的用户变量；列映射时为 None。
    pub UserVar: Option<UserVariable>,
}

/// 按需打开可读可 seek 数据流的回调类型。
pub type ReaderOpener =
    Arc<dyn Fn(&StorageContext) -> Result<Box<dyn ReadSeekCloser>, String> + Send + Sync + 'static>;

#[derive(Clone)]
/// 单个数据文件的 opener 与远程元信息。
pub struct LoadDataReaderInfo {
    /// 打开该文件内容的回调。
    pub Opener: ReaderOpener,
    /// 远程文件元信息（路径、大小、压缩等）。
    pub Remote: Option<SourceFileMeta>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 持久化到 job 表的导入参数快照。
pub struct ImportParameters {
    /// 列与用户变量列表的文本形式。
    pub ColumnsAndVars: String,
    /// SET 子句文本。
    pub SetClause: String,
    /// 脱敏后的文件位置。
    pub FileLocation: String,
    /// 数据格式名。
    pub Format: String,
    /// 其余选项名→值。
    pub Options: HashMap<String, String>,
}

#[derive(Clone)]
/// 一次导入任务的完整执行计划与选项。
pub struct Plan {
    /// 目标库名。
    pub DBName: String,
    /// 目标库 ID。
    pub DBID: i64,
    /// 当前表元信息。
    pub TableInfo: Option<Arc<TableInfo>>,
    /// 期望表元信息（可能与当前不同）。
    pub DesiredTableInfo: Option<Arc<TableInfo>>,
    /// 数据路径（可含 glob / URI）。
    pub Path: String,
    /// 数据格式。
    pub Format: String,
    /// 严格模式（影响错误处理）。
    pub Restrictive: bool,
    /// 时区/位置标识。
    pub LocationID: String,
    /// 会话 SQL Mode。
    pub SQLMode: SQLMode,
    /// 字符集；None 表示未指定。
    pub Charset: Option<String>,
    /// 影响编码的系统变量快照。
    pub ImportantSysVars: HashMap<String, String>,
    /// NULL 字面量定义列表。
    pub FieldNullDef: Vec<String>,
    /// NULL 是否允许被包围符包裹。
    pub NullValueOptEnclosed: bool,
    /// 字段/行分隔配置。
    pub LineFieldsInfo: LineFieldsInfo,
    /// 跳过的文件头行数。
    pub IgnoreLines: u64,
    /// 本地磁盘配额。
    pub DiskQuota: ByteSize,
    /// 导入后 checksum 级别。
    pub Checksum: PostOpLevel,
    /// 工作线程数。
    pub ThreadCnt: usize,
    /// 分布式任务最大节点数。
    pub MaxNodeCnt: i32,
    /// 写 TiKV 限速。
    pub MaxWriteSpeed: ByteSize,
    /// 是否按行切分大文件。
    pub SplitFile: bool,
    /// 最多记录的错误条数。
    pub MaxRecordedErrors: i64,
    /// 重复键处理模式。
    pub OnDupKey: OnDupKeyMode,
    /// 是否异步分离执行。
    pub Detached: bool,
    /// 是否禁用 TiKV import mode。
    pub DisableTiKVImportMode: bool,
    /// 单个引擎最大大小。
    pub MaxEngineSize: ByteSize,
    /// 全局排序 cloud storage URI。
    pub CloudStorageURI: String,
    /// 是否跳过导入前置检查。
    pub DisablePrecheck: bool,
    /// 任务分组键。
    pub GroupKey: String,
    /// DistSQL 扫描并发。
    pub DistSQLScanConcurrency: usize,
    /// 是否来自 IMPORT INTO（相对 LOAD DATA）。
    pub InImportInto: bool,
    /// 数据源类型。
    pub DataSourceType: DataSourceType,
    /// 持久化参数快照。
    pub Parameters: Option<ImportParameters>,
    /// 用户显式指定的选项名集合。
    pub SpecifiedOptionNames: HashSet<String>,
    /// 发起用户。
    pub User: String,
    /// TiKV 是否 RaftKV2。
    pub IsRaftKV2: bool,
    /// 源文件总字节数。
    pub TotalFileSize: i64,
    /// 内部选项：强制合并步骤。
    pub ForceMergeStep: bool,
    /// 内部选项：手动恢复模式。
    pub ManualRecovery: bool,
    /// 多租户 keyspace 名。
    pub Keyspace: String,
    /// 是否使用新排序规则。
    pub UseNewCollate: Option<bool>,
}

/// 使用安全默认值初始化计划（格式 auto、checksum required 等）。
impl Default for Plan {
    fn default() -> Self {
        Self {
            DBName: String::new(),
            DBID: 0,
            TableInfo: None,
            DesiredTableInfo: None,
            Path: String::new(),
            Format: DataFormatAuto.to_owned(),
            Restrictive: false,
            LocationID: String::new(),
            SQLMode: SQLMode::default(),
            Charset: None,
            ImportantSysVars: HashMap::new(),
            FieldNullDef: Vec::new(),
            NullValueOptEnclosed: false,
            LineFieldsInfo: newDefaultLineFieldsInfo(),
            IgnoreLines: 0,
            // Go keeps this zero until disk-capacity based quota adjustment.
            DiskQuota: ByteSize(0),
            Checksum: PostOpLevel::Required,
            ThreadCnt: 1,
            MaxNodeCnt: 0,
            MaxWriteSpeed: unlimitedWriteSpeed,
            SplitFile: false,
            MaxRecordedErrors: 100,
            OnDupKey: OnDupKeyModeError,
            Detached: false,
            DisableTiKVImportMode: false,
            MaxEngineSize: ByteSize(0),
            CloudStorageURI: String::new(),
            DisablePrecheck: false,
            GroupKey: String::new(),
            DistSQLScanConcurrency: 0,
            InImportInto: false,
            DataSourceType: DataSourceTypeFile,
            Parameters: None,
            SpecifiedOptionNames: HashSet::new(),
            User: String::new(),
            IsRaftKV2: false,
            TotalFileSize: 0,
            ForceMergeStep: false,
            ManualRecovery: false,
            Keyspace: String::new(),
            UseNewCollate: None,
        }
    }
}

impl Plan {
    /// 返回重复键处理模式。
    pub fn GetOnDupKeyMode(&self) -> OnDupKeyMode {
        self.OnDupKey
    }

    /// 返回新排序规则开关，未设置时用 default_value。
    pub fn GetUseNewCollateOrDefault(&self, default_value: bool) -> bool {
        self.UseNewCollate.unwrap_or(default_value)
    }

    /// 设置是否使用新排序规则。
    pub fn setUseNewCollate(&mut self, use_new_collate: bool) {
        self.UseNewCollate = Some(use_new_collate);
    }

    /// 按数据源类型与目标节点 CPU 数填充默认线程/配额等选项。
    pub fn initDefaultOptions(&mut self, target_node_cpu_count: usize) {
        // 查询导入默认 2 线程；文件导入默认 CPU/2。
        self.ThreadCnt = if self.DataSourceType == DataSourceTypeQuery {
            2
        } else {
            (target_node_cpu_count / 2).max(1)
        };
        self.Checksum = PostOpLevel::Required;
        self.MaxWriteSpeed = unlimitedWriteSpeed;
        self.MaxRecordedErrors = 100;
        self.OnDupKey = OnDupKeyModeError;
        self.Charset = Some(defaultCharacterSet.to_owned());
        if self.MaxEngineSize.0 == 0 {
            self.MaxEngineSize = ByteSize(5 * 96 * 1024 * 1024);
        }
    }

    /// 按 CPU 上限裁剪线程数；全局排序时强制关闭 TiKV import mode。
    pub fn adjustOptions(&mut self, target_node_cpu_count: usize) {
        let limit = if self.DataSourceType == DataSourceTypeQuery {
            target_node_cpu_count.saturating_mul(2)
        } else {
            target_node_cpu_count
        };
        self.ThreadCnt = self.ThreadCnt.min(limit);
        // 全局排序路径不走 TiKV import mode。
        if self.IsGlobalSort() {
            self.DisableTiKVImportMode = true;
        }
    }

    /// 是否本地排序（未配置 cloud storage URI）。
    pub fn IsLocalSort(&self) -> bool {
        self.CloudStorageURI.is_empty()
    }

    /// 是否全局排序（配置了 cloud storage）。
    pub fn IsGlobalSort(&self) -> bool {
        !self.IsLocalSort()
    }

    /// 非 CSV 格式时拒绝仅 CSV 可用的选项。
    pub fn CheckNonCSVFormatOptions(&self) -> Result<(), String> {
        if matches!(
            self.Format.as_str(),
            DataFormatCSV | DataFormatDelimitedData | DataFormatAuto
        ) {
            return Ok(());
        }
        // 下列选项仅 CSV/定界文本合法。
        const CSV_ONLY: &[&str] = &[
            "character_set",
            "fields_terminated_by",
            "fields_enclosed_by",
            "fields_escaped_by",
            "fields_defined_null_by",
            "lines_terminated_by",
            "skip_rows",
            "split_file",
        ];
        if let Some(name) = CSV_ONLY
            .iter()
            .find(|name| self.SpecifiedOptionNames.contains(**name))
        {
            return Err(format!("option {name} only supports CSV format"));
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
/// 从 AST 提取的列列表、赋值与 FIELDS/LINES 子句。
pub struct ASTArgs {
    /// 文件位置引用类型。
    pub FileLocRef: ast::FileLocRef,
    /// 列名或用户变量列表。
    pub ColumnsAndUserVars: Vec<ast::ColumnNameOrUserVar>,
    /// SET 赋值列表。
    pub ColumnAssignments: Vec<ast::Assignment>,
    /// ON DUPLICATE 处理类型。
    pub OnDuplicate: ast::OnDuplicateKeyHandlingType,
    /// FIELDS 子句。
    pub FieldsInfo: Option<ast::FieldsClause>,
    /// LINES 子句。
    pub LinesInfo: Option<ast::LinesClause>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单个执行步骤的字节数与行数摘要。
pub struct StepSummary {
    /// 处理字节数。
    pub Bytes: i64,
    /// 处理行数。
    pub RowCnt: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 整次导入各阶段摘要与冲突统计。
pub struct Summary {
    /// 编码阶段摘要。
    pub EncodeSummary: StepSummary,
    /// 合并阶段摘要。
    pub MergeSummary: StepSummary,
    /// 摄入阶段摘要。
    pub IngestSummary: StepSummary,
    /// 收集冲突阶段摘要。
    pub CollectConflictsSummary: StepSummary,
    /// 解决冲突阶段摘要。
    pub ResolveConflictsSummary: StepSummary,
    /// 成功导入行数。
    pub ImportedRows: i64,
    /// 冲突行数。
    pub ConflictRowCnt: u64,
    /// 冲突是否超出阈值。
    pub TooManyConflicts: bool,
}

/// 导入所需的会话能力：系统变量、权限、选项求值等。
pub trait ImportSessionContext {
    fn SQLMode(&self) -> SQLMode;
    fn SystemVariable(&self, name: &str) -> Option<String>;
    fn DistSQLScanConcurrency(&self) -> usize;
    fn User(&self) -> String;
    fn Keyspace(&self) -> String;
    fn LocationID(&self) -> String;
    fn EvalLoadDataOption(&self, option: &ast::LoadDataOpt) -> Result<ImportOptionValue, String>;
    fn FormatColumnOrUserVariable(
        &self,
        value: &ast::ColumnNameOrUserVar,
    ) -> Result<String, String>;
    fn FormatAssignment(&self, value: &ast::Assignment) -> Result<String, String>;
    fn RedactURL(&self, value: &str) -> String;
    fn IsNextGen(&self) -> bool;
    fn SEMEnabled(&self) -> bool;
    fn MaxDistTaskNodes(&self) -> i32;
    fn AutoMaxDistTaskNodes(&self) -> Result<i32, String>;
    fn DefaultCloudStorageURI(&self) -> String;
}

/// 已解析的 LOAD DATA 计划视图。
pub trait ResolvedLoadDataPlan {
    fn DBName(&self) -> &str;
    fn DBID(&self) -> i64;
    fn Path(&self) -> &str;
    fn Charset(&self) -> Option<&str>;
    fn FileLocRef(&self) -> ast::FileLocRef;
    fn OnDuplicate(&self) -> ast::OnDuplicateKeyHandlingType;
    fn FieldsInfo(&self) -> Option<&ast::FieldsClause>;
    fn LinesInfo(&self) -> Option<&ast::LinesClause>;
    fn IgnoreLines(&self) -> Option<u64>;
    fn ColumnsAndUserVars(&self) -> &[ast::ColumnNameOrUserVar];
    fn ColumnAssignments(&self) -> &[ast::Assignment];
}

/// 已解析的 IMPORT INTO 计划视图。
pub trait ResolvedImportIntoPlan {
    fn DBName(&self) -> &str;
    fn DBID(&self) -> i64;
    fn Path(&self) -> &str;
    fn Format(&self) -> Option<&str>;
    fn ColumnsAndUserVars(&self) -> &[ast::ColumnNameOrUserVar];
    fn ColumnAssignments(&self) -> &[ast::Assignment];
    fn Options(&self) -> &[ast::LoadDataOpt];
    fn HasSelectPlan(&self) -> bool;
}

#[derive(Clone, Debug, PartialEq)]
/// 导入选项求值后的类型化取值。
pub enum ImportOptionValue {
    /// 字符串选项值。
    String(String),
    /// 整数选项值。
    Integer(i64),
    /// 布尔选项值（无参选项视为 true）。
    Boolean(bool),
}

/// SET 子句单列赋值表达式。
pub trait ColAssignExpression: Send + Sync {
    fn Eval(&self, session: &Session) -> Result<Datum, String>;
}

/// 将 AST 赋值编译为可求值表达式。
pub trait ColAssignExpressionBuilder: Send + Sync {
    fn Build(
        &self,
        context: &litExprContext,
        use_new_collation: bool,
    ) -> Result<(Arc<dyn ColAssignExpression>, Vec<SQLWarn>), String>;
}

/// 从 AST Assignment 构造表达式构建器。
pub trait ColumnAssignmentFactory: Send + Sync {
    fn BuildAssignment(
        &self,
        assignment: &ast::Assignment,
    ) -> Result<Arc<dyn ColAssignExpressionBuilder>, String>;
}

/// 原始 Datum 到列类型的转换与当前时间填充。
pub trait ImportDatumConverter: Send + Sync {
    fn CastColumnValue(&self, value: Datum, column: &table::Column) -> Result<Datum, String>;
    fn CurrentTime(&self, column: &table::Column) -> Result<Datum, String>;
}

/// 按格式创建 mydump 行解析器。
pub trait ImportParserFactory: Send + Sync {
    fn NewParser(
        &self,
        format: &str,
        reader: Box<dyn ReadSeekCloser>,
        file: &SourceFileMeta,
        plan: &Plan,
    ) -> Result<Box<dyn MydumpParser>, String>;
}

/// 估算解压后真实大小与 Parquet 膨胀比。
pub trait ImportSizeEstimator: Send + Sync {
    fn EstimateRealSize(
        &self,
        context: &StorageContext,
        file: &SourceFileMeta,
        storage: &dyn Storage,
    ) -> Result<i64, String>;

    fn ParquetExpansionRatio(
        &self,
        context: &StorageContext,
        file_path: &str,
        file_size: i64,
        storage: &dyn Storage,
    ) -> Result<f64, String>;
}

/// 打开外部对象存储。
pub trait ImportStorageFactory: Send + Sync {
    fn Open(
        &self,
        context: &StorageContext,
        uri: &str,
        target: &str,
    ) -> Result<SharedStorage, String>;
}

/// 探测 TiKV 是否为 RaftKV2。
pub trait TiKVConfigProbe: Send + Sync {
    fn IsRaftKV2(&self) -> Result<bool, String>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// 调度放大因子，用于资源参数估算。
pub struct ScheduleTuneFactors {
    /// 资源放大系数。
    pub AmplifyFactor: f64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 计算得到的线程/节点/扫描并发参数。
pub struct ResourceParams {
    /// 建议线程数。
    pub ThreadCnt: usize,
    /// 建议最大节点数。
    pub MaxNodeCnt: i32,
    /// DistSQL 扫描并发。
    pub DistSQLScanConcurrency: usize,
}

/// 目标节点 CPU、索引占比采样与资源参数计算。
pub trait ImportResourceCalculator: Send + Sync {
    fn TargetNodeCPUCnt(&self) -> Result<usize, String>;
    fn ScheduleTuneFactors(&self, keyspace: &str) -> Result<ScheduleTuneFactors, String>;
    fn SampleIndexSizeRatio(
        &self,
        controller: &LoadDataController,
        keyspace_codec: &[u8],
    ) -> Result<f64, String>;
    fn Calculate(
        &self,
        total_real_size: i64,
        target_node_cpu_count: usize,
        index_size_ratio: f64,
        factors: ScheduleTuneFactors,
    ) -> ResourceParams;
}

/// 线程安全共享的外部存储句柄。
pub type SharedStorage = Arc<Mutex<Box<dyn Storage + Send>>>;

/// 导入控制器：持有计划、映射、存储与解析/估算依赖。
pub struct LoadDataController {
    /// 导入计划。
    pub Plan: Plan,
    /// AST 参数。
    pub ASTArgs: ASTArgs,
    /// 目标表。
    pub Table: Arc<dyn Table>,
    /// 字段映射。
    pub FieldMappings: Vec<FieldMapping>,
    /// 插入列（含 SET 列）。
    pub InsertColumns: Vec<Arc<table::Column>>,
    /// SET 表达式构建器列表。
    pub ColumnAssignments: Vec<Arc<dyn ColAssignExpressionBuilder>>,
    /// Datum 类型转换器。
    pub DatumConverter: Arc<dyn ImportDatumConverter>,
    /// 估算的解压后总大小。
    pub TotalRealSize: i64,
    /// 执行节点数。
    pub ExecuteNodesCnt: usize,
    /// 数据源存储。
    data_store: Option<SharedStorage>,
    /// 已发现的数据文件。
    data_files: Vec<SourceFileMeta>,
    /// 全局排序存储。
    global_sort_store: Option<SharedStorage>,
    /// 解析器工厂。
    parser_factory: Arc<dyn ImportParserFactory>,
    /// 大小估算器。
    size_estimator: Arc<dyn ImportSizeEstimator>,
    /// 存储工厂。
    storage_factory: Arc<dyn ImportStorageFactory>,
    /// TiKV 配置探测。
    tikv_config_probe: Arc<dyn TiKVConfigProbe>,
    /// 资源计算器。
    resource_calculator: Arc<dyn ImportResourceCalculator>,
    /// 保护 SET 表达式编译的互斥锁。
    col_assign_mu: Mutex<()>,
}

/// 构造控制器所需的一组可注入服务。
pub struct LoadDataControllerServices {
    /// Datum 类型转换器。
    /// Datum 转换服务。
    pub DatumConverter: Arc<dyn ImportDatumConverter>,
    /// 列赋值工厂。
    pub AssignmentFactory: Arc<dyn ColumnAssignmentFactory>,
    /// 解析器工厂。
    pub ParserFactory: Arc<dyn ImportParserFactory>,
    /// 大小估算服务。
    pub SizeEstimator: Arc<dyn ImportSizeEstimator>,
    /// 存储工厂。
    pub StorageFactory: Arc<dyn ImportStorageFactory>,
    /// TiKV 探测服务。
    pub TiKVConfigProbe: Arc<dyn TiKVConfigProbe>,
    /// 资源计算服务。
    pub ResourceCalculator: Arc<dyn ImportResourceCalculator>,
}

/// 控制器构造时的一次性配置闭包。
pub type ControllerOption = Box<dyn FnOnce(&mut LoadDataController) + Send>;

/// 注入已打开的数据源存储。
pub fn WithDataStore(storage: SharedStorage) -> ControllerOption {
    Box::new(move |controller| controller.data_store = Some(storage))
}

/// 注入已打开的全局排序存储。
pub fn WithGlobalSortStore(storage: SharedStorage) -> ControllerOption {
    Box::new(move |controller| controller.global_sort_store = Some(storage))
}

/// 根据计划、表与 AST 参数构造控制器并校验字段参数。
pub fn NewLoadDataController(
    plan: Plan,
    table: Arc<dyn Table>,
    ast_args: ASTArgs,
    services: LoadDataControllerServices,
    options: Vec<ControllerOption>,
) -> Result<LoadDataController, String> {
    let (field_mappings, names) = buildFieldMappings(&*table, &ast_args.ColumnsAndUserVars)?;
    let insert_columns = buildInsertColumns(&*table, &names, &ast_args.ColumnAssignments)?;
    let mut column_assignments = Vec::with_capacity(ast_args.ColumnAssignments.len());
    for assignment in &ast_args.ColumnAssignments {
        column_assignments.push(services.AssignmentFactory.BuildAssignment(assignment)?);
    }
    let mut controller = LoadDataController {
        Plan: plan,
        ASTArgs: ast_args,
        Table: table,
        FieldMappings: field_mappings,
        InsertColumns: insert_columns,
        ColumnAssignments: column_assignments,
        DatumConverter: services.DatumConverter,
        TotalRealSize: 0,
        ExecuteNodesCnt: 1,
        data_store: None,
        data_files: Vec::new(),
        global_sort_store: None,
        parser_factory: services.ParserFactory,
        size_estimator: services.SizeEstimator,
        storage_factory: services.StorageFactory,
        tikv_config_probe: services.TiKVConfigProbe,
        resource_calculator: services.ResourceCalculator,
        col_assign_mu: Mutex::new(()),
    };
    for option in options {
        option(&mut controller);
    }
    controller.checkFieldParams()?;
    Ok(controller)
}

impl LoadDataController {
    /// Parquet 时区位置；未设置时默认 UTC。
    pub fn ParquetLocation(&self) -> &str {
        if self.Plan.LocationID.is_empty() {
            "UTC"
        } else {
            &self.Plan.LocationID
        }
    }

    /// 探测并记录是否 RaftKV2。
    pub fn InitTiKVConfigs(&mut self) -> Result<(), String> {
        self.Plan.IsRaftKV2 = self.tikv_config_probe.IsRaftKV2()?;
        Ok(())
    }

    /// 由计划字段配置生成 mydump CSV 配置。
    pub fn GenerateCSVConfig(&self) -> astersql_lightning_mydump::CsvConfig {
        generateCSVConfig(
            &self.Plan.FieldNullDef,
            &self.Plan.LineFieldsInfo,
            self.Plan.InImportInto,
            self.Plan.NullValueOptEnclosed,
        )
    }

    /// 校验路径、格式与 FIELDS/LINES 约束。
    pub fn checkFieldParams(&self) -> Result<(), String> {
        if self.Plan.DataSourceType == DataSourceTypeFile && self.Plan.Path.is_empty() {
            return Err("load data path is empty".to_owned());
        }
        if self.Plan.InImportInto
            && !matches!(
                self.Plan.Format.as_str(),
                DataFormatCSV | DataFormatParquet | DataFormatSQL | DataFormatAuto
            )
        {
            return Err(format!("unsupported import format {}", self.Plan.Format));
        }
        if !self.Plan.InImportInto {
            if self.Plan.NullValueOptEnclosed
                && self.Plan.LineFieldsInfo.FieldsEnclosedBy.is_empty()
            {
                return Err(
                    "NULL DEFINED BY OPTIONALLY ENCLOSED requires FIELDS ENCLOSED BY".into(),
                );
            }
            if self.Plan.LineFieldsInfo.LinesTerminatedBy.is_empty() {
                return Err("LINES TERMINATED BY is empty".into());
            }
            if self.Plan.LineFieldsInfo.FieldsTerminatedBy.is_empty() {
                return Err("FIELDS TERMINATED BY is empty".into());
            }
        }
        let enclosed = &self.Plan.LineFieldsInfo.FieldsEnclosedBy;
        let terminated = &self.Plan.LineFieldsInfo.FieldsTerminatedBy;
        if !enclosed.is_empty()
            && (enclosed.starts_with(terminated) || terminated.starts_with(enclosed))
        {
            return Err("FIELDS ENCLOSED BY and TERMINATED BY must not prefix each other".into());
        }
        self.Plan.CheckNonCSVFormatOptions()
    }

    /// 编译 SET 赋值表达式（加锁避免并发编译）。
    pub fn CreateColAssignSimpleExprs(
        &self,
        context: &litExprContext,
    ) -> Result<(Vec<Arc<dyn ColAssignExpression>>, Vec<SQLWarn>), String> {
        let _guard = self
            .col_assign_mu
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut expressions = Vec::with_capacity(self.ColumnAssignments.len());
        let mut warnings = Vec::new();
        for assignment in &self.ColumnAssignments {
            let (expression, mut current_warnings) =
                assignment.Build(context, self.Table.UseNewCollate())?;
            expressions.push(expression);
            warnings.append(&mut current_warnings);
        }
        Ok((expressions, warnings))
    }

    /// 打开数据源与（若全局排序）cloud storage。
    pub fn InitDataStore(&mut self, context: &StorageContext) -> Result<(), String> {
        // 查询数据源无需打开文件存储。
        if self.Plan.DataSourceType == DataSourceTypeQuery {
            return Ok(());
        }
        if self.data_store.is_none() {
            let source = parse_data_source_path(&self.Plan.Path)?;
            self.data_store = Some(initExternalStore(
                context,
                &source.storage_uri,
                "IMPORT INTO data source",
                self.storage_factory.as_ref(),
            )?);
        }
        if self.Plan.IsGlobalSort() && self.global_sort_store.is_none() {
            self.global_sort_store = Some(GetSortStore(
                context,
                &self.Plan.CloudStorageURI,
                self.storage_factory.as_ref(),
            )?);
        }
        Ok(())
    }

    /// Verify source credentials before asynchronous prepare discovers all matching files.
    pub fn CheckDataSourceAccess(&self, context: &StorageContext) -> Result<(), String> {
        let source = parse_data_source_path(&self.Plan.Path)?;
        check_data_source_glob(&source.file_name_key)?;
        let storage = initExternalStore(
            context,
            &source.storage_uri,
            "IMPORT INTO data source",
            self.storage_factory.as_ref(),
        )?;
        let storage = storage
            .lock()
            .map_err(|_| "data storage lock is poisoned".to_owned())?;
        let result = if let Some(glob_index) = source
            .file_name_key
            .as_bytes()
            .iter()
            .position(|byte| matches!(byte, b'*' | b'['))
        {
            let common_prefix = if source.is_local {
                String::new()
            } else {
                source.file_name_key[..glob_index].to_owned()
            };
            let option = WalkOption {
                ObjPrefix: common_prefix,
                SkipSubDir: true,
                ..WalkOption::default()
            };
            let mut stopped_after_first_object = false;
            let walked = storage.WalkDir(context, Some(&option), &mut |remote_path, _| {
                let mut reader = storage.Open(context, remote_path, None)?;
                reader.Close()?;
                stopped_after_first_object = true;
                Err(std::io::Error::other("data-source-access-check-complete").into())
            });
            if stopped_after_first_object {
                Ok(())
            } else {
                walked.map_err(|error| format!("{}: failed to access data source", error))
            }
        } else {
            storage
                .Open(context, &source.file_name_key, None)
                .and_then(|mut reader| reader.Close().map_err(Into::into))
                .map_err(|error| format!("{}: Please check the file location is correct", error))
        };
        storage.Close();
        result
    }

    /// 枚举/匹配数据文件，估算真实大小并检测格式。
    pub fn InitDataFiles(&mut self, context: &StorageContext) -> Result<(), String> {
        // 查询数据源无需打开文件存储。
        if self.Plan.DataSourceType == DataSourceTypeQuery {
            self.data_files.clear();
            self.Plan.TotalFileSize = 0;
            self.TotalRealSize = 0;
            return Ok(());
        }
        validate_server_path(&self.Plan.Path, self.Plan.InImportInto)?;
        let source = parse_data_source_path(&self.Plan.Path)?;
        check_data_source_glob(&source.file_name_key)?;
        self.InitDataStore(context)?;
        let storage = Arc::clone(self.data_store.as_ref().expect("checked above"));
        let storage = storage
            .lock()
            .map_err(|_| "data storage lock is poisoned".to_owned())?;
        let pattern = source.file_name_key;
        let mut raw_files = Vec::new();
        // 通配符：WalkDir 过滤；否则打开单文件。
        if has_glob(&pattern) {
            storage
                .WalkDir(context, None, &mut |path, size| {
                    if glob_matches(&pattern, path) {
                        raw_files.push((path.to_owned(), size));
                    }
                    Ok(())
                })
                .map_err(|error| error.to_string())?;
        } else {
            let reader = storage
                .Open(context, &pattern, None)
                .map_err(|error| error.to_string())?;
            raw_files.push((
                pattern.clone(),
                reader.GetFileSize().map_err(|e| e.to_string())?,
            ));
        }
        raw_files.sort_by(|left, right| left.0.cmp(&right.0));
        self.data_files.clear();
        self.Plan.TotalFileSize = 0;
        self.TotalRealSize = 0;
        for (path, size) in raw_files {
            self.detectAndUpdateFormat(&path);
            let mut file = SourceFileMeta {
                path: path.clone(),
                source_type: self.getSourceType(),
                compression: compression_from_path(&path),
                file_size: size,
                ..SourceFileMeta::default()
            };
            // real_size = 估算解压大小 × 格式膨胀比。
            let expansion = estimateFormatSizeExpansionRatio(
                context,
                &file.path,
                file.file_size,
                file.source_type,
                &**storage,
                self.size_estimator.as_ref(),
            )?;
            file.real_size = ((self
                .size_estimator
                .EstimateRealSize(context, &file, &**storage)?
                as f64)
                * expansion) as i64;
            self.Plan.TotalFileSize = self.Plan.TotalFileSize.saturating_add(file.file_size);
            self.TotalRealSize = self.TotalRealSize.saturating_add(file.real_size);
            self.data_files.push(file);
        }
        Ok(())
    }

    /// 为每个数据文件构造 opener 与远程元信息。
    pub fn GetLoadDataReaderInfos(&self) -> Result<Vec<LoadDataReaderInfo>, String> {
        let storage = self
            .data_store
            .as_ref()
            .ok_or_else(|| "data storage is not initialized".to_owned())?;
        Ok(self
            .data_files
            .iter()
            .cloned()
            .map(|file| {
                let shared = Arc::clone(storage);
                let path = file.path.clone();
                LoadDataReaderInfo {
                    Opener: Arc::new(move |context| {
                        let guard = shared
                            .lock()
                            .map_err(|_| "data storage lock is poisoned".to_owned())?;
                        let mut reader = guard
                            .Open(context, &path, None)
                            .map_err(|error| error.to_string())?;
                        let mut bytes = Vec::new();
                        reader
                            .seek(SeekFrom::Start(0))
                            .and_then(|_| reader.read_to_end(&mut bytes))
                            .map_err(|error| error.to_string())?;
                        Ok(Box::new(std::io::Cursor::new(bytes)) as Box<dyn ReadSeekCloser>)
                    }),
                    Remote: Some(file),
                }
            })
            .collect())
    }

    /// 打开指定文件的解析器并跳过 IgnoreLines 行。
    pub fn GetParser(
        &self,
        context: &StorageContext,
        file: &SourceFileMeta,
    ) -> Result<Box<dyn MydumpParser>, String> {
        let info = self
            .GetLoadDataReaderInfos()?
            .into_iter()
            .find(|info| {
                info.Remote
                    .as_ref()
                    .is_some_and(|remote| remote.path == file.path)
            })
            .ok_or_else(|| format!("data file {} is not initialized", file.path))?;
        if self.Plan.Format == DataFormatParquet {
            return self
                .OpenParquetParser(context, file)
                .map(|parser| parser as Box<dyn MydumpParser>);
        }
        let reader = (info.Opener)(context)?;
        let mut parser =
            self.parser_factory
                .NewParser(&self.Plan.Format, reader, file, &self.Plan)?;
        HandleSkipNRows(parser.as_mut(), self.Plan.IgnoreLines)?;
        Ok(parser)
    }

    /// Open Parquet from bounded streams; exact small files bypass the footer opener.
    pub fn OpenParquetParser(
        &self,
        context: &StorageContext,
        file: &SourceFileMeta,
    ) -> Result<Box<dyn MydumpParser + Send>, String> {
        let parser = self.OpenParquetFile(context, file)?;
        Ok(Box::new(
            astersql_dumpformat_parquetfile::file_parser::ImportParser::new(parser),
        ))
    }
    pub(crate) fn OpenParquetFile(
        &self,
        context: &StorageContext,
        file: &SourceFileMeta,
    ) -> Result<astersql_dumpformat_parquetfile::file_parser::FileParser, String> {
        let storage = self
            .data_store
            .as_ref()
            .ok_or_else(|| "data storage is not initialized".to_string())?
            .clone();
        open_parquet_file(storage, context, file, self.ParquetLocation())
    }
    pub fn OpenParquetParserWithLocation(
        &self,
        context: &StorageContext,
        file: &SourceFileMeta,
        location: &str,
    ) -> Result<Box<dyn MydumpParser + Send>, String> {
        let storage = self
            .data_store
            .as_ref()
            .ok_or_else(|| "data storage is not initialized".to_string())?
            .clone();
        let parser = open_parquet_file(storage, context, file, location)?;
        Ok(Box::new(
            astersql_dumpformat_parquetfile::file_parser::ImportParser::new(parser),
        ))
    }

    /// 返回已初始化的数据文件列表。
    pub fn DataFiles(&self) -> &[SourceFileMeta] {
        &self.data_files
    }

    /// 转换为 Lightning mydump FileInfo 列表。
    pub fn toMyDumpFiles(&self) -> Vec<astersql_lightning_mydump::FileInfo> {
        self.data_files
            .iter()
            .map(|file| astersql_lightning_mydump::FileInfo {
                file_meta: astersql_lightning_mydump::FileMeta {
                    path: file.path.clone(),
                    file_size: file.file_size,
                    real_size: file.real_size,
                    source_type: file.source_type,
                    compression: file.compression,
                    sort_key: file.sort_key.clone(),
                },
                extend_data: file.extend_data.clone(),
            })
            .collect()
    }

    /// 按真实大小与索引占比计算线程/节点/扫描并发。
    pub fn CalResourceParams(&mut self, keyspace_codec: &[u8]) -> Result<(), String> {
        let target_cpu = self.resource_calculator.TargetNodeCPUCnt()?;
        let factors = self
            .resource_calculator
            .ScheduleTuneFactors(&self.Plan.Keyspace)?;
        let index_count = self
            .Plan
            .TableInfo
            .as_deref()
            .map_or(0, crate::GetNumOfIndexGenKV);
        // 无二级索引则跳过采样。
        let index_ratio = if index_count == 0 {
            0.0
        } else {
            self.resource_calculator
                .SampleIndexSizeRatio(self, keyspace_codec)
                .unwrap_or(0.0)
        };
        let parameters = self.resource_calculator.Calculate(
            self.TotalRealSize,
            target_cpu,
            index_ratio,
            factors,
        );
        self.Plan.ThreadCnt = parameters.ThreadCnt;
        self.Plan.MaxNodeCnt = parameters.MaxNodeCnt;
        self.Plan.DistSQLScanConcurrency = parameters.DistSQLScanConcurrency;
        Ok(())
    }

    /// 组装本地 Lightning backend 配置。
    pub fn getLocalBackendCfg(
        &self,
        keyspace: &str,
        pd_address: &str,
        data_directory: &str,
    ) -> LocalBackendConfig {
        LocalBackendConfig {
            PDAddr: pd_address.to_owned(),
            LocalStoreDir: data_directory.to_owned(),
            MaxConnPerStore: 16,
            WorkerConcurrency: self.Plan.ThreadCnt as i32,
            KVWriteBatchSize: 1 << 20,
            RegionSplitBatchSize: 4096,
            RegionSplitConcurrency: std::thread::available_parallelism()
                .map(|parallelism| parallelism.get())
                .unwrap_or(1),
            CheckpointEnabled: false,
            MemTableSize: 64 << 20,
            LocalWriterMemCacheSize: 128 << 20,
            ShouldCheckTiKV: true,
            DupeDetectEnabled: false,
            StoreWriteBWLimit: self.Plan.MaxWriteSpeed.0.max(0) as usize,
            KeyspaceName: keyspace.to_owned(),
            RaftKV2SwitchMode: self.Plan.IsRaftKV2,
            DisableAutomaticCompactions: true,
            BlockSize: 16 << 10,
        }
    }

    /// 返回 `库.表` 全名。
    pub fn FullTableName(&self) -> String {
        format!("{}.{}", self.Plan.DBName, self.Table.Meta().Name.O)
    }

    /// 格式为 auto 时按路径后缀回写 Format。
    pub fn detectAndUpdateFormat(&mut self, path: &str) {
        if self.Plan.Format == DataFormatAuto {
            if let Some(format) = parseFileType(path) {
                self.Plan.Format = format.to_owned();
                if let Some(parameters) = self.Plan.Parameters.as_mut() {
                    parameters.Format = format.to_owned();
                }
            }
        }
    }

    /// 将计划格式映射为 mydump SourceType。
    pub fn getSourceType(&self) -> SourceType {
        match self.Plan.Format.as_str() {
            DataFormatSQL => SourceType::Sql,
            DataFormatParquet => SourceType::Parquet,
            DataFormatCSV | DataFormatDelimitedData => SourceType::Csv,
            _ => SourceType::Ignore,
        }
    }

    /// 关闭数据源与全局排序存储。
    pub fn Close(&mut self) {
        if let Some(storage) = &self.data_store {
            if let Ok(mut storage) = storage.lock() {
                storage.Close();
            }
        }
        if let Some(storage) = &self.global_sort_store {
            if let Ok(mut storage) = storage.lock() {
                storage.Close();
            }
        }
    }
}

/// 从 LOAD DATA AST 视图构造导入计划。
pub fn NewPlanFromLoadDataPlan(
    session: &dyn ImportSessionContext,
    load: &dyn ResolvedLoadDataPlan,
) -> Result<Plan, String> {
    let mut plan = Plan::default();
    plan.DBName = load.DBName().to_owned();
    plan.DBID = load.DBID();
    plan.Path = load.Path().to_owned();
    plan.Format = DataFormatDelimitedData.to_owned();
    plan.Restrictive = session.SQLMode().HasStrictMode()
        && load.OnDuplicate() != ast::OnDuplicateKeyHandlingType::Ignore;
    plan.SQLMode = session.SQLMode();
    plan.Charset = load
        .Charset()
        .map(str::to_owned)
        .or_else(|| session.SystemVariable("character_set_database"));
    plan.ImportantSysVars = getImportantSysVars(session);
    plan.LineFieldsInfo = lineFieldsInfoFromAST(load.FieldsInfo(), load.LinesInfo());
    plan.IgnoreLines = load.IgnoreLines().unwrap_or(0);
    if let Some(fields) = load.FieldsInfo() {
        if let Some(null) = &fields.DefinedNullBy {
            plan.FieldNullDef.push(null.clone());
            plan.NullValueOptEnclosed = fields.NullValueOptEnclosed;
        }
    }
    if plan.FieldNullDef.is_empty() && !plan.LineFieldsInfo.FieldsEnclosedBy.is_empty() {
        plan.FieldNullDef.push("NULL".into());
    }
    if let Some(escape) = plan.LineFieldsInfo.FieldsEscapedBy.as_bytes().first() {
        plan.FieldNullDef
            .push(String::from_utf8_lossy(&[*escape, b'N']).into_owned());
    }
    plan.DistSQLScanConcurrency = session.DistSQLScanConcurrency();
    plan.DataSourceType = DataSourceTypeFile;
    Ok(plan)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 本地导入引擎（Lightning local backend）配置。
pub struct LocalBackendConfig {
    /// PD 地址。
    pub PDAddr: String,
    /// 本地引擎目录。
    pub LocalStoreDir: String,
    /// 每 store 最大连接数。
    pub MaxConnPerStore: usize,
    /// worker 并发度。
    pub WorkerConcurrency: i32,
    /// KV 写批大小。
    pub KVWriteBatchSize: i64,
    /// Region 分裂批大小。
    pub RegionSplitBatchSize: usize,
    /// Region 分裂并发。
    pub RegionSplitConcurrency: usize,
    /// 是否启用 checkpoint。
    pub CheckpointEnabled: bool,
    /// memtable 大小。
    pub MemTableSize: usize,
    /// 本地 writer 内存缓存。
    pub LocalWriterMemCacheSize: i64,
    /// 是否检查 TiKV 状态。
    pub ShouldCheckTiKV: bool,
    /// 是否启用重复检测。
    pub DupeDetectEnabled: bool,
    /// store 写带宽限制。
    pub StoreWriteBWLimit: usize,
    /// keyspace 名。
    pub KeyspaceName: String,
    /// 是否切换 RaftKV2 模式。
    pub RaftKV2SwitchMode: bool,
    /// 禁用自动 compaction。
    pub DisableAutomaticCompactions: bool,
    /// SST block 大小。
    pub BlockSize: i32,
}

/// 打开全局排序用的 cloud storage。
pub fn GetSortStore(
    context: &StorageContext,
    uri: &str,
    factory: &dyn ImportStorageFactory,
) -> Result<SharedStorage, String> {
    if uri.trim().is_empty() {
        return Err("cloud storage URI is empty".into());
    }
    initExternalStore(context, uri, "cloud storage", factory)
}

/// 校验 URI 非空后通过工厂打开外部存储。
pub fn initExternalStore(
    context: &StorageContext,
    uri: &str,
    target: &str,
    factory: &dyn ImportStorageFactory,
) -> Result<SharedStorage, String> {
    let uri = uri.trim();
    if uri.is_empty() {
        return Err(format!("{target} URI is empty"));
    }
    factory
        .Open(context, uri, target)
        .map_err(|error| format!("cannot access {target}: {error}"))
}

/// 估算格式膨胀比；非 Parquet 为 1.0。
pub fn estimateFormatSizeExpansionRatio(
    context: &StorageContext,
    file_path: &str,
    file_size: i64,
    source_type: SourceType,
    storage: &dyn Storage,
    estimator: &dyn ImportSizeEstimator,
) -> Result<f64, String> {
    if source_type != SourceType::Parquet {
        return Ok(1.0);
    }
    if file_size <= 0 {
        return Ok(2.0);
    }
    Ok(estimator
        .ParquetExpansionRatio(context, file_path, file_size, storage)?
        .max(1.0))
}

/// 压缩比采样达到该数量后固化为调和平均。
pub const maxSampledCompressedFiles: usize = 512;

#[derive(Default)]
/// 按压缩类型采样并缓存膨胀比。
pub struct compressionEstimator {
    records: Mutex<Vec<(Compression, Vec<f64>)>>,
    ratios: Mutex<Vec<(Compression, f64)>>,
}

impl compressionEstimator {
    /// 采样或返回已缓存的压缩膨胀比。
    pub fn estimate(
        &self,
        compression: Compression,
        sample: impl FnOnce() -> Result<f64, String>,
    ) -> f64 {
        if compression == Compression::None {
            return 1.0;
        }
        if let Some(ratio) = self
            .ratios
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find_map(|(kind, ratio)| (*kind == compression).then_some(*ratio))
        {
            return ratio;
        }
        let sampled = match sample() {
            Ok(ratio) if ratio > 0.0 => ratio,
            _ => return 1.0,
        };
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = if let Some(index) = records.iter().position(|(kind, _)| *kind == compression)
        {
            &mut records[index].1
        } else {
            records.push((compression, Vec::new()));
            &mut records.last_mut().expect("record inserted above").1
        };
        if current.len() < maxSampledCompressedFiles {
            current.push(sampled);
        }
        if current.len() >= maxSampledCompressedFiles {
            let ratio = getHarmonicMean(current);
            self.ratios
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((compression, ratio));
            ratio
        } else {
            sampled
        }
    }
}

/// 创建空的压缩比记录器。
pub fn newCompressionRecorder() -> compressionEstimator {
    compressionEstimator::default()
}

/// 提供本地/分布式目标节点 CPU 信息。
pub trait TargetNodeCPUProvider {
    fn LocalCPUCount(&self) -> usize;
    fn DistributedTaskEnabled(&self) -> bool;
    fn TargetNodeCPUCount(&self) -> Result<usize, String>;
}

/// 按数据源与路径选择本地 CPU 或分布式目标 CPU。
pub fn GetTargetNodeCPUCnt(
    source_type: DataSourceType,
    path: &str,
    provider: &dyn TargetNodeCPUProvider,
) -> Result<usize, String> {
    if source_type == DataSourceTypeQuery {
        return Ok(provider.LocalCPUCount().max(1));
    }

    let parsed = astersql_objstore::parse::ParseRawURL(path)
        .map_err(|error| format!("invalid import URI {path:?}: {error}"))?;
    if astersql_objstore::parse::IsLocal(&parsed) || !provider.DistributedTaskEnabled() {
        return Ok(provider.LocalCPUCount().max(1));
    }
    provider.TargetNodeCPUCount().map(|count| count.max(1))
}

/// 从 IMPORT INTO AST 构造计划并初始化选项/参数。
pub fn NewImportPlan(
    session: &dyn ImportSessionContext,
    import: &dyn ResolvedImportIntoPlan,
    table: &dyn Table,
    target_node_cpu_count: usize,
) -> Result<Plan, String> {
    let mut plan = Plan::default();
    plan.TableInfo = Some(Arc::new(table.Meta().clone()));
    plan.DesiredTableInfo = plan.TableInfo.clone();
    plan.DBName = import.DBName().to_owned();
    plan.DBID = import.DBID();
    plan.Path = import.Path().to_owned();
    plan.Format = import
        .Format()
        .unwrap_or(DataFormatAuto)
        .to_ascii_lowercase();
    plan.Restrictive = session.SQLMode().HasStrictMode();
    plan.LocationID = session.LocationID();
    plan.SQLMode = session.SQLMode();
    plan.ImportantSysVars = getImportantSysVars(session);
    plan.DistSQLScanConcurrency = session.DistSQLScanConcurrency();
    plan.InImportInto = true;
    plan.DataSourceType = getDataSourceType(import);
    plan.User = session.User();
    plan.Keyspace = session.Keyspace();
    plan.FieldNullDef = defaultFieldNullDef
        .iter()
        .map(|value| (*value).into())
        .collect();
    plan.setUseNewCollate(table.UseNewCollate());
    plan.initDefaultOptions(target_node_cpu_count);
    plan.CloudStorageURI = session.DefaultCloudStorageURI();
    initOptions(&mut plan, session, import.Options())?;
    plan.adjustOptions(target_node_cpu_count);
    plan.initParameters(session, import)?;
    plan.CheckNonCSVFormatOptions()?;
    Ok(plan)
}

impl Plan {
    /// 将列列表、SET、选项序列化为 ImportParameters。
    pub fn initParameters(
        &mut self,
        session: &dyn ImportSessionContext,
        import: &dyn ResolvedImportIntoPlan,
    ) -> Result<(), String> {
        let columns_and_variables = if import.ColumnsAndUserVars().is_empty() {
            String::new()
        } else {
            let formatted = import
                .ColumnsAndUserVars()
                .iter()
                .map(|value| session.FormatColumnOrUserVariable(value))
                .collect::<Result<Vec<_>, _>>()?;
            format!("({})", formatted.join(", "))
        };
        let set_clause = import
            .ColumnAssignments()
            .iter()
            .map(|value| session.FormatAssignment(value))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        let mut options = HashMap::with_capacity(import.Options().len());
        for option in import.Options() {
            let value = match session.EvalLoadDataOption(option)? {
                ImportOptionValue::String(value) => {
                    if option.Name.eq_ignore_ascii_case("cloud_storage_uri") {
                        session.RedactURL(&value)
                    } else {
                        value
                    }
                }
                ImportOptionValue::Integer(value) => value.to_string(),
                ImportOptionValue::Boolean(value) => value.to_string(),
            };
            options.insert(option.Name.clone(), value);
        }
        self.Parameters = Some(ImportParameters {
            ColumnsAndVars: columns_and_variables,
            SetClause: set_clause,
            FileLocation: session.RedactURL(&self.Path),
            Format: self.Format.clone(),
            Options: options,
        });
        Ok(())
    }
}

/// CSV 风格默认字段分隔（逗号/双引号/反斜杠）。
pub fn newDefaultLineFieldsInfo() -> LineFieldsInfo {
    LineFieldsInfo {
        FieldsTerminatedBy: ",".into(),
        FieldsEnclosedBy: "\"".into(),
        FieldsEscapedBy: "\\".into(),
        FieldsOptEnclosed: false,
        LinesStartingBy: String::new(),
        LinesTerminatedBy: String::new(),
    }
}

/// 从 AST FIELDS/LINES 子句填充分隔配置（LOAD DATA 默认制表符）。
fn lineFieldsInfoFromAST(
    fields: Option<&ast::FieldsClause>,
    lines: Option<&ast::LinesClause>,
) -> LineFieldsInfo {
    let mut result = LineFieldsInfo {
        FieldsTerminatedBy: "\t".into(),
        FieldsEscapedBy: "\\".into(),
        LinesTerminatedBy: "\n".into(),
        ..LineFieldsInfo::default()
    };
    if let Some(fields) = fields {
        if let Some(value) = &fields.Terminated {
            result.FieldsTerminatedBy = value.clone();
        }
        if let Some(value) = &fields.Enclosed {
            result.FieldsEnclosedBy = value.clone();
        }
        if let Some(value) = &fields.Escaped {
            result.FieldsEscapedBy = value.clone();
        }
        result.FieldsOptEnclosed = fields.OptEnclosed;
    }
    if let Some(lines) = lines {
        if let Some(value) = &lines.Starting {
            result.LinesStartingBy = value.clone();
        }
        if let Some(value) = &lines.Terminated {
            result.LinesTerminatedBy = value.clone();
        }
    }
    result
}

/// 从 LOAD DATA 计划提取 ASTArgs。
pub fn ASTArgsFromPlan(plan: &dyn ResolvedLoadDataPlan) -> ASTArgs {
    ASTArgs {
        FileLocRef: plan.FileLocRef(),
        ColumnsAndUserVars: plan.ColumnsAndUserVars().to_vec(),
        ColumnAssignments: plan.ColumnAssignments().to_vec(),
        OnDuplicate: plan.OnDuplicate(),
        FieldsInfo: plan.FieldsInfo().cloned(),
        LinesInfo: plan.LinesInfo().cloned(),
    }
}

/// 从 IMPORT INTO 计划提取 ASTArgs。
pub fn ASTArgsFromImportPlan(plan: &dyn ResolvedImportIntoPlan) -> ASTArgs {
    ASTArgs {
        FileLocRef: ast::FileLocRef::ServerOrRemote,
        ColumnsAndUserVars: plan.ColumnsAndUserVars().to_vec(),
        ColumnAssignments: plan.ColumnAssignments().to_vec(),
        OnDuplicate: ast::OnDuplicateKeyHandlingType::Replace,
        FieldsInfo: None,
        LinesInfo: None,
    }
}

/// 解析 IMPORT INTO 语句文本为 ASTArgs。
pub fn ASTArgsFromStmt(statement: &str) -> Result<ASTArgs, String> {
    let node = astersql_parser::New()
        .ParseOneStmt(statement, "", "")
        .map_err(|error| error.to_string())?;
    let import = node
        .into_any()
        .downcast::<ast::ImportIntoStmt>()
        .map_err(|_| format!("statement is not IMPORT INTO: {statement}"))?;
    Ok(ASTArgs {
        FileLocRef: ast::FileLocRef::ServerOrRemote,
        ColumnsAndUserVars: import.ColumnsAndUserVars,
        ColumnAssignments: import.ColumnAssignments,
        OnDuplicate: ast::OnDuplicateKeyHandlingType::Replace,
        FieldsInfo: None,
        LinesInfo: None,
    })
}

/// 将表全部可见列映射为字段映射。
pub fn tableVisCols2FieldMappings(table: &dyn Table) -> (Vec<FieldMapping>, Vec<String>) {
    let columns = table.VisibleCols();
    let mappings = columns
        .iter()
        .cloned()
        .map(|column| FieldMapping {
            Column: Some(column),
            UserVar: None,
        })
        .collect();
    let names = columns
        .iter()
        .map(|column| column.ColumnInfo.Name.O.clone())
        .collect();
    (mappings, names)
}

/// 按列列表（可含用户变量）构建字段映射；空列表则用可见列。
pub fn buildFieldMappings(
    table: &dyn Table,
    columns_and_user_variables: &[ast::ColumnNameOrUserVar],
) -> Result<(Vec<FieldMapping>, Vec<String>), String> {
    if columns_and_user_variables.is_empty() {
        return Ok(tableVisCols2FieldMappings(table));
    }
    let visible = table.VisibleCols();
    let mut mappings = Vec::with_capacity(columns_and_user_variables.len());
    let mut names = Vec::new();
    for item in columns_and_user_variables {
        let column = item
            .ColumnName
            .as_ref()
            .and_then(|name| table::FindCol(&visible, &name.Name.O));
        let user_variable = match item.UserVar.as_ref().map(|expr| &expr.Kind) {
            Some(ast::ExprKind::Variable { Name, .. }) => Some(UserVariable { Name: Name.clone() }),
            Some(_) => return Err("column list user variable is not a variable expression".into()),
            None => None,
        };
        if let Some(name) = &item.ColumnName {
            if column.is_none() {
                return Err(format!("unknown column {}", name.Name.O));
            }
            names.push(name.Name.O.clone());
        }
        mappings.push(FieldMapping {
            Column: column,
            UserVar: user_variable,
        });
    }
    Ok((mappings, names))
}

/// 按名称列表重排列顺序。
pub fn reorderColumnsByNames(
    columns: Vec<Arc<table::Column>>,
    names: &[String],
) -> Result<Vec<Arc<table::Column>>, String> {
    if columns.len() != names.len() {
        return Err("column count does not match column name count".into());
    }
    if names.is_empty() {
        return Ok(columns);
    }
    let positions = names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.to_ascii_lowercase(), index))
        .collect::<HashMap<_, _>>();
    let mut ordered = vec![None; columns.len()];
    for column in columns {
        let position = positions
            .get(&column.ColumnInfo.Name.L)
            .copied()
            .ok_or_else(|| format!("column {} is not in insert list", column.ColumnInfo.Name.O))?;
        if ordered[position].replace(column).is_some() {
            return Err("duplicate insert column".into());
        }
    }
    ordered
        .into_iter()
        .map(|column| column.ok_or_else(|| "missing reordered column".into()))
        .collect()
}

/// 合并列列表与 SET 赋值列，去重并解析为 Column。
pub fn buildInsertColumns(
    table: &dyn Table,
    column_names: &[String],
    assignments: &[ast::Assignment],
) -> Result<Vec<Arc<table::Column>>, String> {
    let visible = table.VisibleCols();
    let mut names = column_names.to_vec();
    names.extend(
        assignments
            .iter()
            .map(|assignment| assignment.Column.Name.O.clone()),
    );
    let mut seen = HashSet::new();
    let mut columns = Vec::with_capacity(names.len());
    for name in names {
        let lower = name.to_ascii_lowercase();
        if !seen.insert(lower) {
            return Err(format!("column {name} specified twice"));
        }
        columns
            .push(table::FindCol(&visible, &name).ok_or_else(|| format!("unknown column {name}"))?);
    }
    Ok(columns)
}

/// 组装 mydump CsvConfig。
pub fn generateCSVConfig(
    field_null_def: &[String],
    line_fields: &LineFieldsInfo,
    in_import_into: bool,
    null_value_optionally_enclosed: bool,
) -> astersql_lightning_mydump::CsvConfig {
    astersql_lightning_mydump::CsvConfig {
        fields_terminated_by: line_fields.FieldsTerminatedBy.clone(),
        fields_enclosed_by: line_fields.FieldsEnclosedBy.clone(),
        lines_terminated_by: line_fields.LinesTerminatedBy.clone(),
        lines_starting_by: line_fields.LinesStartingBy.clone(),
        fields_escaped_by: line_fields.FieldsEscapedBy.clone(),
        null: field_null_def.first().cloned().unwrap_or_default(),
        // IMPORT INTO uses the parser defaults. LOAD DATA additionally allows
        // empty lines and treats quoted NULL as text unless OPTIONALLY ENCLOSED
        // was requested, matching Go's generateCSVConfig.
        allow_empty_line: !in_import_into,
        quoted_null_is_text: !in_import_into && !null_value_optionally_enclosed,
        unescaped_quote: !in_import_into,
        ..astersql_lightning_mydump::CsvConfig::default()
    }
}

/// 跳过解析器前 ignore_lines 行（对应 SKIP N）。
pub fn HandleSkipNRows(parser: &mut dyn MydumpParser, ignore_lines: u64) -> Result<(), String> {
    for _ in 0..ignore_lines {
        parser.ReadRow().map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// 按路径后缀（去掉压缩后缀后）推断数据格式。
pub fn parseFileType(path: &str) -> Option<&'static str> {
    let normalized = path.to_ascii_lowercase();
    let lower = strip_compression_suffix(&normalized);
    if lower.ends_with(".csv") {
        Some(DataFormatCSV)
    } else if lower.ends_with(".sql") {
        Some(DataFormatSQL)
    } else if lower.ends_with(".parquet") {
        Some(DataFormatParquet)
    } else {
        // Go's parser treats an unknown or extensionless file as CSV.
        Some(DataFormatCSV)
    }
}

/// 计算正数样本的调和平均数；空则返回 1.0。
pub fn getHarmonicMean(ratios: &[f64]) -> f64 {
    let (count, inverse_sum) = ratios
        .iter()
        .copied()
        .filter(|ratio| *ratio > 0.0)
        .fold((0_usize, 0.0), |(count, sum), ratio| {
            (count + 1, sum + 1.0 / ratio)
        });
    if count == 0 {
        1.0
    } else {
        count as f64 / inverse_sum
    }
}

/// 收集影响编码的重要系统变量（带默认值）。
pub fn getImportantSysVars(session: &dyn ImportSessionContext) -> HashMap<String, String> {
    const DEFAULTS: &[(&str, &str)] = &[
        ("max_allowed_packet", "67108864"),
        ("div_precision_increment", "4"),
        ("time_zone", "SYSTEM"),
        ("default_week_format", "0"),
        ("block_encryption_mode", "aes-128-ecb"),
        ("group_concat_max_len", "1024"),
    ];
    DEFAULTS
        .iter()
        .map(|(name, default)| {
            (
                (*name).to_owned(),
                session
                    .SystemVariable(name)
                    .unwrap_or_else(|| (*default).into()),
            )
        })
        .collect()
}

/// 有 SELECT 计划则为 Query，否则 File。
pub fn getDataSourceType(plan: &dyn ResolvedImportIntoPlan) -> DataSourceType {
    if plan.HasSelectPlan() {
        DataSourceTypeQuery
    } else {
        DataSourceTypeFile
    }
}

/// 解析并应用 IMPORT INTO 选项，校验 SEM/数据源约束。
fn initOptions(
    plan: &mut Plan,
    session: &dyn ImportSessionContext,
    options: &[ast::LoadDataOpt],
) -> Result<(), String> {
    let mut names = HashSet::with_capacity(options.len());
    for option in options {
        let name = option.Name.to_ascii_lowercase();
        let expects_value = option_expects_value(&name)
            .ok_or_else(|| format!("unknown IMPORT INTO option {name}"))?;
        if expects_value != option.Value.is_some() {
            return Err(format!(
                "invalid presence of value for IMPORT INTO option {name}"
            ));
        }
        if !names.insert(name.clone()) {
            return Err(format!("duplicated IMPORT INTO option {name}"));
        }
    }
    // nextgen + SEM：限制查询导入、本地排序及部分选项。
    if session.IsNextGen() && session.SEMEnabled() {
        if plan.DataSourceType == DataSourceTypeQuery {
            return Err("IMPORT INTO from query is not supported by nextgen SEM".into());
        }
        if plan.IsLocalSort() {
            return Err("IMPORT INTO with local sort is not supported by nextgen SEM".into());
        }
        for name in [
            "disk_quota",
            "max_write_speed",
            "cloud_storage_uri",
            "thread",
            "checksum_table",
            "record_errors",
        ] {
            if names.contains(name) {
                return Err(format!("option {name} is not supported by nextgen kernel"));
            }
        }
    }
    if session.SEMEnabled() {
        for name in [
            "__max_engine_size",
            "__force_merge_step",
            "__manual_recovery",
        ] {
            if names.contains(name) {
                return Err(format!(
                    "option {name} is not supported while SEM is enabled"
                ));
            }
        }
    }
    // 查询导入仅允许 thread / disable_precheck / disk_quota。
    if plan.DataSourceType == DataSourceTypeQuery {
        if let Some(name) = names.iter().find(|name| !is_option_allowed_for_query(name)) {
            return Err(format!(
                "option {name} is not supported for import from query"
            ));
        }
    }
    for option in options {
        let name = option.Name.to_ascii_lowercase();
        plan.SpecifiedOptionNames.insert(name.clone());
        let value = if option.Value.is_some() {
            session.EvalLoadDataOption(option)?
        } else {
            ImportOptionValue::Boolean(true)
        };
        match (name.as_str(), value) {
            ("character_set", ImportOptionValue::String(value)) if is_supported_charset(&value) => {
                plan.Charset = Some(value)
            }
            ("fields_terminated_by", ImportOptionValue::String(value)) if !value.is_empty() => {
                plan.LineFieldsInfo.FieldsTerminatedBy = value
            }
            ("fields_enclosed_by", ImportOptionValue::String(value))
                if value.chars().count() <= 1 =>
            {
                plan.LineFieldsInfo.FieldsEnclosedBy = value
            }
            ("fields_escaped_by", ImportOptionValue::String(value))
                if value.chars().count() <= 1 =>
            {
                plan.LineFieldsInfo.FieldsEscapedBy = value
            }
            ("fields_defined_null_by", ImportOptionValue::String(value)) => {
                plan.FieldNullDef = vec![value]
            }
            ("lines_terminated_by", ImportOptionValue::String(value)) if !value.is_empty() => {
                plan.LineFieldsInfo.LinesTerminatedBy = value
            }
            ("skip_rows", ImportOptionValue::Integer(value)) if value >= 0 => {
                plan.IgnoreLines = value as u64
            }
            ("group_key", ImportOptionValue::String(value))
                if !value.is_empty() && value.len() <= 256 =>
            {
                plan.GroupKey = value
            }
            ("disk_quota", ImportOptionValue::String(value)) => {
                let size = parseByteSize(&value)?;
                if size <= 0 {
                    return Err("disk_quota must be positive".into());
                }
                plan.DiskQuota = ByteSize(size)
            }
            ("thread", ImportOptionValue::Integer(value)) if value > 0 => {
                plan.ThreadCnt = value as usize
            }
            ("max_write_speed", ImportOptionValue::String(value)) => {
                let size = parseByteSize(&value)?;
                if size < 0 {
                    return Err("max_write_speed must not be negative".into());
                }
                plan.MaxWriteSpeed = ByteSize(size)
            }
            ("record_errors", ImportOptionValue::Integer(value)) if value >= -1 => {
                plan.MaxRecordedErrors = value
            }
            ("on_duplicate_key", ImportOptionValue::String(value)) => {
                plan.OnDupKey = match value.to_ascii_lowercase().as_str() {
                    "capture" => OnDupKeyModeCapture,
                    "error" => OnDupKeyModeError,
                    _ => return Err(format!("invalid value for IMPORT INTO option {name}")),
                }
            }
            ("cloud_storage_uri", ImportOptionValue::String(value))
                if value.is_empty() || is_supported_cloud_uri(&value) =>
            {
                plan.CloudStorageURI = value
            }
            ("checksum_table", ImportOptionValue::String(value)) => plan
                .Checksum
                .FromStringValue(&value)
                .map_err(|error| error.to_string())?,
            ("split_file", ImportOptionValue::Boolean(value)) => plan.SplitFile = value,
            ("detached", ImportOptionValue::Boolean(value)) => plan.Detached = value,
            ("disable_tikv_import_mode", ImportOptionValue::Boolean(value)) => {
                plan.DisableTiKVImportMode = value
            }
            ("disable_precheck", ImportOptionValue::Boolean(value)) => plan.DisablePrecheck = value,
            ("__force_merge_step", ImportOptionValue::Boolean(value)) => {
                plan.ForceMergeStep = value
            }
            ("__manual_recovery", ImportOptionValue::Boolean(value)) => plan.ManualRecovery = value,
            ("__max_engine_size", ImportOptionValue::String(value)) => {
                let size = parseByteSize(&value)?;
                if size < 0 {
                    return Err("__max_engine_size must not be negative".into());
                }
                plan.MaxEngineSize = ByteSize(size)
            }
            _ => return Err(format!("invalid value for IMPORT INTO option {name}")),
        }
    }
    // 本地排序不支持 on_duplicate_key。
    if plan.SpecifiedOptionNames.contains("on_duplicate_key") && plan.IsLocalSort() {
        return Err("on_duplicate_key is not supported with local sort".into());
    }
    if plan.SplitFile && plan.IgnoreLines > 1 {
        return Err("skip_rows must be <= 1 when split_file is enabled".into());
    }
    if plan.SplitFile && plan.LineFieldsInfo.LinesTerminatedBy.is_empty() {
        return Err("lines_terminated_by must not be empty when split_file is enabled".into());
    }
    plan.MaxNodeCnt = session.MaxDistTaskNodes();
    if plan.MaxNodeCnt == -1 {
        plan.MaxNodeCnt = session.AutoMaxDistTaskNodes()?;
    }
    Ok(())
}

pub(crate) fn is_option_allowed_for_query(name: &str) -> bool {
    matches!(name, "thread" | "disable_precheck" | "disk_quota")
}

/// 返回选项是否需要取值；未知选项返回 None。
fn option_expects_value(name: &str) -> Option<bool> {
    Some(match name {
        "character_set"
        | "fields_terminated_by"
        | "fields_enclosed_by"
        | "fields_escaped_by"
        | "fields_defined_null_by"
        | "lines_terminated_by"
        | "skip_rows"
        | "group_key"
        | "disk_quota"
        | "thread"
        | "max_write_speed"
        | "checksum_table"
        | "record_errors"
        | "on_duplicate_key"
        | "cloud_storage_uri"
        | "__max_engine_size" => true,
        "split_file"
        | "detached"
        | "disable_tikv_import_mode"
        | "disable_precheck"
        | "__force_merge_step"
        | "__manual_recovery" => false,
        _ => return None,
    })
}

/// 是否为支持的字符集名。
fn is_supported_charset(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "utf8" | "utf8mb4" | "binary" | "latin1" | "ascii" | "gb18030" | "auto"
    )
}

/// 是否为支持的 cloud storage URI（s3/gcs/azure 等）。
fn is_supported_cloud_uri(value: &str) -> bool {
    let Some((scheme, authority_and_path)) = value.split_once("://") else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "s3" | "gcs" | "gs" | "azure" | "azblob"
    ) && authority_and_path
        .split('/')
        .next()
        .is_some_and(|host| !host.is_empty())
}

/// 解析带单位的字节大小字符串（KiB/MiB/GiB 等）。
/// Parse the canonical IMPORT INTO byte-size option for SQL host adapters.
pub fn parseByteSize(value: &str) -> Result<i64, String> {
    let normalized = value.trim().to_ascii_lowercase();
    let units = [
        ("tib", 1_i64 << 40),
        ("tb", 1_i64 << 40),
        ("gib", 1_i64 << 30),
        ("gb", 1_i64 << 30),
        ("mib", 1_i64 << 20),
        ("mb", 1_i64 << 20),
        ("kib", 1_i64 << 10),
        ("kb", 1_i64 << 10),
        ("b", 1_i64),
    ];
    let (number, multiplier) = units
        .iter()
        .find_map(|(suffix, multiplier)| {
            normalized
                .strip_suffix(suffix)
                .map(|number| (number.trim(), *multiplier))
        })
        .unwrap_or((normalized.as_str(), 1));
    let number = number
        .parse::<i64>()
        .map_err(|_| format!("invalid byte size {value}"))?;
    number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("byte size {value} overflows i64"))
}

/// 校验服务器本地路径后缀；LOAD DATA 禁止本地绝对路径。
fn validate_server_path(path: &str, in_import_into: bool) -> Result<(), String> {
    if path.starts_with('/') {
        if !in_import_into {
            return Err("LOAD DATA does not support server-disk paths".into());
        }
        let lower = path.to_ascii_lowercase();
        if !SUPPORTED_SERVER_SUFFIXES
            .iter()
            .any(|suffix| lower.ends_with(suffix))
        {
            return Err("unsupported server-disk file suffix".into());
        }
    }
    Ok(())
}

struct ParsedDataSourcePath {
    storage_uri: String,
    file_name_key: String,
    is_local: bool,
}

/// Split the storage root from the object key while preserving credentials in the URI.
fn parse_data_source_path(path: &str) -> Result<ParsedDataSourcePath, String> {
    let mut parsed = astersql_objstore::parse::ParseRawURL(path)
        .map_err(|error| format!("invalid IMPORT INTO data source URI: {error}"))?;
    let is_local = astersql_objstore::parse::IsLocal(&parsed);
    if is_local {
        let file_name_key = std::path::Path::new(&parsed.path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // The production local factory accepts the requested file path and opens its parent.
        return Ok(ParsedDataSourcePath {
            storage_uri: path.to_owned(),
            file_name_key,
            is_local,
        });
    }
    let file_name_key = parsed.path.trim_matches('/').to_owned();
    parsed.path.clear();
    Ok(ParsedDataSourcePath {
        storage_uri: parsed.String(),
        file_name_key,
        is_local,
    })
}

/// Reject malformed patterns before opening or walking external storage.
fn check_data_source_glob(pattern: &str) -> Result<(), String> {
    let bytes = pattern.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index += 1;
                if index == bytes.len() {
                    return Err(
                        "invalid IMPORT INTO data source URI: Glob pattern error: trailing escape"
                            .into(),
                    );
                }
            }
            b'[' => {
                let start = index;
                index += 1;
                if index < bytes.len() && matches!(bytes[index], b'!' | b'^') {
                    index += 1;
                }
                let content_start = index;
                while index < bytes.len() && bytes[index] != b']' {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
                if index == bytes.len() || index == content_start {
                    return Err(format!(
                        "invalid IMPORT INTO data source URI: Glob pattern error near {}",
                        &pattern[start..]
                    ));
                }
            }
            _ => {}
        }
        index += 1;
    }
    Ok(())
}

/// 去掉 scheme 与前导斜杠，得到存储内相对路径。
pub(crate) fn storage_path(path: &str) -> String {
    if std::path::Path::new(path).is_absolute() {
        return std::path::Path::new(path)
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    }
    path.split_once("://")
        .map_or(path, |(_, tail)| tail)
        .trim_start_matches('/')
        .to_owned()
}

/// 路径是否含通配符。
fn has_glob(path: &str) -> bool {
    path.contains('*') || path.contains('[')
}

/// 简易 glob 匹配（支持 `*` 与字符类 `[a-z]`；`?` 按 Go 路径规则作字面量）。
fn glob_matches(pattern: &str, value: &str) -> bool {
    let (mut pattern_index, mut value_index, mut star, mut checkpoint) = (0, 0, None, 0);
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    while value_index < value.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == value[value_index] {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'[' {
            let mut end = pattern_index + 1;
            while end < pattern.len() && pattern[end] != b']' {
                end += 1;
            }
            if end < pattern.len()
                && glob_class_matches(&pattern[pattern_index + 1..end], value[value_index])
            {
                pattern_index = end + 1;
                value_index += 1;
            } else if let Some(star_index) = star {
                pattern_index = star_index + 1;
                checkpoint += 1;
                value_index = checkpoint;
            } else {
                return false;
            }
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star = Some(pattern_index);
            pattern_index += 1;
            checkpoint = value_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            checkpoint += 1;
            value_index = checkpoint;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn glob_class_matches(class: &[u8], value: u8) -> bool {
    let (negated, class) = if class
        .first()
        .is_some_and(|byte| *byte == b'!' || *byte == b'^')
    {
        (true, &class[1..])
    } else {
        (false, class)
    };
    let mut matched = false;
    let mut index = 0;
    while index < class.len() {
        if index + 2 < class.len() && class[index + 1] == b'-' {
            matched |= class[index] <= value && value <= class[index + 2];
            index += 3;
        } else {
            matched |= class[index] == value;
            index += 1;
        }
    }
    if negated { !matched } else { matched }
}

/// 按后缀判断压缩类型。
fn compression_from_path(path: &str) -> Compression {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".gz") || lower.ends_with(".gzip") {
        Compression::Gz
    } else if lower.ends_with(".zstd") || lower.ends_with(".zst") {
        Compression::Zstd
    } else if lower.ends_with(".snappy") {
        Compression::Snappy
    } else {
        Compression::None
    }
}

/// 去掉已知压缩后缀以便识别真实格式。
fn strip_compression_suffix(path: &str) -> &str {
    [".gzip", ".zstd", ".snappy", ".gz", ".zst"]
        .iter()
        .find_map(|suffix| path.strip_suffix(suffix))
        .unwrap_or(path)
}

struct ClosingObjectReader(Box<dyn astersql_objstore_objectio::Reader>);
impl std::io::Read for ClosingObjectReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}
impl Drop for ClosingObjectReader {
    fn drop(&mut self) {
        let _ = self.0.close();
    }
}
mod parquet_error_bridge {
    pub fn as_parquet(
        e: impl std::fmt::Display,
    ) -> astersql_dumpformat_parquetfile::source_reader::SourceError {
        astersql_dumpformat_parquetfile::source_reader::SourceError::General(e.to_string())
    }
}

pub(crate) fn open_parquet_file(
    storage: SharedStorage,
    context: &StorageContext,
    file: &SourceFileMeta,
    location: &str,
) -> Result<astersql_dumpformat_parquetfile::file_parser::FileParser, String> {
    use astersql_dumpformat_parquetfile::{
        file_parser::FileParser,
        source_reader::{RangeOpener, SourceReader},
    };
    use parquet_error_bridge::as_parquet;

    let range_storage = storage.clone();
    let path = file.path.clone();
    let range_path = path.clone();
    let range_context = context.clone();
    let open: RangeOpener = Arc::new(move |start, end| {
        range_context.check()?;
        let guard = range_storage
            .lock()
            .map_err(|_| as_parquet("data storage lock is poisoned"))?;
        let options = astersql_objstore_storeapi::ReaderOption {
            StartOffset: Some(start as i64),
            EndOffset: Some(end as i64),
            ..Default::default()
        };
        let reader = guard
            .Open(&range_context, &range_path, Some(&options))
            .map_err(as_parquet)?;
        Ok(Box::new(ClosingObjectReader(reader)))
    });
    let size_context = context.clone();
    let source = SourceReader::prepare(
        file.file_size,
        move || {
            size_context.check()?;
            let guard = storage
                .lock()
                .map_err(|_| as_parquet("data storage lock is poisoned"))?;
            let mut reader = guard.Open(&size_context, &path, None).map_err(as_parquet)?;
            let size = reader.file_size();
            let close = reader.close();
            let size = size.map_err(as_parquet)?;
            close.map_err(as_parquet)?;
            u64::try_from(size).map_err(as_parquet)
        },
        open,
    )
    .map_err(|e| e.to_string())?;
    let parser = FileParser::new_with_location(source, location).map_err(|e| e.to_string())?;
    Ok(parser)
}
