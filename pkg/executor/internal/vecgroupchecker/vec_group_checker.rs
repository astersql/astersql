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

// 向量化分组检查器：将有序 Chunk 按 GROUP BY 键切分为连续等值组。
//
// 假定输入 Chunk 已按分组键排序。通过比较相邻行（及跨 Chunk 的首尾键）
// 找出组边界，供聚合算子用 `GetNextGroup` 按 `[begin, end)` 区间迭代。
// 分组键（group-by key）是 GROUP BY 表达式在一行上的编码结果。

use crate::{chunk, codec, expression, types};

/// 可注入的临时列分配函数；测试用它验证分配错误与借还契约。
pub type AllocateBuffer =
    fn(types::EvalType, usize) -> Result<Box<chunk::Column>, expression::Error>;
/// 可注入的临时列释放函数。
pub type ReleaseBuffer = fn(Box<chunk::Column>);

/// Splits an ordered chunk into contiguous groups with equal group-by keys.
/// 将有序 Chunk 切分为分组键相等的连续分组；跨 Chunk 时用上一批末键衔接。
pub struct VecGroupChecker<'ctx> {
    /// 表达式求值上下文（时区等）。
    pub ctx: Option<&'ctx dyn expression::EvalContext>,
    /// 临时列释放入口；None 时使用 expression 全局列池。
    pub releaseBuffer: Option<ReleaseBuffer>,
    /// 临时列分配入口；None 时使用 expression 全局列池。
    pub allocateBuffer: Option<AllocateBuffer>,
    /// 当前 Chunk 最后一行各 GROUP BY 列的 Datum。
    pub lastRowDatums: Vec<types::Datum>,
    /// 上一 Chunk 末行分组键的编码，用于判断跨批组是否连续。
    pub lastGroupKeyOfPrevChk: Vec<u8>,
    /// 当前 Chunk 首行分组键编码。
    pub firstGroupKey: Vec<u8>,
    /// 当前 Chunk 末行分组键编码。
    pub lastGroupKey: Vec<u8>,
    /// 当前 Chunk 首行各 GROUP BY 列的 Datum。
    pub firstRowDatums: Vec<types::Datum>,
    /// 逐行标记：`sameGroup[i]==false` 表示第 i 行开启新组。
    pub sameGroup: Vec<bool>,
    /// 各组结束下标（不含）序列，末尾恒为行数。
    pub groupOffset: Vec<usize>,
    /// GROUP BY 表达式列表。
    pub GroupByItems: Vec<expression::ExprBox>,
    /// 下一次 `GetNextGroup` 将返回的组序号。
    pub nextGroupID: usize,
    /// 当前 Chunk 切分出的组个数。
    pub groupCount: usize,
    /// 是否启用向量化路径（迁移保留字段）。
    pub vecEnabled: bool,
}

/// 构造 `VecGroupChecker`：绑定求值上下文、向量化开关与 GROUP BY 项。
pub fn NewVecGroupChecker<'ctx>(
    ctx: Option<&'ctx dyn expression::EvalContext>,
    vecEnabled: bool,
    items: Vec<expression::ExprBox>,
) -> Box<VecGroupChecker<'ctx>> {
    Box::new(VecGroupChecker {
        ctx,
        releaseBuffer: None,
        allocateBuffer: None,
        vecEnabled,
        GroupByItems: items,
        groupCount: 0,
        nextGroupID: 0,
        sameGroup: Vec::with_capacity(1024),
        lastRowDatums: Vec::new(),
        lastGroupKeyOfPrevChk: Vec::new(),
        firstGroupKey: Vec::new(),
        lastGroupKey: Vec::new(),
        firstRowDatums: Vec::new(),
        groupOffset: Vec::new(),
    })
}

impl VecGroupChecker<'_> {
    /// 对输入 Chunk 执行分组切分。
    ///
    /// 返回值：当前 Chunk 首组是否与上一 Chunk 末组键相同（跨批组延续）。
    /// 无 GROUP BY 项时整批视为一组；首尾键相同时可走快路径只产生一组。
    pub fn SplitIntoGroups(&mut self, chk: &chunk::Chunk) -> Result<bool, expression::Error> {
        let numRows = chk.NumRows();
        self.Reset();
        self.nextGroupID = 0;

        // 无分组键：整块 Chunk 作为单一分组。
        if self.GroupByItems.is_empty() {
            self.groupOffset.push(numRows);
            self.groupCount = 1;
            return Ok(true);
        }

        if numRows == 0 {
            return Err(expression::errors::Errorf(
                "VecGroupChecker requires a non-empty chunk",
            ));
        }

        // 先取首/末行各列 Datum，再编码为分组键以便跨 Chunk 比较。
        for item in self.GroupByItems.clone() {
            self.getFirstAndLastRowDatum(item.as_ref(), chk, numRows)?;
        }
        let ctx = self.ctx.ok_or_else(|| {
            expression::errors::Errorf("VecGroupChecker requires an evaluation context")
        })?;
        self.firstGroupKey = codec::EncodeKey(
            ctx.Location(),
            std::mem::take(&mut self.firstGroupKey),
            self.firstRowDatums.clone(),
        )
        .map_err(expression::Error::from)?;
        self.lastGroupKey = codec::EncodeKey(
            ctx.Location(),
            std::mem::take(&mut self.lastGroupKey),
            self.lastRowDatums.clone(),
        )
        .map_err(expression::Error::from)?;

        // 与上一 Chunk 末键比较，判断首组是否延续。
        let firstSameAsPrevious = !self.lastGroupKeyOfPrevChk.is_empty()
            && self.lastGroupKeyOfPrevChk == self.firstGroupKey;
        self.lastGroupKeyOfPrevChk.clone_from(&self.lastGroupKey);

        // 首尾键相同：整批同组，跳过逐行扫描。
        if self.firstGroupKey == self.lastGroupKey {
            self.groupOffset.push(numRows);
            self.groupCount = 1;
            return Ok(firstSameAsPrevious);
        }

        // 逐列比较相邻行，在 sameGroup 上标记组边界，再收集 groupOffset。
        self.sameGroup.resize(numRows, true);
        self.sameGroup[0] = false;
        for item in self.GroupByItems.clone() {
            self.evalGroupItemsAndResolveGroups(item.as_ref(), chk, numRows)?;
        }
        for index in 1..numRows {
            if !self.sameGroup[index] {
                self.groupOffset.push(index);
            }
        }
        self.groupOffset.push(numRows);
        self.groupCount = self.groupOffset.len();
        Ok(firstSameAsPrevious)
    }

    /// 将指定 GROUP BY 列在首行与末行上的 Datum 追加到缓存，供 EncodeKey 使用。
    fn getFirstAndLastRowDatum(
        &mut self,
        item: &dyn expression::Expression,
        chk: &chunk::Chunk,
        numRows: usize,
    ) -> Result<(), expression::Error> {
        let ctx = self.ctx.ok_or_else(|| {
            expression::errors::Errorf("VecGroupChecker requires an evaluation context")
        })?;
        let evalType = item.GetType(ctx).EvalType();
        if !matches!(
            evalType,
            types::ETInt
                | types::ETReal
                | types::ETDecimal
                | types::ETDatetime
                | types::ETTimestamp
                | types::ETDuration
                | types::ETJson
                | types::ETVectorFloat32
                | types::ETString
        ) {
            return Err(expression::errors::Errorf(format!(
                "unsupported type {evalType:?} during evaluation"
            )));
        }

        let evaluate = |row: chunk::Row| -> Result<types::Datum, expression::Error> {
            let mut datum = types::Datum::default();
            match evalType {
                types::ETInt => {
                    let (value, is_null) = item.EvalInt(ctx, row)?;
                    if is_null {
                        datum.SetNull();
                    } else {
                        datum.SetInt64(value);
                    }
                }
                types::ETReal => {
                    let (value, is_null) = item.EvalReal(ctx, row)?;
                    if is_null {
                        datum.SetNull();
                    } else {
                        datum.SetFloat64(value);
                    }
                }
                types::ETDecimal => {
                    let (value, is_null) = item.EvalDecimal(ctx, row)?;
                    if is_null {
                        datum.SetNull();
                    } else {
                        datum.SetMysqlDecimal(value);
                    }
                }
                types::ETDatetime | types::ETTimestamp => {
                    let (value, is_null) = item.EvalTime(ctx, row)?;
                    if is_null {
                        datum.SetNull();
                    } else {
                        datum.SetMysqlTime(value);
                    }
                }
                types::ETDuration => {
                    let (value, is_null) = item.EvalDuration(ctx, row)?;
                    if is_null {
                        datum.SetNull();
                    } else {
                        datum.SetMysqlDuration(value);
                    }
                }
                types::ETJson => {
                    let (value, is_null) = item.EvalJSON(ctx, row)?;
                    if is_null {
                        datum.SetNull();
                    } else {
                        datum.SetMysqlJSON(value);
                    }
                }
                types::ETVectorFloat32 => {
                    let (value, is_null) = item.EvalVectorFloat32(ctx, row)?;
                    if is_null {
                        datum.SetNull();
                    } else {
                        datum.SetVectorFloat32(value);
                    }
                }
                types::ETString => {
                    let (value, is_null) = item.EvalString(ctx, row)?;
                    if is_null {
                        datum.SetNull();
                    } else {
                        datum.SetString(value, item.GetType(ctx).GetCollate().to_owned());
                    }
                }
                _ => unreachable!("eval type was validated above"),
            }
            Ok(datum)
        };

        self.firstRowDatums.push(evaluate(chk.GetRow(0))?);
        self.lastRowDatums.push(evaluate(chk.GetRow(numRows - 1))?);
        Ok(())
    }

    /// 按求值类型比较相邻行该列是否变化；若变化则将 `sameGroup[index]` 置 false。
    ///
    /// NULL 与非 NULL 视为不同；字符串比较会按列 collation 规范化。
    fn evalGroupItemsAndResolveGroups(
        &mut self,
        item: &dyn expression::Expression,
        chk: &chunk::Chunk,
        numRows: usize,
    ) -> Result<(), expression::Error> {
        let ctx = self.ctx.ok_or_else(|| {
            expression::errors::Errorf("VecGroupChecker requires an evaluation context")
        })?;
        let evalType = item.GetType(ctx).EvalType();
        let allocate = self.allocateBuffer.unwrap_or(expression::GetColumn);
        let release = self.releaseBuffer.unwrap_or(expression::PutColumn);
        let mut column = allocate(evalType, numRows)?;

        let result = (|| {
            expression::EvalExpr(ctx, self.vecEnabled, item, evalType, chk, column.as_mut())?;
            let mut previousIsNull = column.IsNull(0);

            for index in 1..numRows {
                let isNull = column.IsNull(index);
                // 仅当此前各列仍认为同组时才继续比较本列，避免多余计算。
                if self.sameGroup[index] {
                    let valueChanged = if previousIsNull || isNull {
                        // 任一为 NULL：仅当 NULL 状态翻转时才算变化。
                        previousIsNull != isNull
                    } else {
                        match evalType {
                            types::ETInt => column.GetInt64(index) != column.GetInt64(index - 1),
                            types::ETReal => {
                                column.GetFloat64(index) != column.GetFloat64(index - 1)
                            }
                            types::ETDecimal => {
                                column
                                    .GetDecimal(index)
                                    .Compare(&column.GetDecimal(index - 1))
                                    != 0
                            }
                            types::ETDatetime | types::ETTimestamp => {
                                column.GetTime(index).Compare(column.GetTime(index - 1)) != 0
                            }
                            types::ETDuration => {
                                column.GetDuration(index, item.GetType(ctx).GetDecimal() as i32)
                                    != column.GetDuration(
                                        index - 1,
                                        item.GetType(ctx).GetDecimal() as i32,
                                    )
                            }
                            types::ETJson => {
                                types::CompareBinaryJSON(
                                    &column.GetJSON(index - 1),
                                    &column.GetJSON(index),
                                ) != 0
                            }
                            types::ETVectorFloat32 => {
                                column
                                    .GetVectorFloat32(index - 1)
                                    .Compare(&column.GetVectorFloat32(index))
                                    != 0
                            }
                            types::ETString => {
                                // 按 collation 转换后再比较，使 utf8_general_ci 等忽略大小写。
                                codec::ConvertByCollationStr(
                                    column.GetString(index - 1),
                                    item.GetType(ctx),
                                ) != codec::ConvertByCollationStr(
                                    column.GetString(index),
                                    item.GetType(ctx),
                                )
                            }
                            _ => {
                                return Err(expression::errors::Errorf(format!(
                                    "unsupported type {evalType:?} during evaluation"
                                )));
                            }
                        }
                    };
                    if valueChanged {
                        self.sameGroup[index] = false;
                    }
                }
                previousIsNull = isNull;
            }
            Ok(())
        })();
        release(column);
        result
    }

    /// 返回下一组在 Chunk 中的半开区间 `[begin, end)`，并推进 `nextGroupID`。
    pub fn GetNextGroup(&mut self) -> (usize, usize) {
        let begin = if self.nextGroupID == 0 {
            0
        } else {
            self.groupOffset[self.nextGroupID - 1]
        };
        let end = self.groupOffset[self.nextGroupID];
        self.nextGroupID += 1;
        (begin, end)
    }

    /// 是否已消费完当前 Chunk 的全部分组。
    pub fn IsExhausted(&self) -> bool {
        self.nextGroupID >= self.groupCount
    }

    /// 清空本批切分状态（保留 `lastGroupKeyOfPrevChk` 以便跨 Chunk 衔接）。
    pub fn Reset(&mut self) {
        self.groupOffset.clear();
        self.groupCount = 0;
        self.sameGroup.clear();
        self.firstGroupKey.clear();
        self.lastGroupKey.clear();
        self.firstRowDatums.clear();
        self.lastRowDatums.clear();
    }

    /// 返回当前 Chunk 切分出的分组个数。
    pub fn GroupCount(&self) -> usize {
        self.groupCount
    }
}
