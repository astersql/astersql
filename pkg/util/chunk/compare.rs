// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Row 列值比较、Datum 对 Row 比较，以及有序 Chunk 上的上下界二分查找。
//
// 对应 Go `pkg/util/chunk/compare.go`。比较结果用 `i32`：负值表示左小、0 相等、正值左大；
// NULL 排序与 Go 一致（两 NULL 相等，NULL 小于非 NULL）。`LowerBound`/`UpperBound` 假设目标列非递减。

// chunk.Row 中各类列值的比较、Datum 对 Row 的比较，以及 Chunk 的边界查找。

use std::cmp::Ordering;

// CompareFunc is a function to compare the two values in Row, the two columns must have the same type.
// CompareFunc 对应 Go 的函数类型；Go 可以返回 nil，这里由 GetCompareFunc 外层 Option 表达。
/// 两行同类型列值的比较闭包；返回值语义同 Go 的 `cmp`（-1/0/1）。
pub type CompareFunc = Box<dyn Fn(Row, usize, Row, usize) -> i32 + Send + Sync>;

/// 将 `Ordering` 映射为 Go 风格的 -1/0/1。
fn ordering_to_i32(ord: Ordering) -> i32 {
    match ord {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

fn cmp_i64(left: i64, right: i64) -> i32 {
    ordering_to_i32(left.cmp(&right))
}

fn cmp_u64(left: u64, right: u64) -> i32 {
    ordering_to_i32(left.cmp(&right))
}

/// 浮点比较：NaN 小于任意非 NaN，两 NaN 视为相等（对齐 Go `cmp.Compare`）。
fn cmp_f64(left: f64, right: f64) -> i32 {
    // cmp.Compare orders NaN before every non-NaN value and considers two NaNs equal.
    if left < right || (left.is_nan() && !right.is_nan()) {
        -1
    } else if left > right || (!left.is_nan() && right.is_nan()) {
        1
    } else {
        0
    }
}

// GetCompareFunc gets a compare function for the field type.
// GetCompareFunc 根据 FieldType 类型选择 Row 列比较函数；未知类型保持 Go 的 nil 返回语义。
/// 按列 `FieldType`（含无符号标志与 collation）选取比较函数；未知类型返回 `None`。
pub fn GetCompareFunc(tp: &types::FieldType) -> Option<CompareFunc> {
    match tp.GetType() {
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeYear => {
            if mysql::HasUnsignedFlag(tp.GetFlag()) {
                return Some(Box::new(cmpUint64));
            }
            Some(Box::new(cmpInt64))
        }
        mysql::TypeFloat => Some(Box::new(cmpFloat32)),
        mysql::TypeDouble => Some(Box::new(cmpFloat64)),
        mysql::TypeString
        | mysql::TypeVarString
        | mysql::TypeVarchar
        | mysql::TypeBlob
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob => Some(genCmpStringFunc(tp.GetCollate().to_owned())),
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => Some(Box::new(cmpTime)),
        mysql::TypeDuration => Some(Box::new(cmpDuration)),
        mysql::TypeNewDecimal => Some(Box::new(cmpMyDecimal)),
        mysql::TypeSet | mysql::TypeEnum => Some(Box::new(cmpNameValue)),
        mysql::TypeBit => Some(Box::new(cmpBit)),
        mysql::TypeJSON => Some(Box::new(cmpJSON)),
        mysql::TypeTiDBVectorFloat32 => Some(Box::new(cmpVectorFloat32)),
        mysql::TypeNull => Some(Box::new(cmpNullConst)),
        _ => None,
    }
}

// cmpNull 对应 Go 的 NULL 排序规则：两个 NULL 相等，左 NULL 更小，右 NULL 更大。
fn cmpNull(lNull: bool, rNull: bool) -> i32 {
    if lNull && rNull {
        return 0;
    }
    if lNull {
        return -1;
    }
    1
}

// cmpInt64 对应 Go 的有符号整数列比较，先处理 NULL，再比较实际 int64 值。
fn cmpInt64(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    cmp_i64(l.GetInt64(lCol), r.GetInt64(rCol))
}

// cmpUint64 对应 Go 的无符号整数列比较。
fn cmpUint64(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    cmp_u64(l.GetUint64(lCol), r.GetUint64(rCol))
}

// genCmpStringFunc 对应 Go 的闭包生成器，把 collation 捕获到字符串比较函数里。
fn genCmpStringFunc(collation: String) -> CompareFunc {
    Box::new(move |l: Row, lCol: usize, r: Row, rCol: usize| {
        cmpStringWithCollationInfo(l, lCol, r, rCol, &collation)
    })
}

// cmpStringWithCollationInfo 保留 types.CompareString 的调用形状，排序规则由 FieldType 提供。
fn cmpStringWithCollationInfo(l: Row, lCol: usize, r: Row, rCol: usize, collation: &str) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    types::CompareString(&l.GetString(lCol), &r.GetString(rCol), collation)
}

// cmpFloat32 对应 Go 中 float32 先转 float64 再调用 cmp.Compare 的逻辑。
fn cmpFloat32(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    cmp_f64(l.GetFloat32(lCol) as f64, r.GetFloat32(rCol) as f64)
}

// cmpFloat64 对应 Go 的 float64 列比较。
fn cmpFloat64(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    cmp_f64(l.GetFloat64(lCol), r.GetFloat64(rCol))
}

// cmpMyDecimal 对应 Go 的 MyDecimal.Compare 调用。
fn cmpMyDecimal(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    let (lDec, rDec) = (l.GetMyDecimal(lCol), r.GetMyDecimal(rCol));
    lDec.Compare(&rDec) as i32
}

// cmpTime 对应 Go 的 Time.Compare 调用。
fn cmpTime(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    let (lTime, rTime) = (l.GetTime(lCol), r.GetTime(rCol));
    lTime.Compare(rTime)
}

// cmpDuration 对应 Go 的 Duration 字段比较；fsp 参数在原实现中固定传 0。
fn cmpDuration(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    let (lDur, rDur) = (
        l.GetDuration(lCol, 0).Duration,
        r.GetDuration(rCol, 0).Duration,
    );
    cmp_i64(lDur as i64, rDur as i64)
}

// cmpNameValue 对应 Go 中 Enum/Set 的 name,value 读取；比较只使用 value。
fn cmpNameValue(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    let (_, lVal) = l.getNameValue(lCol);
    let (_, rVal) = r.getNameValue(rCol);
    cmp_u64(lVal, rVal)
}

// cmpBit 对应 Go 的 BinaryLiteral 比较，先把原始 bytes 包成 MySQL bit 字面量。
fn cmpBit(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    let lBit = types::BinaryLiteral(l.GetBytes(lCol));
    let rBit = types::BinaryLiteral(r.GetBytes(rCol));
    lBit.Compare(rBit)
}

// cmpJSON 对应 Go 的 BinaryJSON 比较入口。
fn cmpJSON(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    let (lJ, rJ) = (l.GetJSON(lCol), r.GetJSON(rCol));
    types::CompareBinaryJSON(&lJ, &rJ)
}

// cmpNullConst 对应 TypeNull 的常量比较函数；所有输入都视为相等。
fn cmpNullConst(_l: Row, _lCol: usize, _r: Row, _rCol: usize) -> i32 {
    0
}

// cmpVectorFloat32 对应 Go 的 VectorFloat32.Compare 调用。
fn cmpVectorFloat32(l: Row, lCol: usize, r: Row, rCol: usize) -> i32 {
    let (lNull, rNull) = (l.IsNull(lCol), r.IsNull(rCol));
    if lNull || rNull {
        return cmpNull(lNull, rNull);
    }
    let (lv, rv) = (l.GetVectorFloat32(lCol), r.GetVectorFloat32(rCol));
    lv.Compare(&rv)
}

// Compare compares the value with ad.
// We assume that the collation information of the column is the same with the datum.
// Compare 对应 Go 的 Row 值与 Datum 比较；Datum 的特殊 Kind 保持原 switch 顺序。
/// 将一行中某列的值与 `Datum` 比较；`KindMinNotNull`/`KindMaxValue` 等哨兵按 Go 语义处理。
pub fn Compare(row: Row, colIdx: usize, ad: &types::Datum) -> i32 {
    match ad.Kind() {
        types::KindNull => {
            if row.IsNull(colIdx) {
                return 0;
            }
            1
        }
        types::KindMinNotNull => {
            if row.IsNull(colIdx) {
                return -1;
            }
            1
        }
        types::KindMaxValue => -1,
        types::KindInt64 => cmp_i64(row.GetInt64(colIdx), ad.GetInt64()),
        types::KindUint64 => cmp_u64(row.GetUint64(colIdx), ad.GetUint64()),
        types::KindFloat32 => cmp_f64(row.GetFloat32(colIdx) as f64, ad.GetFloat32() as f64),
        types::KindFloat64 => cmp_f64(row.GetFloat64(colIdx), ad.GetFloat64()),
        types::KindString => {
            types::CompareString(&row.GetString(colIdx), &ad.GetString(), &ad.Collation())
        }
        types::KindBytes | types::KindBinaryLiteral | types::KindMysqlBit => {
            ordering_to_i32(row.GetBytes(colIdx).cmp(&ad.GetBytes()))
        }
        types::KindMysqlDecimal => {
            let (l, r) = (row.GetMyDecimal(colIdx), ad.GetMysqlDecimal());
            l.Compare(&r) as i32
        }
        types::KindMysqlDuration => {
            let (l, r) = (
                row.GetDuration(colIdx, 0).Duration,
                ad.GetMysqlDuration().Duration,
            );
            cmp_i64(l as i64, r as i64)
        }
        types::KindMysqlEnum => {
            let (l, r) = (row.GetEnum(colIdx).Value, ad.GetMysqlEnum().Value);
            cmp_u64(l, r)
        }
        types::KindMysqlSet => {
            let (l, r) = (row.GetSet(colIdx).Value, ad.GetMysqlSet().Value);
            cmp_u64(l, r)
        }
        types::KindMysqlJSON => {
            let (l, r) = (row.GetJSON(colIdx), ad.GetMysqlJSON());
            types::CompareBinaryJSON(&l, &r)
        }
        types::KindVectorFloat32 => {
            let (l, r) = (row.GetVectorFloat32(colIdx), ad.GetVectorFloat32());
            l.Compare(&r)
        }
        types::KindMysqlTime => {
            let (l, r) = (row.GetTime(colIdx), ad.GetMysqlTime());
            l.Compare(r)
        }
        _ => 0,
    }
}

impl Chunk {
    // LowerBound searches on the non-decreasing Column colIdx,
    // returns the smallest index i such that the value at row i is not less than `d`.
    // LowerBound 对应 Go 的 sort.Search 版本；match 在闭包中被更新，因此这里显式记录命中状态。
    /// 在非递减列上求下界：最小的 `i` 使得行 `i` 的列值不小于 `d`；第二返回值表示是否精确命中。
    pub fn LowerBound(&self, colIdx: usize, d: &types::Datum) -> (usize, bool) {
        let num_rows = self.NumRows();
        // 末行仍小于 d 时，插入点在末尾且无精确匹配。
        if Compare(self.GetRow(num_rows - 1), colIdx, d) < 0 {
            return (num_rows, false);
        }

        let mut index = 0;
        let mut hi = num_rows;
        let mut found_match = false;
        while index < hi {
            let i = index + (hi - index) / 2;
            let cmp = Compare(self.GetRow(i), colIdx, d);
            if cmp == 0 {
                found_match = true;
            }
            if cmp >= 0 {
                hi = i;
            } else {
                index = i + 1;
            }
        }
        (index, found_match)
    }

    // UpperBound searches on the non-decreasing Column colIdx,
    // returns the smallest index i such that the value at row i is larger than `d`.
    // UpperBound 同样保留 sort.Search 的二分语义，只把 Go 闭包改成显式循环。
    /// 在非递减列上求上界：最小的 `i` 使得行 `i` 的列值严格大于 `d`。
    pub fn UpperBound(&self, colIdx: usize, d: &types::Datum) -> usize {
        let mut index = 0;
        let mut hi = self.NumRows();
        while index < hi {
            let i = index + (hi - index) / 2;
            if Compare(self.GetRow(i), colIdx, d) > 0 {
                hi = i;
            } else {
                index = i + 1;
            }
        }
        index
    }
}
