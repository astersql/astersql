// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// Rust implementation of pkg/tablecodec/tablecodec.go, preserving its key, row,
// index-value, temporary-index, and key-range behavior.
//
// 表级 KV key/value 编解码（对应 Go `pkg/tablecodec`）。
//
// 负责表前缀、行记录 key（handle）、二级索引 key/value、meta key、
// 临时索引（DDL backfill）、以及表/索引 key range 的生成与解析。
// Handle 可以是整数主键或 common handle（联合主键列编码）。

use std::collections::HashMap;

// Go var 块中的错误值来自 dbterror.ClassXEval。
/// 构造无效 key 标准错误。
pub fn errInvalidKey() -> Box<terror::Error> {
    dbterror::ClassXEval.NewStd(errno::ErrInvalidKey)
}

/// 构造无效记录 key 标准错误。
pub fn errInvalidRecordKey() -> Box<terror::Error> {
    dbterror::ClassXEval.NewStd(errno::ErrInvalidRecordKey)
}

/// 构造无效索引 key 标准错误。
pub fn errInvalidIndexKey() -> Box<terror::Error> {
    dbterror::ClassXEval.NewStd(errno::ErrInvalidIndexKey)
}

/// 基于原型错误生成带堆栈与消息的 SharedError。
fn invalid_key_error(prototype: &terror::Error, message: String) -> errors::SharedError {
    prototype.GenWithStack(&message, &[])
}

/// 将底层错误包装为 SharedError。
fn trace_error(error: impl std::fmt::Display) -> errors::SharedError {
    errors::New(error.to_string())
}

// 表、记录、索引和 meta 前缀与 Go []byte 变量一致。
/// 表 key 全局前缀 `t`。
pub static tablePrefix: &[u8] = b"t";
/// 行记录分隔前缀 `_r`。
pub static recordPrefixSep: &[u8] = b"_r";
/// 索引分隔前缀 `_i`。
pub static indexPrefixSep: &[u8] = b"_i";
/// 元数据 key 前缀 `m`。
pub static metaPrefix: &[u8] = b"m";

/// 表/索引 ID 编码长度（8 字节）。
pub const idLen: usize = 8;
/// `t` + tableID + `_r`/`_i` 的前缀总长度。
pub const prefixLen: usize = 1 + idLen + 2;
// RecordRowKeyLen is public for calculating average row size.
/// 整数 handle 行 key 的典型长度（用于估算平均行大小）。
pub const RecordRowKeyLen: usize = prefixLen + idLen;
/// 表前缀字节长度。
pub const tablePrefixLength: usize = 1;
/// 行分隔符长度。
pub const recordPrefixSepLength: usize = 2;
/// meta 前缀长度。
pub const metaPrefixLength: usize = 1;
// MaxOldEncodeValueLen is the maximum len of the old encoding of index value.
/// 旧版索引 value 编码的最大长度。
pub const MaxOldEncodeValueLen: usize = 9;

// CommonHandleFlag is the flag used to decode the common handle in an unique index value.
/// 唯一索引 value 中标识 common handle 的 flag。
pub const CommonHandleFlag: u8 = 127;
// PartitionIDFlag is the flag used to decode the partition ID.
// Used in both global index values and global index keys (for V1+ non-unique indexes).
// In keys: PartitionIDFlag + partition_id (8 bytes) + inner_handle_encoded (IntHandle)
// In values: PartitionIDFlag + partition_id (8 bytes)
/// 全局索引 key/value 中标识分区 ID 的 flag。
pub const PartitionIDFlag: u8 = 126;
// IndexVersionFlag is the flag used to decode the index's version info.
/// 索引 value 版本信息 flag。
pub const IndexVersionFlag: u8 = 125;
// RestoreDataFlag is the flag that RestoreData begin with.
// See rowcodec.Encoder.Encode and rowcodec.row.toBytes
/// 索引 value 中 RestoreData（列原始值）段起始 flag。
pub const RestoreDataFlag: u8 = rowcodec::CodecVer;

// TableSplitKeyLen is the length of key 't{table_id}' which is used for table split.
/// 表分裂用 key `t{table_id}` 的长度。
pub const TableSplitKeyLen: usize = 1 + idLen;

// init 对应 Go 包初始化：给 kv 包注入 DecodeTableIDFunc，避免 kv 反向依赖 tablecodec。
/// 包初始化：向 kv 注入 DecodeTableIDFunc，避免 kv 反向依赖本模块。
pub fn init() {
    unsafe {
        kv::DecodeTableIDFunc = Some(|key: kv::Key| -> i64 {
            // preCheck, avoid the noise error log.
            if key.0.len() >= TableSplitKeyLen && hasTablePrefix(&key.0) {
                return DecodeTableID(key);
            }
            0
        });
    }
}

// TablePrefix returns table's prefix 't'.
/// 返回表前缀字节 `t`。
pub fn TablePrefix() -> &'static [u8] {
    tablePrefix
}

// MetaPrefix returns meta prefix 'm'.
/// 返回 meta 前缀字节 `m`。
pub fn MetaPrefix() -> &'static [u8] {
    metaPrefix
}

// EncodeRowKey encodes the table id and record handle into a kv.Key
// EncodeRowKey 先拼接 `t{tableID}_r`，再追加已编码 handle。
/// 编码行 key：`t{tableID}_r` + 已编码 handle。
pub fn EncodeRowKey(tableID: i64, encodedHandle: &[u8]) -> kv::Key {
    let mut buf = Vec::with_capacity(prefixLen + encodedHandle.len());
    buf = appendTableRecordPrefix(buf, tableID);
    buf.extend_from_slice(encodedHandle);
    kv::Key(buf)
}

// EncodeRowKeyWithHandle encodes the table id, row handle into a kv.Key
/// 使用 Handle 接口编码行 key。
pub fn EncodeRowKeyWithHandle(tableID: i64, handle: Box<dyn kv::Handle>) -> kv::Key {
    EncodeRowKey(tableID, &handle.Encoded())
}

// CutRowKeyPrefix cuts the row key prefix.
/// 去掉行 key 前缀，保留 handle 编码部分。
pub fn CutRowKeyPrefix(key: kv::Key) -> Vec<u8> {
    key.0[prefixLen..].to_vec()
}

// EncodeRecordKey encodes the recordPrefix, row handle into a kv.Key.
// EncodeRecordKey 对 PartitionHandle 会替换 recordPrefix 为真实 partition id 前缀。
/// 在记录前缀上追加 handle；PartitionHandle 会替换为真实分区前缀。
pub fn EncodeRecordKey(mut recordPrefix: kv::Key, h: Box<dyn kv::Handle>) -> kv::Key {
    let mut buf = Vec::with_capacity(recordPrefix.0.len() + h.Len());
    if let Some(ph) = h.as_any().downcast_ref::<kv::PartitionHandle>() {
        recordPrefix = GenTableRecordPrefix(ph.PartitionID);
    }
    buf.extend_from_slice(&recordPrefix.0);
    buf.extend_from_slice(&h.Encoded());
    kv::Key(buf)
}

// hasTablePrefix 与 Go 一样只读第一个字节；调用者需保证 key 非空。
/// 检查首字节是否为表前缀（调用方保证非空）。
pub fn hasTablePrefix(key: &[u8]) -> bool {
    key[0] == tablePrefix[0]
}

// hasRecordPrefixSep 检查 `_r` 分隔符；调用者需保证长度至少为 2。
/// 检查是否以 `_r` 行分隔符开头（调用方保证长度≥2）。
pub fn hasRecordPrefixSep(key: &[u8]) -> bool {
    key[0] == recordPrefixSep[0] && key[1] == recordPrefixSep[1]
}

// DecodeRecordKey decodes the key and gets the tableID, handle.
/// 解码记录 key，得到 tableID 与 handle（整数或 common）。
pub fn DecodeRecordKey(key: kv::Key) -> Result<(i64, Box<dyn kv::Handle>), errors::SharedError> {
    if key.0.len() <= prefixLen {
        return Err(invalid_key_error(
            &errInvalidRecordKey(),
            format!("invalid record key - {:?}", key),
        ));
    }

    let original = key.clone();
    if !hasTablePrefix(&key.0) {
        return Err(invalid_key_error(
            &errInvalidRecordKey(),
            format!("invalid record key - {:?}", original),
        ));
    }

    let (mut key, tableID) = codec::DecodeInt(&key.0[tablePrefixLength..]).map_err(trace_error)?;

    if !hasRecordPrefixSep(key) {
        return Err(invalid_key_error(
            &errInvalidRecordKey(),
            format!("invalid record key - {:?}", original),
        ));
    }

    key = &key[recordPrefixSepLength..];
    // 剩余 8 字节视为整数 handle，否则按 common handle（联合主键）解析。
    if key.len() == 8 {
        let (_, intHandle) = codec::DecodeInt(key).map_err(trace_error)?;
        return Ok((tableID, Box::new(kv::IntHandle(intHandle))));
    }
    let h = kv::NewCommonHandle(key.to_vec()).map_err(|err| {
        invalid_key_error(
            &errInvalidRecordKey(),
            format!("invalid record key - {:?} {:?}", original, err),
        )
    })?;
    Ok((tableID, Box::new(h)))
}

// DecodeIndexKey decodes the key and gets the tableID, indexID, indexValues.
/// 解码索引 key，得到 tableID、indexID 与列值字符串。
pub fn DecodeIndexKey(key: kv::Key) -> Result<(i64, i64, Vec<String>), errors::SharedError> {
    let original = key.clone();
    let (tableID, indexID, isRecord) = DecodeKeyHead(key.clone())?;
    if isRecord {
        return Err(invalid_key_error(
            &errInvalidIndexKey(),
            format!("invalid index key - {:?}", original),
        ));
    }
    let indexKey = key.0[prefixLen + idLen..].to_vec();
    let indexValues = DecodeValuesBytesToStrings(indexKey).map_err(|err| {
        invalid_key_error(
            &errInvalidIndexKey(),
            format!("invalid index key - {:?} {:?}", original, err),
        )
    })?;
    Ok((tableID, indexID, indexValues))
}

// DecodeValuesBytesToStrings decode the raw bytes to strings for each columns.
// FIXME: Without the schema information, we can only decode the raw kind of
// the column. For instance, MysqlTime is internally saved as uint64.
/// 将索引列原始字节逐列解码为调试用字符串（无 schema 时仅按 kind）。
pub fn DecodeValuesBytesToStrings(mut b: Vec<u8>) -> Result<Vec<String>, errors::SharedError> {
    let mut datumValues = Vec::new();
    while !b.is_empty() {
        let (remain, d) = codec::DecodeOne(&b).map_err(trace_error)?;
        let strv = d.ToString().map_err(|err| errors::New(err.to_string()))?;
        datumValues.push(strv);
        b = remain.to_vec();
    }
    Ok(datumValues)
}

// EncodeMetaKey encodes the key and field into meta key.
/// 编码 meta key：`m` + encoded(key) + encoded(field)。
pub fn EncodeMetaKey(key: &[u8], field: &[u8]) -> kv::Key {
    let mut ek = Vec::with_capacity(
        metaPrefix.len()
            + codec::EncodedBytesLength(key.len())
            + 8
            + codec::EncodedBytesLength(field.len()),
    );
    ek.extend_from_slice(metaPrefix);
    ek = codec::EncodeBytes(ek, key);
    ek = codec::EncodeUint(ek, structure::HashData as u64);
    ek = codec::EncodeBytes(ek, field);
    kv::Key(ek)
}

// EncodeMetaKeyPrefix encodes the key prefix into meta key
/// 编码仅含 key 部分的 meta 前缀，用于范围扫描。
pub fn EncodeMetaKeyPrefix(key: &[u8]) -> kv::Key {
    let mut ek = Vec::with_capacity(metaPrefix.len() + codec::EncodedBytesLength(key.len()) + 8);
    ek.extend_from_slice(metaPrefix);
    ek = codec::EncodeBytes(ek, key);
    ek = codec::EncodeUint(ek, structure::HashData as u64);
    kv::Key(ek)
}

// DecodeMetaKey decodes the key and get the meta key and meta field.
/// 解码 meta key 为 (key, field) 原始字节。
pub fn DecodeMetaKey(ek: kv::Key) -> Result<(Vec<u8>, Vec<u8>), errors::SharedError> {
    if !ek.0.starts_with(metaPrefix) {
        return Err(errors::New("invalid encoded hash data key prefix"));
    }
    let (remain, key) = codec::DecodeBytes(&ek.0[metaPrefixLength..], None).map_err(trace_error)?;
    let (remain, tp) = codec::DecodeUint(remain).map_err(trace_error)?;
    if tp as structure::TypeFlag != structure::HashData {
        return Err(errors::New(format!(
            "invalid encoded hash data key flag {}",
            tp as u8
        )));
    }
    let (_, field) = codec::DecodeBytes(remain, None).map_err(trace_error)?;
    Ok((key, field))
}

// DecodeKeyHead decodes the key's head and gets the tableID, indexID. isRecordKey is true when is a record key.
/// 解析 key 头部：tableID、indexID（记录则为 0）、是否为记录 key。
pub fn DecodeKeyHead(key: kv::Key) -> Result<(i64, i64, bool), errors::SharedError> {
    let original = key.clone();
    if !key.0.starts_with(tablePrefix) {
        return Err(invalid_key_error(
            &errInvalidKey(),
            format!("invalid key - {:?}", original),
        ));
    }

    let (key, tableID) = codec::DecodeInt(&key.0[tablePrefix.len()..]).map_err(trace_error)?;

    if key.starts_with(recordPrefixSep) {
        return Ok((tableID, 0, true));
    }
    if !key.starts_with(indexPrefixSep) {
        return Err(invalid_key_error(
            &errInvalidKey(),
            format!("invalid key - {:?}", original),
        ));
    }

    let (_, indexID) = codec::DecodeInt(&key[indexPrefixSep.len()..]).map_err(trace_error)?;
    Ok((tableID, indexID, false))
}

// DecodeIndexID decodes indexID from the key.
// this method simply extract index id part, and no other checking.
// Caller should make sure the key is an index key.
/// 从索引 key 中取出 indexID。
pub fn DecodeIndexID(key: kv::Key) -> Result<i64, errors::SharedError> {
    let (_, indexID) = codec::DecodeInt(&key.0[tablePrefix.len() + 8 + indexPrefixSep.len()..])
        .map_err(trace_error)?;
    Ok(indexID)
}

// DecodeTableID decodes the table ID of the key, if the key is not table key, returns 0.
/// 从 key 中解码 tableID；非法时返回 0。
pub fn DecodeTableID(key: kv::Key) -> i64 {
    let mut key = key.0;
    if !key.starts_with(tablePrefix) {
        // If the key is in API V2, then ignore the prefix
        if key.len() > 4 && key[0] == b'x' {
            key = key[4..].to_vec();
        } else {
            return 0;
        }
        if !key.starts_with(tablePrefix) {
            return 0;
        }
    }
    match codec::DecodeInt(&key[tablePrefix.len()..]) {
        Ok((_, table_id)) => table_id,
        Err(_) => 0,
    }
}

// DecodeRowKey decodes the key and gets the handle.
/// 解码行 key 得到 handle（不返回 tableID）。
pub fn DecodeRowKey(key: kv::Key) -> Result<Box<dyn kv::Handle>, errors::SharedError> {
    // In the read path, remove the keyspace prefix
    // to ensure compatibility with the key parsing implemented in the mock.
    let tempKey = rowcodec::RemoveKeyspacePrefix(&key.0);

    if tempKey.len() < RecordRowKeyLen
        || !hasTablePrefix(&tempKey)
        || !hasRecordPrefixSep(&tempKey[prefixLen - 2..])
    {
        return Err(invalid_key_error(
            &errInvalidKey(),
            format!("invalid key - {:?}", tempKey),
        ));
    }
    if tempKey.len() == RecordRowKeyLen {
        let u = u64::from_be_bytes(tempKey[prefixLen..].try_into().unwrap());
        return Ok(Box::new(kv::IntHandle(codec::DecodeCmpUintToInt(u))));
    }
    kv::NewCommonHandle(tempKey[prefixLen..].to_vec())
        .map(|handle| Box::new(handle) as Box<dyn kv::Handle>)
        // Go's NewCommonHandle reports this stable public error for malformed
        // common-handle payloads; do not leak the lower-level codec detail.
        .map_err(|_| errors::New("invalid encoded key"))
}

// EncodeValue encodes a go value to bytes.
// This function may return both a valid encoded bytes and an error (actually `"pingcap/errors".ErrorGroup`). If the caller
// expects to handle these errors according to `SQL_MODE` or other configuration, please refer to `pkg/errctx`.
/// 按列类型将 Datum 编码为存储字节。
pub fn EncodeValue(
    loc: Option<time::Location>,
    b: Vec<u8>,
    raw: types::Datum,
) -> Result<Vec<u8>, errors::SharedError> {
    let mut v = types::Datum::default();
    flatten(loc.clone(), raw, &mut v)?;
    codec::EncodeValue(loc.unwrap_or(time::UTC), b, vec![v]).map_err(trace_error)
}

// EncodeRow encode row data and column ids into a slice of byte.
// valBuf and values pass by caller, for reducing EncodeRow allocates temporary bufs. If you pass valBuf and values as nil,
// EncodeRow will allocate it.
// This function may return both a valid encoded bytes and an error (actually `"pingcap/errors".ErrorGroup`). If the caller
// expects to handle these errors according to `SQL_MODE` or other configuration, please refer to `pkg/errctx`.
/// 编码整行：优先新行编码，失败或配置时回退旧编码。
pub fn EncodeRow(
    loc: Option<time::Location>,
    row: Vec<types::Datum>,
    colIDs: Vec<i64>,
    mut valBuf: Vec<u8>,
    values: Option<Vec<types::Datum>>,
    checksum: Option<Box<dyn rowcodec::Checksum>>,
    mut e: rowcodec::Encoder,
) -> Result<Vec<u8>, errors::SharedError> {
    if row.len() != colIDs.len() {
        return Err(errors::New(format!(
            "EncodeRow error: data and columnID count not match {} vs {}",
            row.len(),
            colIDs.len()
        )));
    }
    if e.Enable {
        valBuf.clear();
        return e
            .Encode(loc.as_ref(), colIDs, row, checksum, valBuf)
            .map_err(trace_error);
    }
    EncodeOldRow(loc, row, colIDs, valBuf, values)
}

// EncodeOldRow encode row data and column ids into a slice of byte.
// Row layout: colID1, value1, colID2, value2, .....
// valBuf and values pass by caller, for reducing EncodeOldRow allocates temporary bufs. If you pass valBuf and values as nil,
// EncodeOldRow will allocate it.
/// 使用旧版行格式编码列值。
pub fn EncodeOldRow(
    loc: Option<time::Location>,
    row: Vec<types::Datum>,
    colIDs: Vec<i64>,
    mut valBuf: Vec<u8>,
    values: Option<Vec<types::Datum>>,
) -> Result<Vec<u8>, errors::SharedError> {
    if row.len() != colIDs.len() {
        return Err(errors::New(format!(
            "EncodeRow error: data and columnID count not match {} vs {}",
            row.len(),
            colIDs.len()
        )));
    }
    valBuf.clear();
    let mut values = values.unwrap_or_else(|| vec![types::Datum::default(); row.len() * 2]);
    for (i, c) in row.into_iter().enumerate() {
        let id = colIDs[i];
        values[2 * i].SetInt64(id);
        if let Err(err) = flatten(loc.clone(), c, &mut values[2 * i + 1]) {
            return Err(trace_error(err));
        }
    }
    if values.is_empty() {
        // We could not set nil value into kv.
        valBuf.push(codec::NilFlag);
        return Ok(valBuf);
    }
    codec::EncodeValue(loc.unwrap_or(time::UTC), valBuf, values)
        .map_err(trace_error)
}

// flatten 对应 Go 的 datum 存储格式归一化：时间、duration、enum/set、bit 等写成基础整数或原始值。
/// 将 Datum 展平为可编码的底层表示（处理特殊类型包装）。
pub fn flatten(
    loc: Option<time::Location>,
    data: types::Datum,
    ret: &mut types::Datum,
) -> Result<(), errors::SharedError> {
    match data.Kind() {
        types::KindMysqlTime => {
            // for mysql datetime, timestamp and date type
            let mut t = data.GetMysqlTime();
            if t.Type() == mysql::TypeTimestamp && loc.is_some() && loc != Some(time::UTC) {
                t.ConvertTimeZone(loc.unwrap(), time::UTC)
                    .map_err(trace_error)?;
            }
            let v = t.ToPackedUint().map_err(trace_error)?;
            ret.SetUint64(v);
            Ok(())
        }
        types::KindMysqlDuration => {
            // for mysql time type
            ret.SetInt64(data.GetMysqlDuration().Duration as i64);
            Ok(())
        }
        types::KindMysqlEnum => {
            ret.SetUint64(data.GetMysqlEnum().Value);
            Ok(())
        }
        types::KindMysqlSet => {
            ret.SetUint64(data.GetMysqlSet().Value);
            Ok(())
        }
        types::KindBinaryLiteral | types::KindMysqlBit => {
            // We don't need to handle errors here since the literal is ensured to be able to store in uint64 in convertToMysqlBit.
            let val = data
                .GetBinaryLiteral()
                .ToInt(types::StrictContext.clone())
                .map_err(trace_error)?;
            ret.SetUint64(val);
            Ok(())
        }
        _ => {
            *ret = data;
            Ok(())
        }
    }
}

// DecodeColumnValue decodes data to a Datum according to the column info.
/// 解码单列存储字节为 Datum。
pub fn DecodeColumnValue(
    data: Vec<u8>,
    ft: Box<types::FieldType>,
    loc: Option<time::Location>,
) -> Result<types::Datum, errors::SharedError> {
    let (_, d) = codec::DecodeOne(&data).map_err(trace_error)?;
    Unflatten(d, ft, loc)
}

// DecodeColumnValueWithDatum decodes data to an existing Datum according to the column info.
/// 在已有 Datum 缓冲上解码单列值。
pub fn DecodeColumnValueWithDatum(
    data: Vec<u8>,
    ft: Box<types::FieldType>,
    loc: Option<time::Location>,
    result: &mut types::Datum,
) -> Result<(), errors::SharedError> {
    let (_, d) = codec::DecodeOne(&data).map_err(trace_error)?;
    *result = Unflatten(d, ft, loc).map_err(trace_error)?;
    Ok(())
}

// DecodeRowWithMapNew decode a row to datum map.
// DecodeRowWithMapNew 针对 rowcodec 新格式：构造 reqCols 后交给 DatumMapDecoder。
/// 用新行解码器按列 ID 映射解码行。
pub fn DecodeRowWithMapNew(
    b: Option<Vec<u8>>,
    cols: HashMap<i64, Box<types::FieldType>>,
    loc: Option<time::Location>,
    row: Option<HashMap<i64, types::Datum>>,
) -> Result<HashMap<i64, types::Datum>, errors::SharedError> {
    let mut row = row.unwrap_or_else(|| HashMap::with_capacity(cols.len()));
    let Some(b) = b else {
        return Ok(row);
    };
    if b.len() == 1 && b[0] == codec::NilFlag {
        return Ok(row);
    }

    let mut reqCols = Vec::with_capacity(cols.len());
    for (id, tp) in cols {
        reqCols.push(rowcodec::ColInfo {
            ID: id,
            IsPKHandle: false,
            VirtualGenCol: false,
            Ft: *tp,
        });
    }
    let mut rd = rowcodec::NewDatumMapDecoder(reqCols, loc);
    rd.DecodeToDatumMap(&b, Some(row)).map_err(trace_error)
}

// DecodeRowWithMap decodes a byte slice into datums with an existing row map.
// Row layout: colID1, value1, colID2, value2, .....
/// 按列 ID 映射解码行（自动选择新旧格式）。
pub fn DecodeRowWithMap(
    mut b: Option<Vec<u8>>,
    cols: HashMap<i64, Box<types::FieldType>>,
    loc: Option<time::Location>,
    row: Option<HashMap<i64, types::Datum>>,
) -> Result<HashMap<i64, types::Datum>, errors::SharedError> {
    let mut row = row.unwrap_or_else(|| HashMap::with_capacity(cols.len()));
    let Some(mut data_stream) = b.take() else {
        return Ok(row);
    };
    if data_stream.len() == 1 && data_stream[0] == codec::NilFlag {
        return Ok(row);
    }
    let mut cnt = 0usize;
    while !data_stream.is_empty() {
        // Get col id.
        let (data, remain) = codec::CutOne(data_stream).map_err(trace_error)?;
        data_stream = remain;
        let (_, cid) = codec::DecodeOne(&data).map_err(trace_error)?;

        // Get col value.
        let (data, remain) = codec::CutOne(data_stream).map_err(trace_error)?;
        data_stream = remain;
        let id = cid.GetInt64();
        if let Some(ft) = cols.get(&id) {
            let (_, v) = codec::DecodeOne(&data).map_err(trace_error)?;
            let v = Unflatten(v, ft.clone(), loc.clone()).map_err(trace_error)?;
            row.insert(id, v);
            cnt += 1;
            if cnt == cols.len() {
                // Get enough data.
                break;
            }
        }
    }
    Ok(row)
}

// DecodeRowToDatumMap decodes a byte slice into datums.
// Row layout: colID1, value1, colID2, value2, .....
// Default value columns, generated columns and handle columns are unprocessed.
/// 将行字节解码为列 ID → Datum 映射。
pub fn DecodeRowToDatumMap(
    b: Option<Vec<u8>>,
    cols: HashMap<i64, Box<types::FieldType>>,
    loc: Option<time::Location>,
) -> Result<HashMap<i64, types::Datum>, errors::SharedError> {
    if !rowcodec::IsNewFormat(b.as_deref().unwrap_or_default()) {
        return DecodeRowWithMap(b, cols, loc, None);
    }
    DecodeRowWithMapNew(b, cols, loc, None)
}

// DecodeHandleToDatumMap decodes a handle into datum map.
/// 将 handle 中的主键列填充进 Datum 映射。
pub fn DecodeHandleToDatumMap(
    handle: Option<Box<dyn kv::Handle>>,
    handleColIDs: Vec<i64>,
    cols: HashMap<i64, Box<types::FieldType>>,
    loc: Option<time::Location>,
    row: Option<HashMap<i64, types::Datum>>,
) -> Result<HashMap<i64, types::Datum>, errors::SharedError> {
    let mut row = row.unwrap_or_else(|| HashMap::with_capacity(cols.len()));
    let Some(handle) = handle else {
        return Ok(row);
    };
    if handleColIDs.is_empty() {
        return Ok(row);
    }
    for (idx, id) in handleColIDs.into_iter().enumerate() {
        let Some(ft) = cols.get(&id) else {
            continue;
        };
        if types::NeedRestoredData(ft) {
            continue;
        }
        let d = decodeHandleToDatum(handle.Copy(), ft.clone(), idx)?;
        let d = Unflatten(d, ft.clone(), loc.clone())?;
        if !row.contains_key(&id) {
            row.insert(id, d);
        }
    }
    Ok(row)
}

// decodeHandleToDatum decodes a handle to a specific column datum.
/// 把 handle 解码为与主键列对应的 Datum 列表。
pub fn decodeHandleToDatum(
    handle: Box<dyn kv::Handle>,
    ft: Box<types::FieldType>,
    idx: usize,
) -> Result<types::Datum, errors::SharedError> {
    if handle.IsInt() {
        let d = if mysql::HasUnsignedFlag(ft.GetFlag()) {
            types::NewUintDatum(handle.IntValue() as u64)
        } else {
            types::NewIntDatum(handle.IntValue())
        };
        return Ok(d);
    }
    // Decode common handle to Datum.
    let encoded = handle.EncodedCol(idx);
    let (_, d) = codec::DecodeOne(&encoded).map_err(trace_error)?;
    Ok(d)
}

// CutRowNew cuts encoded row into byte slices and return columns' byte slice.
// Row layout: colID1, value1, colID2, value2, .....
/// 按需要的列从新行编码中裁剪出列字节。
pub fn CutRowNew(
    data: Option<Vec<u8>>,
    colIDs: HashMap<i64, usize>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    let Some(mut data) = data else {
        return Ok(Vec::new());
    };
    if data.len() == 1 && data[0] == codec::NilFlag {
        return Ok(Vec::new());
    }

    let mut cnt = 0usize;
    let mut row = vec![Vec::new(); colIDs.len()];
    while !data.is_empty() && cnt < colIDs.len() {
        // Get col id.
        let (remain, cid) = codec::CutColumnID(data).map_err(trace_error)?;
        data = remain;

        // Get col value.
        let (b, remain) = codec::CutOne(data).map_err(trace_error)?;
        data = remain;

        if let Some(offset) = colIDs.get(&cid) {
            row[*offset] = b;
            cnt += 1;
        }
    }
    Ok(row)
}

// UnflattenDatums converts raw datums to column datums.
/// 批量 Unflatten：按 FieldType 还原 Datum。
pub fn UnflattenDatums(
    mut datums: Vec<types::Datum>,
    fts: Vec<Box<types::FieldType>>,
    loc: Option<time::Location>,
) -> Result<Vec<types::Datum>, errors::SharedError> {
    for i in 0..datums.len() {
        let ft = fts[i].clone();
        let uDatum = Unflatten(datums[i].clone(), ft, loc.clone()).map_err(trace_error)?;
        datums[i] = uDatum;
    }
    Ok(datums)
}

// Unflatten converts a raw datum to a column datum.
// Unflatten 按列类型把存储层基础 datum 恢复为 SQL 类型 datum。
/// 将原始 Datum 按列类型还原（时间、枚举、JSON 等）。
pub fn Unflatten(
    mut datum: types::Datum,
    ft: Box<types::FieldType>,
    loc: Option<time::Location>,
) -> Result<types::Datum, errors::SharedError> {
    if datum.IsNull() {
        return Ok(datum);
    }
    match ft.GetType() {
        mysql::TypeFloat => {
            datum.SetFloat32(datum.GetFloat64() as f32);
            Ok(datum)
        }
        mysql::TypeVarchar
        | mysql::TypeString
        | mysql::TypeVarString
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeBlob
        | mysql::TypeLongBlob => {
            datum.SetString(datum.GetString(), ft.GetCollate().to_string());
            Ok(datum)
        }
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeYear
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeDouble => Ok(datum),
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
            let mut t = types::NewTime(types::ZeroCoreTime, ft.GetType(), ft.GetDecimal() as i32);
            t.FromPackedUint(datum.GetUint64()).map_err(trace_error)?;
            if ft.GetType() == mysql::TypeTimestamp && !t.IsZero() {
                t.ConvertTimeZone(time::UTC, loc.unwrap_or(time::UTC))
                    .map_err(trace_error)?;
            }
            datum.SetUint64(0);
            datum.SetMysqlTime(t);
            Ok(datum)
        }
        mysql::TypeDuration => {
            // duration should read fsp from column meta data
            let dur = types::Duration {
                Duration: time::Duration(datum.GetInt64()),
                Fsp: ft.GetDecimal() as i32,
            };
            datum.SetMysqlDuration(dur);
            Ok(datum)
        }
        mysql::TypeEnum => {
            // ignore error deliberately, to read empty enum value.
            let enumv = types::ParseEnumValue(ft.GetElems(), datum.GetUint64())
                .unwrap_or_else(|_| types::Enum::default());
            datum.SetMysqlEnum(enumv, ft.GetCollate().to_string());
            Ok(datum)
        }
        mysql::TypeSet => {
            let set =
                types::ParseSetValue(ft.GetElems(), datum.GetUint64()).map_err(trace_error)?;
            datum.SetMysqlSet(set, ft.GetCollate().to_string());
            Ok(datum)
        }
        mysql::TypeBit => {
            let val = datum.GetUint64();
            let byteSize = (ft.GetFlen() + 7) >> 3;
            datum.SetUint64(0);
            datum.SetMysqlBit(types::NewBinaryLiteralFromUint(val, byteSize));
            Ok(datum)
        }
        _ => Ok(datum),
    }
}

// EncodeIndexSeekKey encodes an index value to kv.Key.
/// 编码索引 seek key：`t{tableID}_i{idxID}` + 可选已编码索引列。
pub fn EncodeIndexSeekKey(tableID: i64, idxID: i64, encodedValue: Option<Vec<u8>>) -> kv::Key {
    let encodedValue = encodedValue.unwrap_or_default();
    let mut key = Vec::with_capacity(RecordRowKeyLen + encodedValue.len());
    key = appendTableIndexPrefix(key, tableID);
    key = codec::EncodeInt(key, idxID);
    key.extend_from_slice(&encodedValue);
    kv::Key(key)
}

// CutIndexKey cuts encoded index key into colIDs to bytes slices map.
// The returned value b is the remaining bytes of the key which would be empty if it is unique index or handle data
// if it is non-unique index.
/// 切割索引 key 为前缀与各列值字节。
pub fn CutIndexKey(
    key: kv::Key,
    colIDs: Vec<i64>,
) -> Result<(HashMap<i64, Vec<u8>>, Vec<u8>), errors::SharedError> {
    let mut b = key.0[prefixLen + idLen..].to_vec();
    let mut values = HashMap::with_capacity(colIDs.len());
    for id in colIDs {
        let (val, remain) = codec::CutOne(b).map_err(trace_error)?;
        b = remain;
        values.insert(id, val);
    }
    Ok((values, b))
}

// CutIndexPrefix cuts the index prefix.
/// 去掉索引前缀，保留索引列编码。
pub fn CutIndexPrefix(key: kv::Key) -> Vec<u8> {
    key.0[prefixLen + idLen..].to_vec()
}

// CutIndexKeyTo cuts encoded index key into colIDs to bytes slices.
// The caller should prepare the memory of the result values.
/// 将索引 key 切割结果写入调用方提供的缓冲。
pub fn CutIndexKeyTo(
    key: kv::Key,
    values: &mut Vec<Vec<u8>>,
) -> Result<Vec<u8>, errors::SharedError> {
    let mut b = key.0[prefixLen + idLen..].to_vec();
    for i in 0..values.len() {
        let (val, remain) = codec::CutOne(b).map_err(trace_error)?;
        b = remain;
        values[i] = val;
    }
    Ok(b)
}

// CutIndexKeyNew cuts encoded index key into colIDs to bytes slices.
// The returned value b is the remaining bytes of the key which would be empty if it is unique index or handle data
// if it is non-unique index.
/// 新版切割：返回前缀与列值切片列表。
pub fn CutIndexKeyNew(
    key: kv::Key,
    length: usize,
) -> Result<(Vec<Vec<u8>>, Vec<u8>), errors::SharedError> {
    let mut values = vec![Vec::new(); length];
    let b = CutIndexKeyTo(key, &mut values)?;
    Ok((values, b))
}

// CutCommonHandle cuts encoded common handle key into colIDs to bytes slices.
// The returned value b is the remaining bytes of the key which would be empty if it is unique index or handle data
// if it is non-unique index.
/// 从 common handle 编码中按列切割。
pub fn CutCommonHandle(
    key: kv::Key,
    length: usize,
) -> Result<(Vec<Vec<u8>>, Vec<u8>), errors::SharedError> {
    let mut b = key.0[prefixLen..].to_vec();
    let mut values = Vec::with_capacity(length);
    for _ in 0..length {
        let (val, remain) = codec::CutOne(b).map_err(trace_error)?;
        b = remain;
        values.push(val);
    }
    Ok((values, b))
}

// HandleStatus is the handle status in index.
/// 索引解码时对 handle 的处理策略。
pub type HandleStatus = i32;

// HandleDefault means decode handle value as int64 or bytes when DecodeIndexKV.
/// 默认：按有符号整数处理 handle。
pub const HandleDefault: HandleStatus = 0;
// HandleIsUnsigned means decode handle value as uint64 when DecodeIndexKV.
/// handle 按无符号整数解释。
pub const HandleIsUnsigned: HandleStatus = 1;
// HandleNotNeeded means no need to decode handle value when DecodeIndexKV.
/// 不需要从索引中解析 handle。
pub const HandleNotNeeded: HandleStatus = 2;

// reEncodeHandle encodes the handle as a Datum so it can be properly decoded later.
// If it is common handle, it returns the encoded column values.
// If it is int handle, it is encoded as int Datum or uint Datum decided by the unsigned.
/// 按列信息重新编码 handle 到索引 value。
pub fn reEncodeHandle(
    handle: Box<dyn kv::Handle>,
    unsigned: bool,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    let handleColLen = if handle.IsInt() { 1 } else { handle.NumCols() };
    let result = Vec::with_capacity(handleColLen);
    reEncodeHandleTo(handle, unsigned, Vec::new(), result)
}

// reEncodeHandleTo 保留 Go 的可复用 buf/result 参数，用于减少 index decode 的分配。
/// 将 handle 重编码写入指定缓冲。
pub fn reEncodeHandleTo(
    handle: Box<dyn kv::Handle>,
    unsigned: bool,
    buf: Vec<u8>,
    mut result: Vec<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    if !handle.IsInt() {
        for i in 0..handle.NumCols() {
            result.push(handle.EncodedCol(i));
        }
        return Ok(result);
    }
    let mut handleDatum = types::NewIntDatum(handle.IntValue());
    if unsigned {
        handleDatum.SetUint64(handleDatum.GetUint64());
    }
    let intHandleBytes =
        codec::EncodeValue(time::UTC, buf, vec![handleDatum]).map_err(trace_error)?;
    result.push(intHandleBytes);
    Ok(result)
}

// reEncodeHandleConsiderNewCollation encodes the handle as a Datum so it can be properly decoded later.
/// 考虑新 collation 规则重编码 handle。
pub fn reEncodeHandleConsiderNewCollation(
    useNewCollate: bool,
    handle: Box<dyn kv::Handle>,
    columns: Vec<rowcodec::ColInfo>,
    restoreData: Vec<u8>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    let mut cHandleBytes = Vec::with_capacity(handle.NumCols());
    for i in 0..handle.NumCols() {
        cHandleBytes.push(handle.EncodedCol(i));
    }
    if restoreData.is_empty() {
        return Ok(cHandleBytes);
    }
    // Remove some extra columns(ID < 0), such like `model.ExtraPhysTblID`.
    // They are not belong to common handle and no need to restore data.
    let mut idx = columns.len();
    while idx > 0 && columns[idx - 1].ID < 0 {
        idx -= 1;
    }
    decodeRestoredValuesV5(
        useNewCollate,
        clone_col_infos(&columns[..idx]),
        cHandleBytes,
        restoreData,
    )
}

/// 克隆列信息切片。
fn clone_col_infos(columns: &[rowcodec::ColInfo]) -> Vec<rowcodec::ColInfo> {
    columns
        .iter()
        .map(|column| rowcodec::ColInfo {
            ID: column.ID,
            IsPKHandle: column.IsPKHandle,
            VirtualGenCol: column.VirtualGenCol,
            Ft: column.Ft.clone(),
        })
        .collect()
}

// decodeRestoredValues 解码 v4.0 旧恢复数据格式。
/// 解码索引 value 中的 RestoreData（旧 collation 路径）。
pub fn decodeRestoredValues(
    columns: Vec<rowcodec::ColInfo>,
    restoredVal: Vec<u8>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    let mut colIDs = HashMap::with_capacity(columns.len());
    for (i, col) in columns.iter().enumerate() {
        colIDs.insert(col.ID, i);
    }
    // We don't need to decode handle here, and colIDs >= 0 always.
    let rd = rowcodec::NewByteDecoder(columns, vec![-1], None, None);
    rd.DecodeToBytesNoHandle(&colIDs, &restoredVal)
        .map_err(trace_error)
}

// decodeRestoredValuesV5 decodes index values whose format is introduced in TiDB 5.0.
// Unlike the format in TiDB 4.0, the new format is optimized for storage space:
// 1. If the index is a composed index, only the non-binary string column's value need to write to value, not all.
// 2. If a string column's collation is _bin, then we only write the number of the truncated spaces to value.
// 3. If a string column is char, not varchar, then we use the sortKey directly.
/// 解码索引 value 中的 RestoreData（v5 / 新 collation）。
pub fn decodeRestoredValuesV5(
    useNewCollate: bool,
    columns: Vec<rowcodec::ColInfo>,
    mut results: Vec<Vec<u8>>,
    restoredVal: Vec<u8>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    let colIDOffsets = buildColumnIDOffsets(clone_col_infos(&columns));
    let colInfosNeedRestore = buildRestoredColumn(useNewCollate, clone_col_infos(&columns));
    let rd = rowcodec::NewByteDecoder(colInfosNeedRestore, Vec::new(), None, None);
    let mut newResults = rd
        .DecodeToBytesNoHandle(&colIDOffsets, &restoredVal)
        .map_err(trace_error)?;
    for i in 0..newResults.len() {
        let noRestoreData = newResults[i].is_empty();
        if noRestoreData {
            newResults[i] = results[i].clone();
            continue;
        }
        if collate::IsBinCollation(columns[i].Ft.GetCollate()) {
            let noPaddingDatum =
                DecodeColumnValue(results[i].clone(), Box::new(columns[i].Ft.clone()), None)?;
            let paddingCountDatum = DecodeColumnValue(
                newResults[i].clone(),
                types::NewFieldType(mysql::TypeLonglong),
                None,
            )?;
            let noPaddingStr = noPaddingDatum.GetString();
            let paddingCount = paddingCountDatum.GetInt64() as usize;
            // Skip if padding count is 0.
            if paddingCount == 0 {
                newResults[i] = results[i].clone();
                continue;
            }
            let mut newDatum = noPaddingDatum;
            newDatum.SetString(
                format!("{}{}", noPaddingStr, " ".repeat(paddingCount)),
                newDatum.Collation(),
            );
            newResults[i].clear();
            newResults[i].push(rowcodec::BytesFlag);
            newResults[i] = codec::EncodeBytes(newResults[i].clone(), &newDatum.GetBytes());
        }
    }
    Ok(newResults)
}

// buildColumnIDOffsets 构造 colID 到列偏移的映射。
/// 构建列 ID 到偏移的映射。
pub fn buildColumnIDOffsets(allCols: Vec<rowcodec::ColInfo>) -> HashMap<i64, usize> {
    let mut colIDOffsets = HashMap::with_capacity(allCols.len());
    for (i, col) in allCols.iter().enumerate() {
        colIDOffsets.insert(col.ID, i);
    }
    colIDOffsets
}

// buildRestoredColumn 根据新 collate 规则挑出需要恢复数据的列，并为 _bin 字符串改用 unsigned longlong。
/// 根据索引列构建 RestoreData 所需列布局。
pub fn buildRestoredColumn(
    useNewCollate: bool,
    allCols: Vec<rowcodec::ColInfo>,
) -> Vec<rowcodec::ColInfo> {
    let mut restoredColumns = Vec::with_capacity(allCols.len());
    for (i, col) in allCols.iter().enumerate() {
        if !types::NeedRestoredDataWithCollate(&col.Ft, useNewCollate) {
            continue;
        }
        let mut copyColInfo = rowcodec::ColInfo {
            ID: col.ID,
            IsPKHandle: col.IsPKHandle,
            VirtualGenCol: col.VirtualGenCol,
            Ft: col.Ft.clone(),
        };
        if collate::IsBinCollation(col.Ft.GetCollate()) {
            // Change the fieldType from string to uint since we store the number of the truncated spaces.
            // NOTE: the corresponding datum is generated as `types.NewUintDatum(paddingSize)`, and the raw data is
            // encoded via `encodeUint`. Thus we should mark the field type as unsigened here so that the BytesDecoder
            // can decode it correctly later. Otherwise there might be issues like #47115.
            copyColInfo.Ft = *types::NewFieldType(mysql::TypeLonglong);
            copyColInfo.Ft.AddFlag(mysql::UnsignedFlag);
        } else {
            copyColInfo.Ft = allCols[i].Ft.clone();
        }
        restoredColumns.push(copyColInfo);
    }
    restoredColumns
}

// decodeIndexKvOldCollation 解码旧 collation/旧 index value 布局。
/// 旧 collation 下解码索引 KV 的 handle 与列值。
pub fn decodeIndexKvOldCollation(
    key: Vec<u8>,
    value: Vec<u8>,
    hdStatus: HandleStatus,
    buf: Vec<u8>,
    mut resultValues: Vec<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    let b = CutIndexKeyTo(kv::Key(key), &mut resultValues)?;
    if hdStatus == HandleNotNeeded {
        return Ok(resultValues);
    }
    let handle: Box<dyn kv::Handle>;
    if !b.is_empty() {
        // non-unique index
        handle = decodeHandleInIndexKey(b)?;
        resultValues = reEncodeHandleTo(handle, hdStatus == HandleIsUnsigned, buf, resultValues)?;
    } else {
        // In unique int handle index.
        handle = DecodeIntHandleInIndexValue(value);
        resultValues = reEncodeHandleTo(handle, hdStatus == HandleIsUnsigned, buf, resultValues)?;
    }
    Ok(resultValues)
}

// getIndexVersion 检查 index value 尾部 version 标记；没有标记时返回 0。
/// 从索引 value 中读取版本号。
pub fn getIndexVersion(value: &[u8]) -> i32 {
    if value.len() <= MaxOldEncodeValueLen {
        return 0;
    }
    let tailLen = value[0] as usize;
    if (tailLen == 0 || tailLen == 1) && value[1] == IndexVersionFlag {
        return value[2] as i32;
    }
    0
}

// DecodeIndexKVEx looks like DecodeIndexKV, the difference is that it tries to reduce allocations.
/// 扩展版索引 KV 解码，支持更多选项。
pub fn DecodeIndexKVEx(
    key: Vec<u8>,
    value: Vec<u8>,
    colsLen: usize,
    hdStatus: HandleStatus,
    columns: Vec<rowcodec::ColInfo>,
    buf: Vec<u8>,
    preAlloc: Vec<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    if value.len() <= MaxOldEncodeValueLen {
        return decodeIndexKvOldCollation(key, value, hdStatus, buf, preAlloc);
    }
    if getIndexVersion(&value) == 1 {
        return decodeIndexKvForClusteredIndexVersion1(
            collate::NewCollationEnabled(),
            key,
            value,
            colsLen,
            hdStatus,
            columns,
        );
    }
    decodeIndexKvGeneral(key, value, colsLen, hdStatus, columns)
}

// DecodeIndexKV uses to decode index key values.
//
//	`colsLen` is expected to be index columns count.
//	`columns` is expected to be index columns + handle columns(if hdStatus is not HandleNotNeeded).
/// 解码索引 key/value，得到 handle 与列值。
pub fn DecodeIndexKV(
    key: Vec<u8>,
    value: Vec<u8>,
    colsLen: usize,
    hdStatus: HandleStatus,
    columns: Vec<rowcodec::ColInfo>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    DecodeIndexKVWithCollate(
        collate::NewCollationEnabled(),
        key,
        value,
        colsLen,
        hdStatus,
        columns,
    )
}

// DecodeIndexKVWithCollate is similar to DecodeIndexKV but with explicit useNewCollate param.
/// 带 collation 信息的索引 KV 解码。
pub fn DecodeIndexKVWithCollate(
    useNewCollate: bool,
    key: Vec<u8>,
    value: Vec<u8>,
    colsLen: usize,
    hdStatus: HandleStatus,
    columns: Vec<rowcodec::ColInfo>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    if value.len() <= MaxOldEncodeValueLen {
        let preAlloc = vec![Vec::new(); colsLen];
        return decodeIndexKvOldCollation(key, value, hdStatus, Vec::new(), preAlloc);
    }
    if getIndexVersion(&value) == 1 {
        return decodeIndexKvForClusteredIndexVersion1(
            useNewCollate,
            key,
            value,
            colsLen,
            hdStatus,
            columns,
        );
    }
    decodeIndexKvGeneral(key, value, colsLen, hdStatus, columns)
}

// DecodeIndexHandle uses to decode the handle from index key/value.
// DecodeIndexHandle 先跳过 index columns；非唯一索引从 key suffix 解 handle，唯一索引从 value 解 handle。
/// 仅从索引 KV 中解码 handle。
pub fn DecodeIndexHandle(
    key: Vec<u8>,
    value: Vec<u8>,
    colsLen: usize,
) -> Result<Option<Box<dyn kv::Handle>>, errors::SharedError> {
    let mut b = key[prefixLen + idLen..].to_vec();
    for _ in 0..colsLen {
        let (_, remain) = codec::CutOne(b).map_err(trace_error)?;
        b = remain;
    }
    if !b.is_empty() {
        let mut handle = decodeHandleInIndexKey(b)?;
        // If len(value) >= 9, it may contain partition id.
        // We should decode it and return a partition handle.
        if value.len() >= 9 {
            let seg = SplitIndexValue(value);
            if !seg.PartitionID.is_empty() {
                let (_, pid) = codec::DecodeInt(&seg.PartitionID).map_err(trace_error)?;
                // For GlobalIndexVersionV1+, the handle from the key may already be a
                // PartitionHandle (partition ID encoded in key). To avoid creating a
                // nested PartitionHandle, extract the inner handle first.
                // For V1: use partition ID from value (authoritative source).
                // TODO: For V2+, use partition ID from key (PartitionHandle) instead.
                if let Some(ph) = handle.as_any().downcast_ref::<kv::PartitionHandle>() {
                    handle = ph.Handle.Copy();
                }
                handle = Box::new(kv::NewPartitionHandle(pid, handle));
            }
        }
        return Ok(Some(handle));
    } else if value.len() >= 8 {
        return DecodeHandleInIndexValue(value);
    }
    // Should never execute to here.
    Err(errors::New(format!(
        "no handle in index key: {:?}, value: {:?}",
        key, value
    )))
}

// decodeHandleInIndexKey 从非唯一 index key suffix 中解析 handle，并支持 V1+ 全局索引的 PartitionHandle 前缀。
/// 从非唯一索引 key 尾部解析 handle。
pub fn decodeHandleInIndexKey(
    mut keySuffix: Vec<u8>,
) -> Result<Box<dyn kv::Handle>, errors::SharedError> {
    // Check if this is a PartitionHandle (for global non-unique indexes V1+)
    if !keySuffix.is_empty() && keySuffix[0] == PartitionIDFlag {
        // Format: PartitionIDFlag + partition_id (8 bytes) + inner_handle
        keySuffix = keySuffix[1..].to_vec(); // Skip the flag
        let (remain, partID) = codec::DecodeInt(&keySuffix).map_err(trace_error)?;
        // Decode the inner handle
        let innerHandle = decodeHandleInIndexKey(remain.to_vec())?;
        return Ok(Box::new(kv::NewPartitionHandle(partID, innerHandle)));
    }

    let (remain, d) = codec::DecodeOne(&keySuffix).map_err(trace_error)?;
    if remain.is_empty() && d.Kind() == types::KindInt64 {
        return Ok(Box::new(kv::IntHandle(d.GetInt64())));
    }
    kv::NewCommonHandle(keySuffix)
        .map(|handle| Box::new(handle) as Box<dyn kv::Handle>)
        .map_err(trace_error)
}

// DecodeHandleInIndexValue decodes handle in unqiue index value.
/// 从唯一索引 value 中解析 handle。
pub fn DecodeHandleInIndexValue(
    value: Vec<u8>,
) -> Result<Option<Box<dyn kv::Handle>>, errors::SharedError> {
    if value.len() <= MaxOldEncodeValueLen {
        return Ok(Some(DecodeIntHandleInIndexValue(value)));
    }
    let seg = SplitIndexValue(value);
    let mut handle: Option<Box<dyn kv::Handle>> = None;
    if !seg.IntHandle.is_empty() {
        handle = Some(DecodeIntHandleInIndexValue(seg.IntHandle));
    }
    if !seg.CommonHandle.is_empty() {
        handle = Some(Box::new(
            kv::NewCommonHandle(seg.CommonHandle).map_err(trace_error)?,
        ));
    }
    if !seg.PartitionID.is_empty() {
        let (_, pid) = codec::DecodeInt(&seg.PartitionID).map_err(trace_error)?;
        if let Some(inner) = handle {
            handle = Some(Box::new(kv::NewPartitionHandle(pid, inner)));
        }
    }
    Ok(handle)
}

// DecodeIntHandleInIndexValue uses to decode index value as int handle id.
/// 将索引 value 中的整数 handle 字节解码为 IntHandle。
pub fn DecodeIntHandleInIndexValue(data: Vec<u8>) -> Box<dyn kv::Handle> {
    Box::new(kv::IntHandle(
        u64::from_be_bytes(data[..8].try_into().unwrap()) as i64,
    ))
}

// EncodeTableIndexPrefix encodes index prefix with tableID and idxID.
/// 生成表索引前缀 `t{tableID}_i{idxID}`。
pub fn EncodeTableIndexPrefix(tableID: i64, idxID: i64) -> kv::Key {
    let mut key = Vec::with_capacity(prefixLen + idLen);
    key = appendTableIndexPrefix(key, tableID);
    key = codec::EncodeInt(key, idxID);
    kv::Key(key)
}

// EncodeTablePrefix encodes the table prefix to generate a key
/// 生成表前缀 `t{tableID}`。
pub fn EncodeTablePrefix(tableID: i64) -> kv::Key {
    let mut key = Vec::with_capacity(tablePrefixLength + idLen);
    key.extend_from_slice(tablePrefix);
    kv::Key(codec::EncodeInt(key, tableID))
}

// appendTableRecordPrefix appends table record prefix  "t[tableID]_r".
/// 向缓冲追加 `t{tableID}_r`。
pub fn appendTableRecordPrefix(mut buf: Vec<u8>, tableID: i64) -> Vec<u8> {
    buf.extend_from_slice(tablePrefix);
    buf = codec::EncodeInt(buf, tableID);
    buf.extend_from_slice(recordPrefixSep);
    buf
}

// appendTableIndexPrefix appends table index prefix  "t[tableID]_i".
/// 向缓冲追加 `t{tableID}_i`。
pub fn appendTableIndexPrefix(mut buf: Vec<u8>, tableID: i64) -> Vec<u8> {
    buf.extend_from_slice(tablePrefix);
    buf = codec::EncodeInt(buf, tableID);
    buf.extend_from_slice(indexPrefixSep);
    buf
}

// GenTableRecordPrefix composes record prefix with tableID: "t[tableID]_r".
/// 生成表记录前缀 key。
pub fn GenTableRecordPrefix(tableID: i64) -> kv::Key {
    let buf = Vec::with_capacity(tablePrefix.len() + 8 + recordPrefixSep.len());
    kv::Key(appendTableRecordPrefix(buf, tableID))
}

// GenTableIndexPrefix composes index prefix with tableID: "t[tableID]_i".
/// 生成表索引区域前缀 key。
pub fn GenTableIndexPrefix(tableID: i64) -> kv::Key {
    let buf = Vec::with_capacity(tablePrefix.len() + 8 + indexPrefixSep.len());
    kv::Key(appendTableIndexPrefix(buf, tableID))
}

// IsRecordKey is used to check whether the key is an record key.
/// 判断是否为行记录 key。
pub fn IsRecordKey(k: &[u8]) -> bool {
    k.len() > 11 && k[0] == b't' && k[10] == b'r'
}

// IsIndexKey is used to check whether the key is an index key.
/// 判断是否为索引 key。
pub fn IsIndexKey(k: &[u8]) -> bool {
    k.len() > 11 && k[0] == b't' && k[10] == b'i'
}

// IsTableKey is used to check whether the key is a table key.
/// 判断是否为表相关 key（含表前缀）。
pub fn IsTableKey(k: &[u8]) -> bool {
    k.len() == 9 && k[0] == b't'
}

// IsUntouchedIndexKValue uses to check whether the key is index key, and the value is untouched,
// since the untouched index key/value is no need to commit.
/// 判断索引 KV 是否为 untouched（未修改）标记。
pub fn IsUntouchedIndexKValue(k: &[u8], v: &[u8]) -> bool {
    if !IsIndexKey(k) {
        return false;
    }
    let vLen = v.len();
    if IsTempIndexKey(k) {
        return vLen > 0 && v[vLen - 1] == kv::UnCommitIndexKVFlag;
    }
    if vLen <= MaxOldEncodeValueLen {
        // vLen = 1/9 for legacy layout, 4 for common-handle-v1 layout
        return (vLen == 1 || vLen == 9 || vLen == 4) && v[vLen - 1] == kv::UnCommitIndexKVFlag;
    }
    // New index value format
    let tailLen = v[0] as usize;
    if tailLen < 8 {
        // Non-unique index.
        return tailLen >= 1 && v[vLen - 1] == kv::UnCommitIndexKVFlag;
    }
    // Unique index
    tailLen == 9
}

// GenTablePrefix composes table record and index prefix: "t[tableID]".
/// 生成表级前缀（用于分裂等）。
pub fn GenTablePrefix(tableID: i64) -> kv::Key {
    let mut buf = Vec::with_capacity(tablePrefix.len() + 8);
    buf.extend_from_slice(tablePrefix);
    kv::Key(codec::EncodeInt(buf, tableID))
}

// TruncateToRowKeyLen truncates the key to row key length if the key is longer than row key.
/// 将 key 截断到标准行 key 长度。
pub fn TruncateToRowKeyLen(key: kv::Key) -> kv::Key {
    if key.0.len() > RecordRowKeyLen {
        return kv::Key(key.0[..RecordRowKeyLen].to_vec());
    }
    key
}

// GetTableHandleKeyRange returns table handle's key range with tableID.
/// 返回表全部行记录的 [start, end) key 范围。
pub fn GetTableHandleKeyRange(tableID: i64) -> (Vec<u8>, Vec<u8>) {
    let startKey = EncodeRowKeyWithHandle(tableID, Box::new(kv::IntHandle(i64::MIN)));
    let endKey = EncodeRowKeyWithHandle(tableID, Box::new(kv::IntHandle(i64::MAX)));
    (startKey.0, endKey.0)
}

// GetTableIndexKeyRange returns table index's key range with tableID and indexID.
/// 返回指定索引的 [start, end) key 范围。
pub fn GetTableIndexKeyRange(tableID: i64, indexID: i64) -> (Vec<u8>, Vec<u8>) {
    let startKey = EncodeIndexSeekKey(tableID, indexID, None);
    let endKey = EncodeIndexSeekKey(tableID, indexID, Some(vec![255]));
    (startKey.0, endKey.0)
}

// GetIndexKeyBuf reuse or allocate buffer
/// 复用或新建索引 key 编码缓冲。
pub fn GetIndexKeyBuf(buf: Option<Vec<u8>>, defaultCap: usize) -> Vec<u8> {
    if let Some(mut buf) = buf {
        buf.clear();
        return buf;
    }
    Vec::with_capacity(defaultCap)
}

// GenIndexKey generates index key using input physical table id
/// 根据表/索引元数据与列值生成索引 key，并返回是否唯一等信息。
pub fn GenIndexKey(
    enc: codec::Encoder,
    loc: Option<time::Location>,
    tblInfo: Box<model::TableInfo>,
    idxInfo: Box<model::IndexInfo>,
    phyTblID: i64,
    mut indexedValues: Vec<types::Datum>,
    h: Option<Box<dyn kv::Handle>>,
    buf: Option<Vec<u8>>,
) -> Result<(Vec<u8>, bool), errors::SharedError> {
    let mut distinct = false;
    if idxInfo.Unique {
        // See https://dev.mysql.com/doc/refman/5.7/en/create-index.html
        // A UNIQUE index creates a constraint such that all values in the index must be distinct.
        // An error occurs if you try to add a new row with a key value that matches an existing row.
        // For all engines, a UNIQUE index permits multiple NULL values for columns that can contain NULL.
        distinct = true;
        for cv in &indexedValues {
            if cv.IsNull() {
                distinct = false;
                break;
            }
        }
    }
    // For string columns, indexes can be created using only the leading part of column values,
    // using col_name(length) syntax to specify an index prefix length.
    TruncateIndexValues(tblInfo.clone(), idxInfo.clone(), &mut indexedValues);
    let mut key = GetIndexKeyBuf(buf, RecordRowKeyLen + indexedValues.len() * 9 + 9);
    key = appendTableIndexPrefix(key, phyTblID);
    key = codec::EncodeInt(key, idxInfo.ID);
    key = enc
        .EncodeKey(loc.unwrap_or(time::UTC), key, indexedValues)
        .map_err(trace_error)?;
    if !distinct {
        if let Some(h) = h {
            // For PartitionHandle on global indexes V1+, we must encode BOTH partition ID and inner handle
            // in the key to prevent collisions when different partitions have duplicate handles.
            // This is critical after EXCHANGE PARTITION, which can create duplicate _tidb_rowid values.
            // Only use the new format for version >= V1. Legacy indexes (version 0) use the old format.
            if idxInfo.GlobalIndexVersion >= model::GlobalIndexVersionV1 {
                if tblInfo.HasClusteredIndex() {
                    return Err(errors::New(
                        "clustered index is not supported in GlobalIndexVersionV1+",
                    ));
                }
                let Some(ph) = h.as_any().downcast_ref::<kv::PartitionHandle>() else {
                    return Err(errors::New(
                        "handle is not a PartitionHandle in GlobalIndexVersionV1+",
                    ));
                };
                // Encode as: PartitionIDFlag + partition_id (8 bytes) + inner_handle_encoded
                key.push(PartitionIDFlag);
                key = codec::EncodeInt(key, ph.PartitionID);
            }

            if h.IsInt() {
                // We choose the efficient path here instead of calling `codec.EncodeKey`
                // because the int handle must be an int64, and it must be comparable.
                // This remains correct until codec.encodeSignedInt is changed.
                key.push(codec::IntHandleFlag);
                key = codec::EncodeInt(key, h.IntValue());
            } else {
                key.extend_from_slice(&h.Encoded());
            }
        }
    }
    Ok((key, distinct))
}

// TempIndexPrefix used to generate temporary index ID from index ID.
/// 临时索引 ID 高位标记，用于 DDL backfill 阶段。
pub const TempIndexPrefix: i64 = 0x7fff000000000000;

// IndexIDMask used to get index id from index ID/temp index ID.
/// 从临时索引 ID 还原真实 indexID 的掩码。
pub const IndexIDMask: i64 = 0xffffffffffff;

// IndexKey2TempIndexKey generates a temporary index key.
/// 就地将普通索引 key 转为临时索引 key（改写 indexID 高位）。
pub fn IndexKey2TempIndexKey(key: &mut [u8]) {
    let idxIDBytes = &key[prefixLen..prefixLen + idLen];
    let idxID = codec::DecodeCmpUintToInt(u64::from_be_bytes(idxIDBytes.try_into().unwrap()));
    let eid = codec::EncodeIntToCmpUint(TempIndexPrefix | idxID);
    key[prefixLen..prefixLen + idLen].copy_from_slice(&eid.to_be_bytes());
}

// TempIndexKey2IndexKey generates an index key from temporary index key.
/// 就地将临时索引 key 还原为普通索引 key。
pub fn TempIndexKey2IndexKey(tempIdxKey: &mut [u8]) {
    let tmpIdxIDBytes = &tempIdxKey[prefixLen..prefixLen + idLen];
    let tempIdxID =
        codec::DecodeCmpUintToInt(u64::from_be_bytes(tmpIdxIDBytes.try_into().unwrap()));
    let eid = codec::EncodeIntToCmpUint(tempIdxID & IndexIDMask);
    tempIdxKey[prefixLen..prefixLen + idLen].copy_from_slice(&eid.to_be_bytes());
}

// IsTempIndexKey checks whether the input key is for a temp index.
/// 判断索引 key 是否带有临时索引前缀标记。
pub fn IsTempIndexKey(indexKey: &[u8]) -> bool {
    let indexIDKey = &indexKey[prefixLen..prefixLen + 8];
    let indexID = codec::DecodeCmpUintToInt(u64::from_be_bytes(indexIDKey.try_into().unwrap()));
    let tempIndexID = TempIndexPrefix | indexID;
    tempIndexID == indexID
}

// TempIndexValueFlag is the flag of temporary index value.
/// 临时索引 value 元素类型 flag。
pub type TempIndexValueFlag = u8;

// TempIndexValueFlagNormal means the following value is a distinct the normal index value.
/// 普通（distinct）临时索引 value。
pub const TempIndexValueFlagNormal: TempIndexValueFlag = 0;
// TempIndexValueFlagNonDistinctNormal means the following value is the non-distinct normal index value.
/// 非 distinct 的普通临时索引 value。
pub const TempIndexValueFlagNonDistinctNormal: TempIndexValueFlag = 1;
// TempIndexValueFlagDeleted means the following value is the distinct and deleted index value.
/// 已删除标记的临时索引 value。
pub const TempIndexValueFlagDeleted: TempIndexValueFlag = 2;
// TempIndexValueFlagNonDistinctDeleted means the following value is the non-distinct deleted index value.
/// 非 distinct 的删除标记临时索引 value。
pub const TempIndexValueFlagNonDistinctDeleted: TempIndexValueFlag = 3;

// TempIndexValue is the value of temporary index.
// It contains one or more element, each element represents a history index operations on the original index.
// A temp index value element is encoded as one of:
//   - [flag 1 byte][value_length 2 bytes ] [value value_len bytes]   [key_version 1 byte] {distinct normal}
//   - [flag 1 byte][value value_len bytes]                           [key_version 1 byte] {non-distinct normal}
//   - [flag 1 byte][handle_length 2 bytes] [handle handle_len bytes] [key_version 1 byte] {distinct deleted}
//   - [flag 1 byte]                                                  [key_version 1 byte] {non-distinct deleted}
//
// The temp index value is encoded as:
//   - [element 1][element 2]...[element n] {for distinct values}
//   - [element 1]                          {for non-distinct values}
/// 临时索引 value：多个可选元素组成的列表。
pub type TempIndexValue = Vec<Option<Box<TempIndexValueElem>>>;

// TempIndexValueExt 为 Go 的 TempIndexValue 方法提供 Rust trait 包装。
/// 临时索引 value 的编码与查询扩展方法。
pub trait TempIndexValueExt {
    /// 是否为空临时索引 value。
    fn IsEmpty(&self) -> bool;
    /// 返回最新一个临时索引操作元素。
    fn Current(&self) -> Option<&TempIndexValueElem>;
    /// 合并阶段过滤被后续操作覆盖的历史元素。
    fn FilterOverwritten(self) -> TempIndexValue;
}

impl TempIndexValueExt for TempIndexValue {
    // IsEmpty checks whether the value is empty.
    fn IsEmpty(&self) -> bool {
        self.is_empty()
    }

    // Current returns the current latest temp index value.
    fn Current(&self) -> Option<&TempIndexValueElem> {
        self.last().and_then(|v| v.as_deref())
    }

    // FilterOverwritten is used by the temp index merge process to remove the overwritten index operations.
    // For example, the value {temp_idx_key -> [h2, h2d, h3, h1d]} recorded four operations on the original index.
    // Since 'h2d' overwrites 'h2', we can remove 'h2' from the value.
    fn FilterOverwritten(mut self) -> TempIndexValue {
        if self.len() <= 1 || !self[0].as_ref().map(|v| v.Distinct).unwrap_or(false) {
            return self;
        }
        // Go hydrates normal distinct handles from the encoded index value
        // before filtering. Rust uses a non-null placeholder handle, so do the
        // equivalent here before comparing history entries.
        for elem in self.iter_mut().flatten() {
            if !elem.Delete
                && let Ok(Some(handle)) = DecodeHandleInIndexValue(elem.Value.clone())
            {
                elem.Handle = handle;
            }
        }
        // 从后往前扫描：同一 handle 只保留最近一次操作，更早的置空后滤除。
        let mut occurred = kv::NewHandleMap();
        for i in (0..self.len()).rev() {
            let Some(elem) = self[i].as_ref() else {
                continue;
            };
            if occurred.Get(elem.Handle.as_ref()).is_none() {
                occurred.Set(elem.Handle.as_ref(), Box::new(()));
            } else {
                self[i] = None;
            }
        }
        self.into_iter().filter(|elem| elem.is_some()).collect()
    }
}

// TempIndexValueElem represents a history index operations on the original index.
// A temp index value element is encoded as one of:
//   - [flag 1 byte][value_length 2 bytes ] [value value_len bytes]   [key_version 1 byte] {distinct normal}
//   - [flag 1 byte][value value_len bytes]                           [key_version 1 byte] {non-distinct normal}
//   - [flag 1 byte][handle_length 2 bytes] [handle handle_len bytes] [partitionIdFlag 1 byte] [partitionID 8 bytes] [key_version 1 byte] {distinct deleted}
//   - [flag 1 byte]                                                  [key_version 1 byte] {non-distinct deleted}
/// 单个临时索引 value 元素：类型、handle、会话等。
pub struct TempIndexValueElem {
    pub Value: Vec<u8>,
    pub Handle: Box<dyn kv::Handle>,
    pub KeyVer: u8,
    pub Delete: bool,
    pub Distinct: bool,

    // Global means it's a global Index, for partitioned tables. Currently only used in `distinct` + `deleted` scenarios.
    pub Global: bool,
}

// TempIndexKeyTypeNone means the key is not a temporary index key.
/// 临时索引元素类型：无。
pub const TempIndexKeyTypeNone: u8 = 0;
// TempIndexKeyTypeDelete indicates this value is written in the delete-only stage.
/// 临时索引元素类型：删除。
pub const TempIndexKeyTypeDelete: u8 = b'd';
// TempIndexKeyTypeBackfill indicates this value is written in the backfill stage.
/// 临时索引元素类型：回填写入。
pub const TempIndexKeyTypeBackfill: u8 = b'b';
// TempIndexKeyTypeMerge indicates this value is written in the merge stage.
/// 临时索引元素类型：合并。
pub const TempIndexKeyTypeMerge: u8 = b'm';
// TempIndexKeyTypePartitionIDFlag indicates the following value is partition id.
/// 临时索引元素中携带 partition id 的标记。
pub const TempIndexKeyTypePartitionIDFlag: u8 = b'p';

impl TempIndexValueElem {
    // Encode encodes the temp index value.
    /// 按 Delete/Distinct/Global 组合编码单个临时索引元素。
    pub fn Encode(&self, buf: Option<Vec<u8>>) -> Vec<u8> {
        let mut buf = buf.unwrap_or_default();
        if self.Delete {
            if self.Distinct {
                let handle = self.Handle.Copy();
                let (hEncoded, hLen) = if handle.IsInt() {
                    (
                        codec::EncodeUint(Vec::new(), handle.IntValue() as u64),
                        idLen as u16,
                    )
                } else {
                    let encoded = handle.Encoded();
                    let len = encoded.len() as u16;
                    (encoded, len)
                };
                // flag + handle length + handle + [partition id] + temp key version
                if buf.capacity() == 0 {
                    let mut l = hLen as usize + 4;
                    if self.Global {
                        l += 9;
                    }
                    buf = Vec::with_capacity(l);
                }
                buf.push(TempIndexValueFlagDeleted);
                buf.push((hLen >> 8) as u8);
                buf.push(hLen as u8);
                buf.extend_from_slice(&hEncoded);
                if self.Global {
                    buf.push(TempIndexKeyTypePartitionIDFlag);
                    let ph = self
                        .Handle
                        .as_any()
                        .downcast_ref::<kv::PartitionHandle>()
                        .expect("global temp index delete uses PartitionHandle");
                    buf.extend_from_slice(&codec::EncodeInt(Vec::new(), ph.PartitionID));
                }
                buf.push(self.KeyVer);
                return buf;
            }
            // flag + temp key version
            if buf.capacity() == 0 {
                buf = Vec::with_capacity(2);
            }
            buf.push(TempIndexValueFlagNonDistinctDeleted);
            buf.push(self.KeyVer);
            return buf;
        }
        if self.Distinct {
            // flag + value length + value + temp key version
            if buf.capacity() == 0 {
                buf = Vec::with_capacity(self.Value.len() + 4);
            }
            buf.push(TempIndexValueFlagNormal);
            let vLen = self.Value.len() as u16;
            buf.push((vLen >> 8) as u8);
            buf.push(vLen as u8);
            buf.extend_from_slice(&self.Value);
            buf.push(self.KeyVer);
            return buf;
        }
        // flag + value + temp key version
        if buf.capacity() == 0 {
            buf = Vec::with_capacity(self.Value.len() + 2);
        }
        buf.push(TempIndexValueFlagNonDistinctNormal);
        buf.extend_from_slice(&self.Value);
        buf.push(self.KeyVer);
        buf
    }

    // DecodeOne decodes one temp index value element.
    pub fn DecodeOne(&mut self, mut b: Vec<u8>) -> Result<Vec<u8>, errors::SharedError> {
        let flag = b[0];
        b = b[1..].to_vec();
        match flag {
            TempIndexValueFlagNormal => {
                let vLen = ((b[0] as u16) << 8) + b[1] as u16;
                b = b[2..].to_vec();
                self.Value = b[..vLen as usize].to_vec();
                b = b[vLen as usize..].to_vec();
                self.KeyVer = b[0];
                b = b[1..].to_vec();
                self.Distinct = true;
                Ok(b)
            }
            TempIndexValueFlagNonDistinctNormal => {
                self.Value = b[..b.len() - 1].to_vec();
                self.KeyVer = b[b.len() - 1];
                Ok(Vec::new())
            }
            TempIndexValueFlagDeleted => {
                let hLen = ((b[0] as u16) << 8) + b[1] as u16;
                b = b[2..].to_vec();
                if hLen as usize == idLen {
                    self.Handle = DecodeIntHandleInIndexValue(b[..idLen].to_vec());
                } else {
                    self.Handle = Box::new(
                        kv::NewCommonHandle(b[..hLen as usize].to_vec()).map_err(trace_error)?,
                    );
                }
                b = b[hLen as usize..].to_vec();
                if b[0] == TempIndexKeyTypePartitionIDFlag {
                    self.Global = true;
                    let (_, pid) = codec::DecodeInt(&b[1..9]).map_err(trace_error)?;
                    self.Handle = Box::new(kv::NewPartitionHandle(pid, self.Handle.Copy()));
                    b = b[9..].to_vec();
                }
                self.KeyVer = b[0];
                b = b[1..].to_vec();
                self.Distinct = true;
                self.Delete = true;
                Ok(b)
            }
            TempIndexValueFlagNonDistinctDeleted => {
                self.KeyVer = b[0];
                b = b[1..].to_vec();
                self.Delete = true;
                Ok(b)
            }
            _ => Err(errors::New("invalid temp index value")),
        }
    }
}

// DecodeTempIndexValue decodes the temp index value.
/// 解码临时索引 value 字节为元素列表。
pub fn DecodeTempIndexValue(mut value: Vec<u8>) -> Result<TempIndexValue, errors::SharedError> {
    let mut values = Vec::new();
    while !value.is_empty() {
        let mut v = TempIndexValueElem {
            Value: Vec::new(),
            Handle: Box::new(kv::IntHandle(0)),
            KeyVer: 0,
            Delete: false,
            Distinct: false,
            Global: false,
        };
        value = v.DecodeOne(value)?;
        values.push(Some(Box::new(v)));
    }
    Ok(values)
}

// TempIndexValueIsUntouched returns true if the value is untouched.
// All the temp index value has the suffix of temp key version.
// All the temp key versions differ from the uncommitted KV flag.
/// 判断临时索引 value 是否为 untouched。
pub fn TempIndexValueIsUntouched(b: &[u8]) -> bool {
    !b.is_empty() && b[b.len() - 1] == kv::UnCommitIndexKVFlag
}

// GenIndexValuePortal is the portal for generating index value.
// TiDB has several physical index-value layouts. The layout is selected by
// table/index metadata and the current scenario; it is not a simple version
// upgrade path. A new cluster can still contain or generate more than one
// layout. The variants are:
//
//  1. Legacy compact layout, also known as "Old Encoding".
//  2. Extensible layout, used by common-handle unique indexes, global indexes,
//     and restored-data cases. This is also called IndexValueVersion0.
//  3. Clustered common-handle V1 layout, used by tables whose
//     CommonHandleVersion is 1. This is also called IndexValueForClusteredIndexVersion1.
//
// 该入口仅根据表元数据选择 Go 的两个生成实现：common handle v1 走新布局，其余走 version0/legacy 兼容路径。
/// 索引 value 生成入口：按版本/聚簇索引等分派具体实现。
pub fn GenIndexValuePortal(
    useNewCollate: bool,
    loc: Option<time::Location>,
    tblInfo: Box<model::TableInfo>,
    idxInfo: Box<model::IndexInfo>,
    needRestoredData: bool,
    distinct: bool,
    untouched: bool,
    indexedValues: Vec<types::Datum>,
    h: Box<dyn kv::Handle>,
    partitionID: i64,
    restoredData: Vec<types::Datum>,
    buf: Option<Vec<u8>>,
) -> Result<Vec<u8>, errors::SharedError> {
    if tblInfo.IsCommonHandle && tblInfo.CommonHandleVersion == 1 {
        return GenIndexValueForClusteredIndexVersion1(
            useNewCollate,
            loc,
            tblInfo,
            idxInfo,
            needRestoredData,
            distinct,
            untouched,
            indexedValues,
            h,
            partitionID,
            restoredData,
            buf,
        );
    }
    genIndexValueVersion0(
        loc,
        tblInfo,
        idxInfo,
        needRestoredData,
        distinct,
        untouched,
        indexedValues,
        h,
        partitionID,
        buf,
    )
}

// TryGetCommonPkColumnRestoredIds get the IDs of primary key columns which need restored data if the table has common handle.
// Caller need to make sure the table has common handle.
/// 尝试获取需要 RestoreData 的公共主键列 ID。
pub fn TryGetCommonPkColumnRestoredIds(
    useNewCollate: bool,
    tbl: Box<model::TableInfo>,
) -> Vec<i64> {
    let mut pkColIDs = Vec::new();
    let mut pkIdx: Option<model::IndexInfo> = None;
    for idx in &tbl.Indices {
        if idx.Primary {
            pkIdx = Some(idx.clone());
            break;
        }
    }
    let Some(pkIdx) = pkIdx else {
        return pkColIDs;
    };
    for idxCol in pkIdx.Columns {
        if types::NeedRestoredDataWithCollate(
            &tbl.Columns[idxCol.Offset as usize].FieldType,
            useNewCollate,
        ) {
            pkColIDs.push(tbl.Columns[idxCol.Offset as usize].ID);
        }
    }
    pkColIDs
}

// GenIndexValueForClusteredIndexVersion1 generates the index value for the clustered index with version 1(New in v5.0.0).
/// 为聚簇索引版本 1 生成索引 value。
pub fn GenIndexValueForClusteredIndexVersion1(
    useNewCollate: bool,
    loc: Option<time::Location>,
    tblInfo: Box<model::TableInfo>,
    idxInfo: Box<model::IndexInfo>,
    idxValNeedRestoredData: bool,
    distinct: bool,
    untouched: bool,
    indexedValues: Vec<types::Datum>,
    h: Box<dyn kv::Handle>,
    partitionID: i64,
    handleRestoredData: Vec<types::Datum>,
    buf: Option<Vec<u8>>,
) -> Result<Vec<u8>, errors::SharedError> {
    let mut idxVal = buf.unwrap_or_default();
    idxVal.clear();
    idxVal.push(0);
    let mut tailLen = 0usize;
    // Version info.
    idxVal.push(IndexVersionFlag);
    idxVal.push(1);

    if distinct {
        idxVal = encodeCommonHandle(idxVal, h.Copy());
    }
    if idxInfo.Global {
        idxVal = encodePartitionID(idxVal, partitionID);
    }
    if idxValNeedRestoredData || !handleRestoredData.is_empty() {
        let mut colIds = Vec::with_capacity(idxInfo.Columns.len());
        let mut allRestoredData =
            Vec::with_capacity(handleRestoredData.len() + idxInfo.Columns.len());
        for (i, idxCol) in idxInfo.Columns.iter().enumerate() {
            let col = tblInfo.Columns[idxCol.Offset as usize].clone();
            // If the column is the primary key's column,
            // the restored data will be written later. Skip writing it here to avoid redundancy.
            if mysql::HasPriKeyFlag(col.GetFlag()) {
                continue;
            }
            let changing_ft = model::GetIdxChangingFieldType(idxCol, &col);
            if types::NeedRestoredDataWithCollate(changing_ft, useNewCollate) {
                colIds.push(col.ID);
                if collate::IsBinCollation(changing_ft.GetCollate()) {
                    allRestoredData.push(types::NewUintDatum(stringutil::GetTailSpaceCount(
                        &indexedValues[i].GetString(),
                    ) as u64));
                } else {
                    allRestoredData.push(indexedValues[i].clone());
                }
            }
        }

        if !handleRestoredData.is_empty() {
            let pkColIDs = TryGetCommonPkColumnRestoredIds(useNewCollate, tblInfo.clone());
            colIds.extend(pkColIDs);
            allRestoredData.extend(handleRestoredData);
        }

        let mut rd = rowcodec::Encoder::new(true);
        // Encode row restored value.
        idxVal = rd
            .Encode(loc.as_ref(), colIds, allRestoredData, None, idxVal)
            .map_err(trace_error)?;
    }

    if untouched {
        tailLen = 1;
        idxVal.push(kv::UnCommitIndexKVFlag);
    }
    idxVal[0] = tailLen as u8;

    Ok(idxVal)
}

// genIndexValueVersion0 create index value for both local and global index.
/// 生成版本 0 的索引 value（旧布局）。
pub fn genIndexValueVersion0(
    loc: Option<time::Location>,
    tblInfo: Box<model::TableInfo>,
    idxInfo: Box<model::IndexInfo>,
    idxValNeedRestoredData: bool,
    distinct: bool,
    untouched: bool,
    indexedValues: Vec<types::Datum>,
    h: Box<dyn kv::Handle>,
    partitionID: i64,
    buf: Option<Vec<u8>>,
) -> Result<Vec<u8>, errors::SharedError> {
    let mut idxVal = buf.unwrap_or_default();
    idxVal.clear();
    idxVal.push(0);
    let mut newEncode = false;
    let mut tailLen = 0usize;
    if !h.IsInt() && distinct {
        idxVal = encodeCommonHandle(idxVal, h.Copy());
        newEncode = true;
    }
    if idxInfo.Global {
        idxVal = encodePartitionID(idxVal, partitionID);
        newEncode = true;
    }
    if idxValNeedRestoredData {
        let mut colIds = Vec::with_capacity(idxInfo.Columns.len());
        for col in &idxInfo.Columns {
            colIds.push(tblInfo.Columns[col.Offset as usize].ID);
        }
        let mut rd = rowcodec::Encoder::new(true);
        // Encode row restored value.
        idxVal = rd
            .Encode(loc.as_ref(), colIds, indexedValues, None, idxVal)
            .map_err(trace_error)?;
        newEncode = true;
    }

    if newEncode {
        if h.IsInt() && distinct {
            // The len of the idxVal is always >= 10 since len (restoredValue) > 0.
            tailLen += 8;
            idxVal.extend_from_slice(&EncodeHandleInUniqueIndexValue(h.Copy(), false));
        } else if idxVal.len() < 10 {
            // Padding the len to 10
            let paddingLen = 10 - idxVal.len();
            tailLen += paddingLen;
            idxVal.resize(10, 0);
        }
        if untouched {
            // If index is untouched and fetch here means the key is exists in TiKV, but not in txn mem-buffer,
            // then should also write the untouched index key/value to mem-buffer to make sure the data
            // is consistent with the index in txn mem-buffer.
            tailLen += 1;
            idxVal.push(kv::UnCommitIndexKVFlag);
        }
        idxVal[0] = tailLen as u8;
    } else {
        // Old index value encoding.
        idxVal.clear();
        if distinct {
            idxVal = EncodeHandleInUniqueIndexValue(h, untouched);
        }
        if untouched {
            // If index is untouched and fetch here means the key is exists in TiKV, but not in txn mem-buffer,
            // then should also write the untouched index key/value to mem-buffer to make sure the data
            // is consistent with the index in txn mem-buffer.
            idxVal.push(kv::UnCommitIndexKVFlag);
        }
        if idxVal.is_empty() {
            idxVal.push(b'0');
        }
    }
    Ok(idxVal)
}

// TruncateIndexValues truncates the index values created using only the leading part of column values.
/// 按前缀长度截断多列索引值。
pub fn TruncateIndexValues(
    tblInfo: Box<model::TableInfo>,
    idxInfo: Box<model::IndexInfo>,
    indexedValues: &mut Vec<types::Datum>,
) {
    for i in 0..indexedValues.len() {
        let idxCol = idxInfo.Columns[i].clone();
        let tblCol = tblInfo.Columns[idxCol.Offset as usize].clone();
        TruncateIndexValue(&mut indexedValues[i], idxCol, tblCol);
    }
}

// TruncateIndexValue truncate one value in the index.
/// 截断单列索引值到指定前缀。
pub fn TruncateIndexValue(
    v: &mut types::Datum,
    idxCol: model::IndexColumn,
    tblCol: model::ColumnInfo,
) {
    let noPrefixIndex = idxCol.Length == types::UnspecifiedLength as isize;
    if noPrefixIndex {
        return;
    }
    let notStringType = v.Kind() != types::KindString && v.Kind() != types::KindBytes;
    if notStringType {
        return;
    }
    let colValue = v.GetBytes();
    if tblCol.GetCharset() == charset::CharsetBin || tblCol.GetCharset() == charset::CharsetASCII {
        // Count character length by bytes if charset is binary or ascii.
        if colValue.len() > idxCol.Length as usize {
            // truncate value and limit its length
            if v.Kind() == types::KindBytes {
                v.SetBytes(colValue[..idxCol.Length as usize].to_vec());
            } else {
                v.SetString(
                    v.GetString()[..idxCol.Length as usize].to_string(),
                    tblCol.GetCollate().to_string(),
                );
            }
        }
    } else if String::from_utf8_lossy(&colValue).chars().count() > idxCol.Length as usize {
        // Count character length by characters for other rune-based charsets, they are all internally encoded as UTF-8.
        let truncateStr = String::from_utf8_lossy(&colValue)
            .chars()
            .take(idxCol.Length as usize)
            .collect();
        // truncate value and limit its length
        v.SetString(truncateStr, tblCol.GetCollate().to_string());
    }
}

// EncodeHandleInUniqueIndexValue encodes handle in data.
/// 将 handle 编码进唯一索引 value（可带 untouched 标记）。
pub fn EncodeHandleInUniqueIndexValue(h: Box<dyn kv::Handle>, isUntouched: bool) -> Vec<u8> {
    if h.IsInt() {
        let mut data = vec![0; 8];
        data.copy_from_slice(&(h.IntValue() as u64).to_be_bytes());
        return data;
    }
    let untouchedFlag = if isUntouched { 1 } else { 0 };
    encodeCommonHandle(vec![untouchedFlag], h)
}

// encodeCommonHandle 追加 common handle 标记、长度和原始 encoded handle。
/// 向索引 value 追加 CommonHandleFlag 与 common handle 字节。
pub fn encodeCommonHandle(mut idxVal: Vec<u8>, h: Box<dyn kv::Handle>) -> Vec<u8> {
    idxVal.push(CommonHandleFlag);
    let hLen = h.Encoded().len() as u16;
    idxVal.push((hLen >> 8) as u8);
    idxVal.push(hLen as u8);
    idxVal.extend_from_slice(&h.Encoded());
    idxVal
}

// encodePartitionID 追加全局索引 partition id 标记和 8 字节编码值。
/// 向索引 value 追加 PartitionIDFlag 与分区 ID。
pub fn encodePartitionID(mut idxVal: Vec<u8>, partitionID: i64) -> Vec<u8> {
    idxVal.push(PartitionIDFlag);
    codec::EncodeInt(idxVal, partitionID)
}

// IndexValueSegments use to store result of SplitIndexValue.
/// 索引 value 按 flag 拆分后的各段原始字节。
pub struct IndexValueSegments {
    pub CommonHandle: Vec<u8>,
    pub PartitionID: Vec<u8>,
    pub RestoredValues: Vec<u8>,
    pub IntHandle: Vec<u8>,
}

impl Default for IndexValueSegments {
    fn default() -> Self {
        Self {
            CommonHandle: Vec::new(),
            PartitionID: Vec::new(),
            RestoredValues: Vec::new(),
            IntHandle: Vec::new(),
        }
    }
}

// SplitIndexValue decodes segments in index value for both non-clustered and clustered table.
/// 按版本将索引 value 拆分为各逻辑段。
pub fn SplitIndexValue(value: Vec<u8>) -> IndexValueSegments {
    if getIndexVersion(&value) == 0 {
        // For Old Encoding (IntHandle without any others options)
        if value.len() <= MaxOldEncodeValueLen {
            let mut segs = IndexValueSegments::default();
            segs.IntHandle = value;
            return segs;
        }
        // For IndexValueVersion0
        return splitIndexValueForIndexValueVersion0(value);
    }
    // For IndexValueForClusteredIndexVersion1
    splitIndexValueForClusteredIndexVersion1(value)
}

// splitIndexValueForIndexValueVersion0 splits index value into segments.
/// 按版本 0 布局拆分索引 value。
pub fn splitIndexValueForIndexValueVersion0(mut value: Vec<u8>) -> IndexValueSegments {
    let mut segs = IndexValueSegments::default();
    let tailLen = value[0] as usize;
    let tail = value[value.len() - tailLen..].to_vec();
    value = value[1..value.len() - tailLen].to_vec();
    if tail.len() >= 8 {
        segs.IntHandle = tail[..8].to_vec();
    }
    if !value.is_empty() && value[0] == CommonHandleFlag {
        let handleLen = ((value[1] as u16) << 8) + value[2] as u16;
        let handleEndOff = 3 + handleLen as usize;
        segs.CommonHandle = value[3..handleEndOff].to_vec();
        value = value[handleEndOff..].to_vec();
    }
    if !value.is_empty() && value[0] == PartitionIDFlag {
        segs.PartitionID = value[1..9].to_vec();
        value = value[9..].to_vec();
    }
    if !value.is_empty() && value[0] == RestoreDataFlag {
        segs.RestoredValues = value;
    }
    segs
}

// splitIndexValueForClusteredIndexVersion1 splits index value into segments.
/// 按聚簇索引版本 1 布局拆分索引 value。
pub fn splitIndexValueForClusteredIndexVersion1(mut value: Vec<u8>) -> IndexValueSegments {
    let mut segs = IndexValueSegments::default();
    let tailLen = value[0] as usize;
    // Skip the tailLen and version info.
    value = value[3..value.len() - tailLen].to_vec();
    if !value.is_empty() && value[0] == CommonHandleFlag {
        let handleLen = ((value[1] as u16) << 8) + value[2] as u16;
        let handleEndOff = 3 + handleLen as usize;
        segs.CommonHandle = value[3..handleEndOff].to_vec();
        value = value[handleEndOff..].to_vec();
    }
    if !value.is_empty() && value[0] == PartitionIDFlag {
        segs.PartitionID = value[1..9].to_vec();
        value = value[9..].to_vec();
    }
    if !value.is_empty() && value[0] == RestoreDataFlag {
        segs.RestoredValues = value;
    }
    segs
}

// decodeIndexKvForClusteredIndexVersion1 解码 common-handle-v1 index value，包括 restored values、handle 和 partition id。
/// 聚簇索引版本 1 的索引 KV 解码实现。
pub fn decodeIndexKvForClusteredIndexVersion1(
    useNewCollate: bool,
    key: Vec<u8>,
    value: Vec<u8>,
    colsLen: usize,
    hdStatus: HandleStatus,
    columns: Vec<rowcodec::ColInfo>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    let segs = splitIndexValueForClusteredIndexVersion1(value);
    let (mut resultValues, keySuffix) = CutIndexKeyNew(kv::Key(key), colsLen)?;
    if !segs.RestoredValues.is_empty() {
        resultValues = decodeRestoredValuesV5(
            useNewCollate,
            clone_col_infos(&columns[..colsLen]),
            resultValues,
            segs.RestoredValues.clone(),
        )?;
    }
    if hdStatus == HandleNotNeeded {
        return Ok(resultValues);
    }
    let handle = if !segs.CommonHandle.is_empty() {
        // In unique common handle index.
        kv::NewCommonHandle(segs.CommonHandle).map_err(trace_error)?
    } else {
        // In non-unique index, decode handle in keySuffix.
        kv::NewCommonHandle(keySuffix).map_err(trace_error)?
    };
    let handleBytes = reEncodeHandleConsiderNewCollation(
        useNewCollate,
        Box::new(handle),
        clone_col_infos(&columns[colsLen..]),
        segs.RestoredValues,
    )?;
    resultValues.extend(handleBytes);
    if !segs.PartitionID.is_empty() {
        let (_, pid) = codec::DecodeInt(&segs.PartitionID).map_err(trace_error)?;
        let datum = types::NewIntDatum(pid);
        let pidBytes =
            codec::EncodeValue(time::UTC, Vec::new(), vec![datum]).map_err(trace_error)?;
        resultValues.push(pidBytes);
    }
    Ok(resultValues)
}

// decodeIndexKvGeneral decodes index key value pair of new layout in an extensible way.
/// 通用索引 KV 解码（非聚簇索引版本 1 路径）。
pub fn decodeIndexKvGeneral(
    key: Vec<u8>,
    value: Vec<u8>,
    colsLen: usize,
    hdStatus: HandleStatus,
    columns: Vec<rowcodec::ColInfo>,
) -> Result<Vec<Vec<u8>>, errors::SharedError> {
    let segs = splitIndexValueForIndexValueVersion0(value);
    let (mut resultValues, keySuffix) = CutIndexKeyNew(kv::Key(key), colsLen)?;
    if !segs.RestoredValues.is_empty() {
        // new collation
        resultValues = decodeRestoredValues(
            clone_col_infos(&columns[..colsLen]),
            segs.RestoredValues.clone(),
        )?;
    }
    if hdStatus == HandleNotNeeded {
        return Ok(resultValues);
    }

    let handle = if !segs.IntHandle.is_empty() {
        // In unique int handle index.
        DecodeIntHandleInIndexValue(segs.IntHandle)
    } else if !segs.CommonHandle.is_empty() {
        // In unique common handle index.
        decodeHandleInIndexKey(segs.CommonHandle)?
    } else {
        // In non-unique index, decode handle in keySuffix
        decodeHandleInIndexKey(keySuffix)?
    };
    let handleBytes = reEncodeHandle(handle, hdStatus == HandleIsUnsigned)?;
    resultValues.extend(handleBytes);
    if !segs.PartitionID.is_empty() {
        let (_, pid) = codec::DecodeInt(&segs.PartitionID).map_err(trace_error)?;
        let datum = types::NewIntDatum(pid);
        let pidBytes =
            codec::EncodeValue(time::UTC, Vec::new(), vec![datum]).map_err(trace_error)?;
        resultValues.push(pidBytes);
    }
    Ok(resultValues)
}

// IndexKVIsUnique uses to judge if an index is unique, it can handle the KV committed by txn already, it doesn't consider the untouched flag.
/// 根据 value 布局判断该索引 KV 是否对应唯一索引。
pub fn IndexKVIsUnique(value: Vec<u8>) -> bool {
    if value.len() <= MaxOldEncodeValueLen {
        return value.len() == 8;
    }
    if getIndexVersion(&value) == 1 {
        let segs = splitIndexValueForClusteredIndexVersion1(value);
        return !segs.CommonHandle.is_empty();
    }
    let segs = splitIndexValueForIndexValueVersion0(value);
    !segs.IntHandle.is_empty() || !segs.CommonHandle.is_empty()
}

// VerifyTableIDForRanges verifies that all given ranges are valid to decode the table id.
/// 校验一组 KeyRange 是否都属于同一 tableID，并返回该 ID 列表。
pub fn VerifyTableIDForRanges(
    keyRanges: Box<kv::KeyRanges>,
) -> Result<Vec<i64>, errors::SharedError> {
    let mut tids = Vec::with_capacity(keyRanges.PartitionNum());
    let collectFunc =
        |ranges: &[kv::KeyRange], _idx: &[i32]| -> Result<(), errors::SharedError> {
            if ranges.is_empty() {
                return Ok(());
            }
            let tid = DecodeTableID(ranges[0].StartKey.clone());
            if tid <= 0 {
                return Err(errors::New("Incorrect keyRange is constrcuted"));
            }
            tids.push(tid);
            for i in 1..ranges.len() {
                let tmpTID = DecodeTableID(ranges[i].StartKey.clone());
                if tmpTID <= 0 {
                    return Err(errors::New("Incorrect keyRange is constrcuted"));
                }
                if tid != tmpTID {
                    return Err(errors::New(
                        "Using multi partition's ranges as single table's",
                    ));
                }
            }
            Ok(())
        };
    keyRanges.ForEachPartitionWithErr(collectFunc)?;
    Ok(tids)
}
