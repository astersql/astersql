// Copyright 2026 AsterSQL.

// Shared runtime for the independently migrated LIKE and math vector kernels.
//
// 遗留向量化求值运行时：为独立迁移的 LIKE / 数学等内核提供共享类型。
//
// 包含常量级别、行/列（Chunk）、求值上下文与警告、标量/向量化求值 trait，
// 以及字面量表达式与数学函数公共基座。向量化指按批（Chunk）而非逐行求值。

use std::fmt;
use std::sync::{Arc, Mutex};

use mathutil::MysqlRng;
use types_dependency::decimal::mydecimal::{DecimalError, MyDecimal};

/// 表达式常量级别：不可折叠 / 仅上下文常量 / 严格常量。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConstLevel {
    ConstNone,
    ConstOnlyInContext,
    ConstStrict,
}

/// 行下标包装（对应 Go `chunk.Row`）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Row(pub usize);

/// 简易 Chunk：仅记录行数，供向量化接口签名对齐。
#[derive(Clone, Debug, Default)]
pub struct Chunk {
    rows: usize,
}

impl Chunk {
    pub fn new(rows: usize) -> Self {
        Self { rows }
    }

    pub fn NumRows(&self) -> usize {
        self.rows
    }
}

/// 求值上下文：收集 SQL 警告（如溢出截断提示）。
#[derive(Clone, Debug, Default)]
pub struct EvalContext {
    warnings: Arc<Mutex<Vec<String>>>,
}

impl EvalContext {
    /// 追加一条警告消息。
    pub fn append_warning(&self, warning: impl Into<String>) {
        self.warnings
            .lock()
            .expect("expression warning mutex poisoned")
            .push(warning.into());
    }

    /// 返回已收集警告的快照。
    pub fn warnings(&self) -> Vec<String> {
        self.warnings
            .lock()
            .expect("expression warning mutex poisoned")
            .clone()
    }
}

/// 求值错误：通用消息、DOUBLE/BIGINT 溢出、DECIMAL 错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvalError {
    Message(String),
    DoubleOverflow(String),
    BigIntOverflow(String),
    Decimal(DecimalError),
}

impl fmt::Display for EvalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => formatter.write_str(message),
            // MySQL 错误码 1690：数值超出范围。
            Self::DoubleOverflow(expression) => {
                write!(
                    formatter,
                    "[types:1690]DOUBLE value is out of range in '{expression}'"
                )
            }
            Self::BigIntOverflow(expression) => {
                write!(
                    formatter,
                    "[types:1690]BIGINT value is out of range in '{expression}'"
                )
            }
            Self::Decimal(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for EvalError {}

impl From<DecimalError> for EvalError {
    fn from(value: DecimalError) -> Self {
        Self::Decimal(value)
    }
}

/// 本模块求值 Result 别名。
pub type Result<T> = std::result::Result<T, EvalError>;

/// 字段类型精简视图（目前仅 unsigned 标志）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FieldType {
    pub unsigned: bool,
}

/// 列存储：null 位图 + 各求值类型专用缓冲区。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ColumnKind {
    #[default]
    Uninitialized,
    Int,
    Real,
    Decimal,
    String,
}

#[derive(Clone, Debug, Default)]
pub struct Column {
    kind: ColumnKind,
    nulls: Vec<bool>,
    ints: Vec<i64>,
    reals: Vec<f64>,
    decimals: Vec<MyDecimal>,
    strings: Vec<String>,
}

impl Column {
    /// 重置 null 位图至指定长度。
    fn resize_nulls(&mut self, size: usize, is_null: bool) {
        self.nulls.clear();
        self.nulls.resize(size, is_null);
    }

    pub fn ResizeInt64(&mut self, size: usize, is_null: bool) {
        self.kind = ColumnKind::Int;
        self.resize_nulls(size, is_null);
        self.ints.clear();
        self.ints.resize(size, 0);
    }

    pub fn ResizeFloat64(&mut self, size: usize, is_null: bool) {
        self.kind = ColumnKind::Real;
        self.resize_nulls(size, is_null);
        self.reals.clear();
        self.reals.resize(size, 0.0);
    }

    pub fn ResizeDecimal(&mut self, size: usize, is_null: bool) {
        self.kind = ColumnKind::Decimal;
        self.resize_nulls(size, is_null);
        self.decimals.clear();
        self.decimals.resize(size, MyDecimal::default());
    }

    /// 为追加字符串预留容量（先清空再 reserve）。
    pub fn ReserveString(&mut self, size: usize) {
        self.kind = ColumnKind::String;
        self.nulls.clear();
        self.strings.clear();
        self.nulls.reserve(size);
        self.strings.reserve(size);
    }

    pub fn AppendNull(&mut self) {
        self.nulls.push(true);
        self.strings.push(String::new());
    }

    pub fn AppendString(&mut self, value: String) {
        self.nulls.push(false);
        self.strings.push(value);
    }

    pub fn IsNull(&self, row: usize) -> bool {
        self.nulls[row]
    }

    pub fn SetNull(&mut self, row: usize, is_null: bool) {
        self.nulls[row] = is_null;
    }

    /// 将多列 NULL 按行做逻辑或合并到本列（向量化短路用）。
    pub fn MergeNulls(&mut self, columns: &[&Column]) {
        assert!(
            matches!(
                self.kind,
                ColumnKind::Int | ColumnKind::Real | ColumnKind::Decimal
            ),
            "result column should be fixed-length type"
        );
        for column in columns {
            assert_eq!(
                self.nulls.len(),
                column.nulls.len(),
                "should ensure all columns have the same length"
            );
        }
        for row in 0..self.nulls.len() {
            self.nulls[row] |= columns.iter().any(|column| column.IsNull(row));
        }
    }

    pub fn Int64s(&self) -> &[i64] {
        &self.ints
    }
    pub fn Int64sMut(&mut self) -> &mut [i64] {
        &mut self.ints
    }
    pub fn Float64s(&self) -> &[f64] {
        &self.reals
    }
    pub fn Float64sMut(&mut self) -> &mut [f64] {
        &mut self.reals
    }
    pub fn Decimals(&self) -> &[MyDecimal] {
        &self.decimals
    }
    pub fn DecimalsMut(&mut self) -> &mut [MyDecimal] {
        &mut self.decimals
    }
    pub fn GetString(&self, row: usize) -> &str {
        &self.strings[row]
    }
    pub fn GetBytes(&self, row: usize) -> &[u8] {
        self.strings[row].as_bytes()
    }
}

/// 遗留表达式接口：标量求值 + 向量化求值 + 常量级别与类型。
pub trait LegacyExpression: Send + Sync {
    fn EvalString(&self, ctx: &EvalContext, row: Row) -> Result<Option<String>>;
    fn EvalInt(&self, ctx: &EvalContext, row: Row) -> Result<Option<i64>>;
    fn ConstLevel(&self) -> ConstLevel;
    fn VecEvalString(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()>;
    fn VecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()>;
    fn VecEvalReal(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()>;
    fn VecEvalDecimal(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()>;
    fn GetType(&self, ctx: &EvalContext) -> FieldType;
}

/// 表达式引用（Arc 动态分发）。
pub type ExprRef = Arc<dyn LegacyExpression>;

/// 字面量内部存储的各类型值向量。
#[derive(Clone, Debug)]
enum LiteralValues {
    Strings(Vec<Option<String>>),
    Ints(Vec<Option<i64>>),
    Reals(Vec<Option<f64>>),
    Decimals(Vec<Option<MyDecimal>>),
}

/// 字面量表达式：常量或按行取值的列式字面量。
#[derive(Clone, Debug)]
pub struct LiteralExpression {
    values: LiteralValues,
    field_type: FieldType,
    const_level: ConstLevel,
}

impl LiteralExpression {
    /// 严格常量字符串。
    pub fn constant_string(value: Option<&str>) -> ExprRef {
        Arc::new(Self {
            values: LiteralValues::Strings(vec![value.map(str::to_owned)]),
            field_type: FieldType::default(),
            const_level: ConstLevel::ConstStrict,
        })
    }

    /// 严格常量整型。
    pub fn constant_int(value: Option<i64>) -> ExprRef {
        Arc::new(Self {
            values: LiteralValues::Ints(vec![value]),
            field_type: FieldType::default(),
            const_level: ConstLevel::ConstStrict,
        })
    }

    /// 按行字符串列（非常量）。
    pub fn strings(values: Vec<Option<&str>>) -> ExprRef {
        Arc::new(Self {
            values: LiteralValues::Strings(
                values
                    .into_iter()
                    .map(|value| value.map(str::to_owned))
                    .collect(),
            ),
            field_type: FieldType::default(),
            const_level: ConstLevel::ConstNone,
        })
    }

    pub fn ints(values: Vec<Option<i64>>) -> ExprRef {
        Arc::new(Self {
            values: LiteralValues::Ints(values),
            field_type: FieldType::default(),
            const_level: ConstLevel::ConstNone,
        })
    }

    /// 无符号整型列：以 i64 位模式存储，字段标记 unsigned。
    pub fn uints(values: Vec<Option<u64>>) -> ExprRef {
        Arc::new(Self {
            values: LiteralValues::Ints(
                values
                    .into_iter()
                    .map(|value| value.map(|value| value as i64))
                    .collect(),
            ),
            field_type: FieldType { unsigned: true },
            const_level: ConstLevel::ConstNone,
        })
    }

    pub fn reals(values: Vec<Option<f64>>) -> ExprRef {
        Arc::new(Self {
            values: LiteralValues::Reals(values),
            field_type: FieldType::default(),
            const_level: ConstLevel::ConstNone,
        })
    }

    pub fn decimals(values: Vec<Option<MyDecimal>>) -> ExprRef {
        Arc::new(Self {
            values: LiteralValues::Decimals(values),
            field_type: FieldType::default(),
            const_level: ConstLevel::ConstNone,
        })
    }

    /// 按行取元素；长度为 1 时广播到所有行（常量列语义）。
    fn index<T>(values: &[Option<T>], row: usize) -> Option<&Option<T>> {
        values
            .get(row)
            .or_else(|| (values.len() == 1).then(|| &values[0]))
    }
}

impl LegacyExpression for LiteralExpression {
    fn EvalString(&self, _ctx: &EvalContext, row: Row) -> Result<Option<String>> {
        match &self.values {
            LiteralValues::Strings(values) => Ok(Self::index(values, row.0).cloned().flatten()),
            _ => Err(EvalError::Message("expression is not a string".into())),
        }
    }

    fn EvalInt(&self, _ctx: &EvalContext, row: Row) -> Result<Option<i64>> {
        match &self.values {
            LiteralValues::Ints(values) => Ok(Self::index(values, row.0).copied().flatten()),
            _ => Err(EvalError::Message("expression is not an integer".into())),
        }
    }

    fn ConstLevel(&self) -> ConstLevel {
        self.const_level
    }

    fn VecEvalString(&self, _ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        result.ReserveString(input.NumRows());
        for row in 0..input.NumRows() {
            match self.EvalString(&EvalContext::default(), Row(row))? {
                Some(value) => result.AppendString(value),
                None => result.AppendNull(),
            }
        }
        Ok(())
    }

    fn VecEvalInt(&self, _ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        result.ResizeInt64(input.NumRows(), false);
        let LiteralValues::Ints(values) = &self.values else {
            return Err(EvalError::Message("expression is not an integer".into()));
        };
        for row in 0..input.NumRows() {
            match Self::index(values, row).copied().flatten() {
                Some(value) => result.Int64sMut()[row] = value,
                None => result.SetNull(row, true),
            }
        }
        Ok(())
    }

    fn VecEvalReal(&self, _ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        result.ResizeFloat64(input.NumRows(), false);
        let LiteralValues::Reals(values) = &self.values else {
            return Err(EvalError::Message("expression is not a real".into()));
        };
        for row in 0..input.NumRows() {
            match Self::index(values, row).copied().flatten() {
                Some(value) => result.Float64sMut()[row] = value,
                None => result.SetNull(row, true),
            }
        }
        Ok(())
    }

    fn VecEvalDecimal(&self, _ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        result.ResizeDecimal(input.NumRows(), false);
        let LiteralValues::Decimals(values) = &self.values else {
            return Err(EvalError::Message("expression is not a decimal".into()));
        };
        for row in 0..input.NumRows() {
            match Self::index(values, row).cloned().flatten() {
                Some(value) => result.DecimalsMut()[row] = value,
                None => result.SetNull(row, true),
            }
        }
        Ok(())
    }

    fn GetType(&self, _ctx: &EvalContext) -> FieldType {
        self.field_type
    }
}

/// 数学内置函数公共字段：参数、MySQL RNG、返回 DECIMAL 精度。
#[derive(Clone)]
pub struct MathBase {
    pub args: Vec<ExprRef>,
    pub mysql_rng: Arc<MysqlRng>,
    pub ret_decimal: i32,
}

impl MathBase {
    /// 使用种子 0 构造。
    pub fn new(args: Vec<ExprRef>) -> Self {
        Self::with_seed(args, 0)
    }

    /// 指定 RAND 种子构造。
    pub fn with_seed(args: Vec<ExprRef>, seed: i64) -> Self {
        Self {
            args,
            mysql_rng: Arc::from(mathutil::NewWithSeed(seed)),
            ret_decimal: 0,
        }
    }

    /// 设置返回 DECIMAL 的小数位数。
    pub fn with_ret_decimal(mut self, decimal: i32) -> Self {
        self.ret_decimal = decimal;
        self
    }
}
