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

// 表级 schema 的格编码、比较、join 与 SQL 还原。
//
// 对应 Go `table.go`。把 `TableInfo` 编码为（排序规则、列 map、索引 map、
// 自增 ID、分片位数等）元组格；列缺失时按默认值规则补齐，索引缺失则删除。
// join 后根据索引回填列上的键标志，并拒绝无键的 AUTO_INCREMENT。

use crate::{
    AnyValue, Bool, Collation, Equality, EqualitySingleton, ErrMsgAtMapKey,
    ErrMsgAutoTypeWithoutKey, IncompatibleError, Int64, LatticeBox, LatticeMap, LatticeRef, Map,
    Maybe, MaybeSingletonInterface, MaybeSingletonString, Singleton, Tuple, Type, ast, format,
    latticeMap, model, mysql, typ, types,
};
use std::any::Any;
use std::collections::HashMap;
use std::fmt::{self, Debug};

// 列元组各维下标：默认值 / 生成列表达式 / STORED / 字段类型。
/// 默认值维。
const COLUMN_DEFAULT: usize = 0;
/// 生成列表达式维。
const COLUMN_GENERATED_EXPR: usize = 1;
/// 生成列是否 STORED。
const COLUMN_GENERATED_STORED: usize = 2;
/// 列字段类型（`typ`）维。
const COLUMN_FIELD_TYPE: usize = 3;

// 索引元组各维：列切片 / 非唯一 / 非主键 / 索引类型。
/// 索引列切片维。
const INDEX_COLUMNS: usize = 0;
/// `!Unique` 维（true 表示非唯一）。
const INDEX_NOT_UNIQUE: usize = 1;
/// `!Primary` 维。
const INDEX_NOT_PRIMARY: usize = 2;
/// 索引类型（BTREE/HASH 等）维。
const INDEX_TYPE: usize = 3;

// 表元组各维下标。
/// 表级排序规则维。
const TABLE_COLLATE: usize = 0;
/// 列 map 维。
const TABLE_COLUMNS: usize = 1;
/// 索引 map 维。
const TABLE_INDICES: usize = 2;
/// AUTO_INCREMENT 当前值维。
const TABLE_AUTO_INC_ID: usize = 3;
/// `SHARD_ROW_ID_BITS` 维。
const TABLE_SHARD_ROW_ID_BITS: usize = 4;
/// `AUTO_RANDOM_BITS` 维。
const TABLE_AUTO_RANDOM_BITS: usize = 5;
/// 预分裂 Region 数量维。
const TABLE_PRE_SPLIT_REGIONS: usize = 6;
/// 表压缩选项维。
const TABLE_COMPRESSION: usize = 7;

/// 从元组某一维解包并克隆出具体类型。
fn value<T: Clone + 'static>(tuple: &Tuple, index: usize) -> T {
    tuple[index]
        .Unwrap()
        .downcast_ref::<T>()
        .expect("schemacmp lattice type")
        .clone()
}

/// 将列元信息编码为列元组格。
fn encodeColumnInfoToLattice(column: &model::ColumnInfo) -> Tuple {
    Tuple(vec![
        MaybeSingletonInterface(column.DefaultValue.clone()),
        Singleton(column.GeneratedExprString.clone()),
        Singleton(column.GeneratedStored),
        Box::new(Type(&column.FieldType)),
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 索引中的单列：名称与前缀长度。
struct IndexColumn {
    name: String,
    length: isize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 索引列切片，作为 `Equality` 单点参与格运算。
struct IndexColumnSlice(Vec<IndexColumn>);

impl Equality for IndexColumnSlice {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn Equals(&self, other: &dyn Equality) -> bool {
        other.as_any().downcast_ref::<Self>() == Some(self)
    }
}

/// 将索引元信息编码为索引元组格。
fn encodeIndexInfoToLattice(index: &model::IndexInfo) -> Tuple {
    let columns = index
        .Columns
        .iter()
        .map(|column| IndexColumn {
            name: column.Name.L.clone(),
            length: column.Length,
        })
        .collect();
    Tuple(vec![
        EqualitySingleton(IndexColumnSlice(columns)),
        Box::new(Bool(!index.Unique)),
        Box::new(Bool(!index.Primary)),
        Singleton(index.Tp),
    ])
}

/// 由带 PriKeyFlag 的列合成隐式 PRIMARY KEY 索引元组。
fn encodeImplicitPrimaryKeyToLattice(column: &model::ColumnInfo) -> Tuple {
    Tuple(vec![
        EqualitySingleton(IndexColumnSlice(vec![IndexColumn {
            name: column.Name.L.clone(),
            length: types::UnspecifiedLength,
        }])),
        Box::new(Bool(false)),
        Box::new(Bool(false)),
        Singleton(ast::IndexType::Btree),
    ])
}

#[derive(Clone, Default)]
/// 列名→列元组的 map。
///
/// 缺失列：若有默认值则可比较；join 时去掉键标志并必要时补标准默认值。
struct ColumnMap(HashMap<String, Tuple>);

impl LatticeMap for ColumnMap {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn New(&self) -> Box<dyn LatticeMap> {
        Box::new(Self::default())
    }
    fn Insert(&mut self, key: String, value: LatticeBox) {
        self.0.insert(
            key,
            value
                .as_any()
                .downcast_ref::<Tuple>()
                .expect("column tuple")
                .clone(),
        );
    }
    fn Get(&self, key: &str) -> Option<LatticeRef<'_>> {
        self.0.get(key).map(|value| value as LatticeRef<'_>)
    }
    fn ForEach(
        &self,
        f: &mut dyn FnMut(&str, LatticeRef<'_>) -> Result<(), IncompatibleError>,
    ) -> Result<(), IncompatibleError> {
        for (key, value) in &self.0 {
            f(key, value)?;
        }
        Ok(())
    }
    fn CompareWithNil(&self, value: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let tuple = value
            .as_any()
            .downcast_ref::<Tuple>()
            .expect("column tuple");
        if tuple[COLUMN_FIELD_TYPE]
            .as_any()
            .downcast_ref::<typ>()
            .unwrap()
            .hasDefault()
        {
            Ok(1)
        } else {
            Err(IncompatibleError::message(
                "column with no default value cannot be missing",
            ))
        }
    }
    fn JoinWithNil(&self, value: LatticeRef<'_>) -> Result<Option<LatticeBox>, IncompatibleError> {
        let mut column = value
            .as_any()
            .downcast_ref::<Tuple>()
            .expect("column tuple")
            .clone();
        let field_type = column[COLUMN_FIELD_TYPE]
            .as_any_mut()
            .downcast_mut::<typ>()
            .unwrap();
        if field_type.setFlagForMissingColumn() && field_type.isNotNull() {
            column[COLUMN_DEFAULT] =
                Maybe(Some(Singleton(field_type.getStandardDefaultModelValue())));
        }
        Ok(Some(Box::new(column)))
    }
    fn ShouldDeleteIncompatibleJoin(&self) -> bool {
        false
    }
    fn clone_box(&self) -> Box<dyn LatticeMap> {
        Box::new(self.clone())
    }
}

#[derive(Clone, Default)]
/// 索引名→索引元组的 map。
///
/// 缺失索引视为更大一侧更“宽”；join 时丢弃仅一侧存在或不兼容的索引。
struct IndexMap(HashMap<String, Tuple>);

impl LatticeMap for IndexMap {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn New(&self) -> Box<dyn LatticeMap> {
        Box::new(Self::default())
    }
    fn Insert(&mut self, key: String, value: LatticeBox) {
        self.0.insert(
            key,
            value
                .as_any()
                .downcast_ref::<Tuple>()
                .expect("index tuple")
                .clone(),
        );
    }
    fn Get(&self, key: &str) -> Option<LatticeRef<'_>> {
        self.0.get(key).map(|value| value as LatticeRef<'_>)
    }
    fn ForEach(
        &self,
        f: &mut dyn FnMut(&str, LatticeRef<'_>) -> Result<(), IncompatibleError>,
    ) -> Result<(), IncompatibleError> {
        for (key, value) in &self.0 {
            f(key, value)?;
        }
        Ok(())
    }
    fn CompareWithNil(&self, _value: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        Ok(-1)
    }
    fn JoinWithNil(&self, _value: LatticeRef<'_>) -> Result<Option<LatticeBox>, IncompatibleError> {
        Ok(None)
    }
    fn ShouldDeleteIncompatibleJoin(&self) -> bool {
        true
    }
    fn clone_box(&self) -> Box<dyn LatticeMap> {
        Box::new(self.clone())
    }
}

/// 编码表 schema 时可显式覆盖的表选项字段。
#[derive(Clone, Debug)]
pub struct TableOptions {
    pub Collate: String,
    pub AutoIncID: i64,
    pub ShardRowIDBits: u64,
    pub AutoRandomBits: u64,
    pub PreSplitRegions: u64,
    pub Compression: String,
}

impl Default for TableOptions {
    fn default() -> Self {
        Self {
            Collate: "utf8mb4_bin".to_owned(),
            AutoIncID: 0,
            ShardRowIDBits: 0,
            AutoRandomBits: 0,
            PreSplitRegions: 0,
            Compression: String::new(),
        }
    }
}

/// 将表定义与选项编码为表元组格。
fn encodeTableInfoToLattice(info: &model::TableInfo, options: &TableOptions) -> Tuple {
    // 若无显式 PRIMARY，则把 PriKeyFlag 列编码为名为 primary 的隐式主键索引。
    let explicit_primary = info.Indices.iter().any(|index| index.Primary);
    let mut indices = IndexMap::default();
    for index in &info.Indices {
        indices
            .0
            .insert(index.Name.L.clone(), encodeIndexInfoToLattice(index));
    }
    let mut columns = ColumnMap::default();
    for column in &info.Columns {
        columns
            .0
            .insert(column.Name.L.clone(), encodeColumnInfoToLattice(column));
        if !explicit_primary && column.GetFlag() & mysql::PriKeyFlag != 0 {
            indices.0.insert(
                "primary".to_owned(),
                encodeImplicitPrimaryKeyToLattice(column),
            );
        }
    }
    Tuple(vec![
        Box::new(Collation(&options.Collate)),
        Map(Box::new(columns)),
        Map(Box::new(indices)),
        Box::new(Int64(options.AutoIncID)),
        Singleton(options.ShardRowIDBits),
        Singleton(options.AutoRandomBits),
        Singleton(options.PreSplitRegions),
        MaybeSingletonString(options.Compression.clone()),
    ])
}

/// 把默认值枚举格式化为 SQL 字面量片段。
fn defaultValueString(value: &model::DefaultValue) -> String {
    match value {
        model::DefaultValue::Bool(value) => value.to_string(),
        model::DefaultValue::Int(value) => value.to_string(),
        model::DefaultValue::Uint(value) => value.to_string(),
        model::DefaultValue::Float(value) => value.to_string(),
        model::DefaultValue::String(value) => String::from_utf8_lossy(value).into_owned(),
    }
}

/// 将列元组还原为 CREATE TABLE 中的列定义片段。
fn restoreColumn(ctx: &mut format::RestoreCtx<'_>, column: &[AnyValue], name: &str) {
    let field_type = column[COLUMN_FIELD_TYPE]
        .downcast_ref::<types::FieldType>()
        .unwrap();
    let _ = ctx.WriteName(name);
    let _ = ctx.WritePlain(" ");
    let _ = field_type.Restore(ctx);
    let generated = column[COLUMN_GENERATED_EXPR]
        .downcast_ref::<String>()
        .unwrap();
    if !generated.is_empty() {
        let _ = ctx.WriteKeyWord(" GENERATED ALWAYS AS ");
        let _ = ctx.WritePlain(&format!("({generated})"));
    }
    if *column[COLUMN_GENERATED_STORED]
        .downcast_ref::<bool>()
        .unwrap()
    {
        let _ = ctx.WriteKeyWord(" STORED");
    }
    if mysql::HasNotNullFlag(field_type.GetFlag()) {
        let _ = ctx.WriteKeyWord(" NOT NULL");
    }
    if let Some(default) = column[COLUMN_DEFAULT].downcast_ref::<model::DefaultValue>() {
        let _ = ctx.WriteKeyWord(" DEFAULT ");
        let _ = ctx.WritePlain(&defaultValueString(default));
    }
    if mysql::HasAutoIncrementFlag(field_type.GetFlag()) {
        let _ = ctx.WriteKeyWord(" AUTO_INCREMENT");
    }
}

/// 索引类型枚举到 SQL 关键字。
fn indexTypeName(index_type: ast::IndexType) -> String {
    index_type.to_string()
}

/// 将索引元组还原为 PRIMARY/UNIQUE/KEY 定义片段。
fn restoreIndex(ctx: &mut format::RestoreCtx<'_>, index: &[AnyValue], name: &str) {
    let primary = !*index[INDEX_NOT_PRIMARY].downcast_ref::<bool>().unwrap();
    let unique = !*index[INDEX_NOT_UNIQUE].downcast_ref::<bool>().unwrap();
    if primary {
        let _ = ctx.WriteKeyWord("PRIMARY KEY");
    } else if unique {
        let _ = ctx.WriteKeyWord("UNIQUE KEY ");
        let _ = ctx.WriteName(name);
    } else {
        let _ = ctx.WriteKeyWord("KEY ");
        let _ = ctx.WriteName(name);
    }
    let index_type = *index[INDEX_TYPE].downcast_ref::<ast::IndexType>().unwrap();
    if index_type != ast::IndexType::Btree {
        let _ = ctx.WriteKeyWord(" USING ");
        let _ = ctx.WriteKeyWord(&indexTypeName(index_type));
    }
    let _ = ctx.WritePlain(" (");
    for (position, column) in index[INDEX_COLUMNS]
        .downcast_ref::<IndexColumnSlice>()
        .unwrap()
        .0
        .iter()
        .enumerate()
    {
        if position > 0 {
            let _ = ctx.WritePlain(", ");
        }
        let _ = ctx.WriteName(&column.name);
        if column.length != types::UnspecifiedLength {
            let _ = ctx.WritePlain(&format!("({})", column.length));
        }
    }
    let _ = ctx.WritePlain(")");
}

#[derive(Clone)]
/// 已编码为格元素的表 schema，支持 Compare/Join/Restore。
pub struct Table {
    value: LatticeBox,
}

/// 用默认 `TableOptions` 编码表信息。
pub fn Encode(info: &model::TableInfo) -> Table {
    EncodeWithOptions(
        info,
        &TableOptions {
            Collate: info.Collate.clone(),
            AutoIncID: info.AutoIncID,
            ShardRowIDBits: info.ShardRowIDBits,
            AutoRandomBits: info.AutoRandomBits,
            PreSplitRegions: info.PreSplitRegions,
            Compression: info.Compression.clone(),
        },
    )
}

/// 带显式表选项编码表信息。
pub fn EncodeWithOptions(info: &model::TableInfo, options: &TableOptions) -> Table {
    Table {
        value: Box::new(encodeTableInfoToLattice(info, options)),
    }
}

/// 从编码后的表中解出各列 `FieldType`。
pub fn DecodeColumnFieldTypes(table: &Table) -> HashMap<String, types::FieldType> {
    let values = table.value.Unwrap();
    let table = values.downcast_ref::<Vec<AnyValue>>().unwrap();
    let columns = table[TABLE_COLUMNS]
        .downcast_ref::<HashMap<String, AnyValue>>()
        .unwrap();
    columns
        .iter()
        .map(|(name, value)| {
            let column = value.downcast_ref::<Vec<AnyValue>>().unwrap();
            (
                name.clone(),
                column[COLUMN_FIELD_TYPE]
                    .downcast_ref::<types::FieldType>()
                    .unwrap()
                    .clone(),
            )
        })
        .collect()
}

impl Table {
    /// 还原为 `CREATE TABLE` SQL（列/索引按名字典序）。
    pub fn Restore(&self, ctx: &mut format::RestoreCtx<'_>, table_name: &str) {
        let values = self.value.Unwrap();
        let table = values.downcast_ref::<Vec<AnyValue>>().unwrap();
        let _ = ctx.WriteKeyWord("CREATE TABLE ");
        let _ = ctx.WriteName(table_name);
        let _ = ctx.WritePlain("(");
        let columns = table[TABLE_COLUMNS]
            .downcast_ref::<HashMap<String, AnyValue>>()
            .unwrap();
        let mut columns = columns.iter().collect::<Vec<_>>();
        columns.sort_by_key(|(name, _)| *name);
        for (position, (name, value)) in columns.into_iter().enumerate() {
            if position > 0 {
                let _ = ctx.WritePlain(", ");
            }
            restoreColumn(ctx, value.downcast_ref::<Vec<AnyValue>>().unwrap(), name);
        }
        let indices = table[TABLE_INDICES]
            .downcast_ref::<HashMap<String, AnyValue>>()
            .unwrap();
        let mut indices = indices.iter().collect::<Vec<_>>();
        indices.sort_by_key(|(name, _)| *name);
        for (name, value) in indices {
            let _ = ctx.WritePlain(", ");
            restoreIndex(ctx, value.downcast_ref::<Vec<AnyValue>>().unwrap(), name);
        }
        let _ = ctx.WritePlain(")");
        let _ = ctx.WriteKeyWord(" COLLATE ");
        let _ = ctx.WritePlain(table[TABLE_COLLATE].downcast_ref::<String>().unwrap());
        let shard = *table[TABLE_SHARD_ROW_ID_BITS]
            .downcast_ref::<u64>()
            .unwrap();
        if shard > 0 {
            let _ = ctx.WriteKeyWord(" SHARD_ROW_ID_BITS ");
            let _ = ctx.WritePlain(&shard.to_string());
        }
        let auto_random = *table[TABLE_AUTO_RANDOM_BITS].downcast_ref::<u64>().unwrap();
        if auto_random > 0 {
            let _ = ctx.WritePlain("/*");
            let _ = ctx.WriteKeyWord(" AUTO_RANDOM_BITS ");
            let _ = ctx.WritePlain(&format!("{auto_random} */"));
        }
        if let Some(compression) = table[TABLE_COMPRESSION].downcast_ref::<String>() {
            if !compression.is_empty() {
                let _ = ctx.WriteKeyWord(" COMPRESSION ");
                let _ = ctx.WriteString(compression);
            }
        }
    }

    /// 比较两张表的 schema 偏序。
    pub fn Compare(&self, other: Table) -> Result<i32, IncompatibleError> {
        self.value.Compare(other.value.as_ref())
    }

    /// 求两张表 schema 的上确界，并回填键标志、校验 AUTO_INCREMENT。
    pub fn Join(&self, other: Table) -> Result<Table, IncompatibleError> {
        let mut joined = self.value.Join(other.value.as_ref())?;
        // 根据 join 后的索引推导每列应有的 Pri/Unique/Multiple 键标志。
        let mut key_flags = HashMap::<String, usize>::new();
        {
            let table = joined.as_any().downcast_ref::<Tuple>().unwrap();
            let indices = table[TABLE_INDICES]
                .as_any()
                .downcast_ref::<latticeMap>()
                .unwrap();
            let indices = indices.inner.as_any().downcast_ref::<IndexMap>().unwrap();
            for index in indices.0.values() {
                let columns = index[INDEX_COLUMNS].Unwrap();
                let columns = columns.downcast_ref::<IndexColumnSlice>().unwrap();
                if columns.0.is_empty() {
                    continue;
                }
                if !value::<bool>(index, INDEX_NOT_PRIMARY) {
                    for column in &columns.0 {
                        *key_flags.entry(column.name.clone()).or_default() |= mysql::PriKeyFlag;
                    }
                } else if !value::<bool>(index, INDEX_NOT_UNIQUE) && columns.0.len() == 1 {
                    *key_flags.entry(columns.0[0].name.clone()).or_default() |=
                        mysql::UniqueKeyFlag;
                } else {
                    *key_flags.entry(columns.0[0].name.clone()).or_default() |=
                        mysql::MultipleKeyFlag;
                }
            }
        }
        {
            let table = joined.as_any_mut().downcast_mut::<Tuple>().unwrap();
            let columns = table[TABLE_COLUMNS]
                .as_any_mut()
                .downcast_mut::<latticeMap>()
                .unwrap();
            let columns = columns
                .inner
                .as_any_mut()
                .downcast_mut::<ColumnMap>()
                .unwrap();
            for (name, column) in &mut columns.0 {
                let field_type = column[COLUMN_FIELD_TYPE]
                    .as_any_mut()
                    .downcast_mut::<typ>()
                    .unwrap();
                let flags = key_flags.get(name).copied().unwrap_or(0);
                if flags == 0 && field_type.inAutoIncrement() {
                    return Err(IncompatibleError {
                        Msg: ErrMsgAtMapKey,
                        Args: vec![
                            AnyValue::new(name.clone()),
                            AnyValue::new(IncompatibleError::message(ErrMsgAutoTypeWithoutKey)),
                        ],
                    });
                }
                field_type.setAntiKeyFlags(flags);
            }
        }
        Ok(Table { value: joined })
    }

    /// 以默认表名 `tbl` 还原 SQL 字符串。
    pub fn String(&self) -> String {
        let mut bytes = Vec::new();
        let mut ctx = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut bytes);
        self.Restore(&mut ctx, "tbl");
        String::from_utf8(bytes).expect("restore emits UTF-8")
    }
}

impl Debug for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.String())
    }
}
