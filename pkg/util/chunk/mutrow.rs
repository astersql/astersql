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

// 可变单行 Chunk（`MutRow`）：以容量为 1 的列式布局承载一行，支持按列写值。
//
// 对应 Go `mutrow.go`。`GoAny` 模拟 Go `interface{}` 的类型分支；`Datum` 为
// TiDB 内部通用值容器。`ToRow` 返回的 `Row` 视图仅在本 `MutRow` 存活期间有效。

use crate::{Chunk, Column, Row, mysql, new_column_reference_id, sizeTime, types};

/// 模拟 Go `interface{}` 的异构值枚举，供 `MutRowFromValues` / `SetValue` 使用。
pub enum GoAny {
    /// SQL NULL。
    Nil,
    /// 平台宽度有符号整数。
    Int(isize),
    Int64(i64),
    Uint64(u64),
    Float64(f64),
    Float32(f32),
    String(String),
    Bytes(Vec<u8>),
    /// MySQL BIT / 二进制字面量。
    BinaryLiteral(types::BinaryLiteral),
    /// MySQL DECIMAL（定点数）。
    MyDecimal(Box<types::MyDecimal>),
    /// MySQL DATE/DATETIME/TIMESTAMP。
    Time(types::Time),
    /// 二进制 JSON。
    BinaryJSON(types::BinaryJSON),
    /// TiDB 向量类型。
    VectorFloat32(types::VectorFloat32),
    /// MySQL TIME 时长。
    Duration(types::Duration),
    Enum(types::Enum),
    Set(types::Set),
    /// 未识别类型：写值时保持原载荷不动。
    Other,
}

/// A mutable single-row chunk. `ToRow` is valid while this value remains alive.
///
/// 可变单行 Chunk；`ToRow` 在本值存活期间有效。
pub struct MutRow {
    /// 持有单行数据的 Chunk（capacity=1）。
    pub c: Box<Chunk>,
    /// 行下标（恒为 0）。
    pub idx: usize,
}

impl MutRow {
    /// 返回指向本 Chunk 当前行的只读 `Row` 视图。
    pub fn ToRow(&self) -> Row {
        Row::view(&self.c, self.idx)
    }

    /// 列数。
    pub fn Len(&self) -> usize {
        self.c.columns.len()
    }

    /// 深拷贝底层 Chunk，得到独立可改的副本。
    pub fn Clone(&self) -> MutRow {
        MutRow {
            c: self.c.CopyConstruct(),
            idx: self.idx,
        }
    }

    /// 用另一行的全部列覆盖本行（先清 null，再按定长/变长拷贝字节）。
    pub fn SetRow(&mut self, row: Row) {
        let source = unsafe { &*row.c };
        assert_eq!(self.c.columns.len(), source.columns.len());
        for (columnIndex, sourceColumn) in source.columns.iter().enumerate() {
            let destination = &mut self.c.columns[columnIndex];
            cleanColOfMutRow(destination);
            if sourceColumn.IsNull(row.idx) {
                continue;
            }
            // 定长按 elemBuf 宽度切片；变长按 offsets 取 data 区间。
            if sourceColumn.IsFixed() {
                let width = sourceColumn.elemBuf.len();
                let start = row.idx * width;
                set_fixed_bytes(destination, &sourceColumn.data[start..start + width]);
            } else {
                let start = sourceColumn.offsets[row.idx] as usize;
                let end = sourceColumn.offsets[row.idx + 1] as usize;
                setMutRowBytes(destination, &sourceColumn.data[start..end]);
            }
            mark_not_null(destination);
        }
    }

    /// 按列下标批量写入 `GoAny` 值。
    pub fn SetValues(&mut self, values: Vec<GoAny>) {
        for (index, value) in values.into_iter().enumerate() {
            self.SetValue(index, value);
        }
    }

    /// 写入单列：先清列，再按 `GoAny` 变体编码；`Nil` 保持 null。
    pub fn SetValue(&mut self, columnIndex: usize, value: GoAny) {
        let column = &mut self.c.columns[columnIndex];
        cleanColOfMutRow(column);
        match value {
            GoAny::Nil => return,
            GoAny::Int(value) => set_fixed_bytes(column, &(value as u64).to_le_bytes()),
            GoAny::Int64(value) => set_fixed_bytes(column, &value.to_le_bytes()),
            GoAny::Uint64(value) => set_fixed_bytes(column, &value.to_le_bytes()),
            GoAny::Float64(value) => set_fixed_bytes(column, &value.to_bits().to_le_bytes()),
            GoAny::Float32(value) => set_fixed_bytes(column, &value.to_bits().to_le_bytes()),
            GoAny::String(value) => setMutRowBytes(column, value.as_bytes()),
            GoAny::Bytes(value) => setMutRowBytes(column, &value),
            GoAny::BinaryLiteral(value) => setMutRowBytes(column, &value),
            GoAny::Duration(value) => set_fixed_bytes(column, &value.Duration.to_le_bytes()),
            GoAny::MyDecimal(value) => {
                write_sized(column, types::MyDecimalStructSize, &decimal_bytes(&value))
            }
            GoAny::Time(value) => write_sized(column, sizeTime, &time_bytes(value)),
            GoAny::Enum(value) => setMutRowNameValue(column, &value.Name, value.Value),
            GoAny::Set(value) => setMutRowNameValue(column, &value.Name, value.Value),
            GoAny::BinaryJSON(value) => setMutRowJSON(column, value),
            GoAny::VectorFloat32(value) => {
                let encoded = value.ZeroCopySerialize();
                setMutRowBytes(column, &encoded);
            }
            // Go's default type-switch branch leaves the existing payload in place.
            // Go 默认分支保留原有载荷。
            GoAny::Other => {}
        }
        mark_not_null(column);
    }

    /// 按列下标批量写入 `Datum`。
    pub fn SetDatums(&mut self, datums: Vec<types::Datum>) {
        for (index, datum) in datums.into_iter().enumerate() {
            self.SetDatum(index, datum);
        }
    }

    /// 将单个 `Datum` 写入指定列（按 Kind 分支编码）。
    pub fn SetDatum(&mut self, columnIndex: usize, datum: types::Datum) {
        let kind = datum.Kind();
        if !matches!(
            kind,
            types::KindInt64
                | types::KindUint64
                | types::KindFloat64
                | types::KindFloat32
                | types::KindString
                | types::KindBytes
                | types::KindBinaryLiteral
                | types::KindMysqlBit
                | types::KindMysqlTime
                | types::KindMysqlDuration
                | types::KindMysqlDecimal
                | types::KindMysqlJSON
                | types::KindVectorFloat32
                | types::KindMysqlEnum
                | types::KindMysqlSet
        ) {
            self.c.columns[columnIndex] = makeMutRowColumn(datum_to_value(&datum));
            return;
        }
        let column = &mut self.c.columns[columnIndex];
        cleanColOfMutRow(column);
        if datum.IsNull() {
            return;
        }
        match kind {
            types::KindInt64 => set_fixed_bytes(column, &datum.GetInt64().to_le_bytes()),
            types::KindUint64 | types::KindFloat64 => {
                set_fixed_bytes(column, &datum.GetUint64().to_le_bytes())
            }
            types::KindFloat32 => {
                set_fixed_bytes(column, &datum.GetFloat32().to_bits().to_le_bytes())
            }
            types::KindString
            | types::KindBytes
            | types::KindBinaryLiteral
            | types::KindMysqlBit => setMutRowBytes(column, &datum.GetBytes()),
            types::KindMysqlTime => {
                write_sized(column, sizeTime, &time_bytes(datum.GetMysqlTime()))
            }
            types::KindMysqlDuration => {
                set_fixed_bytes(column, &datum.GetMysqlDuration().Duration.to_le_bytes())
            }
            types::KindMysqlDecimal => write_sized(
                column,
                types::MyDecimalStructSize,
                &decimal_bytes(&datum.GetMysqlDecimal()),
            ),
            types::KindMysqlJSON => setMutRowJSON(column, datum.GetMysqlJSON()),
            types::KindVectorFloat32 => {
                let vector = datum.GetVectorFloat32();
                setMutRowBytes(column, vector.ZeroCopySerialize());
            }
            types::KindMysqlEnum => {
                let value = datum.GetMysqlEnum();
                setMutRowNameValue(column, &value.Name, value.Value);
            }
            types::KindMysqlSet => {
                let value = datum.GetMysqlSet();
                setMutRowNameValue(column, &value.Name, value.Value);
            }
            _ => {}
        }
        mark_not_null(column);
    }

    /// Copies a row subset into the single-row destination.
    ///
    /// Go aliases the source byte slices here. Rust keeps the same bytes and
    /// null/offset layout while owning them, avoiding dangling slice aliases.
    ///
    /// 将源行自某列起的后缀拷入本行对应位置；Rust 拥有字节副本，避免悬垂别名。
    pub fn ShallowCopyPartialRow(&mut self, columnIndex: usize, row: Row) {
        let source = unsafe { &*row.c };
        for (offset, sourceColumn) in source.columns.iter().enumerate() {
            let destination = &mut self.c.columns[columnIndex + offset];
            cleanColOfMutRow(destination);
            if sourceColumn.IsNull(row.idx) {
                continue;
            }
            if sourceColumn.IsFixed() {
                let width = sourceColumn.elemBuf.len();
                let start = row.idx * width;
                set_fixed_bytes(destination, &sourceColumn.data[start..start + width]);
            } else {
                let start = sourceColumn.offsets[row.idx] as usize;
                let end = sourceColumn.offsets[row.idx + 1] as usize;
                setMutRowBytes(destination, &sourceColumn.data[start..end]);
            }
            mark_not_null(destination);
        }
    }
}

/// 由一组 `GoAny` 构造单行 `MutRow`（每值一列，capacity=1）。
pub fn MutRowFromValues(values: Vec<GoAny>) -> MutRow {
    let columns = values.into_iter().map(makeMutRowColumn).collect();
    MutRow {
        c: Box::new(Chunk {
            sel: None,
            columns,
            numVirtualRows: 0,
            capacity: 1,
            requiredRows: 1,
            inCompleteChunk: false,
        }),
        idx: 0,
    }
}

/// 由 `Datum` 列表构造 `MutRow`。
pub fn MutRowFromDatums(datums: Vec<types::Datum>) -> MutRow {
    MutRowFromValues(datums.iter().map(datum_to_value).collect())
}

/// 按字段类型填零值，构造空内容的单行 `MutRow`。
pub fn MutRowFromTypes(fieldTypes: Vec<types::FieldType>) -> MutRow {
    MutRowFromValues(fieldTypes.iter().map(zeroValForType).collect())
}

/// 将 `Datum` 转为对应的 `GoAny` 变体。
fn datum_to_value(datum: &types::Datum) -> GoAny {
    match datum.Kind() {
        types::KindNull => GoAny::Nil,
        types::KindInt64 => GoAny::Int64(datum.GetInt64()),
        types::KindUint64 => GoAny::Uint64(datum.GetUint64()),
        types::KindFloat32 => GoAny::Float32(datum.GetFloat32()),
        types::KindFloat64 => GoAny::Float64(f64::from_bits(datum.GetUint64())),
        types::KindString | types::KindBytes => GoAny::Bytes(datum.GetBytes().to_vec()),
        types::KindBinaryLiteral | types::KindMysqlBit => {
            GoAny::BinaryLiteral(types::BinaryLiteral(datum.GetBytes().to_vec()))
        }
        types::KindMysqlDecimal => GoAny::MyDecimal(Box::new(datum.GetMysqlDecimal().clone())),
        types::KindMysqlDuration => GoAny::Duration(datum.GetMysqlDuration()),
        types::KindMysqlEnum => GoAny::Enum(datum.GetMysqlEnum().clone()),
        types::KindMysqlSet => GoAny::Set(datum.GetMysqlSet().clone()),
        types::KindMysqlTime => GoAny::Time(datum.GetMysqlTime()),
        types::KindMysqlJSON => GoAny::BinaryJSON(datum.GetMysqlJSON()),
        types::KindVectorFloat32 => GoAny::VectorFloat32(datum.GetVectorFloat32().Clone()),
        types::KindInterface => datum
            .GetInterface()
            .map(|value| interface_to_value(value.as_ref()))
            .unwrap_or(GoAny::Nil),
        _ => GoAny::Nil,
    }
}

/// Convert the concrete payload of a Go-style interface datum through the
/// same set of types accepted by `makeMutRowColumn`.
fn interface_to_value(value: &dyn std::any::Any) -> GoAny {
    if let Some(value) = value.downcast_ref::<isize>() {
        GoAny::Int(*value)
    } else if let Some(value) = value.downcast_ref::<i32>() {
        GoAny::Int64(*value as i64)
    } else if let Some(value) = value.downcast_ref::<i64>() {
        GoAny::Int64(*value)
    } else if let Some(value) = value.downcast_ref::<u64>() {
        GoAny::Uint64(*value)
    } else if let Some(value) = value.downcast_ref::<f32>() {
        GoAny::Float32(*value)
    } else if let Some(value) = value.downcast_ref::<f64>() {
        GoAny::Float64(*value)
    } else if let Some(value) = value.downcast_ref::<String>() {
        GoAny::String(value.clone())
    } else if let Some(value) = value.downcast_ref::<Vec<u8>>() {
        GoAny::Bytes(value.clone())
    } else if let Some(value) = value.downcast_ref::<types::BinaryLiteral>() {
        GoAny::BinaryLiteral(value.clone())
    } else if let Some(value) = value.downcast_ref::<types::MyDecimal>() {
        GoAny::MyDecimal(Box::new(value.clone()))
    } else if let Some(value) = value.downcast_ref::<types::Time>() {
        GoAny::Time(*value)
    } else if let Some(value) = value.downcast_ref::<types::BinaryJSON>() {
        GoAny::BinaryJSON(value.clone())
    } else if let Some(value) = value.downcast_ref::<types::VectorFloat32>() {
        GoAny::VectorFloat32(value.Clone())
    } else if let Some(value) = value.downcast_ref::<types::Duration>() {
        GoAny::Duration(*value)
    } else if let Some(value) = value.downcast_ref::<types::Enum>() {
        GoAny::Enum(value.clone())
    } else if let Some(value) = value.downcast_ref::<types::Set>() {
        GoAny::Set(value.clone())
    } else {
        GoAny::Other
    }
}

/// 按 MySQL 字段类型给出零值 `GoAny`（无符号整数走 Uint64）。
fn zeroValForType(fieldType: &types::FieldType) -> GoAny {
    match fieldType.GetType() {
        mysql::TypeFloat => GoAny::Float32(0.0),
        mysql::TypeDouble => GoAny::Float64(0.0),
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeYear => {
            // UNSIGNED 标志决定有符号/无符号零值。
            if mysql::HasUnsignedFlag(fieldType.GetFlag()) {
                GoAny::Uint64(0)
            } else {
                GoAny::Int64(0)
            }
        }
        mysql::TypeString | mysql::TypeVarString | mysql::TypeVarchar => {
            GoAny::String(String::new())
        }
        mysql::TypeBlob | mysql::TypeTinyBlob | mysql::TypeMediumBlob | mysql::TypeLongBlob => {
            GoAny::Bytes(Vec::new())
        }
        mysql::TypeDuration => GoAny::Duration(types::Duration::default()),
        mysql::TypeNewDecimal => GoAny::MyDecimal(Box::default()),
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
            GoAny::Time(types::Time::default())
        }
        mysql::TypeBit => GoAny::BinaryLiteral(types::BinaryLiteral::default()),
        mysql::TypeSet => GoAny::Set(types::Set::default()),
        mysql::TypeEnum => GoAny::Enum(types::Enum::default()),
        mysql::TypeJSON => GoAny::BinaryJSON(types::CreateBinaryJSON(())),
        mysql::TypeTiDBVectorFloat32 => {
            GoAny::VectorFloat32(types::ParseVectorFloat32("[]").expect("empty vector is valid"))
        }
        _ => GoAny::Nil,
    }
}

/// 将单个 `GoAny` 编码为一列（length=1）的 `Column`。
fn makeMutRowColumn(value: GoAny) -> Column {
    match value {
        GoAny::Nil => {
            // NULL：变长空列并把 nullBitmap 置 0。
            let mut column = makeMutRowBytesColumn(&[]);
            column.nullBitmap[0] = 0;
            column
        }
        GoAny::Int(value) => makeMutRowUint64Column(value as u64),
        GoAny::Int64(value) => makeMutRowUint64Column(value as u64),
        GoAny::Uint64(value) => makeMutRowUint64Column(value),
        GoAny::Float64(value) => makeMutRowUint64Column(value.to_bits()),
        GoAny::Float32(value) => {
            let mut column = newMutRowFixedLenColumn(4);
            set_fixed_bytes(&mut column, &value.to_bits().to_le_bytes());
            column
        }
        GoAny::String(value) => makeMutRowBytesColumn(value.as_bytes()),
        GoAny::Bytes(value) => makeMutRowBytesColumn(&value),
        GoAny::BinaryLiteral(value) => makeMutRowBytesColumn(&value),
        GoAny::MyDecimal(value) => {
            let mut column = newMutRowFixedLenColumn(types::MyDecimalStructSize);
            write_sized(
                &mut column,
                types::MyDecimalStructSize,
                &decimal_bytes(&value),
            );
            column
        }
        GoAny::Time(value) => {
            let mut column = newMutRowFixedLenColumn(sizeTime);
            write_sized(&mut column, sizeTime, &time_bytes(value));
            column
        }
        GoAny::BinaryJSON(value) => {
            let mut column = newMutRowVarLenColumn(value.Value.len() + 1);
            setMutRowJSON(&mut column, value);
            column
        }
        GoAny::VectorFloat32(value) => makeMutRowBytesColumn(&value.ZeroCopySerialize()),
        GoAny::Duration(value) => makeMutRowUint64Column(value.Duration as u64),
        GoAny::Enum(value) => {
            let mut column = newMutRowVarLenColumn(value.Name.len() + 8);
            setMutRowNameValue(&mut column, &value.Name, value.Value);
            column
        }
        GoAny::Set(value) => {
            let mut column = newMutRowVarLenColumn(value.Name.len() + 8);
            setMutRowNameValue(&mut column, &value.Name, value.Value);
            column
        }
        GoAny::Other => Column::default(),
    }
}

/// 新建单行定长列骨架（data/elemBuf 置零，nullBitmap=1）。
fn newMutRowFixedLenColumn(elementSize: usize) -> Column {
    Column {
        length: 1,
        elemBuf: vec![0; elementSize],
        data: vec![0; elementSize],
        nullBitmap: vec![1],
        offsets: Vec::new(),
        avoidReusing: false,
        reference_id: new_column_reference_id(),
    }
}

/// 新建单行变长列骨架（offsets=[0, valueSize]）。
fn newMutRowVarLenColumn(valueSize: usize) -> Column {
    Column {
        length: 1,
        offsets: vec![0, valueSize as i64],
        data: vec![0; valueSize],
        nullBitmap: vec![1],
        elemBuf: Vec::new(),
        avoidReusing: false,
        reference_id: new_column_reference_id(),
    }
}

/// 8 字节定长列写入小端 u64。
fn makeMutRowUint64Column(value: u64) -> Column {
    let mut column = newMutRowFixedLenColumn(8);
    set_fixed_bytes(&mut column, &value.to_le_bytes());
    column
}

/// 变长列写入原始字节。
fn makeMutRowBytesColumn(bytes: &[u8]) -> Column {
    let mut column = newMutRowVarLenColumn(bytes.len());
    column.data.copy_from_slice(bytes);
    column
}

/// 写值前清空：offsets 归零，nullBitmap[0]=0（表示先视为 NULL）。
fn cleanColOfMutRow(column: &mut Column) {
    column.offsets.fill(0);
    if column.nullBitmap.is_empty() {
        column.nullBitmap.push(0);
    } else {
        column.nullBitmap[0] = 0;
    }
}

/// 标记当前单行非空。
fn mark_not_null(column: &mut Column) {
    if column.nullBitmap.is_empty() {
        column.nullBitmap.push(1);
    } else {
        column.nullBitmap[0] = 1;
    }
}

/// 定长列：覆盖 data 并同步 elemBuf 宽度。
fn set_fixed_bytes(column: &mut Column, bytes: &[u8]) {
    column.data.resize(bytes.len(), 0);
    column.data.copy_from_slice(bytes);
    column.elemBuf.resize(bytes.len(), 0);
}

/// 定长结构体写入：先按 size 清零再拷贝（截断或填零）。
fn write_sized(column: &mut Column, size: usize, bytes: &[u8]) {
    column.data.resize(size, 0);
    column.data.fill(0);
    let copied = size.min(bytes.len());
    column.data[..copied].copy_from_slice(&bytes[..copied]);
    column.elemBuf.resize(size, 0);
}

/// 将 `MyDecimal` 序列化为固定长度字节布局。
fn decimal_bytes(decimal: &types::MyDecimal) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(types::MyDecimalStructSize);
    bytes.push(decimal.digitsInt as u8);
    bytes.push(decimal.digitsFrac as u8);
    bytes.push(decimal.resultFrac as u8);
    bytes.push(u8::from(decimal.negative));
    for word in decimal.wordBuf {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    debug_assert_eq!(bytes.len(), types::MyDecimalStructSize);
    bytes
}

/// 提取 `Time` 的核心时间戳字节。
fn time_bytes(value: types::Time) -> [u8; sizeTime] {
    value.coreTime.0.to_ne_bytes()
}

/// 变长列写入：更新 data 与 offsets[0..1]，清空 elemBuf。
fn setMutRowBytes(column: &mut Column, bytes: &[u8]) {
    column.data.clear();
    column.data.extend_from_slice(bytes);
    if column.offsets.len() < 2 {
        column.offsets.resize(2, 0);
    }
    column.offsets[0] = 0;
    column.offsets[1] = bytes.len() as i64;
    column.elemBuf.clear();
}

/// Enum/Set：小端 value + name 字节拼进变长列。
fn setMutRowNameValue(column: &mut Column, name: &str, value: u64) {
    let mut bytes = Vec::with_capacity(name.len() + 8);
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes.extend_from_slice(name.as_bytes());
    setMutRowBytes(column, &bytes);
}

/// JSON：TypeCode 前缀 + Value 载荷。
fn setMutRowJSON(column: &mut Column, json: types::BinaryJSON) {
    let mut bytes = Vec::with_capacity(json.Value.len() + 1);
    bytes.push(json.TypeCode);
    bytes.extend_from_slice(&json.Value);
    setMutRowBytes(column, &bytes);
}
