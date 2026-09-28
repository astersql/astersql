// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// mydump 目录加载器：扫描存储中的 dump 文件并组装库/表/视图元数据。
//
// 通过 FileRouter 将路径映射为 schema/table/类型，应用 filter 规则，汇总
// schema 文件与数据分片。支持自定义 FileIterator、扫描上限与压缩体积估算。
// MDLoader 是后续 Region 切分与导入的入口元数据源。

use crate::*;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Read;
use std::sync::{Arc, Mutex};

/// 估算压缩比时采样的最大解压字节数。
const SAMPLE_COMPRESSED_FILE_SIZE: usize = 4096;
#[derive(Clone, Debug, Default)]
/// 一个数据库的 dump 元数据：建库语句文件、表与视图列表、字符集。
pub struct MDDatabaseMeta {
    pub name: String,
    pub schema_file: FileInfo,
    pub tables: Vec<MDTableMeta>,
    pub views: Vec<MDTableMeta>,
    pub char_set: String,
}
/// 以给定字符集构造空的数据库元数据。
pub fn NewMDDatabaseMeta(char_set: &str) -> MDDatabaseMeta {
    MDDatabaseMeta {
        char_set: char_set.into(),
        ..Default::default()
    }
}
impl MDDatabaseMeta {
    /// 读取建库 SQL；缺失时回退为 `CREATE DATABASE IF NOT EXISTS`。
    pub fn GetSchema(&self, store: &dyn Storage) -> String {
        if !self.schema_file.file_meta.path.is_empty() {
            if let Ok(schema) = ExportStatement(store, &self.schema_file, &self.char_set) {
                let text = String::from_utf8_lossy(&schema).trim().to_owned();
                if !text.is_empty() {
                    return text;
                }
            }
        }
        format!(
            "CREATE DATABASE IF NOT EXISTS `{}`",
            self.name.replace('`', "``")
        )
    }
}
#[derive(Clone, Debug, Default)]
/// 表或视图元数据：schema 文件、数据分片、总大小与行序标志。
pub struct MDTableMeta {
    pub db: String,
    pub name: String,
    pub schema_file: FileInfo,
    pub data_files: Vec<FileInfo>,
    pub char_set: String,
    pub total_size: i64,
    pub index_ratio: f64,
    pub is_row_ordered: bool,
}
/// 以给定字符集构造空表元数据（默认 is_row_ordered=true）。
pub fn NewMDTableMeta(char_set: &str) -> MDTableMeta {
    MDTableMeta {
        char_set: char_set.into(),
        is_row_ordered: true,
        ..Default::default()
    }
}
impl MDTableMeta {
    /// 读取建表/建视图 SQL；schema 文件缺失则返回 Schema 错误。
    pub fn GetSchema(&self, store: &dyn Storage) -> Result<String, MydumpError> {
        if self.schema_file.file_meta.path.is_empty() {
            return Err(MydumpError::Schema(format!(
                "schema file for {}.{} not found",
                self.db, self.name
            )));
        }
        Ok(
            String::from_utf8_lossy(&ExportStatement(store, &self.schema_file, &self.char_set)?)
                .trim()
                .into(),
        )
    }
}
#[derive(Clone, Debug, Default)]
/// 源文件扩展元数据：真实大小、行数与路由扩展列。
pub struct SourceFileMeta {
    pub path: String,
    pub source_type: SourceType,
    pub compression: Compression,
    pub sort_key: String,
    pub file_size: i64,
    pub extend_data: ExtendColumnData,
    pub real_size: i64,
    pub rows: i64,
}
/// Loader 扫描阶段配置：文件上限、并发、是否跳过体积估算等。
pub struct MDLoaderSetupConfig {
    pub max_scan_files: usize,
    pub scan_file_concurrency: usize,
    pub skip_real_size_estimation: bool,
    pub support_partial_result: bool,
    pub file_iterator: Option<Arc<dyn FileIterator>>,
}
/// 默认几乎无上限扫描，并发 8，不跳过体积估算。
impl Default for MDLoaderSetupConfig {
    fn default() -> Self {
        Self {
            max_scan_files: usize::MAX,
            scan_file_concurrency: 8,
            skip_real_size_estimation: false,
            support_partial_result: false,
            file_iterator: None,
        }
    }
}
/// 返回默认扫描配置。
pub fn DefaultMDLoaderSetupConfig() -> MDLoaderSetupConfig {
    Default::default()
}
/// 一次性修改 MDLoaderSetupConfig 的闭包选项。
pub type MDLoaderSetupOption = Box<dyn FnOnce(&mut MDLoaderSetupConfig) + Send>;
/// 限制最多扫描文件数，并开启部分结果支持。
pub fn WithMaxScanFiles(v: usize) -> MDLoaderSetupOption {
    Box::new(move |c| {
        if v > 0 {
            c.max_scan_files = v;
            c.support_partial_result = true;
        }
    })
}
/// 设置扫描并发度（至少为 1）。
pub fn WithScanFileConcurrency(v: usize) -> MDLoaderSetupOption {
    Box::new(move |c| c.scan_file_concurrency = v.max(1))
}
/// 是否跳过压缩文件真实大小估算。
pub fn WithSkipRealSizeEstimation(v: bool) -> MDLoaderSetupOption {
    Box::new(move |c| c.skip_real_size_estimation = v)
}
/// 出错时是否仍返回已扫描的部分结果。
pub fn ReturnPartialResultOnError(v: bool) -> MDLoaderSetupOption {
    Box::new(move |c| c.support_partial_result = v)
}
/// 注入自定义文件枚举器，替代默认 `store.list()`。
pub fn WithFileIterator(v: Arc<dyn FileIterator>) -> MDLoaderSetupOption {
    Box::new(move |c| c.file_iterator = Some(v))
}
#[derive(Clone, Debug)]
/// Loader 业务配置：字符集、路由规则与表过滤器。
pub struct LoaderConfig {
    pub char_set: String,
    pub file_routes: Vec<FileRouteRule>,
    pub filter: Vec<String>,
}
/// 默认 utf8mb4 与内置文件路由规则。
impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            char_set: "utf8mb4".into(),
            file_routes: default_file_route_rules(),
            filter: Vec::new(),
        }
    }
}
/// 克隆一份 LoaderConfig。
pub fn NewLoaderCfg(c: &LoaderConfig) -> LoaderConfig {
    c.clone()
}
#[derive(Clone, Debug)]
/// FileIterator 产出的原始路径与大小。
pub struct RawFile {
    pub path: String,
    pub size: i64,
}
/// 枚举待加载文件的抽象；用于外部数据源注入。
pub trait FileIterator: Send + Sync {
    fn IterateFiles(
        &self,
        handler: &mut dyn FnMut(&str, i64) -> Result<(), MydumpError>,
    ) -> Result<(), MydumpError>;
}
/// 基于预置文件列表的迭代器。
pub struct AllFileIterator {
    pub files: Vec<RawFile>,
}
/// 依次回调列表中每个路径与大小。
impl FileIterator for AllFileIterator {
    fn IterateFiles(
        &self,
        h: &mut dyn FnMut(&str, i64) -> Result<(), MydumpError>,
    ) -> Result<(), MydumpError> {
        for f in &self.files {
            h(&f.path, f.size)?
        }
        Ok(())
    }
}
/// 已组装的库表元数据、底层 Storage 与全量文件索引。
pub struct MDLoader {
    databases: Vec<MDDatabaseMeta>,
    store: Arc<dyn Storage>,
    all_files: HashMap<String, FileInfo>,
    filter: Vec<String>,
}
/// 使用配置与选项构造 MDLoader（委托 NewLoaderWithStore）。
pub fn NewLoader(
    cfg: LoaderConfig,
    store: Arc<dyn Storage>,
    options: Vec<MDLoaderSetupOption>,
) -> Result<MDLoader, MydumpError> {
    NewLoaderWithStore(cfg, store, options)
}
/// 扫描文件、路由分类并构建数据库/表/视图元数据树。
pub fn NewLoaderWithStore(
    cfg: LoaderConfig,
    store: Arc<dyn Storage>,
    options: Vec<MDLoaderSetupOption>,
) -> Result<MDLoader, MydumpError> {
    let mut setup = DefaultMDLoaderSetupConfig();
    for option in options {
        option(&mut setup)
    }
    let router = NewFileRouter(&cfg.file_routes, Logger::default())?;
    let mut files = Vec::new();
    if let Some(iterator) = setup.file_iterator {
        iterator.IterateFiles(&mut |path, size| {
            files.push((path.to_owned(), size));
            Ok(())
        })?;
    } else {
        files = store.list()?;
    }
    // 路径排序后截断到 max_scan_files，保证部分扫描结果稳定。
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.truncate(setup.max_scan_files);
    let mut dbs: BTreeMap<String, MDDatabaseMeta> = BTreeMap::new();
    for (path, size) in files {
        // 无路由命中、Ignore 类型或 filter 排除的文件直接跳过。
        let Some(route) = router.Route(&path)? else {
            continue;
        };
        if route.source_type == SourceType::Ignore {
            continue;
        }
        if should_skip_rules(&cfg.filter, &route.schema, &route.name) {
            continue;
        }
        let info = FileInfo {
            file_meta: FileMeta {
                path: path.clone(),
                file_size: size,
                real_size: if setup.skip_real_size_estimation
                    || route.compression == Compression::None
                {
                    size
                } else {
                    EstimateRealSizeForFile(
                        &SourceFileMeta {
                            path: path.clone(),
                            source_type: route.source_type,
                            compression: route.compression,
                            file_size: size,
                            ..Default::default()
                        },
                        store.as_ref(),
                    )
                },
                source_type: route.source_type,
                compression: route.compression,
                sort_key: route.key.clone(),
            },
            ..Default::default()
        };
        let db = dbs.entry(route.schema.clone()).or_insert_with(|| {
            let mut d = NewMDDatabaseMeta(&cfg.char_set);
            d.name = route.schema.clone();
            d
        });
        match route.source_type {
            SourceType::SchemaSchema => {
                if !db.schema_file.file_meta.path.is_empty() {
                    return Err(MydumpError::Schema(format!(
                        "invalid database schema file, duplicated item - {path}"
                    )));
                }
                db.schema_file = info.clone();
            }
            SourceType::ViewSchema => {
                if db.views.iter().any(|view| {
                    view.name == route.name && !view.schema_file.file_meta.path.is_empty()
                }) {
                    return Err(MydumpError::Schema(format!(
                        "invalid view schema file, duplicated item - {path}"
                    )));
                }
                insert_meta(&mut db.views, &route, &info, &cfg.char_set);
            }
            SourceType::TableSchema | SourceType::Sql | SourceType::Csv | SourceType::Parquet => {
                if route.source_type == SourceType::TableSchema
                    && db.tables.iter().any(|table| {
                        table.name == route.name && !table.schema_file.file_meta.path.is_empty()
                    })
                {
                    return Err(MydumpError::Schema(format!(
                        "invalid table schema file, duplicated item - {path}"
                    )));
                }
                insert_meta(&mut db.tables, &route, &info, &cfg.char_set)
            }
            _ => {}
        }
    }
    let mut databases = dbs.into_values().collect::<Vec<_>>();
    pruneViewPlaceholders(&mut databases);
    for database in &mut databases {
        database
            .tables
            .sort_by(|left, right| left.total_size.cmp(&right.total_size));
    }
    let mut all_files = HashMap::new();
    for database in &databases {
        for table in &database.tables {
            if !table.schema_file.file_meta.path.is_empty() {
                all_files.insert(
                    table.schema_file.file_meta.path.clone(),
                    table.schema_file.clone(),
                );
            }
            for file in &table.data_files {
                all_files.insert(file.file_meta.path.clone(), file.clone());
            }
        }
    }
    Ok(MDLoader {
        databases,
        store,
        all_files,
        filter: cfg.filter,
    })
}
/// 按路由结果插入或更新表/视图元数据（schema 或数据分片）。
fn insert_meta(list: &mut Vec<MDTableMeta>, route: &RouteResult, info: &FileInfo, charset: &str) {
    let i = list
        .iter()
        .position(|t| t.name == route.name)
        .unwrap_or_else(|| {
            let mut t = NewMDTableMeta(charset);
            t.db = route.schema.clone();
            t.name = route.name.clone();
            list.push(t);
            list.len() - 1
        });
    let table = &mut list[i];
    match route.source_type {
        SourceType::TableSchema | SourceType::ViewSchema => table.schema_file = info.clone(),
        SourceType::Sql | SourceType::Csv | SourceType::Parquet => {
            table.total_size += info.file_meta.real_size;
            table.data_files.push(info.clone());
            table
                .data_files
                .sort_by(|a, b| a.file_meta.sort_key.cmp(&b.file_meta.sort_key))
        }
        _ => {}
    }
}
impl MDLoader {
    /// 返回已加载的数据库元数据切片。
    pub fn GetDatabases(&self) -> &[MDDatabaseMeta] {
        &self.databases
    }
    /// 返回底层 Storage 的 Arc 克隆。
    pub fn GetStore(&self) -> Arc<dyn Storage> {
        Arc::clone(&self.store)
    }
    /// 返回路径到 FileInfo 的全量索引。
    pub fn GetAllFiles(&self) -> &HashMap<String, FileInfo> {
        &self.all_files
    }
    /// 按 filter 规则判断 schema.table 是否应跳过。
    pub fn shouldSkip(&self, schema: &str, table: &str) -> bool {
        should_skip_rules(&self.filter, schema, table)
    }
}

/// 解释 `!` 排除前缀与 glob，决定目标是否未被包含。
fn should_skip_rules(filter: &[String], schema: &str, table: &str) -> bool {
    if filter.is_empty() {
        return false;
    }
    let target = if table.is_empty() {
        schema.to_owned()
    } else {
        format!("{schema}.{table}")
    };
    let mut included = false;
    for rule in filter {
        let (exclude, pattern) = rule
            .strip_prefix('!')
            .map_or((false, rule.as_str()), |pattern| (true, pattern));
        let pattern = if table.is_empty() {
            pattern
                .split_once('.')
                .map_or(pattern, |(schema, _)| schema)
        } else {
            pattern
        };
        if glob_match(pattern, &target) {
            included = !exclude;
        }
    }
    !included
}

/// 简化 glob：`*` 任意长度、`?` 单字符，大小写不敏感。
fn glob_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut reachable = vec![false; value.len() + 1];
    reachable[0] = true;
    for &token in pattern {
        // DP：reachable[j] 表示 pattern 前缀能否匹配 value[..j]。
        if token == b'*' {
            for index in 1..=value.len() {
                reachable[index] |= reachable[index - 1];
            }
        } else {
            for index in (1..=value.len()).rev() {
                reachable[index] = reachable[index - 1]
                    && (token == b'?' || token.eq_ignore_ascii_case(&value[index - 1]));
            }
            reachable[0] = false;
        }
    }
    reachable[value.len()]
}
/// 固定并发工作线程池处理任务队列，按输入顺序汇总结果。
pub fn ParallelProcess<T: Send + 'static, R: Send + 'static>(
    items: Vec<T>,
    concurrency: usize,
    process: impl Fn(T) -> Result<R, MydumpError> + Send + Sync + 'static,
) -> Result<Vec<R>, MydumpError> {
    let count = items.len();
    let q = Arc::new(Mutex::new(
        items.into_iter().enumerate().collect::<VecDeque<_>>(),
    ));
    let out = Arc::new(Mutex::new(Vec::new()));
    let err = Arc::new(Mutex::new(None));
    let p = Arc::new(process);
    std::thread::scope(|s| {
        for _ in 0..concurrency.max(1) {
            let (q, o, e, p) = (q.clone(), out.clone(), err.clone(), p.clone());
            s.spawn(move || {
                loop {
                    if e.lock().unwrap().is_some() {
                        break;
                    }
                    let Some((i, v)) = q.lock().unwrap().pop_front() else {
                        break;
                    };
                    match p(v) {
                        Ok(v) => o.lock().unwrap().push((i, v)),
                        Err(x) => {
                            *e.lock().unwrap() = Some(x);
                            break;
                        }
                    }
                }
            });
        }
    });
    if let Some(e) = err.lock().unwrap().take() {
        return Err(e);
    }
    let mut values = out.lock().unwrap().drain(..).collect::<Vec<_>>();
    values.sort_by_key(|x| x.0);
    debug_assert_eq!(values.len(), count);
    Ok(values.into_iter().map(|x| x.1).collect())
}
/// 累加文件 file_size。
pub fn calculateFileBytes(files: &[FileInfo]) -> i64 {
    files.iter().map(|f| f.file_meta.file_size).sum()
}
/// 无压缩直接返回 file_size；否则按采样压缩比估算。
pub fn EstimateRealSizeForFile(meta: &SourceFileMeta, store: &dyn Storage) -> i64 {
    if meta.compression == Compression::None {
        return meta.file_size;
    }
    SampleFileCompressRatio(meta, store)
        .map(|r| (meta.file_size as f64 * r) as i64)
        .unwrap_or(meta.file_size)
}
/// 解压采样前缀估算压缩比（解压字节/采样压缩字节）。
pub fn SampleFileCompressRatio(
    meta: &SourceFileMeta,
    store: &dyn Storage,
) -> Result<f64, MydumpError> {
    if meta.compression == Compression::None {
        return Ok(1.0);
    }
    let mut reader = store.open(&meta.path, meta.compression)?;
    let mut sample = Vec::new();
    reader
        .by_ref()
        .take(SAMPLE_COMPRESSED_FILE_SIZE as u64)
        .read_to_end(&mut sample)?;
    Ok((sample.len() as f64
        / (meta.file_size.max(1) as f64).min(SAMPLE_COMPRESSED_FILE_SIZE as f64))
    .max(1.0))
}
/// 自由函数形式的 GetAllFiles。
pub fn GetAllFiles(l: &MDLoader) -> &HashMap<String, FileInfo> {
    l.GetAllFiles()
}
/// 自由函数形式的 GetDatabases。
pub fn GetDatabases(l: &MDLoader) -> &[MDDatabaseMeta] {
    l.GetDatabases()
}
/// 自由函数形式的 GetStore。
pub fn GetStore(l: &MDLoader) -> Arc<dyn Storage> {
    l.GetStore()
}
/// 调用 FileIterator::IterateFiles。
pub fn IterateFiles(
    i: &dyn FileIterator,
    h: &mut dyn FnMut(&str, i64) -> Result<(), MydumpError>,
) -> Result<(), MydumpError> {
    i.IterateFiles(h)
}
/// 由路由结果构造 FileInfo。
pub fn constructFileInfo(path: &str, size: i64, r: &RouteResult) -> FileInfo {
    FileInfo {
        file_meta: FileMeta {
            path: path.into(),
            file_size: size,
            real_size: size,
            source_type: r.source_type,
            compression: r.compression,
            sort_key: r.key.clone(),
        },
        ..Default::default()
    }
}
/// 确保 BTreeMap 中存在对应库，SchemaSchema 时更新建库文件。
pub fn insertDB<'a>(
    databases: &'a mut BTreeMap<String, MDDatabaseMeta>,
    route: &RouteResult,
    info: &FileInfo,
    charset: &str,
) -> (&'a mut MDDatabaseMeta, bool) {
    let existed = databases.contains_key(&route.schema);
    let database = databases.entry(route.schema.clone()).or_insert_with(|| {
        let mut database = NewMDDatabaseMeta(charset);
        database.name = route.schema.clone();
        database.schema_file = info.clone();
        database
    });
    if route.source_type == SourceType::SchemaSchema {
        database.schema_file = info.clone();
    }
    (database, existed)
}
/// 向库中插入/更新表元数据，返回此前是否已存在同名表。
pub fn insertTable(
    database: &mut MDDatabaseMeta,
    route: &RouteResult,
    info: &FileInfo,
    charset: &str,
) -> bool {
    let existed = database.tables.iter().any(|table| table.name == route.name);
    insert_meta(&mut database.tables, route, info, charset);
    existed
}
/// 向库中插入/更新视图元数据，返回此前是否已存在同名视图。
pub fn insertView(
    database: &mut MDDatabaseMeta,
    route: &RouteResult,
    info: &FileInfo,
    charset: &str,
) -> bool {
    let existed = database.views.iter().any(|view| view.name == route.name);
    insert_meta(&mut database.views, route, info, charset);
    existed
}
/// 移除与视图同名的占位表（mydumper 可能先输出同名表 schema）。
pub fn pruneViewPlaceholders(databases: &mut [MDDatabaseMeta]) -> usize {
    let mut pruned = 0;
    for database in databases {
        let views = database
            // 视图名集合：同名表视为占位符并剔除。
            .views
            .iter()
            .map(|view| view.name.as_str())
            .collect::<std::collections::HashSet<_>>();
        let old_len = database.tables.len();
        database
            .tables
            .retain(|table| !views.contains(table.name.as_str()));
        pruned += old_len - database.tables.len();
    }
    pruned
}
/// 自由函数形式的 shouldSkip。
pub fn shouldSkip(loader: &MDLoader, schema: &str, table: &str) -> bool {
    loader.shouldSkip(schema, table)
}
/// 校验扫描并发度必须为正。
pub fn setup(config: &MDLoaderSetupConfig) -> Result<(), MydumpError> {
    if config.scan_file_concurrency == 0 {
        Err(MydumpError::Configuration(
            "scan file concurrency must be positive".into(),
        ))
    } else {
        Ok(())
    }
}
/// 对单路径执行路由。
pub fn route(router: &dyn FileRouter, path: &str) -> Result<Option<RouteResult>, MydumpError> {
    router.Route(path)
}
