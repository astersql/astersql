// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 行解码器：将新格式 row 字节解到 Datum map、chunk.Chunk 或旧 datum bytes。
//
// 三类解码器共享 `decoder` 基座（内嵌 `row`）；列元数据由 `ColInfo` 描述。

// rowcodec 中三类行解码器：解到 Datum map、解到 chunk.Chunk、以及解回旧 datum bytes。

use std::collections::HashMap;

// decoder contains base util for decode row.
// decoder 是三个具体解码器共用的基础状态，Go 里匿名嵌入 row；这里显式保留 row 字段便于对照。
/// 三类解码器共用的基座：持有 `row`、列元信息、handle 列 ID 与时区。
pub struct decoder {
    row: row,
    pub columns: Vec<ColInfo>,
    pub handleColIDs: Vec<i64>,
    // Go 的 *time.Location 允许 nil；用 Option 表达该空值语义。
    pub loc: Option<time::Location>,
}

impl decoder {
    fn fromBytes(&mut self, row_data: &[u8]) -> Result<(), errors::SharedError> {
        self.row.fromBytes(row_data)
    }

    fn getData(&self, index: usize) -> &[u8] {
        self.row.getData(index)
    }

    /// 判断指定列在 row 中是否为 NULL（含缺列时的默认值语义）。
    pub fn ColumnIsNull(
        &mut self,
        row_data: &[u8],
        column_id: i64,
        default_value: Option<&[u8]>,
    ) -> Result<bool, errors::SharedError> {
        self.row.ColumnIsNull(row_data, column_id, default_value)
    }
}

// NewDecoder creates a decoder.
// NewDecoder 对应 Go 构造函数，只填入列元信息、handle 列 ID 和时区，不解析任何行数据。
/// 构造基础解码器：填入列元信息、handle 列 ID 和时区，不解析行数据。
pub fn NewDecoder(
    columns: Vec<ColInfo>,
    handleColIDs: Vec<i64>,
    loc: Option<time::Location>,
) -> decoder {
    decoder {
        row: row::default(),
        columns,
        handleColIDs,
        loc,
    }
}

// ColInfo is used as column meta info for row decoder.
// ColInfo 保留 Go 中解码所需的列元数据：列 ID、是否主键句柄、是否虚拟生成列以及字段类型。
/// 解码所需列元数据：列 ID、主键句柄、虚拟生成列标记与字段类型。
#[derive(Clone)]
pub struct ColInfo {
    pub ID: i64,
    pub IsPKHandle: bool,
    pub VirtualGenCol: bool,
    // Rust 用值类型表达 Go 的 *types.FieldType；解码路径会无条件解引用该字段。
    pub Ft: types::FieldType,
}

// DatumMapDecoder decodes the row to datum map.
// DatumMapDecoder 把 row 字节解成 colID -> Datum 的映射，保持 Go 里嵌入 decoder 的结构。
/// 将 row 字节解码为 `colID -> Datum` 映射。
pub struct DatumMapDecoder {
    pub decoder: decoder,
}

// NewDatumMapDecoder creates a DatumMapDecoder.
// NewDatumMapDecoder 只设置普通列和时区；handle 列在 Go 构造函数中没有传入。
/// 构造 DatumMap 解码器（不设置 handle 列）。
pub fn NewDatumMapDecoder(columns: Vec<ColInfo>, loc: Option<time::Location>) -> DatumMapDecoder {
    DatumMapDecoder {
        decoder: decoder {
            row: row::default(),
            columns,
            handleColIDs: Vec::new(),
            loc,
        },
    }
}

impl DatumMapDecoder {
    /// 返回行校验和及是否存在。
    pub fn GetChecksum(&self) -> (u32, bool) {
        self.decoder.row.GetChecksum()
    }

    /// 返回校验和版本号（仅在已编码 checksum 时有效）。
    pub fn ChecksumVersion(&self) -> i32 {
        self.decoder.row.ChecksumVersion()
    }

    // DecodeToDatumMap decodes byte slices to datum map.
    // DecodeToDatumMap 先把原始 rowData 解析到内部 row，再按列 ID 解出 Datum。
    /// 解析 rowData 并按列配置填充 Datum map。
    pub fn DecodeToDatumMap(
        &mut self,
        rowData: &[u8],
        row: Option<HashMap<i64, types::Datum>>,
    ) -> Result<HashMap<i64, types::Datum>, errors::SharedError> {
        // Go 允许传入 nil map；这里用 Option 在入口处补一个容量等于列数的 HashMap。
        let mut row = row.unwrap_or_else(|| HashMap::with_capacity(self.decoder.columns.len()));
        if let Err(err) = self.decoder.fromBytes(rowData) {
            return Err(err);
        }

        for i in 0..self.decoder.columns.len() {
            let col = &self.decoder.columns[i];
            let (idx, isNil, notFound) = self.decoder.row.findColID(col.ID);
            if !notFound && !isNil {
                // 找到非 NULL 列时，复用 row 中保存的列数据 slice，不额外做业务校验。
                let colData = self.decoder.getData(idx);
                let d = self.decodeColDatum(col, colData)?;
                row.insert(col.ID, d);
                continue;
            }

            if isNil {
                // Go Datum 通过 SetNull 标记 SQL NULL；保持这个显式分支。
                let mut d = types::Datum::default();
                d.SetNull();
                row.insert(col.ID, d);
                continue;
            }
        }
        Ok(row)
    }

    // decodeColDatum 按字段类型把列字节解成 TiDB Datum。
    // 这里的 match 顺序和 Go switch 保持一致，便于检查每个 MySQL 类型分支是否遗漏。
    fn decodeColDatum(
        &self,
        col: &ColInfo,
        colData: &[u8],
    ) -> Result<types::Datum, errors::SharedError> {
        let mut d = types::Datum::default();
        match col.Ft.GetType() {
            mysql::TypeLonglong
            | mysql::TypeLong
            | mysql::TypeInt24
            | mysql::TypeShort
            | mysql::TypeTiny => {
                // 整数列根据 unsigned flag 决定写入 Uint64 还是 Int64，保持 Go 的二进制解码函数。
                if mysql::HasUnsignedFlag(col.Ft.GetFlag()) {
                    d.SetUint64(decodeUint(colData));
                } else {
                    d.SetInt64(decodeInt(colData));
                }
            }
            mysql::TypeYear => {
                d.SetInt64(decodeInt(colData));
            }
            mysql::TypeFloat => {
                let (_remain, fVal) = codec::DecodeFloat(colData)?;
                d.SetFloat32(fVal as f32);
            }
            mysql::TypeDouble => {
                let (_remain, fVal) = codec::DecodeFloat(colData)?;
                d.SetFloat64(fVal);
            }
            mysql::TypeVarString
            | mysql::TypeVarchar
            | mysql::TypeString
            | mysql::TypeBlob
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob => {
                // Go string(colData) 会无条件复制原始字节；Rust String 要求 UTF-8，后续接线需决定等价表示。
                d.SetBytesAsString(
                    colData.to_vec(),
                    col.Ft.GetCollate().to_owned(),
                    colData.len() as u32,
                );
            }
            mysql::TypeNewDecimal => {
                let (_remain, dec, precision, frac) = codec::DecodeDecimal(colData)?;
                d.SetMysqlDecimal(dec);
                d.SetLength(precision);
                d.SetFrac(frac);
            }
            mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
                let mut t = types::Time::default();
                t.SetType(col.Ft.GetType());
                t.SetFsp(col.Ft.GetDecimal() as i32);
                t.FromPackedUint(decodeUint(colData))
                    .map_err(|error| errors::New(error.to_string()))?;
                if col.Ft.GetType() == mysql::TypeTimestamp && !t.IsZero() {
                    // Go 在 DatumMapDecoder 中没有检查 loc 是否 nil，按原语义直接做 UTC -> loc 转换。
                    let target = self.decoder.loc.unwrap_or(time::UTC);
                    t.ConvertTimeZone(time::UTC, target)
                        .map_err(|error| errors::New(error.to_string()))?;
                }
                d.SetMysqlTime(t);
            }
            mysql::TypeDuration => {
                let mut dur = types::Duration::default();
                // Go 把解出的有符号 int64 直接转 time.Duration；保留负 duration 语义。
                dur.Duration = decodeInt(colData);
                dur.Fsp = col.Ft.GetDecimal() as i32;
                d.SetMysqlDuration(dur);
            }
            mysql::TypeEnum => {
                // ignore error deliberately, to read empty enum value.
                // Go 故意忽略 enum 解析错误以读取空 enum 值；这里保留相同容错分支。
                let enumVal = match types::ParseEnumValue(col.Ft.GetElems(), decodeUint(colData)) {
                    Ok(v) => v,
                    Err(_err) => types::Enum::default(),
                };
                d.SetMysqlEnum(enumVal, col.Ft.GetCollate().to_owned());
            }
            mysql::TypeSet => {
                let set = types::ParseSetValue(col.Ft.GetElems(), decodeUint(colData))
                    .map_err(|error| errors::New(error.to_string()))?;
                d.SetMysqlSet(set, col.Ft.GetCollate().to_owned());
            }
            mysql::TypeBit => {
                let byteSize = (col.Ft.GetFlen() + 7) >> 3;
                d.SetMysqlBit(types::NewBinaryLiteralFromUint(
                    decodeUint(colData),
                    byteSize,
                ));
            }
            mysql::TypeJSON => {
                let mut j = types::BinaryJSON::default();
                // Go 直接读取 colData[0] 和 colData[1:]，假设 row 编码保证 JSON 数据非空。
                j.TypeCode = colData[0];
                j.Value = colData[1..].to_vec();
                d.SetMysqlJSON(j);
            }
            mysql::TypeTiDBVectorFloat32 => {
                let (v, _remain) = types::ZeroCopyDeserializeVectorFloat32(colData)
                    .map_err(|error| errors::New(error.to_string()))?;
                d.SetVectorFloat32(v);
            }
            _ => {
                return Err(errors::Errorf(format!("unknown type {}", col.Ft.GetType())));
            }
        }
        Ok(d)
    }
}

// ChunkDecoder decodes the row to chunk.Chunk.
// ChunkDecoder 把 row 字节追加到 chunk.Chunk；默认值回调由调用方提供，可能为空。
/// 将 row 字节追加到列式 `chunk.Chunk` 的解码器。
pub struct ChunkDecoder {
    pub decoder: decoder,
    pub defDatum: Option<Box<dyn Fn(usize, &mut chunk::Chunk) -> Result<(), errors::SharedError>>>,
}

// NewChunkDecoder creates a NewChunkDecoder.
// NewChunkDecoder 保存列元数据、handle 列和默认值回调；不立即解码 rowData。
/// 构造 Chunk 解码器：列元数据、handle 列、默认值回调与时区。
pub fn NewChunkDecoder(
    columns: Vec<ColInfo>,
    handleColIDs: Vec<i64>,
    defDatum: Option<Box<dyn Fn(usize, &mut chunk::Chunk) -> Result<(), errors::SharedError>>>,
    loc: Option<time::Location>,
) -> ChunkDecoder {
    ChunkDecoder {
        decoder: decoder {
            row: row::default(),
            columns,
            handleColIDs,
            loc,
        },
        defDatum,
    }
}

impl ChunkDecoder {
    // DecodeToChunk decodes a row to chunk.
    // DecodeToChunk 是 chunk 解码主流程：特殊列、虚拟列、row value、handle fallback、默认值依次处理。
    /// 将一行解码并追加到 chunk；`commitTS`/`handle` 用于特殊列与缺列补值。
    pub fn DecodeToChunk(
        &mut self,
        rowData: &[u8],
        commitTS: u64,
        handle: Option<&dyn kv::Handle>,
        chk: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        if let Err(err) = self.decoder.fromBytes(rowData) {
            return Err(err);
        }

        for colIdx in 0..self.decoder.columns.len() {
            let col = &self.decoder.columns[colIdx];
            if col.ID == model::ExtraCommitTSID {
                // ExtraCommitTSID 出现时 Go 通过 intest.Assert 要求 commitTS 有效；Release 下仍按值决定追加 NULL。
                intest::Assert(
                    commitTS > 0,
                    &["commitTS should be valid if ExtraCommitTSID exists".into()],
                );
                if commitTS > 0 {
                    chk.AppendUint64(colIdx, commitTS);
                } else {
                    chk.AppendNull(colIdx);
                }
                continue;
            }
            // fill the virtual column value after row calculation
            // 虚拟生成列的真实值由后续表达式计算填充，本解码器只先占一个 NULL。
            if col.VirtualGenCol {
                chk.AppendNull(colIdx);
                continue;
            }
            if col.ID == model::ExtraRowChecksumID {
                // row checksum 特殊列不从当前 row value 解码，保持 Go 的 NULL 占位。
                chk.AppendNull(colIdx);
                continue;
            }

            let (idx, isNil, notFound) = self.decoder.row.findColID(col.ID);
            if !notFound && !isNil {
                let colData = self.decoder.getData(idx);
                self.decodeColToChunk(colIdx, col, colData, chk)?;
                continue;
            }

            // Only try to decode handle when there is no corresponding column in the value.
            // This is because the information in handle may be incomplete in some cases.
            // For example, prefixed clustered index like 'primary key(col1(1))' only store the leftmost 1 char in the handle.
            // 只有 value 缺列时才尝试从 handle 补值，避免前缀聚簇索引等场景用不完整 handle 覆盖真实列值。
            if self.tryAppendHandleColumn(colIdx, col, handle, chk) {
                continue;
            }

            if isNil {
                chk.AppendNull(colIdx);
                continue;
            }

            if self.defDatum.is_none() {
                chk.AppendNull(colIdx);
                continue;
            }

            // 默认值回调可能访问列定义或表达式上下文；错误按 Go 语义原样返回。
            if let Some(defDatum) = &self.defDatum {
                defDatum(colIdx, chk)?;
            }
        }
        Ok(())
    }

    // tryAppendHandleColumn 尝试把 handle 中的列值直接追加进 chunk。
    // Go 的 kv.Handle 是接口并可为 nil；以 Option<&dyn kv::Handle> 保留该语义。
    fn tryAppendHandleColumn(
        &self,
        colIdx: usize,
        col: &ColInfo,
        handle: Option<&dyn kv::Handle>,
        chk: &mut chunk::Chunk,
    ) -> bool {
        let Some(handle) = handle else {
            return false;
        };
        if handle.IsInt() && col.ID == self.decoder.handleColIDs[0] {
            // 单整数 handle 可直接转成 int64 列值；handleColIDs[0] 的越界行为沿用 Go 的前置假设。
            chk.AppendInt64(colIdx, handle.IntValue());
            return true;
        }
        for (i, id) in self.decoder.handleColIDs.iter().enumerate() {
            if col.ID == *id {
                if types::NeedRestoredData(&col.Ft) {
                    // 需要 restored data 的类型不能只依赖 encoded handle，否则会丢失恢复信息。
                    return false;
                }
                let mut coder = codec::NewDecoder(
                    chk as *mut chunk::Chunk,
                    self.decoder.loc.unwrap_or(time::UTC),
                );
                let mut field_type = col.Ft.clone();
                let result = coder.DecodeOne(handle.EncodedCol(i), colIdx, &mut field_type);
                return result.is_ok();
            }
        }
        false
    }

    // decodeColToChunk 按字段类型把列字节追加到 chunk 的指定列。
    // 和 decodeColDatum 相比，这里直接调用 chunk Append 系列方法，部分类型还有 chunk 写入前处理。
    fn decodeColToChunk(
        &self,
        colIdx: usize,
        col: &ColInfo,
        colData: &[u8],
        chk: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        match col.Ft.GetType() {
            mysql::TypeLonglong
            | mysql::TypeLong
            | mysql::TypeInt24
            | mysql::TypeShort
            | mysql::TypeTiny => {
                if mysql::HasUnsignedFlag(col.Ft.GetFlag()) {
                    chk.AppendUint64(colIdx, decodeUint(colData));
                } else {
                    chk.AppendInt64(colIdx, decodeInt(colData));
                }
            }
            mysql::TypeYear => {
                chk.AppendInt64(colIdx, decodeInt(colData));
            }
            mysql::TypeFloat => {
                let (_remain, fVal) = codec::DecodeFloat(colData)?;
                chk.AppendFloat32(colIdx, fVal as f32);
            }
            mysql::TypeDouble => {
                let (_remain, fVal) = codec::DecodeFloat(colData)?;
                chk.AppendFloat64(colIdx, fVal);
            }
            mysql::TypeVarString
            | mysql::TypeVarchar
            | mysql::TypeString
            | mysql::TypeBlob
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob => {
                chk.AppendBytes(colIdx, colData);
            }
            mysql::TypeNewDecimal => {
                let (_remain, mut dec, _precision, frac) = codec::DecodeDecimal(colData)?;
                if col.Ft.GetDecimal() != types::UnspecifiedLength as isize
                    && frac as isize > col.Ft.GetDecimal()
                {
                    // chunk 写入 decimal 前需要按字段 scale 四舍五入，保持 Go 的 ModeHalfUp。
                    let mut to = types::MyDecimal::default();
                    dec.Round(&mut to, col.Ft.GetDecimal(), types::ModeHalfUp)
                        .map_err(|error| errors::New(error.to_string()))?;
                    dec = to;
                }
                chk.AppendMyDecimal(colIdx, &dec);
            }
            mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
                let mut t = types::Time::default();
                t.SetType(col.Ft.GetType());
                t.SetFsp(col.Ft.GetDecimal() as i32);
                t.FromPackedUint(decodeUint(colData))
                    .map_err(|error| errors::New(error.to_string()))?;
                if col.Ft.GetType() == mysql::TypeTimestamp
                    && self.decoder.loc.is_some()
                    && !t.IsZero()
                {
                    // ChunkDecoder 的 Go 版本在时区非 nil 时才转换 TIMESTAMP。
                    let target = self.decoder.loc.unwrap_or(time::UTC);
                    t.ConvertTimeZone(time::UTC, target)
                        .map_err(|error| errors::New(error.to_string()))?;
                }
                chk.AppendTime(colIdx, t);
            }
            mysql::TypeDuration => {
                let mut dur = types::Duration::default();
                // Duration 可能为负，不能简单按 Rust 标准库的无符号 Duration 解释。
                dur.Duration = decodeInt(colData);
                dur.Fsp = col.Ft.GetDecimal() as i32;
                chk.AppendDuration(colIdx, dur);
            }
            mysql::TypeEnum => {
                // ignore error deliberately, to read empty enum value.
                // 和 Datum 解码一致，enum 解析失败时写入空 enum，保证可读取空 enum 值。
                let enumVal = match types::ParseEnumValue(col.Ft.GetElems(), decodeUint(colData)) {
                    Ok(v) => v,
                    Err(_err) => types::Enum::default(),
                };
                chk.AppendEnum(colIdx, enumVal);
            }
            mysql::TypeSet => {
                let set = types::ParseSetValue(col.Ft.GetElems(), decodeUint(colData))
                    .map_err(|error| errors::New(error.to_string()))?;
                chk.AppendSet(colIdx, set);
            }
            mysql::TypeBit => {
                let byteSize = (col.Ft.GetFlen() + 7) >> 3;
                chk.AppendBytes(
                    colIdx,
                    &types::NewBinaryLiteralFromUint(decodeUint(colData), byteSize).0,
                );
            }
            mysql::TypeJSON => {
                let mut j = types::BinaryJSON::default();
                j.TypeCode = colData[0];
                j.Value = colData[1..].to_vec();
                chk.AppendJSON(colIdx, j);
            }
            mysql::TypeTiDBVectorFloat32 => {
                let (v, _remain) = types::ZeroCopyDeserializeVectorFloat32(colData)
                    .map_err(|error| errors::New(error.to_string()))?;
                chk.AppendVectorFloat32(colIdx, v);
            }
            _ => {
                return Err(errors::Errorf(format!("unknown type {}", col.Ft.GetType())));
            }
        }
        Ok(())
    }
}

// BytesDecoder decodes the row to old datums bytes.
// BytesDecoder 把新 rowcodec 格式转换回旧 datum 字节格式，用于兼容老的 flag+payload 表示。
/// 将新 rowcodec 格式转回旧 datum 字节（flag+payload）的解码器。
pub struct BytesDecoder {
    pub decoder: decoder,
    pub defBytes: Option<Box<dyn Fn(usize) -> Result<Vec<u8>, errors::SharedError>>>,
}

// NewByteDecoder creates a BytesDecoder.
// defBytes: provided default value bytes in old datum format(flag+colData).
// NewByteDecoder 保存旧 datum 默认值回调；回调返回的字节已经是 flag+colData 格式。
/// 构造 Bytes 解码器；`defBytes` 返回旧 datum 格式默认值。
pub fn NewByteDecoder(
    columns: Vec<ColInfo>,
    handleColIDs: Vec<i64>,
    defBytes: Option<Box<dyn Fn(usize) -> Result<Vec<u8>, errors::SharedError>>>,
    loc: Option<time::Location>,
) -> BytesDecoder {
    BytesDecoder {
        decoder: decoder {
            row: row::default(),
            columns,
            handleColIDs,
            loc,
        },
        defBytes,
    }
}

impl BytesDecoder {
    // decodeToBytesInternal 是 DecodeToBytesNoHandle 和 DecodeToBytes 的公共实现。
    // outputOffset 决定列 ID 在返回二维字节数组中的位置，handle/cacheBytes 用于缺列时补主键值。
    fn decodeToBytesInternal(
        &self,
        outputOffset: &HashMap<i64, usize>,
        handle: Option<&dyn kv::Handle>,
        value: &[u8],
        cacheBytes: &[u8],
    ) -> Result<Vec<Vec<u8>>, errors::SharedError> {
        let mut r = row::default();
        if let Err(err) = r.fromBytes(value) {
            return Err(err);
        }
        let mut values = vec![Vec::<u8>::new(); outputOffset.len()];
        for i in 0..self.decoder.columns.len() {
            let col = &self.decoder.columns[i];
            let tp = fieldType2Flag(
                col.Ft.ArrayType().GetType(),
                col.Ft.GetFlag() & mysql::UnsignedFlag == 0,
            );
            let colID = col.ID;
            // Go map 缺失时会得到 0 值；显式 unwrap_or(0) 保留这个宽松行为。
            let offset = *outputOffset.get(&colID).unwrap_or(&0);
            let (idx, isNil, notFound) = r.findColID(colID);
            if !notFound && !isNil {
                let val = r.getData(idx);
                values[offset] = self.encodeOldDatum(tp, val);
                continue;
            }

            // Only try to decode handle when there is no corresponding column in the value.
            // This is because the information in handle may be incomplete in some cases.
            // For example, prefixed clustered index like 'primary key(col1(1))' only store the leftmost 1 char in the handle.
            // 缺列时才尝试从 handle 解码，避免不完整 handle 覆盖 value 里已有的真实列数据。
            if self.tryDecodeHandle(&mut values, offset, col, handle, cacheBytes) {
                continue;
            }

            if isNil {
                values[offset] = vec![NilFlag];
                continue;
            }

            if let Some(defBytes) = &self.defBytes {
                let defVal = defBytes(i)?;
                if !defVal.is_empty() {
                    // Go 直接复用默认值回调返回的旧 datum 字节；这里保留同样的覆盖逻辑。
                    values[offset] = defVal;
                    continue;
                }
            }

            values[offset] = vec![NilFlag];
        }
        Ok(values)
    }

    // tryDecodeHandle 尝试把 handle 编码成旧 datum 字节。
    // cacheBytes 对应 Go 中可复用的临时缓冲区，用 Vec 克隆表达 append 到同一缓冲区的意图。
    fn tryDecodeHandle(
        &self,
        values: &mut Vec<Vec<u8>>,
        offset: usize,
        col: &ColInfo,
        handle: Option<&dyn kv::Handle>,
        cacheBytes: &[u8],
    ) -> bool {
        let Some(handle) = handle else {
            return false;
        };
        if types::NeedRestoredData(&col.Ft) {
            return false;
        }
        if col.IsPKHandle || col.ID == model::ExtraHandleID {
            let mut handleData = cacheBytes.to_vec();
            if mysql::HasUnsignedFlag(col.Ft.GetFlag()) {
                handleData.push(UintFlag);
                handleData = codec::EncodeUint(handleData, handle.IntValue() as u64);
            } else {
                handleData.push(IntFlag);
                handleData = codec::EncodeInt(handleData, handle.IntValue());
            }
            values[offset] = handleData;
            return true;
        }
        let mut handleData = Vec::<u8>::new();
        for (i, hid) in self.decoder.handleColIDs.iter().enumerate() {
            if col.ID == *hid {
                // 复合 handle 列已经是旧 datum 编码片段，Go 直接 append EncodedCol(i)。
                handleData.extend_from_slice(&handle.EncodedCol(i));
            }
        }
        if !handleData.is_empty() {
            values[offset] = handleData;
            return true;
        }
        false
    }

    // DecodeToBytesNoHandle decodes raw byte slice to row data without handle.
    // DecodeToBytesNoHandle 对应 handle 为 nil、cacheBytes 为 nil 的旧 datum 解码入口。
    /// 解码为旧 datum 字节数组，不使用 handle 补列。
    pub fn DecodeToBytesNoHandle(
        &self,
        outputOffset: &HashMap<i64, usize>,
        value: &[u8],
    ) -> Result<Vec<Vec<u8>>, errors::SharedError> {
        self.decodeToBytesInternal(outputOffset, None, value, &[])
    }

    // DecodeToBytes decodes raw byte slice to row data.
    // DecodeToBytes 带 handle 和缓存缓冲区，允许缺列时从主键 handle 补旧 datum 字节。
    /// 解码为旧 datum 字节数组；缺列时可从 handle 补主键值。
    pub fn DecodeToBytes(
        &self,
        outputOffset: &HashMap<i64, usize>,
        handle: &dyn kv::Handle,
        value: &[u8],
        cacheBytes: &[u8],
    ) -> Result<Vec<Vec<u8>>, errors::SharedError> {
        self.decodeToBytesInternal(outputOffset, Some(handle), value, cacheBytes)
    }

    // encodeOldDatum 把 rowcodec 内部列字节转换为旧 datum flag+payload 形式。
    // 对 Bytes/Int/Uint 分支使用旧 codec 的紧凑编码；其它类型直接拼接原 flag 和 payload。
    fn encodeOldDatum(&self, tp: u8, val: &[u8]) -> Vec<u8> {
        // Go 使用 binary.MaxVarintLen64 预估容量；用 10 表示 uint64 varint 最大长度。
        let mut buf = Vec::with_capacity(1 + 10 + val.len());
        match tp {
            BytesFlag => {
                buf.push(CompactBytesFlag);
                buf = codec::EncodeCompactBytes(buf, val);
            }
            IntFlag => {
                buf.push(VarintFlag);
                buf = codec::EncodeVarint(buf, decodeInt(val));
            }
            UintFlag => {
                buf.push(VaruintFlag);
                buf = codec::EncodeUvarint(buf, decodeUint(val));
            }
            _ => {
                buf.push(tp);
                buf.extend_from_slice(val);
            }
        }
        buf
    }
}

// fieldType2Flag transforms field type into kv type flag.
// fieldType2Flag 将 MySQL 字段类型映射为旧 datum 的首字节 flag，signed 参数保持 Go 中“是否有符号”的含义。
/// 将 MySQL 字段类型映射为旧 datum 首字节 flag；`signed` 表示有符号整数。
pub fn fieldType2Flag(tp: u8, signed: bool) -> u8 {
    match tp {
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong => {
            if signed {
                IntFlag
            } else {
                UintFlag
            }
        }
        mysql::TypeFloat | mysql::TypeDouble => FloatFlag,
        mysql::TypeBlob
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob
        | mysql::TypeString
        | mysql::TypeVarchar
        | mysql::TypeVarString => BytesFlag,
        mysql::TypeDatetime | mysql::TypeDate | mysql::TypeTimestamp => UintFlag,
        mysql::TypeDuration => IntFlag,
        mysql::TypeNewDecimal => DecimalFlag,
        mysql::TypeYear => IntFlag,
        mysql::TypeEnum | mysql::TypeBit | mysql::TypeSet => UintFlag,
        mysql::TypeJSON => JSONFlag,
        mysql::TypeTiDBVectorFloat32 => VectorFloat32Flag,
        mysql::TypeNull => NilFlag,
        _ => {
            // Go 这里 panic(fmt.Sprintf(...))，表示未知字段类型是调用方错误而不是普通解码错误。
            panic!("unknown field type {}", tp);
        }
    }
}
