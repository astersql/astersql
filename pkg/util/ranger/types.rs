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

// 索引/表扫描范围（Range）的基础类型与区间运算，对齐 Go `pkg/util/ranger/types.go`。
//
// Range 表示物理计划构建阶段生成的扫描区间（多列 Datum 上下界 + 开闭区间）；
// Ranges 是区间列表，并实现 MutableRanges，以支持计划缓存（plan-cache）复用时重建。
// 提供子集判断、交集、编码为 key 边界、点范围判定与内存估算等能力。

// Range/Ranges 基础类型及区间运算，对齐 pkg/util/ranger/types.go。

#![allow(dead_code)]
#![allow(non_snake_case)]

use crate::{codec, collate, errctx, errors, types};

type GoError = errors::Error;

/// Object-safe context boundary for mutable range rebuilds. Plain `Ranges`
/// intentionally does not inspect it; planner-owned mutable implementations
/// can extend this contract when their formal module is connected.
/// 可变范围重建时所需的对象安全上下文边界；普通 `Ranges` 不读取它。
pub trait RangeRebuildContext: Send + Sync {}

impl<T> RangeRebuildContext for T where T: planctx::PlanContext + Send + Sync {}

/// 可变范围接口：计划缓存复用时，缓存计划中的范围可能需要按会话上下文重建。
// MutableRanges represents a range may change after it is created.
// It's mainly designed for plan-cache, since some ranges in a cached plan have to be rebuild when reusing.
// MutableRanges 对应 Go interface：缓存计划复用时，范围对象可能需要重新构造。
// 这里保留 Range/Rebuild/CloneForPlanCache 三个方法语义，动态分发细节留给后续模块接线确认。
pub trait MutableRanges {
    // Range returns the underlying range values.
    // 返回底层 Ranges；Go 里可能直接返回 nil slice，用 Ranges 占位结构表达。
    fn Range(&self) -> Ranges;

    // Rebuild rebuilds the underlying ranges again.
    // Rebuild 在普通 Ranges 上是空操作；其它实现可按 PlanContext 重建范围。
    fn Rebuild(&mut self, sctx: &dyn RangeRebuildContext) -> Result<(), GoError>;

    // CloneForPlanCache clones the MutableRanges for plan cache.
    // 计划缓存复制时需要深拷贝 Range 指针，避免复用期间修改共享对象。
    fn CloneForPlanCache(&self) -> Option<Box<dyn MutableRanges>>;
}

/// 范围数组，实现 MutableRanges；Go 的 nil slice 在 API 边界用 `Option<Ranges>` 表达。
// Ranges implements the MutableRanges interface for range array.
// Go nil slice is represented as `Option<Ranges>` at the Rust API boundary.
#[derive(Clone, Default)]
pub struct Ranges(pub Vec<Range>);

impl std::ops::Deref for Ranges {
    type Target = Vec<Range>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Ranges {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl IntoIterator for Ranges {
    type Item = Range;
    type IntoIter = std::vec::IntoIter<Range>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Ranges {
    type Item = &'a Range;
    type IntoIter = std::slice::Iter<'a, Range>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl Ranges {
    // Range returns the range array.
    // Range 返回自身底层数组；对应 Go 值接收者直接 return rs。
    pub fn Range(&self) -> Ranges {
        self.clone()
    }

    // Rebuild rebuilds this range.
    // 普通 Ranges 不依赖 PlanContext，Go 原实现固定返回 nil error。
    pub fn Rebuild(&mut self, _sctx: &dyn RangeRebuildContext) -> Result<(), GoError> {
        Ok(())
    }

    // CloneForPlanCache clones the MutableRanges for plan cache.
    // Go 代码先判断 nil slice，再逐个调用 Range.Clone；这里保留 nil slice 特判。
    pub fn CloneForPlanCache(&self) -> Option<Box<dyn MutableRanges>> {
        Some(Box::new(self.clone()))
    }

    // MemUsage gets the memory usage of ranges.
    // MemUsage 汇总每个非 nil Range 的内存估算；nil 元素在 Go 中若出现会 panic，这里仅保留注释说明。
    pub fn MemUsage(&self) -> i64 {
        let mut sum = 0_i64;
        for ran in &self.0 {
            sum += ran.MemUsage();
        }
        sum
    }

    // Subset checks if a list of ranges(rs) is a subset of another list of ranges(superRanges).
    // This is true if every range in the first list is a subset of any
    // range in the second list. Also, we check if all elements of superRanges are covered.
    // Ranges.Subset 先确认 rs 的每个范围都被某个 superRange 覆盖，再确认 superRanges 每项都被命中过。
    pub fn Subset(&self, tc: types::Context, superRanges: Ranges) -> bool {
        let mut subset = false;
        let mut superRangesCovered = vec![false; superRanges.len()];

        if self.is_empty() {
            return superRanges.is_empty();
        } else if superRanges.is_empty() {
            // unrestricted superRanges and restricted rs
            // Go 原语义把空 superRanges 视为不受限制，因此 restricted rs 属于它的子集。
            return true;
        }

        for subRange in &self.0 {
            subset = false;
            for (i, superRange) in superRanges.iter().enumerate() {
                if subRange.Subset(tc.clone(), superRange) {
                    subset = true;
                    superRangesCovered[i] = true;
                    break;
                }
            }
            if !subset {
                return false;
            }
        }
        for covered in superRangesCovered.iter() {
            if !*covered {
                return false;
            }
        }

        true
    }

    // IntersectRanges computes pairwise intersection between each element in rs and otherRangeList.
    // IntersectRanges 对两组范围做两两交集；比较器不匹配或比较出错时按 Go 原实现直接返回 nil。
    pub fn IntersectRanges(&self, tc: types::Context, otherRanges: Ranges) -> Option<Ranges> {
        let mut result = Ranges::default();
        for rsRange in &self.0 {
            for otherRange in &otherRanges.0 {
                let subsetLength = std::cmp::min(rsRange.LowVal.len(), otherRange.LowVal.len());
                if !checkCollators(rsRange, otherRange, subsetLength) {
                    return None;
                }
                let (oneIntersection, err) = rsRange.IntersectRange(tc.clone(), otherRange);
                if err.is_some() {
                    return None;
                }
                if let Some(oneIntersection) = oneIntersection {
                    result.push(oneIntersection);
                }
            }
        }
        Some(result)
    }
}

impl MutableRanges for Ranges {
    fn Range(&self) -> Ranges {
        Ranges::Range(self)
    }

    fn Rebuild(&mut self, sctx: &dyn RangeRebuildContext) -> Result<(), GoError> {
        Ranges::Rebuild(self, sctx)
    }

    fn CloneForPlanCache(&self) -> Option<Box<dyn MutableRanges>> {
        Ranges::CloneForPlanCache(self)
    }
}

/// 物理计划构建阶段生成的单个扫描范围；字段顺序对齐 Go struct。
// Range represents a range generated in physical plan building phase.
// Range 表示物理计划构建阶段生成的一个扫描范围；字段顺序保持 Go struct。
pub struct Range {
    /// 下界 Datum 列表（是否开区间由 LowExclude 决定）。
    pub LowVal: Vec<types::Datum>, // Low value is exclusive.
    /// 上界 Datum 列表（是否开区间由 HighExclude 决定）。
    pub HighVal: Vec<types::Datum>, // High value is exclusive.
    /// 各列比较所用的排序规则（collator）。
    pub Collators: Vec<Box<dyn collate::Collator>>,
    /// 为 true 时下界为开区间（不包含 LowVal）。
    pub LowExclude: bool,
    /// 为 true 时上界为开区间（不包含 HighVal）。
    pub HighExclude: bool,
}

impl Clone for Range {
    fn clone(&self) -> Self {
        self.Clone()
    }
}

impl Default for Range {
    fn default() -> Self {
        Self {
            LowVal: Vec::new(),
            HighVal: Vec::new(),
            Collators: Vec::new(),
            LowExclude: false,
            HighExclude: false,
        }
    }
}

impl Range {
    // Width returns the width of this range.
    // Width 返回低边界 datum 数量；Go 里直接 len(ran.LowVal)。
    pub fn Width(&self) -> usize {
        self.LowVal.len()
    }

    // Clone clones a Range.
    // Clone 深拷贝 LowVal/HighVal/Collators slice，保留 Go 中 nil 接收者返回 nil 的语义在调用侧用 Option 表达。
    pub fn Clone(&self) -> Range {
        let mut newRange = Range {
            LowVal: Vec::with_capacity(self.LowVal.len()),
            HighVal: Vec::with_capacity(self.HighVal.len()),
            LowExclude: self.LowExclude,
            HighExclude: self.HighExclude,
            ..Default::default()
        };
        for i in 0..self.LowVal.len() {
            newRange.LowVal.push(self.LowVal[i].clone());
        }
        for i in 0..self.HighVal.len() {
            newRange.HighVal.push(self.HighVal[i].clone());
        }
        newRange
            .Collators
            .extend(self.Collators.iter().map(|collator| collator.Clone()));
        newRange
    }

    // IsPoint returns if the range is a point.
    // IsPoint 使用 RangerContext 中的类型上下文和 RegardNULLAsPoint 开关判断点范围。
    pub fn IsPoint(&self, sctx: &rangerctx::RangerContext<'_>) -> bool {
        self.isPoint(sctx.TypeCtx.clone(), sctx.RegardNULLAsPoint)
    }

    // isPoint 对应 Go 的私有方法：逐列比较 LowVal/HighVal，并在 NULL 是否可视为点上按参数分支。
    fn isPoint(&self, tc: types::Context, regardNullAsPoint: bool) -> bool {
        if self.LowVal.len() != self.HighVal.len() {
            return false;
        }
        for i in 0..self.LowVal.len() {
            let a = self.LowVal[i].clone();
            let b = self.HighVal[i].clone();
            if a.Kind() == types::KindMinNotNull || b.Kind() == types::KindMaxValue {
                return false;
            }
            // Datum.Compare 可能因类型/排序规则比较失败；Go 原逻辑把错误当作“不是点范围”。
            let Ok(cmp) = a.Compare(tc.clone(), &b, self.Collators[i].as_ref()) else {
                return false;
            };
            if cmp != 0 {
                return false;
            }

            if a.IsNull() && b.IsNull() {
                // [NULL, NULL]
                // NULL 点范围是否成立由调用方开关决定，便于区分 nullable/non-nullable 场景。
                if !regardNullAsPoint {
                    return false;
                }
            }
        }
        !self.LowExclude && !self.HighExclude
    }

    // IsOnlyNull checks if the range has [NULL, NULL] or [NULL NULL, NULL NULL] range.
    // IsOnlyNull 要求每一列的左右边界都是 NULL。
    pub fn IsOnlyNull(&self) -> bool {
        for i in 0..self.LowVal.len() {
            let a = self.LowVal[i].clone();
            let b = self.HighVal[i].clone();
            if !(a.IsNull() && b.IsNull()) {
                return false;
            }
        }
        true
    }

    // IsPointNonNullable returns if the range is a point without NULL.
    // 非 nullable 场景下 NULL 不被当作点。
    pub fn IsPointNonNullable(&self, tc: types::Context) -> bool {
        self.isPoint(tc, false)
    }

    // IsPointNullable returns if the range is a point.
    // TODO: unify the parameter type with IsPointNullable and IsPoint
    // nullable 场景下 [NULL, NULL] 可按点范围处理。
    pub fn IsPointNullable(&self, tc: types::Context) -> bool {
        self.isPoint(tc, true)
    }

    // IsFullRange check if the range is full scan range
    // IsFullRange 判断范围是否覆盖完整扫描空间；unsigned int handle 有单独边界语义。
    pub fn IsFullRange(&self, unsignedIntHandle: bool) -> bool {
        if unsignedIntHandle {
            if self.LowVal.len() != 1 || self.HighVal.len() != 1 {
                return false;
            }
            return isBoundaryValue(self.LowVal[0].clone(), true, true)
                && isBoundaryValue(self.HighVal[0].clone(), true, false);
        }
        if self.LowVal.len() != self.HighVal.len() {
            return false;
        }
        for i in 0..self.LowVal.len() {
            let leftIsBoundary = isBoundaryValue(self.LowVal[i].clone(), false, true);
            let leftIsNull = self.LowVal[i].IsNull();
            let rightIsBoundary = isBoundaryValue(self.HighVal[i].clone(), false, false);
            let rightIsNull = self.HighVal[i].IsNull();
            // treat [NULL, +inf), (-inf, NULL] as full range
            // Go 特意把一侧 NULL 加另一侧无穷视为 full range，但双 NULL 不是 full range。
            if (!leftIsBoundary && !leftIsNull)
                || (!rightIsBoundary && !rightIsNull)
                || (leftIsNull && rightIsNull)
            {
                return false;
            }
        }
        true
    }

    // String implements the Stringer interface.
    // don't use it in the product.
    // String 仅用于调试展示，产品路径不应依赖该输出格式。
    pub fn String(&self) -> String {
        self.string(errors::RedactLogDisable)
    }

    // Redact is to print the range with redacting sensitive data.
    // Redact 按传入脱敏策略格式化范围边界。
    pub fn Redact(&self, redact: &str) -> String {
        self.string(redact)
    }

    // String implements the Stringer interface.
    // string 保持 Go 格式：左括号/低边界列表/逗号/高边界列表/右括号。
    fn string(&self, redact: &str) -> String {
        let mut lowStrs = Vec::with_capacity(self.LowVal.len());
        for d in self.LowVal.iter() {
            lowStrs.push(dealWithRedact(formatDatum(d.clone(), true), redact));
        }
        let mut highStrs = Vec::with_capacity(self.LowVal.len());
        for d in self.HighVal.iter() {
            highStrs.push(dealWithRedact(formatDatum(d.clone(), false), redact));
        }
        let mut l = "[";
        let mut r = "]";
        if self.LowExclude {
            l = "(";
        }
        if self.HighExclude {
            r = ")";
        }
        format!("{}{},{}{}", l, lowStrs.join(" "), highStrs.join(" "), r)
    }

    // Encode encodes the range to its encoded value.
    // Encode 把 LowVal/HighVal 编成 key 边界；该过程只做内存编码，不访问外部存储。
    pub fn Encode(
        &self,
        ec: errctx::Context,
        loc: &chrono_tz::Tz,
        mut lowBuffer: Vec<u8>,
        mut highBuffer: Vec<u8>,
    ) -> (Option<Vec<u8>>, Option<Vec<u8>>, Option<GoError>) {
        // Go 先用 lowBuffer[:0] 复用容量；用 clear 保留“复用缓冲区”的意图。
        lowBuffer.clear();
        lowBuffer = match codec::EncodeKey(*loc, lowBuffer, self.LowVal.clone()) {
            Ok(encoded) => encoded,
            Err(error) => match ec.HandleError(Some(error)) {
                Some(error) => return (None, None, Some(error.into())),
                None => Vec::new(),
            },
        };
        if self.LowExclude {
            // 排除低边界时使用 PrefixNext，把起点推进到该 key 的下一个前缀位置。
            lowBuffer = kv::Key(lowBuffer).PrefixNext().0;
        }

        highBuffer.clear();
        highBuffer = match codec::EncodeKey(*loc, highBuffer, self.HighVal.clone()) {
            Ok(encoded) => encoded,
            Err(error) => match ec.HandleError(Some(error)) {
                Some(error) => return (None, None, Some(error.into())),
                None => Vec::new(),
            },
        };
        if !self.HighExclude {
            // 包含高边界时也推进到 PrefixNext，形成 TiDB key range 的右开区间。
            highBuffer = kv::Key(highBuffer).PrefixNext().0;
        }
        (Some(lowBuffer), Some(highBuffer), None)
    }

    // Equal checks if two ranges are equal.
    // Equal 对应 Go 的 nil/指针相等/字段逐项比较顺序；other 用 Option 表示可能的 nil。
    pub fn Equal(&self, other: Option<&Range>) -> bool {
        let Some(other) = other else {
            return false;
        };
        if std::ptr::eq(self, other) {
            return true;
        }
        if self.LowExclude != other.LowExclude || self.HighExclude != other.HighExclude {
            return false;
        }
        if self.LowVal.len() != other.LowVal.len() || self.HighVal.len() != other.HighVal.len() {
            return false;
        }
        for i in 0..self.LowVal.len() {
            if !self.LowVal[i].Equals(&other.LowVal[i]) {
                return false;
            }
        }
        for i in 0..self.HighVal.len() {
            if !self.HighVal[i].Equals(&other.HighVal[i]) {
                return false;
            }
        }
        true
    }

    // PrefixEqualLen tells you how long the prefix of the range is a point.
    // e.g. If this range is (1 2 3, 1 2 +inf), then the return value is 2.
    // PrefixEqualLen 返回左右边界从前往后连续相等的列数；比较错误会立即返回错误。
    pub fn PrefixEqualLen(&self, tc: types::Context) -> (usize, Option<GoError>) {
        // Here, len(ran.LowVal) always equal to len(ran.HighVal)
        // 调用方保证 LowVal/HighVal 等长，这里按 Go 原注释不再重复检查。
        for i in 0..self.LowVal.len() {
            let cmp = match self.LowVal[i].Compare(
                tc.clone(),
                &self.HighVal[i],
                self.Collators[i].as_ref(),
            ) {
                Ok(cmp) => cmp,
                Err(error) => return (0, Some(errors::Trace(error))),
            };
            if cmp != 0 {
                return (i, None);
            }
        }
        (self.LowVal.len(), None)
    }

    // MemUsage gets the memory usage of range.
    // MemUsage 估算 Range 及其 Datum 内容大小；Collator 本体大小按 Go 原实现忽略。
    pub fn MemUsage(&self) -> i64 {
        let mut sum = EmptyRangeSize + (self.Collators.len() as i64) * 16;
        for val in self.LowVal.iter() {
            sum += val.MemUsage();
        }
        for val in self.HighVal.iter() {
            sum += val.MemUsage();
        }
        // We ignore size of collator currently.
        sum
    }

    // Subset for Range type, check if range(ran) is a subset of another range(otherRange).
    // This is done by:
    //   - Both ran and otherRange have the same collators. This is not needed for the current code path.
    //     But, it is used here for future use of the function.
    //   - Checking if the lower/upper bound of otherRange covers the corresponding lower/upper bound of ran.
    //     Thus include checking open/closed inetrvals.
    // Range.Subset 判断当前范围是否被 otherRange 覆盖，先检查宽度和 collator，再检查开闭区间兼容性。
    pub fn Subset(&self, tc: types::Context, otherRange: &Range) -> bool {
        if self.LowVal.len() < otherRange.LowVal.len() {
            return false;
        }

        if !checkCollators(self, otherRange, otherRange.LowVal.len()) {
            return false;
        }

        // Either otherRange is closed or both ranges have the same open/close setting.
        // super 范围闭合时可覆盖更多；若 super 是开区间，则子范围也必须同样开，保持 Go 逻辑。
        let lowExcludeOK = !otherRange.LowExclude || self.LowExclude == otherRange.LowExclude;
        let highExcludeOK = !otherRange.HighExclude || self.HighExclude == otherRange.HighExclude;
        if !lowExcludeOK || !highExcludeOK {
            return false;
        }

        prefix(
            tc.clone(),
            &otherRange.LowVal,
            &self.LowVal,
            otherRange.LowVal.len(),
            &self.Collators,
        ) && prefix(
            tc,
            &otherRange.HighVal,
            &self.HighVal,
            otherRange.LowVal.len(),
            &self.Collators,
        )
    }

    // IntersectRange computes intersection between two ranges. err is set of something went wrong
    // during comparison.
    // IntersectRange 计算两个范围的交集；无交集返回 None，比较失败返回错误占位。
    pub fn IntersectRange(
        &self,
        tc: types::Context,
        otherRange: &Range,
    ) -> (Option<Range>, Option<GoError>) {
        let intersectLength = std::cmp::max(self.LowVal.len(), otherRange.LowVal.len());
        let mut result = Range {
            LowVal: Vec::with_capacity(intersectLength),
            HighVal: Vec::with_capacity(intersectLength),
            Collators: Vec::with_capacity(intersectLength),
            ..Default::default()
        };

        let mut otherRangeMoreGranual = false;
        if self.LowVal.len() > otherRange.LowVal.len() {
            result.Collators = self.Collators.iter().map(|c| c.Clone()).collect();
        } else {
            result.Collators = otherRange.Collators.iter().map(|c| c.Clone()).collect();
            otherRangeMoreGranual = true;
        }

        let (mut lowVsHigh, err) = compareLexicographically(
            tc.clone(),
            &self.LowVal,
            &otherRange.HighVal,
            &result.Collators,
            self.LowExclude,
            otherRange.HighExclude,
            true,
            false,
        );
        if let Some(err) = err {
            return (Some(Range::default()), Some(err));
        }
        if lowVsHigh == 1 {
            // 当前低边界大于对方高边界时无交集。
            return (None, None);
        }

        let (nextLowVsHigh, err) = compareLexicographically(
            tc.clone(),
            &otherRange.LowVal,
            &self.HighVal,
            &result.Collators,
            otherRange.LowExclude,
            self.HighExclude,
            true,
            false,
        );
        lowVsHigh = nextLowVsHigh;
        if let Some(err) = err {
            return (Some(Range::default()), Some(err));
        }
        if lowVsHigh == 1 {
            return (None, None);
        }

        let (lowVsLow, err) = compareLexicographically(
            tc.clone(),
            &self.LowVal,
            &otherRange.LowVal,
            &result.Collators,
            self.LowExclude,
            otherRange.LowExclude,
            true,
            true,
        );
        if let Some(err) = err {
            return (Some(Range::default()), Some(err));
        }
        if lowVsLow == -1 || (lowVsLow == 0 && otherRangeMoreGranual) {
            result.LowVal = otherRange.LowVal.clone();
            result.LowExclude = otherRange.LowExclude;
        } else {
            result.LowVal = self.LowVal.clone();
            result.LowExclude = self.LowExclude;
        }

        let (highVsHigh, err) = compareLexicographically(
            tc,
            &self.HighVal,
            &otherRange.HighVal,
            &result.Collators,
            self.HighExclude,
            otherRange.HighExclude,
            false,
            false,
        );
        if let Some(err) = err {
            return (Some(Range::default()), Some(err));
        }
        if highVsHigh == 1 || (highVsHigh == 0 && otherRangeMoreGranual) {
            result.HighVal = otherRange.HighVal.clone();
            result.HighExclude = otherRange.HighExclude;
        } else {
            result.HighVal = self.HighVal.clone();
            result.HighExclude = self.HighExclude;
        }
        (Some(result), None)
    }
}

/// 判断范围列表中是否存在全表/全索引扫描的 full range。
// HasFullRange checks if any range in the slice is a full range.
// HasFullRange 扫描范围列表，只要存在 full range 就返回 true。
pub fn HasFullRange(ranges: &[Range], unsignedIntHandle: bool) -> bool {
    for ran in ranges.iter() {
        if ran.IsFullRange(unsignedIntHandle) {
            return true;
        }
    }
    false
}

// dealWithRedact 按 Go 的 errors.RedactLog* 策略处理单个 datum 字符串。
fn dealWithRedact(input: String, redact: &str) -> String {
    if input == "-inf" || input == "+inf" {
        return input;
    }
    if redact == errors::RedactLogDisable {
        return input;
    } else if redact == errors::RedactLogEnable {
        return "?".to_string();
    }
    format!("‹{}›", input)
}

/// 空 Range 结构体本身的字节大小，用于 MemUsage 估算。
// EmptyRangeSize is the size of empty range.
pub const EmptyRangeSize: i64 = std::mem::size_of::<Range>() as i64;

// isBoundaryValue 判断 datum 是否是某侧边界哨兵值；unsignedIntHandle 会把 uint64 低边界 0 视作起点。
fn isBoundaryValue(d: types::Datum, unsignedIntHandle: bool, isLeftSide: bool) -> bool {
    let isRightSide = !isLeftSide;
    match d.Kind() {
        types::KindMinNotNull => isLeftSide, // -inf
        types::KindMaxValue => isRightSide,  // +inf
        types::KindInt64 => {
            let v = d.GetInt64();
            (v == i64::MIN && isLeftSide) || // -inf
                (v == i64::MAX && isRightSide) // +inf
        }
        types::KindUint64 => {
            let v = d.GetUint64();
            (v == 0 && unsignedIntHandle && isLeftSide) || // 0
                (v == u64::MAX && isRightSide) // +inf
        }
        _ => false, // for other types, no concept of boundary value
    }
}

// formatDatum 把 Datum 转为 Range.String 使用的文本；左右边界会影响极值是否显示成 ±inf。
fn formatDatum(d: types::Datum, isLeftSide: bool) -> String {
    match d.Kind() {
        types::KindNull => "NULL".to_string(),
        types::KindMinNotNull => "-inf".to_string(),
        types::KindMaxValue => "+inf".to_string(),
        types::KindInt64 => {
            let v = d.GetInt64();
            match v {
                i64::MIN => {
                    if isLeftSide {
                        return "-inf".to_string();
                    }
                }
                i64::MAX => {
                    if !isLeftSide {
                        return "+inf".to_string();
                    }
                }
                _ => {}
            }
            v.to_string()
        }
        types::KindUint64 => {
            let v = d.GetUint64();
            if v == u64::MAX && !isLeftSide {
                return "+inf".to_string();
            }
            v.to_string()
        }
        // Go's `%v` prints primitive floats directly. Debug-printing the
        // Rust DatumValue enum would expose `Float32(...)` / `Float64(...)`.
        types::KindFloat32 => d.GetFloat32().to_string(),
        types::KindFloat64 => d.GetFloat64().to_string(),
        // Go formats both []byte and string with `%q`: the underlying text is
        // quoted and escaped, not the language-specific DatumValue wrapper.
        types::KindBytes => {
            let escaped = d
                .GetBytes()
                .into_iter()
                .flat_map(std::ascii::escape_default)
                .map(char::from)
                .collect::<String>();
            format!("\"{escaped}\"")
        }
        types::KindString => format!("{:?}", d.GetString()),
        types::KindMysqlEnum => format!("\"{}\"", d.GetMysqlEnum().String()),
        types::KindMysqlSet => format!("\"{}\"", d.GetMysqlSet().String()),
        types::KindMysqlJSON => format!("\"{}\"", d.GetMysqlJSON().String()),
        types::KindBinaryLiteral | types::KindMysqlBit => {
            format!("\"{}\"", d.GetBinaryLiteral().String())
        }
        _ => format!("{:?}", d.GetValue()),
    }
}

// extendBound extends a partial bound slice by appending "infinite" sentinel values.
// It's used when constructing multi-column index scan ranges.
// The logic depends on whether the bound is a lower or upper bound,
// and whether it's open (exclusive) or closed (inclusive):
//   - Lower Bound (`low == true`):
// - Open -> append +∞ (represented by MaxInt64): exclude current value, start just above
//   - Closed -> append –∞ (represented by MinInt64): include all lower values
//   - Upper Bound (`low == false`):
// - Open -> append –∞ (represented by MinInt64): exclude current value, stop just below
//   - Closed -> append +∞ (represented by MaxInt64): include all higher values
// This padding is essential in multi-column indexes when only a prefix of the columns
// is constrained. The remaining columns are filled with ±∞ to form complete range bounds.
// extendBound 给较短边界补齐无穷哨兵，确保多列索引范围比较时维度一致。
fn extendBound(
    mut bound: Vec<types::Datum>,
    lowIndex: usize,
    highIndex: usize,
    low: bool,
    open: bool,
) -> Vec<types::Datum> {
    for _i in lowIndex..highIndex {
        if low {
            if open {
                // Open lower bound -> +∞ (exclude the current value)
                // 开低边界补 +∞，表示排除当前前缀后从更大的后缀开始。
                bound.push(types::MaxValueDatum());
            } else {
                // Closed lower bound -> –∞ (include all lower values)
                // 闭低边界补 -∞，表示包含当前前缀下所有更小后缀。
                bound.push(types::MinNotNullDatum());
            }
        } else if open {
            // Open upper bound -> –∞ (exclude the current value)
            // 开高边界补 -∞，表示停在当前前缀之前。
            bound.push(types::MinNotNullDatum());
        } else {
            // Closed upper bound -> +∞ (include all higher values)
            // 闭高边界补 +∞，表示包含当前前缀下所有更大后缀。
            bound.push(types::MaxValueDatum());
        }
    }
    bound
}

// compareLexicographically compares two bounds from two ranges and returns 0, 1, -1
// for equal, greater than or less than respectively. It gets the two bounds,
// collations and if each bound is open (open1, open2) or closed. In addition,
// it also gets if each bound is lower or upper (low1, low2).
// Lower bounds logically can be extended with -infinity and upper bounds can be extended with +infinity.
// compareLexicographically 先补齐边界长度，再逐列按 collator 比较；值相等时用开闭区间和上下界身份打破平局。
fn compareLexicographically(
    tc: types::Context,
    bound1: &[types::Datum],
    bound2: &[types::Datum],
    collators: &[Box<dyn collate::Collator>],
    open1: bool,
    open2: bool,
    low1: bool,
    low2: bool,
) -> (i32, Option<GoError>) {
    let n1 = bound1.len();
    let n2 = bound2.len();
    let mut localBound1 = bound1.to_vec();
    let mut localBound2 = bound2.to_vec();

    if n1 < n2 {
        // Copy bound1 before extending
        // Go 里复制后再补齐，避免修改原 slice；同样 clone 后 append。
        localBound1 = extendBound(localBound1, n1, n2, low1, open1);
    } else if n2 < n1 {
        // Copy bound2 before extending
        localBound2 = extendBound(localBound2, n2, n1, low2, open2);
    }

    let n = std::cmp::max(n1, n2);
    for i in 0..n {
        let cmp = match localBound1[i].Compare(tc.clone(), &localBound2[i], collators[i].as_ref()) {
            Ok(cmp) => cmp,
            Err(error) => return (0, Some(error)),
        };
        if cmp != 0 {
            return (cmp, None);
        }
    }

    match (open1, open2) {
        (false, false) => (0, None),
        _ if open1 == open2 => {
            if low1 == low2 {
                (0, None)
            } else if low1 {
                (1, None)
            } else {
                (-1, None)
            }
        }
        (true, _) => {
            if low1 {
                (1, None)
            } else {
                (-1, None)
            }
        }
        _ => {
            // Same as case open2:
            // 只有第二个边界是开区间时，用它是低/高边界决定相对顺序。
            if low2 { (-1, None) } else { (1, None) }
        }
    }
}

// Check if a list of Datum is a prefix of another list of Datum. This is useful for checking if
// lower/upper bound of a range is a subset of another.
// prefix 判断 superValue 是否是 supValue 的前缀；比较错误或任一列不相等都返回 false。
fn prefix(
    tc: types::Context,
    superValue: &[types::Datum],
    supValue: &[types::Datum],
    length: usize,
    collators: &[Box<dyn collate::Collator>],
) -> bool {
    for i in 0..length {
        let Ok(cmp) = superValue[i].Compare(tc.clone(), &supValue[i], collators[i].as_ref()) else {
            return false;
        };
        if cmp != 0 {
            return false;
        }
    }
    true
}

// checkCollators 确保两个 Range 在指定前缀长度内使用相同 collator。
fn checkCollators(ran1: &Range, ran2: &Range, length: usize) -> bool {
    // Make sure both ran and superRange have the same collations.
    // The current code path for this function always will have same collation
    // for ran and superRange. It is added here for future
    // use of the function.
    for i in 0..length {
        if ran1.Collators[i].as_any().type_id() != ran2.Collators[i].as_any().type_id() {
            return false;
        }
    }
    true
}
