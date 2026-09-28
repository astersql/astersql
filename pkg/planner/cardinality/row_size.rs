// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 平均行宽（row size）估算。
//
// 为代价模型（cost model）提供索引扫描、表扫描与 chunk/磁盘格式下的平均行宽；
// 优先用列直方图的 TotColSize，缺失时退回每列 8 byte 的伪列宽。

use crate::*;

// 优化器按统计信息估算行宽。
// expression、kv、mysql、planctx、statistics、tablecodec 与 chunk 等跨包类型留待后续模块接线。

/// 统计缺失时每列的伪平均宽度（字节）。
const pseudoColSize: f64 = 8.0;

/// GetIndexAvgRowSize 对应 Go 的索引扫描平均行宽估算。
/// 索引 key 前缀长度和非唯一索引额外分隔符按 Go 中的固定常量叠加。
pub fn GetIndexAvgRowSize(
    ctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    cols: &[&expression::Column],
    isUnique: bool,
) -> f64 {
    let mut size = GetAvgRowSize(ctx, coll, cols, true, true);
    // tablePrefix(1) + tableID(8) + indexPrefix(2) + indexID(8)。
    // index scan 的 cols 一定包含 handle，Go 这里不再补 rowID。
    size += 19.0;
    if !isUnique {
        // 非唯一索引用 "_" 串接 handle，保留 Go 的额外 1 byte 估算。
        size += 1.0;
    }
    size
}

/// GetTableAvgRowSize 对应 Go 的表扫描平均行宽估算，排除索引 key-value 对。
pub fn GetTableAvgRowSize(
    ctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    cols: &[&expression::Column],
    storeType: kv::StoreType,
    handleInCols: bool,
) -> f64 {
    let mut size = GetAvgRowSize(ctx, coll, cols, false, true);
    match storeType {
        kv::TiKV => {
            size += tablecodec::RecordRowKeyLen as f64;
            // TiKV 的 cols 总是包含 row_id，前缀行宽需要扣掉它自身长度。
            size -= 8.0;
        }
        kv::TiFlash => {
            if !handleInCols {
                size += 8.0; // row_id length
            }
        }
        _ => {}
    }
    // Go 用 max(0, size) 避免兼容路径产生负行宽。
    size.max(0.0)
}

/// GetAvgRowSize 按给定列集合估算平均行宽。
/// 伪统计、缺失统计或实时行数为 0 时退回每列 8 byte 的固定估算。
pub fn GetAvgRowSize(
    ctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    cols: &[&expression::Column],
    isEncodedKey: bool,
    isForScan: bool,
) -> f64 {
    let sessionVars = ctx.GetSessionVars();
    let mut size = 0.0;
    if coll.Pseudo || coll.ColNum() == 0 || coll.RealtimeCount == 0 {
        size = pseudoColSize * cols.len() as f64;
    } else {
        for col in cols {
            let colHist = coll.GetCol(col.UniqueID);
            // 兼容旧版本统计：TotColSize 缺失且并非全 NULL 时，用伪列宽兜底。
            if colHist.is_none()
                || (!colHist.unwrap().IsHandle
                    && colHist.unwrap().TotColSize == 0
                    && colHist.unwrap().NullCount != coll.RealtimeCount)
            {
                size += pseudoColSize;
                continue;
            }
            let colHist = colHist.unwrap();
            // key/value 编码与 chunk RPC 格式的列宽不同，保留 Go 的三路选择。
            if sessionVars.EnableChunkRPC && !isForScan {
                size += AvgColSizeChunkFormat(colHist, coll.RealtimeCount);
            } else {
                size += AvgColSize(colHist, coll.RealtimeCount, isEncodedKey);
            }
        }
    }
    size = size.max(0.0);
    if sessionVars.EnableChunkRPC && !isForScan {
        // Chunk RPC 为每列补 1/8 byte 的 null bitmap。
        return size + cols.len() as f64 / 8.0;
    }
    // 非 chunk 格式每列补 1 byte flag，和 Go encode 细节保持一致。
    size + cols.len() as f64
}

/// GetAvgRowSizeDataInDiskByRows 对应 Go 中按 DataInDiskByRows 表示估算列宽。
pub fn GetAvgRowSizeDataInDiskByRows(
    coll: &statistics::HistColl,
    cols: &[&expression::Column],
) -> f64 {
    let mut size = 0.0;
    if coll.Pseudo || coll.ColNum() == 0 || coll.RealtimeCount == 0 {
        for col in cols {
            size += chunk::EstimateTypeWidth(col.GetStaticType()) as f64;
        }
    } else {
        for col in cols {
            let colHist = coll.GetCol(col.UniqueID);
            if colHist.is_none()
                || (!colHist.unwrap().IsHandle
                    && colHist.unwrap().TotColSize == 0
                    && colHist.unwrap().NullCount != coll.RealtimeCount)
            {
                size += chunk::EstimateTypeWidth(col.GetStaticType()) as f64;
                continue;
            }
            size += AvgColSizeDataInDiskByRows(colHist.unwrap(), coll.RealtimeCount);
        }
    }
    // DataInDiskByRows 为每列额外记录 8 byte 大小信息。
    (size + (8 * cols.len()) as f64).max(0.0)
}

/// AvgColSize 对应 Go 的直方图平均列宽估算。
/// 这些常量来自 encode 与 Datum::ConvertTo 的编码形状。
pub fn AvgColSize(c: &statistics::Column, count: i64, isKey: bool) -> f64 {
    if count == 0 {
        return 0.0;
    }
    // handle 作为 value 编码时真实长度可能小于 8，这里沿用 Go 的近似值。
    if c.IsHandle {
        return 8.0;
    }
    let histCount = c.TotalRowCount();
    let mut notNullRatio = 1.0;
    if histCount > 0.0 {
        notNullRatio = (1.0 - c.NullCount as f64 / histCount).max(0.0);
    }
    match c.Histogram.Tp.GetType() {
        mysql::TypeFloat
        | mysql::TypeDouble
        | mysql::TypeDuration
        | mysql::TypeDate
        | mysql::TypeDatetime
        | mysql::TypeTimestamp => return 8.0 * notNullRatio,
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeYear
        | mysql::TypeEnum
        | mysql::TypeBit
        | mysql::TypeSet => {
            if isKey {
                return 8.0 * notNullRatio;
            }
        }
        _ => {}
    }
    // Go 保留两位小数；这里用 round 保持同一舍入粒度。
    ((c.TotColSize as f64 / count as f64 * 100.0).round() / 100.0).max(0.0)
}

/// AvgColSizeChunkFormat 对应 Go 的 chunk 格式平均列宽估算。
pub fn AvgColSizeChunkFormat(c: &statistics::Column, count: i64) -> f64 {
    if count == 0 {
        return 0.0;
    }
    let fixedLen = chunk::GetFixedLen(&c.Histogram.Tp);
    if fixedLen != chunk::VarElemLen {
        return fixedLen as f64;
    }
    // 变长类型需要 offsets；Go 还用 Log2(avgSize) 近似 LEN 开销。
    let avgSize = c.TotColSize as f64 / count as f64;
    if avgSize < 1.0 {
        return ((avgSize * 100.0).round() / 100.0).max(0.0) + 8.0;
    }
    (((avgSize - avgSize.log2()) * 100.0).round() / 100.0).max(0.0) + 8.0
}

/// AvgColSizeDataInDiskByRows 对应 Go 的 DataInDiskByRows 平均列宽估算。
pub fn AvgColSizeDataInDiskByRows(c: &statistics::Column, count: i64) -> f64 {
    if count == 0 {
        return 0.0;
    }
    let histCount = c.TotalRowCount();
    let mut notNullRatio = 1.0;
    if histCount > 0.0 {
        notNullRatio = 1.0 - c.NullCount as f64 / histCount;
    }
    let size = chunk::GetFixedLen(&c.Histogram.Tp);
    if size != chunk::VarElemLen {
        return size as f64 * notNullRatio;
    }
    // 变长类型沿用 Go 的两位小数和 Log2 修正。
    let avgSize = c.TotColSize as f64 / count as f64;
    if avgSize < 1.0 {
        return (avgSize * 100.0).round() / 100.0;
    }
    ((avgSize - avgSize.log2()) * 100.0).round() / 100.0
}
