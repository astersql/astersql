// Copyright 2026 AsterSQL.

// 分区表运行时模型：分区键路由、访问路径、DML、连接与聚合辅助。
//
// 模拟 TiDB 分区表（partition table）的核心语义：按 RANGE / LIST / HASH
// 将行映射到物理分区，并提供点查、批量点查、全局索引、分区扫描、
// 行锁以及相关子查询 Apply 等查询路径的内存实现，供单元测试复用。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

/// 分区键与行字段使用的轻量值类型。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Value {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Int(i64),
    /// 无符号整数。
    UInt(u64),
    /// 文本。
    Text(String),
}

impl Value {
    /// 将数值型值转为分区计算用的 i128；NULL / 文本返回 None。
    fn partition_number(&self) -> Option<i128> {
        match self {
            Self::Int(value) => Some(*value as i128),
            Self::UInt(value) => Some(*value as i128),
            Self::Null | Self::Text(_) => None,
        }
    }
}

/// 一行数据：按列下标排列的 `Value` 向量。
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Row(pub Vec<Value>);

impl Row {
    /// 按列下标取字段，越界返回 `ColumnOutOfBounds`。
    pub fn value(&self, column: usize) -> Result<&Value, PartitionError> {
        self.0
            .get(column)
            .ok_or(PartitionError::ColumnOutOfBounds(column))
    }
}

/// RANGE 分区定义：名称 + 上界（`None` 表示 MAXVALUE）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RangePartition {
    /// 分区名。
    pub name: String,
    /// `None` is MAXVALUE.
    /// `None` 表示 MAXVALUE（无上界）。
    pub less_than: Option<i128>,
}

/// 分区策略：RANGE / LIST / HASH，均绑定一个分区键列下标。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Partitioning {
    /// VALUES LESS THAN 上界严格递增的 RANGE 分区。
    Range {
        column: usize,
        partitions: Vec<RangePartition>,
    },
    /// 各分区取值集合互不相交的 LIST 分区。
    List {
        column: usize,
        partitions: Vec<(String, BTreeSet<Value>)>,
    },
    /// 对分区键取绝对值再模分区数的 HASH 分区。
    Hash { column: usize, partitions: usize },
}

impl Partitioning {
    /// 构造 RANGE 分区；校验非空、上界严格递增、MAXVALUE 只能在末尾。
    pub fn range(
        column: usize,
        partitions: impl IntoIterator<Item = (impl Into<String>, Option<i128>)>,
    ) -> Result<Self, PartitionError> {
        let partitions: Vec<_> = partitions
            .into_iter()
            .map(|(name, less_than)| RangePartition {
                name: name.into(),
                less_than,
            })
            .collect();
        if partitions.is_empty() {
            return Err(PartitionError::InvalidDefinition(
                "range partition list is empty".to_owned(),
            ));
        }
        let mut last = None;
        for (index, partition) in partitions.iter().enumerate() {
            match partition.less_than {
                Some(bound) if last.is_some_and(|previous| bound <= previous) => {
                    return Err(PartitionError::InvalidDefinition(
                        "range bounds must be strictly increasing".to_owned(),
                    ));
                }
                Some(bound) => last = Some(bound),
                None if index + 1 != partitions.len() => {
                    return Err(PartitionError::InvalidDefinition(
                        "MAXVALUE must be the final range partition".to_owned(),
                    ));
                }
                None => {}
            }
        }
        Ok(Self::Range { column, partitions })
    }

    /// 构造 LIST 分区；校验非空且各分区取值互不相交。
    pub fn list(
        column: usize,
        partitions: impl IntoIterator<Item = (impl Into<String>, impl IntoIterator<Item = Value>)>,
    ) -> Result<Self, PartitionError> {
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for (name, values) in partitions {
            let mut partition_values = BTreeSet::new();
            let mut duplicate = false;
            for value in values {
                if !partition_values.insert(value.clone()) || !seen.insert(value) {
                    duplicate = true;
                }
            }
            if partition_values.is_empty() || duplicate {
                return Err(PartitionError::InvalidDefinition(
                    "list partition values must be non-empty and disjoint".to_owned(),
                ));
            }
            result.push((name.into(), partition_values));
        }
        if result.is_empty() {
            return Err(PartitionError::InvalidDefinition(
                "list partition list is empty".to_owned(),
            ));
        }
        Ok(Self::List {
            column,
            partitions: result,
        })
    }

    /// 构造 HASH 分区；分区数必须为正。
    pub fn hash(column: usize, partitions: usize) -> Result<Self, PartitionError> {
        if partitions == 0 {
            return Err(PartitionError::InvalidDefinition(
                "hash partition count must be positive".to_owned(),
            ));
        }
        Ok(Self::Hash { column, partitions })
    }

    /// 计算行应落入的分区下标。
    ///
    /// RANGE：NULL → 0；数值取第一个 `value < less_than`（或 MAXVALUE）的分区。
    /// LIST：精确匹配取值集合；HASH：`abs(number) % partitions`（NULL 当作 0）。
    pub fn partition_for(&self, row: &Row) -> Result<usize, PartitionError> {
        match self {
            Self::Range { column, partitions } => {
                let value = row.value(*column)?;
                // TiDB RANGE：NULL 固定落到第一个分区。
                if matches!(value, Value::Null) {
                    return Ok(0);
                }
                let value = value.partition_number().ok_or_else(|| {
                    PartitionError::TypeMismatch("range partition key must be numeric".to_owned())
                })?;
                partitions
                    .iter()
                    .position(|partition| partition.less_than.is_none_or(|bound| value < bound))
                    .ok_or(PartitionError::NoPartition)
            }
            Self::List { column, partitions } => {
                let value = row.value(*column)?;
                partitions
                    .iter()
                    .position(|(_, values)| values.contains(value))
                    .ok_or(PartitionError::NoPartition)
            }
            Self::Hash { column, partitions } => {
                let value = row.value(*column)?;
                let number = match value {
                    Value::Null => 0,
                    Value::Int(_) | Value::UInt(_) => value
                        .partition_number()
                        .expect("integer partition key has a numeric value"),
                    Value::Text(_) => {
                        return Err(PartitionError::TypeMismatch(
                            "hash partition key must be numeric".to_owned(),
                        ));
                    }
                };
                Ok(number.unsigned_abs() as usize % partitions)
            }
        }
    }

    /// 按分区下标返回分区名；HASH 使用 `p{index}` 命名。
    pub fn partition_name(&self, index: usize) -> Option<String> {
        match self {
            Self::Range { partitions, .. } => partitions.get(index).map(|p| p.name.clone()),
            Self::List { partitions, .. } => partitions.get(index).map(|p| p.0.clone()),
            Self::Hash { partitions, .. } if index < *partitions => Some(format!("p{index}")),
            Self::Hash { .. } => None,
        }
    }

    /// 分区个数。
    fn len(&self) -> usize {
        match self {
            Self::Range { partitions, .. } => partitions.len(),
            Self::List { partitions, .. } => partitions.len(),
            Self::Hash { partitions, .. } => *partitions,
        }
    }
}

/// 查询访问路径标签，对应执行器选择的物理算子类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPath {
    /// 空结果（键不存在）。
    TableDual,
    /// 主键点查。
    PointGet,
    /// 批量主键点查。
    BatchPointGet,
    /// 按分区扫描。
    PartitionScan,
    /// 经全局唯一索引定位后再回表。
    GlobalIndex,
    /// 多路索引结果合并。
    IndexMerge,
}

/// 连接类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JoinKind {
    /// 内连接。
    Inner,
    /// 左外连接。
    LeftOuter,
    /// 半连接（存在即保留左行）。
    Semi,
    /// 反半连接（不存在才保留左行）。
    AntiSemi,
}

/// 一次查询的路径标签、触及的分区名与结果行。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryResult {
    /// 选用的访问路径。
    pub path: AccessPath,
    /// 结果涉及的分区名列表。
    pub partitions: Vec<String>,
    /// 结果行。
    pub rows: Vec<Row>,
}

/// 分区定义、路由或 DML 过程中的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PartitionError {
    /// 分区定义非法。
    InvalidDefinition(String),
    /// 列下标越界。
    ColumnOutOfBounds(usize),
    /// 分区键类型不匹配。
    TypeMismatch(String),
    /// 找不到匹配分区。
    NoPartition,
    /// 主键或唯一键冲突。
    DuplicateKey(Value),
    /// 请求了不存在的分区名。
    UnknownPartition(String),
    /// 行锁被其他事务持有。
    LockConflict { key: Value, owner: u64 },
}

impl fmt::Display for PartitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDefinition(message) | Self::TypeMismatch(message) => {
                formatter.write_str(message)
            }
            Self::ColumnOutOfBounds(column) => {
                write!(formatter, "column {column} is out of bounds")
            }
            Self::NoPartition => formatter.write_str("table has no partition for value"),
            Self::DuplicateKey(value) => write!(formatter, "duplicate key {value:?}"),
            Self::UnknownPartition(name) => write!(formatter, "unknown partition {name}"),
            Self::LockConflict { key, owner } => {
                write!(formatter, "key {key:?} is locked by transaction {owner}")
            }
        }
    }
}

impl std::error::Error for PartitionError {}

/// 内部存储行：原始行 + 已计算的分区下标。
#[derive(Clone, Debug)]
struct StoredRow {
    row: Row,
    partition: usize,
}

/// 内存分区表：主键索引、唯一索引、行锁与分区信息开关。
#[derive(Clone, Debug)]
pub struct PartitionTable {
    /// 分区策略。
    partitioning: Partitioning,
    /// 是否在结果中报告分区名。
    partition_info_enabled: bool,
    /// 主键列下标。
    primary_key: usize,
    /// 唯一列集合（模拟全局唯一索引）。
    unique_columns: BTreeSet<usize>,
    /// 主键 → 存储行。
    rows: BTreeMap<Value, StoredRow>,
    /// 唯一列 →（唯一值 → 主键）。
    unique_indexes: HashMap<usize, BTreeMap<Value, Value>>,
    /// 主键 → 持锁事务 ID。
    locks: BTreeMap<Value, u64>,
}

impl PartitionTable {
    /// 用给定分区策略、主键列与唯一列集合构造空表。
    pub fn new(
        partitioning: Partitioning,
        primary_key: usize,
        unique_columns: impl IntoIterator<Item = usize>,
    ) -> Self {
        let unique_columns: BTreeSet<_> = unique_columns.into_iter().collect();
        let unique_indexes = unique_columns
            .iter()
            .map(|column| (*column, BTreeMap::new()))
            .collect();
        Self {
            partitioning,
            partition_info_enabled: true,
            primary_key,
            unique_columns,
            rows: BTreeMap::new(),
            unique_indexes,
            locks: BTreeMap::new(),
        }
    }

    /// 插入一行：校验主键 / 唯一键冲突后按分区键落盘。
    pub fn insert(&mut self, row: Row) -> Result<(), PartitionError> {
        let key = row.value(self.primary_key)?.clone();
        if self.rows.contains_key(&key) {
            return Err(PartitionError::DuplicateKey(key));
        }
        let partition = self.partitioning.partition_for(&row)?;
        // 先检查所有唯一索引，再统一写入，避免半写入。
        for column in &self.unique_columns {
            let value = row.value(*column)?;
            if !matches!(value, Value::Null)
                && self
                    .unique_indexes
                    .get(column)
                    .is_some_and(|index| index.contains_key(value))
            {
                return Err(PartitionError::DuplicateKey(value.clone()));
            }
        }
        for column in &self.unique_columns {
            let value = row.value(*column)?.clone();
            if !matches!(value, Value::Null) {
                self.unique_indexes
                    .get_mut(column)
                    .expect("unique index initialized")
                    .insert(value, key.clone());
            }
        }
        self.rows.insert(key, StoredRow { row, partition });
        Ok(())
    }

    /// 主键点查；未命中返回 TableDual。
    pub fn point_get(&self, key: &Value) -> QueryResult {
        match self.rows.get(key) {
            Some(stored) => QueryResult {
                path: AccessPath::PointGet,
                partitions: self
                    .reported_partition_name(stored.partition)
                    .into_iter()
                    .collect(),
                rows: vec![stored.row.clone()],
            },
            None => QueryResult {
                path: AccessPath::TableDual,
                partitions: Vec::new(),
                rows: Vec::new(),
            },
        }
    }

    /// 批量主键点查，汇总触及的分区名。
    pub fn batch_point_get(&self, keys: &[Value]) -> QueryResult {
        let mut partitions = BTreeSet::new();
        let rows = keys
            .iter()
            .filter_map(|key| self.rows.get(key))
            .map(|stored| {
                if let Some(name) = self.reported_partition_name(stored.partition) {
                    partitions.insert(name);
                }
                stored.row.clone()
            })
            .collect();
        QueryResult {
            path: AccessPath::BatchPointGet,
            partitions: partitions.into_iter().collect(),
            rows,
        }
    }

    /// 经全局唯一索引查找主键再回表；未命中为 TableDual。
    pub fn global_index_get(&self, column: usize, value: &Value) -> QueryResult {
        let Some(key) = self
            .unique_indexes
            .get(&column)
            .and_then(|index| index.get(value))
        else {
            return QueryResult {
                path: AccessPath::TableDual,
                partitions: Vec::new(),
                rows: Vec::new(),
            };
        };
        let mut result = self.point_get(key);
        result.path = AccessPath::GlobalIndex;
        result
    }

    /// 按分区名列表扫描；空列表表示全部分区。
    ///
    /// 关闭分区信息时忽略名称校验并返回全表行。
    pub fn scan_partitions(&self, names: &[&str]) -> Result<QueryResult, PartitionError> {
        if !self.partition_info_enabled {
            return Ok(QueryResult {
                path: AccessPath::PartitionScan,
                partitions: Vec::new(),
                rows: self
                    .rows
                    .values()
                    .map(|stored| stored.row.clone())
                    .collect(),
            });
        }
        let requested: BTreeSet<_> = names.iter().copied().collect();
        for name in &requested {
            if !(0..self.partitioning.len()).any(|index| {
                self.partitioning
                    .partition_name(index)
                    .as_deref()
                    .is_some_and(|candidate| candidate == *name)
            }) {
                return Err(PartitionError::UnknownPartition((*name).to_owned()));
            }
        }
        let rows = self
            .rows
            .values()
            .filter(|stored| {
                requested.is_empty()
                    || self
                        .partitioning
                        .partition_name(stored.partition)
                        .as_deref()
                        .is_some_and(|name| requested.contains(name))
            })
            .map(|stored| stored.row.clone())
            .collect();
        Ok(QueryResult {
            path: AccessPath::PartitionScan,
            partitions: (0..self.partitioning.len())
                .filter_map(|index| self.partitioning.partition_name(index))
                .filter(|name| requested.is_empty() || requested.contains(name.as_str()))
                .collect(),
            rows,
        })
    }

    /// 先分区扫描再按列排序并应用 OFFSET/LIMIT。
    pub fn ordered_limit(
        &self,
        names: &[&str],
        column: usize,
        descending: bool,
        offset: usize,
        limit: usize,
    ) -> Result<QueryResult, PartitionError> {
        let mut result = self.scan_partitions(names)?;
        for row in &result.rows {
            row.value(column)?;
        }
        result.rows.sort_by(|left, right| {
            let order = left.0[column].cmp(&right.0[column]);
            if descending { order.reverse() } else { order }
        });
        result.rows = result.rows.into_iter().skip(offset).take(limit).collect();
        Ok(result)
    }

    /// 全表过滤；结果路径标为 PartitionScan。
    pub fn filter(
        &self,
        mut predicate: impl FnMut(&Row) -> bool,
    ) -> Result<QueryResult, PartitionError> {
        let mut partitions = BTreeSet::new();
        let rows = self
            .rows
            .values()
            .filter(|stored| predicate(&stored.row))
            .map(|stored| {
                if let Some(name) = self.reported_partition_name(stored.partition) {
                    partitions.insert(name);
                }
                stored.row.clone()
            })
            .collect();
        Ok(QueryResult {
            path: AccessPath::PartitionScan,
            partitions: partitions.into_iter().collect(),
            rows,
        })
    }

    /// 多路主键 / 唯一索引查找结果去重合并（IndexMerge）。
    pub fn index_merge(&self, lookups: &[(usize, Value)]) -> QueryResult {
        let mut keys = BTreeSet::new();
        for (column, value) in lookups {
            if *column == self.primary_key && self.rows.contains_key(value) {
                keys.insert(value.clone());
            }
            if let Some(key) = self
                .unique_indexes
                .get(column)
                .and_then(|index| index.get(value))
            {
                keys.insert(key.clone());
            }
        }
        let mut result = self.batch_point_get(&keys.into_iter().collect::<Vec<_>>());
        result.path = AccessPath::IndexMerge;
        result
    }

    /// 删除指定列上的唯一索引；两者都成功删除才返回 true。
    pub fn drop_unique_index(&mut self, column: usize) -> bool {
        self.unique_columns.remove(&column) & self.unique_indexes.remove(&column).is_some()
    }

    /// 开关：结果中是否报告分区名。
    pub fn set_partition_info_enabled(&mut self, enabled: bool) {
        self.partition_info_enabled = enabled;
    }

    /// 在分区信息开启时返回分区名。
    fn reported_partition_name(&self, partition: usize) -> Option<String> {
        self.partition_info_enabled
            .then(|| self.partitioning.partition_name(partition))
            .flatten()
    }

    /// 更新行：先删后插，插入失败则恢复旧行。
    pub fn update(&mut self, key: &Value, row: Row) -> Result<bool, PartitionError> {
        if !self.rows.contains_key(key) {
            return Ok(false);
        }
        let old_lock = self.locks.get(key).copied();
        let old = self.delete(key).expect("existing row can be deleted");
        if let Err(error) = self.insert(row) {
            self.insert(old.expect("existing row returned"))
                .expect("old row can be restored");
            if let Some(owner) = old_lock {
                self.locks.insert(key.clone(), owner);
            }
            return Err(error);
        }
        Ok(true)
    }

    /// 删除主键对应行，并清理唯一索引与行锁。
    pub fn delete(&mut self, key: &Value) -> Result<Option<Row>, PartitionError> {
        let Some(stored) = self.rows.remove(key) else {
            return Ok(None);
        };
        for column in &self.unique_columns {
            if let Ok(value) = stored.row.value(*column) {
                self.unique_indexes
                    .get_mut(column)
                    .expect("unique index initialized")
                    .remove(value);
            }
        }
        self.locks.remove(key);
        Ok(Some(stored.row))
    }

    /// 为事务获取行锁；被其他事务占用时返回 `LockConflict`。
    pub fn lock(&mut self, transaction: u64, key: &Value) -> Result<bool, PartitionError> {
        if !self.rows.contains_key(key) {
            return Ok(false);
        }
        match self.locks.get(key) {
            Some(owner) if *owner != transaction => Err(PartitionError::LockConflict {
                key: key.clone(),
                owner: *owner,
            }),
            _ => {
                self.locks.insert(key.clone(), transaction);
                Ok(true)
            }
        }
    }

    /// 释放某事务持有的全部行锁。
    pub fn unlock_transaction(&mut self, transaction: u64) {
        self.locks.retain(|_, owner| *owner != transaction);
    }
}

/// 按等值键做嵌套循环连接；NULL 键不匹配。
pub fn join_rows(
    left: &[Row],
    right: &[Row],
    left_key: usize,
    right_key: usize,
    kind: JoinKind,
) -> Result<Vec<Row>, PartitionError> {
    for row in left {
        row.value(left_key)?;
    }
    for row in right {
        row.value(right_key)?;
    }
    let mut result = Vec::new();
    for left_row in left {
        let matches: Vec<_> = right
            .iter()
            .filter(|right_row| {
                let left_value = &left_row.0[left_key];
                let right_value = &right_row.0[right_key];
                !matches!(left_value, Value::Null)
                    && !matches!(right_value, Value::Null)
                    && left_value == right_value
            })
            .collect();
        match kind {
            JoinKind::Inner | JoinKind::LeftOuter => {
                if matches.is_empty() && kind == JoinKind::LeftOuter {
                    // 左外连接无匹配时右侧重填 NULL。
                    let mut values = left_row.0.clone();
                    let right_width = right.first().map_or(1, |row| row.0.len());
                    values.extend(std::iter::repeat_n(Value::Null, right_width));
                    result.push(Row(values));
                } else {
                    for right_row in matches {
                        let mut values = left_row.0.clone();
                        values.extend(right_row.0.clone());
                        result.push(Row(values));
                    }
                }
            }
            JoinKind::Semi if !matches.is_empty() => result.push(left_row.clone()),
            JoinKind::AntiSemi if matches.is_empty() => result.push(left_row.clone()),
            JoinKind::Semi | JoinKind::AntiSemi => {}
        }
    }
    Ok(result)
}

/// 多路行集合并；`distinct` 为真时按行去重。
pub fn union_rows(inputs: &[&[Row]], distinct: bool) -> Vec<Row> {
    if distinct {
        inputs
            .iter()
            .flat_map(|rows| rows.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    } else {
        inputs
            .iter()
            .flat_map(|rows| rows.iter().cloned())
            .collect()
    }
}

/// 按分组列聚合：每组返回 (COUNT, SUM)。
pub fn grouped_count_sum(
    rows: &[Row],
    group_column: usize,
    value_column: usize,
) -> Result<BTreeMap<Value, (usize, i128)>, PartitionError> {
    let mut groups = BTreeMap::new();
    for row in rows {
        let group = row.value(group_column)?.clone();
        let value = row.value(value_column)?.partition_number().ok_or_else(|| {
            PartitionError::TypeMismatch("aggregate value must be numeric".to_owned())
        })?;
        let entry = groups.entry(group).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += value;
    }
    Ok(groups)
}

/// 在 `[lower, upper)` 区间内均匀生成 Region 分裂键（不含两端）。
///
/// Region 是 TiKV 的数据分片单位；分裂键决定各 Region 的边界。
pub fn split_region_keys(
    lower: i64,
    upper: i64,
    region_count: usize,
) -> Result<Vec<i64>, PartitionError> {
    if region_count == 0 || upper <= lower {
        return Err(PartitionError::InvalidDefinition(
            "split range and region count must be positive".to_owned(),
        ));
    }
    let width = i128::from(upper) - i128::from(lower);
    Ok((1..region_count)
        .map(|part| (i128::from(lower) + width * part as i128 / region_count as i128) as i64)
        .collect())
}

/// 相关 Apply：对外层每行调用内层闭包，可选限制内层行数后拼接输出。
pub fn correlated_apply(
    outer: &[Row],
    mut inner: impl FnMut(&Row) -> Vec<Row>,
    limit: Option<usize>,
) -> Vec<Row> {
    let mut result = Vec::new();
    for row in outer {
        for inner_row in inner(row).into_iter().take(limit.unwrap_or(usize::MAX)) {
            let mut values = row.0.clone();
            values.extend(inner_row.0);
            result.push(Row(values));
        }
    }
    result
}
