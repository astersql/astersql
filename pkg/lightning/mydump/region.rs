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

// 将 dump 数据文件切分为可并行导入的 TableRegion。
//
// Region 在此指 Lightning 导入任务分片（文件字节区间 + 行号范围），
// 不是 TiKV 的键范围 Region。大 CSV 会按 region_size 切开；再按引擎
// 容量与 batch_import_ratio 分配 engine_id，以便多引擎并行导入。
use crate::*;
use std::sync::Arc;
/// 压缩文件未知解压长度时使用的“无限”结束偏移哨兵。
pub const TableFileSizeINF: i64 = 10 * 1024 * 1024 * 1024 * 1024;
/// 估算压缩文件行数时相对 real_size 的放大系数。
pub const CompressSizeFactor: i64 = 5;
/// Go `config.DefaultBatchSize`，用于未配置 engine_data_size 的回退值。
const DEFAULT_BATCH_SIZE: f64 = (100_i64 * 1024 * 1024 * 1024) as f64;
/// 避免只略微超过 region_size 的 CSV 被拆分。
const LARGE_CSV_LOWER_THRESHOLD_RATIO: i64 = 10;
#[derive(Clone, Debug)]
/// 单表数据的一个导入分片：绑定引擎、库表、文件元数据与 Chunk 区间。
pub struct TableRegion {
    /// 目标导入引擎编号。
    pub engine_id: i32,
    /// 库名。
    pub db: String,
    /// 表名。
    pub table: String,
    /// 源文件元数据（路径、类型、压缩、大小等）。
    pub file_meta: SourceFileMeta,
    /// 路径路由扩展列数据。
    pub extend_data: ExtendColumnData,
    /// 本分片覆盖的字节偏移与行号范围。
    pub chunk: Chunk,
}
impl TableRegion {
    /// 本分片最小行号（prev_row_id_max + 1）。
    pub fn RowIDMin(&self) -> i64 {
        self.chunk.prev_row_id_max + 1
    }
    /// 本分片估计/实际行数。
    pub fn Rows(&self) -> i64 {
        self.chunk.row_id_max - self.chunk.prev_row_id_max
    }
    /// 分片起始字节偏移。
    pub fn Offset(&self) -> i64 {
        self.chunk.offset
    }
    /// 分片字节长度（end_offset - offset）。
    pub fn Size(&self) -> i64 {
        self.chunk.end_offset - self.chunk.offset
    }
}
/// 按分片大小与 batch 策略为各 TableRegion 分配 engine_id。
///
/// 总数据量不超过 batch_size 时全部落在引擎 0；否则用 beta 函数近似
/// 调整各引擎目标容量，使前几个引擎略小、后续逐步放大（配合 batch_import_ratio）。
pub fn AllocateEngineIDs(
    regions: &mut [TableRegion],
    sizes: &[f64],
    batch_size: f64,
    batch_import_ratio: f64,
    engine_concurrency: f64,
) {
    // 总量够小则无需拆引擎。
    let total: f64 = sizes.iter().sum();
    if total <= batch_size {
        return;
    }
    let mut id = 0i32;
    let mut used = 0.0;
    // 估算所需引擎数，并用 lgamma 计算 Beta 归一化项 inverse_beta。
    let ratio = total * (1.0 - batch_import_ratio) / batch_size;
    let mut engine_count = ratio.ceil();
    let mut inverse_beta = (libm::lgamma(engine_count + batch_import_ratio)
        - libm::lgamma(engine_count)
        - libm::lgamma(batch_import_ratio))
    .exp();
    let mut target = batch_size;
    loop {
        if engine_count <= 0.0 || engine_count > engine_concurrency {
            engine_count = engine_concurrency;
            break;
        }
        let real_ratio = engine_count - inverse_beta;
        if real_ratio >= ratio {
            target = total * (1.0 - batch_import_ratio) / real_ratio;
            break;
        }
        inverse_beta *= 1.0 + batch_import_ratio / engine_count;
        engine_count += 1.0;
    }
    for (region, size) in regions.iter_mut().zip(sizes.iter().copied()) {
        region.engine_id = id;
        used += size;
        if used >= target {
            used = 0.0;
            id += 1;
            let index = id as f64;
            if index >= engine_count {
                target = batch_size;
            } else {
                target *= batch_import_ratio / (engine_count - index) + 1.0;
            }
        }
    }
}
#[derive(Clone, Debug)]
/// 数据切分与引擎分配的配置参数。
pub struct DataDivideConfig {
    /// 列数（部分格式可能用到）。
    pub column_count: usize,
    /// 单个引擎目标数据量（字节）。
    pub engine_data_size: f64,
    /// 批量导入比例，影响引擎容量递增。
    pub batch_import_ratio: f64,
    /// 引擎并发上限。
    pub engine_concurrency: usize,
    /// 大 CSV 按此字节阈值切分 Region。
    pub region_size: i64,
    /// 是否启用严格格式校验。
    pub strict_format: bool,
    /// CSV 解析配置。
    pub csv: CsvConfig,
    /// 字符集名。
    pub charset: String,
    /// 非法字符替换串。
    pub invalid_char_replacement: String,
    /// 可选 I/O 工作池。
    pub io_workers: Option<Arc<WorkerPool>>,
}
/// 构造带默认阈值的 DataDivideConfig（引擎 100GiB、region 256MiB 等）。
pub fn NewDataDivideConfig() -> DataDivideConfig {
    DataDivideConfig {
        column_count: 0,
        engine_data_size: 100.0 * 1024.0 * 1024.0 * 1024.0,
        batch_import_ratio: 0.75,
        engine_concurrency: 4,
        region_size: 256 * 1024 * 1024,
        strict_format: false,
        csv: CsvConfig::default(),
        charset: "utf8mb4".into(),
        invalid_char_replacement: "�".into(),
        io_workers: None,
    }
}
/// 按表的数据文件列表生成 TableRegion，并分配引擎 ID。
pub fn MakeTableRegions(
    table: &MDTableMeta,
    cfg: &DataDivideConfig,
    store: &dyn Storage,
) -> Result<Vec<TableRegion>, MydumpError> {
    let mut regions = Vec::new();
    let mut sizes = Vec::new();
    let mut prev_row = 0;
    for file in &table.data_files {
        let meta = SourceFileMeta {
            path: file.file_meta.path.clone(),
            source_type: file.file_meta.source_type,
            compression: file.file_meta.compression,
            sort_key: file.file_meta.sort_key.clone(),
            file_size: file.file_meta.file_size,
            real_size: file.file_meta.real_size,
            extend_data: file.extend_data.clone(),
            ..Default::default()
        };
        // 大 CSV 切分；Parquet 单独处理；其余整文件一个 Region。
        let (mut parts, part_sizes) = match meta.source_type {
            SourceType::Csv
                if cfg.strict_format
                    && meta.compression == Compression::None
                    && meta.file_size
                        > cfg.region_size + cfg.region_size / LARGE_CSV_LOWER_THRESHOLD_RATIO =>
            {
                let parts = SplitLargeCSV(&meta, cfg, store)?;
                let sizes = parts.iter().map(|part| part.Size() as f64).collect();
                (parts, sizes)
            }
            SourceType::Parquet => (
                vec![makeParquetFileRegion(&meta, 0)],
                vec![meta.real_size as f64],
            ),
            _ => (
                vec![MakeSourceFileRegion(&meta, 0, cfg.column_count)],
                vec![meta.real_size as f64],
            ),
        };
        let row_id_base = prev_row;
        for part in &mut parts {
            part.db = table.db.clone();
            part.table = table.name.clone();
            part.chunk.prev_row_id_max += row_id_base;
            part.chunk.row_id_max += row_id_base;
        }
        if let Some(last) = parts.last() {
            prev_row = last.chunk.row_id_max;
        }
        regions.extend(parts);
        sizes.extend(part_sizes);
    }
    let batch = CalculateBatchSize(
        cfg.engine_data_size,
        table.is_row_ordered,
        table.total_size as f64,
    );
    AllocateEngineIDs(
        &mut regions,
        &sizes,
        batch,
        cfg.batch_import_ratio,
        cfg.engine_concurrency as f64,
    );
    Ok(regions)
}
/// 计算引擎 batch 大小：行有序时用配置值，否则按总量均分并设下限。
pub fn CalculateBatchSize(size: f64, is_row_ordered: bool, total: f64) -> f64 {
    if size > 0.0 {
        size
    } else if is_row_ordered {
        DEFAULT_BATCH_SIZE
    } else {
        DEFAULT_BATCH_SIZE.max(total)
    }
}
/// 为单个源文件构造覆盖整文件的 TableRegion，并估算行号上界。
pub fn MakeSourceFileRegion(
    meta: &SourceFileMeta,
    prev_row: i64,
    column_count: usize,
) -> TableRegion {
    // 压缩文件 end_offset 用 INF；行数按 real_size 与系数估算。
    let compressed = meta.compression != Compression::None;
    let divisor = if meta.source_type == SourceType::Csv {
        column_count as i64
    } else {
        column_count as i64 + 2
    };
    let rows = if compressed {
        meta.real_size * CompressSizeFactor / divisor
    } else {
        meta.file_size / divisor
    };
    TableRegion {
        engine_id: 0,
        db: String::new(),
        table: String::new(),
        file_meta: meta.clone(),
        extend_data: meta.extend_data.clone(),
        chunk: Chunk {
            offset: 0,
            end_offset: if compressed {
                TableFileSizeINF
            } else {
                meta.file_size
            },
            real_offset: 0,
            prev_row_id_max: prev_row,
            row_id_max: prev_row + rows,
            columns: Vec::new(),
        },
    }
}
/// Parquet 文件 Region：end_offset 为 i64::MAX，行号优先用 meta.rows。
pub fn makeParquetFileRegion(meta: &SourceFileMeta, prev_row: i64) -> TableRegion {
    TableRegion {
        engine_id: 0,
        db: String::new(),
        table: String::new(),
        file_meta: meta.clone(),
        extend_data: meta.extend_data.clone(),
        chunk: Chunk {
            offset: 0,
            end_offset: i64::MAX,
            real_offset: 0,
            prev_row_id_max: prev_row,
            row_id_max: prev_row
                + if meta.rows > 0 {
                    meta.rows
                } else {
                    meta.file_size
                },
            columns: Vec::new(),
        },
    }
}
/// 打开 CSV 文件并构造带字符集转换的 CsvParser。
pub fn openCSVParser(
    meta: &SourceFileMeta,
    cfg: &DataDivideConfig,
    store: &dyn Storage,
) -> Result<CsvParser, MydumpError> {
    let mut raw = Vec::new();
    store
        .open(&meta.path, meta.compression)?
        .read_to_end(&mut raw)?;
    let reader = Box::new(StringReader::from_bytes(raw));
    let convertor = NewCharsetConvertor(&cfg.charset, &cfg.invalid_char_replacement)?;
    NewCSVParser(&cfg.csv, reader, false, Some(convertor))
}
/// 读取 CSV 表头列名。
pub fn getHeaderColumn(parser: &mut CsvParser) -> Result<(Vec<String>, i64), MydumpError> {
    let columns = parser.ReadColumns()?;
    let (mut data_start, _) = parser.Pos();
    // Go CSV parser reports CRLF headers at the CR byte when no record has yet
    // been split. Preserve that boundary so subsequent regions match Go.
    if data_start >= 2 {
        parser.SetPos(data_start - 2, 0)?;
        if parser.peekBytes(2) == Some(b"\r\n") {
            data_start -= 1;
        }
    }
    parser.SetPos(data_start, 0)?;
    Ok((columns, data_start))
}
/// 将大 CSV 按 region_size 切成多个 TableRegion，可选保留表头列名。
pub fn SplitLargeCSV(
    meta: &SourceFileMeta,
    cfg: &DataDivideConfig,
    store: &dyn Storage,
) -> Result<Vec<TableRegion>, MydumpError> {
    let mut parser = openCSVParser(meta, cfg, store)?;
    let (columns, data_start) = if cfg.csv.header {
        getHeaderColumn(&mut parser)?
    } else {
        (Vec::new(), 0)
    };
    let remaining = meta.file_size - data_start;
    let region_count = (remaining + cfg.region_size - 1) / cfg.region_size;
    let quotient = remaining / region_count;
    let remainder = remaining % region_count;
    let mut split_points = Vec::with_capacity(region_count.saturating_sub(1) as usize);
    let mut point = data_start;
    for index in 0..region_count - 1 {
        point += quotient + i64::from(index < remainder);
        split_points.push(point);
    }
    for split_point in &mut split_points {
        let mut split_parser = openCSVParser(meta, cfg, store)?;
        split_parser.SetPos(*split_point, 0)?;
        match split_parser.ReadUntilTerminator() {
            Ok(()) => *split_point = split_parser.Pos().0,
            Err(MydumpError::Eof) => *split_point = meta.file_size,
            Err(error) => return Err(error),
        }
    }
    split_points.push(meta.file_size);

    let divisor = cfg.column_count as i64;
    let mut previous_offset = data_start;
    let mut previous_row_id_max = 0;
    let mut regions = Vec::with_capacity(split_points.len());
    for end_offset in split_points {
        if previous_offset == end_offset {
            continue;
        }
        let row_id_max = previous_row_id_max + (end_offset - previous_offset) / divisor;
        regions.push(TableRegion {
            engine_id: 0,
            db: String::new(),
            table: String::new(),
            file_meta: meta.clone(),
            extend_data: meta.extend_data.clone(),
            chunk: Chunk {
                offset: previous_offset,
                end_offset,
                real_offset: 0,
                prev_row_id_max: previous_row_id_max,
                row_id_max,
                columns: columns.clone(),
            },
        });
        previous_offset = end_offset;
        previous_row_id_max = row_id_max;
    }
    Ok(regions)
}
