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

// Joiner：按连接类型生成匹配/未匹配结果行。
//
// Hash Join / Index Join 等在键相等后，把外表与内表行交给 Joiner，由其根据
// Inner/Outer/Semi/Anti 语义与 other condition（三值逻辑）决定输出或标记。
// 本文件上半保留 Go 映射注释，下半为可运行实现。

// Joiner 接口、不同 join 类型的匹配/未匹配输出规则，以及过滤后状态回写流程。
//
// Joiner is used to generate join results according to the join type.
// A typical instruction flow is:
//	hasMatch, HasNull := false, false
//	for innerIter.Current() != innerIter.End() {
//	    matched, isNull, err := j.TryToMatchInners(Outer, innerIter, chk)
//	    // handle err
//	    hasMatch = hasMatch || matched
//	    HasNull = HasNull || isNull
//	}
//	if !hasMatch {
//	    j.OnMissMatch(HasNull, Outer, chk)
//	}
// NOTE: This interface is **not** thread-safe.
// TODO: unit test
// for all join type
//  1. no filter, no inline projection
//  2. no filter, inline projection
//  3. no filter, inline projection to empty column
//  4. filter, no inline projection
//  5. filter, inline projection
//  6. filter, inline projection to empty column
// pub trait Joiner {
// TryToMatchInners 对应 Go 接口方法：用一个 outer row 扫一批 inner row，并把结果写入 chk。
// 返回值中的 isNull 只对 anti/semi 系列有意义，用于区分条件为 NULL 还是 false。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         chk: &mut chunk::Chunk,
//         opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error>;
//
// TryToMatchOuters 对应 Go 接口方法：用一个 inner row 扫一批 outer row，常见于 outer 侧建表。
//     fn TryToMatchOuters(
//         &mut self,
//         outer: &mut chunk::Iterator,
//         inner: chunk::Row,
//         chk: &mut chunk::Chunk,
//         outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error>;
//
// OnMissMatch 对应 Go 接口方法：outer 行未匹配时按 join 类型输出或忽略。
//     fn OnMissMatch(&mut self, hasNull: bool, outer: chunk::Row, chk: &mut chunk::Chunk);
//
// isSemiJoinWithoutCondition 对应 Go 优化判断：无条件 semi join 命中一个 inner 即可结束。
//     fn isSemiJoinWithoutCondition(&self) -> bool;
//
// Clone deep copies a Joiner.
//     fn CloneJoiner(&self) -> Box<dyn Joiner>;
// }
//
// JoinerType returns the join type of a Joiner.
// pub fn JoinerType(j: &dyn std::any::Any) -> plannerbase::JoinType {
// Go 使用 type switch；用 Any 仅保留判别顺序，不负责真实动态分发接线。
//     if j.is::<semiJoiner>() {
//         plannerbase::SemiJoin
//     } else if j.is::<antiSemiJoiner>() {
//         plannerbase::AntiSemiJoin
//     } else if j.is::<leftOuterSemiJoiner>() {
//         plannerbase::LeftOuterSemiJoin
//     } else if j.is::<antiLeftOuterSemiJoiner>() {
//         plannerbase::AntiLeftOuterSemiJoin
//     } else if j.is::<leftOuterJoiner>() {
//         plannerbase::LeftOuterJoin
//     } else if j.is::<rightOuterJoiner>() {
//         plannerbase::RightOuterJoin
//     } else {
//         plannerbase::InnerJoin
//     }
// }
//
// NewJoiner create a joiner
// pub fn NewJoiner(
//     ctx: sessionctx::Context,
//     joinType: plannerbase::JoinType,
//     outerIsRight: bool,
//     defaultInner: Vec<types::Datum>,
//     filter: Vec<expression::Expression>,
//     lhsColTypes: Vec<*mut types::FieldType>,
//     rhsColTypes: Vec<*mut types::FieldType>,
//     childrenUsed: Option<Vec<Vec<usize>>>,
//     isNA: bool,
// ) -> Box<dyn Joiner> {
//     let mut base = baseJoiner {
//         ctx,
//         conditions: filter,
//         defaultInner: chunk::Row::default(),
//         outerIsRight,
//         chk: None,
//         shallowRow: chunk::MutRow::default(),
//         selected: Vec::with_capacity(chunk::InitialCapacity),
//         isNull: Vec::with_capacity(chunk::InitialCapacity),
//         maxChunkSize: ctx.GetSessionVars().MaxChunkSize,
//         lUsed: None,
//         rUsed: None,
//     };
//
//     if let Some(childrenUsed) = childrenUsed {
// lUsed/rUsed 必须保持 Go 中 children schema 的原始顺序，不能因为 join 方向被反转而重排。
//         base.lUsed = Some(childrenUsed[0].clone());
//         base.rUsed = Some(childrenUsed[1].clone());
//         logutil::BgLogger().Debug(
//             "InlineProjection",
//             zap::Ints("lUsed", base.lUsed.as_ref().unwrap()),
//             zap::Ints("rUsed", base.rUsed.as_ref().unwrap()),
//             zap::Int("lCount", lhsColTypes.len()),
//             zap::Int("rCount", rhsColTypes.len()),
//         );
//     }
//
//     if joinType == plannerbase::LeftOuterJoin || joinType == plannerbase::RightOuterJoin {
//         let mut innerColTypes = lhsColTypes.clone();
//         if !outerIsRight {
//             innerColTypes = rhsColTypes.clone();
//         }
//         base.initDefaultInner(innerColTypes, defaultInner);
//     }
//
// shallowRowType 不受 inline projection 裁剪，因为 join condition 可能还要读被输出裁掉的列。
//     let mut shallowRowType = Vec::with_capacity(lhsColTypes.len() + rhsColTypes.len());
//     shallowRowType.extend(lhsColTypes.clone());
//     shallowRowType.extend(rhsColTypes.clone());
//
//     match joinType {
//         plannerbase::SemiJoin => {
//             base.shallowRow = chunk::MutRowFromTypes(shallowRowType);
//             Box::new(semiJoiner { baseJoiner: base })
//         }
//         plannerbase::AntiSemiJoin => {
//             base.shallowRow = chunk::MutRowFromTypes(shallowRowType);
//             if isNA {
//                 Box::new(nullAwareAntiSemiJoiner { baseJoiner: base })
//             } else {
//                 Box::new(antiSemiJoiner { baseJoiner: base })
//             }
//         }
//         plannerbase::LeftOuterSemiJoin => {
//             base.shallowRow = chunk::MutRowFromTypes(shallowRowType);
//             Box::new(leftOuterSemiJoiner { baseJoiner: base })
//         }
//         plannerbase::AntiLeftOuterSemiJoin => {
//             base.shallowRow = chunk::MutRowFromTypes(shallowRowType);
//             if isNA {
//                 Box::new(nullAwareAntiLeftOuterSemiJoiner { baseJoiner: base })
//             } else {
//                 Box::new(antiLeftOuterSemiJoiner { baseJoiner: base })
//             }
//         }
//         plannerbase::LeftOuterJoin | plannerbase::RightOuterJoin | plannerbase::InnerJoin => {
//             if !base.conditions.is_empty() {
//                 let vars = ctx.GetSessionVars();
//                 base.chk = Some(chunk::New(shallowRowType, vars.InitChunkSize, vars.MaxChunkSize));
//             }
//             match joinType {
//                 plannerbase::LeftOuterJoin => Box::new(leftOuterJoiner { baseJoiner: base }),
//                 plannerbase::RightOuterJoin => Box::new(rightOuterJoiner { baseJoiner: base }),
//                 plannerbase::InnerJoin => Box::new(innerJoiner { baseJoiner: base }),
//                 _ => panic!("unsupported join type in func NewJoiner()"),
//             }
//         }
//         _ => panic!("unsupported join type in func NewJoiner()"),
//     }
// }
//
// pub type outerRowStatusFlag = u8;
//
// pub const outerRowUnmatched: outerRowStatusFlag = 0;
// pub const outerRowMatched: outerRowStatusFlag = 1;
// pub const outerRowHasNull: outerRowStatusFlag = 2;
//
// baseJoiner 对应 Go 公共结构体，保存 filter、默认 inner row、inline projection 列和临时 chunk。
// pub struct baseJoiner {
//     pub ctx: sessionctx::Context,
//     pub conditions: Vec<expression::Expression>,
//     pub defaultInner: chunk::Row,
//     pub outerIsRight: bool,
//     pub chk: Option<chunk::Chunk>,
//     pub shallowRow: chunk::MutRow,
//     pub selected: Vec<bool>,
//     pub isNull: Vec<bool>,
//     pub maxChunkSize: usize,
// lUsed/rUsed show which columns are used by father for left child and right child.
// NOTE:
// 1. every columns are used if lUsed/rUsed is nil.
// 2. no columns are used if lUsed/rUsed is not nil but the size of lUsed/rUsed is 0.
//     pub lUsed: Option<Vec<usize>>,
//     pub rUsed: Option<Vec<usize>>,
// }
//
// impl baseJoiner {
// initDefaultInner 对应 Go 方法：用 inner 类型构造一行默认值，供 outer join 未匹配时补 NULL/默认值。
//     pub fn initDefaultInner(&mut self, innerTypes: Vec<*mut types::FieldType>, defaultInner: Vec<types::Datum>) {
//         let mut mutableRow = chunk::MutRowFromTypes(innerTypes.clone());
//         mutableRow.SetDatums(defaultInner[..innerTypes.len()].to_vec());
//         self.defaultInner = mutableRow.ToRow();
//     }
//
// makeJoinRowToChunk 对应 Go 方法：先 AppendRow 增加虚拟行，再追加另一侧列。
//     pub fn makeJoinRowToChunk(
//         &self,
//         chk: &mut chunk::Chunk,
//         lhs: chunk::Row,
//         rhs: chunk::Row,
//         lUsed: Option<&Vec<usize>>,
//         rUsed: Option<&Vec<usize>>,
//     ) {
// Call AppendRow() first to increment the virtual rows.
// Fix: https://github.com/pingcap/tidb/issues/5771
//         let lWide = chk.AppendRowByColIdxs(lhs, lUsed);
//         chk.AppendPartialRowByColIdxs(lWide, rhs, rUsed);
//     }
//
// makeShallowJoinRow shallow copies `inner` and `outer` into `shallowRow`.
// It should not consider `j.lUsed` and `j.rUsed`, because the columns which
// need to be used in `j.conditions` may not exist in outputs.
//     pub fn makeShallowJoinRow(&mut self, isRightJoin: bool, mut inner: chunk::Row, mut outer: chunk::Row) {
//         if !isRightJoin {
//             std::mem::swap(&mut inner, &mut outer);
//         }
//         self.shallowRow.ShallowCopyPartialRow(0, inner);
//         self.shallowRow.ShallowCopyPartialRow(inner.Len(), outer);
//     }
//
// filter 对应 Go 方法：过滤一个 outer row 和多个 inner row 构造出的 join 结果。
//     pub fn filter(
//         &mut self,
//         mut input: &mut chunk::Chunk,
//         output: &mut chunk::Chunk,
//         mut outerColLen: usize,
//         lUsed: Option<&Vec<usize>>,
//         rUsed: Option<&Vec<usize>>,
//     ) -> Result<bool, errors::Error> {
//         self.selected = expression::VectorizedFilter(
//             self.ctx.GetExprCtx().GetEvalCtx(),
//             self.ctx.GetSessionVars().EnableVectorizedExpression,
//             &self.conditions,
//             chunk::NewIterator4Chunk(input),
//             self.selected.clone(),
//         )?;
//
// 批量复制 selected 行；outerIsRight 决定 inner/outer 在 input 中的列偏移。
//         let mut innerColOffset = 0;
//         let mut outerColOffset = input.NumCols() - outerColLen;
//         let mut innerColLen = input.NumCols() - outerColLen;
//         if !self.outerIsRight {
//             innerColOffset = outerColLen;
//             outerColOffset = 0;
//         }
//         if lUsed.is_some() || rUsed.is_some() {
//             let mut lSize = outerColOffset;
//             if !self.outerIsRight {
//                 lSize = innerColOffset;
//             }
//             let l = lUsed.cloned().unwrap_or_default();
//             let r = rUsed.cloned().unwrap_or_default();
//             let mut used = Vec::with_capacity(l.len() + r.len());
//             used.extend(l.iter().copied());
//             for col in &r {
//                 used.push(*col + lSize);
//             }
//             input = input.Prune(&used);
//
//             innerColOffset = 0;
//             outerColOffset = l.len();
//             innerColLen = l.len();
//             outerColLen = r.len();
//             if !self.outerIsRight {
//                 innerColOffset = l.len();
//                 outerColOffset = 0;
//                 std::mem::swap(&mut innerColLen, &mut outerColLen);
//             }
//         }
//         chunk::CopySelectedJoinRowsWithSameOuterRows(
//             input,
//             innerColOffset,
//             innerColLen,
//             outerColOffset,
//             outerColLen,
//             &self.selected,
//             output,
//         )
//     }
//
// filterAndCheckOuterRowStatus 对应 Go 方法：过滤多 outer + 单 inner 的结果，并回写每个 outer 的状态。
//     pub fn filterAndCheckOuterRowStatus(
//         &mut self,
//         mut input: &mut chunk::Chunk,
//         output: &mut chunk::Chunk,
//         innerColsLen: usize,
//         mut outerRowStatus: Vec<outerRowStatusFlag>,
//         lUsed: Option<&Vec<usize>>,
//         rUsed: Option<&Vec<usize>>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
//         let (selected, isNull) = expression::VectorizedFilterConsiderNull(
//             self.ctx.GetExprCtx().GetEvalCtx(),
//             self.ctx.GetSessionVars().EnableVectorizedExpression,
//             &self.conditions,
//             chunk::NewIterator4Chunk(input),
//             self.selected.clone(),
//             self.isNull.clone(),
//         )?;
//         self.selected = selected;
//         self.isNull = isNull;
//         for i in 0..self.selected.len() {
//             if self.isNull[i] {
//                 outerRowStatus[i] = outerRowHasNull;
//             } else if !self.selected[i] {
//                 outerRowStatus[i] = outerRowUnmatched;
//             }
//         }
//
//         if lUsed.is_some() || rUsed.is_some() {
//             let mut lSize = innerColsLen;
//             if !self.outerIsRight {
//                 lSize = input.NumCols() - innerColsLen;
//             }
//             let l = lUsed.cloned().unwrap_or_default();
//             let r = rUsed.cloned().unwrap_or_default();
//             let mut used = Vec::with_capacity(l.len() + r.len());
//             used.extend(l.iter().copied());
//             for col in &r {
//                 used.push(*col + lSize);
//             }
//             input = input.Prune(&used);
//         }
//         chunk::CopySelectedJoinRowsDirect(input, &self.selected, output)?;
//         Ok(outerRowStatus)
//     }
//
// Clone 对应 Go 方法：深拷贝 conditions、临时 chunk/shallowRow、默认 inner 和 inline projection。
//     pub fn Clone(&self) -> baseJoiner {
//         let mut base = baseJoiner {
//             ctx: self.ctx,
//             conditions: Vec::with_capacity(self.conditions.len()),
//             defaultInner: chunk::Row::default(),
//             outerIsRight: self.outerIsRight,
//             chk: None,
//             shallowRow: chunk::MutRow::default(),
//             selected: Vec::with_capacity(self.selected.len()),
//             isNull: Vec::with_capacity(self.isNull.len()),
//             maxChunkSize: self.maxChunkSize,
//             lUsed: self.lUsed.clone(),
//             rUsed: self.rUsed.clone(),
//         };
//         for con in &self.conditions {
//             base.conditions.push(con.Clone());
//         }
//         if let Some(chk) = &self.chk {
//             base.chk = Some(chk.CopyConstruct());
//         } else {
//             base.shallowRow = self.shallowRow.Clone();
//         }
//         if !self.defaultInner.IsEmpty() {
//             base.defaultInner = self.defaultInner.CopyConstruct();
//         }
//         base
//     }
// }
//
// pub struct semiJoiner {
//     pub baseJoiner: baseJoiner,
// }
//
// impl Joiner for semiJoiner {
// TryToMatchInners 对应 SemiJoin：无条件时命中任一 inner 即输出 outer；有条件时 NULL 当 false 处理。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         chk: &mut chunk::Chunk,
//         _opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error> {
//         if inners.Len() == 0 {
//             return Ok((false, false));
//         }
//         if self.baseJoiner.conditions.is_empty() {
//             chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//             inners.ReachEnd();
//             return Ok((true, false));
//         }
//
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         let mut inner = inners.Current();
//         while inner != inners.End() {
//             self.baseJoiner.makeShallowJoinRow(self.baseJoiner.outerIsRight, inner, outer);
// For SemiJoin, null result of join conditions is treated as false.
//             let (matched, _, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if matched {
//                 chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//                 inners.ReachEnd();
//                 return Ok((true, false));
//             }
//             inner = inners.Next();
//         }
//         inners.Error()?;
//         Ok((false, false))
//     }
//
// TryToMatchOuters 对应 SemiJoin 的 outer 批量匹配。
//     fn TryToMatchOuters(
//         &mut self,
//         outers: &mut chunk::Iterator,
//         inner: chunk::Row,
//         chk: &mut chunk::Chunk,
//         mut outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
//         outerRowStatus.clear();
//         let mut outer = outers.Current();
//         let mut numToAppend = chk.RequiredRows() - chk.NumRows();
//         if self.baseJoiner.conditions.is_empty() {
//             while outer != outers.End() && numToAppend > 0 {
//                 chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//                 outerRowStatus.push(outerRowMatched);
//                 outer = outers.Next();
//                 numToAppend -= 1;
//             }
//             return Ok(outerRowStatus);
//         }
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         while outer != outers.End() && numToAppend > 0 {
//             self.baseJoiner.makeShallowJoinRow(self.baseJoiner.outerIsRight, inner, outer);
//             let (matched, _, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if matched {
//                 outerRowStatus.push(outerRowMatched);
//                 chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//             } else {
//                 outerRowStatus.push(outerRowUnmatched);
//             }
//             outer = outers.Next();
//             numToAppend -= 1;
//         }
//         outers.Error()?;
//         Ok(outerRowStatus)
//     }
//
//     fn OnMissMatch(&mut self, _hasNull: bool, _outer: chunk::Row, _chk: &mut chunk::Chunk) {}
//
//     fn isSemiJoinWithoutCondition(&self) -> bool {
//         self.baseJoiner.conditions.is_empty()
//     }
//
//     fn CloneJoiner(&self) -> Box<dyn Joiner> {
//         Box::new(semiJoiner { baseJoiner: self.baseJoiner.Clone() })
//     }
// }
//
// NAAJType is join detail type only used by null-aware AntiLeftOuterSemiJoin.
// pub type NAAJType = u8;
//
// Unknown for those default value.
// pub const Unknown: NAAJType = 0;
// LeftHasNullRightNotNull means lhs is a null key, and rhs is not a null key.
// pub const LeftHasNullRightNotNull: NAAJType = 1;
// LeftHasNullRightHasNull means lhs is a null key, and rhs is a null key.
// pub const LeftHasNullRightHasNull: NAAJType = 2;
// LeftNotNullRightNotNull means lhs is in not a null key, and rhs is not a null key.
// pub const LeftNotNullRightNotNull: NAAJType = 3;
// LeftNotNullRightHasNull means lhs is in not a null key, and rhs is a null key.
// pub const LeftNotNullRightHasNull: NAAJType = 4;
//
// pub struct nullAwareAntiSemiJoiner {
//     pub baseJoiner: baseJoiner,
// }
//
// impl Joiner for nullAwareAntiSemiJoiner {
// TryToMatchInners 对应 null-aware anti semi join：inner 来自 NULL bucket 或 same-key bucket。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         _chk: &mut chunk::Chunk,
//         _opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error> {
//         if inners.Len() == 0 {
//             return Ok((false, false));
//         }
//         if self.baseJoiner.conditions.is_empty() {
// 没有 other condition 时，右侧存在有效行即可确定 NOT IN 不输出 probe row。
//             inners.ReachEnd();
//             return Ok((true, false));
//         }
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         let mut inner = inners.Current();
//         while inner != inners.End() {
//             self.baseJoiner.makeShallowJoinRow(self.baseJoiner.outerIsRight, inner, outer);
//             let (valid, _, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if valid {
//                 inners.ReachEnd();
//                 return Ok((true, false));
//             }
// false 或 NULL 表示该 inner 行不能作为有效右侧，继续查找。
//             inner = inners.Next();
//         }
//         inners.Error()?;
//         Ok((false, false))
//     }
//
//     fn TryToMatchOuters(
//         &mut self,
//         _outer: &mut chunk::Iterator,
//         _inner: chunk::Row,
//         _chk: &mut chunk::Chunk,
//         outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
// Go 仍是 TODO: use the Outer build；这里保留接口形状。
//         Ok(outerRowStatus)
//     }
//
//     fn OnMissMatch(&mut self, _hasNull: bool, outer: chunk::Row, chk: &mut chunk::Chunk) {
//         chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//     }
//
//     fn isSemiJoinWithoutCondition(&self) -> bool {
//         self.baseJoiner.conditions.is_empty()
//     }
//
//     fn CloneJoiner(&self) -> Box<dyn Joiner> {
//         Box::new(nullAwareAntiSemiJoiner { baseJoiner: self.baseJoiner.Clone() })
//     }
// }
//
// pub struct antiSemiJoiner {
//     pub baseJoiner: baseJoiner,
// }
//
// impl Joiner for antiSemiJoiner {
// TryToMatchInners 对应 AntiSemiJoin：条件为 NULL 时不算 matched，但要把 hasNull 带给 OnMissMatch。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         _chk: &mut chunk::Chunk,
//         _opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error> {
//         if inners.Len() == 0 {
//             return Ok((false, false));
//         }
//         if self.baseJoiner.conditions.is_empty() {
//             inners.ReachEnd();
//             return Ok((true, false));
//         }
//
//         let mut hasNull = false;
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         let mut inner = inners.Current();
//         while inner != inners.End() {
//             self.baseJoiner.makeShallowJoinRow(self.baseJoiner.outerIsRight, inner, outer);
//             let (matched, isNull, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if matched {
//                 inners.ReachEnd();
//                 return Ok((true, false));
//             }
//             hasNull = hasNull || isNull;
//             inner = inners.Next();
//         }
//         inners.Error()?;
//         Ok((false, hasNull))
//     }
//
//     fn TryToMatchOuters(
//         &mut self,
//         outers: &mut chunk::Iterator,
//         inner: chunk::Row,
//         chk: &mut chunk::Chunk,
//         mut outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
//         outerRowStatus.clear();
//         let mut numToAppend = chk.RequiredRows() - chk.NumRows();
//         if self.baseJoiner.conditions.is_empty() {
//             while outers.Current() != outers.End() {
//                 outerRowStatus.push(outerRowMatched);
//                 outers.Next();
//             }
//             return Ok(outerRowStatus);
//         }
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         let mut outer = outers.Current();
//         while outer != outers.End() && numToAppend > 0 {
//             self.baseJoiner.makeShallowJoinRow(self.baseJoiner.outerIsRight, inner, outer);
//             let (matched, isNull, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if matched {
//                 outerRowStatus.push(outerRowMatched);
//             } else if isNull {
//                 outerRowStatus.push(outerRowHasNull);
//             } else {
//                 outerRowStatus.push(outerRowUnmatched);
//             }
//             outer = outers.Next();
//             numToAppend -= 1;
//         }
//         outers.Error()?;
//         Ok(outerRowStatus)
//     }
//
//     fn OnMissMatch(&mut self, hasNull: bool, outer: chunk::Row, chk: &mut chunk::Chunk) {
//         if !hasNull {
//             chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//         }
//     }
//
//     fn isSemiJoinWithoutCondition(&self) -> bool {
//         self.baseJoiner.conditions.is_empty()
//     }
//
//     fn CloneJoiner(&self) -> Box<dyn Joiner> {
//         Box::new(antiSemiJoiner { baseJoiner: self.baseJoiner.Clone() })
//     }
// }
//
// pub struct leftOuterSemiJoiner {
//     pub baseJoiner: baseJoiner,
// }
//
// impl leftOuterSemiJoiner {
// onMatch 对应 Go 辅助方法：输出 outer 行并追加 1。
//     pub fn onMatch(&self, outer: chunk::Row, chk: &mut chunk::Chunk) {
//         let lWide = chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//         chk.AppendInt64(lWide, 1);
//     }
// }
//
// impl Joiner for leftOuterSemiJoiner {
// TryToMatchInners 对应 LeftOuterSemiJoin：matched 输出 1，NULL 状态留给 miss match。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         chk: &mut chunk::Chunk,
//         _opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error> {
//         if inners.Len() == 0 {
//             return Ok((false, false));
//         }
//         if self.baseJoiner.conditions.is_empty() {
//             self.onMatch(outer, chk);
//             inners.ReachEnd();
//             return Ok((true, false));
//         }
//
//         let mut hasNull = false;
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         let mut inner = inners.Current();
//         while inner != inners.End() {
//             self.baseJoiner.makeShallowJoinRow(false, inner, outer);
//             let (matched, isNull, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if matched {
//                 self.onMatch(outer, chk);
//                 inners.ReachEnd();
//                 return Ok((true, false));
//             }
//             hasNull = hasNull || isNull;
//             inner = inners.Next();
//         }
//         inners.Error()?;
//         Ok((false, hasNull))
//     }
//
//     fn TryToMatchOuters(
//         &mut self,
//         outers: &mut chunk::Iterator,
//         inner: chunk::Row,
//         chk: &mut chunk::Chunk,
//         mut outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
//         outerRowStatus.clear();
//         let mut outer = outers.Current();
//         let mut numToAppend = chk.RequiredRows() - chk.NumRows();
//         if self.baseJoiner.conditions.is_empty() {
//             while outer != outers.End() && numToAppend > 0 {
//                 self.onMatch(outer, chk);
//                 outerRowStatus.push(outerRowMatched);
//                 outer = outers.Next();
//                 numToAppend -= 1;
//             }
//             return Ok(outerRowStatus);
//         }
//
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         while outer != outers.End() && numToAppend > 0 {
//             self.baseJoiner.makeShallowJoinRow(false, inner, outer);
//             let (matched, isNull, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if matched {
//                 self.onMatch(outer, chk);
//                 outerRowStatus.push(outerRowMatched);
//             } else if isNull {
//                 outerRowStatus.push(outerRowHasNull);
//             } else {
//                 outerRowStatus.push(outerRowUnmatched);
//             }
//             outer = outers.Next();
//             numToAppend -= 1;
//         }
//         outers.Error()?;
//         Ok(outerRowStatus)
//     }
//
//     fn OnMissMatch(&mut self, hasNull: bool, outer: chunk::Row, chk: &mut chunk::Chunk) {
//         let lWide = chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//         if hasNull {
//             chk.AppendNull(lWide);
//         } else {
//             chk.AppendInt64(lWide, 0);
//         }
//     }
//
//     fn isSemiJoinWithoutCondition(&self) -> bool {
//         self.baseJoiner.conditions.is_empty()
//     }
//
//     fn CloneJoiner(&self) -> Box<dyn Joiner> {
//         Box::new(leftOuterSemiJoiner { baseJoiner: self.baseJoiner.Clone() })
//     }
// }
//
// pub struct nullAwareAntiLeftOuterSemiJoiner {
//     pub baseJoiner: baseJoiner,
// }
//
// impl nullAwareAntiLeftOuterSemiJoiner {
// onMatch 对应 Go 的 NAAJ 输出规则，opt[0] 描述左右 NA-EQ key 是否含 NULL。
//     pub fn onMatch(&self, outer: chunk::Row, chk: &mut chunk::Chunk, opt: &[NAAJType]) {
//         match opt[0] {
//             LeftNotNullRightNotNull => {
// either side are not null. (x NOT IN (x...)) --> (rhs, 0)
//                 let lWide = chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//                 chk.AppendInt64(lWide, 0);
//             }
//             LeftNotNullRightHasNull => {
// right side has a null NA-EQ key. (x NOT IN (null...)) --> (rhs, null)
//                 let lWide = chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//                 chk.AppendNull(lWide);
//             }
//             LeftHasNullRightHasNull | LeftHasNullRightNotNull => {
// left side has a null NA-EQ key. (null NOT IN (what ever valid inner)) --(rhs, null)
//                 let lWide = chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//                 chk.AppendNull(lWide);
//             }
//             _ => {}
//         }
//     }
// }
//
// impl Joiner for nullAwareAntiLeftOuterSemiJoiner {
// TryToMatchInners 对应 null-aware anti left outer semi join 的特殊条件处理。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         chk: &mut chunk::Chunk,
//         opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error> {
//         if inners.Len() == 0 {
//             return Ok((false, false));
//         }
// 与普通 AntiLeftOuterSemiJoiner 不同：conditions 只包含 inner filter，NULL 不再作为 hasNull 传播。
//         if self.baseJoiner.conditions.is_empty() {
//             self.onMatch(outer, chk, opt);
//             inners.ReachEnd();
//             return Ok((true, false));
//         }
//
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         let mut inner = inners.Current();
//         while inner != inners.End() {
//             self.baseJoiner.makeShallowJoinRow(false, inner, outer);
//             let (valid, _, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if valid {
// once find a valid inner row, we can determine the result already.
//                 self.onMatch(outer, chk, opt);
//                 inners.ReachEnd();
//                 return Ok((true, false));
//             }
//             inner = inners.Next();
//         }
//         inners.Error()?;
//         Ok((false, false))
//     }
//
//     fn TryToMatchOuters(
//         &mut self,
//         _outer: &mut chunk::Iterator,
//         _inner: chunk::Row,
//         _chk: &mut chunk::Chunk,
//         _outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
// Go 仍是 TODO；保留未实现 outer build 路径。
//         Ok(Vec::new())
//     }
//
//     fn OnMissMatch(&mut self, _hasNull: bool, outer: chunk::Row, chk: &mut chunk::Chunk) {
// 走到这里表示 NOT IN 的短路径都没命中：空集合或非空但没有 x/null，输出 1。
//         let lWide = chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//         chk.AppendInt64(lWide, 1);
//     }
//
//     fn isSemiJoinWithoutCondition(&self) -> bool {
//         self.baseJoiner.conditions.is_empty()
//     }
//
//     fn CloneJoiner(&self) -> Box<dyn Joiner> {
//         Box::new(nullAwareAntiLeftOuterSemiJoiner { baseJoiner: self.baseJoiner.Clone() })
//     }
// }
//
// pub struct antiLeftOuterSemiJoiner {
//     pub baseJoiner: baseJoiner,
// }
//
// impl antiLeftOuterSemiJoiner {
// onMatch 对应 Go 辅助方法：anti left outer semi 命中时输出 0。
//     pub fn onMatch(&self, outer: chunk::Row, chk: &mut chunk::Chunk) {
//         let lWide = chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//         chk.AppendInt64(lWide, 0);
//     }
// }
//
// impl Joiner for antiLeftOuterSemiJoiner {
// TryToMatchInners 对应 AntiLeftOuterSemiJoin：matched 输出 0，miss 时按 hasNull 输出 NULL/1。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         chk: &mut chunk::Chunk,
//         _opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error> {
//         if inners.Len() == 0 {
//             return Ok((false, false));
//         }
//         if self.baseJoiner.conditions.is_empty() {
//             self.onMatch(outer, chk);
//             inners.ReachEnd();
//             return Ok((true, false));
//         }
//         let mut hasNull = false;
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         let mut inner = inners.Current();
//         while inner != inners.End() {
//             self.baseJoiner.makeShallowJoinRow(false, inner, outer);
//             let (matched, isNull, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if matched {
//                 self.onMatch(outer, chk);
//                 inners.ReachEnd();
//                 return Ok((true, false));
//             }
//             hasNull = hasNull || isNull;
//             inner = inners.Next();
//         }
//         inners.Error()?;
//         Ok((false, hasNull))
//     }
//
//     fn TryToMatchOuters(
//         &mut self,
//         outers: &mut chunk::Iterator,
//         inner: chunk::Row,
//         chk: &mut chunk::Chunk,
//         mut outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
//         outerRowStatus.clear();
//         let mut outer = outers.Current();
//         let mut numToAppend = chk.RequiredRows() - chk.NumRows();
//         if self.baseJoiner.conditions.is_empty() {
//             while outer != outers.End() && numToAppend > 0 {
//                 self.onMatch(outer, chk);
//                 outerRowStatus.push(outerRowMatched);
//                 outer = outers.Next();
//                 numToAppend -= 1;
//             }
//             return Ok(outerRowStatus);
//         }
//         let evalCtx = self.baseJoiner.ctx.GetExprCtx().GetEvalCtx();
//         while outer != outers.End() && numToAppend > 0 {
//             self.baseJoiner.makeShallowJoinRow(false, inner, outer);
//             let (matched, isNull, err) = expression::EvalBool(evalCtx, &self.baseJoiner.conditions, self.baseJoiner.shallowRow.ToRow());
//             if let Some(err) = err {
//                 return Err(err);
//             }
//             if matched {
//                 self.onMatch(outer, chk);
//                 outerRowStatus.push(outerRowMatched);
//             } else if isNull {
//                 outerRowStatus.push(outerRowHasNull);
//             } else {
//                 outerRowStatus.push(outerRowUnmatched);
//             }
//             outer = outers.Next();
//             numToAppend -= 1;
//         }
//         outers.Error()?;
//         Ok(outerRowStatus)
//     }
//
//     fn OnMissMatch(&mut self, hasNull: bool, outer: chunk::Row, chk: &mut chunk::Chunk) {
//         let lWide = chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//         if hasNull {
//             chk.AppendNull(lWide);
//         } else {
//             chk.AppendInt64(lWide, 1);
//         }
//     }
//
//     fn isSemiJoinWithoutCondition(&self) -> bool {
//         self.baseJoiner.conditions.is_empty()
//     }
//
//     fn CloneJoiner(&self) -> Box<dyn Joiner> {
//         Box::new(antiLeftOuterSemiJoiner { baseJoiner: self.baseJoiner.Clone() })
//     }
// }
//
// pub struct leftOuterJoiner {
//     pub baseJoiner: baseJoiner,
// }
//
// impl Joiner for leftOuterJoiner {
// TryToMatchInners 对应 LeftOuterJoin：有 filter 时先写临时 chk，再批量过滤复制到输出。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         chk: &mut chunk::Chunk,
//         _opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error> {
//         if inners.Len() == 0 {
//             return Ok((false, false));
//         }
//         let has_filter = !self.baseJoiner.conditions.is_empty();
//         let mut lUsed = self.baseJoiner.lUsed.as_ref();
//         let mut rUsed = self.baseJoiner.rUsed.as_ref();
//         let mut lUsedForFilter = None;
//         let mut rUsedForFilter = None;
//         if has_filter {
//             self.baseJoiner.chk.as_mut().unwrap().Reset();
//             lUsed = None;
//             rUsed = None;
//             lUsedForFilter = self.baseJoiner.lUsed.as_ref();
//             rUsedForFilter = self.baseJoiner.rUsed.as_ref();
//         }
//
//         let mut numToAppend = chk.RequiredRows() - chk.NumRows();
//         while inners.Current() != inners.End() && numToAppend > 0 {
//             let target = if has_filter { self.baseJoiner.chk.as_mut().unwrap() } else { chk };
//             self.baseJoiner.makeJoinRowToChunk(target, outer, inners.Current(), lUsed, rUsed);
//             inners.Next();
//             numToAppend -= 1;
//         }
//         inners.Error()?;
//         if !has_filter {
//             return Ok((true, false));
//         }
//
//         let matched = self
//             .baseJoiner
//             .filter(self.baseJoiner.chk.as_mut().unwrap(), chk, outer.Len(), lUsedForFilter, rUsedForFilter)?;
//         Ok((matched, false))
//     }
//
//     fn TryToMatchOuters(
//         &mut self,
//         outers: &mut chunk::Iterator,
//         inner: chunk::Row,
//         chk: &mut chunk::Chunk,
//         mut outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
//         let has_filter = !self.baseJoiner.conditions.is_empty();
//         let mut lUsed = self.baseJoiner.lUsed.as_ref();
//         let mut rUsed = self.baseJoiner.rUsed.as_ref();
//         let mut lUsedForFilter = None;
//         let mut rUsedForFilter = None;
//         if has_filter {
//             self.baseJoiner.chk.as_mut().unwrap().Reset();
//             lUsed = None;
//             rUsed = None;
//             lUsedForFilter = self.baseJoiner.lUsed.as_ref();
//             rUsedForFilter = self.baseJoiner.rUsed.as_ref();
//         }
//
//         let mut outer = outers.Current();
//         let numToAppend = chk.RequiredRows() - chk.NumRows();
//         let mut cursor = 0;
//         while outer != outers.End() && cursor < numToAppend {
//             let target = if has_filter { self.baseJoiner.chk.as_mut().unwrap() } else { chk };
//             self.baseJoiner.makeJoinRowToChunk(target, outer, inner, lUsed, rUsed);
//             outer = outers.Next();
//             cursor += 1;
//         }
//         outers.Error()?;
//         outerRowStatus.clear();
//         for _ in 0..cursor {
//             outerRowStatus.push(outerRowMatched);
//         }
//         if !has_filter {
//             return Ok(outerRowStatus);
//         }
//         self.baseJoiner.filterAndCheckOuterRowStatus(
//             self.baseJoiner.chk.as_mut().unwrap(),
//             chk,
//             inner.Len(),
//             outerRowStatus,
//             lUsedForFilter,
//             rUsedForFilter,
//         )
//     }
//
//     fn OnMissMatch(&mut self, _hasNull: bool, outer: chunk::Row, chk: &mut chunk::Chunk) {
//         let lWide = chk.AppendRowByColIdxs(outer, self.baseJoiner.lUsed.as_ref());
//         chk.AppendPartialRowByColIdxs(lWide, self.baseJoiner.defaultInner, self.baseJoiner.rUsed.as_ref());
//     }
//
//     fn isSemiJoinWithoutCondition(&self) -> bool {
//         false
//     }
//
//     fn CloneJoiner(&self) -> Box<dyn Joiner> {
//         Box::new(leftOuterJoiner { baseJoiner: self.baseJoiner.Clone() })
//     }
// }
//
// pub struct rightOuterJoiner {
//     pub baseJoiner: baseJoiner,
// }
//
// impl Joiner for rightOuterJoiner {
// TryToMatchInners 对应 RightOuterJoin：拼接时 inner 放左、outer 放右。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         chk: &mut chunk::Chunk,
//         _opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error> {
//         if inners.Len() == 0 {
//             return Ok((false, false));
//         }
//         let has_filter = !self.baseJoiner.conditions.is_empty();
//         let mut lUsed = self.baseJoiner.lUsed.as_ref();
//         let mut rUsed = self.baseJoiner.rUsed.as_ref();
//         let mut lUsedForFilter = None;
//         let mut rUsedForFilter = None;
//         if has_filter {
//             self.baseJoiner.chk.as_mut().unwrap().Reset();
//             lUsed = None;
//             rUsed = None;
//             lUsedForFilter = self.baseJoiner.lUsed.as_ref();
//             rUsedForFilter = self.baseJoiner.rUsed.as_ref();
//         }
//
//         let mut numToAppend = chk.RequiredRows() - chk.NumRows();
//         while inners.Current() != inners.End() && numToAppend > 0 {
//             let target = if has_filter { self.baseJoiner.chk.as_mut().unwrap() } else { chk };
//             self.baseJoiner.makeJoinRowToChunk(target, inners.Current(), outer, lUsed, rUsed);
//             inners.Next();
//             numToAppend -= 1;
//         }
//         inners.Error()?;
//         if !has_filter {
//             return Ok((true, false));
//         }
//         let matched = self
//             .baseJoiner
//             .filter(self.baseJoiner.chk.as_mut().unwrap(), chk, outer.Len(), lUsedForFilter, rUsedForFilter)?;
//         Ok((matched, false))
//     }
//
//     fn TryToMatchOuters(
//         &mut self,
//         outers: &mut chunk::Iterator,
//         inner: chunk::Row,
//         chk: &mut chunk::Chunk,
//         mut outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
//         let has_filter = !self.baseJoiner.conditions.is_empty();
//         let mut lUsed = self.baseJoiner.lUsed.as_ref();
//         let mut rUsed = self.baseJoiner.rUsed.as_ref();
//         let mut lUsedForFilter = None;
//         let mut rUsedForFilter = None;
//         if has_filter {
//             self.baseJoiner.chk.as_mut().unwrap().Reset();
//             lUsed = None;
//             rUsed = None;
//             lUsedForFilter = self.baseJoiner.lUsed.as_ref();
//             rUsedForFilter = self.baseJoiner.rUsed.as_ref();
//         }
//
//         let mut outer = outers.Current();
//         let numToAppend = chk.RequiredRows() - chk.NumRows();
//         let mut cursor = 0;
//         while outer != outers.End() && cursor < numToAppend {
//             let target = if has_filter { self.baseJoiner.chk.as_mut().unwrap() } else { chk };
//             self.baseJoiner.makeJoinRowToChunk(target, inner, outer, lUsed, rUsed);
//             outer = outers.Next();
//             cursor += 1;
//         }
//         outerRowStatus.clear();
//         for _ in 0..cursor {
//             outerRowStatus.push(outerRowMatched);
//         }
//         if !has_filter {
//             return Ok(outerRowStatus);
//         }
//         self.baseJoiner.filterAndCheckOuterRowStatus(
//             self.baseJoiner.chk.as_mut().unwrap(),
//             chk,
//             inner.Len(),
//             outerRowStatus,
//             lUsedForFilter,
//             rUsedForFilter,
//         )
//     }
//
//     fn OnMissMatch(&mut self, _hasNull: bool, outer: chunk::Row, chk: &mut chunk::Chunk) {
//         let lWide = chk.AppendRowByColIdxs(self.baseJoiner.defaultInner, self.baseJoiner.lUsed.as_ref());
//         chk.AppendPartialRowByColIdxs(lWide, outer, self.baseJoiner.rUsed.as_ref());
//     }
//
//     fn isSemiJoinWithoutCondition(&self) -> bool {
//         false
//     }
//
//     fn CloneJoiner(&self) -> Box<dyn Joiner> {
//         Box::new(rightOuterJoiner { baseJoiner: self.baseJoiner.Clone() })
//     }
// }
//
// pub struct innerJoiner {
//     pub baseJoiner: baseJoiner,
// }
//
// impl Joiner for innerJoiner {
// TryToMatchInners 对应 InnerJoin：outerIsRight 决定左右拼接顺序。
//     fn TryToMatchInners(
//         &mut self,
//         outer: chunk::Row,
//         inners: &mut chunk::Iterator,
//         chk: &mut chunk::Chunk,
//         _opt: &[NAAJType],
//     ) -> Result<(bool, bool), errors::Error> {
//         if inners.Len() == 0 {
//             return Ok((false, false));
//         }
//         let has_filter = !self.baseJoiner.conditions.is_empty();
//         let mut lUsed = self.baseJoiner.lUsed.as_ref();
//         let mut rUsed = self.baseJoiner.rUsed.as_ref();
//         let mut lUsedForFilter = None;
//         let mut rUsedForFilter = None;
//         if has_filter {
//             self.baseJoiner.chk.as_mut().unwrap().Reset();
//             lUsed = None;
//             rUsed = None;
//             lUsedForFilter = self.baseJoiner.lUsed.as_ref();
//             rUsedForFilter = self.baseJoiner.rUsed.as_ref();
//         }
//
//         let mut inner = inners.Current();
//         let mut numToAppend = chk.RequiredRows() - chk.NumRows();
//         while inner != inners.End() && numToAppend > 0 {
//             let target = if has_filter { self.baseJoiner.chk.as_mut().unwrap() } else { chk };
//             if self.baseJoiner.outerIsRight {
//                 self.baseJoiner.makeJoinRowToChunk(target, inner, outer, lUsed, rUsed);
//             } else {
//                 self.baseJoiner.makeJoinRowToChunk(target, outer, inner, lUsed, rUsed);
//             }
//             inner = inners.Next();
//             numToAppend -= 1;
//         }
//         inners.Error()?;
//         if !has_filter {
//             return Ok((true, false));
//         }
//         let matched = self
//             .baseJoiner
//             .filter(self.baseJoiner.chk.as_mut().unwrap(), chk, outer.Len(), lUsedForFilter, rUsedForFilter)?;
//         Ok((matched, false))
//     }
//
//     fn TryToMatchOuters(
//         &mut self,
//         outers: &mut chunk::Iterator,
//         inner: chunk::Row,
//         chk: &mut chunk::Chunk,
//         mut outerRowStatus: Vec<outerRowStatusFlag>,
//     ) -> Result<Vec<outerRowStatusFlag>, errors::Error> {
//         let has_filter = !self.baseJoiner.conditions.is_empty();
//         let mut lUsed = self.baseJoiner.lUsed.as_ref();
//         let mut rUsed = self.baseJoiner.rUsed.as_ref();
//         let mut lUsedForFilter = None;
//         let mut rUsedForFilter = None;
//         if has_filter {
//             self.baseJoiner.chk.as_mut().unwrap().Reset();
//             lUsed = None;
//             rUsed = None;
//             lUsedForFilter = self.baseJoiner.lUsed.as_ref();
//             rUsedForFilter = self.baseJoiner.rUsed.as_ref();
//         }
//
//         let mut outer = outers.Current();
//         let numToAppend = chk.RequiredRows() - chk.NumRows();
//         let mut cursor = 0;
//         while outer != outers.End() && cursor < numToAppend {
//             let target = if has_filter { self.baseJoiner.chk.as_mut().unwrap() } else { chk };
//             if self.baseJoiner.outerIsRight {
//                 self.baseJoiner.makeJoinRowToChunk(target, inner, outer, lUsed, rUsed);
//             } else {
//                 self.baseJoiner.makeJoinRowToChunk(target, outer, inner, lUsed, rUsed);
//             }
//             outer = outers.Next();
//             cursor += 1;
//         }
//         outers.Error()?;
//         outerRowStatus.clear();
//         for _ in 0..cursor {
//             outerRowStatus.push(outerRowMatched);
//         }
//         if !has_filter {
//             return Ok(outerRowStatus);
//         }
//         self.baseJoiner.filterAndCheckOuterRowStatus(
//             self.baseJoiner.chk.as_mut().unwrap(),
//             chk,
//             inner.Len(),
//             outerRowStatus,
//             lUsedForFilter,
//             rUsedForFilter,
//         )
//     }
//
//     fn OnMissMatch(&mut self, _hasNull: bool, _outer: chunk::Row, _chk: &mut chunk::Chunk) {}
//
//     fn isSemiJoinWithoutCondition(&self) -> bool {
//         false
//     }
//
//     fn CloneJoiner(&self) -> Box<dyn Joiner> {
//         Box::new(innerJoiner { baseJoiner: self.baseJoiner.Clone() })
//     }
// }
// */
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 一行连接数据：各列为 `Value`。
pub type Row = Vec<Value>;
/// Other condition：对拼接行求值，`Some(true/false)` 或 `None`（SQL NULL）。
pub type Predicate = Arc<dyn Fn(&[Value]) -> Result<Option<bool>, String> + Send + Sync>;

/// SQL 连接类型（半连接 / 外连接 / 内连接等）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinType {
    Semi,
    AntiSemi,
    LeftOuterSemi,
    AntiLeftOuterSemi,
    LeftOuter,
    RightOuter,
    FullOuter,
    Inner,
}
/// 批量匹配外表时，每行相对当前内表行的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OuterRowStatus {
    Unmatched,
    Matched,
    HasNull,
}
/// Null-aware Anti Join 的键侧 NULL 组合细分（仅 NAAJ 路径使用）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NaajType {
    #[default]
    Unknown,
    LeftHasNullRightNotNull,
    LeftHasNullRightHasNull,
    LeftNotNullRightNotNull,
    LeftNotNullRightHasNull,
}

/// 按连接类型与条件生成结果行的执行器。
#[derive(Clone)]
pub struct Joiner {
    join_type: JoinType,
    null_aware: bool,
    outer_is_right: bool,
    default_inner: Row,
    conditions: Vec<Predicate>,
    left_used: Option<Vec<usize>>,
    right_used: Option<Vec<usize>>,
}

/// 单次 TryToMatchInners 的汇总：是否命中、是否见过 NULL、消耗了多少内表行。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchResult {
    pub matched: bool,
    pub has_null: bool,
    pub consumed: usize,
}

impl Joiner {
    /// 构造 Joiner；`max_chunk_size` 必须为正，null-aware 仅允许 Anti 系列。
    pub fn new(
        join_type: JoinType,
        outer_is_right: bool,
        default_inner: Row,
        conditions: Vec<Predicate>,
        children_used: Option<[Vec<usize>; 2]>,
        null_aware: bool,
        max_chunk_size: usize,
    ) -> Result<Self, String> {
        if max_chunk_size == 0 {
            return Err("max chunk size must be positive".into());
        }
        if null_aware && !matches!(join_type, JoinType::AntiSemi | JoinType::AntiLeftOuterSemi) {
            return Err("null-aware mode is only valid for anti joins".into());
        }
        let (left_used, right_used) = children_used
            .map(|used| (Some(used[0].clone()), Some(used[1].clone())))
            .unwrap_or((None, None));
        Ok(Self {
            join_type,
            null_aware,
            outer_is_right,
            default_inner,
            conditions,
            left_used,
            right_used,
        })
    }
    /// 返回当前连接类型。
    pub fn join_type(&self) -> JoinType {
        self.join_type
    }
    /// 半连接族且无 other condition 时，命中一条内表即可提前结束。
    pub fn is_semi_join_without_condition(&self) -> bool {
        matches!(
            self.join_type,
            JoinType::Semi
                | JoinType::AntiSemi
                | JoinType::LeftOuterSemi
                | JoinType::AntiLeftOuterSemi
        ) && self.conditions.is_empty()
    }

    /// 用一条外表行扫描一批内表行，按类型写入 `output`。
    ///
    /// Semi 命中后输出外表并耗尽内表迭代；AntiSemi 命中不写行；外半连接写带标记行；
    /// Inner/Outer 写拼接投影行。
    pub fn try_to_match_inners(
        &self,
        outer: &Row,
        inners: &[Row],
        output: &mut Vec<Row>,
        naaj: NaajType,
    ) -> Result<MatchResult, String> {
        if inners.is_empty() {
            return Ok(MatchResult::default());
        }
        let mut result = MatchResult::default();
        for inner in inners {
            result.consumed += 1;
            let joined = self.join_rows(inner, outer);
            let (matched, is_null) = self.evaluate(&joined)?;
            if !self.null_aware
                && matches!(
                    self.join_type,
                    JoinType::AntiSemi | JoinType::LeftOuterSemi | JoinType::AntiLeftOuterSemi
                )
            {
                result.has_null |= is_null;
            }
            if !matched {
                continue;
            }
            result.matched = true;
            match self.join_type {
                JoinType::Semi => output.push(self.project_outer(outer)),
                JoinType::AntiSemi => {}
                JoinType::LeftOuterSemi => output.push(self.with_marker(outer, Some(true))),
                JoinType::AntiLeftOuterSemi if self.null_aware => {
                    output.push(self.naaj_match_row(outer, naaj))
                }
                JoinType::AntiLeftOuterSemi => output.push(self.with_marker(outer, Some(false))),
                JoinType::LeftOuter
                | JoinType::RightOuter
                | JoinType::FullOuter
                | JoinType::Inner => output.push(self.project_joined(inner, outer)),
            }
            // 半连接族命中后无需继续扫内表。
            if matches!(
                self.join_type,
                JoinType::Semi
                    | JoinType::AntiSemi
                    | JoinType::LeftOuterSemi
                    | JoinType::AntiLeftOuterSemi
            ) {
                result.consumed = inners.len();
                break;
            }
        }
        Ok(result)
    }

    /// 用一条内表行扫描一批外表行，返回每行状态，并按类型写匹配输出。
    ///
    /// Null-aware Anti 系列在此路径暂不产出状态（与 Go TODO 形状一致）。
    pub fn try_to_match_outers(
        &self,
        outers: &[Row],
        inner: &Row,
        output: &mut Vec<Row>,
    ) -> Result<Vec<OuterRowStatus>, String> {
        if self.null_aware
            && matches!(
                self.join_type,
                JoinType::AntiSemi | JoinType::AntiLeftOuterSemi
            )
        {
            return Ok(Vec::new());
        }
        let mut statuses = Vec::new();
        for outer in outers {
            let joined = self.join_rows(inner, outer);
            let (matched, is_null) = self.evaluate(&joined)?;
            statuses.push(if matched {
                OuterRowStatus::Matched
            } else if is_null && !matches!(self.join_type, JoinType::Semi) {
                OuterRowStatus::HasNull
            } else {
                OuterRowStatus::Unmatched
            });
            if !matched {
                continue;
            }
            match self.join_type {
                JoinType::Semi => output.push(self.project_outer(outer)),
                JoinType::AntiSemi => {}
                JoinType::LeftOuterSemi => output.push(self.with_marker(outer, Some(true))),
                JoinType::AntiLeftOuterSemi => output.push(self.with_marker(outer, Some(false))),
                JoinType::LeftOuter
                | JoinType::RightOuter
                | JoinType::FullOuter
                | JoinType::Inner => output.push(self.project_joined(inner, outer)),
            }
        }
        Ok(statuses)
    }

    /// 外表未匹配时按类型补行：Anti 输出外表；外半连接写 false/NULL 标记；外连接拼默认内表行。
    pub fn on_miss_match(&self, has_null: bool, outer: &Row, output: &mut Vec<Row>) {
        match self.join_type {
            JoinType::Semi | JoinType::Inner => {}
            JoinType::AntiSemi => {
                if self.null_aware || !has_null {
                    output.push(self.project_outer(outer));
                }
            }
            JoinType::LeftOuterSemi => {
                output.push(self.with_marker(outer, if has_null { None } else { Some(false) }))
            }
            JoinType::AntiLeftOuterSemi => {
                output.push(self.with_marker(outer, if has_null { None } else { Some(true) }))
            }
            JoinType::LeftOuter | JoinType::RightOuter | JoinType::FullOuter => {
                let inner = if self.default_inner.is_empty() {
                    vec![Value::Null]
                } else {
                    self.default_inner.clone()
                };
                output.push(self.project_joined(&inner, outer));
            }
        }
    }

    /// 条件求值：全部 true 才匹配；false 支配 NULL，否则 NULL 累积到 has_null。
    fn evaluate(&self, row: &[Value]) -> Result<(bool, bool), String> {
        if self.conditions.is_empty() {
            return Ok((true, false));
        }
        let mut has_null = false;
        for condition in &self.conditions {
            match condition(row)? {
                Some(true) => {}
                Some(false) => return Ok((false, false)),
                None => has_null = true,
            }
        }
        Ok((!has_null, has_null))
    }
    /// 按 outer_is_right 决定左右顺序，拼接 inner/outer 供条件求值。
    fn join_rows(&self, inner: &Row, outer: &Row) -> Row {
        let mut row = Vec::with_capacity(inner.len() + outer.len());
        if self.outer_is_right {
            row.extend(inner.iter().cloned());
            row.extend(outer.iter().cloned());
        } else {
            row.extend(outer.iter().cloned());
            row.extend(inner.iter().cloned());
        }
        row
    }
    /// 按 inline projection 下标裁列；`None` 表示保留全部列。
    fn select(row: &Row, used: &Option<Vec<usize>>) -> Row {
        used.as_ref()
            .map(|indices| {
                indices
                    .iter()
                    .filter_map(|index| row.get(*index).cloned())
                    .collect()
            })
            .unwrap_or_else(|| row.clone())
    }
    /// 投影外表列（考虑 outer 在左还是右）。
    fn project_outer(&self, outer: &Row) -> Row {
        if self.outer_is_right {
            Self::select(outer, &self.right_used)
        } else {
            Self::select(outer, &self.left_used)
        }
    }
    /// 投影左右两侧并拼接为输出行。
    fn project_joined(&self, inner: &Row, outer: &Row) -> Row {
        let (left, right) = if self.outer_is_right {
            (inner, outer)
        } else {
            (outer, inner)
        };
        let mut row = Self::select(left, &self.left_used);
        row.extend(Self::select(right, &self.right_used));
        row
    }
    /// 在外表投影后追加匹配标记列（true/false/NULL）。
    fn with_marker(&self, outer: &Row, marker: Option<bool>) -> Row {
        let mut row = self.project_outer(outer);
        row.push(marker.map(Value::Bool).unwrap_or(Value::Null));
        row
    }
    /// Null-aware AntiLeftOuterSemi：仅双侧非 NULL 键匹配时标记 false，其余标记 NULL。
    fn naaj_match_row(&self, outer: &Row, kind: NaajType) -> Row {
        match kind {
            NaajType::LeftNotNullRightNotNull => self.with_marker(outer, Some(false)),
            NaajType::LeftNotNullRightHasNull
            | NaajType::LeftHasNullRightHasNull
            | NaajType::LeftHasNullRightNotNull
            | NaajType::Unknown => self.with_marker(outer, None),
        }
    }
}
