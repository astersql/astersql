// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 索引（index）元数据模型。
//
// 覆盖普通索引、列存索引（倒排/向量/全文）、全局索引版本开关、
// 前缀覆盖判定，以及外键场景下 partial index 条件约束。

use super::{
    BackfillState, SchemaState, StatePublic, TableInfo, ast, base, kerneltype, mysql, parser,
    removingObjPrefix, types,
};
use std::any::Any;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// 向量索引距离度量名称（如 L2、COSINE）。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DistanceMetric(pub Cow<'static, str>);
/// L2（欧氏）距离。
pub const DistanceMetricL2: DistanceMetric = DistanceMetric(Cow::Borrowed("L2"));
/// 余弦距离。
pub const DistanceMetricCosine: DistanceMetric = DistanceMetric(Cow::Borrowed("COSINE"));
/// 内积距离。
pub const DistanceMetricInnerProduct: DistanceMetric =
    DistanceMetric(Cow::Borrowed("INNER_PRODUCT"));

/// 索引变更临时名前缀。
const changingIndexPrefix: &str = "_Idx$_";
/// 余弦距离函数的 SQL 名。
const VecCosineDistance: &str = "vec_cosine_distance";
/// L2 距离函数的 SQL 名。
const VecL2Distance: &str = "vec_l2_distance";

/// 全局索引版本：遗留（legacy）。
pub const GlobalIndexVersionLegacy: u8 = 0;
/// 全局索引版本 V1。
pub const GlobalIndexVersionV1: u8 = 1;
/// 全局索引版本 V2。
pub const GlobalIndexVersionV2: u8 = 2;

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

/// 运行时开关：是否支持全局索引 V1。
static globalIndexV1Supported: OnceLock<AtomicBool> = OnceLock::new();

fn globalIndexV1SupportFlag() -> &'static AtomicBool {
    globalIndexV1Supported.get_or_init(|| AtomicBool::new(kerneltype::IsNextGen()))
}

/// 设置是否支持全局索引 V1。
pub fn SetGlobalIndexV1Supported(supported: bool) {
    globalIndexV1SupportFlag().store(supported, Ordering::Release);
}
/// 查询是否支持全局索引 V1。
pub fn GetGlobalIndexV1Supported() -> bool {
    globalIndexV1SupportFlag().load(Ordering::Acquire)
}

/// 为变更中的索引生成表内唯一临时名 `_Idx$_Origin_N`。
pub fn GenUniqueChangingIndexName(table: &TableInfo, old: &IndexInfo) -> String {
    let names: HashSet<&str> = table
        .Indices
        .iter()
        .map(|index| index.Name.L.as_str())
        .collect();
    for suffix in 0.. {
        let candidate = format!("{changingIndexPrefix}{}_{}", old.Name.O, suffix);
        if !names.contains(candidate.to_lowercase().as_str()) {
            return candidate;
        }
    }
    unreachable!()
}

/// 可索引向量函数名 → 距离度量 的静态映射。
pub fn IndexableFnNameToDistanceMetric() -> &'static HashMap<&'static str, DistanceMetric> {
    static VALUE: OnceLock<HashMap<&'static str, DistanceMetric>> = OnceLock::new();
    VALUE.get_or_init(|| {
        HashMap::from([
            (VecCosineDistance, DistanceMetricCosine.clone()),
            (VecL2Distance, DistanceMetricL2.clone()),
        ])
    })
}
/// 距离度量 → 可索引向量函数名 的反向静态映射。
pub fn IndexableDistanceMetricToFnName() -> &'static HashMap<DistanceMetric, &'static str> {
    static VALUE: OnceLock<HashMap<DistanceMetric, &'static str>> = OnceLock::new();
    VALUE.get_or_init(|| {
        HashMap::from([
            (DistanceMetricCosine.clone(), VecCosineDistance),
            (DistanceMetricL2.clone(), VecL2Distance),
        ])
    })
}

/// 向量索引附加信息：维度与距离度量。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VectorIndexInfo {
    #[serde(rename = "dimension")]
    pub Dimension: u64,
    #[serde(rename = "distance_metric")]
    pub DistanceMetric: DistanceMetric,
}

/// 倒排索引附加信息：列 ID、有符号性、类型字节宽度。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct InvertedIndexInfo {
    #[serde(rename = "column_id")]
    pub ColumnID: i64,
    #[serde(rename = "is_signed")]
    pub IsSigned: bool,
    #[serde(rename = "type_size")]
    pub TypeSize: u8,
}

/// 将字段类型映射为倒排索引描述；不支持的类型返回 `None`。
pub fn FieldTypeToInvertedIndexInfo(
    field_type: &types::FieldType,
    column_id: i64,
) -> Option<InvertedIndexInfo> {
    let unsigned = mysql::HasUnsignedFlag(field_type.GetFlag());
    // 按 MySQL 类型确定存储宽度与是否有符号。
    let (TypeSize, IsSigned) = match field_type.GetType() {
        mysql::TypeTiny => (1, !unsigned),
        mysql::TypeShort => (2, !unsigned),
        mysql::TypeInt24 | mysql::TypeLong => (4, !unsigned),
        mysql::TypeLonglong => (8, !unsigned),
        mysql::TypeYear | mysql::TypeEnum => (2, false),
        mysql::TypeSet => (8, false),
        mysql::TypeDatetime | mysql::TypeDate | mysql::TypeTimestamp => (8, false),
        mysql::TypeDuration => (8, true),
        _ => return None,
    };
    Some(InvertedIndexInfo {
        ColumnID: column_id,
        IsSigned,
        TypeSize,
    })
}

/// 全文索引分词器类型标签。
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FullTextParserType(pub Cow<'static, str>);
/// 无效分词器。
pub const FullTextParserTypeInvalid: FullTextParserType =
    FullTextParserType(Cow::Borrowed("INVALID"));
/// STANDARD_V1 分词器。
pub const FullTextParserTypeStandardV1: FullTextParserType =
    FullTextParserType(Cow::Borrowed("STANDARD_V1"));
/// MULTILINGUAL_V1 分词器。
pub const FullTextParserTypeMultilingualV1: FullTextParserType =
    FullTextParserType(Cow::Borrowed("MULTILINGUAL_V1"));
impl Default for FullTextParserType {
    fn default() -> Self {
        FullTextParserTypeInvalid.clone()
    }
}
impl FullTextParserType {
    /// 返回面向 SQL 的简短名称（STANDARD / MULTILINGUAL / INVALID）。
    pub fn SQLName(&self) -> &'static str {
        match self.0.as_ref() {
            "STANDARD_V1" => "STANDARD",
            "MULTILINGUAL_V1" => "MULTILINGUAL",
            _ => "INVALID",
        }
    }
    /// 返回内部完整类型字符串。
    pub fn String(&self) -> &str {
        self.0.as_ref()
    }
}
/// 按 SQL 名称解析全文分词器类型（大小写不敏感）。
pub fn GetFullTextParserTypeBySQLName(name: &str) -> FullTextParserType {
    match name.to_ascii_uppercase().as_str() {
        "STANDARD" => FullTextParserTypeStandardV1.clone(),
        "MULTILINGUAL" => FullTextParserTypeMultilingualV1.clone(),
        _ => FullTextParserTypeInvalid.clone(),
    }
}

/// 全文索引附加信息。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FullTextIndexInfo {
    #[serde(rename = "parser_type")]
    pub ParserType: FullTextParserType,
}

/// 列存索引种类：倒排 / 向量 / 全文 / 未指定。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum ColumnarIndexType {
    #[default]
    NA = 0,
    Inverted = 1,
    Vector = 2,
    Fulltext = 3,
}
impl serde::Serialize for ColumnarIndexType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(*self as u8)
    }
}
impl<'de> serde::Deserialize<'de> for ColumnarIndexType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match u8::deserialize(deserializer)? {
            0 => Ok(Self::NA),
            1 => Ok(Self::Inverted),
            2 => Ok(Self::Vector),
            3 => Ok(Self::Fulltext),
            value => Err(serde::de::Error::custom(format_args!(
                "invalid columnar index type {value}"
            ))),
        }
    }
}
impl ColumnarIndexType {
    /// 返回面向 SQL/展示的类型名。
    pub fn SQLName(self) -> &'static str {
        match self {
            Self::Vector => "vector index",
            Self::Inverted => "inverted index",
            Self::Fulltext => "fulltext index",
            Self::NA => "columnar index",
        }
    }
}

/// Region 预拆分策略：键范围上下界与目标 Region 数。
///
/// Region 是分布式 KV 层的数据分片单位。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct RegionSplitPolicy {
    #[serde(rename = "lower")]
    pub Lower: Vec<String>,
    #[serde(rename = "upper")]
    pub Upper: Vec<String>,
    #[serde(rename = "regions")]
    pub Regions: i64,
}
impl RegionSplitPolicy {
    /// 克隆本策略。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
}

/// 索引中的单列定义：名称、表内偏移、前缀长度。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IndexColumn {
    #[serde(rename = "name")]
    pub Name: ast::CIStr,
    #[serde(rename = "offset")]
    pub Offset: isize,
    /// 前缀索引长度；`UnspecifiedLength` 表示整列。
    #[serde(rename = "length")]
    pub Length: isize,
    /// 是否使用变更中的目标列类型。
    #[serde(
        rename = "using_changing_type",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub UseChangingType: bool,
}
impl IndexColumn {
    /// 克隆本索引列。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
}

/// 完整索引元数据：列列表、唯一/主键、列存扩展、partial 条件等。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct IndexInfo {
    #[serde(rename = "id")]
    pub ID: i64,
    #[serde(rename = "idx_name", alias = "name")]
    pub Name: ast::CIStr,
    #[serde(rename = "tbl_name")]
    pub Table: ast::CIStr,
    #[serde(rename = "idx_cols")]
    pub Columns: Vec<IndexColumn>,
    #[serde(rename = "state")]
    pub State: SchemaState,
    /// 回填（backfill）状态：索引重建过程中的进度。
    #[serde(rename = "backfill_state")]
    pub BackfillState: BackfillState,
    #[serde(rename = "comment")]
    pub Comment: String,
    #[serde(rename = "index_type")]
    pub Tp: ast::IndexType,
    #[serde(rename = "is_unique")]
    pub Unique: bool,
    #[serde(rename = "is_primary")]
    pub Primary: bool,
    #[serde(rename = "is_invisible")]
    pub Invisible: bool,
    /// 是否为全局索引（跨分区一张索引）。
    #[serde(rename = "is_global")]
    pub Global: bool,
    /// 是否为多值索引（MV Index）。
    #[serde(rename = "mv_index")]
    pub MVIndex: bool,
    #[serde(rename = "vector_index")]
    pub VectorInfo: Option<VectorIndexInfo>,
    #[serde(rename = "inverted_index")]
    pub InvertedInfo: Option<InvertedIndexInfo>,
    #[serde(rename = "full_text_index")]
    pub FullTextInfo: Option<FullTextIndexInfo>,
    /// partial index 条件表达式字符串。
    #[serde(rename = "condition_expr_string")]
    pub ConditionExprString: String,
    #[serde(rename = "affect_column", skip_serializing_if = "Option::is_none")]
    pub AffectColumn: Option<Vec<IndexColumn>>,
    #[serde(rename = "global_index_version", skip_serializing_if = "is_zero_u8")]
    pub GlobalIndexVersion: u8,
    #[serde(
        rename = "region_split_policy",
        skip_serializing_if = "Option::is_none"
    )]
    pub RegionSplitPolicy: Option<RegionSplitPolicy>,
}

impl IndexInfo {
    /// 将索引 ID 写入哈希器（用于计划缓存等）。
    pub fn Hash64(&self, hasher: &mut dyn base::Hasher) {
        hasher.HashInt64(self.ID);
    }
    /// 按 ID 判断是否与另一对象相等。
    pub fn Equals(&self, other: &dyn Any) -> bool {
        other
            .downcast_ref::<IndexInfo>()
            .is_some_and(|index| index.ID == self.ID)
    }
    /// 克隆本索引元数据。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
    /// 是否处于变更临时命名中。
    pub fn IsChanging(&self) -> bool {
        self.Name.O.starts_with(changingIndexPrefix)
    }
    /// 是否处于删除墓碑命名中。
    pub fn IsRemoving(&self) -> bool {
        self.Name.O.starts_with(removingObjPrefix)
    }
    /// 从墓碑名还原原始索引名。
    pub fn GetRemovingOriginName(&self) -> String {
        self.Name
            .O
            .strip_prefix(removingObjPrefix)
            .unwrap_or(&self.Name.O)
            .to_owned()
    }
    /// 从变更临时名还原原始索引名。
    pub fn GetChangingOriginName(&self) -> String {
        let name = self
            .Name
            .O
            .strip_prefix(changingIndexPrefix)
            .unwrap_or(&self.Name.O);
        name.rsplit_once('_')
            .map_or_else(|| name.to_owned(), |(origin, _)| origin.to_owned())
    }
    /// 是否包含前缀索引列（Length 非 Unspecified）。
    pub fn HasPrefixIndex(&self) -> bool {
        self.Columns
            .iter()
            .any(|column| column.Length != types::UnspecifiedLength)
    }
    /// 索引列是否引用指定列 ID。
    pub fn HasColumnInIndexColumns(&self, table: &TableInfo, column_id: i64) -> bool {
        self.Columns.iter().any(|column| {
            column.Offset >= 0
                && table
                    .Columns
                    .get(column.Offset as usize)
                    .is_some_and(|value| value.ID == column_id)
        })
    }
    /// 按列名查找索引列定义。
    pub fn FindColumnByName(&self, name: &str) -> Option<&IndexColumn> {
        FindIndexColumnByName(&self.Columns, name).1
    }
    /// Schema 状态是否为 Public（对外可见可用）。
    pub fn IsPublic(&self) -> bool {
        self.State == StatePublic
    }
    /// 是否为列存索引（向量/倒排/全文之一）。
    pub fn IsColumnarIndex(&self) -> bool {
        self.VectorInfo.is_some() || self.InvertedInfo.is_some() || self.FullTextInfo.is_some()
    }
    /// 返回具体列存索引类型。
    pub fn GetColumnarIndexType(&self) -> ColumnarIndexType {
        if self.VectorInfo.is_some() {
            ColumnarIndexType::Vector
        } else if self.InvertedInfo.is_some() {
            ColumnarIndexType::Inverted
        } else if self.FullTextInfo.is_some() {
            ColumnarIndexType::Fulltext
        } else {
            ColumnarIndexType::NA
        }
    }
    /// 是否带有 partial index 条件。
    pub fn HasCondition(&self) -> bool {
        !self.ConditionExprString.is_empty()
    }
    /// 将条件字符串解析为 AST 表达式节点。
    pub fn ConditionExpr(&self) -> Result<ast::ExprNode, String> {
        let mut parser = parser::New();
        // 包成 SELECT 语句再取第一列表达式，复用现有 SQL 解析器。
        let statement = parser
            .ParseOneStmt(&format!("select {}", self.ConditionExprString), "", "")
            .map_err(|error| error.to_string())?;
        let select = statement
            .into_any()
            .downcast::<ast::SelectStmt>()
            .map_err(|_| "condition did not parse as SELECT".to_owned())?;
        select
            .Fields
            .Fields
            .first()
            .and_then(|field| field.Expr.clone())
            .ok_or_else(|| "condition SELECT has no expression".to_owned())
    }
}

/// 在索引列表中查找能前缀覆盖指定列序列的索引。
pub fn FindIndexByColumns<'a>(
    table: &TableInfo,
    indices: &'a [IndexInfo],
    columns: &[ast::CIStr],
) -> Option<&'a IndexInfo> {
    indices
        .iter()
        .find(|index| IsIndexPrefixCovered(table, index, columns))
}

/// 判断索引左侧列是否按序覆盖给定列序列（含前缀长度约束）。
pub fn IsIndexPrefixCovered(table: &TableInfo, index: &IndexInfo, columns: &[ast::CIStr]) -> bool {
    if index.Columns.len() < columns.len() {
        return false;
    }
    columns.iter().zip(&index.Columns).all(|(wanted, indexed)| {
        if wanted.L != indexed.Name.L || indexed.Offset < 0 {
            return false;
        }
        let Some(column) = table.Columns.get(indexed.Offset as usize) else {
            return false;
        };
        // 前缀索引的 Length 必须足以覆盖列完整 flen，否则不算覆盖。
        indexed.Length == types::UnspecifiedLength || indexed.Length >= column.GetFlen()
    })
}

/// 在索引列表中查找可同时满足外键列与 partial 条件的覆盖索引。
pub fn FindIndexByColumnsForForeignKey<'a>(
    table: &TableInfo,
    indices: &'a [IndexInfo],
    columns: &[ast::CIStr],
) -> Option<&'a IndexInfo> {
    indices
        .iter()
        .find(|index| IsIndexPrefixCoveredForForeignKey(table, index, columns))
}
/// 外键场景：既要前缀覆盖，partial 条件也只能是外键列上的 `IS NOT NULL`。
pub fn IsIndexPrefixCoveredForForeignKey(
    table: &TableInfo,
    index: &IndexInfo,
    columns: &[ast::CIStr],
) -> bool {
    IsIndexPrefixCovered(table, index, columns)
        && isIndexConditionCoveredByForeignKeyCols(index, columns)
}
/// 校验 partial 条件是否仅为某外键列的 `IS NOT NULL`。
fn isIndexConditionCoveredByForeignKeyCols(index: &IndexInfo, columns: &[ast::CIStr]) -> bool {
    if !index.HasCondition() {
        return true;
    }
    let Ok(expression) = index.ConditionExpr() else {
        return false;
    };
    // 仅接受 `col IS NOT NULL` 形态。
    let ast::ExprKind::IsNull { Expr, Not: true } = expression.Kind else {
        return false;
    };
    let ast::ExprKind::Column(column) = Expr.Kind else {
        return false;
    };
    columns.iter().any(|candidate| column.Name.L == candidate.L)
}

/// 按索引 ID 查找。
pub fn FindIndexInfoByID(indices: &[IndexInfo], id: i64) -> Option<&IndexInfo> {
    indices.iter().find(|index| index.ID == id)
}
/// 按列名查找索引列，返回 (偏移, 可选引用)；未找到偏移为 -1。
pub fn FindIndexColumnByName<'a>(
    columns: &'a [IndexColumn],
    name: &str,
) -> (isize, Option<&'a IndexColumn>) {
    columns
        .iter()
        .enumerate()
        .find(|(_, column)| column.Name.L == name)
        .map_or((-1, None), |(offset, column)| {
            (offset as isize, Some(column))
        })
}

/// 在 next-gen 内核上启用全局索引 V1 支持。
pub fn InitGlobalIndexSupport() {
    if kerneltype::IsNextGen() {
        SetGlobalIndexV1Supported(true);
    }
}
