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

// TTL 物理表元信息、过期时间计算与扫描范围拆分。
//
// 将 infoschema 中的表/分区解析为可扫描的 `PhysicalTable`，按主键类型
// 把 TiKV Region 边界转成 Datum 扫描区间，并按 TTL interval 计算过期时间点。
// 扫描范围统一使用半开区间 `[start, end)`，与 Go 的 TTL 扫描约定保持一致。

// TTL 物理表元信息、过期时间计算和扫描范围拆分逻辑。

// getTableKeyColumns 对应 Go 中根据表句柄形态选择 TTL 扫描键列的逻辑。
// 返回的列与 FieldType 顺序必须和 Go 保持一致，后续 SplitScanRanges 依赖第一个键列类型。
/* Mechanical draft retained for migration history.
pub fn getTableKeyColumns(
    tbl: &model::TableInfo,
) -> Result<(Vec<model::ColumnInfo>, Vec<types::FieldType>), errors::Error> {
    if tbl.PKIsHandle {
        for (i, col) in tbl.Columns.iter().enumerate() {
            if mysql::HasPriKeyFlag(col.GetFlag()) {
                return Ok((
                    vec![tbl.Columns[i].clone()],
                    vec![tbl.Columns[i].FieldType.clone()],
                ));
            }
        }
        return Err(errors::Errorf(format!(
            "Cannot find primary key for table: {}",
            tbl.Name
        )));
    }

    if tbl.IsCommonHandle {
        let idxInfo = tables::FindPrimaryIndex(tbl);
        let mut columns = Vec::with_capacity(idxInfo.Columns.len());
        let mut fieldTypes = Vec::with_capacity(idxInfo.Columns.len());
        for idxCol in &idxInfo.Columns {
            // Go 通过 idxCol.Offset 回到 TableInfo.Columns，保留这层索引语义。
            columns.push(tbl.Columns[idxCol.Offset].clone());
            fieldTypes.push(tbl.Columns[idxCol.Offset].FieldType.clone());
        }
        return Ok((columns, fieldTypes));
    }

    let extraHandleColInfo = model::NewExtraHandleColInfo();
    Ok((
        vec![extraHandleColInfo.clone()],
        vec![extraHandleColInfo.FieldType.clone()],
    ))
}

// ScanRange is the range to scan. The range is: [Start, End)
// ScanRange 对应 Go 的同名结构，Start/End 为空表示无边界。
#[derive(Clone, Debug, Default)]
pub struct ScanRange {
    pub Start: Vec<types::Datum>,
    pub End: Vec<types::Datum>,
}

// newFullRange 对应 Go 的空 ScanRange，表示扫描整张物理表。
pub fn newFullRange() -> ScanRange {
    ScanRange::default()
}

// newDatumRange 对应 Go 的构造函数，只有非 NULL Datum 才写入范围边界。
pub fn newDatumRange(start: types::Datum, end: types::Datum) -> ScanRange {
    let mut r = ScanRange::default();
    if !start.IsNull() {
        r.Start = vec![start];
    }
    if !end.IsNull() {
        r.End = vec![end];
    }
    r
}

// nullDatum 对应 Go 中创建 NULL Datum 的小工具。
pub fn nullDatum() -> types::Datum {
    let mut d = types::Datum::default();
    d.SetNull();
    d
}

// PhysicalTable is used to provide some information for a physical table in TTL job
// PhysicalTable 对应 TTL 任务中的物理表描述，分区表会把 ID 指向具体 partition。
pub struct PhysicalTable {
    // ID is the physical ID of the table
    pub ID: i64,
    // Schema is the database name of the table
    pub Schema: ast::CIStr,
    // Go 嵌入 *model.TableInfo；这里显式命名，便于阅读字段来源。
    pub TableInfo: model::TableInfo,
    // Partition is the partition name
    pub Partition: ast::CIStr,
    // PartitionDef is the partition definition
    pub PartitionDef: Option<model::PartitionDefinition>,
    // KeyColumns is the cluster index key columns for the table
    pub KeyColumns: Vec<model::ColumnInfo>,
    // KeyColumnTypes is the types of the key columns
    pub KeyColumnTypes: Vec<types::FieldType>,
    // TimeColum is the time column used for TTL
    pub TimeColumn: model::ColumnInfo,
}

// NewBasePhysicalTable create a new PhysicalTable with specific timeColumn.
// Go 返回 *PhysicalTable；返回所有权对象，错误路径保持原来的校验顺序。
pub fn NewBasePhysicalTable(
    schema: ast::CIStr,
    tbl: &model::TableInfo,
    partition: ast::CIStr,
    timeColumn: &model::ColumnInfo,
) -> Result<PhysicalTable, errors::Error> {
    if tbl.State != model::StatePublic {
        return Err(errors::Errorf(format!(
            "table '{}.{}' is not a public table",
            schema, tbl.Name
        )));
    }

    let (keyColumns, keyColumnTypes) = getTableKeyColumns(tbl)?;

    let mut physicalID: i64;
    let mut partitionDef: Option<model::PartitionDefinition> = None;
    if tbl.Partition.is_none() {
        if !partition.L.is_empty() {
            return Err(errors::Errorf(format!(
                "table '{}.{}' is not a partitioned table",
                schema, tbl.Name
            )));
        }
        physicalID = tbl.ID;
    } else {
        if partition.L.is_empty() {
            return Err(errors::Errorf(format!(
                "partition name is required, table '{}.{}' is a partitioned table",
                schema, tbl.Name
            )));
        }

        for def in &tbl.Partition.as_ref().unwrap().Definitions {
            if def.Name.L == partition.L {
                partitionDef = Some(def.clone());
            }
        }

        if partitionDef.is_none() {
            return Err(errors::Errorf(format!(
                "partition '{}' is not found in ttl table '{}.{}'",
                partition.O, schema, tbl.Name
            )));
        }

        physicalID = partitionDef.as_ref().unwrap().ID;
    }

    Ok(PhysicalTable {
        ID: physicalID,
        Schema: schema,
        TableInfo: tbl.clone(),
        Partition: partition,
        PartitionDef: partitionDef,
        KeyColumns: keyColumns,
        KeyColumnTypes: keyColumnTypes,
        TimeColumn: timeColumn.clone(),
    })
}

// NewPhysicalTable create a new PhysicalTable
// 它先读取 TTLInfo，再查找公开的 TTL 时间列，最后复用 NewBasePhysicalTable。
pub fn NewPhysicalTable(
    schema: ast::CIStr,
    tbl: &model::TableInfo,
    partition: ast::CIStr,
) -> Result<PhysicalTable, errors::Error> {
    let ttlInfo = match &tbl.TTLInfo {
        Some(info) => info,
        None => {
            return Err(errors::Errorf(format!(
                "table '{}.{}' is not a ttl table",
                schema, tbl.Name
            )));
        }
    };

    let timeColumn = match tbl.FindPublicColumnByName(ttlInfo.ColumnName.L.clone()) {
        Some(col) => col,
        None => {
            return Err(errors::Errorf(format!(
                "time column '{}' is not public in ttl table '{}.{}'",
                ttlInfo.ColumnName, schema, tbl.Name
            )));
        }
    };

    NewBasePhysicalTable(schema, tbl, partition, &timeColumn)
}

impl PhysicalTable {
    // ValidateKeyPrefix validates a key prefix
    // Go 只校验前缀长度不超过 key columns，真正类型匹配留给调用方。
    pub fn ValidateKeyPrefix(&self, key: &[types::Datum]) -> Result<(), errors::Error> {
        if key.len() > self.KeyColumns.len() {
            return Err(errors::Errorf(format!(
                "invalid key length: {}, expected {}",
                key.len(),
                self.KeyColumns.len()
            )));
        }
        Ok(())
    }
}

// mockExpireTimeKey 是 Go context.WithValue 的私有 key；只用于测试覆盖过期时间。
pub struct mockExpireTimeKey {}

// SetMockExpireTime can only used in test
// Rust 保留 context 写入形状，不实现真实类型擦除存储。
pub fn SetMockExpireTime(ctx: context::Context, tm: time::Time) -> context::Context {
    context::WithValue(ctx, mockExpireTimeKey {}, tm)
}

// EvalExpireTime returns the expired time.
// 这个自由函数对应 Go 中通过 SQL 表达式计算 INTERVAL 的实现。
pub fn EvalExpireTime(
    now: time::Time,
    interval: &str,
    unit: ast::TimeUnitType,
) -> Result<time::Time, errors::Error> {
    // Firstly, we should use the UTC time zone to compute the expired time to avoid time shift caused by DST.
    // The start time should be a time with the same datetime string as `now` but it is in the UTC timezone.
    // 这里保持 Go 的“同一日期时间字符串、换成 UTC location”的处理，而不是直接转 UTC。
    let start = time::Date(
        now.Year(),
        now.Month(),
        now.Day(),
        now.Hour(),
        now.Minute(),
        now.Second(),
        now.Nanosecond(),
        time::UTC,
    );

    let exprCtx = exprstatic::NewExprContext();
    // we need to set the location to UTC to make sure the time is in the same timezone as the start time.
    intest::Assert(exprCtx.GetEvalCtx().Location() == time::UTC);
    let expr = expression::ParseSimpleExpr(
        exprCtx.clone(),
        format!(
            "FROM_UNIXTIME(0) + INTERVAL {} MICROSECOND - INTERVAL {} {}",
            start.UnixMicro(),
            interval,
            unit.String()
        ),
    )?;

    let (tm, _, _) = expr.EvalTime(exprCtx.GetEvalCtx(), chunk::Row::default())?;
    let end = tm.GoTime(time::UTC)?;

    // Then we should add the duration between the time get from the previous SQL and the start time to the now time.
    // 再把 SQL 表达式得到的差值加回原 now，保留原时区，并截断到秒避免测试中精度差异。
    let expiredTime = now.Add(end.Sub(start)).Truncate(time::Second);
    Ok(expiredTime)
}

impl PhysicalTable {
    // FullName returns the full name of the table
    pub fn FullName(&self) -> String {
        if !self.Partition.L.is_empty() {
            return format!("{}.{}.{}", self.Schema.O, self.TableInfo.Name.O, self.Partition.O);
        }
        format!("{}.{}", self.Schema.O, self.TableInfo.Name.O)
    }

    // EvalExpireTime returns the expired time for the current time.
    // It uses the global timezone in session to evaluation the context
    // and the return time is in the same timezone of now argument.
    pub fn EvalExpireTime(
        &self,
        ctx: context::Context,
        se: &dyn session::Session,
        now: time::Time,
    ) -> Result<time::Time, errors::Error> {
        if intest::InTest {
            if let Some(tm) = ctx.Value::<time::Time>(mockExpireTimeKey {}) {
                return Ok(tm);
            }
        }

        // Use the global time zone to compute expire time.
        // Different timezones may have different results event with the same "now" time and TTL expression.
        // Go 先把 now 转到全局时区计算 TTL，再转回调用方传入的 now.Location()。
        let globalTz = se.GlobalTimeZone(ctx)?;
        let start = now.In(globalTz);
        let ttlInfo = self.TableInfo.TTLInfo.as_ref().unwrap();
        let expire = EvalExpireTime(
            start,
            &ttlInfo.IntervalExprStr,
            ast::TimeUnitType(ttlInfo.IntervalTimeUnit),
        )?;

        Ok(expire.In(now.Location()))
    }

    // SplitScanRanges split ranges for TTL scan
    // 该方法根据主键/公共句柄类型选择拆分策略；非 TiKV 或不适合拆分时退回全表范围。
    pub fn SplitScanRanges(
        &self,
        ctx: context::Context,
        store: &dyn kv::Storage,
        splitCnt: i32,
    ) -> Result<Vec<ScanRange>, errors::Error> {
        if self.KeyColumns.len() < 1 || splitCnt <= 1 {
            return Ok(vec![newFullRange()]);
        }

        let tikvStore = match store.as_tikv_storage() {
            Some(s) => s,
            None => return Ok(vec![newFullRange()]),
        };

        let ft = &self.KeyColumns[0].FieldType;
        match ft.GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeInt24 => {
                if self.KeyColumns.len() > 1 {
                    return self.splitCommonHandleRanges(
                        ctx,
                        tikvStore,
                        splitCnt,
                        true,
                        mysql::HasUnsignedFlag(ft.GetFlag()),
                        None,
                    );
                }
                self.splitIntRanges(ctx, tikvStore, splitCnt)
            }
            mysql::TypeBit => self.splitCommonHandleRanges(ctx, tikvStore, splitCnt, false, false, None),
            mysql::TypeString | mysql::TypeVarString | mysql::TypeVarchar => {
                let mut decode: Option<fn(Vec<u8>) -> types::Datum> = None;
                if !mysql::HasBinaryFlag(ft.GetFlag()) {
                    match ft.GetCharset().as_str() {
                        charset::CharsetASCII | charset::CharsetLatin1 => {
                            // ASCII and Latin1 are 8-bit charset, we can use GetASCIIPrefixDatumFromBytes to decode it.
                            decode = Some(GetASCIIPrefixDatumFromBytes);
                        }
                        charset::CharsetUTF8 | charset::CharsetUTF8MB4 => {
                            match ft.GetCollate().as_str() {
                                charset::CollationUTF8 | charset::CollationUTF8MB4 | "utf8mb4_0900_bin" => {
                                    // We can only use GetASCIIPrefixDatumFromBytes to decode UTF8 and UTF8MB4 when they are
                                    // "utf8_bin" or "utf8mb4_bin" collation.
                                    decode = Some(GetASCIIPrefixDatumFromBytes);
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                    if decode.is_none() {
                        return Ok(vec![newFullRange()]);
                    }
                }
                self.splitCommonHandleRanges(ctx, tikvStore, splitCnt, false, false, decode)
            }
            _ => Ok(vec![newFullRange()]),
        }
    }
}

// unsignedEdge 对应 Go 中无符号主键边界的转换。
// NULL 表示 math.MaxInt64 右侧边界；0 则转换成 NULL 让范围从头开始。
pub fn unsignedEdge(d: types::Datum) -> types::Datum {
    if d.IsNull() {
        return types::NewUintDatum((i64::MAX as u64) + 1);
    }
    if d.GetInt64() == 0 {
        return nullDatum();
    }
    types::NewUintDatum(d.GetInt64() as u64)
}

impl PhysicalTable {
    // splitIntRanges 对应单列 int handle 表的 region 切分。
    pub fn splitIntRanges(
        &self,
        ctx: context::Context,
        store: tikv::Storage,
        splitCnt: i32,
    ) -> Result<Vec<ScanRange>, errors::Error> {
        let recordPrefix = tablecodec::GenTableRecordPrefix(self.ID);
        let (startKey, endKey) = tablecodec::GetTableHandleKeyRange(self.ID);
        let keyRanges = self.splitRawKeyRanges(ctx, store, startKey, endKey, splitCnt)?;

        if keyRanges.len() <= 1 {
            return Ok(vec![newFullRange()]);
        }

        let ft = &self.KeyColumnTypes[0];
        let unsigned = mysql::HasUnsignedFlag(ft.GetFlag());
        let mut scanRanges = Vec::with_capacity(keyRanges.len() + 1);
        let mut curScanStart = nullDatum();
        for (i, keyRange) in keyRanges.iter().enumerate() {
            if i != 0 && curScanStart.IsNull() {
                break;
            }

            let mut curScanEnd = nullDatum();
            if i < keyRanges.len() - 1 {
                if let Some(val) = GetNextIntHandle(keyRange.EndKey.clone(), recordPrefix.clone()) {
                    curScanEnd = types::NewIntDatum(val.IntValue());
                }
            }

            if !curScanStart.IsNull()
                && !curScanEnd.IsNull()
                && curScanStart.GetInt64() >= curScanEnd.GetInt64()
            {
                continue;
            }

            if !unsigned {
                // primary key is signed or range
                scanRanges.push(newDatumRange(curScanStart.clone(), curScanEnd.clone()));
            } else if !curScanStart.IsNull() && curScanStart.GetInt64() >= 0 {
                // primary key is unsigned and range is in the right half side
                scanRanges.push(newDatumRange(unsignedEdge(curScanStart.clone()), unsignedEdge(curScanEnd.clone())));
            } else if !curScanEnd.IsNull() && curScanEnd.GetInt64() <= 0 {
                // primary key is unsigned and range is in the left half side
                scanRanges.push(newDatumRange(unsignedEdge(curScanStart.clone()), unsignedEdge(curScanEnd.clone())));
            } else {
                // primary key is unsigned and the start > math.MaxInt64 && end < math.MaxInt64
                // we must split it to two ranges
                scanRanges.push(newDatumRange(unsignedEdge(curScanStart.clone()), nullDatum()));
                scanRanges.push(newDatumRange(nullDatum(), unsignedEdge(curScanEnd.clone())));
            }
            curScanStart = curScanEnd;
        }
        Ok(scanRanges)
    }

    // splitCommonHandleRanges 对应公共句柄表的 region 切分。
    // isInt/unsigned/decode 三个参数保留 Go 的分支控制：整数公共句柄、无符号处理、字符串前缀解码。
    pub fn splitCommonHandleRanges(
        &self,
        ctx: context::Context,
        store: tikv::Storage,
        splitCnt: i32,
        isInt: bool,
        unsigned: bool,
        decode: Option<fn(Vec<u8>) -> types::Datum>,
    ) -> Result<Vec<ScanRange>, errors::Error> {
        let recordPrefix = tablecodec::GenTableRecordPrefix(self.ID);
        let startKey = recordPrefix.clone();
        let endKey = recordPrefix.PrefixNext();
        let keyRanges = self.splitRawKeyRanges(ctx, store, startKey, endKey, splitCnt)?;

        if keyRanges.len() <= 1 {
            return Ok(vec![newFullRange()]);
        }

        let mut scanRanges = Vec::with_capacity(keyRanges.len());
        let mut curScanStart = nullDatum();
        for (i, keyRange) in keyRanges.iter().enumerate() {
            let mut curScanEnd = nullDatum();
            if i != keyRanges.len() - 1 {
                if isInt {
                    curScanEnd = GetNextIntDatumFromCommonHandle(
                        keyRange.EndKey.clone(),
                        recordPrefix.clone(),
                        unsigned,
                    );
                } else {
                    curScanEnd = GetNextBytesHandleDatum(keyRange.EndKey.clone(), recordPrefix.clone());
                    if let Some(decode_fn) = decode {
                        curScanEnd = decode_fn(curScanEnd.GetBytes());
                    }

                    // "" is the smallest value for string/[]byte, skip to add it to ranges.
                    if curScanEnd.GetBytes().is_empty() {
                        continue;
                    }
                }
            }

            if !curScanStart.IsNull() && !curScanEnd.IsNull() {
                // Sometimes curScanStart >= curScanEnd because the edge datum is an approximate value.
                // At this time, we should skip this range to ensure the incremental of ranges.
                let cmp = curScanStart.Compare(
                    types::StrictContext,
                    &curScanEnd,
                    collate::GetBinaryCollator(),
                )?;
                intest::AssertNoError(Ok(()));
                if cmp >= 0 {
                    continue;
                }
            }

            scanRanges.push(newDatumRange(curScanStart.clone(), curScanEnd.clone()));
            if curScanEnd.IsNull() {
                break;
            }
            curScanStart = curScanEnd;
        }
        Ok(scanRanges)
    }

    // splitRawKeyRanges 对应按 TiKV region 分组生成原始 key range 的逻辑。
    // 这里保留 LocateKeyRange、oversizeCnt 和日志字段，本身不会真的访问 region cache。
    pub fn splitRawKeyRanges(
        &self,
        ctx: context::Context,
        store: tikv::Storage,
        startKey: kv::Key,
        endKey: kv::Key,
        splitCnt: i32,
    ) -> Result<Vec<kv::KeyRange>, errors::Error> {
        let mut maxSleep = 20000;
        if intest::InTest {
            maxSleep = 500; // reduce the max sleep time in test
        }

        let regionCache = store.GetRegionCache();
        let mut regions = regionCache.LocateKeyRange(
            tikv::NewBackofferWithVars(ctx, maxSleep, None),
            startKey.clone(),
            endKey.clone(),
        )?;

        let regionsCnt = regions.len();
        let regionsPerRange = regionsCnt / splitCnt as usize;
        let mut oversizeCnt = regionsCnt % splitCnt as usize;
        let mut ranges = Vec::with_capacity(std::cmp::min(regionsCnt, splitCnt as usize));
        while !regions.is_empty() {
            let startRegion = regions[0].clone();

            let mut endRegionIdx = regionsPerRange - 1;
            if oversizeCnt > 0 {
                endRegionIdx += 1;
            }

            let endRegion = regions[endRegionIdx].clone();

            let mut rangeStartKey = kv::Key(startRegion.StartKey);
            if rangeStartKey.Cmp(&startKey) < 0 {
                rangeStartKey = startKey.clone();
            }

            let mut rangeEndKey = kv::Key(endRegion.EndKey);
            if rangeEndKey.Cmp(&endKey) > 0 {
                rangeEndKey = endKey.clone();
            }

            ranges.push(kv::KeyRange {
                StartKey: rangeStartKey,
                EndKey: rangeEndKey,
            });
            oversizeCnt = oversizeCnt.saturating_sub(1);
            regions = regions[(endRegionIdx + 1)..].to_vec();
        }
        logutil::BgLogger().Info(
            "TTL table raw key ranges split",
            vec![
                zap::Int("regionsCnt", regionsCnt as i32),
                zap::Int("shouldSplitCnt", splitCnt),
                zap::Int("actualSplitCnt", ranges.len() as i32),
                zap::Int64("tableID", self.ID),
                zap::String("db", self.Schema.O.clone()),
                zap::String("table", self.TableInfo.Name.O.clone()),
                zap::String("partition", self.Partition.O.clone()),
            ],
        );
        Ok(ranges)
    }
}

// Go 的 init 会在包加载时填充三个公共句柄类型标记；用可变静态保留同名全局状态。
pub static mut commonHandleBytesByte: u8 = 0;
pub static mut commonHandleIntByte: u8 = 0;
pub static mut commonHandleUintByte: u8 = 0;

// init 对应 Go 的包初始化函数，依赖 codec.EncodeKey 得到不同 Datum 编码的首字节。
pub fn init() {
    let mut key = codec::EncodeKey(time::UTC, None, vec![types::NewBytesDatum(Vec::<u8>::new())])
        .expect("codec EncodeKey for bytes datum");
    terror::MustNil(Ok(()));
    unsafe {
        commonHandleBytesByte = key[0];
    }

    key = codec::EncodeKey(time::UTC, None, vec![types::NewIntDatum(0)])
        .expect("codec EncodeKey for int datum");
    terror::MustNil(Ok(()));
    unsafe {
        commonHandleIntByte = key[0];
    }

    key = codec::EncodeKey(time::UTC, None, vec![types::NewUintDatum(0)])
        .expect("codec EncodeKey for uint datum");
    terror::MustNil(Ok(()));
    unsafe {
        commonHandleUintByte = key[0];
    }
}

// GetNextIntHandle is used for int handle tables.
// It returns the min handle whose encoded key is or after argument `key`
// If it cannot find a valid value, a null datum will be returned.
pub fn GetNextIntHandle(key: kv::Key, recordPrefix: Vec<u8>) -> Option<kv::Handle> {
    if key.Cmp(&recordPrefix) > 0 && !key.HasPrefix(&recordPrefix) {
        return None;
    }

    if key.Cmp(&recordPrefix) <= 0 {
        return Some(kv::IntHandle(i64::MIN));
    }

    let suffix = &key[recordPrefix.len()..];
    let mut encodedVal = suffix.to_vec();
    if suffix.len() < 8 {
        encodedVal = vec![0; 8];
        encodedVal[..suffix.len()].copy_from_slice(suffix);
    }

    let mut findNext = false;
    if suffix.len() > 8 {
        findNext = true;
        encodedVal.truncate(8);
    }

    let u = codec::DecodeCmpUintToInt(binary::BigEndian::Uint64(&encodedVal));
    if !findNext {
        return Some(kv::IntHandle(u));
    }

    if u == i64::MAX {
        return None;
    }

    Some(kv::IntHandle(u + 1))
}

// GetNextIntDatumFromCommonHandle is used for common handle tables with int value.
// It returns the min handle whose encoded key is or after argument `key`
// If it cannot find a valid value, a null datum will be returned.
pub fn GetNextIntDatumFromCommonHandle(
    key: kv::Key,
    recordPrefix: Vec<u8>,
    unsigned: bool,
) -> types::Datum {
    if key.Cmp(&recordPrefix) > 0 && !key.HasPrefix(&recordPrefix) {
        return nullDatum();
    }

    let typeByte = unsafe {
        if unsigned {
            commonHandleUintByte
        } else {
            commonHandleIntByte
        }
    };

    let mut minDatum = types::Datum::default();
    if unsigned {
        minDatum.SetUint64(0);
    } else {
        minDatum.SetInt64(i64::MIN);
    }

    if key.Cmp(&recordPrefix) <= 0 {
        return minDatum;
    }

    let mut encodedVal = key[recordPrefix.len()..].to_vec();
    if encodedVal[0] < typeByte {
        return minDatum;
    }

    if encodedVal[0] > typeByte {
        return nullDatum();
    }

    if encodedVal.len() < 9 {
        let mut newVal = vec![0; 9];
        newVal[..encodedVal.len()].copy_from_slice(&encodedVal);
        encodedVal = newVal;
    }

    let (_, mut v) = match codec::DecodeOne(&encodedVal) {
        Ok((remain, datum)) => {
            intest::AssertNoError(Ok(()));
            (remain, datum)
        }
        Err(err) => {
            // should never happen
            terror::Log(errors::Annotatef(
                err,
                format!(
                    "TTL decode common handle failed, key: {}",
                    hex::EncodeToString(&key)
                ),
            ));
            return nullDatum();
        }
    };

    if encodedVal.len() > 9 {
        if (unsigned && v.GetUint64() == u64::MAX) || (!unsigned && v.GetInt64() == i64::MAX) {
            return nullDatum();
        }

        if unsigned {
            v.SetUint64(v.GetUint64() + 1);
        } else {
            v.SetInt64(v.GetInt64() + 1);
        }
    }

    v
}

// GetNextBytesHandleDatum is used for a table with one binary or string column common handle.
// It returns the minValue whose encoded key is or after argument `key`
// If it cannot find a valid value, a null datum will be returned.
pub fn GetNextBytesHandleDatum(key: kv::Key, recordPrefix: Vec<u8>) -> types::Datum {
    if key.Cmp(&recordPrefix) > 0 && !key.HasPrefix(&recordPrefix) {
        return nullDatum();
    }

    if key.Cmp(&recordPrefix) <= 0 {
        let mut d = types::Datum::default();
        d.SetBytes(Vec::<u8>::new());
        return d;
    }

    let mut encodedVal = key[recordPrefix.len()..].to_vec();
    let commonHandleBytesByte = unsafe { commonHandleBytesByte };
    if encodedVal[0] < commonHandleBytesByte {
        let mut d = types::Datum::default();
        d.SetBytes(Vec::<u8>::new());
        return d;
    }

    if encodedVal[0] > commonHandleBytesByte {
        return nullDatum();
    }

    if let Ok((remain, mut v)) = codec::DecodeOne(&encodedVal) {
        if !remain.is_empty() {
            v.SetBytes(kv::Key(v.GetBytes()).Next());
        }
        return v;
    }

    encodedVal = encodedVal[1..].to_vec();
    let mut brokenGroupEndIdx: isize = encodedVal.len() as isize - 1;
    let mut brokenGroupEmptyBytes = encodedVal.len() % 9;
    let mut i = 7;
    while i + 1 < encodedVal.len() {
        let emptyBytes = 255 - encodedVal[i + 1] as usize;
        if emptyBytes != 0 || i + 1 == encodedVal.len() - 1 {
            brokenGroupEndIdx = i as isize;
            brokenGroupEmptyBytes = emptyBytes;
            break;
        }
        i += 9;
    }

    for _ in 0..brokenGroupEmptyBytes {
        if encodedVal[brokenGroupEndIdx as usize] > 0 {
            break;
        }
        brokenGroupEndIdx -= 1;
    }

    if brokenGroupEndIdx < 0 {
        let mut d = types::Datum::default();
        d.SetBytes(Vec::<u8>::new());
        return d;
    }

    let mut val = Vec::with_capacity(encodedVal.len());
    for i in 0..=brokenGroupEndIdx as usize {
        if i % 9 == 8 {
            continue;
        }
        val.push(encodedVal[i]);
    }
    let mut d = types::Datum::default();
    d.SetBytes(val);
    d
}

// GetASCIIPrefixDatumFromBytes is used to convert bytes to string datum which only contains ASCII prefix string.
// The ASCII prefix string only contains visible characters and `\t`, `\n`, `\r`.
// "abc" -> "abc"
// "\0abc" -> ""
// "ab\x01c" -> "ab"
// "ab\xffc" -> "ab"
// "ab\rc\xff" -> "ab\rc"
pub fn GetASCIIPrefixDatumFromBytes(mut bs: Vec<u8>) -> types::Datum {
    for (i, c) in bs.iter().copied().enumerate() {
        if (0x20..=0x7E).contains(&c) {
            // visible characters from ` ` to `~`
            continue;
        }

        if c == b'\t' || c == b'\n' || c == b'\r' {
            continue;
        }

        // Go 这里切到第一个非可见 ASCII 字节之前，保留前缀作为 string datum。
        bs.truncate(i);
        break;
    }
    types::NewStringDatum(String::from_utf8_lossy(&bs).to_string())
}
*/

use crate::task::Datum;

/// 扫描键列的值类型：有符号/无符号整数或字节串。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeyKind {
    #[default]
    SignedInt,
    UnsignedInt,
    Bytes,
}
/// 简化列元信息：名称、是否 public、键类型。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Column {
    pub name: String,
    pub public: bool,
    pub key_kind: KeyKind,
}
/// 分区定义：物理分区 ID 与名称。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
}
/// 表级 TTL 配置：时间列名、间隔数值与时间单位。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TTLInfo {
    pub column_name: String,
    pub interval: String,
    pub unit: TimeUnit,
}
/// TTL 间隔时间单位（与 `TTL_JOB_INTERVAL` 语义对齐）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimeUnit {
    Microsecond,
    Second,
    Minute,
    HourMinute,
    Hour,
    Day,
    Week,
    #[default]
    Month,
    Quarter,
    Year,
}
/// 构造 PhysicalTable 所需的最小表元信息子集。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub public: bool,
    pub pk_is_handle: bool,
    pub common_handle: bool,
    pub columns: Vec<Column>,
    pub primary_index_offsets: Vec<usize>,
    pub partitions: Vec<PartitionDefinition>,
    pub ttl: Option<TTLInfo>,
}

/// 按句柄形态选择 TTL 扫描键列：整型主键 / 聚簇索引 / 隐式 `_tidb_rowid`。
pub fn getTableKeyColumns(table: &TableInfo) -> Result<Vec<Column>, String> {
    if table.pk_is_handle {
        return table
            .columns
            .iter()
            .find(|column| {
                column.public
                    && matches!(column.key_kind, KeyKind::SignedInt | KeyKind::UnsignedInt)
            })
            .cloned()
            .map(|column| vec![column])
            .ok_or_else(|| format!("Cannot find primary key for table: {}", table.name));
    }
    if table.common_handle {
        if table.primary_index_offsets.is_empty() {
            return Err(format!("Cannot find primary key for table: {}", table.name));
        }
        return table
            .primary_index_offsets
            .iter()
            .map(|offset| {
                table
                    .columns
                    .get(*offset)
                    .filter(|column| column.public)
                    .cloned()
                    .ok_or_else(|| format!("invalid primary key column offset {offset}"))
            })
            .collect();
    }
    // 无用户主键时 TiDB 使用隐式 row id 作为记录句柄。
    Ok(vec![Column {
        name: "_tidb_rowid".into(),
        public: true,
        key_kind: KeyKind::SignedInt,
    }])
}

/// 逻辑扫描区间：Start/End 为空表示该侧开放（全表或半开）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanRange {
    pub Start: Vec<Datum>,
    pub End: Vec<Datum>,
}
/// 构造覆盖整表的开放区间。
pub fn newFullRange() -> ScanRange {
    ScanRange::default()
}
/// 由起止 Datum 构造区间；Null 表示该侧开放。
pub fn newDatumRange(start: Datum, end: Datum) -> ScanRange {
    ScanRange {
        Start: if matches!(start, Datum::Null) {
            Vec::new()
        } else {
            vec![start]
        },
        End: if matches!(end, Datum::Null) {
            Vec::new()
        } else {
            vec![end]
        },
    }
}
/// 返回表示开放边界的 Null Datum。
pub fn nullDatum() -> Datum {
    Datum::Null
}

/// 可执行 TTL 扫描的物理表视图（含分区与键列）。
#[derive(Clone, Debug)]
pub struct PhysicalTable {
    pub ID: i64,
    pub Schema: String,
    pub TableInfo: TableInfo,
    pub Partition: String,
    pub PartitionDef: Option<PartitionDefinition>,
    pub KeyColumns: Vec<Column>,
    pub TimeColumn: Column,
}
/// 在已知时间列前提下构造 PhysicalTable，并解析分区物理 ID。
pub fn NewBasePhysicalTable(
    schema: &str,
    table: &TableInfo,
    partition: &str,
    time_column: Column,
) -> Result<PhysicalTable, String> {
    if !table.public {
        return Err(format!(
            "table '{}.{}' is not a public table",
            schema, table.name
        ));
    }
    let key_columns = getTableKeyColumns(table)?;
    let (id, definition) = if table.partitions.is_empty() {
        if !partition.is_empty() {
            return Err(format!(
                "table '{}.{}' is not a partitioned table",
                schema, table.name
            ));
        }
        (table.id, None)
    } else {
        if partition.is_empty() {
            return Err(format!(
                "partition name is required, table '{}.{}' is a partitioned table",
                schema, table.name
            ));
        }
        let definition = table
            .partitions
            .iter()
            .find(|definition| definition.name.eq_ignore_ascii_case(partition))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "partition '{partition}' is not found in ttl table '{}.{}'",
                    schema, table.name
                )
            })?;
        (definition.id, Some(definition))
    };
    Ok(PhysicalTable {
        ID: id,
        Schema: schema.into(),
        TableInfo: table.clone(),
        Partition: partition.into(),
        PartitionDef: definition,
        KeyColumns: key_columns,
        TimeColumn: time_column,
    })
}
/// 校验表启用 TTL 后查找时间列，再委托 NewBasePhysicalTable。
pub fn NewPhysicalTable(
    schema: &str,
    table: &TableInfo,
    partition: &str,
) -> Result<PhysicalTable, String> {
    let ttl = table
        .ttl
        .as_ref()
        .ok_or_else(|| format!("table '{}.{}' is not a ttl table", schema, table.name))?;
    let time_column = table
        .columns
        .iter()
        .find(|column| column.public && column.name.eq_ignore_ascii_case(&ttl.column_name))
        .cloned()
        .ok_or_else(|| {
            format!(
                "time column '{}' is not public in ttl table '{}.{}'",
                ttl.column_name, schema, table.name
            )
        })?;
    NewBasePhysicalTable(schema, table, partition, time_column)
}
impl PhysicalTable {
    /// 校验键前缀长度不超过 KeyColumns。
    pub fn ValidateKeyPrefix(&self, key: &[Datum]) -> Result<(), String> {
        if key.len() > self.KeyColumns.len() {
            Err(format!(
                "invalid key length: {}, expected {}",
                key.len(),
                self.KeyColumns.len()
            ))
        } else {
            Ok(())
        }
    }
    /// 返回 schema.table 或 schema.table.partition 全名。
    pub fn FullName(&self) -> String {
        if self.Partition.is_empty() {
            format!("{}.{}", self.Schema, self.TableInfo.name)
        } else {
            format!("{}.{}.{}", self.Schema, self.TableInfo.name, self.Partition)
        }
    }
    /// 按表上 TTLInfo 计算“早于此时间戳的行视为过期”。
    pub fn EvalExpireTime(&self, now_seconds: i64) -> Result<i64, String> {
        let ttl = self
            .TableInfo
            .ttl
            .as_ref()
            .ok_or_else(|| "TTL info is missing".to_owned())?;
        EvalExpireTime(now_seconds, &ttl.interval, ttl.unit)
    }
    /// 按 Region 边界把表记录前缀拆成多个 Datum 扫描区间。
    pub fn SplitScanRanges(
        &self,
        regions: Option<&dyn RegionProvider>,
        split_count: usize,
    ) -> Result<Vec<ScanRange>, String> {
        // 无法拆分或只要求一段时直接返回全表范围。
        if self.KeyColumns.is_empty() || split_count <= 1 {
            return Ok(vec![newFullRange()]);
        }
        let Some(regions) = regions else {
            return Ok(vec![newFullRange()]);
        };
        let prefix = record_prefix(self.ID);
        let raw = splitRawKeyRanges(regions, &prefix, &prefix_next(prefix.clone()), split_count)?;
        if raw.len() <= 1 {
            return Ok(vec![newFullRange()]);
        }
        // 以首个键列类型把原始 Region 结束键转成下一区间的起始 Datum。
        let key_kind = self.KeyColumns[0].key_kind;
        if key_kind == KeyKind::UnsignedInt {
            return Ok(split_unsigned_int_ranges(&raw, &prefix));
        }
        let mut output = Vec::new();
        let mut start = Datum::Null;
        for (index, range) in raw.iter().enumerate() {
            let end = if index + 1 == raw.len() {
                Datum::Null
            } else {
                match key_kind {
                    KeyKind::SignedInt => GetNextIntHandle(&range.end, &prefix)
                        .map(Datum::Int)
                        .unwrap_or(Datum::Null),
                    KeyKind::UnsignedInt => unreachable!("handled before the common path"),
                    KeyKind::Bytes => GetNextBytesHandleDatum(&range.end, &prefix),
                }
            };
            if datum_less(&start, &end)
                || matches!(start, Datum::Null)
                || matches!(end, Datum::Null)
            {
                output.push(newDatumRange(start.clone(), end.clone()));
            }
            start = end;
        }
        Ok(output)
    }
}

/// 公历日期 → 距 Unix epoch 的整天数（Howard Hinnant 算法）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
/// 距 Unix epoch 的整天数 → 公历年月日。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}
/// 返回指定年月的天数（含闰年二月）。
fn month_days(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
/// 从当前时刻减去 TTL 间隔，得到过期水位时间戳（秒）。
pub fn EvalExpireTime(now_seconds: i64, interval: &str, unit: TimeUnit) -> Result<i64, String> {
    if unit == TimeUnit::HourMinute {
        let value = interval.trim().trim_matches('\'');
        let (hours, minutes) = value
            .split_once(':')
            .ok_or_else(|| format!("invalid TTL HOUR_MINUTE interval {interval}"))?;
        let hours: i64 = hours
            .parse()
            .map_err(|error| format!("invalid TTL interval {interval}: {error}"))?;
        let minutes: i64 = minutes
            .parse()
            .map_err(|error| format!("invalid TTL interval {interval}: {error}"))?;
        return Ok(now_seconds - hours * 3600 - minutes * 60);
    }
    let amount: i64 = interval
        .trim()
        .parse()
        .map_err(|error| format!("invalid TTL interval {interval}: {error}"))?;
    match unit {
        TimeUnit::Microsecond => Ok(now_seconds - amount / 1_000_000),
        TimeUnit::Second => Ok(now_seconds - amount),
        TimeUnit::Minute => Ok(now_seconds - amount * 60),
        TimeUnit::HourMinute => unreachable!("handled before integer interval parsing"),
        TimeUnit::Hour => Ok(now_seconds - amount * 3600),
        TimeUnit::Day => Ok(now_seconds - amount * 86400),
        TimeUnit::Week => Ok(now_seconds - amount * 7 * 86400),
        // 月/季/年按公历回退，并钳制到目标月的合法日。
        TimeUnit::Month | TimeUnit::Quarter | TimeUnit::Year => {
            let days = now_seconds.div_euclid(86400);
            let seconds = now_seconds.rem_euclid(86400);
            let (year, month, day) = civil_from_days(days);
            let months = match unit {
                TimeUnit::Month => amount,
                TimeUnit::Quarter => amount * 3,
                TimeUnit::Year => amount * 12,
                _ => unreachable!(),
            };
            let total = year * 12 + month - 1 - months;
            let new_year = total.div_euclid(12);
            let new_month = total.rem_euclid(12) + 1;
            Ok(days_from_civil(
                new_year,
                new_month,
                day.min(month_days(new_year, new_month)),
            ) * 86400
                + seconds)
        }
    }
}

/// TiKV 原始键字节区间（半开）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyRange {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}
/// 查询覆盖 [start, end) 的 Region 列表。
pub trait RegionProvider {
    fn locate_key_range(&self, start: &[u8], end: &[u8]) -> Result<Vec<KeyRange>, String>;
}
/// 将 Region 列表均匀合并为约 split_count 段原始键范围。
pub fn splitRawKeyRanges(
    provider: &dyn RegionProvider,
    start: &[u8],
    end: &[u8],
    split_count: usize,
) -> Result<Vec<KeyRange>, String> {
    let regions = provider.locate_key_range(start, end)?;
    if regions.is_empty() {
        return Ok(Vec::new());
    }
    let groups = split_count.min(regions.len());
    let base = regions.len() / groups;
    let extra = regions.len() % groups;
    let mut output = Vec::with_capacity(groups);
    let mut cursor = 0;
    for group in 0..groups {
        let length = base + usize::from(group < extra);
        let first = &regions[cursor];
        let last = &regions[cursor + length - 1];
        output.push(KeyRange {
            start: if first.start.as_slice() < start {
                start.to_vec()
            } else {
                first.start.clone()
            },
            end: if last.end.as_slice() > end {
                end.to_vec()
            } else {
                last.end.clone()
            },
        });
        cursor += length;
    }
    Ok(output)
}
/// 构造表记录键前缀：`t{table_id}_r`（table_id 按有符号大端编码）。
fn record_prefix(table_id: i64) -> Vec<u8> {
    let mut prefix = vec![b't'];
    prefix.extend_from_slice(&((table_id as u64 ^ (1 << 63)).to_be_bytes()));
    prefix.extend_from_slice(b"_r");
    prefix
}
/// 计算字典序上严格大于 key 的最短前缀（用于半开区间上界）。
fn prefix_next(mut key: Vec<u8>) -> Vec<u8> {
    for index in (0..key.len()).rev() {
        if key[index] != 0xff {
            key[index] += 1;
            key.truncate(index + 1);
            return key;
        }
    }
    key.push(0);
    key
}
/// 同类型 Datum 比较；类型不符时保守视为可推进。
fn datum_less(left: &Datum, right: &Datum) -> bool {
    match (left, right) {
        (Datum::Int(a), Datum::Int(b)) => a < b,
        (Datum::UInt(a), Datum::UInt(b)) => a < b,
        (Datum::Bytes(a), Datum::Bytes(b)) => a < b,
        (Datum::String(a), Datum::String(b)) => a < b,
        _ => true,
    }
}

/// 将有符号 handle 编码顺序转换为无符号扫描顺序。
///
/// TiKV 的原始 Region 边界按 `i64` 从负到正排列，而 SQL 无符号值的
/// 顺序是 `0..=u64::MAX`。因此跨过 0 的原始区间必须分为高半区与低半区，
/// 与 Go `splitIntRanges` / `unsignedEdge` 保持一致。
fn split_unsigned_int_ranges(raw: &[KeyRange], prefix: &[u8]) -> Vec<ScanRange> {
    fn unsigned_edge(edge: Option<i64>) -> Datum {
        match edge {
            None => Datum::UInt(i64::MAX as u64 + 1),
            Some(0) => Datum::Null,
            Some(value) => Datum::UInt(value as u64),
        }
    }

    let mut output = Vec::with_capacity(raw.len() + 1);
    let mut start = None;
    for (index, range) in raw.iter().enumerate() {
        if index != 0 && start.is_none() {
            break;
        }
        let end = if index + 1 == raw.len() {
            None
        } else {
            GetNextIntHandle(&range.end, prefix)
        };
        if matches!((start, end), (Some(left), Some(right)) if left >= right) {
            continue;
        }

        if start.is_some_and(|value| value >= 0) || end.is_some_and(|value| value <= 0) {
            output.push(newDatumRange(unsigned_edge(start), unsigned_edge(end)));
        } else {
            output.push(newDatumRange(unsigned_edge(start), Datum::Null));
            output.push(newDatumRange(Datum::Null, unsigned_edge(end)));
        }
        start = end;
    }
    output
}
/// 从记录键解析下一整数句柄；越出前缀返回 None。
pub fn GetNextIntHandle(key: &[u8], record_prefix: &[u8]) -> Option<i64> {
    if key > record_prefix && !key.starts_with(record_prefix) {
        return None;
    }
    if key <= record_prefix {
        return Some(i64::MIN);
    }
    let suffix = &key[record_prefix.len()..];
    let mut encoded = [0u8; 8];
    encoded[..suffix.len().min(8)].copy_from_slice(&suffix[..suffix.len().min(8)]);
    let value = (u64::from_be_bytes(encoded) ^ (1 << 63)) as i64;
    if suffix.len() > 8 {
        value.checked_add(1)
    } else {
        Some(value)
    }
}
/// 聚簇索引整数句柄的下一 Datum；失败则为 Null。
pub fn GetNextIntDatumFromCommonHandle(key: &[u8], record_prefix: &[u8], unsigned: bool) -> Datum {
    let Some(value) = GetNextIntHandle(key, record_prefix) else {
        return Datum::Null;
    };
    if unsigned {
        Datum::UInt(value as u64)
    } else {
        Datum::Int(value)
    }
}
/// 字节串句柄的下一 Datum（对后缀做 prefix_next）。
pub fn GetNextBytesHandleDatum(key: &[u8], record_prefix: &[u8]) -> Datum {
    if key > record_prefix && !key.starts_with(record_prefix) {
        return Datum::Null;
    }
    if key <= record_prefix {
        return Datum::Bytes(Vec::new());
    }
    let mut value = key[record_prefix.len()..].to_vec();
    if !value.is_empty() {
        value = prefix_next(value);
    }
    Datum::Bytes(value)
}
/// 截取可见 ASCII/空白前缀作为 string Datum，供范围边界可读化。
pub fn GetASCIIPrefixDatumFromBytes(bytes: &[u8]) -> Datum {
    let end = bytes
        .iter()
        .position(|byte| {
            !((*byte >= 0x20 && *byte <= 0x7e) || matches!(*byte, b'\t' | b'\n' | b'\r'))
        })
        .unwrap_or(bytes.len());
    Datum::String(String::from_utf8_lossy(&bytes[..end]).into_owned())
}
