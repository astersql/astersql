// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 这个文件集中承载 dumpling 导出层的大部分静态配置语义：
// 包括默认值、命令行映射、输出格式选择、过滤器解析、模板校验和若干兼容性辅助函数。
// Rust 版本没有完整搬运 Go 侧所有外部依赖，而是保留对 CLI 和测试真正重要的语义骨架，
// 因此注释会特别说明哪些行为是“Go 对齐点”，哪些只是当前 arm64-safe 适配的最小实现。
// 阅读时可以把它当成“export 包的配置总装配中心”。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
// CSV 方言决定的不是整个导出流程，而是 CSV writer 的少数兼容性分支。
pub enum CSVDialect {
    #[default]
    // 默认方言覆盖 MySQL / MariaDB / TiDB 这一组语义接近的目标。
    CSVDialectDefault = 0,
    // Snowflake 需要把二进制列编码成 HEX 文本。
    CSVDialectSnowflake = 1,
    // Redshift 与 Snowflake 在二进制列输出格式上保持一致。
    CSVDialectRedshift = 2,
    // BigQuery 采用 Base64 作为二进制列文本表示。
    CSVDialectBigQuery = 3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
// BinaryFormat 只描述“字节列如何落成文本”，不涉及 CSV 分隔符等外围格式。
// 与 Go 侧 DialectBinaryFormatMap 的映射保持一一对应，避免跨方言导出时编码漂移。
pub enum BinaryFormat {
    #[default]
    BinaryFormatUTF8 = 0,
    BinaryFormatHEX = 1,
    BinaryFormatBase64 = 2,
}

pub fn DialectBinaryFormatMap(d: CSVDialect) -> BinaryFormat {
    // Go 里这是一个静态 map；Rust 用函数表达，减少全局初始化负担。
    match d {
        CSVDialect::CSVDialectDefault => BinaryFormat::BinaryFormatUTF8,
        CSVDialect::CSVDialectSnowflake | CSVDialect::CSVDialectRedshift => {
            BinaryFormat::BinaryFormatHEX
        }
        CSVDialect::CSVDialectBigQuery => BinaryFormat::BinaryFormatBase64,
    }
}

// Security 聚合 CLI 传入的 TLS 文件路径和已经读入内存的证书内容。
// 当前简化实现没有直接保存 TLS 配置对象，而是把原始材料留给后续构造阶段消费。
pub struct Security {
    // CAPath / CertPath / KeyPath 对应命令行 `--ca/--cert/--key` 三件套。
    pub CAPath: String,
    pub CertPath: String,
    pub KeyPath: String,
    // 字节数组允许测试或后续流程绕过文件系统，直接注入证书内容。
    pub SSLCABytes: Vec<u8>,
    pub SSLCertBytes: Vec<u8>,
    pub SSLKeyBytes: Vec<u8>,
}

impl Default for Security {
    fn default() -> Self {
        Self {
            CAPath: String::new(),
            CertPath: String::new(),
            KeyPath: String::new(),
            SSLCABytes: vec![],
            SSLCertBytes: vec![],
            SSLKeyBytes: vec![],
        }
    }
}

// Config 是 dumpling export 的总配置对象。
// 字段很多，但大致可以分成五组：
// 1. 连接与安全；
// 2. 导出范围与过滤；
// 3. 输出格式与模板；
// 4. 观测与外部依赖；
// 5. Parquet / PD / TLS 这类进阶兼容项。
pub struct Config {
    // BackendOptions 为对象存储后端预留；当前简化实现默认留空。
    pub BackendOptions: BackendOptions,
    // 这一组布尔开关描述用户在 CLI 上显式选择了哪些行为模式。
    pub SpecifiedTables: bool,
    pub AllowCleartextPasswords: bool,
    pub SortByPk: bool,
    pub NoViews: bool,
    pub NoSequences: bool,
    pub NoHeader: bool,
    pub NoSchemas: bool,
    pub NoData: bool,
    pub CompleteInsert: bool,
    pub TransactionalConsistency: bool,
    pub EscapeBackslash: bool,
    pub DumpEmptyDatabase: bool,
    pub PosAfterConnect: bool,
    pub CompressType: CompressType,
    // 连接四元组与认证信息，决定如何连到上游数据库。
    pub Host: String,
    pub Port: i32,
    pub Threads: i32,
    pub User: String,
    pub Password: String,
    pub Security: Security,
    // 这一组控制日志输出、导出目录和快照/一致性视图。
    pub LogLevel: String,
    pub LogFile: String,
    pub LogFormat: String,
    pub OutputDirPath: String,
    pub StatusAddr: String,
    pub Snapshot: String,
    pub Consistency: String,
    pub CsvNullValue: String,
    pub SQL: String,
    pub CsvSeparator: String,
    pub CsvDelimiter: String,
    pub CsvLineTerminator: String,
    pub Databases: Vec<String>,
    // TableFilter 和 Tables 分别表示“规则级过滤”和“显式表集合”。
    pub TableFilter: Arc<dyn Filter>,
    pub Where: String,
    pub FileType: String,
    pub ServerInfo: ServerInfo,
    pub Logger: Option<Logger>,
    pub OutputFileTemplate: OutputTemplate,
    pub Rows: u64,
    pub ReadTimeout: Duration,
    pub TiDBMemQuotaQuery: u64,
    pub FileSize: u64,
    pub StatementSize: u64,
    pub SessionParams: HashMap<String, String>,
    pub Tables: DatabaseTables,
    pub columnFilter: columnFilterConfig,
    pub columnProjection: HashMap<(String, String), columnProjection>,
    // Collation / CSV dialect / partitions 都是格式兼容和范围裁剪的细节选项。
    pub CollationCompatible: String,
    pub CsvOutputDialect: CSVDialect,
    pub Partitions: Vec<String>,
    // Prometheus 与外部存储对象通常在运行前期或懒加载阶段再真正初始化。
    pub Labels: Labels,
    pub PromFactory: Arc<dyn Factory>,
    pub PromRegistry: Arc<dyn Registry>,
    pub ExtStorage: Option<Arc<dyn Storage>>,
    pub MinTLSVersion: u16,
    pub IOTotalBytes: Arc<AtomicU64>,
    pub Net: String,
    // 下面三项只在 keyspace / GC 控制场景才真正使用。
    pub PDAddr: String,
    pub ClusterSSLCA: String,
    pub ClusterSSLCert: String,
    pub ClusterSSLKey: String,
    // Parquet 额外保留自己的压缩和块大小参数，不与通用 CompressType 混用。
    pub ParquetCompressType: CompressType,
    pub ParquetPageSize: i64,
    pub ParquetRowGroupSize: i64,
}

pub fn ServerInfoUnknown() -> ServerInfo {
    // 默认值明确表达“尚未探测到服务端类型”，比伪造 MySQL/TiDB 更安全。
    ServerInfo {
        ServerType: ServerType::ServerTypeUnknown,
        ServerVersion: None,
        HasTiKV: false,
    }
}

// UnspecifiedSize 既表示“用户没填”，也表示某些 split 特性应当关闭。
pub const UnspecifiedSize: u64 = 0;
// INSERT 语句目标大小默认与 Go 侧保持一致，避免 SQL writer 行为漂移。
pub const DefaultStatementSize: u64 = 1_000_000;
// 这是下游 session variable 使用的键名常量，而不是默认值本身。
pub const TiDBMemQuotaQueryName: &str = "tidb_mem_quota_query";
// 默认过滤器会排除系统库，再配合 `*.*` 允许普通业务库全部进入候选集。
pub const DefaultTableFilter: &str =
    "!/^(mysql|sys|INFORMATION_SCHEMA|PERFORMANCE_SCHEMA|METRICS_SCHEMA|INSPECTION_SCHEMA)$/.*";
// 下面三项常量更多是为导出执行层和 safepoint 协调逻辑服务。
pub const defaultTaskChannelCapacity: usize = 128;
pub const defaultDumpGCSafePointTTL: i64 = 5 * 60;
pub const LooseCollationCompatible: &str = "loose";
pub const StrictCollationCompatible: &str = "strict";
pub const dumplingServiceSafePointPrefix: &str = "dumpling";

pub fn DefaultConfig() -> Config {
    // 默认 filter 先允许所有表，后续 CLI 解析再根据用户输入收窄。
    let all_filter = filter_parse(&["*.*".into()]).unwrap();
    Config {
        BackendOptions: BackendOptions::default(),
        Databases: vec![],
        Host: "127.0.0.1".into(),
        User: "root".into(),
        Port: 3306,
        Password: String::new(),
        // 线程数、状态端口和 statement size 都沿用 Go 既有默认值。
        Threads: 4,
        Logger: None,
        StatusAddr: ":8281".into(),
        FileSize: UnspecifiedSize,
        StatementSize: DefaultStatementSize,
        OutputDirPath: ".".into(),
        ServerInfo: ServerInfoUnknown(),
        SortByPk: true,
        Tables: HashMap::new(),
        columnFilter: columnFilterConfig::default(),
        columnProjection: HashMap::new(),
        Snapshot: String::new(),
        Consistency: ConsistencyTypeAuto.to_string(),
        // 视图/序列默认不导，和上游 dumpling 的保守策略一致。
        NoViews: true,
        NoSequences: true,
        Rows: UnspecifiedSize,
        Where: String::new(),
        // 默认倾向于输出可直接被 MySQL 生态重新消费的文本形式。
        EscapeBackslash: true,
        FileType: String::new(),
        NoHeader: false,
        NoSchemas: false,
        NoData: false,
        CsvNullValue: "\\N".into(),
        SQL: String::new(),
        TableFilter: all_filter,
        DumpEmptyDatabase: true,
        // CSV 默认值与 Go CLI 帮助文案保持一致。
        CsvDelimiter: "\"".into(),
        CsvSeparator: ",".into(),
        CsvLineTerminator: "\r\n".into(),
        SessionParams: HashMap::new(),
        // 输出模板默认交给 dumpformat 模块统一生成。
        OutputFileTemplate: DefaultOutputFileTemplate(),
        PosAfterConnect: false,
        CollationCompatible: LooseCollationCompatible.to_string(),
        CsvOutputDialect: CSVDialect::CSVDialectDefault,
        SpecifiedTables: false,
        // 默认 factory / registry 让配置对象拿来即可跑，不要求调用方额外装配。
        PromFactory: NewDefaultFactory(),
        PromRegistry: NewDefaultRegistry(),
        TransactionalConsistency: true,
        PDAddr: String::new(),
        ClusterSSLCA: String::new(),
        ClusterSSLCert: String::new(),
        ClusterSSLKey: String::new(),
        // Parquet 默认值直接对齐 writer 侧常量，避免 CLI 与输出层分叉。
        ParquetCompressType: DefaultCompressionType,
        ParquetPageSize: MiB,
        ParquetRowGroupSize: DefaultRowGroupMemoryLimitBytes,
        AllowCleartextPasswords: false,
        CompleteInsert: false,
        CompressType: CompressType::NoCompression,
        Security: Security::default(),
        LogLevel: String::new(),
        LogFile: String::new(),
        LogFormat: String::new(),
        Partitions: vec![],
        Labels: Labels::default(),
        ExtStorage: None,
        MinTLSVersion: 0,
        IOTotalBytes: Arc::new(AtomicU64::new(0)),
        Net: "tcp".into(),
        // 0 表示“不额外覆盖”，后续由驱动配置路径决定真实超时与 TLS 最低版本。
        ReadTimeout: Duration::from_secs(0),
        TiDBMemQuotaQuery: 0,
    }
}

impl Config {
    pub fn String(&self) -> String {
        // Rust 简化版只输出最关键的几项摘要，避免为 JSON 序列化引入额外依赖噪声。
        format!(
            "{{\"Host\":\"{}\",\"Port\":{},\"User\":\"{}\",\"Consistency\":\"{}\",\"FileType\":\"{}\"}}",
            self.Host, self.Port, self.User, self.Consistency, self.FileType
        )
    }

    pub fn GetDriverConfig(&self, db: &str) -> MysqlConfig {
        // 这里只负责把 Config 里的连接参数投影成驱动层配置，不做连通性检查。
        let mut cfg = MysqlConfig::default();
        cfg.User = self.User.clone();
        cfg.Passwd = self.Password.clone();
        // `Net` 允许测试或特殊场景覆盖默认 tcp，例如 unix socket。
        cfg.Net = if self.Net.is_empty() {
            "tcp".into()
        } else {
            self.Net.clone()
        };
        cfg.Addr = format!("{}:{}", self.Host, self.Port);
        cfg.DBName = db.to_string();
        cfg.AllowCleartextPasswords = self.AllowCleartextPasswords;
        cfg.ReadTimeout = self.ReadTimeout;
        cfg
    }

    pub fn createExternalStorage(&mut self) -> Result<Arc<dyn Storage>> {
        // 已有存储对象时直接复用，避免同一配置反复创建底层存储句柄。
        if let Some(s) = &self.ExtStorage {
            return Ok(s.clone());
        }
        // 当前 arm64-safe 版本默认回落到内存/本地风格存储实现。
        let s = Arc::new(MemStorage::new(self.OutputDirPath.clone()));
        self.ExtStorage = Some(s.clone());
        Ok(s)
    }

    pub fn clone_for_mutate(&self) -> Config {
        // 这个克隆函数不是简单 `Clone`：它有意重置部分运行时字段，例如 StatusAddr/Labels。
        // 这样调用方可以在共享大部分静态配置的前提下，安全地产生一个可局部修改的新副本。
        Config {
            BackendOptions: BackendOptions::default(),
            // 先复制所有导出行为开关，保证语义模式与原配置一致。
            SpecifiedTables: self.SpecifiedTables,
            AllowCleartextPasswords: self.AllowCleartextPasswords,
            SortByPk: self.SortByPk,
            NoViews: self.NoViews,
            NoSequences: self.NoSequences,
            NoHeader: self.NoHeader,
            NoSchemas: self.NoSchemas,
            NoData: self.NoData,
            CompleteInsert: self.CompleteInsert,
            TransactionalConsistency: self.TransactionalConsistency,
            EscapeBackslash: self.EscapeBackslash,
            DumpEmptyDatabase: self.DumpEmptyDatabase,
            PosAfterConnect: self.PosAfterConnect,
            CompressType: self.CompressType,
            // 连接身份与 TLS 材料照搬，避免 clone 后丢失连接能力。
            Host: self.Host.clone(),
            Port: self.Port,
            Threads: self.Threads,
            User: self.User.clone(),
            Password: self.Password.clone(),
            Security: Security {
                CAPath: self.Security.CAPath.clone(),
                CertPath: self.Security.CertPath.clone(),
                KeyPath: self.Security.KeyPath.clone(),
                SSLCABytes: self.Security.SSLCABytes.clone(),
                SSLCertBytes: self.Security.SSLCertBytes.clone(),
                SSLKeyBytes: self.Security.SSLKeyBytes.clone(),
            },
            // 日志配置保留，但 StatusAddr 刻意清空，避免副本抢占同一监听端口。
            LogLevel: self.LogLevel.clone(),
            LogFile: self.LogFile.clone(),
            LogFormat: self.LogFormat.clone(),
            OutputDirPath: self.OutputDirPath.clone(),
            StatusAddr: String::new(),
            Snapshot: self.Snapshot.clone(),
            Consistency: self.Consistency.clone(),
            CsvNullValue: self.CsvNullValue.clone(),
            SQL: self.SQL.clone(),
            CsvSeparator: self.CsvSeparator.clone(),
            CsvDelimiter: self.CsvDelimiter.clone(),
            CsvLineTerminator: self.CsvLineTerminator.clone(),
            Databases: self.Databases.clone(),
            TableFilter: self.TableFilter.clone(),
            Where: self.Where.clone(),
            FileType: self.FileType.clone(),
            // 这里保留 ServerInfo / Logger / Template，是为了让副本继续复用已探测结果。
            ServerInfo: self.ServerInfo.clone(),
            Logger: self.Logger.clone(),
            OutputFileTemplate: self.OutputFileTemplate.clone(),
            Rows: self.Rows,
            ReadTimeout: self.ReadTimeout,
            TiDBMemQuotaQuery: self.TiDBMemQuotaQuery,
            FileSize: self.FileSize,
            StatementSize: self.StatementSize,
            SessionParams: self.SessionParams.clone(),
            Tables: self.Tables.clone(),
            columnFilter: self.columnFilter.clone(),
            columnProjection: self.columnProjection.clone(),
            // 格式兼容项和分区列表也应继承，否则副本很容易跑出不同结果。
            CollationCompatible: self.CollationCompatible.clone(),
            CsvOutputDialect: self.CsvOutputDialect,
            Partitions: self.Partitions.clone(),
            // Labels 重置为默认值，避免把前一次注册/标记状态带进副本。
            Labels: Labels::default(),
            PromFactory: self.PromFactory.clone(),
            PromRegistry: self.PromRegistry.clone(),
            ExtStorage: self.ExtStorage.clone(),
            MinTLSVersion: self.MinTLSVersion,
            IOTotalBytes: self.IOTotalBytes.clone(),
            Net: self.Net.clone(),
            // PD / cluster TLS / parquet 参数都属于真正影响输出行为的配置，必须完整继承。
            PDAddr: self.PDAddr.clone(),
            ClusterSSLCA: self.ClusterSSLCA.clone(),
            ClusterSSLCert: self.ClusterSSLCert.clone(),
            ClusterSSLKey: self.ClusterSSLKey.clone(),
            ParquetCompressType: self.ParquetCompressType,
            ParquetPageSize: self.ParquetPageSize,
            ParquetRowGroupSize: self.ParquetRowGroupSize,
        }
    }
}

pub fn timestampDirName() -> String {
    // Rust 简化版用 epoch 秒数生成目录名，满足“每次运行不同”即可。
    // Go 用 RFC3339；这里不追求同一字面格式，只保留“目录名唯一性”语义。
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", d.as_secs())
}

pub fn ParseFileSize(file_size_str: &str) -> Result<u64> {
    // 空字符串保留成未指定状态，而不是强行解释成 0 字节限制。
    // 这样调用方后续还能区分“未开启 split”和“显式要求极小文件”。
    if file_size_str.is_empty() {
        return Ok(UnspecifiedSize);
    }
    if let Ok(file_size_mib) = file_size_str.parse::<u64>() {
        return Ok(file_size_mib.wrapping_mul(MiB as u64));
    }
    RAMInBytes(file_size_str)
        .map(|size| size as u64)
        .map_err(|_| errors_errorf(format!("failed to parse filesize (-F '{file_size_str}')")))
}

pub fn ParseTableFilter(tables_list: &[String], filters: &[String]) -> Result<Arc<dyn Filter>> {
    if !tables_list.is_empty() {
        // Go permits --tables-list with the registered default filter, but rejects an
        // explicitly replaced filter because the two selection modes are ambiguous.
        let default_filters = ["*.*".to_string(), DefaultTableFilter.to_string()];
        if filters != default_filters {
            return Err(errors_new(
                "cannot pass --tables-list and --filter together",
            ));
        }
        return filter_parse(tables_list);
    }
    if filters.is_empty() {
        // 用户没传 filter 时，自动拼上默认系统库排除规则与全表通配。
        return filter_parse(&[DefaultTableFilter.into(), "*.*".into()]);
    }
    filter_parse(filters)
}

pub fn GetConfTables(tables_list: &[String]) -> Result<DatabaseTables> {
    // CLI 上的 `db.table` 文本会在这里转成导出器真正消费的结构化表集合。
    let mut db_tables = DatabaseTables::new();
    for table_and_db in tables_list {
        let (database, table) = table_and_db.split_once('.').ok_or_else(|| {
            errors_errorf(format!(
                "--tables-list only accepts qualified table names, but `{table_and_db}` lacks a dot"
            ))
        })?;
        db_tables.AppendTable(
            database.to_string(),
            TableInfo {
                Name: table.to_string(),
                AvgRowLength: 0,
                Type: TableType::TableTypeBase,
            },
        );
    }
    Ok(db_tables)
}

pub fn ParseOutputDialect(output_dialect: &str) -> Result<CSVDialect> {
    // 大小写不敏感，兼容脚本和历史配置里不统一的写法。
    // 只接受 Go 版公开的四类值，避免 Rust 单方扩展 CLI 契约。
    match output_dialect.to_ascii_lowercase().as_str() {
        "" | "default" => Ok(CSVDialect::CSVDialectDefault),
        "snowflake" => Ok(CSVDialect::CSVDialectSnowflake),
        "redshift" => Ok(CSVDialect::CSVDialectRedshift),
        "bigquery" => Ok(CSVDialect::CSVDialectBigQuery),
        other => Err(errors_errorf(format!("unknown output dialect {other}"))),
    }
}

pub fn parseParquetCompressType(compress_type: &str) -> Result<CompressType> {
    // Parquet 允许自己的压缩参数，故意不复用通用 `--compress` 的语义。
    // 空字符串同样视作“走默认值”，与 Go CLI 的体验保持一致。
    match compress_type.to_ascii_lowercase().as_str() {
        "" | "no-compression" => Ok(CompressType::NoCompression),
        "gzip" => Ok(CompressType::Gzip),
        "snappy" => Ok(CompressType::Snappy),
        "zstd" => Ok(CompressType::Zstd),
        other => Err(errors_errorf(format!(
            "unknown parquet compress type {other}"
        ))),
    }
}

fn parseCompressType(compress_type: &str) -> Result<CompressType> {
    match compress_type.to_ascii_lowercase().as_str() {
        "" | "no-compression" => Ok(CompressType::NoCompression),
        "gzip" => Ok(CompressType::Gzip),
        "snappy" => Ok(CompressType::Snappy),
        "zstd" => Ok(CompressType::Zstd),
        other => Err(errors_errorf(format!("unknown compress type {other}"))),
    }
}

pub fn adjustConfig(conf: &mut Config, fns: &[fn(&mut Config) -> Result<()>]) -> Result<()> {
    // 顺序执行一组修正函数，方便把“读取配置”和“规范化配置”解耦。
    // 谁先执行会影响最终结果，因此这里刻意保持调用方传入顺序。
    for f in fns {
        f(conf)?;
    }
    Ok(())
}

pub fn buildTLSConfig(_conf: &mut Config) -> Result<()> {
    // 当前最小实现不真正构造 TLS 对象，但保留扩展挂点以贴近 Go 结构。
    // 这也让上层调整链可以继续保持和 Go 相似的调用顺序。
    Ok(())
}

pub fn validateSpecifiedSQL(conf: &Config) -> Result<()> {
    // `--sql` 会绕过常规表枚举，因此不能再叠加 where/partitions 这类范围条件。
    // 这些限制不是解析器能力问题，而是为了避免多套范围语义相互冲突。
    if !conf.SQL.is_empty() && !conf.Where.is_empty() {
        return Err(errors_new(
            "can't specify both --sql and --where at the same time. Please try to combine them into --sql",
        ));
    }
    if !conf.SQL.is_empty() && !conf.Partitions.is_empty() {
        return Err(errors_new(
            "can't specify both --sql and --partitions at the same time",
        ));
    }
    Ok(())
}

pub fn adjustFileFormat(conf: &mut Config) -> Result<()> {
    // 先统一成小写，后面的分支只处理规范化后的值。
    conf.FileType = conf.FileType.to_ascii_lowercase();
    match conf.FileType.as_str() {
        "" => {
            // 未显式指定文件类型时，按是否使用自定义 SQL 推导默认输出格式。
            if !conf.SQL.is_empty() {
                conf.FileType = FileFormatCSVString.to_string();
            } else {
                conf.FileType = FileFormatSQLTextString.to_string();
            }
        }
        FileFormatSQLTextString => {
            // 自定义 SQL 无法安全映射到 SQL 文本导出，因此只能落 CSV。
            if !conf.SQL.is_empty() {
                return Err(errors_errorf(format!(
                    "unsupported config.FileType '{}' when we specify --sql, please unset --filetype or set it to 'csv'",
                    conf.FileType
                )));
            }
        }
        FileFormatCSVString => {}
        FileFormatParquetString => {
            // Parquet 走专属压缩选项，禁止和通用压缩开关叠加。
            if conf.CompressType != CompressType::NoCompression {
                return Err(errors_errorf(
                    "parquet does not support --compress, please unset it or use --parquet-compress instead",
                ));
            }
        }
        other => return Err(errors_errorf(format!("unknown config.FileType '{other}'"))),
    }
    Ok(())
}

pub fn matchMysqlBugversion(info: &ServerInfo) -> bool {
    // 这是 Go 侧保留的版本门槛逻辑，只对 MySQL 生效，不影响 TiDB。
    // 区间采用开区间比较：8.0.2 和 8.0.23 本身都不算命中。
    if info.ServerType != ServerType::ServerTypeMySQL {
        return false;
    }
    let Some(current) = &info.ServerVersion else {
        return false;
    };
    let start = parse_semver("8.0.2");
    let end = parse_semver("8.0.23");
    start.LessThan(current) && current.LessThan(&end)
}

pub fn normalizePartitions(partitions: &[String]) -> Vec<String> {
    // 分区名统一 trim + lower-case，并去重，降低后续比较复杂度。
    // 返回值保留首次出现顺序，方便日志和输出仍与用户输入大致对应。
    let mut seen = HashMap::new();
    let mut result = Vec::new();
    for p in partitions {
        let p = p.trim().to_ascii_lowercase();
        if p.is_empty() || seen.contains_key(&p) {
            continue;
        }
        seen.insert(p.clone(), ());
        result.push(p);
    }
    result
}

pub fn outputTemplateUsesIndex(tmpl: &OutputTemplate, template_name: &str) -> bool {
    // split 模式下必须在条件块外保留独立 `{{.Index}}`，否则多个 chunk 可能同名。
    // 这里的检查是启发式的，但已经足够覆盖当前测试关心的风险模型。
    let Some(text) = tmpl.defines.get(template_name) else {
        return false;
    };
    // Strip {{if ...}}...{{end}} / {{if ...}}...{{else}}...{{end}} blocks, then require standalone {{.Index}}.
    let mut s = text.clone();
    while let Some(start) = s.find("{{if") {
        let Some(end_rel) = s[start..].find("{{end}}") else {
            break;
        };
        let end = start + end_rel + "{{end}}".len();
        s.replace_range(start..end, "");
    }
    s.contains("{{.Index}}")
}

/// Minimal flag set for unit tests (Go pflag subset).
/// 这里只覆盖 config 测试真正会用到的那部分 pflag 能力。
#[derive(Clone, Debug, Default)]
pub struct FlagSet {
    // values 保存解析后的最终值；defaults 只服务 Parse 阶段判断 bool/缺省值。
    values: HashMap<String, String>,
    changed: HashSet<String>,
    defaults: HashMap<String, String>,
    arrays: HashMap<String, Vec<String>>,
}

impl FlagSet {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn String(&mut self, name: &str, default: &str, _usage: &str) {
        // 测试 stub 不关心 usage 文案，只保留默认值与最终值。
        self.defaults.insert(name.into(), default.into());
        self.values
            .entry(name.into())
            .or_insert_with(|| default.to_string());
    }
    pub fn Int(&mut self, name: &str, default: i32, _usage: &str) {
        self.String(name, &default.to_string(), _usage);
    }
    pub fn Uint64(&mut self, name: &str, default: u64, _usage: &str) {
        self.String(name, &default.to_string(), _usage);
    }
    pub fn Bool(&mut self, name: &str, default: bool, _usage: &str) {
        self.String(name, if default { "true" } else { "false" }, _usage);
    }
    pub fn StringSlice(&mut self, name: &str, default: &[&str], _usage: &str) {
        self.String(name, &default.join(","), _usage);
    }
    pub fn StringArray(&mut self, name: &str) {
        self.arrays.insert(name.into(), vec![]);
    }
    pub fn GetStringArray(&self, name: &str) -> Vec<String> {
        self.arrays.get(name).cloned().unwrap_or_default()
    }
    fn setFlagValue(&mut self, name: &str, value: &str) {
        if let Some(array) = self.arrays.get_mut(name) {
            array.push(value.into());
        } else {
            self.values.insert(name.into(), value.into());
        }
    }
    pub fn Parse(&mut self, args: &[&str]) -> Result<()> {
        // 这里只实现 `--name value` 和 `--name=value` 两种最常见形式。
        // 其目的不是替代真正 pflag，而是给配置测试提供一个稳定输入面。
        let mut i = 0;
        while i < args.len() {
            let a = args[i];
            if !a.starts_with("--") {
                return Err(errors_new(format!("unknown arg {a}")));
            }
            let name = a.trim_start_matches('-');
            if let Some((n, v)) = name.split_once('=') {
                self.setFlagValue(n, v);
                self.changed.insert(n.to_string());
                i += 1;
                continue;
            }
            let def = self.defaults.get(name).cloned().unwrap_or_default();
            let is_bool = def == "true" || def == "false";
            if is_bool {
                // bool flag 出现即视为 true，贴近常见 CLI 语义。
                // 这也意味着 stub 不支持显式 `--flag=false` 覆盖默认值。
                self.values.insert(name.to_string(), "true".into());
                self.changed.insert(name.to_string());
                i += 1;
                continue;
            }
            i += 1;
            if i >= args.len() {
                // 对缺值参数直接报错，避免静默吞掉下一个 flag。
                return Err(errors_new(format!("flag --{name} needs a value")));
            }
            self.setFlagValue(name, args[i]);
            self.changed.insert(name.to_string());
            i += 1;
        }
        Ok(())
    }
    pub fn Changed(&self, name: &str) -> bool {
        // 某些校验需要区分“用户显式传了默认值”和“完全没设置过”。
        self.changed.contains(name)
    }
    pub fn GetString(&self, name: &str) -> String {
        // getter 一律返回拥有所有权的值，保持调用方接口简单。
        self.values.get(name).cloned().unwrap_or_default()
    }
    pub fn GetInt(&self, name: &str) -> i32 {
        // 解析失败回落到 0，方便后续统一由业务校验拦截非法值。
        self.GetString(name).parse().unwrap_or(0)
    }
    pub fn GetIntResult(&self, name: &str) -> Result<i32> {
        self.GetString(name)
            .parse()
            .map_err(|_| errors_new(format!("invalid value for --{name}")))
    }
    pub fn GetUint64(&self, name: &str) -> u64 {
        // 与 GetInt 一样，stub 更关注“能走通流程”而不是保留精细错误类型。
        self.GetString(name).parse().unwrap_or(0)
    }
    pub fn GetUint64Result(&self, name: &str) -> Result<u64> {
        self.GetString(name)
            .parse()
            .map_err(|_| errors_new(format!("invalid value for --{name}")))
    }
    pub fn GetBool(&self, name: &str) -> bool {
        // 兼容几种常见 true 表示即可，足以覆盖测试场景。
        matches!(self.GetString(name).as_str(), "true" | "1" | "TRUE")
    }
    pub fn GetStringSlice(&self, name: &str) -> Vec<String> {
        let s = self.GetString(name);
        if s.is_empty() {
            vec![]
        } else {
            // StringSlice 在 stub 里用逗号拼接保存，读取时再拆回向量。
            s.split(',').map(|x| x.to_string()).collect()
        }
    }
}

impl Config {
    pub fn DefineFlags(&self, flags: &mut FlagSet) {
        // 这里保留的是 config 测试需要的 flag 子集，不追求覆盖 Go 的全部 CLI 文案。
        // 数据源与筛选入口。
        flags.String("database", "", "");
        flags.StringSlice("tables-list", &[], "");
        flags.StringArray("column-filter");
        flags.String("column-filter-file", "", "");
        flags.Bool("case-sensitive", false, "");
        // 连接参数。
        flags.String("host", &self.Host, "");
        flags.String("user", &self.User, "");
        flags.Int("port", self.Port, "");
        flags.String("password", "", "");
        flags.Bool("allow-cleartext-passwords", false, "");
        // 并发与文件切分相关参数。
        flags.Int("threads", self.Threads, "");
        flags.String("filesize", "", "");
        flags.Uint64("statement-size", self.StatementSize, "");
        // 输出目录与日志格式。
        flags.String("output", &self.OutputDirPath, "");
        flags.String("loglevel", "info", "");
        flags.String("logfile", "", "");
        flags.String("logfmt", "text", "");
        // 一致性视图与快照。
        flags.String("consistency", &self.Consistency, "");
        flags.String("snapshot", "", "");
        // 结构对象与排序相关导出开关。
        flags.Bool("no-views", self.NoViews, "");
        flags.Bool("no-sequences", self.NoSequences, "");
        flags.Bool("sort-by-pk", self.SortByPk, "");
        flags.String("status-addr", &self.StatusAddr, "");
        flags.Uint64("rows", 0, "");
        flags.String("where", "", "");
        // 文件格式与 CSV 细节。
        flags.Bool("escape-backslash", self.EscapeBackslash, "");
        flags.String("filetype", "", "");
        flags.Bool("no-header", false, "");
        flags.Bool("no-schemas", false, "");
        flags.Bool("no-data", false, "");
        flags.String("csv-null-value", &self.CsvNullValue, "");
        flags.String("sql", "", "");
        flags.StringSlice("filter", &["*.*", DefaultTableFilter], "");
        flags.Bool("case-sensitive", false, "");
        flags.Bool("dump-empty-database", self.DumpEmptyDatabase, "");
        flags.String("csv-separator", &self.CsvSeparator, "");
        flags.String("csv-delimiter", &self.CsvDelimiter, "");
        flags.String("csv-line-terminator", &self.CsvLineTerminator, "");
        flags.String("output-filename-template", "", "");
        flags.Bool("complete-insert", false, "");
        // 方言、分区和集群相关参数。
        flags.String("csv-output-dialect", "", "");
        flags.StringSlice("partitions", &[], "");
        flags.String("pd", "", "");
        flags.String("cluster-tls-ca", "", "");
        flags.String("cluster-tls-cert", "", "");
        flags.String("cluster-tls-key", "", "");
        // Parquet / 压缩 / 安全 / 事务兼容补充参数。
        flags.String("parquet-compress", "", "");
        flags.String("parquet-page-size", "1MiB", "");
        flags.String(
            "parquet-row-group-size",
            &format!("{}B", DefaultRowGroupMemoryLimitBytes),
            "",
        );
        flags.String("compress", "", "");
        flags.Uint64("tidb-mem-quota-query", 0, "");
        flags.String("ca", "", "");
        flags.String("cert", "", "");
        flags.String("key", "", "");
        flags.Bool("transactional-consistency", true, "");
        flags.String("read-timeout", "0s", "");
        // session params 在简化版里暂未真正展开，但仍保留入口。
        flags.String("params", "", "");
    }

    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        // 第一阶段：把扁平 flag 值逐项拷回 Config。
        self.Databases = flags.GetStringSlice("database");
        self.Host = flags.GetString("host");
        self.User = flags.GetString("user");
        self.Port = flags.GetIntResult("port")?;
        self.Password = flags.GetString("password");
        self.AllowCleartextPasswords = flags.GetBool("allow-cleartext-passwords");
        self.Threads = flags.GetIntResult("threads")?;
        self.StatementSize = flags.GetUint64Result("statement-size")?;
        self.OutputDirPath = flags.GetString("output");
        self.LogLevel = flags.GetString("loglevel");
        self.LogFile = flags.GetString("logfile");
        self.LogFormat = flags.GetString("logfmt");
        self.Consistency = flags.GetString("consistency");
        self.Snapshot = flags.GetString("snapshot");
        self.NoViews = flags.GetBool("no-views");
        self.NoSequences = flags.GetBool("no-sequences");
        self.SortByPk = flags.GetBool("sort-by-pk");
        self.StatusAddr = flags.GetString("status-addr");
        self.Rows = flags.GetUint64Result("rows")?;
        self.Where = flags.GetString("where");
        self.EscapeBackslash = flags.GetBool("escape-backslash");
        self.FileType = flags.GetString("filetype");
        self.NoHeader = flags.GetBool("no-header");
        self.NoSchemas = flags.GetBool("no-schemas");
        self.NoData = flags.GetBool("no-data");
        self.CsvNullValue = flags.GetString("csv-null-value");
        self.SQL = flags.GetString("sql");
        self.DumpEmptyDatabase = flags.GetBool("dump-empty-database");
        self.TiDBMemQuotaQuery = flags.GetUint64Result("tidb-mem-quota-query")?;
        self.Security.CAPath = flags.GetString("ca");
        self.Security.CertPath = flags.GetString("cert");
        self.Security.KeyPath = flags.GetString("key");
        self.CsvSeparator = flags.GetString("csv-separator");
        self.CsvDelimiter = flags.GetString("csv-delimiter");
        self.CsvLineTerminator = flags.GetString("csv-line-terminator");
        self.CompleteInsert = flags.GetBool("complete-insert");
        self.TransactionalConsistency = flags.GetBool("transactional-consistency");
        self.Partitions = normalizePartitions(&flags.GetStringSlice("partitions"));
        // 基础约束优先在这里拦截，避免后续逻辑建立在明显非法值上。
        // 线程数小于等于 0 时，split/并发导出逻辑都会失去意义。
        if self.Threads <= 0 {
            return Err(errors_new(format!(
                "--threads is set to {}. It should be greater than 0",
                self.Threads
            )));
        }
        if self.CsvSeparator.is_empty() {
            return Err(errors_new(
                "--csv-separator is set to \"\". It must not be an empty string",
            ));
        }
        let tables_list = flags.GetStringSlice("tables-list");
        let file_size_str = flags.GetString("filesize");
        let filters = flags.GetStringSlice("filter");
        self.parseColumnFilterOptions(
            &flags.GetStringArray("column-filter"),
            &flags.GetString("column-filter-file"),
            flags.GetBool("case-sensitive"),
        )?;
        let mut output_filename_format = flags.GetString("output-filename-template");
        // 表清单、结构化表集合和过滤器三者需要在这里同时推导完成。
        // 这样下游无论消费显式表集合还是 pattern filter，都能看到一致结果。
        self.SpecifiedTables = !tables_list.is_empty();
        self.Tables = GetConfTables(&tables_list)?;
        self.TableFilter = ParseTableFilter(&tables_list, &filters)?;
        self.FileSize = ParseFileSize(&file_size_str)?;
        // 自定义 SQL 且模板留空时，需要退回匿名模板以保证文件名总是可生成。
        // 否则 SQL-only 导出会缺失 table 上下文，默认模板可能无法渲染。
        if output_filename_format.is_empty() && !self.SQL.is_empty() {
            output_filename_format = DefaultAnonymousOutputFileTemplateText.to_string();
        }
        let tmpl = ParseOutputFileTemplate(&output_filename_format)?;
        let output_split = self.Rows != UnspecifiedSize || self.FileSize != UnspecifiedSize;
        // rows 和 filesize 任一生效，都意味着一个逻辑表可能写出多个物理文件。
        // split 模式必须强制检查模板是否含有可落地的 Index 占位符。
        if flags.Changed("output-filename-template")
            && output_split
            && !outputTemplateUsesIndex(&tmpl, "data")
        {
            // 这里直接在配置阶段报错，比真正写文件时发现覆盖风险更安全。
            return Err(errors_new(
                "--output-filename-template must include a standalone {{.Index}} outside conditional blocks (for example: '{{.DB}}.{{.Table}}.{{.Index}}') when split mode is enabled by --rows/-r or --filesize/-F; otherwise chunk files may overwrite each other",
            ));
        }
        self.OutputFileTemplate = tmpl;
        self.CompressType = parseCompressType(&flags.GetString("compress"))?;
        // 格式相关的派生值放在最后解析，确保前面的 filetype/template 已就位。
        // dialect 只对 CSV 路径有意义，但提前解析能让错误更早暴露。
        let dialect = flags.GetString("csv-output-dialect");
        if !dialect.is_empty() && !self.FileType.eq_ignore_ascii_case(FileFormatCSVString) {
            return Err(errors_errorf(format!(
                "csv-output-dialect is only supported when dumping whole table to csv, not compatible with {}",
                self.FileType
            )));
        }
        self.CsvOutputDialect = ParseOutputDialect(&dialect)?;
        // Parquet 压缩与尺寸解析相互独立，分别有自己的默认值兜底。
        self.ParquetCompressType = parseParquetCompressType(&flags.GetString("parquet-compress"))?;
        self.ParquetPageSize = RAMInBytes(&flags.GetString("parquet-page-size"))
            .map_err(|_| errors_new("failed to parse --parquet-page-size"))?;
        self.ParquetRowGroupSize = RAMInBytes(&flags.GetString("parquet-row-group-size"))
            .map_err(|_| errors_new("failed to parse --parquet-row-group-size"))?;
        // GC 控制相关地址与证书参数直接原样挂入配置，真正使用发生在更后面。
        self.PDAddr = flags.GetString("pd");
        self.ClusterSSLCA = flags.GetString("cluster-tls-ca");
        self.ClusterSSLCert = flags.GetString("cluster-tls-cert");
        self.ClusterSSLKey = flags.GetString("cluster-tls-key");
        // 简化版当前不在这里解析 read-timeout / params / TLS bytes 等更深层配置。
        Ok(())
    }
}
