// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Datum / Chunk 的通用编解码与哈希工具。
//
// 对应 Go `pkg/util/codec/codec.go`。编码以首字节 flag 标明类型（int/bytes/decimal/JSON 等），
// 支持 memcomparable 的 Key 编码与紧凑 Value 编码；并提供 Join 用的 SerializeKeys、
// 聚合用的 HashGroupKey，以及把编码结果直接写入 Chunk 的 `Decoder`。
// Collation（字符序）影响字符串 key/hash 的生成。

// First byte in the encoded value which specifies the encoding type.
// 以下 flag 保持 Go 文件的首字节编码取值；其它文件会按这些值识别 Datum 类型。
pub const NilFlag: u8 = 0;
const bytesFlag: u8 = 1;
const compactBytesFlag: u8 = 2;
const intFlag: u8 = 3;
const uintFlag: u8 = 4;
const floatFlag: u8 = 5;
const decimalFlag: u8 = 6;
const durationFlag: u8 = 7;
const varintFlag: u8 = 8;
const uvarintFlag: u8 = 9;
const jsonFlag: u8 = 10;
const vectorFloat32Flag: u8 = 20;
const maxFlag: u8 = 250;

// IntHandleFlag is only used to encode int handle key.
// IntHandleFlag 对应 Go 的导出常量，专门复用 intFlag 编码 int handle key。
pub const IntHandleFlag: u8 = intFlag;

// Go 通过 unsafe.Sizeof 取得固定宽度类型字节数；直接写死，含义相同。
const sizeUint64: usize = 8;
const sizeUint8: usize = 1;
const sizeUint32: usize = 4;
const sizeFloat64: usize = 8;

/// 将任意 Display 错误包装为 SharedError。
fn shared_error(error: impl std::fmt::Display) -> errors::SharedError {
    errors::New(error.to_string())
}

// Encoder encodes Datum values with a fixed new collation setting.
// Encoder 对应 Go 的编码器，只保存是否启用新 collation 的开关。
pub struct Encoder {
    useNewCollate: bool,
}

// NewEncoder creates an Encoder with the given new collation setting.
// NewEncoder 保留 Go 构造函数语义：固定本次编码所使用的新 collation 设置。
pub fn NewEncoder(useNewCollate: bool) -> Encoder {
    Encoder { useNewCollate }
}

impl Encoder {
    // UseNewCollate returns whether the encoder is using new collation.
    /// 返回是否启用新 collation。
    pub fn UseNewCollate(&self) -> bool {
        self.useNewCollate
    }

    // encode will encode a datum and append it to a byte slice. If comparable1 is true, the encoded bytes can be sorted as it's original order.
    // If hash is true, the encoded bytes can be checked equal as it's original value.
    // encode 是 Go 的核心 Datum 列表编码方法；这里保留 Kind 分发和错误返回路径。
    fn encode(
        &self,
        loc: time::Location,
        mut b: Vec<u8>,
        vals: Vec<types::Datum>,
        comparable1: bool,
    ) -> Result<Vec<u8>, errors::SharedError> {
        b = preRealloc(b, &vals, comparable1);
        for val in vals {
            match val.Kind() {
                types::KindInt64 => b = encodeSignedInt(b, val.GetInt64(), comparable1),
                types::KindUint64 => b = encodeUnsignedInt(b, val.GetUint64(), comparable1),
                types::KindFloat32 | types::KindFloat64 => {
                    b.push(floatFlag);
                    b = EncodeFloat(b, val.GetFloat64());
                }
                types::KindString => b = self.encodeString(b, val, comparable1),
                types::KindBytes => b = encodeBytes(b, val.GetBytes(), comparable1),
                types::KindMysqlTime => {
                    b.push(uintFlag);
                    // timestamp 编码需要按 Go 逻辑先转 UTC；EncodeMySQLTime 内保留该时区处理。
                    b = EncodeMySQLTime(loc, val.GetMysqlTime(), mysql::TypeUnspecified, b)?;
                }
                types::KindMysqlDuration => {
                    // duration may have negative value, so we cannot use String to encode directly.
                    b.push(durationFlag);
                    b = EncodeInt(b, val.GetMysqlDuration().Duration as i64);
                }
                types::KindMysqlDecimal => {
                    b.push(decimalFlag);
                    b = EncodeDecimal(b, &val.GetMysqlDecimal(), val.Length(), val.Frac())?;
                }
                types::KindMysqlEnum => {
                    b = encodeUnsignedInt(b, val.GetMysqlEnum().Value, comparable1)
                }
                types::KindMysqlSet => {
                    b = encodeUnsignedInt(b, val.GetMysqlSet().Value, comparable1)
                }
                types::KindMysqlBit | types::KindBinaryLiteral => {
                    // Go 认为 BinaryLiteral 在 convertToMysqlBit 里已保证可转 uint64；错误只记录。
                    let val_u64 = val
                        .GetBinaryLiteral()
                        .ToInt(types::StrictContext.clone())
                        .unwrap_or_default();
                    b = encodeUnsignedInt(b, val_u64, comparable1);
                }
                types::KindMysqlJSON => {
                    b.push(jsonFlag);
                    let j = val.GetMysqlJSON();
                    b.push(j.TypeCode);
                    b.extend_from_slice(&j.Value);
                }
                types::KindVectorFloat32 => {
                    // Always do a small deser + ser for sanity check.
                    b.push(vectorFloat32Flag);
                    b = val.GetVectorFloat32().SerializeTo(b);
                }
                types::KindNull => b.push(NilFlag),
                types::KindMinNotNull => b.push(bytesFlag),
                types::KindMaxValue => b.push(maxFlag),
                _ => {
                    return Err(errors::Errorf(format!(
                        "unsupport encode type {}",
                        val.Kind()
                    )));
                }
            }
        }
        Ok(b)
    }

    // EncodeKey appends the encoded values to byte slice b using the encoder's fixed collation setting.
    // Rust 没有 Go variadic；这里用 Vec<Datum> 表示 v ...types.Datum。
    pub fn EncodeKey(
        &self,
        loc: time::Location,
        b: Vec<u8>,
        v: Vec<types::Datum>,
    ) -> Result<Vec<u8>, errors::SharedError> {
        self.encode(loc, b, v, true)
    }

    // EncodeValue appends the encoded values to byte slice b using the encoder's fixed collation setting.
    pub fn EncodeValue(
        &self,
        loc: time::Location,
        b: Vec<u8>,
        v: Vec<types::Datum>,
    ) -> Result<Vec<u8>, errors::SharedError> {
        self.encode(loc, b, v, false)
    }

    // encodeString 对应 Go 方法：新 collation + comparable 编码时使用 immutable collation key。
    fn encodeString(&self, b: Vec<u8>, val: types::Datum, comparable1: bool) -> Vec<u8> {
        if self.useNewCollate && comparable1 {
            return encodeBytes(
                b,
                collate::GetCollatorWithCollate(self.useNewCollate, &val.Collation())
                    .ImmutableKey(&val.GetString()),
                true,
            );
        }
        encodeBytes(b, val.GetBytes(), comparable1)
    }

    // HashCode encodes a Datum into a unique byte slice using the encoder's fixed collation setting.
    // 该函数刻意避开 EncodeValue 的截断/校验逻辑，使哈希输入尽量无损。
    pub fn HashCode(&self, mut b: Vec<u8>, d: types::Datum) -> Vec<u8> {
        match d.Kind() {
            types::KindInt64 => b = encodeSignedInt(b, d.GetInt64(), false),
            types::KindUint64 => b = encodeUnsignedInt(b, d.GetUint64(), false),
            types::KindFloat32 | types::KindFloat64 => {
                b.push(floatFlag);
                b = EncodeFloat(b, d.GetFloat64());
            }
            types::KindString => b = self.encodeString(b, d, false),
            types::KindBytes => b = encodeBytes(b, d.GetBytes(), false),
            types::KindMysqlTime => {
                b.push(uintFlag);
                let t = d.GetMysqlTime().CoreTime();
                b = encodeUnsignedInt(b, t.0, true);
            }
            types::KindMysqlDuration => {
                // duration may have negative value, so we cannot use String to encode directly.
                b.push(durationFlag);
                b = EncodeInt(b, d.GetMysqlDuration().Duration as i64);
            }
            types::KindMysqlDecimal => {
                b.push(decimalFlag);
                let dec_str = d.GetMysqlDecimal().ToString();
                b = encodeBytes(b, dec_str, false);
            }
            types::KindMysqlEnum => b = encodeUnsignedInt(b, d.GetMysqlEnum().Value, false),
            types::KindMysqlSet => b = encodeUnsignedInt(b, d.GetMysqlSet().Value, false),
            types::KindMysqlBit | types::KindBinaryLiteral => {
                b = encodeBytes(b, d.GetBinaryLiteral().0, false);
            }
            types::KindMysqlJSON => {
                b.push(jsonFlag);
                let j = d.GetMysqlJSON();
                b.push(j.TypeCode);
                b.extend_from_slice(&j.Value);
            }
            types::KindVectorFloat32 => {
                b.push(vectorFloat32Flag);
                b = d.GetVectorFloat32().SerializeTo(b);
            }
            types::KindNull => b.push(NilFlag),
            types::KindMinNotNull => b.push(bytesFlag),
            types::KindMaxValue => b.push(maxFlag),
            _ => {
                // Go 这里记录 Warn 和 stack，不返回错误；同样保持“尽力记录后返回已有 b”的语义。
                logutil::BgLogger()
                    .warn("trying to calculate HashCode of an unexpected type of Datum");
            }
        }
        b
    }
}

// preRealloc 对应 Go 预估编码后容量的辅助函数；无法判断的类型直接返回原 buffer。
fn preRealloc(mut b: Vec<u8>, vals: &[types::Datum], comparable1: bool) -> Vec<u8> {
    let mut size = 0usize;
    for val in vals {
        match val.Kind() {
            types::KindInt64
            | types::KindUint64
            | types::KindMysqlEnum
            | types::KindMysqlSet
            | types::KindMysqlBit
            | types::KindBinaryLiteral => size += sizeInt(comparable1),
            types::KindString | types::KindBytes => size += sizeBytes(val.GetBytes(), comparable1),
            types::KindMysqlTime
            | types::KindMysqlDuration
            | types::KindFloat32
            | types::KindFloat64 => size += 9,
            types::KindNull | types::KindMinNotNull | types::KindMaxValue => size += 1,
            types::KindMysqlJSON => size += 2 + val.GetBytes().len(),
            types::KindVectorFloat32 => size += 1 + val.GetVectorFloat32().SerializedSize(),
            types::KindMysqlDecimal => size += 1 + types::MyDecimalStructSize,
            _ => return b,
        }
    }
    b.reserve(size);
    b
}

// EstimateValueSize uses to estimate the value size of the encoded values.
// EstimateValueSize 保留 Go 对单个 Datum 的编码尺寸估算；decimal 分支会透传尺寸计算错误。
pub fn EstimateValueSize(
    typeCtx: types::Context,
    val: types::Datum,
) -> Result<usize, errors::SharedError> {
    let l = match val.Kind() {
        types::KindInt64 => valueSizeOfSignedInt(val.GetInt64()),
        types::KindUint64 => valueSizeOfUnsignedInt(val.GetUint64()),
        types::KindFloat32
        | types::KindFloat64
        | types::KindMysqlTime
        | types::KindMysqlDuration => 9,
        types::KindString | types::KindBytes => valueSizeOfBytes(val.GetBytes()),
        types::KindMysqlDecimal => {
            valueSizeOfDecimal(&val.GetMysqlDecimal(), val.Length(), val.Frac())? + 1
        }
        types::KindMysqlEnum => valueSizeOfUnsignedInt(val.GetMysqlEnum().Value),
        types::KindMysqlSet => valueSizeOfUnsignedInt(val.GetMysqlSet().Value),
        types::KindMysqlBit | types::KindBinaryLiteral => {
            let v = val.GetBinaryLiteral().ToInt(typeCtx).unwrap_or_default();
            valueSizeOfUnsignedInt(v)
        }
        types::KindMysqlJSON => 2 + val.GetMysqlJSON().Value.len(),
        types::KindVectorFloat32 => 1 + val.GetVectorFloat32().SerializedSize(),
        types::KindNull | types::KindMinNotNull | types::KindMaxValue => 1,
        _ => {
            return Err(errors::Errorf(format!(
                "unsupported encode type {}",
                val.Kind()
            )));
        }
    };
    Ok(l)
}

// EncodeMySQLTime encodes datum of `KindMysqlTime` to []byte.
/// 将 MySQL 时间类型编码为可比较的 uint 打包格式。
pub fn EncodeMySQLTime(
    loc: time::Location,
    mut t: types::Time,
    mut tp: u8,
    b: Vec<u8>,
) -> Result<Vec<u8>, errors::SharedError> {
    // Encoding timestamp need to consider timezone. If it's not in UTC, transform to UTC first.
    // 这里保留 Go 与 coprocessor 协议兼容的 UTC 转换分支。
    if tp == mysql::TypeUnspecified {
        tp = t.Type();
    }
    if tp == mysql::TypeTimestamp && loc != time::UTC {
        t.ConvertTimeZone(loc, time::UTC)
            .map_err(|error| errors::New(error.to_string()))?;
    }
    let v = t
        .ToPackedUint()
        .map_err(|error| errors::New(error.to_string()))?;
    Ok(EncodeUint(b, v))
}

// encodeBytes 按 comparable 开关选择 mem-comparable bytes 或 compact bytes 编码。
fn encodeBytes(mut b: Vec<u8>, v: Vec<u8>, comparable1: bool) -> Vec<u8> {
    if comparable1 {
        b.push(bytesFlag);
        b = EncodeBytes(b, &v);
    } else {
        b.push(compactBytesFlag);
        b = EncodeCompactBytes(b, &v);
    }
    b
}

/// Compact bytes 载荷尺寸：变长长度前缀 + 数据。
fn valueSizeOfBytes(v: Vec<u8>) -> usize {
    valueSizeOfSignedInt(v.len() as i64) + v.len()
}

/// 预估 bytes 编码占用（comparable 走 memcomparable 分组）。
fn sizeBytes(v: Vec<u8>, comparable1: bool) -> usize {
    if comparable1 {
        // EncodeBytes 按 8 字节分组，每组额外 1 个 marker。
        return 1 + (v.len() / encGroupSize + 1) * (encGroupSize + 1);
    }
    1 + binary::MaxVarintLen64 + v.len()
}

/// 有符号整数：comparable 用定长 int，否则用 varint。
fn encodeSignedInt(mut b: Vec<u8>, v: i64, comparable1: bool) -> Vec<u8> {
    if comparable1 {
        b.push(intFlag);
        EncodeInt(b, v)
    } else {
        b.push(varintFlag);
        EncodeVarint(b, v)
    }
}

/// 估算有符号整数 varint 编码尺寸（含 flag）。
fn valueSizeOfSignedInt(mut v: i64) -> usize {
    if v < 0 {
        v = 0 - v - 1;
    }
    // flag occupy 1 bit and at lease 1 bit.
    let mut size = 2usize;
    v >>= 6;
    while v > 0 {
        size += 1;
        v >>= 7;
    }
    size
}

/// 无符号整数：comparable 用定长 uint，否则用 uvarint。
fn encodeUnsignedInt(mut b: Vec<u8>, v: u64, comparable1: bool) -> Vec<u8> {
    if comparable1 {
        b.push(uintFlag);
        EncodeUint(b, v)
    } else {
        b.push(uvarintFlag);
        EncodeUvarint(b, v)
    }
}

/// 估算无符号整数 uvarint 编码尺寸（含 flag）。
fn valueSizeOfUnsignedInt(mut v: u64) -> usize {
    // flag occupy 1 bit and at lease 1 bit.
    let mut size = 2usize;
    v >>= 7;
    while v > 0 {
        size += 1;
        v >>= 7;
    }
    size
}

/// 预估整数编码占用：定长 9 或 1+MaxVarintLen。
fn sizeInt(comparable1: bool) -> usize {
    if comparable1 {
        9
    } else {
        1 + binary::MaxVarintLen64
    }
}

// EncodeKey appends the encoded values to byte slice b, returns the appended slice.
// 包级 EncodeKey 使用当前全局新 collation 开关构造 Encoder。
pub fn EncodeKey(
    loc: time::Location,
    b: Vec<u8>,
    v: Vec<types::Datum>,
) -> Result<Vec<u8>, errors::SharedError> {
    NewEncoder(collate::NewCollationEnabled()).EncodeKey(loc, b, v)
}

// EncodeValue appends the encoded values to byte slice b, returning the appended slice.
/// 包级 EncodeValue：使用当前全局新 collation 开关。
pub fn EncodeValue(
    loc: time::Location,
    b: Vec<u8>,
    v: Vec<types::Datum>,
) -> Result<Vec<u8>, errors::SharedError> {
    NewEncoder(collate::NewCollationEnabled()).EncodeValue(loc, b, v)
}

// EncodeHashChunkRowIdx encodes value for further comparison.
// 该函数保留 Go 针对 chunk.Row 单列生成 hash flag 与 payload 的分支。
pub fn EncodeHashChunkRowIdx(
    typeCtx: types::Context,
    row: chunk::Row,
    tp: *mut types::FieldType,
    idx: usize,
) -> Result<(u8, Vec<u8>), errors::SharedError> {
    if row.IsNull(idx) {
        return Ok((NilFlag, Vec::new()));
    }
    let flag_and_bytes = match unsafe { (*tp).GetType() } {
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeYear => {
            let mut flag = uvarintFlag;
            if !mysql::HasUnsignedFlag(unsafe { (*tp).GetFlag() }) && row.GetInt64(idx) < 0 {
                flag = varintFlag;
            }
            (flag, row.GetRaw(idx))
        }
        mysql::TypeFloat => {
            // Go 使用 unsafe.Slice 直接读取 f64 内存；用 to_ne_bytes 复制，保留负零规范化分支。
            let mut f = row.GetFloat32(idx) as f64;
            if f == 0.0 {
                f = 0.0;
            }
            (floatFlag, f.to_ne_bytes().to_vec())
        }
        mysql::TypeDouble => {
            let mut f = row.GetFloat64(idx);
            if f == 0.0 {
                f = 0.0;
            }
            (floatFlag, f.to_ne_bytes().to_vec())
        }
        mysql::TypeVarchar
        | mysql::TypeVarString
        | mysql::TypeString
        | mysql::TypeBlob
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob => (
            compactBytesFlag,
            ConvertByCollation(row.GetBytes(idx), unsafe { &*tp }),
        ),
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
            let v = row.GetTime(idx).ToPackedUint().map_err(shared_error)?;
            (uintFlag, v.to_ne_bytes().to_vec())
        }
        mysql::TypeDuration => {
            // duration may have negative value, so we cannot use String to encode directly.
            (durationFlag, row.GetRaw(idx))
        }
        mysql::TypeNewDecimal => {
            // If hash is true, we only consider the original value of this decimal and ignore it's precision.
            (
                decimalFlag,
                row.GetMyDecimal(idx).ToHashKey().map_err(shared_error)?,
            )
        }
        mysql::TypeEnum => encode_hash_enum(row, tp, idx),
        mysql::TypeSet => {
            let s = types::ParseSetValue(unsafe { (*tp).GetElems() }, row.GetSet(idx).Value)
                .map_err(shared_error)?;
            (
                compactBytesFlag,
                ConvertByCollation(s.Name.into_bytes(), unsafe { &*tp }),
            )
        }
        mysql::TypeBit => {
            let v = types::BinaryLiteral(row.GetBytes(idx))
                .ToInt(typeCtx)
                .unwrap_or_default();
            (uvarintFlag, v.to_ne_bytes().to_vec())
        }
        mysql::TypeJSON => (jsonFlag, row.GetJSON(idx).HashValue(Vec::new())),
        mysql::TypeTiDBVectorFloat32 => (
            vectorFloat32Flag,
            row.GetVectorFloat32(idx).SerializeTo(Vec::new()),
        ),
        _ => {
            return Err(errors::Errorf(format!(
                "unsupport column type for encode {}",
                unsafe { (*tp).GetType() }
            )));
        }
    };
    Ok(flag_and_bytes)
}

/// Enum 列哈希编码：可作为整数或按枚举名字符串（经 collation）编码。
fn encode_hash_enum(row: chunk::Row, tp: *mut types::FieldType, idx: usize) -> (u8, Vec<u8>) {
    if mysql::HasEnumSetAsIntFlag(unsafe { (*tp).GetFlag() }) {
        let v = row.GetEnum(idx).Value;
        return (uvarintFlag, v.to_ne_bytes().to_vec());
    }
    let v = row.GetEnum(idx).Value;
    let mut str_value = String::new();
    if let Ok(enum_value) = types::ParseEnumValue(unsafe { (*tp).GetElems() }, v) {
        // str will be empty string if v out of definition of enum.
        str_value = enum_value.Name;
    }
    (
        compactBytesFlag,
        ConvertByCollation(str_value.into_bytes(), unsafe { &*tp }),
    )
}

// SerializeMode is for some special cases during serialize key.
// SerializeMode 保留 Go iota 枚举，用于 join key 序列化时的符号位和变长长度策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerializeMode {
    // Normal means serialize in the normal way.
    Normal,
    // NeedSignFlag 表示整数列比较需要显式保留 signed/unsigned flag。
    NeedSignFlag,
    // KeepVarColumnLength 表示变长列前需要记录长度。
    KeepVarColumnLength,
}

// preAllocForSerializedKeyBuffer 先计算每行 key 长度，再一次性分配共享 buffer。
fn preAllocForSerializedKeyBuffer(
    buildKeyIndexs: Vec<usize>,
    chk: *mut chunk::Chunk,
    tps: Vec<*mut types::FieldType>,
    usedRows: Vec<usize>,
    filterVector: Option<Vec<bool>>,
    nullVector: &mut [bool],
    serializeModes: Vec<SerializeMode>,
    serializedKeys: &mut Vec<Vec<u8>>,
    serializedKeyLens: &mut Vec<usize>,
    mut serializedKeysBuffer: Vec<u8>,
) -> Result<Vec<u8>, errors::SharedError> {
    for (i, idx) in buildKeyIndexs.iter().enumerate() {
        let column = unsafe { (*chk).Column(*idx) };
        // Go 的 canSkip 闭包会顺手标记 nullVector；这里保留该副作用。
        let mut can_skip = |index: usize| {
            if column.IsNull(index) {
                nullVector[index] = true;
            }
            filterVector.as_ref().map_or(false, |v| !v[index]) || nullVector[index]
        };

        match unsafe { (*tps[i]).GetType() } {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeYear => {
                let flag_byte_num =
                    matches!(serializeModes[i], SerializeMode::NeedSignFlag) as usize;
                for (j, physical_row_index) in usedRows.iter().enumerate() {
                    if can_skip(*physical_row_index) {
                        continue;
                    }
                    serializedKeyLens[j] += flag_byte_num + 8;
                }
            }
            mysql::TypeFloat | mysql::TypeDouble => {
                add_fixed_len_for_rows(&usedRows, serializedKeyLens, &mut can_skip, sizeFloat64)
            }
            mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
                add_fixed_len_for_rows(&usedRows, serializedKeyLens, &mut can_skip, sizeUint64)
            }
            mysql::TypeDuration => {
                add_fixed_len_for_rows(&usedRows, serializedKeyLens, &mut can_skip, 8)
            }
            mysql::TypeVarchar
            | mysql::TypeVarString
            | mysql::TypeString
            | mysql::TypeBlob
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob => {
                let collator = collate::GetCollator(unsafe { (*tps[i]).GetCollate() });
                let size_byte_num =
                    if matches!(serializeModes[i], SerializeMode::KeepVarColumnLength) {
                        sizeUint32
                    } else {
                        0
                    };
                for (j, physical_row_index) in usedRows.iter().enumerate() {
                    if can_skip(*physical_row_index) {
                        continue;
                    }
                    let bytes = column.GetBytes(*physical_row_index);
                    let value = String::from_utf8_lossy(&bytes);
                    let str_len = collator.MaxKeyLen(&value) as usize;
                    serializedKeyLens[j] += size_byte_num + str_len;
                }
            }
            mysql::TypeNewDecimal => {
                let size_byte_num =
                    if matches!(serializeModes[i], SerializeMode::KeepVarColumnLength) {
                        sizeUint32
                    } else {
                        0
                    };
                let ds = column.Decimals();
                for (j, physical_row_index) in usedRows.iter().enumerate() {
                    if can_skip(*physical_row_index) {
                        continue;
                    }
                    serializedKeyLens[j] += ds[*physical_row_index]
                        .HashKeySize()
                        .map_err(shared_error)?
                        + size_byte_num;
                }
            }
            mysql::TypeEnum => prealloc_enum_key(
                column,
                unsafe { &*tps[i] },
                &usedRows,
                serializedKeyLens,
                &mut can_skip,
                &serializeModes[i],
            ),
            mysql::TypeSet => prealloc_set_key(
                column,
                unsafe { &*tps[i] },
                &usedRows,
                serializedKeyLens,
                &mut can_skip,
                &serializeModes[i],
            )?,
            mysql::TypeBit => {
                let sign_flag_len = if matches!(serializeModes[i], SerializeMode::NeedSignFlag) {
                    size::SizeOfByte as usize
                } else {
                    0
                };
                add_fixed_len_for_rows(
                    &usedRows,
                    serializedKeyLens,
                    &mut can_skip,
                    sign_flag_len + sizeUint64,
                );
            }
            mysql::TypeJSON => {
                let size_byte_num =
                    if matches!(serializeModes[i], SerializeMode::KeepVarColumnLength) {
                        sizeUint32
                    } else {
                        0
                    };
                for (j, physical_row_index) in usedRows.iter().enumerate() {
                    if can_skip(*physical_row_index) {
                        continue;
                    }
                    serializedKeyLens[j] += size_byte_num
                        + column.GetJSON(*physical_row_index).CalculateHashValueSize() as usize;
                }
            }
            mysql::TypeNull => {}
            _ => {
                return Err(errors::Errorf(format!(
                    "unsupport column type for pre-alloc {}",
                    unsafe { (*tps[i]).GetType() }
                )));
            }
        }
    }

    let total_mem_usage: usize = serializedKeyLens.iter().sum();
    serializedKeysBuffer.resize(total_mem_usage, 0);
    for i in 0..serializedKeys.len() {
        let row_len = serializedKeyLens[i];
        // Go 使用 `buffer[start:start:start+rowLen]`：长度为 0，容量为 rowLen。
        // Rust 的独立 Vec 无法借用共享 buffer，同时保持相同的追加语义与容量约束。
        serializedKeys[i] = Vec::with_capacity(row_len);
    }
    Ok(serializedKeysBuffer)
}

/// 为未跳过的行累加固定长度字段占用。
fn add_fixed_len_for_rows<F: FnMut(usize) -> bool>(
    usedRows: &[usize],
    serializedKeyLens: &mut [usize],
    can_skip: &mut F,
    elem_len: usize,
) {
    for (j, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        serializedKeyLens[j] += elem_len;
    }
}

/// 预估 Enum 列在 join key 中的字节长度。
fn prealloc_enum_key<F: FnMut(usize) -> bool>(
    column: &chunk::Column,
    tp: &types::FieldType,
    usedRows: &[usize],
    serializedKeyLens: &mut [usize],
    can_skip: &mut F,
    serializeMode: &SerializeMode,
) {
    if mysql::HasEnumSetAsIntFlag(tp.GetFlag()) {
        let mut elem_len = sizeUint64;
        if matches!(serializeMode, SerializeMode::NeedSignFlag) {
            elem_len += size::SizeOfByte as usize;
        }
        add_fixed_len_for_rows(usedRows, serializedKeyLens, can_skip, elem_len);
        return;
    }
    let size_byte_num = if matches!(serializeMode, SerializeMode::KeepVarColumnLength) {
        sizeUint32
    } else {
        0
    };
    let collator = collate::GetCollator(tp.GetCollate());
    for (j, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        let v = column.GetEnum(*physical_row_index).Value;
        let str_value = types::ParseEnumValue(tp.GetElems(), v)
            .map(|e| e.Name)
            .unwrap_or_default();
        serializedKeyLens[j] += size_byte_num + collator.MaxKeyLen(&str_value) as usize;
    }
}

/// 预估 Set 列在 join key 中的字节长度。
fn prealloc_set_key<F: FnMut(usize) -> bool>(
    column: &chunk::Column,
    tp: &types::FieldType,
    usedRows: &[usize],
    serializedKeyLens: &mut [usize],
    can_skip: &mut F,
    serializeMode: &SerializeMode,
) -> Result<(), errors::SharedError> {
    let size_byte_num = if matches!(serializeMode, SerializeMode::KeepVarColumnLength) {
        sizeUint32
    } else {
        0
    };
    let collator = collate::GetCollator(tp.GetCollate());
    for (j, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        let s = types::ParseSetValue(tp.GetElems(), column.GetSet(*physical_row_index).Value)
            .map_err(shared_error)?;
        serializedKeyLens[j] += size_byte_num + collator.MaxKeyLen(&s.Name) as usize;
    }
    Ok(())
}

// serializeKeysImpl 对应 Go 的第二阶段序列化：逐列把每行 payload 写入预分配 key buffer。
fn serializeKeysImpl(
    typeCtx: types::Context,
    chk: *mut chunk::Chunk,
    tps: Vec<*mut types::FieldType>,
    buildKeyIndexs: Vec<usize>,
    usedRows: Vec<usize>,
    filterVector: Option<Vec<bool>>,
    nullVector: Option<&[bool]>,
    serializeModes: Vec<SerializeMode>,
    serializedKeys: &mut Vec<Vec<u8>>,
) -> Result<(), errors::SharedError> {
    let can_skip = |index: usize| {
        filterVector.as_ref().map_or(false, |v| !v[index])
            || nullVector.as_ref().map_or(false, |v| v[index])
    };

    for (i, idx) in buildKeyIndexs.iter().enumerate() {
        let column = unsafe { (*chk).Column(*idx) };
        let serializeMode = &serializeModes[i];
        let tp = unsafe { &*tps[i] };
        match tp.GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeYear => {
                let i64s = column.Int64s();
                for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
                    if can_skip(*physical_row_index) {
                        continue;
                    }
                    if matches!(serializeMode, SerializeMode::NeedSignFlag) {
                        let flag = if !mysql::HasUnsignedFlag(tp.GetFlag())
                            && i64s[*physical_row_index] < 0
                        {
                            intFlag
                        } else {
                            uintFlag
                        };
                        serializedKeys[logical_row_index].push(flag);
                    }
                    serializedKeys[logical_row_index]
                        .extend_from_slice(&column.GetRaw(*physical_row_index));
                }
            }
            mysql::TypeFloat | mysql::TypeDouble => {
                serialize_float_columns(column, &usedRows, &can_skip, serializedKeys, tp.GetType());
            }
            mysql::TypeVarchar
            | mysql::TypeVarString
            | mysql::TypeString
            | mysql::TypeBlob
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob => {
                serialize_string_columns(
                    column,
                    &usedRows,
                    &can_skip,
                    serializedKeys,
                    tp,
                    serializeMode,
                );
            }
            mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
                let ts = column.Times();
                for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
                    if can_skip(*physical_row_index) {
                        continue;
                    }
                    let v = ts[*physical_row_index]
                        .ToPackedUint()
                        .map_err(shared_error)?;
                    serializedKeys[logical_row_index].extend_from_slice(&v.to_ne_bytes());
                }
            }
            mysql::TypeDuration => {
                for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
                    if can_skip(*physical_row_index) {
                        continue;
                    }
                    serializedKeys[logical_row_index]
                        .extend_from_slice(&column.GetRaw(*physical_row_index));
                }
            }
            mysql::TypeNewDecimal => serialize_decimal_columns(
                column,
                &usedRows,
                &can_skip,
                serializedKeys,
                serializeMode,
            )?,
            mysql::TypeEnum => serialize_enum_columns(
                column,
                &usedRows,
                &can_skip,
                serializedKeys,
                tp,
                serializeMode,
            )?,
            mysql::TypeSet => serialize_set_columns(
                column,
                &usedRows,
                &can_skip,
                serializedKeys,
                tp,
                serializeMode,
            )?,
            mysql::TypeBit => serialize_bit_columns(
                &typeCtx,
                column,
                &usedRows,
                &can_skip,
                serializedKeys,
                serializeMode,
            ),
            mysql::TypeJSON => {
                serialize_json_columns(column, &usedRows, &can_skip, serializedKeys, serializeMode)
            }
            mysql::TypeNull => {}
            _ => {
                return Err(errors::Errorf(format!(
                    "unsupport column type for encode {}",
                    tp.GetType()
                )));
            }
        }
    }
    Ok(())
}

/// 将 float/double 列序列化进各行 key buffer。
fn serialize_float_columns<F: Fn(usize) -> bool>(
    column: &chunk::Column,
    usedRows: &[usize],
    can_skip: &F,
    serializedKeys: &mut Vec<Vec<u8>>,
    tp: u8,
) {
    for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        // Go 为了让 -0 与 0 的 hash 一致，遇到零值时重新赋值为 +0。
        let mut f = if tp == mysql::TypeFloat {
            column.Float32s()[*physical_row_index] as f64
        } else {
            column.Float64s()[*physical_row_index]
        };
        if f == 0.0 {
            f = 0.0;
        }
        serializedKeys[logical_row_index].extend_from_slice(&f.to_ne_bytes());
    }
}

/// 将字符串/Blob 列按 collation 序列化进 key buffer。
fn serialize_string_columns<F: Fn(usize) -> bool>(
    column: &chunk::Column,
    usedRows: &[usize],
    can_skip: &F,
    serializedKeys: &mut Vec<Vec<u8>>,
    tp: &types::FieldType,
    serializeMode: &SerializeMode,
) {
    let collator = collate::GetCollator(tp.GetCollate());
    for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        let bytes = column.GetBytes(*physical_row_index);
        let value = String::from_utf8_lossy(&bytes);
        let data = collator.ImmutableKey(&value);
        if matches!(serializeMode, SerializeMode::KeepVarColumnLength) {
            serializedKeys[logical_row_index].extend_from_slice(&(data.len() as u32).to_ne_bytes());
        }
        serializedKeys[logical_row_index].extend_from_slice(&data);
    }
}

/// 将 decimal 列序列化进 key buffer。
fn serialize_decimal_columns<F: Fn(usize) -> bool>(
    column: &chunk::Column,
    usedRows: &[usize],
    can_skip: &F,
    serializedKeys: &mut Vec<Vec<u8>>,
    serializeMode: &SerializeMode,
) -> Result<(), errors::SharedError> {
    let ds = column.Decimals();
    for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        let b = ds[*physical_row_index].ToHashKey().map_err(shared_error)?;
        if matches!(serializeMode, SerializeMode::KeepVarColumnLength) {
            // for decimal, the size must be less than uint8.MAX, so use uint8 here.
            serializedKeys[logical_row_index].push(b.len() as u8);
        }
        serializedKeys[logical_row_index].extend_from_slice(&b);
    }
    Ok(())
}

/// 将 enum 列序列化进 key buffer。
fn serialize_enum_columns<F: Fn(usize) -> bool>(
    column: &chunk::Column,
    usedRows: &[usize],
    can_skip: &F,
    serializedKeys: &mut Vec<Vec<u8>>,
    tp: &types::FieldType,
    serializeMode: &SerializeMode,
) -> Result<(), errors::SharedError> {
    if mysql::HasEnumSetAsIntFlag(tp.GetFlag()) {
        for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
            if can_skip(*physical_row_index) {
                continue;
            }
            if matches!(serializeMode, SerializeMode::NeedSignFlag) {
                serializedKeys[logical_row_index].push(uintFlag);
            }
            serializedKeys[logical_row_index]
                .extend_from_slice(&column.GetEnum(*physical_row_index).Value.to_ne_bytes());
        }
        return Ok(());
    }
    let collator = collate::GetCollator(tp.GetCollate());
    for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        let str_value =
            types::ParseEnumValue(tp.GetElems(), column.GetEnum(*physical_row_index).Value)
                .map(|e| e.Name)
                .unwrap_or_default();
        let b = collator.ImmutableKey(&str_value);
        if matches!(serializeMode, SerializeMode::KeepVarColumnLength) {
            serializedKeys[logical_row_index].extend_from_slice(&(b.len() as u32).to_ne_bytes());
        }
        serializedKeys[logical_row_index].extend_from_slice(&b);
    }
    Ok(())
}

/// 将 set 列序列化进 key buffer。
fn serialize_set_columns<F: Fn(usize) -> bool>(
    column: &chunk::Column,
    usedRows: &[usize],
    can_skip: &F,
    serializedKeys: &mut Vec<Vec<u8>>,
    tp: &types::FieldType,
    serializeMode: &SerializeMode,
) -> Result<(), errors::SharedError> {
    let collator = collate::GetCollator(tp.GetCollate());
    for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        let s = types::ParseSetValue(tp.GetElems(), column.GetSet(*physical_row_index).Value)
            .map_err(shared_error)?;
        let b = collator.ImmutableKey(&s.Name);
        if matches!(serializeMode, SerializeMode::KeepVarColumnLength) {
            serializedKeys[logical_row_index].extend_from_slice(&(b.len() as u32).to_ne_bytes());
        }
        serializedKeys[logical_row_index].extend_from_slice(&b);
    }
    Ok(())
}

/// 将 bit 列序列化进 key buffer。
fn serialize_bit_columns<F: Fn(usize) -> bool>(
    typeCtx: &types::Context,
    column: &chunk::Column,
    usedRows: &[usize],
    can_skip: &F,
    serializedKeys: &mut Vec<Vec<u8>>,
    serializeMode: &SerializeMode,
) {
    for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        let v = types::BinaryLiteral(column.GetBytes(*physical_row_index).to_vec())
            .ToInt(typeCtx.clone())
            .unwrap_or_default();
        if matches!(serializeMode, SerializeMode::NeedSignFlag) {
            serializedKeys[logical_row_index].push(uintFlag);
        }
        serializedKeys[logical_row_index].extend_from_slice(&v.to_ne_bytes());
    }
}

/// 将 JSON 列哈希后序列化进 key buffer。
fn serialize_json_columns<F: Fn(usize) -> bool>(
    column: &chunk::Column,
    usedRows: &[usize],
    can_skip: &F,
    serializedKeys: &mut Vec<Vec<u8>>,
    serializeMode: &SerializeMode,
) {
    let mut jsonHashBuffer = Vec::new();
    for (logical_row_index, physical_row_index) in usedRows.iter().enumerate() {
        if can_skip(*physical_row_index) {
            continue;
        }
        jsonHashBuffer.clear();
        jsonHashBuffer = column
            .GetJSON(*physical_row_index)
            .HashValue(jsonHashBuffer);
        if matches!(serializeMode, SerializeMode::KeepVarColumnLength) {
            serializedKeys[logical_row_index]
                .extend_from_slice(&(jsonHashBuffer.len() as u32).to_ne_bytes());
        }
        serializedKeys[logical_row_index].extend_from_slice(&jsonHashBuffer);
    }
}

// SerializeKeys is used in join.
// Go 先预分配再序列化，并在测试环境校验每行 Vec 容量没有增长。
pub fn SerializeKeys(
    typeCtx: types::Context,
    chk: *mut chunk::Chunk,
    tps: Vec<*mut types::FieldType>,
    buildKeyIndexs: Vec<usize>,
    usedRows: Vec<usize>,
    filterVector: Option<Vec<bool>>,
    nullVector: &mut [bool],
    serializeModes: Vec<SerializeMode>,
    serializedKeys: &mut Vec<Vec<u8>>,
    serializedKeyLens: &mut Vec<usize>,
    serializedKeysBuffer: Vec<u8>,
) -> Result<Vec<u8>, errors::SharedError> {
    let serializedKeysBuffer = preAllocForSerializedKeyBuffer(
        buildKeyIndexs.clone(),
        chk,
        tps.clone(),
        usedRows.clone(),
        filterVector.clone(),
        nullVector,
        serializeModes.clone(),
        serializedKeys,
        serializedKeyLens,
        serializedKeysBuffer,
    )?;
    let caps_for_test = if intest::InTest.load(std::sync::atomic::Ordering::SeqCst) {
        serializedKeys
            .iter()
            .map(|v| v.capacity())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    serializeKeysImpl(
        typeCtx,
        chk,
        tps,
        buildKeyIndexs,
        usedRows,
        filterVector,
        Some(nullVector),
        serializeModes,
        serializedKeys,
    )?;
    if intest::InTest.load(std::sync::atomic::Ordering::SeqCst) {
        for (i, key) in serializedKeys.iter().enumerate() {
            if caps_for_test[i] < key.capacity() {
                panic!("Before: {}, After: {}", caps_for_test[i], key.capacity());
            }
        }
    }
    Ok(serializedKeysBuffer)
}

// HashChunkColumns writes the encoded value of each row's column, which of index `colIdx`, to h.
/// 对 Chunk 指定列的每一行写入哈希器（全选）。
pub fn HashChunkColumns(
    typeCtx: types::Context,
    h: &mut [Box<dyn std::hash::Hasher>],
    chk: *mut chunk::Chunk,
    tp: *mut types::FieldType,
    colIdx: usize,
    buf: Vec<u8>,
    isNull: &mut [bool],
) -> Result<(), errors::SharedError> {
    HashChunkSelected(typeCtx, h, chk, tp, colIdx, buf, isNull, None, false)
}

// HashChunkSelected writes the encoded value of selected row's column, which of index `colIdx`, to h.
// Go 的 Hash.Write 永不返回错误；仍忽略写入返回，保留原注释语义。
pub fn HashChunkSelected(
    typeCtx: types::Context,
    h: &mut [Box<dyn std::hash::Hasher>],
    chk: *mut chunk::Chunk,
    tp: *mut types::FieldType,
    colIdx: usize,
    mut buf: Vec<u8>,
    isNull: &mut [bool],
    sel: Option<Vec<bool>>,
    ignoreNull: bool,
) -> Result<(), errors::SharedError> {
    let column = unsafe { (*chk).Column(colIdx) };
    let rows = unsafe { (*chk).NumRows() };
    buf.resize(1, 0);
    for i in 0..rows {
        if sel.as_ref().map_or(false, |s| !s[i]) {
            continue;
        }
        let (flag, b) = if column.IsNull(i) {
            isNull[i] = !ignoreNull;
            (NilFlag, Vec::new())
        } else {
            // 这里借用 EncodeHashChunkRowIdx 的单行单列分支，避免复制 Go 中同一长 switch。
            EncodeHashChunkRowIdx(typeCtx.clone(), unsafe { (*chk).GetRow(i) }, tp, colIdx)?
        };
        buf[0] = flag;
        h[i].write(&buf);
        h[i].write(&b);
    }
    Ok(())
}

// HashChunkRow writes the encoded values to w.
// If two rows are logically equal, it will generate the same bytes.
/// 将一行多列编码写入 writer；逻辑相等的行生成相同字节。
pub fn HashChunkRow(
    typeCtx: types::Context,
    w: &mut dyn std::io::Write,
    row: chunk::Row,
    allTypes: Vec<*mut types::FieldType>,
    colIdx: Vec<usize>,
    mut buf: Vec<u8>,
) -> Result<(), errors::SharedError> {
    buf.resize(1, 0);
    for (i, idx) in colIdx.iter().enumerate() {
        let (flag, b) = EncodeHashChunkRowIdx(typeCtx.clone(), row.clone(), allTypes[i], *idx)?;
        buf[0] = flag;
        w.write_all(&buf).map_err(shared_error)?;
        w.write_all(&b).map_err(shared_error)?;
    }
    Ok(())
}

// EqualChunkRow returns a boolean reporting whether row1 and row2 with their types and column index are logically equal.
/// 比较两行在给定列上的逻辑相等性（基于哈希编码）。
pub fn EqualChunkRow(
    typeCtx: types::Context,
    row1: chunk::Row,
    allTypes1: Vec<*mut types::FieldType>,
    colIdx1: Vec<usize>,
    row2: chunk::Row,
    allTypes2: Vec<*mut types::FieldType>,
    colIdx2: Vec<usize>,
) -> Result<bool, errors::SharedError> {
    if colIdx1.len() != colIdx2.len() {
        return Err(errors::Errorf(format!(
            "Internal error: Hash columns count mismatch, col1: {}, col2: {}",
            colIdx1.len(),
            colIdx2.len()
        )));
    }
    for i in 0..colIdx1.len() {
        let (flag1, b1) =
            EncodeHashChunkRowIdx(typeCtx.clone(), row1.clone(), allTypes1[i], colIdx1[i])?;
        let (flag2, b2) =
            EncodeHashChunkRowIdx(typeCtx.clone(), row2.clone(), allTypes2[i], colIdx2[i])?;
        if flag1 != flag2 || b1 != b2 {
            return Ok(false);
        }
    }
    Ok(true)
}

// Decode decodes values from a byte slice generated with EncodeKey or EncodeValue before.
/// 解码 EncodeKey/EncodeValue 生成的字节序列为 Datum 列表。
pub fn Decode(b: Vec<u8>, size: usize) -> Result<Vec<types::Datum>, errors::SharedError> {
    if b.is_empty() {
        return Err(errors::New("invalid encoded key"));
    }
    let mut values = Vec::with_capacity(size);
    let mut remaining = b.as_slice();
    while !remaining.is_empty() {
        let (remain, d) = DecodeOne(remaining)?;
        values.push(d);
        remaining = remain;
    }
    Ok(values)
}

// DecodeRange decodes the range values from a byte slice that generated by EncodeKey.
// loc can be nil and only used in when the corresponding type is `mysql.TypeTimestamp`.
/// 解码范围扫描用的编码 key，处理末尾边界 flag。
pub fn DecodeRange(
    b: Vec<u8>,
    size: usize,
    idxColumnTypes: Option<Vec<u8>>,
    loc: time::Location,
) -> Result<(Vec<types::Datum>, Vec<u8>), errors::SharedError> {
    if b.is_empty() {
        return Err(errors::New("invalid encoded key: length of key is zero"));
    }
    let mut remaining = b.as_slice();
    let mut values = Vec::with_capacity(size);
    let mut i = 0usize;
    while remaining.len() > 1 {
        let decoded = if let Some(ref types_vec) = idxColumnTypes {
            if i >= types_vec.len() {
                return Err(errors::New("invalid length of index's columns"));
            }
            if types::IsTypeTime(types_vec[i]) {
                // handle datetime values specially since they are encoded to int and we'll get int values if using DecodeOne.
                DecodeAsDateTime(remaining, types_vec[i], loc)?
            } else if types::IsTypeFloat(types_vec[i]) {
                DecodeAsFloat32(remaining, types_vec[i])?
            } else {
                DecodeOne(remaining)?
            }
        } else {
            DecodeOne(remaining)?
        };
        remaining = decoded.0;
        values.push(decoded.1);
        i += 1;
    }
    if remaining.len() == 1 {
        match remaining[0] {
            NilFlag => values.push(types::Datum::default()),
            bytesFlag => values.push(types::MinNotNullDatum()),
            // `maxFlag + 1` for PrefixNext.
            x if x == maxFlag || x == maxFlag + 1 => values.push(types::MaxValueDatum()),
            _ => {
                return Err(errors::Errorf(format!(
                    "invalid encoded key flag {}",
                    remaining[0]
                )));
            }
        }
    }
    Ok((values, Vec::new()))
}

// DecodeOne decodes on datum from a byte slice generated with EncodeKey or EncodeValue.
/// 解码单个 Datum，返回剩余字节与值。
pub fn DecodeOne(b: &[u8]) -> Result<(&[u8], types::Datum), errors::SharedError> {
    if b.is_empty() {
        return Err(errors::New("invalid encoded key"));
    }
    let flag = b[0];
    let mut remaining = &b[1..];
    let mut d = types::Datum::default();
    match flag {
        intFlag => {
            let (remain, v) = DecodeInt(remaining)?;
            remaining = remain;
            d.SetInt64(v);
        }
        uintFlag => {
            let (remain, v) = DecodeUint(remaining)?;
            remaining = remain;
            d.SetUint64(v);
        }
        varintFlag => {
            let (remain, v) = DecodeVarint(remaining)?;
            remaining = remain;
            d.SetInt64(v);
        }
        uvarintFlag => {
            let (remain, v) = DecodeUvarint(remaining)?;
            remaining = remain;
            d.SetUint64(v);
        }
        floatFlag => {
            let (remain, v) = DecodeFloat(remaining)?;
            remaining = remain;
            d.SetFloat64(v);
        }
        bytesFlag => {
            let (remain, v) = DecodeBytes(remaining, None)?;
            remaining = remain;
            d.SetBytes(v);
        }
        compactBytesFlag => {
            let (remain, v) = DecodeCompactBytes(remaining)?;
            remaining = remain;
            d.SetBytes(v.to_vec());
        }
        decimalFlag => {
            let (remain, dec, precision, frac) = DecodeDecimal(remaining)?;
            remaining = remain;
            d.SetMysqlDecimal(dec);
            d.SetLength(precision);
            d.SetFrac(frac);
        }
        durationFlag => {
            let (remain, r) = DecodeInt(remaining)?;
            remaining = remain;
            // use max fsp, let outer to do round manually.
            d.SetMysqlDuration(types::Duration {
                Duration: time::Duration(r),
                Fsp: types::MaxFsp,
            });
        }
        jsonFlag => {
            let size = types::PeekBytesAsJSON(remaining).map_err(shared_error)?;
            d.SetMysqlJSON(types::BinaryJSON {
                TypeCode: remaining[0],
                Value: remaining[1..size].to_vec(),
            });
            remaining = &remaining[size..];
        }
        vectorFloat32Flag => {
            let (v, rest) =
                types::ZeroCopyDeserializeVectorFloat32(remaining).map_err(shared_error)?;
            d.SetVectorFloat32(v);
            remaining = rest;
        }
        NilFlag => {}
        _ => return Err(errors::Errorf(format!("invalid encoded key flag {}", flag))),
    }
    Ok((remaining, d))
}

// DecodeAsDateTime decodes on datum from []byte of `KindMysqlTime`.
/// 将编码为整数的时间值还原为 MySQL Time Datum。
pub fn DecodeAsDateTime(
    b: &[u8],
    tp: u8,
    loc: time::Location,
) -> Result<(&[u8], types::Datum), errors::SharedError> {
    if b.is_empty() {
        return Err(errors::New("invalid encoded key"));
    }
    let flag = b[0];
    let mut remaining = &b[1..];
    let v = match flag {
        uintFlag => {
            let (remain, v) = DecodeUint(remaining)?;
            remaining = remain;
            v
        }
        uvarintFlag => {
            // Datetime can be encoded as Uvarint.
            let (remain, v) = DecodeUvarint(remaining)?;
            remaining = remain;
            v
        }
        NilFlag => return Ok((remaining, types::Datum::default())),
        _ => return Err(errors::Errorf(format!("invalid encoded key flag {}", flag))),
    };
    let mut t = types::NewTime(types::ZeroCoreTime, tp, 0);
    t.FromPackedUint(v).map_err(shared_error)?;
    if tp == mysql::TypeTimestamp && !t.IsZero() {
        t.ConvertTimeZone(time::UTC, loc).map_err(shared_error)?;
    }
    let mut d = types::Datum::default();
    d.SetMysqlTime(t);
    Ok((remaining, d))
}

// DecodeAsFloat32 decodes value for mysql.TypeFloat.
/// 专用于 mysql.TypeFloat 的浮点解码。
pub fn DecodeAsFloat32(b: &[u8], tp: u8) -> Result<(&[u8], types::Datum), errors::SharedError> {
    if b.is_empty() || tp != mysql::TypeFloat {
        return Err(errors::New("invalid encoded key"));
    }
    let flag = b[0];
    let remaining = &b[1..];
    if flag != floatFlag {
        return Err(errors::Errorf(format!(
            "invalid encoded key flag {} for DecodeAsFloat32",
            flag
        )));
    }
    let (remain, v) = DecodeFloat(remaining)?;
    let mut d = types::Datum::default();
    d.SetFloat32FromF64(v);
    Ok((remain, d))
}

// CutOne cuts the first encoded value from b.
/// 切下首个编码值，返回 (值字节, 剩余)。
pub fn CutOne(b: Vec<u8>) -> Result<(Vec<u8>, Vec<u8>), errors::SharedError> {
    let l = peek(&b)?;
    Ok((b[..l].to_vec(), b[l..].to_vec()))
}

// CutColumnID cuts the column ID from b.
/// 跳过 flag 后解码列 ID（varint）。
pub fn CutColumnID(mut b: Vec<u8>) -> Result<(Vec<u8>, i64), errors::SharedError> {
    if b.is_empty() {
        return Err(errors::New("invalid encoded key"));
    }
    // skip the flag.
    b = b[1..].to_vec();
    let (remaining, value) = DecodeVarint(&b)?;
    Ok((remaining.to_vec(), value))
}

// SetRawValues set raw datum values from a row data.
/// 按编码边界把行数据切成各列 Raw Datum。
pub fn SetRawValues(
    mut data: Vec<u8>,
    values: &mut Vec<types::Datum>,
) -> Result<(), errors::SharedError> {
    for value in values {
        let l = peek(&data)?;
        // Go 使用 data[:l:l] 限制 cap，避免后续 append 覆盖；Rust Vec 复制天然切断别名。
        value.SetRaw(data[..l].to_vec());
        data = data[l..].to_vec();
    }
    Ok(())
}

// peek peeks the first encoded value from b and returns its length.
/// 窥探首个编码值的完整字节长度（不消费语义值）。
fn peek(b: &[u8]) -> Result<usize, errors::SharedError> {
    let originLength = b.len();
    if b.is_empty() {
        return Err(errors::New("invalid encoded key"));
    }
    let flag = b[0];
    let mut length = 1usize;
    let b = &b[1..];
    let l = match flag {
        NilFlag => 0,
        intFlag | uintFlag | floatFlag | durationFlag => 8,
        bytesFlag => peekBytes(b)?,
        compactBytesFlag => peekCompactBytes(b)?,
        decimalFlag => types::DecimalPeak(b).map_err(shared_error)?,
        varintFlag => peekVarint(b)?,
        uvarintFlag => peekUvarint(b)?,
        jsonFlag => types::PeekBytesAsJSON(b).map_err(shared_error)?,
        vectorFloat32Flag => types::PeekBytesAsVectorFloat32(b).map_err(shared_error)?,
        _ => return Err(errors::Errorf(format!("invalid encoded key flag {}", flag))),
    };
    length += l;
    if length == 0 {
        return Err(errors::New("invalid encoded key"));
    }
    if length > originLength {
        return Err(errors::Errorf(format!(
            "invalid encoded key, expected length: {}, actual length: {}",
            length, originLength
        )));
    }
    Ok(length)
}

/// 窥探 memcomparable bytes 编码长度。
fn peekBytes(b: &[u8]) -> Result<usize, errors::SharedError> {
    let mut offset = 0usize;
    loop {
        if b.len() < offset + encGroupSize + 1 {
            return Err(errors::New("insufficient bytes to decode value"));
        }
        // The byte slice is encoded into many groups: 8 data bytes and 1 marker.
        let marker = b[offset + encGroupSize];
        let padCount = encMarker - marker;
        offset += encGroupSize + 1;
        if padCount != 0 {
            break;
        }
    }
    Ok(offset)
}

/// 窥探 compact bytes 编码长度。
fn peekCompactBytes(b: &[u8]) -> Result<usize, errors::SharedError> {
    // Get length.
    let (v, n) = binary::Varint(b.to_vec());
    if n < 0 {
        return Err(errors::New("value larger than 64 bits"));
    }
    if n == 0 {
        return Err(errors::New("insufficient bytes to decode value"));
    }
    let encoded_len = v
        .checked_add(n as i64)
        .filter(|length| *length >= 0)
        .ok_or_else(|| errors::New("invalid encoded key"))? as usize;
    if b.len() < encoded_len {
        return Err(errors::Errorf(format!(
            "insufficient bytes to decode value, expected length: {}",
            n
        )));
    }
    Ok(encoded_len)
}

/// 窥探 varint 占用字节数。
fn peekVarint(b: &[u8]) -> Result<usize, errors::SharedError> {
    let (_, n) = binary::Varint(b.to_vec());
    if n < 0 {
        return Err(errors::New("value larger than 64 bits"));
    }
    Ok(n as usize)
}

/// 窥探 uvarint 占用字节数。
fn peekUvarint(b: &[u8]) -> Result<usize, errors::SharedError> {
    let (_, n) = binary::Uvarint(b.to_vec());
    if n < 0 {
        return Err(errors::New("value larger than 64 bits"));
    }
    Ok(n as usize)
}

// Decoder is used to decode value to chunk.
// Decoder 对应 Go 的 chunk 解码器，复用 buf 避免 DecodeBytes 重复分配。
pub struct Decoder {
    chk: *mut chunk::Chunk,
    timezone: time::Location,
    buf: Vec<u8>,
}

// NewDecoder creates a Decoder.
pub fn NewDecoder(chk: *mut chunk::Chunk, timezone: time::Location) -> Decoder {
    Decoder {
        chk,
        timezone,
        buf: Vec::new(),
    }
}

impl Decoder {
    // DecodeOne decodes one value to chunk and returns the remained bytes.
    /// 解码一个值写入 Chunk，返回剩余字节。
    pub fn DecodeOne(
        &mut self,
        b: Vec<u8>,
        colIdx: usize,
        ft: *mut types::FieldType,
    ) -> Result<Vec<u8>, errors::SharedError> {
        if b.is_empty() {
            return Err(errors::New("invalid encoded key"));
        }
        let chk = self.chk;
        let flag = b[0];
        let mut remaining = &b[1..];
        match flag {
            intFlag => {
                let (remain, v) = DecodeInt(remaining)?;
                remaining = remain;
                appendIntToChunk(v, chk, colIdx, ft);
            }
            uintFlag => {
                let (remain, v) = DecodeUint(remaining)?;
                remaining = remain;
                appendUintToChunk(v, chk, colIdx, ft, self.timezone)?;
            }
            varintFlag => {
                let (remain, v) = DecodeVarint(remaining)?;
                remaining = remain;
                appendIntToChunk(v, chk, colIdx, ft);
            }
            uvarintFlag => {
                let (remain, v) = DecodeUvarint(remaining)?;
                remaining = remain;
                appendUintToChunk(v, chk, colIdx, ft, self.timezone)?;
            }
            floatFlag => {
                let (remain, v) = DecodeFloat(remaining)?;
                remaining = remain;
                appendFloatToChunk(v, chk, colIdx, ft);
            }
            bytesFlag => {
                let (remain, decoded) =
                    DecodeBytes(remaining, Some(std::mem::take(&mut self.buf)))?;
                remaining = remain;
                self.buf = decoded;
                unsafe { (*chk).AppendBytes(colIdx, &self.buf) };
            }
            compactBytesFlag => {
                let (remain, v) = DecodeCompactBytes(remaining)?;
                remaining = remain;
                unsafe { (*chk).AppendBytes(colIdx, v) };
            }
            decimalFlag => {
                let (remain, mut dec, _, frac) = DecodeDecimal(remaining)?;
                remaining = remain;
                let field_frac = unsafe { (*ft).GetDecimal() };
                if field_frac != types::UnspecifiedLength as isize && frac as isize > field_frac {
                    // Go 会按字段 decimal 四舍五入；这里保留 Round 的错误传播。
                    let mut to = types::MyDecimal::default();
                    dec.Round(&mut to, field_frac, types::ModeHalfUp)
                        .map_err(shared_error)?;
                    dec = to;
                }
                unsafe { (*chk).AppendMyDecimal(colIdx, &dec) };
            }
            durationFlag => {
                let (remain, r) = DecodeInt(remaining)?;
                remaining = remain;
                unsafe {
                    (*chk).AppendDuration(
                        colIdx,
                        types::Duration {
                            Duration: time::Duration(r),
                            Fsp: (*ft).GetDecimal() as i32,
                        },
                    );
                }
            }
            jsonFlag => {
                let size = types::PeekBytesAsJSON(remaining).map_err(shared_error)?;
                unsafe {
                    (*chk).AppendJSON(
                        colIdx,
                        types::BinaryJSON {
                            TypeCode: remaining[0],
                            Value: remaining[1..size].to_vec(),
                        },
                    )
                };
                remaining = &remaining[size..];
            }
            vectorFloat32Flag => {
                let (v, rest) =
                    types::ZeroCopyDeserializeVectorFloat32(remaining).map_err(shared_error)?;
                unsafe { (*chk).AppendVectorFloat32(colIdx, v) };
                remaining = rest;
            }
            NilFlag => unsafe { (*chk).AppendNull(colIdx) },
            _ => return Err(errors::Errorf(format!("invalid encoded key flag {}", flag))),
        }
        Ok(remaining.to_vec())
    }
}

/// 按字段类型把有符号整数值追加到 Chunk（Duration 特例）。
fn appendIntToChunk(val: i64, chk: *mut chunk::Chunk, colIdx: usize, ft: *mut types::FieldType) {
    match unsafe { (*ft).GetType() } {
        mysql::TypeDuration => unsafe {
            (*chk).AppendDuration(
                colIdx,
                types::Duration {
                    Duration: time::Duration(val),
                    Fsp: (*ft).GetDecimal() as i32,
                },
            );
        },
        _ => unsafe { (*chk).AppendInt64(colIdx, val) },
    }
}

/// 按字段类型把无符号值追加到 Chunk（时间/枚举/集合/Bit 等）。
fn appendUintToChunk(
    val: u64,
    chk: *mut chunk::Chunk,
    colIdx: usize,
    ft: *mut types::FieldType,
    loc: time::Location,
) -> Result<(), errors::SharedError> {
    match unsafe { (*ft).GetType() } {
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
            let mut t = types::NewTime(types::ZeroCoreTime, unsafe { (*ft).GetType() }, unsafe {
                (*ft).GetDecimal() as i32
            });
            t.FromPackedUint(val).map_err(shared_error)?;
            if unsafe { (*ft).GetType() } == mysql::TypeTimestamp && !t.IsZero() {
                t.ConvertTimeZone(time::UTC, loc).map_err(shared_error)?;
            }
            unsafe { (*chk).AppendTime(colIdx, t) };
        }
        mysql::TypeEnum => {
            // ignore error deliberately, to read empty enum value.
            let enum_value = types::ParseEnumValue(unsafe { (*ft).GetElems() }, val)
                .unwrap_or(types::Enum::default());
            unsafe { (*chk).AppendEnum(colIdx, enum_value) };
        }
        mysql::TypeSet => {
            let set =
                types::ParseSetValue(unsafe { (*ft).GetElems() }, val).map_err(shared_error)?;
            unsafe { (*chk).AppendSet(colIdx, set) };
        }
        mysql::TypeBit => {
            let byte_size = (unsafe { (*ft).GetFlen() } + 7) >> 3;
            let literal = types::NewBinaryLiteralFromUint(val, byte_size).0;
            unsafe { (*chk).AppendBytes(colIdx, &literal) };
        }
        _ => unsafe { (*chk).AppendUint64(colIdx, val) },
    }
    Ok(())
}

/// 按字段类型追加 float32 或 float64。
fn appendFloatToChunk(val: f64, chk: *mut chunk::Chunk, colIdx: usize, ft: *mut types::FieldType) {
    if unsafe { (*ft).GetType() } == mysql::TypeFloat {
        unsafe { (*chk).AppendFloat32(colIdx, val as f32) };
    } else {
        unsafe { (*chk).AppendFloat64(colIdx, val) };
    }
}

// HashGroupKey encodes each row of this column and append encoded data into buf.
// Only use in the aggregate executor.
/// 聚合执行器：按列类型为每行生成 group key 字节。
pub fn HashGroupKey(
    loc: time::Location,
    n: usize,
    col: *mut chunk::Column,
    mut buf: Vec<Vec<u8>>,
    ft: *mut types::FieldType,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    match unsafe { (*ft).EvalType() } {
        types::ETInt => {
            let i64s = unsafe { (*col).Int64s() };
            for i in 0..n {
                if unsafe { (*col).IsNull(i) } {
                    buf[i].push(NilFlag);
                } else {
                    buf[i] = encodeSignedInt(buf[i].clone(), i64s[i], false);
                }
            }
        }
        types::ETReal => {
            let f64s = unsafe { (*col).Float64s() };
            for i in 0..n {
                if unsafe { (*col).IsNull(i) } {
                    buf[i].push(NilFlag);
                } else {
                    buf[i].push(floatFlag);
                    buf[i] = EncodeFloat(buf[i].clone(), f64s[i]);
                }
            }
        }
        types::ETDecimal => {
            let ds = unsafe { (*col).Decimals() };
            for i in 0..n {
                if unsafe { (*col).IsNull(i) } {
                    buf[i].push(NilFlag);
                } else {
                    buf[i].push(decimalFlag);
                    buf[i] = EncodeDecimal(
                        buf[i].clone(),
                        &ds[i],
                        unsafe { (*ft).GetFlen() as i32 },
                        unsafe { (*ft).GetDecimal() as i32 },
                    )?;
                }
            }
        }
        types::ETDatetime | types::ETTimestamp => {
            let ts = unsafe { (*col).Times() };
            for i in 0..n {
                if unsafe { (*col).IsNull(i) } {
                    buf[i].push(NilFlag);
                } else {
                    buf[i].push(uintFlag);
                    buf[i] = EncodeMySQLTime(loc, ts[i], mysql::TypeUnspecified, buf[i].clone())?;
                }
            }
        }
        types::ETDuration => {
            let ds = unsafe { (*col).GoDurations() };
            for i in 0..n {
                if unsafe { (*col).IsNull(i) } {
                    buf[i].push(NilFlag);
                } else {
                    buf[i].push(durationFlag);
                    buf[i] = EncodeInt(buf[i].clone(), ds[i] as i64);
                }
            }
        }
        types::ETJson => {
            for i in 0..n {
                if unsafe { (*col).IsNull(i) } {
                    buf[i].push(NilFlag);
                } else {
                    buf[i].push(jsonFlag);
                    buf[i] = unsafe { (*col).GetJSON(i) }.HashValue(buf[i].clone());
                }
            }
        }
        types::ETString => {
            let collator = collate::GetCollator(unsafe { (*ft).GetCollate() });
            for i in 0..n {
                if unsafe { (*col).IsNull(i) } {
                    buf[i].push(NilFlag);
                } else {
                    let bytes = unsafe { (*col).GetBytes(i) };
                    let value = String::from_utf8_lossy(&bytes);
                    let key = collator.ImmutableKey(&value);
                    buf[i] = encodeBytes(buf[i].clone(), key, false);
                }
            }
        }
        types::ETVectorFloat32 => {
            for i in 0..n {
                if unsafe { (*col).IsNull(i) } {
                    buf[i].push(NilFlag);
                } else {
                    buf[i] = unsafe { (*col).GetVectorFloat32(i) }.SerializeTo(buf[i].clone());
                }
            }
        }
        _ => {
            return Err(errors::Errorf(format!(
                "unsupported type {} during evaluation",
                unsafe { (*ft).EvalType() }
            )));
        }
    }
    Ok(buf)
}

// ConvertByCollation converts these bytes according to its collation.
/// 按字段 collation 把原始字节转为比较/哈希用 key。
pub fn ConvertByCollation(raw: Vec<u8>, tp: &types::FieldType) -> Vec<u8> {
    let collator = collate::GetCollator(tp.GetCollate());
    let value = String::from_utf8_lossy(&raw);
    collator.Key(&value)
}

// ConvertByCollationStr converts this string according to its collation.
/// 按字段 collation 转换字符串。
pub fn ConvertByCollationStr(str_value: String, tp: &types::FieldType) -> String {
    let collator = collate::GetCollator(tp.GetCollate());
    String::from_utf8_lossy(&collator.Key(&str_value)).into_owned()
}

// Hash64 is for datum hash64 calculation.
/// 通过 HashCode 计算 Datum 的 Hash64。
pub fn Hash64(h: &mut dyn base::Hasher, d: &types::Datum) {
    // let h.cache to receive datum hash value, which is potentially expendable; clean the cache before using it.
    let mut b = h.Cache().to_vec();
    b.clear();
    b = HashCode(b, d.clone());
    h.HashBytes(&b);
    h.SetCache(b);
}

// init 对应 Go 包初始化：把 types.Hash64ForDatum 指向本文件的 Hash64。
pub fn init() {
    unsafe {
        types::Hash64ForDatum = Hash64;
    }
}

// HashCode encodes a Datum into a unique byte slice.
/// 包级 HashCode：使用当前全局新 collation 开关。
pub fn HashCode(b: Vec<u8>, d: types::Datum) -> Vec<u8> {
    NewEncoder(collate::NewCollationEnabled()).HashCode(b, d)
}
