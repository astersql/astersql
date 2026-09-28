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

// Schema 比较的格（lattice）代数核心。
//
// 对应 Go `lattice.go`。格元素通过偏序 `Compare`（返回 -1/0/1 或不相容）与
// 上确界 `Join` 组合。复合结构（Tuple、Map、Maybe、StringList）由基础元素
// 组装，用于列类型与表定义的兼容性判定。

use crate::{mysql, types};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Debug};

/// 类型擦除的可克隆/可比较值，模拟 Go `any` 动态装箱。
trait DynValue: Any + Debug {
    fn as_any(&self) -> &dyn Any;
    fn clone_dyn(&self) -> Box<dyn DynValue>;
    fn eq_dyn(&self, other: &dyn DynValue) -> bool;
}

impl<T> DynValue for T
where
    T: Any + Clone + PartialEq + Debug,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_dyn(&self) -> Box<dyn DynValue> {
        Box::new(self.clone())
    }

    fn eq_dyn(&self, other: &dyn DynValue) -> bool {
        other.as_any().downcast_ref::<T>() == Some(self)
    }
}

/// 可克隆的 Go `any` 表示；`None` 对应 Go `nil`。
/// Cloneable representation of a Go `any` value. `None` represents Go `nil`.
pub struct AnyValue(Option<Box<dyn DynValue>>);

impl AnyValue {
    /// 用具体值构造非空 `AnyValue`。
    pub fn new<T>(value: T) -> Self
    where
        T: Any + Clone + PartialEq + Debug,
    {
        Self(Some(Box::new(value)))
    }

    /// 构造表示 Go `nil` 的空值。
    pub fn nil() -> Self {
        Self(None)
    }

    /// 是否为 `nil`。
    pub fn is_nil(&self) -> bool {
        self.0.is_none()
    }

    /// 尝试向下转型为具体类型引用。
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.as_ref()?.as_any().downcast_ref::<T>()
    }
}

impl Clone for AnyValue {
    fn clone(&self) -> Self {
        Self(self.0.as_ref().map(|value| value.clone_dyn()))
    }
}

impl PartialEq for AnyValue {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => a.eq_dyn(b.as_ref()),
            _ => false,
        }
    }
}

impl Debug for AnyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => Debug::fmt(value, f),
            None => f.write_str("nil"),
        }
    }
}

impl fmt::Display for AnyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_nil() {
            return f.write_str("<nil>");
        }
        macro_rules! display_primitive {
            ($($ty:ty),* $(,)?) => {$ (
                if let Some(value) = self.downcast_ref::<$ty>() {
                    return write!(f, "{}", value);
                }
            )* };
        }
        display_primitive!(
            String, bool, u8, u16, u32, u64, usize, i8, i16, i32, i64, isize
        );
        if let Some(value) = self.downcast_ref::<IncompatibleError>() {
            return write!(f, "{}", value);
        }
        write!(f, "{:?}", self)
    }
}

#[derive(Clone, PartialEq)]
/// 两个格元素不相容时的错误，携带格式化消息模板与参数。
pub struct IncompatibleError {
    /// 类似 Go `fmt` 风格的消息模板（如 `%v`、`%T`）。
    pub Msg: &'static str,
    /// 填充到消息模板中的参数。
    pub Args: Vec<AnyValue>,
}

impl IncompatibleError {
    /// 无参数的纯文本错误。
    pub fn message(msg: &'static str) -> Self {
        Self {
            Msg: msg,
            Args: vec![],
        }
    }
}

impl fmt::Display for IncompatibleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arg = |index: usize| {
            self.Args
                .get(index)
                .map(ToString::to_string)
                .unwrap_or_default()
        };
        match self.Msg {
            ErrMsgTypeMismatch => write!(f, "type mismatch ({} vs {})", arg(0), arg(1)),
            ErrMsgTupleLengthMismatch => {
                write!(f, "tuple length mismatch ({} vs {})", arg(0), arg(1))
            }
            ErrMsgDistinctSingletons => write!(f, "distinct singletons ({} vs {})", arg(0), arg(1)),
            ErrMsgIncompatibleType => {
                write!(f, "incompatible mysql type ({} vs {})", arg(0), arg(1))
            }
            ErrMsgIncompatibleCharset => {
                write!(f, "incompatible charset ({} vs {})", arg(0), arg(1))
            }
            ErrMsgIncompatibleCollation => {
                write!(f, "incompatible collation ({} vs {})", arg(0), arg(1))
            }
            ErrMsgAtTupleIndex => write!(f, "at tuple index {}: {}", arg(0), arg(1)),
            ErrMsgAtMapKey => write!(f, "at map key {:?}: {}", arg(0), arg(1)),
            ErrMsgNonInclusiveBitSets => {
                write!(f, "non-inclusive bit sets ({} vs {})", arg(0), arg(1))
            }
            ErrMsgContradictingOrders => {
                write!(
                    f,
                    "combining contradicting orders ({} && {})",
                    arg(0),
                    arg(1)
                )
            }
            ErrMsgStringListElemMismatch => write!(
                f,
                "at string list index {}: distinct values ({:?} vs {:?})",
                arg(0),
                arg(1),
                arg(2)
            ),
            message if self.Args.is_empty() => f.write_str(message),
            message => write!(f, "{}: {:?}", message, self.Args),
        }
    }
}

impl Debug for IncompatibleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for IncompatibleError {}

/// 格元素动态类型不一致。
pub const ErrMsgTypeMismatch: &str = "type mismatch (%T vs %T)";
/// 元组长度不一致。
pub const ErrMsgTupleLengthMismatch: &str = "tuple length mismatch (%d vs %d)";
/// 单点子格中出现不相等的值。
pub const ErrMsgDistinctSingletons: &str = "distinct singletons (%v vs %v)";
/// MySQL 字段类型编号无法比较/合并。
pub const ErrMsgIncompatibleType: &str = "incompatible mysql type (%v vs %v)";
/// 字符集不相容。
pub const ErrMsgIncompatibleCharset: &str = "incompatible charset (%v vs %v)";
/// 排序规则不相容。
pub const ErrMsgIncompatibleCollation: &str = "incompatible collation (%v vs %v)";
/// 包装元组某一维上的内层错误。
pub const ErrMsgAtTupleIndex: &str = "at tuple index %d: %v";
/// 包装 map 某一键上的内层错误。
pub const ErrMsgAtMapKey: &str = "at map key %q: %v";
/// 两个位集互不包含，无法形成偏序。
pub const ErrMsgNonInclusiveBitSets: &str = "non-inclusive bit sets (%#x vs %#x)";
/// 多维比较结果方向互相矛盾。
pub const ErrMsgContradictingOrders: &str = "combining contradicting orders (%d && %d)";
/// 字符串列表公共前缀位置上元素不同。
pub const ErrMsgStringListElemMismatch: &str =
    "at string list index %d: distinct values (%q vs %q)";

/// 格元素借用引用。
pub type LatticeRef<'a> = &'a dyn Lattice;
/// 堆上装箱的格元素。
pub type LatticeBox = Box<dyn Lattice>;

/// 偏序格元素：可解包、比较、求 join，并支持类型擦除。
///
/// - `Compare`：偏序比较，返回 -1/0/1 或不相容错误
/// - `Join`：求上确界；不相容时返回错误
/// - `Unwrap`：解包为底层 `AnyValue`
pub trait Lattice: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn Unwrap(&self) -> AnyValue;
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError>;
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError>;
    fn clone_box(&self) -> LatticeBox;
}

impl Clone for LatticeBox {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// 用 `TypeId` 字符串近似描述格元素的动态类型（对应 Go `%T`）。
fn lattice_type(value: LatticeRef<'_>) -> String {
    format!("{:?}", value.as_any().type_id())
}

/// 构造类型不匹配错误。
pub fn typeMismatchError(a: LatticeRef<'_>, b: LatticeRef<'_>) -> IncompatibleError {
    IncompatibleError {
        Msg: ErrMsgTypeMismatch,
        Args: vec![
            AnyValue::new(lattice_type(a)),
            AnyValue::new(lattice_type(b)),
        ],
    }
}

/// 构造元组长度不匹配错误。
fn tupleLengthMismatchError(a: usize, b: usize) -> IncompatibleError {
    IncompatibleError {
        Msg: ErrMsgTupleLengthMismatch,
        Args: vec![AnyValue::new(a), AnyValue::new(b)],
    }
}

/// 构造单点值冲突错误。
fn distinctSingletonsErrors(a: AnyValue, b: AnyValue) -> IncompatibleError {
    IncompatibleError {
        Msg: ErrMsgDistinctSingletons,
        Args: vec![a, b],
    }
}

/// 构造 MySQL 类型编号不相容错误。
fn incompatibleTypeError(a: u8, b: u8) -> IncompatibleError {
    IncompatibleError {
        Msg: ErrMsgIncompatibleType,
        Args: vec![AnyValue::new(a), AnyValue::new(b)],
    }
}

/// 构造字符集不相容错误。
pub fn incompatibleCharsetError(a: &str, b: &str) -> IncompatibleError {
    IncompatibleError {
        Msg: ErrMsgIncompatibleCharset,
        Args: vec![AnyValue::new(a.to_owned()), AnyValue::new(b.to_owned())],
    }
}

/// 构造排序规则不相容错误。
pub fn incompatibleCollationError(a: &str, b: &str) -> IncompatibleError {
    IncompatibleError {
        Msg: ErrMsgIncompatibleCollation,
        Args: vec![AnyValue::new(a.to_owned()), AnyValue::new(b.to_owned())],
    }
}

/// 为元组某一维包装内层错误。
fn wrapTupleIndexError(index: usize, inner: IncompatibleError) -> IncompatibleError {
    IncompatibleError {
        Msg: ErrMsgAtTupleIndex,
        Args: vec![AnyValue::new(index), AnyValue::new(inner)],
    }
}

/// 为 map 某一键包装内层错误。
fn wrapMapKeyError(key: String, inner: IncompatibleError) -> IncompatibleError {
    IncompatibleError {
        Msg: ErrMsgAtMapKey,
        Args: vec![AnyValue::new(key), AnyValue::new(inner)],
    }
}

/// 为全序数值类型生成格实现：`Compare` 用 `cmp`，`Join` 取 `max`。
macro_rules! ordered_lattice {
    ($name:ident, $inner:ty) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name(pub $inner);
        impl Lattice for $name {
            fn as_any(&self) -> &dyn Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn Any {
                self
            }
            fn Unwrap(&self) -> AnyValue {
                AnyValue::new(self.0)
            }
            fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
                let Some(other) = other.as_any().downcast_ref::<Self>() else {
                    return Err(typeMismatchError(self, other));
                };
                Ok(match self.0.cmp(&other.0) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                })
            }
            fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
                let Some(other) = other.as_any().downcast_ref::<Self>() else {
                    return Err(typeMismatchError(self, other));
                };
                Ok(Box::new(Self(self.0.max(other.0))))
            }
            fn clone_box(&self) -> LatticeBox {
                Box::new(*self)
            }
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 布尔格：`false < true`，join 为逻辑或。
pub struct Bool(pub bool);

impl Lattice for Bool {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(self.0)
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        Ok(if self.0 == other.0 {
            0
        } else if self.0 {
            1
        } else {
            -1
        })
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        Ok(Box::new(Self(self.0 || other.0)))
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(*self)
    }
}

#[derive(Clone)]
/// 单点子格：仅相等时可比较/join，否则报 distinct singletons。
struct singleton(AnyValue);

impl Lattice for singleton {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        self.0.clone()
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        if self.0 != other.0 {
            Err(distinctSingletonsErrors(self.0.clone(), other.0.clone()))
        } else {
            Ok(0)
        }
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        self.Compare(other)?;
        Ok(self.clone_box())
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(self.clone())
    }
}

/// 将任意可比较值包装为单点格元素。
pub fn Singleton<T>(value: T) -> LatticeBox
where
    T: Any + Clone + PartialEq + Debug,
{
    Box::new(singleton(AnyValue::new(value)))
}

/// 自定义相等语义，供 `EqualitySingleton` 使用（不必依赖 `PartialEq` 跨类型）。
pub trait Equality: Any + Debug {
    fn as_any(&self) -> &dyn Any;
    fn Equals(&self, other: &dyn Equality) -> bool;
}

#[derive(Clone)]
/// 基于 `Equality` trait 的单点格。
struct equalitySingleton<T: Equality + Clone + PartialEq>(T);

impl<T: Equality + Clone + PartialEq> Lattice for equalitySingleton<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(self.0.clone())
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        if self.0.Equals(&other.0) {
            Ok(0)
        } else {
            Err(distinctSingletonsErrors(
                AnyValue::new(self.0.clone()),
                AnyValue::new(other.0.clone()),
            ))
        }
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        self.Compare(other)?;
        Ok(self.clone_box())
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(self.clone())
    }
}

/// 用实现了 `Equality` 的值构造单点格。
pub fn EqualitySingleton<T>(value: T) -> LatticeBox
where
    T: Equality + Clone + PartialEq,
{
    Box::new(equalitySingleton(value))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 位集格：按集合包含关系比较，join 为按位或；互不包含则不相容。
pub struct BitSet(pub usize);

impl Lattice for BitSet {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(self.0)
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        if self.0 == other.0 {
            Ok(0)
        } else if self.0 & !other.0 == 0 {
            Ok(-1)
        } else if other.0 & !self.0 == 0 {
            Ok(1)
        } else {
            Err(IncompatibleError {
                Msg: ErrMsgNonInclusiveBitSets,
                Args: vec![AnyValue::new(self.0), AnyValue::new(other.0)],
            })
        }
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        Ok(Box::new(Self(self.0 | other.0)))
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(*self)
    }
}

ordered_lattice!(Byte, u8);
ordered_lattice!(Int, isize);
ordered_lattice!(Int64, i64);
ordered_lattice!(Uint, usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// MySQL 字段类型编号格：整数族与 BLOB 族分别有宽度序，跨族不相容。
struct fieldTp(u8);

/// 按 Tiny < Short < Int24 < Long < Longlong 的语义序比较整数类型。
fn compareMySQLIntegerType(a: u8, b: u8) -> i32 {
    let rank = |value| match value {
        mysql::TypeTiny => 0,
        mysql::TypeShort => 1,
        mysql::TypeInt24 => 2,
        mysql::TypeLong => 3,
        mysql::TypeLonglong => 4,
        _ => -1,
    };
    rank(a).cmp(&rank(b)) as i32
}

/// 按 TinyBlob < Blob < MediumBlob < LongBlob 的语义序比较 BLOB 类型。
fn compareMySQLBlobType(a: u8, b: u8) -> i32 {
    let rank = |value| match value {
        mysql::TypeTinyBlob => 0,
        mysql::TypeBlob => 1,
        mysql::TypeMediumBlob => 2,
        mysql::TypeLongBlob => 3,
        _ => -1,
    };
    rank(a).cmp(&rank(b)) as i32
}

impl Lattice for fieldTp {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(self.0)
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        if self.0 == other.0 {
            return Ok(0);
        }
        if mysql::IsIntegerType(self.0) && mysql::IsIntegerType(other.0) {
            return Ok(compareMySQLIntegerType(self.0, other.0));
        }
        if types::IsTypeBlob(self.0) && types::IsTypeBlob(other.0) {
            return Ok(compareMySQLBlobType(self.0, other.0));
        }
        Err(incompatibleTypeError(self.0, other.0))
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        let cmp = self.Compare(other)?;
        Ok(if cmp >= 0 {
            self.clone_box()
        } else {
            other.clone_box()
        })
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(*self)
    }
}

/// 构造字段类型编号格元素。
pub fn FieldTp(value: u8) -> LatticeBox {
    Box::new(fieldTp(value))
}

#[derive(Clone)]
/// 元组格：逐维 `Compare`/`Join`，并用 `CombineCompareResult` 合并比较方向。
pub struct Tuple(pub Vec<LatticeBox>);

impl std::ops::Index<usize> for Tuple {
    type Output = LatticeBox;
    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl std::ops::IndexMut<usize> for Tuple {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl Lattice for Tuple {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(
            self.0
                .iter()
                .map(|value| value.Unwrap())
                .collect::<Vec<_>>(),
        )
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        if self.0.len() != other.0.len() {
            return Err(tupleLengthMismatchError(self.0.len(), other.0.len()));
        }
        // 逐维比较并合并方向；任一维方向矛盾则整体不相容。
        let mut result = 0;
        for (index, left) in self.0.iter().enumerate() {
            let next = left
                .Compare(other.0[index].as_ref())
                .map_err(|error| wrapTupleIndexError(index, error))?;
            result = CombineCompareResult(result, next)
                .map_err(|error| wrapTupleIndexError(index, error))?;
        }
        Ok(result)
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        if self.0.len() != other.0.len() {
            return Err(tupleLengthMismatchError(self.0.len(), other.0.len()));
        }
        let values = self
            .0
            .iter()
            .zip(&other.0)
            .enumerate()
            .map(|(index, (left, right))| {
                left.Join(right.as_ref())
                    .map_err(|error| wrapTupleIndexError(index, error))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Box::new(Self(values)))
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(self.clone())
    }
}

/// 合并两次比较结果；方向矛盾时返回 `ErrMsgContradictingOrders`。
pub fn CombineCompareResult(x: i32, y: i32) -> Result<i32, IncompatibleError> {
    if x == y || y == 0 {
        Ok(x)
    } else if x == 0 {
        Ok(y)
    } else {
        Err(IncompatibleError {
            Msg: ErrMsgContradictingOrders,
            Args: vec![AnyValue::new(x), AnyValue::new(y)],
        })
    }
}

#[derive(Clone)]
/// 可选值格：`None` 小于 `Some`；两侧皆有值时委托内层。
struct maybe(Option<LatticeBox>);

impl Lattice for maybe {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        self.0
            .as_ref()
            .map_or_else(AnyValue::nil, |value| value.Unwrap())
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        match (&self.0, &other.0) {
            (None, None) => Ok(0),
            (None, Some(_)) => Ok(-1),
            (Some(_), None) => Ok(1),
            (Some(a), Some(b)) => a.Compare(b.as_ref()),
        }
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        match (&self.0, &other.0) {
            (None, _) => Ok(other.clone_box()),
            (_, None) => Ok(self.clone_box()),
            (Some(a), Some(b)) => Ok(Box::new(Self(Some(a.Join(b.as_ref())?)))),
        }
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(self.clone())
    }
}

/// 构造可选格元素。
pub fn Maybe(inner: Option<LatticeBox>) -> LatticeBox {
    Box::new(maybe(inner))
}

/// `Option<T>` → `Maybe(Singleton)` 的便捷构造。
pub fn MaybeSingletonInterface<T>(value: Option<T>) -> LatticeBox
where
    T: Any + Clone + PartialEq + Debug,
{
    Maybe(value.map(Singleton))
}

/// 空串映射为 `Maybe(None)`，非空映射为 `Maybe(Singleton)`。
pub fn MaybeSingletonString(value: String) -> LatticeBox {
    if value.is_empty() {
        Maybe(None)
    } else {
        Maybe(Some(Singleton(value)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 字符串列表格：公共前缀必须相等，更长的列表更大；join 取较长者。
pub struct StringList(pub Vec<String>);

impl Lattice for StringList {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(self.0.clone())
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        for index in 0..self.0.len().min(other.0.len()) {
            if self.0[index] != other.0[index] {
                return Err(IncompatibleError {
                    Msg: ErrMsgStringListElemMismatch,
                    Args: vec![
                        AnyValue::new(index),
                        AnyValue::new(self.0[index].clone()),
                        AnyValue::new(other.0[index].clone()),
                    ],
                });
            }
        }
        Ok(self.0.len().cmp(&other.0.len()) as i32)
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let cmp = self.Compare(other)?;
        Ok(if cmp <= 0 {
            other.clone_box()
        } else {
            self.clone_box()
        })
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(self.clone())
    }
}

/// 键到格元素的 map 抽象，供列/索引集合编码；定义与缺失键（nil）的比较/join 策略。
pub trait LatticeMap: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn New(&self) -> Box<dyn LatticeMap>;
    fn Insert(&mut self, key: String, value: LatticeBox);
    fn Get(&self, key: &str) -> Option<LatticeRef<'_>>;
    fn ForEach(
        &self,
        f: &mut dyn FnMut(&str, LatticeRef<'_>) -> Result<(), IncompatibleError>,
    ) -> Result<(), IncompatibleError>;
    fn CompareWithNil(&self, value: LatticeRef<'_>) -> Result<i32, IncompatibleError>;
    fn JoinWithNil(&self, value: LatticeRef<'_>) -> Result<Option<LatticeBox>, IncompatibleError>;
    fn ShouldDeleteIncompatibleJoin(&self) -> bool;
    fn clone_box(&self) -> Box<dyn LatticeMap>;
}

impl Clone for Box<dyn LatticeMap> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Clone)]
/// `LatticeMap` 的格包装：对并集上的键逐个比较/join。
pub(crate) struct latticeMap {
    pub(crate) inner: Box<dyn LatticeMap>,
}

impl latticeMap {
    /// 遍历两侧键的并集，对每个键回调左右值（可能一侧为 `None`）。
    fn iter(
        &self,
        other: &Self,
        action: &mut dyn FnMut(
            String,
            Option<LatticeRef<'_>>,
            Option<LatticeRef<'_>>,
        ) -> Result<(), IncompatibleError>,
    ) -> Result<(), IncompatibleError> {
        // 先遍历左侧键，再补齐右侧独有键，形成键的并集。
        let mut visited = HashSet::new();
        self.inner.ForEach(&mut |key, value| {
            visited.insert(key.to_owned());
            action(key.to_owned(), Some(value), other.inner.Get(key))
        })?;
        other.inner.ForEach(&mut |key, value| {
            if visited.contains(key) {
                Ok(())
            } else {
                action(key.to_owned(), None, Some(value))
            }
        })
    }
}

impl Lattice for latticeMap {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        let mut result = HashMap::new();
        let _ = self.inner.ForEach(&mut |key, value| {
            result.insert(key.to_owned(), value.Unwrap());
            Ok(())
        });
        AnyValue::new(result)
    }
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        let mut result = 0;
        self.iter(other, &mut |key, left, right| {
            let next = match (left, right) {
                (Some(a), Some(b)) => a.Compare(b),
                (Some(a), None) => self.inner.CompareWithNil(a),
                (None, Some(b)) => self.inner.CompareWithNil(b).map(|value| -value),
                (None, None) => Ok(0),
            }
            .map_err(|error| wrapMapKeyError(key.clone(), error))?;
            result =
                CombineCompareResult(result, next).map_err(|error| wrapMapKeyError(key, error))?;
            Ok(())
        })?;
        Ok(result)
    }
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        let mut result = self.inner.New();
        self.iter(other, &mut |key, left, right| {
            let joined = match (left, right) {
                (Some(a), Some(b)) => a.Join(b).map(Some),
                (Some(a), None) => self.inner.JoinWithNil(a),
                (None, Some(b)) => self.inner.JoinWithNil(b),
                (None, None) => Ok(None),
            };
            match joined {
                Ok(Some(value)) => {
                    result.Insert(key, value);
                    Ok(())
                }
                Ok(None) => Ok(()),
                // 索引类 map 在不相容时丢弃该键；列 map 则向上抛错。
                Err(_) if self.inner.ShouldDeleteIncompatibleJoin() => Ok(()),
                Err(error) => Err(wrapMapKeyError(key, error)),
            }
        })?;
        Ok(Box::new(Self { inner: result }))
    }
    fn clone_box(&self) -> LatticeBox {
        Box::new(self.clone())
    }
}

/// 将具体 `LatticeMap` 装箱为可参与 `Compare`/`Join` 的格元素。
pub fn Map(map: Box<dyn LatticeMap>) -> LatticeBox {
    Box::new(latticeMap { inner: map })
}
