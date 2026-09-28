// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// MATCH ... AGAINST fallback and the small expression model shared by task 700.
// The fallback follows `fts_to_like.go`: it validates the same strict token
// subset and builds executable ILIKE/IFNULL/AND/OR/NOT expression trees.
//
// 全文检索（FTS，Full-Text Search）的 `MATCH ... AGAINST` 在无法走原生索引时，
// 可降级为 ILIKE 模式匹配表达式树。本模块对应 Go `fts_to_like.go`：
// - 校验查询词是否属于 LIKE 回退支持的严格词法子集；
// - 按布尔模式 / 自然语言模式构造 IFNULL(ILIKE(...))/AND/OR/NOT 树；
// - 同时提供本任务共用的轻量表达式模型（Datum、Column、ScalarFunction 等）。

#[path = "function_traits.rs"]
pub mod function_traits;
#[path = "grouping_sets.rs"]
pub mod grouping_sets;
#[path = "helper.rs"]
pub mod helper;
#[path = "infer_pushdown.rs"]
pub mod infer_pushdown;

use std::fmt;

use thiserror::Error;

/// MySQL 全文检索修饰符：自然语言、布尔模式，以及各自的查询扩展变体。
/// 查询扩展（WITH QUERY EXPANSION）在 LIKE 回退路径中不受支持。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FulltextSearchModifier {
    NaturalLanguage,
    Boolean,
    NaturalLanguageWithQueryExpansion,
    BooleanWithQueryExpansion,
}

impl FulltextSearchModifier {
    /// 是否为布尔模式（含查询扩展变体）。
    pub fn is_boolean_mode(self) -> bool {
        matches!(self, Self::Boolean | Self::BooleanWithQueryExpansion)
    }

    /// 是否为自然语言模式（含查询扩展变体）。
    pub fn is_natural_language_mode(self) -> bool {
        matches!(
            self,
            Self::NaturalLanguage | Self::NaturalLanguageWithQueryExpansion
        )
    }

    /// 是否启用查询扩展；LIKE 回退对此返回不支持错误。
    pub fn with_query_expansion(self) -> bool {
        matches!(
            self,
            Self::NaturalLanguageWithQueryExpansion | Self::BooleanWithQueryExpansion
        )
    }
}

/// 轻量常量取值，对应表达式树中的常量节点。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Datum {
    Null,
    Int(i64),
    String(String),
}

/// 字段类型族，对应 MySQL / TiDB 的 EvalType 粗分类。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FieldKind {
    Unspecified,
    Int,
    Real,
    Decimal,
    String,
    Enum,
    Bit,
    Set,
    Geometry,
    Json,
    Vector,
    Time,
    Duration,
    Year,
}

/// 字段类型元数据：长度、小数位、字符集/校对规则等。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FieldType {
    pub kind: FieldKind,
    pub flen: i32,
    pub decimal: i32,
    pub unsigned: bool,
    pub not_null: bool,
    pub hybrid: bool,
    pub charset: String,
    pub collation: String,
}

impl FieldType {
    /// 构造默认整型字段类型。
    pub fn integer() -> Self {
        Self::new(FieldKind::Int)
    }

    /// 构造 utf8mb4 / utf8mb4_bin 的 VARCHAR 风格字符串类型。
    pub fn varchar() -> Self {
        let mut result = Self::new(FieldKind::String);
        result.charset = "utf8mb4".into();
        result.collation = "utf8mb4_bin".into();
        result
    }

    /// 构造 ENUM 类型占位。
    pub fn enum_type() -> Self {
        Self::new(FieldKind::Enum)
    }

    /// 按类型族构造默认字段；flen/decimal 为 -1 表示未指定。
    pub fn new(kind: FieldKind) -> Self {
        Self {
            kind,
            flen: -1,
            decimal: -1,
            unsigned: false,
            not_null: false,
            hybrid: false,
            charset: String::new(),
            collation: String::new(),
        }
    }

    /// 构造带精度的 DECIMAL 类型。
    pub fn decimal(flen: i32, decimal: i32) -> Self {
        Self {
            flen,
            decimal,
            ..Self::new(FieldKind::Decimal)
        }
    }

    /// 校验 DECIMAL 的 flen/decimal 是否落在 MySQL 合法范围内。
    pub fn is_decimal_valid(&self) -> bool {
        self.kind != FieldKind::Decimal
            || (self.flen >= 0
                && self.flen <= 65
                && self.decimal >= 0
                && self.decimal <= 30
                && self.decimal <= self.flen)
    }
}

/// 列引用节点：unique_id 标识优化器中的唯一列，index 用于按行取值。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Column {
    pub unique_id: i64,
    pub index: usize,
    pub field_type: FieldType,
    pub encodable: bool,
}

impl Column {
    /// 构造默认可编码的列引用。
    pub fn new(unique_id: i64, index: usize, field_type: FieldType) -> Self {
        Self {
            unique_id,
            index,
            field_type,
            encodable: true,
        }
    }
}

/// CAST 目标类型族，用于签名枚举中的 Cast 变体。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CastFamily {
    Int,
    Real,
    Decimal,
    String,
    Json,
    Vector,
    Time,
    Duration,
    Unsupported,
}

/// 标量函数签名：未指定、通用名称，或 CAST 族。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Signature {
    Unspecified,
    Generic(String),
    Cast(CastFamily),
}

impl Signature {
    /// 返回用于调试/展示的签名名称。
    pub fn name(&self) -> &str {
        match self {
            Self::Unspecified => "Unspecified",
            Self::Generic(name) => name,
            Self::Cast(CastFamily::Int) => "CastInt",
            Self::Cast(CastFamily::Real) => "CastReal",
            Self::Cast(CastFamily::Decimal) => "CastDecimal",
            Self::Cast(CastFamily::String) => "CastString",
            Self::Cast(CastFamily::Json) => "CastJson",
            Self::Cast(CastFamily::Vector) => "CastVector",
            Self::Cast(CastFamily::Time) => "CastTime",
            Self::Cast(CastFamily::Duration) => "CastDuration",
            Self::Cast(CastFamily::Unsupported) => "Unsupported",
        }
    }
}

/// 标量函数节点；FTS 场景下可携带 FulltextSearchModifier。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ScalarFunction {
    pub name: String,
    pub signature: Signature,
    pub args: Vec<Expression>,
    pub ret_type: FieldType,
    pub modifier: Option<FulltextSearchModifier>,
    pub metadata_serializable: bool,
}

impl ScalarFunction {
    /// 构造普通标量函数；函数名统一转小写以匹配内置名约定。
    pub fn new(
        name: impl Into<String>,
        signature: Signature,
        args: Vec<Expression>,
        ret_type: FieldType,
    ) -> Self {
        Self {
            name: name.into().to_ascii_lowercase(),
            signature,
            args,
            ret_type,
            modifier: None,
            metadata_serializable: true,
        }
    }

    /// 构造 `fts_mysql_match_against`：参数为搜索常量 + 列列表，并附带修饰符。
    pub fn fts(
        search: Expression,
        columns: Vec<Expression>,
        modifier: FulltextSearchModifier,
    ) -> Self {
        let mut args = Vec::with_capacity(columns.len() + 1);
        args.push(search);
        args.extend(columns);
        let mut result = Self::new(
            "fts_mysql_match_against",
            Signature::Generic("FTSMysqlMatchAgainst".into()),
            args,
            FieldType::new(FieldKind::Real),
        );
        result.modifier = Some(modifier);
        result
    }
}

/// 轻量表达式树：常量、列、标量函数，或不支持节点。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Expression {
    Constant(Datum),
    Column(Column),
    ScalarFunction(ScalarFunction),
    Unsupported(String),
}

impl Expression {
    /// 包装常量 Datum。
    pub fn constant(value: Datum) -> Self {
        Self::Constant(value)
    }

    /// 构造列引用表达式。
    pub fn column(unique_id: i64, index: usize, field_type: FieldType) -> Self {
        Self::Column(Column::new(unique_id, index, field_type))
    }

    /// 构造标量函数表达式。
    pub fn scalar(
        name: impl Into<String>,
        signature: Signature,
        args: Vec<Expression>,
        ret_type: FieldType,
    ) -> Self {
        Self::ScalarFunction(ScalarFunction::new(name, signature, args, ret_type))
    }

    /// 推导表达式返回类型；常量按值形态推断，列/函数取其声明类型。
    pub fn field_type(&self) -> FieldType {
        match self {
            Self::Constant(Datum::String(_)) => FieldType::varchar(),
            Self::Constant(_) => FieldType::integer(),
            Self::Column(column) => column.field_type.clone(),
            Self::ScalarFunction(function) => function.ret_type.clone(),
            Self::Unsupported(_) => FieldType::new(FieldKind::Unspecified),
        }
    }

    /// 收集表达式树中出现的列 unique_id（深度优先）。
    pub fn collect_column_ids(&self, output: &mut Vec<i64>) {
        match self {
            Self::Column(column) => output.push(column.unique_id),
            Self::ScalarFunction(function) => {
                for arg in &function.args {
                    arg.collect_column_ids(output);
                }
            }
            Self::Constant(_) | Self::Unsupported(_) => {}
        }
    }

    /// 生成用于去重/缓存键的规范化字符串表示。
    pub fn canonical_key(&self) -> String {
        match self {
            Self::Constant(value) => format!("const:{value:?}"),
            Self::Column(column) => format!("column:{}", column.unique_id),
            Self::ScalarFunction(function) => format!(
                "{}({})",
                function.name,
                function
                    .args
                    .iter()
                    .map(Self::canonical_key)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Self::Unsupported(value) => format!("unsupported:{value}"),
        }
    }

    /// 将求值结果解释为布尔谓词：NULL→None，非零整数→true，字符串不可作谓词。
    pub fn eval_bool(&self, row: &[Option<&str>]) -> Result<Option<bool>, FtsError> {
        match self.eval(row)? {
            EvalValue::Null => Ok(None),
            EvalValue::Int(value) => Ok(Some(value != 0)),
            EvalValue::String(_) => Err(FtsError::Evaluation("string used as predicate".into())),
        }
    }

    /// 对单行求值；仅实现 FTS→ILIKE 回退树所需的 ilike/ifnull/not/and/or。
    fn eval(&self, row: &[Option<&str>]) -> Result<EvalValue, FtsError> {
        match self {
            Self::Constant(Datum::Null) => Ok(EvalValue::Null),
            Self::Constant(Datum::Int(value)) => Ok(EvalValue::Int(*value)),
            Self::Constant(Datum::String(value)) => Ok(EvalValue::String(value.clone())),
            Self::Column(column) => Ok(row
                .get(column.index)
                .copied()
                .flatten()
                .map(|value| EvalValue::String(value.to_owned()))
                .unwrap_or(EvalValue::Null)),
            Self::Unsupported(value) => Err(FtsError::Evaluation(value.clone())),
            Self::ScalarFunction(function) => match function.name.as_str() {
                "ilike" => {
                    let value = function.args[0].eval(row)?;
                    let pattern = function.args[1].eval(row)?;
                    match (value, pattern) {
                        (EvalValue::Null, _) | (_, EvalValue::Null) => Ok(EvalValue::Null),
                        (EvalValue::String(value), EvalValue::String(pattern)) => {
                            Ok(EvalValue::Int(ilike(&value, &pattern) as i64))
                        }
                        _ => Err(FtsError::Evaluation("invalid ILIKE arguments".into())),
                    }
                }
                "ifnull" => {
                    let first = function.args[0].eval(row)?;
                    if first == EvalValue::Null {
                        function.args[1].eval(row)
                    } else {
                        Ok(first)
                    }
                }
                "not" => Ok(match function.args[0].eval_bool(row)? {
                    Some(value) => EvalValue::Int((!value) as i64),
                    None => EvalValue::Null,
                }),
                // SQL AND：遇假短路为 0；全真为 1；有 NULL 且无假则为 NULL。
                "and" => {
                    let mut saw_null = false;
                    for arg in &function.args {
                        match arg.eval_bool(row)? {
                            Some(false) => return Ok(EvalValue::Int(0)),
                            Some(true) => {}
                            None => saw_null = true,
                        }
                    }
                    Ok(if saw_null {
                        EvalValue::Null
                    } else {
                        EvalValue::Int(1)
                    })
                }
                // SQL OR：遇真短路为 1；全假为 0；有 NULL 且无真则为 NULL。
                "or" => {
                    let mut saw_null = false;
                    for arg in &function.args {
                        match arg.eval_bool(row)? {
                            Some(true) => return Ok(EvalValue::Int(1)),
                            Some(false) => {}
                            None => saw_null = true,
                        }
                    }
                    Ok(if saw_null {
                        EvalValue::Null
                    } else {
                        EvalValue::Int(0)
                    })
                }
                name => Err(FtsError::Evaluation(format!("cannot evaluate {name}"))),
            },
        }
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Constant(value) => write!(formatter, "{value:?}"),
            Self::Column(column) => write!(formatter, "Column#{}", column.unique_id),
            Self::ScalarFunction(function) => write!(
                formatter,
                "{}({})",
                function.name,
                function
                    .args
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Unsupported(value) => formatter.write_str(value),
        }
    }
}

/// 内部求值结果：NULL / 整数 / 字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
enum EvalValue {
    Null,
    Int(i64),
    String(String),
}

/// FTS→ILIKE 回退相关错误：不支持、非法内置形态、求值失败。
#[derive(Debug, Error, Eq, PartialEq)]
pub enum FtsError {
    #[error("{0}")]
    NotSupported(String),
    #[error("{0}")]
    InvalidBuiltin(String),
    #[error("{0}")]
    Evaluation(String),
}

/// 布尔模式解析出的检索词：必选（+）、排除（-）或可选。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FtsSearchTerm {
    pub word: String,
    pub is_required: bool,
    pub is_excluded: bool,
}

impl FtsSearchTerm {
    /// 构造必选词（对应 `+word`）。
    pub fn required(word: impl Into<String>) -> Self {
        Self {
            word: word.into(),
            is_required: true,
            is_excluded: false,
        }
    }

    /// 构造排除词（对应 `-word`）。
    pub fn excluded(word: impl Into<String>) -> Self {
        Self {
            word: word.into(),
            is_required: false,
            is_excluded: true,
        }
    }

    /// 构造可选词（无前缀）。
    pub fn optional(word: impl Into<String>) -> Self {
        Self {
            word: word.into(),
            is_required: false,
            is_excluded: false,
        }
    }
}

/// 按空白切分并解析布尔模式检索串。
pub fn parse_fts_boolean_search_string(text: &str) -> Vec<FtsSearchTerm> {
    text.split_whitespace().map(parse_fts_search_term).collect()
}

/// 解析单个词的前缀修饰：`+` 必选、`-` 排除，其余可选。
pub fn parse_fts_search_term(word: &str) -> FtsSearchTerm {
    match word.as_bytes().first() {
        Some(b'+') => FtsSearchTerm::required(&word[1..]),
        Some(b'-') => FtsSearchTerm::excluded(&word[1..]),
        _ => FtsSearchTerm::optional(word),
    }
}

/// 判断字节是否可作为 FTS 词字符：ASCII 字母数字，或非 ASCII（如中文）。
fn is_fts_word_byte(value: u8) -> bool {
    value.is_ascii_alphanumeric() || value > 127
}

/// 转义 LIKE 特殊字符 `\ % _`，便于拼入 `%term%` 模式。
pub fn escape_fts_like_pattern(term: &str) -> String {
    let escape_count = term
        .bytes()
        .filter(|value| matches!(value, b'\\' | b'%' | b'_'))
        .count();
    let mut result = String::with_capacity(term.len() + escape_count);
    for value in term.chars() {
        if matches!(value, '\\' | '%' | '_') {
            result.push('\\');
        }
        result.push(value);
    }
    result
}

/// 校验搜索串是否可走 LIKE 回退：仅允许词字符；布尔模式下允许首字符 `+/-`。
pub fn validate_fts_search_string_for_like_fallback(
    search_text: &str,
    modifier: FulltextSearchModifier,
) -> Result<(), FtsError> {
    for token in search_text.split_whitespace() {
        // 布尔模式剥掉前缀 +/- 后再检查词体；自然语言模式不允许这些前缀字符。
        let body = if modifier.is_boolean_mode()
            && matches!(token.as_bytes().first(), Some(b'+') | Some(b'-'))
        {
            &token[1..]
        } else {
            token
        };
        if body.is_empty() || body.bytes().any(|value| !is_fts_word_byte(value)) {
            return Err(FtsError::NotSupported(format!(
                "MATCH...AGAINST search term '{token}' is not supported in the LIKE fallback"
            )));
        }
    }
    Ok(())
}

/// 将 MATCH...AGAINST 降级为可执行的 ILIKE 表达式树（按修饰符分派）。
pub fn build_fts_to_ilike_expression(
    columns: &[Expression],
    search_text: &str,
    modifier: FulltextSearchModifier,
) -> Result<Expression, FtsError> {
    if columns.is_empty() {
        return Err(FtsError::NotSupported(
            "MATCH...AGAINST with no columns".into(),
        ));
    }
    if modifier.with_query_expansion() {
        return Err(FtsError::NotSupported(
            "MATCH...AGAINST WITH QUERY EXPANSION is not supported in the LIKE fallback".into(),
        ));
    }
    validate_fts_search_string_for_like_fallback(search_text, modifier)?;
    if search_text.is_empty() {
        return Ok(zero());
    }
    if modifier.is_boolean_mode() {
        return build_fts_boolean_mode(columns, search_text);
    }
    if modifier.is_natural_language_mode() {
        return build_fts_natural_language_mode(columns, search_text);
    }
    Err(FtsError::NotSupported(
        "MATCH...AGAINST modifier is not supported".into(),
    ))
}

/// 布尔模式：必选词 AND、排除词 NOT、仅可选词时 OR；仅排除词则恒假。
fn build_fts_boolean_mode(
    columns: &[Expression],
    search_text: &str,
) -> Result<Expression, FtsError> {
    let terms = parse_fts_boolean_search_string(search_text);
    if terms.is_empty() {
        return Ok(zero());
    }
    let required: Vec<_> = terms
        .iter()
        .filter(|term| term.is_required && !term.word.is_empty())
        .collect();
    let excluded: Vec<_> = terms
        .iter()
        .filter(|term| term.is_excluded && !term.word.is_empty())
        .collect();
    let optional: Vec<_> = terms
        .iter()
        .filter(|term| !term.is_required && !term.is_excluded && !term.word.is_empty())
        .collect();
    // 只有排除词时 MySQL 语义无法单独命中，回退为常量 0。
    if required.is_empty() && optional.is_empty() && !excluded.is_empty() {
        return Ok(zero());
    }

    let mut predicates = Vec::new();
    // 每个必选词：任一列 ILIKE 命中即可（列间 OR）。
    for term in &required {
        predicates.push(compose(
            "or",
            columns
                .iter()
                .map(|column| ilike_predicate(column, &term.word))
                .collect(),
        ));
    }
    // 排除词取反：任一列命中则整行不匹配。
    for term in &excluded {
        let dnf = compose(
            "or",
            columns
                .iter()
                .map(|column| ilike_predicate(column, &term.word))
                .collect(),
        );
        predicates.push(Expression::scalar(
            "not",
            Signature::Generic("UnaryNot".into()),
            vec![dnf],
            FieldType::integer(),
        ));
    }
    // 无必选、有可选时：所有可选词×列构成大 OR；若已有排除谓词则再 AND。
    if required.is_empty() && !optional.is_empty() {
        let optional_dnf = compose(
            "or",
            optional
                .iter()
                .flat_map(|term| {
                    columns
                        .iter()
                        .map(move |column| ilike_predicate(column, &term.word))
                })
                .collect(),
        );
        if predicates.is_empty() {
            return Ok(optional_dnf);
        }
        predicates.push(optional_dnf);
    }
    if predicates.is_empty() {
        Ok(zero())
    } else {
        Ok(compose("and", predicates))
    }
}

/// 自然语言模式：每列对所有词做 OR，再对列结果做 OR（任一列命中任一词即可）。
fn build_fts_natural_language_mode(
    columns: &[Expression],
    search_text: &str,
) -> Result<Expression, FtsError> {
    let words: Vec<_> = search_text.split_whitespace().collect();
    if words.is_empty() {
        return Ok(zero());
    }
    let per_column = columns
        .iter()
        .map(|column| {
            compose(
                "or",
                words
                    .iter()
                    .map(|word| ilike_predicate(column, word))
                    .collect(),
            )
        })
        .collect();
    Ok(compose("or", per_column))
}

/// 从 `fts_mysql_match_against` 内置形态抽出列与常量搜索串，再构建 ILIKE 树。
/// 选择性替换场景仅支持单列；搜索常量为 NULL 时直接返回 NULL。
pub fn build_fts_to_ilike_expression_from_builtin(
    fts: &ScalarFunction,
) -> Result<Expression, FtsError> {
    if fts.name != "fts_mysql_match_against" {
        return Err(FtsError::InvalidBuiltin(format!(
            "expected fts_mysql_match_against, got {}",
            fts.name
        )));
    }
    if !matches!(
        &fts.signature,
        Signature::Generic(name) if name == "FTSMysqlMatchAgainst"
    ) {
        return Err(FtsError::InvalidBuiltin(format!(
            "unexpected builtin signature for fts_mysql_match_against: {}",
            fts.signature.name()
        )));
    }
    if fts.args.len() < 2 {
        return Err(FtsError::InvalidBuiltin(format!(
            "fts_mysql_match_against expects at least 2 args, got {}",
            fts.args.len()
        )));
    }
    if fts.args.len() > 2 {
        return Err(FtsError::NotSupported(
            "multi-column MATCH...AGAINST in selectivity substitution".into(),
        ));
    }
    let search = match &fts.args[0] {
        Expression::Constant(Datum::Null) => return Ok(Expression::Constant(Datum::Null)),
        Expression::Constant(Datum::String(value)) => value,
        Expression::Constant(_) => {
            return Err(FtsError::NotSupported(
                "MATCH...AGAINST with non-string search constant".into(),
            ));
        }
        _ => {
            return Err(FtsError::NotSupported(
                "MATCH...AGAINST with non-constant search string".into(),
            ));
        }
    };
    let modifier = fts
        .modifier
        .ok_or_else(|| FtsError::InvalidBuiltin("missing FTS modifier".into()))?;
    build_fts_to_ilike_expression(&fts.args[1..], search, modifier)
}

/// 构造 `IFNULL(column ILIKE '%escaped%' ESCAPE '\\', 0)`，把 NULL 匹配当成假。
fn ilike_predicate(column: &Expression, term: &str) -> Expression {
    let pattern = Expression::Constant(Datum::String(format!(
        "%{}%",
        escape_fts_like_pattern(term)
    )));
    let escape = Expression::Constant(Datum::Int(92));
    let ilike = Expression::scalar(
        "ilike",
        Signature::Generic("Ilike".into()),
        vec![column.clone(), pattern, escape],
        FieldType::integer(),
    );
    Expression::scalar(
        "ifnull",
        Signature::Generic("IfNull".into()),
        vec![ilike, zero()],
        FieldType::integer(),
    )
}

/// 单元素直接返回；多元素折叠为 AND/OR 标量函数。
fn compose(name: &str, expressions: Vec<Expression>) -> Expression {
    if expressions.len() == 1 {
        return expressions.into_iter().next().expect("one expression");
    }
    Expression::scalar(
        name,
        Signature::Generic(if name == "and" { "LogicAnd" } else { "LogicOr" }.into()),
        expressions,
        FieldType::integer(),
    )
}

/// 常量整数 0，表示谓词恒假。
fn zero() -> Expression {
    Expression::Constant(Datum::Int(0))
}

/// 简化的大小写不敏感子串匹配：剥离模式两端 `%` 并处理 `\` 转义后做 contains。
fn ilike(value: &str, pattern: &str) -> bool {
    let body = pattern.strip_prefix('%').unwrap_or(pattern);
    let body = body.strip_suffix('%').unwrap_or(body);
    let mut literal = String::with_capacity(body.len());
    let mut escaped = false;
    for character in body.chars() {
        if escaped {
            literal.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            literal.push(character);
        }
    }
    if escaped {
        literal.push('\\');
    }
    value.to_lowercase().contains(&literal.to_lowercase())
}
