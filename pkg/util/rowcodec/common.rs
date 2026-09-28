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

// rowcodec 公共常量、行格式辅助、排序器、checksum 输入结构与 keyspace 前缀处理。
//
// 对应 Go `pkg/util/rowcodec/common.go`：定义新行格式版本号、Datum 编码 flag、
// 最短定长整数编解码、按列 ID 排序的 NULL/非 NULL sorter，以及行 checksum
// 与 API V2 keyspace 前缀剥离逻辑。

// 这段逻辑承载 rowcodec 包的公共常量、行格式辅助函数、排序器、checksum 输入结构和 keyspace 前缀处理逻辑。
// 这些 TiDB Go 依赖尚未在 实现中接线，下面保留原始模块和方法调用形状作为占位。

// CodecVer is the constant number that represent the new row format.
// CodecVer 对应 Go 新 row 格式的首字节版本号，row.go/decoder.go 会用它识别新格式。
/// 新行格式首字节版本号（128 / 0x80）。
pub const CodecVer: u8 = 128;

// Go 里的 var 错误值是包级单例；用函数占位，避免在尚未处理的错误类型前发明静态初始化策略。
/// 无效 codec 版本错误。
fn errInvalidCodecVer() -> errors::SharedError {
    errors::New("invalid codec version")
}

/// 无效 checksum 版本错误。
fn errInvalidChecksumVer() -> errors::SharedError {
    errors::New("invalid checksum version")
}

/// 无效 checksum 类型错误。
fn errInvalidChecksumTyp() -> errors::SharedError {
    errors::New("invalid type for checksum")
}

// First byte in the encoded value which specifies the encoding type.
// 以下 flag 保持 Go 中 value 编码首字节取值；编码器和解码器据此选择具体 Datum 解析路径。
/// NULL 编码 flag。
pub const NilFlag: u8 = 0;
/// 普通字节串 flag。
pub const BytesFlag: u8 = 1;
/// 紧凑字节串 flag。
pub const CompactBytesFlag: u8 = 2;
/// 有符号整数 flag。
pub const IntFlag: u8 = 3;
/// 无符号整数 flag。
pub const UintFlag: u8 = 4;
/// 浮点 flag。
pub const FloatFlag: u8 = 5;
/// Decimal flag。
pub const DecimalFlag: u8 = 6;
/// 变长有符号整数 flag。
pub const VarintFlag: u8 = 8;
/// 变长无符号整数 flag。
pub const VaruintFlag: u8 = 9;
/// JSON flag。
pub const JSONFlag: u8 = 10;
/// VectorFloat32 flag。
pub const VectorFloat32Flag: u8 = 20;

// keyspacePrefixLen 和 apiV2TxnModePrefix 只用于 RemoveKeyspacePrefix 的本地判断。
const keyspacePrefixLen: usize = 4;
const apiV2TxnModePrefix: u8 = b'x';

// bytesToU32Slice 对应 Go 的 unsafe.Slice((*uint32)(unsafe.Pointer(&b[0])), len(b)/4)。
// Go 版本返回与原始 []byte 共享底层内存的 []uint32；复制成 Vec<u32>，后续若追求零拷贝需重新审查对齐和生命周期。
fn bytesToU32Slice(b: &[u8]) -> Vec<u32> {
    if b.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(b.len() / 4);
    for chunk in b.chunks_exact(4) {
        out.push(u32::from_le_bytes(chunk.try_into().unwrap()));
    }
    out
}

// bytes2U16Slice 对应 Go 的 unsafe 字节视图转换；这里同样用复制占位，保持小端平台上的可读语义。
fn bytes2U16Slice(b: &[u8]) -> Vec<u16> {
    if b.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for chunk in b.chunks_exact(2) {
        out.push(u16::from_le_bytes(chunk.try_into().unwrap()));
    }
    out
}

// u16SliceToBytes 是 Go unsafe 反向转换：[]uint16 共享为 []byte。
// Rust 按小端序展开为新 Vec<u8>，标明这里尚未保留 Go 的底层别名关系。
fn u16SliceToBytes(u16s: &[u16]) -> Vec<u8> {
    if u16s.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(u16s.len() * 2);
    for v in u16s {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

// u32SliceToBytes 与 Go 版本一样按每个 uint32 展开为 4 字节；当前实现复制数据，不返回别名视图。
fn u32SliceToBytes(u32s: &[u32]) -> Vec<u8> {
    if u32s.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(u32s.len() * 4);
    for v in u32s {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

// encodeInt 按 Go 的最短定长整数策略追加有符号整数：1、2、4 或 8 字节，小端序。
/// 按最短定长（1/2/4/8）小端追加有符号整数。
pub fn encodeInt(mut buf: Vec<u8>, iVal: i64) -> Vec<u8> {
    if iVal as i8 as i64 == iVal {
        buf.push(iVal as u8);
    } else if iVal as i16 as i64 == iVal {
        buf.extend_from_slice(&(iVal as i16).to_le_bytes());
    } else if iVal as i32 as i64 == iVal {
        buf.extend_from_slice(&(iVal as i32).to_le_bytes());
    } else {
        buf.extend_from_slice(&iVal.to_le_bytes());
    }
    buf
}

// decodeInt 按输入长度还原 Go encodeInt 的有符号整数；default 分支保持 Go 对非 1/2/4 长度按 uint64 读取的结构。
/// 按缓冲长度还原有符号整数。
pub fn decodeInt(val: &[u8]) -> i64 {
    match val.len() {
        1 => val[0] as i8 as i64,
        2 => i16::from_le_bytes(val[0..2].try_into().unwrap()) as i64,
        4 => i32::from_le_bytes(val[0..4].try_into().unwrap()) as i64,
        _ => u64::from_le_bytes(val[0..8].try_into().unwrap()) as i64,
    }
}

// encodeUint 按 Go 的最短定长整数策略追加无符号整数：1、2、4 或 8 字节，小端序。
/// 按最短定长（1/2/4/8）小端追加无符号整数。
pub fn encodeUint(mut buf: Vec<u8>, uVal: u64) -> Vec<u8> {
    if uVal as u8 as u64 == uVal {
        buf.push(uVal as u8);
    } else if uVal as u16 as u64 == uVal {
        buf.extend_from_slice(&(uVal as u16).to_le_bytes());
    } else if uVal as u32 as u64 == uVal {
        buf.extend_from_slice(&(uVal as u32).to_le_bytes());
    } else {
        buf.extend_from_slice(&uVal.to_le_bytes());
    }
    buf
}

// decodeUint 按输入长度还原 Go encodeUint 的无符号整数；其它长度沿用 Go default 分支读取 8 字节。
/// 按缓冲长度还原无符号整数。
pub fn decodeUint(val: &[u8]) -> u64 {
    match val.len() {
        1 => u64::from(val[0]),
        2 => u16::from_le_bytes(val[0..2].try_into().unwrap()) as u64,
        4 => u32::from_le_bytes(val[0..4].try_into().unwrap()) as u64,
        _ => u64::from_le_bytes(val[0..8].try_into().unwrap()),
    }
}

// largeNotNullSorter 对应 Go 的 `type largeNotNullSorter Encoder`，用于 sort.Interface 排序 large 行的非空列。
// Go 通过类型转换直接访问 Encoder 字段；显式保存可变引用，强调排序时会同步交换 colIDs32 与 values。
struct largeNotNullSorter<'a> {
    encoder: &'a mut Encoder,
}

impl<'a> largeNotNullSorter<'a> {
    fn from_encoder(encoder: &'a mut Encoder) -> Self {
        Self { encoder }
    }

    // Less implements sort.Interface for large non-null column IDs.
    // Less 保持 Go 的比较语义：只比较非空列区间内的 u32 列 ID。
    fn Less(&self, i: usize, j: usize) -> bool {
        self.encoder.row.colIDs32[i] < self.encoder.row.colIDs32[j]
    }

    // Len implements sort.Interface for large non-null column IDs.
    fn Len(&self) -> usize {
        self.encoder.row.numNotNullCols as usize
    }

    // Swap implements sort.Interface for large non-null column IDs.
    // Swap 同时交换 values，保持列 ID 和 Datum 指针配对关系。
    fn Swap(&mut self, i: usize, j: usize) {
        self.encoder.row.colIDs32.swap(i, j);
        self.encoder.values.swap(i, j);
    }
}

// smallNotNullSorter 对应 Go 的 `type smallNotNullSorter Encoder`，用于小行非空列排序。
struct smallNotNullSorter<'a> {
    encoder: &'a mut Encoder,
}

impl<'a> smallNotNullSorter<'a> {
    fn from_encoder(encoder: &'a mut Encoder) -> Self {
        Self { encoder }
    }

    // Less implements sort.Interface for small non-null column IDs.
    fn Less(&self, i: usize, j: usize) -> bool {
        self.encoder.row.colIDs[i] < self.encoder.row.colIDs[j]
    }

    // Len implements sort.Interface for small non-null column IDs.
    fn Len(&self) -> usize {
        self.encoder.row.numNotNullCols as usize
    }

    // Swap implements sort.Interface for small non-null column IDs.
    // Go 这里同样会交换 values，避免排序后 Datum 与列 ID 错位。
    fn Swap(&mut self, i: usize, j: usize) {
        self.encoder.row.colIDs.swap(i, j);
        self.encoder.values.swap(i, j);
    }
}

// smallNullSorter 对应 Go 的小行 NULL 列排序器；它只重排 NULL 列 ID，不触碰 values。
struct smallNullSorter<'a> {
    encoder: &'a mut Encoder,
}

impl<'a> smallNullSorter<'a> {
    fn from_encoder(encoder: &'a mut Encoder) -> Self {
        Self { encoder }
    }

    // Less implements sort.Interface for small null column IDs.
    fn Less(&self, i: usize, j: usize) -> bool {
        let start = self.encoder.row.numNotNullCols as usize;
        self.encoder.row.colIDs[start + i] < self.encoder.row.colIDs[start + j]
    }

    // Len implements sort.Interface for small null column IDs.
    fn Len(&self) -> usize {
        self.encoder.row.numNullCols as usize
    }

    // Swap implements sort.Interface for small null column IDs.
    // NULL 列没有对应 data/values，因此只交换 colIDs 后半段。
    fn Swap(&mut self, i: usize, j: usize) {
        let start = self.encoder.row.numNotNullCols as usize;
        self.encoder.row.colIDs.swap(start + i, start + j);
    }
}

// largeNullSorter 对应 Go 的 large NULL 列排序器；逻辑与 smallNullSorter 相同，只是列 ID 宽度为 u32。
struct largeNullSorter<'a> {
    encoder: &'a mut Encoder,
}

impl<'a> largeNullSorter<'a> {
    fn from_encoder(encoder: &'a mut Encoder) -> Self {
        Self { encoder }
    }

    // Less implements sort.Interface for large null column IDs.
    fn Less(&self, i: usize, j: usize) -> bool {
        let start = self.encoder.row.numNotNullCols as usize;
        self.encoder.row.colIDs32[start + i] < self.encoder.row.colIDs32[start + j]
    }

    // Len implements sort.Interface for large null column IDs.
    fn Len(&self) -> usize {
        self.encoder.row.numNullCols as usize
    }

    // Swap implements sort.Interface for large null column IDs.
    fn Swap(&mut self, i: usize, j: usize) {
        let start = self.encoder.row.numNotNullCols as usize;
        self.encoder.row.colIDs32.swap(start + i, start + j);
    }
}

// Length of rowkey.
/// TiDB row key 固定长度（19 字节）。
pub const rowKeyLen: usize = 19;
// Index of record flag 'r' in rowkey used by tidb-server.
// The rowkey format is t{8 bytes id}_r{8 bytes handle}
/// row key 中 record 标记 'r' 的下标。
pub const recordPrefixIdx: usize = 10;

// IsRowKey determine whether key is row key.
// this method will be used in unistore.
// IsRowKey 按 TiDB row key 固定格式检查 key：长度至少 19，首字节为 't'，record 标记位置为 'r'。
/// 判断 key 是否为 TiDB 行 key（`t..._r...`）。
pub fn IsRowKey(key: &[u8]) -> bool {
    key.len() >= rowKeyLen && key[0] == b't' && key[recordPrefixIdx] == b'r'
}

// IsNewFormat checks whether row data is in new-format.
// IsNewFormat 只读取 rowData[0] 与 CodecVer 比较；Go 版本假设调用方传入非空 slice。
/// 判断行数据是否为新格式（首字节 == CodecVer）。
pub fn IsNewFormat(rowData: &[u8]) -> bool {
    rowData.first().is_some_and(|version| *version == CodecVer)
}

// FieldTypeFromModelColumn creates a types.FieldType from model.ColumnInfo.
// export for test case and CDC.
// FieldTypeFromModelColumn 对应 Go 的导出辅助函数：从 model.ColumnInfo 克隆 FieldType，供测试和 CDC 使用。
/// 从 ColumnInfo 克隆 FieldType，供测试与 CDC 使用。
pub fn FieldTypeFromModelColumn(col: &model::ColumnInfo) -> types::FieldType {
    col.FieldType.Clone()
}

// ColData combines the column info as well as its datum. It's used to calculate checksum.
// ColData 保留 Go 中嵌入 *model.ColumnInfo 加 Datum 指针的形状，用于 checksum 前按列类型编码 Datum。
/// 列元信息与 Datum 的组合，用于计算行 checksum。
pub struct ColData<'a> {
    pub ColumnInfo: &'a model::ColumnInfo,
    pub Datum: &'a types::Datum,
}

impl ColData<'_> {
    // Encode encodes the column datum into bytes for checksum. If buf provided, append encoded data to it.
    // Encode 调用 appendDatumForChecksum，类型来自 ColumnInfo.GetType，输出追加到传入缓冲。
    /// 将列 Datum 编码为 checksum 用字节。
    pub fn Encode(
        &self,
        loc: Option<&time::Location>,
        buf: Vec<u8>,
    ) -> Result<Vec<u8>, errors::SharedError> {
        appendDatumForChecksum(loc, buf, self.Datum, self.ColumnInfo.GetType())
    }

    // ID 对应 Go 匿名嵌入字段带来的 r.Cols[i].ID 访问；这里提供占位方法保持排序语义可读。
    fn ID(&self) -> i64 {
        self.ColumnInfo.ID
    }
}

// RowData is a list of ColData for row checksum calculation.
// RowData 表示参与行 checksum 的列集合；调用 Encode/Checksum 前 Go 要求 Cols 已按 id 排序。
/// 参与行 checksum 的列集合；调用前 Cols 应按 id 排序。
pub struct RowData<'a> {
    // Cols is a list of ColData which is expected to be sorted by id before calling Encode/Checksum.
    /// 按列 ID 排序的 ColData 列表。
    pub Cols: Vec<ColData<'a>>,
    // Data stores the result of Encode. However, it mostly acts as a buffer for encoding columns on checksum
    // calculation.
    /// 编码缓冲 / Encode 结果。
    pub Data: Vec<u8>,
}

impl RowData<'_> {
    // Len implements sort.Interface for RowData.
    pub fn Len(&self) -> usize {
        self.Cols.len()
    }

    // Less implements sort.Interface for RowData.
    pub fn Less(&self, i: usize, j: usize) -> bool {
        self.Cols[i].ID() < self.Cols[j].ID()
    }

    // Swap implements sort.Interface for RowData.
    pub fn Swap(&mut self, i: usize, j: usize) {
        self.Cols.swap(i, j);
    }

    // Encode encodes all columns into bytes (for test purpose).
    // Encode 复用 RowData.Data 作为缓冲区，逐列调用 ColData::Encode；任何列出错都立即返回。
    /// 将所有列编码到 Data（测试用）。
    pub fn Encode(&mut self, loc: Option<&time::Location>) -> Result<Vec<u8>, errors::SharedError> {
        if !self.Data.is_empty() {
            self.Data.truncate(0);
        }
        for col in &self.Cols {
            // Go 会把同一个 slice 继续传给下一列； clone/赋值只是保留缓冲复用意图。
            self.Data = col.Encode(loc, self.Data.clone())?;
        }
        Ok(self.Data.clone())
    }

    // Checksum calculates the checksum of columns. Callers should make sure columns are sorted by id.
    // Checksum 对每列单独编码后用 crc32.Update 累加；Data 每轮清空以避免列之间混入旧内容。
    /// 按列分别编码后累加 CRC32 checksum。
    pub fn Checksum(&mut self, loc: Option<&time::Location>) -> Result<u32, errors::SharedError> {
        let mut checksum: u32 = 0;
        for col in &self.Cols {
            if !self.Data.is_empty() {
                self.Data.truncate(0);
            }
            self.Data = col.Encode(loc, self.Data.clone())?;
            // Go 使用 crc32.Update(checksum, crc32.IEEETable, r.Data)；这里保留调用形状，具体 crate 待后续接线。
            checksum = crc32_update(checksum, &self.Data);
        }
        Ok(checksum)
    }
}

// appendDatumForChecksum 按 MySQL 类型把 Datum 追加编码到 checksum 缓冲。
// Go 函数带 defer/recover，用于捕获 Datum 与类型不匹配时的 panic；无法直接等价表达 recover，
// 因此在函数头标明需要后续用 panic::catch_unwind 或类型化错误接线。
fn appendDatumForChecksum(
    loc: Option<&time::Location>,
    mut buf: Vec<u8>,
    dat: &types::Datum,
    typ: u8,
) -> Result<Vec<u8>, errors::SharedError> {
    // Go: defer recover 后用 errors.Annotatef 包装为 "encode datum(%s) as %s for checksum"。
    // Rust 直接执行分支；panic 捕获、Datum String 和 TypeStr 格式化留给后续真实类型接线。
    if dat.IsNull() {
        return Ok(buf);
    }

    let kind = dat.Kind();
    let valid_kind = match typ {
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeInt24
        | mysql::TypeYear
        | mysql::TypeEnum
        | mysql::TypeSet => matches!(
            kind,
            types::KindInt64 | types::KindUint64 | types::KindMysqlEnum | types::KindMysqlSet
        ),
        mysql::TypeVarchar
        | mysql::TypeVarString
        | mysql::TypeString
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob
        | mysql::TypeBlob => matches!(kind, types::KindString | types::KindBytes),
        mysql::TypeTimestamp | mysql::TypeDatetime | mysql::TypeDate | mysql::TypeNewDate => {
            kind == types::KindMysqlTime
        }
        mysql::TypeDuration => kind == types::KindMysqlDuration,
        mysql::TypeFloat => matches!(kind, types::KindFloat32 | types::KindFloat64),
        mysql::TypeDouble => kind == types::KindFloat64,
        mysql::TypeNewDecimal => kind == types::KindMysqlDecimal,
        mysql::TypeBit => matches!(kind, types::KindBinaryLiteral | types::KindMysqlBit),
        mysql::TypeJSON => kind == types::KindMysqlJSON,
        mysql::TypeTiDBVectorFloat32 => kind == types::KindVectorFloat32,
        mysql::TypeNull | mysql::TypeGeometry => true,
        _ => false,
    };
    if !valid_kind {
        return Err(errors::New(format!(
            "encode datum kind {} as type {} for checksum",
            kind, typ
        )));
    }

    let out = match typ {
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeInt24
        | mysql::TypeYear => {
            buf.extend_from_slice(&dat.GetUint64().to_le_bytes());
            buf
        }
        mysql::TypeVarchar
        | mysql::TypeVarString
        | mysql::TypeString
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob
        | mysql::TypeBlob => appendLengthValue(buf, &dat.GetBytes()),
        mysql::TypeTimestamp | mysql::TypeDatetime | mysql::TypeDate | mysql::TypeNewDate => {
            let mut t = dat.GetMysqlTime();
            if t.Type() == mysql::TypeTimestamp
                && loc.is_some_and(|location| *location != time::UTC)
            {
                // Go 仅在 timestamp 且 loc 非 nil、非 UTC 时做时区转换；失败时直接返回该错误。
                t.ConvertTimeZone(*loc.expect("checked above"), time::UTC)
                    .map_err(|error| errors::New(error.to_string()))?;
            }
            appendLengthValue(buf, t.String().as_bytes())
        }
        mysql::TypeDuration => appendLengthValue(buf, dat.GetMysqlDuration().String().as_bytes()),
        mysql::TypeFloat | mysql::TypeDouble => {
            let mut v = if typ == mysql::TypeFloat && kind == types::KindFloat32 {
                f64::from(dat.GetFloat32())
            } else {
                dat.GetFloat64()
            };
            if v.is_infinite() || v.is_nan() {
                // because ticdc has such a transform
                // TiCDC 会把 NaN/Inf 归零；checksum 编码必须复刻这个兼容行为。
                v = 0.0;
            }
            buf.extend_from_slice(&v.to_bits().to_le_bytes());
            buf
        }
        mysql::TypeNewDecimal => appendLengthValue(buf, dat.GetMysqlDecimal().String().as_bytes()),
        mysql::TypeEnum => {
            buf.extend_from_slice(&dat.GetMysqlEnum().Value.to_le_bytes());
            buf
        }
        mysql::TypeSet => {
            buf.extend_from_slice(&dat.GetMysqlSet().Value.to_le_bytes());
            buf
        }
        mysql::TypeBit => {
            // ticdc transforms a bit value as the following way, no need to handle truncate error here.
            // Go 故意忽略 ToInt 的错误，但仍使用该调用随错误返回的饱和值 MaxUint64。
            let literal = dat.GetBinaryLiteral();
            let first_non_zero = literal.0.iter().position(|byte| *byte != 0);
            let significant = first_non_zero.map_or(&[][..], |index| &literal.0[index..]);
            let v = if significant.len() > 8 {
                u64::MAX
            } else {
                significant
                    .iter()
                    .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte))
            };
            buf.extend_from_slice(&v.to_le_bytes());
            buf
        }
        mysql::TypeJSON => appendLengthValue(buf, dat.GetMysqlJSON().String().as_bytes()),
        mysql::TypeTiDBVectorFloat32 => dat.GetVectorFloat32().SerializeTo(buf),
        mysql::TypeNull | mysql::TypeGeometry => buf,
        _ => {
            // 未识别类型沿用 Go 的 errInvalidChecksumTyp，提示 checksum 不支持该 MySQL 类型。
            return Err(errInvalidChecksumTyp());
        }
    };
    Ok(out)
}

// appendLengthValue 先追加 4 字节小端长度，再追加原始值 bytes；用于字符串、时间、decimal、JSON 等变长类型。
fn appendLengthValue(mut buf: Vec<u8>, val: &[u8]) -> Vec<u8> {
    buf.extend_from_slice(&(val.len() as u32).to_le_bytes());
    buf.extend_from_slice(val);
    buf
}

// RemoveKeyspacePrefix is used to remove keyspace prefix from the key if it's
// nextgen kernel.
// RemoveKeyspacePrefix 在 nextgen kernel 的 UT/standalone 场景下剥掉 API V2 txn mode keyspace 前缀。
/// 在 nextgen / UT / standalone 场景下剥掉 API V2 keyspace 前缀。
pub fn RemoveKeyspacePrefix(key: &[u8]) -> &[u8] {
    if kerneltype::IsClassic() {
        return key;
    }
    // If it is not in UT and not run in standalone TiDB, the removing of the
    // keyspace prefix from the keys is performed in client-go.
    // 非测试且非 standalone TiDB 时，client-go 已经负责去前缀；这里必须保持原 key 不变。
    if !intest::InTest.load(std::sync::atomic::Ordering::SeqCst)
        && !kv::StandAloneTiDB.load(std::sync::atomic::Ordering::Relaxed)
    {
        return key;
    }

    if key.len() <= keyspacePrefixLen {
        return key;
    }

    if key[0] != apiV2TxnModePrefix {
        return key;
    }
    &key[keyspacePrefixLen..]
}

/// 以 initial 为初值更新 CRC32（IEEE）。
pub fn crc32_update(initial: u32, bytes: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new_with_initial(initial);
    hasher.update(bytes);
    hasher.finalize()
}
