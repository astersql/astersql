// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TTL 物理表元信息、过期时间计算与扫描范围拆分。
//
// 将 infoschema 中的表/分区解析为可扫描的 `PhysicalTable`，按主键类型
// 把 TiKV Region 边界转成 Datum 扫描区间，并按 TTL interval 计算过期时间点。
// 扫描范围统一使用半开区间 `[start, end)`，与 Go 的 TTL 扫描约定保持一致。

// TTL 物理表元信息、过期时间计算和扫描范围拆分逻辑。

// getTableKeyColumns 对应 Go 中根据表句柄形态选择 TTL 扫描键列的逻辑。
// 返回的列与 FieldType 顺序必须和 Go 保持一致，后续 SplitScanRanges 依赖第一个键列类型。

use crate::task::Datum;

/// 扫描键列的值类型：有符号/无符号整数或字节串。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeyKind {
    #[default]
    SignedInt,
    UnsignedInt,
    Bytes,
    Float,
    Set,
}
/// 简化列元信息：名称、是否 public、键类型。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Column {
    pub id: i64,
    pub name: String,
    pub public: bool,
    pub key_kind: KeyKind,
    pub nullable: bool,
    pub hidden: bool,
}
/// TTL 次级索引选择所需的索引列元信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexColumn {
    pub column_offset: usize,
    pub prefix_length: Option<usize>,
}
/// TTL 次级索引选择所需的索引元信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<IndexColumn>,
    pub unique: bool,
    pub primary: bool,
    pub public: bool,
    pub invisible: bool,
    pub global: bool,
    pub multi_valued: bool,
    pub columnar: bool,
    pub conditional: bool,
}
/// 分区定义：物理分区 ID 与名称。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
}
/// 表级 TTL 配置：时间列名、间隔数值与时间单位。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TTLInfo {
    pub column_name: String,
    pub interval: String,
    pub unit: TimeUnit,
}
/// TTL 间隔时间单位（与 `TTL_JOB_INTERVAL` 语义对齐）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimeUnit {
    Microsecond,
    Second,
    Minute,
    HourMinute,
    Hour,
    Day,
    Week,
    #[default]
    Month,
    Quarter,
    Year,
}
/// 构造 PhysicalTable 所需的最小表元信息子集。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub public: bool,
    pub pk_is_handle: bool,
    pub common_handle: bool,
    pub columns: Vec<Column>,
    pub primary_index_offsets: Vec<usize>,
    pub indexes: Vec<IndexInfo>,
    pub partitions: Vec<PartitionDefinition>,
    pub ttl: Option<TTLInfo>,
}

/// 按句柄形态选择 TTL 扫描键列：整型主键 / 聚簇索引 / 隐式 `_tidb_rowid`。
pub fn getTableKeyColumns(table: &TableInfo) -> Result<Vec<Column>, String> {
    if table.pk_is_handle {
        return table
            .columns
            .iter()
            .find(|column| {
                column.public
                    && matches!(column.key_kind, KeyKind::SignedInt | KeyKind::UnsignedInt)
            })
            .cloned()
            .map(|column| vec![column])
            .ok_or_else(|| format!("Cannot find primary key for table: {}", table.name));
    }
    if table.common_handle {
        if table.primary_index_offsets.is_empty() {
            return Err(format!("Cannot find primary key for table: {}", table.name));
        }
        return table
            .primary_index_offsets
            .iter()
            .map(|offset| {
                table
                    .columns
                    .get(*offset)
                    .filter(|column| column.public)
                    .cloned()
                    .ok_or_else(|| format!("invalid primary key column offset {offset}"))
            })
            .collect();
    }
    // 无用户主键时 TiDB 使用隐式 row id 作为记录句柄。
    Ok(vec![Column {
        name: "_tidb_rowid".into(),
        id: -1,
        public: true,
        key_kind: KeyKind::SignedInt,
        nullable: false,
        hidden: false,
    }])
}

/// 逻辑扫描区间：Start/End 为空表示该侧开放（全表或半开）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanRange {
    pub Start: Vec<Datum>,
    pub End: Vec<Datum>,
}
/// 构造覆盖整表的开放区间。
pub fn newFullRange() -> ScanRange {
    ScanRange::default()
}
/// 由起止 Datum 构造区间；Null 表示该侧开放。
pub fn newDatumRange(start: Datum, end: Datum) -> ScanRange {
    ScanRange {
        Start: if matches!(start, Datum::Null) {
            Vec::new()
        } else {
            vec![start]
        },
        End: if matches!(end, Datum::Null) {
            Vec::new()
        } else {
            vec![end]
        },
    }
}
/// 返回表示开放边界的 Null Datum。
pub fn nullDatum() -> Datum {
    Datum::Null
}

/// 可执行 TTL 扫描的物理表视图（含分区与键列）。
#[derive(Clone, Debug)]
pub struct PhysicalTable {
    pub ID: i64,
    pub Schema: String,
    pub TableInfo: TableInfo,
    pub Partition: String,
    pub PartitionDef: Option<PartitionDefinition>,
    pub KeyColumns: Vec<Column>,
    pub TimeColumn: Column,
    pub Indices: Vec<IndexInfo>,
}
/// 在已知时间列前提下构造 PhysicalTable，并解析分区物理 ID。
pub fn NewBasePhysicalTable(
    schema: &str,
    table: &TableInfo,
    partition: &str,
    time_column: Column,
) -> Result<PhysicalTable, String> {
    if !table.public {
        return Err(format!(
            "table '{}.{}' is not a public table",
            schema, table.name
        ));
    }
    let key_columns = getTableKeyColumns(table)?;
    let (id, definition) = if table.partitions.is_empty() {
        if !partition.is_empty() {
            return Err(format!(
                "table '{}.{}' is not a partitioned table",
                schema, table.name
            ));
        }
        (table.id, None)
    } else {
        if partition.is_empty() {
            return Err(format!(
                "partition name is required, table '{}.{}' is a partitioned table",
                schema, table.name
            ));
        }
        let definition = table
            .partitions
            .iter()
            .find(|definition| definition.name.eq_ignore_ascii_case(partition))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "partition '{partition}' is not found in ttl table '{}.{}'",
                    schema, table.name
                )
            })?;
        (definition.id, Some(definition))
    };
    Ok(PhysicalTable {
        ID: id,
        Schema: schema.into(),
        TableInfo: table.clone(),
        Partition: partition.into(),
        PartitionDef: definition,
        KeyColumns: key_columns,
        TimeColumn: time_column,
        Indices: table.indexes.clone(),
    })
}
/// 校验表启用 TTL 后查找时间列，再委托 NewBasePhysicalTable。
pub fn NewPhysicalTable(
    schema: &str,
    table: &TableInfo,
    partition: &str,
) -> Result<PhysicalTable, String> {
    let ttl = table
        .ttl
        .as_ref()
        .ok_or_else(|| format!("table '{}.{}' is not a ttl table", schema, table.name))?;
    let time_column = table
        .columns
        .iter()
        .find(|column| column.public && column.name.eq_ignore_ascii_case(&ttl.column_name))
        .cloned()
        .ok_or_else(|| {
            format!(
                "time column '{}' is not public in ttl table '{}.{}'",
                ttl.column_name, schema, table.name
            )
        })?;
    NewBasePhysicalTable(schema, table, partition, time_column)
}
impl PhysicalTable {
    /// 按物理索引顺序描述 SELECT 列、ORDER BY 列和表键投影。
    pub fn BuildTTLIndexScanPlan(&self, index: &IndexInfo) -> Result<TTLIndexScanPlan, String> {
        if !index.public
            || index.invisible
            || index.global
            || index.multi_valued
            || index.columnar
            || index.conditional
            || index.columns.is_empty()
        {
            return Err(format!(
                "index {} is not a supported TTL scan index",
                index.name
            ));
        }
        if index.primary && (self.TableInfo.pk_is_handle || self.TableInfo.common_handle) {
            return Err(format!(
                "clustered primary index {} uses the table scan path",
                index.name
            ));
        }
        let mut index_columns = Vec::with_capacity(index.columns.len());
        for index_column in &index.columns {
            if index_column.prefix_length.is_some() {
                return Err(format!("index {} contains a prefix column", index.name));
            }
            let column = self
                .TableInfo
                .columns
                .get(index_column.column_offset)
                .filter(|column| column.public && !column.hidden)
                .cloned()
                .ok_or_else(|| format!("index {} contains an invalid column", index.name))?;
            index_columns.push(column);
        }
        if index_columns[0].id != self.TimeColumn.id {
            return Err(format!(
                "TTL column {} is not the first index column",
                self.TimeColumn.name
            ));
        }
        if index.unique && index_columns.iter().skip(1).any(|column| column.nullable) {
            return Err(format!(
                "unique index {} has nullable pagination columns",
                index.name
            ));
        }
        let mut key_offsets = Vec::with_capacity(self.KeyColumns.len());
        let mut key_columns_in_index = 0;
        for key in &self.KeyColumns {
            let offset = index_columns.iter().position(|column| column.id == key.id);
            key_columns_in_index += usize::from(offset.is_some());
            key_offsets.push(offset);
        }
        if !index.unique && key_columns_in_index > 0 && key_columns_in_index < self.KeyColumns.len()
        {
            return Err(format!(
                "index {} contains only part of the table key",
                index.name
            ));
        }
        if !index.unique
            && key_columns_in_index == 0
            && self
                .KeyColumns
                .iter()
                .any(|column| column.key_kind == KeyKind::UnsignedInt)
        {
            return Err(format!(
                "index {} cannot seek by an unsigned table-key suffix",
                index.name
            ));
        }
        let mut order_columns = index_columns.clone();
        if !index.unique && key_columns_in_index == 0 {
            order_columns.extend(self.KeyColumns.clone());
        }
        if order_columns
            .iter()
            .any(|column| matches!(column.key_kind, KeyKind::Float | KeyKind::Set))
        {
            return Err(format!(
                "index {} contains an unsupported pagination type",
                index.name
            ));
        }
        let mut scan_columns = index_columns;
        let mut key_column_offsets = Vec::with_capacity(key_offsets.len());
        for (key, offset) in self.KeyColumns.iter().zip(key_offsets) {
            key_column_offsets.push(match offset {
                Some(offset) => offset,
                None => {
                    let offset = scan_columns.len();
                    scan_columns.push(key.clone());
                    offset
                }
            });
        }
        Ok(TTLIndexScanPlan {
            Index: index.clone(),
            ScanColumns: scan_columns,
            OrderColumns: order_columns,
            KeyColumnOffsets: key_column_offsets,
        })
    }

    /// 选择 Go 优先级相同的最优 TTL 索引。
    pub fn FindTTLIndex(&self) -> Option<IndexInfo> {
        self.Indices
            .iter()
            .filter_map(|index| self.BuildTTLIndexScanPlan(index).ok())
            .min_by_key(|plan| {
                let full_key = plan
                    .KeyColumnOffsets
                    .iter()
                    .all(|offset| *offset < plan.Index.columns.len());
                let priority = if plan.Index.columns.len() == 1 {
                    0
                } else if full_key {
                    1
                } else {
                    2
                };
                (priority, plan.OrderColumns.len(), plan.ScanColumns.len())
            })
            .map(|plan| plan.Index)
    }

    /// 按索引 Region 边界拆分 TTL 时间列范围。
    pub fn SplitIndexScanRanges(
        &self,
        regions: Option<&dyn RegionProvider>,
        index: &IndexInfo,
        expire_time: i64,
        split_count: usize,
    ) -> Result<Vec<ScanRange>, String> {
        self.BuildTTLIndexScanPlan(index)?;
        if split_count <= 1 {
            return Ok(vec![newFullRange()]);
        }
        let Some(regions) = regions else {
            return Ok(vec![newFullRange()]);
        };
        let prefix = index_prefix(self.ID, index.id);
        let raw = splitRawKeyRanges(regions, &prefix, &prefix_next(prefix.clone()), split_count)?;
        if raw.len() <= 1 {
            return Ok(vec![newFullRange()]);
        }
        let mut result = Vec::new();
        let mut start = Datum::Null;
        for (position, range) in raw.iter().enumerate() {
            let end = if position + 1 == raw.len() {
                Datum::Null
            } else {
                decode_index_time_boundary(&range.end, &prefix)
                    .map(|value| Datum::Time(value.min(expire_time)))
                    .unwrap_or(Datum::Null)
            };
            if matches!(start, Datum::Null)
                || matches!(end, Datum::Null)
                || datum_less(&start, &end)
            {
                result.push(newDatumRange(start.clone(), end.clone()));
                start = end;
            }
        }
        Ok(if result.is_empty() {
            vec![newFullRange()]
        } else {
            result
        })
    }
    /// 校验键前缀长度不超过 KeyColumns。
    pub fn ValidateKeyPrefix(&self, key: &[Datum]) -> Result<(), String> {
        if key.len() > self.KeyColumns.len() {
            Err(format!(
                "invalid key length: {}, expected {}",
                key.len(),
                self.KeyColumns.len()
            ))
        } else {
            Ok(())
        }
    }
    /// 返回 schema.table 或 schema.table.partition 全名。
    pub fn FullName(&self) -> String {
        if self.Partition.is_empty() {
            format!("{}.{}", self.Schema, self.TableInfo.name)
        } else {
            format!("{}.{}.{}", self.Schema, self.TableInfo.name, self.Partition)
        }
    }
    /// 按表上 TTLInfo 计算“早于此时间戳的行视为过期”。
    pub fn EvalExpireTime(&self, now_seconds: i64) -> Result<i64, String> {
        let ttl = self
            .TableInfo
            .ttl
            .as_ref()
            .ok_or_else(|| "TTL info is missing".to_owned())?;
        EvalExpireTime(now_seconds, &ttl.interval, ttl.unit)
    }
    /// 按 Region 边界把表记录前缀拆成多个 Datum 扫描区间。
    pub fn SplitScanRanges(
        &self,
        regions: Option<&dyn RegionProvider>,
        split_count: usize,
    ) -> Result<Vec<ScanRange>, String> {
        // 无法拆分或只要求一段时直接返回全表范围。
        if self.KeyColumns.is_empty() || split_count <= 1 {
            return Ok(vec![newFullRange()]);
        }
        let Some(regions) = regions else {
            return Ok(vec![newFullRange()]);
        };
        let prefix = record_prefix(self.ID);
        let raw = splitRawKeyRanges(regions, &prefix, &prefix_next(prefix.clone()), split_count)?;
        if raw.len() <= 1 {
            return Ok(vec![newFullRange()]);
        }
        // 以首个键列类型把原始 Region 结束键转成下一区间的起始 Datum。
        let key_kind = self.KeyColumns[0].key_kind;
        if key_kind == KeyKind::UnsignedInt {
            return Ok(split_unsigned_int_ranges(&raw, &prefix));
        }
        let mut output = Vec::new();
        let mut start = Datum::Null;
        for (index, range) in raw.iter().enumerate() {
            let end = if index + 1 == raw.len() {
                Datum::Null
            } else {
                match key_kind {
                    KeyKind::SignedInt => GetNextIntHandle(&range.end, &prefix)
                        .map(Datum::Int)
                        .unwrap_or(Datum::Null),
                    KeyKind::UnsignedInt => unreachable!("handled before the common path"),
                    KeyKind::Bytes => GetNextBytesHandleDatum(&range.end, &prefix),
                    KeyKind::Float | KeyKind::Set => Datum::Null,
                }
            };
            if datum_less(&start, &end)
                || matches!(start, Datum::Null)
                || matches!(end, Datum::Null)
            {
                output.push(newDatumRange(start.clone(), end.clone()));
            }
            start = end;
        }
        Ok(output)
    }
}

/// TTL index scan 的结果布局与严格分页顺序。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TTLIndexScanPlan {
    pub Index: IndexInfo,
    pub ScanColumns: Vec<Column>,
    pub OrderColumns: Vec<Column>,
    pub KeyColumnOffsets: Vec<usize>,
}
impl TTLIndexScanPlan {
    pub fn OrderKey<'a>(&self, row: &'a [Datum]) -> &'a [Datum] {
        &row[..self.OrderColumns.len()]
    }
    pub fn TableKey(&self, row: &[Datum]) -> Vec<Datum> {
        self.KeyColumnOffsets
            .iter()
            .map(|offset| row[*offset].clone())
            .collect()
    }
}

/// 公历日期 → 距 Unix epoch 的整天数（Howard Hinnant 算法）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
/// 距 Unix epoch 的整天数 → 公历年月日。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}
/// 返回指定年月的天数（含闰年二月）。
fn month_days(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
/// 从当前时刻减去 TTL 间隔，得到过期水位时间戳（秒）。
pub fn EvalExpireTime(now_seconds: i64, interval: &str, unit: TimeUnit) -> Result<i64, String> {
    if unit == TimeUnit::HourMinute {
        let value = interval.trim().trim_matches('\'');
        let (hours, minutes) = value
            .split_once(':')
            .ok_or_else(|| format!("invalid TTL HOUR_MINUTE interval {interval}"))?;
        let hours: i64 = hours
            .parse()
            .map_err(|error| format!("invalid TTL interval {interval}: {error}"))?;
        let minutes: i64 = minutes
            .parse()
            .map_err(|error| format!("invalid TTL interval {interval}: {error}"))?;
        return Ok(now_seconds - hours * 3600 - minutes * 60);
    }
    let amount: i64 = interval
        .trim()
        .parse()
        .map_err(|error| format!("invalid TTL interval {interval}: {error}"))?;
    match unit {
        TimeUnit::Microsecond => Ok(now_seconds - amount / 1_000_000),
        TimeUnit::Second => Ok(now_seconds - amount),
        TimeUnit::Minute => Ok(now_seconds - amount * 60),
        TimeUnit::HourMinute => unreachable!("handled before integer interval parsing"),
        TimeUnit::Hour => Ok(now_seconds - amount * 3600),
        TimeUnit::Day => Ok(now_seconds - amount * 86400),
        TimeUnit::Week => Ok(now_seconds - amount * 7 * 86400),
        // 月/季/年按公历回退，并钳制到目标月的合法日。
        TimeUnit::Month | TimeUnit::Quarter | TimeUnit::Year => {
            let days = now_seconds.div_euclid(86400);
            let seconds = now_seconds.rem_euclid(86400);
            let (year, month, day) = civil_from_days(days);
            let months = match unit {
                TimeUnit::Month => amount,
                TimeUnit::Quarter => amount * 3,
                TimeUnit::Year => amount * 12,
                _ => unreachable!(),
            };
            let total = year * 12 + month - 1 - months;
            let new_year = total.div_euclid(12);
            let new_month = total.rem_euclid(12) + 1;
            Ok(days_from_civil(
                new_year,
                new_month,
                day.min(month_days(new_year, new_month)),
            ) * 86400
                + seconds)
        }
    }
}

/// TiKV 原始键字节区间（半开）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyRange {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}
/// 查询覆盖 [start, end) 的 Region 列表。
pub trait RegionProvider {
    fn locate_key_range(&self, start: &[u8], end: &[u8]) -> Result<Vec<KeyRange>, String>;
}
/// 将 Region 列表均匀合并为约 split_count 段原始键范围。
pub fn splitRawKeyRanges(
    provider: &dyn RegionProvider,
    start: &[u8],
    end: &[u8],
    split_count: usize,
) -> Result<Vec<KeyRange>, String> {
    let regions = provider.locate_key_range(start, end)?;
    if regions.is_empty() {
        return Ok(Vec::new());
    }
    let groups = split_count.min(regions.len());
    let base = regions.len() / groups;
    let extra = regions.len() % groups;
    let mut output = Vec::with_capacity(groups);
    let mut cursor = 0;
    for group in 0..groups {
        let length = base + usize::from(group < extra);
        let first = &regions[cursor];
        let last = &regions[cursor + length - 1];
        output.push(KeyRange {
            start: if first.start.as_slice() < start {
                start.to_vec()
            } else {
                first.start.clone()
            },
            end: if last.end.as_slice() > end {
                end.to_vec()
            } else {
                last.end.clone()
            },
        });
        cursor += length;
    }
    Ok(output)
}
/// 构造表记录键前缀：`t{table_id}_r`（table_id 按有符号大端编码）。
fn record_prefix(table_id: i64) -> Vec<u8> {
    let mut prefix = vec![b't'];
    prefix.extend_from_slice(&((table_id as u64 ^ (1 << 63)).to_be_bytes()));
    prefix.extend_from_slice(b"_r");
    prefix
}

fn index_prefix(table_id: i64, index_id: i64) -> Vec<u8> {
    let mut prefix = record_prefix(table_id);
    prefix.extend_from_slice(b"_i");
    prefix.extend_from_slice(&index_id.to_be_bytes());
    prefix
}

fn decode_index_time_boundary(key: &[u8], prefix: &[u8]) -> Option<i64> {
    let encoded = key.strip_prefix(prefix)?;
    let bytes: [u8; 8] = encoded.get(..8)?.try_into().ok()?;
    Some(i64::from_be_bytes(bytes))
}
/// 计算字典序上严格大于 key 的最短前缀（用于半开区间上界）。
fn prefix_next(mut key: Vec<u8>) -> Vec<u8> {
    for index in (0..key.len()).rev() {
        if key[index] != 0xff {
            key[index] += 1;
            key.truncate(index + 1);
            return key;
        }
    }
    key.push(0);
    key
}
/// 同类型 Datum 比较；类型不符时保守视为可推进。
fn datum_less(left: &Datum, right: &Datum) -> bool {
    match (left, right) {
        (Datum::Int(a), Datum::Int(b)) => a < b,
        (Datum::UInt(a), Datum::UInt(b)) => a < b,
        (Datum::Bytes(a), Datum::Bytes(b)) => a < b,
        (Datum::String(a), Datum::String(b)) => a < b,
        _ => true,
    }
}

/// 将有符号 handle 编码顺序转换为无符号扫描顺序。
///
/// TiKV 的原始 Region 边界按 `i64` 从负到正排列，而 SQL 无符号值的
/// 顺序是 `0..=u64::MAX`。因此跨过 0 的原始区间必须分为高半区与低半区，
/// 与 Go `splitIntRanges` / `unsignedEdge` 保持一致。
fn split_unsigned_int_ranges(raw: &[KeyRange], prefix: &[u8]) -> Vec<ScanRange> {
    fn unsigned_edge(edge: Option<i64>) -> Datum {
        match edge {
            None => Datum::UInt(i64::MAX as u64 + 1),
            Some(0) => Datum::Null,
            Some(value) => Datum::UInt(value as u64),
        }
    }

    let mut output = Vec::with_capacity(raw.len() + 1);
    let mut start = None;
    for (index, range) in raw.iter().enumerate() {
        if index != 0 && start.is_none() {
            break;
        }
        let end = if index + 1 == raw.len() {
            None
        } else {
            GetNextIntHandle(&range.end, prefix)
        };
        if matches!((start, end), (Some(left), Some(right)) if left >= right) {
            continue;
        }

        if start.is_some_and(|value| value >= 0) || end.is_some_and(|value| value <= 0) {
            output.push(newDatumRange(unsigned_edge(start), unsigned_edge(end)));
        } else {
            output.push(newDatumRange(unsigned_edge(start), Datum::Null));
            output.push(newDatumRange(Datum::Null, unsigned_edge(end)));
        }
        start = end;
    }
    output
}
/// 从记录键解析下一整数句柄；越出前缀返回 None。
pub fn GetNextIntHandle(key: &[u8], record_prefix: &[u8]) -> Option<i64> {
    if key > record_prefix && !key.starts_with(record_prefix) {
        return None;
    }
    if key <= record_prefix {
        return Some(i64::MIN);
    }
    let suffix = &key[record_prefix.len()..];
    let mut encoded = [0u8; 8];
    encoded[..suffix.len().min(8)].copy_from_slice(&suffix[..suffix.len().min(8)]);
    let value = (u64::from_be_bytes(encoded) ^ (1 << 63)) as i64;
    if suffix.len() > 8 {
        value.checked_add(1)
    } else {
        Some(value)
    }
}
/// 聚簇索引整数句柄的下一 Datum；失败则为 Null。
pub fn GetNextIntDatumFromCommonHandle(key: &[u8], record_prefix: &[u8], unsigned: bool) -> Datum {
    let Some(value) = GetNextIntHandle(key, record_prefix) else {
        return Datum::Null;
    };
    if unsigned {
        Datum::UInt(value as u64)
    } else {
        Datum::Int(value)
    }
}
/// 字节串句柄的下一 Datum（对后缀做 prefix_next）。
pub fn GetNextBytesHandleDatum(key: &[u8], record_prefix: &[u8]) -> Datum {
    if key > record_prefix && !key.starts_with(record_prefix) {
        return Datum::Null;
    }
    if key <= record_prefix {
        return Datum::Bytes(Vec::new());
    }
    let mut value = key[record_prefix.len()..].to_vec();
    if !value.is_empty() {
        value = prefix_next(value);
    }
    Datum::Bytes(value)
}
/// 截取可见 ASCII/空白前缀作为 string Datum，供范围边界可读化。
pub fn GetASCIIPrefixDatumFromBytes(bytes: &[u8]) -> Datum {
    let end = bytes
        .iter()
        .position(|byte| {
            !((*byte >= 0x20 && *byte <= 0x7e) || matches!(*byte, b'\t' | b'\n' | b'\r'))
        })
        .unwrap_or(bytes.len());
    Datum::String(String::from_utf8_lossy(&bytes[..end]).into_owned())
}
