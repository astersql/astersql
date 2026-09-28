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

// 列类型（`FieldType`）的格编解码与 join。
//
// 对应 Go `type.go`。将 MySQL 字段类型拆成元组格（类型编号、长度、小数位、
// 标志位、可空、反键标志、默认值标志、排序规则、枚举元素），再通过 `typ`
// 实现 `Lattice`，用于 schema 合并时求列类型上确界。

use crate::{
    AnyValue, Bool, Byte, Charset, Collation, FieldTp, IncompatibleError, Int, Lattice, LatticeBox,
    LatticeRef, Maybe, Singleton, StringList, Tuple, charset, model, mysql, typeMismatchError,
    types,
};

/// 主键/唯一/普通索引相关标志掩码。
const flagMaskKeys: usize = mysql::PriKeyFlag | mysql::UniqueKeyFlag | mysql::MultipleKeyFlag;
/// 自增与“无默认值”标志掩码。
const flagMaskDefVal: usize = mysql::AutoIncrementFlag | mysql::NoDefaultValueFlag;
/// 反键编码后表示“不属于任何键”的哨兵值。
const notPartOfKeys: u8 = !0u8;

// 下列常量是 FieldType 编码到 Tuple 时各维的下标。
/// 类型编号维。
const fieldTypeTupleIndexTp: usize = 0;
/// 显示长度 / flen 维。
const fieldTypeTupleIndexFlen: usize = 1;
/// 小数位维。
const fieldTypeTupleIndexDec: usize = 2;
/// 其余需精确相等的标志位（单点）维。
const fieldTypeTupleIndexFlagSingleton: usize = 3;
/// 可空性维（true 表示允许 NULL）。
const fieldTypeTupleIndexFlagNull: usize = 4;
/// 反键标志维（键标志取反编码，便于 join 取“更宽松”方向）。
const fieldTypeTupleIndexFlagAntiKeys: usize = 5;
/// 默认值相关标志维。
const fieldTypeTupleIndexFlagDefVal: usize = 6;
/// 排序规则维。
const fieldTypeTupleIndexCollate: usize = 7;
/// ENUM/SET 元素列表维。
const fieldTypeTupleIndexElems: usize = 8;

/// AUTO_INCREMENT 列未作为键时的错误消息。
pub const ErrMsgAutoTypeWithoutKey: &str = "auto type but not defined as a key";

/// 将键相关标志取反并反转位序，使 join 偏向“更少键约束”。
fn encodeAntiKeys(flag: usize) -> u8 {
    !((flag & flagMaskKeys) as u8).reverse_bits()
}

/// 反解 `encodeAntiKeys` 的结果，还原键标志。
fn decodeAntiKeys(encoded: u8) -> usize {
    (!encoded).reverse_bits() as usize
}

/// 把 `FieldType` 编码为多维元组格。
fn encodeFieldTypeToLattice(ft: &types::FieldType) -> Tuple {
    let (flen, decimal): (LatticeBox, LatticeBox) = if ft.GetType() == mysql::TypeNewDecimal {
        (Singleton(ft.GetFlen()), Singleton(ft.GetDecimal()))
    } else {
        (Box::new(Int(ft.GetFlen())), Box::new(Int(ft.GetDecimal())))
    };

    let default_flags = if mysql::HasAutoIncrementFlag(ft.GetFlag())
        || !mysql::HasNoDefaultValueFlag(ft.GetFlag())
    {
        Maybe(Some(Singleton(ft.GetFlag() & flagMaskDefVal)))
    } else {
        Maybe(None)
    };

    Tuple(vec![
        FieldTp(ft.GetType()),
        flen,
        decimal,
        Singleton(ft.GetFlag() & !(flagMaskDefVal | mysql::NotNullFlag | flagMaskKeys)),
        Box::new(Bool(!mysql::HasNotNullFlag(ft.GetFlag()))),
        Box::new(Byte(encodeAntiKeys(ft.GetFlag()))),
        default_flags,
        Box::new(Collation(ft.GetCollate())),
        Box::new(StringList(ft.GetElems().to_vec())),
    ])
}

/// 从解包后的元组值中按下标取出具体类型。
fn downcast_value<T: 'static>(values: &[AnyValue], index: usize) -> &T {
    values[index]
        .downcast_ref::<T>()
        .expect("schemacmp lattice value has the Go-compatible type")
}

/// 从元组格还原 `FieldType`（含标志位与 charset/collate）。
fn decodeFieldTypeFromLattice(tuple: &Tuple, elems_present: bool) -> types::FieldType {
    let unwrapped = tuple.Unwrap();
    let values = unwrapped
        .downcast_ref::<Vec<AnyValue>>()
        .expect("Tuple.Unwrap must return Vec<AnyValue>");

    let mut flags = *downcast_value::<usize>(&values, fieldTypeTupleIndexFlagSingleton);
    flags |= decodeAntiKeys(*downcast_value::<u8>(
        &values,
        fieldTypeTupleIndexFlagAntiKeys,
    ));
    if !*downcast_value::<bool>(&values, fieldTypeTupleIndexFlagNull) {
        flags |= mysql::NotNullFlag;
    }
    if let Some(default_flags) = values[fieldTypeTupleIndexFlagDefVal].downcast_ref::<usize>() {
        flags |= *default_flags;
    } else {
        flags |= mysql::NoDefaultValueFlag;
    }

    let collate = downcast_value::<String>(&values, fieldTypeTupleIndexCollate).clone();
    let charset_name = collate
        .split_once('_')
        .map_or(collate.as_str(), |(charset_name, _)| {
            if charset_name.is_empty() {
                collate.as_str()
            } else {
                charset_name
            }
        })
        .to_lowercase();
    let charset_name = Charset(charset_name).value;

    let mut builder = types::NewFieldTypeBuilder();
    builder
        .SetType(*downcast_value::<u8>(&values, fieldTypeTupleIndexTp))
        .SetFlen(*downcast_value::<isize>(&values, fieldTypeTupleIndexFlen))
        .SetDecimal(*downcast_value::<isize>(&values, fieldTypeTupleIndexDec))
        .SetFlag(flags)
        .SetCharset(charset_name)
        .SetCollate(collate);
    let elems = downcast_value::<Vec<String>>(&values, fieldTypeTupleIndexElems);
    if elems_present {
        builder.SetElems(elems.clone());
    }
    builder.Build()
}

#[derive(Clone)]
/// 参与格运算的列类型包装，内部持规范化后的 `FieldType`。
pub struct typ {
    field_type: types::FieldType,
}

/// 经编解码 round-trip 构造规范化的 `typ`。
pub fn Type(ft: &types::FieldType) -> typ {
    let encoded = encodeFieldTypeToLattice(ft);
    typ {
        field_type: decodeFieldTypeFromLattice(&encoded, ft.GetElemsOption().is_some()),
    }
}

impl typ {
    /// 是否具备默认值（自增或未标 NoDefaultValue）。
    pub fn hasDefault(&self) -> bool {
        mysql::HasAutoIncrementFlag(self.field_type.GetFlag())
            || !mysql::HasNoDefaultValueFlag(self.field_type.GetFlag())
    }

    /// 列在另一侧缺失时调整标志：去掉键标志与 NoDefaultValue；返回原先是否无默认值。
    pub fn setFlagForMissingColumn(&mut self) -> bool {
        let mut flags = self.field_type.GetFlag() & !flagMaskKeys;
        let had_no_default = mysql::HasNoDefaultValueFlag(flags);
        flags &= !mysql::NoDefaultValueFlag;
        self.field_type.SetFlag(flags);
        had_no_default
    }

    /// 是否 NOT NULL。
    pub fn isNotNull(&self) -> bool {
        mysql::HasNotNullFlag(self.field_type.GetFlag())
    }

    /// 是否 AUTO_INCREMENT。
    pub(crate) fn inAutoIncrement(&self) -> bool {
        mysql::HasAutoIncrementFlag(self.field_type.GetFlag())
    }

    /// 用索引推导出的键标志覆盖列上的键相关 flag。
    pub fn setAntiKeyFlags(&mut self, flag: usize) {
        self.field_type
            .SetFlag((self.field_type.GetFlag() & !flagMaskKeys) | (flag & flagMaskKeys));
    }

    /// 按类型给出缺失列补齐时的标准默认值字符串。
    pub fn getStandardDefaultValue(&self) -> Box<dyn std::any::Any> {
        let decimal = self.field_type.GetDecimal();
        let tail = if decimal > 0 {
            format!(".{}", "0".repeat(decimal as usize))
        } else {
            String::new()
        };

        match self.field_type.GetType() {
            mysql::TypeTiny
            | mysql::TypeInt24
            | mysql::TypeShort
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeFloat
            | mysql::TypeDouble
            | mysql::TypeNewDecimal => Box::new("0".to_owned()),
            mysql::TypeTimestamp | mysql::TypeDatetime => {
                Box::new(format!("0000-00-00 00:00:00{tail}"))
            }
            mysql::TypeDate => Box::new("0000-00-00".to_owned()),
            mysql::TypeDuration => Box::new(format!("00:00:00{tail}")),
            mysql::TypeYear => Box::new("0000".to_owned()),
            mysql::TypeJSON => Box::new("null".to_owned()),
            mysql::TypeTiDBVectorFloat32 => Box::new("[]".to_owned()),
            mysql::TypeEnum => Box::new(
                self.field_type
                    .GetElems()
                    .first()
                    .cloned()
                    .expect("ENUM standard default requires at least one element"),
            ),
            mysql::TypeString if self.field_type.GetCollate() == charset::CollationBin => {
                Box::new(String::from_utf8(vec![0; self.field_type.GetFlen() as usize]).unwrap())
            }
            _ => Box::new(String::new()),
        }
    }

    /// 将 Go 的标准默认值语义映射到 Rust `ColumnInfo` 的带类型表示。
    pub(crate) fn getStandardDefaultModelValue(&self) -> model::DefaultValue {
        match self.field_type.GetType() {
            mysql::TypeTiny
            | mysql::TypeInt24
            | mysql::TypeShort
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeFloat
            | mysql::TypeDouble
            | mysql::TypeNewDecimal => model::DefaultValue::Int(0),
            _ => {
                let value = *self
                    .getStandardDefaultValue()
                    .downcast::<String>()
                    .expect("standard default value is represented as a string");
                model::DefaultValue::String(value.into_bytes())
            }
        }
    }
}

impl Lattice for typ {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(self.field_type.clone())
    }

    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<typ>() else {
            return Err(typeMismatchError(self, other));
        };
        encodeFieldTypeToLattice(&self.field_type)
            .Compare(&encodeFieldTypeToLattice(&other.field_type))
    }

    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<typ>() else {
            return Err(typeMismatchError(self, other));
        };
        let joined = encodeFieldTypeToLattice(&self.field_type)
            .Join(&encodeFieldTypeToLattice(&other.field_type))?;
        let joined = joined
            .as_any()
            .downcast_ref::<Tuple>()
            .expect("Tuple.Join must return Tuple");
        // Go's StringList keeps nil/non-nil slice identity when Join returns one
        // operand. Preserve the corresponding Option state in Rust as well.
        let elems_present = if self.field_type.GetElems().len() <= other.field_type.GetElems().len()
        {
            other.field_type.GetElemsOption().is_some()
        } else {
            self.field_type.GetElemsOption().is_some()
        };
        let field_type = decodeFieldTypeFromLattice(joined, elems_present);

        // AUTO_INCREMENT 必须属于某键；反键全 1 表示不属于任何键。
        if mysql::HasAutoIncrementFlag(field_type.GetFlag())
            && encodeAntiKeys(field_type.GetFlag()) == notPartOfKeys
        {
            return Err(IncompatibleError {
                Msg: ErrMsgAutoTypeWithoutKey,
                Args: vec![],
            });
        }

        Ok(Box::new(typ { field_type }))
    }

    fn clone_box(&self) -> LatticeBox {
        Box::new(self.clone())
    }
}
