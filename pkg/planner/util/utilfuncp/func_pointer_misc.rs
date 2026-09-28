// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Cycle-breaking callback slots and plan-cache clone helpers.
//
// The Go package exposes package-level function variables because the concrete
// planner implementations would otherwise introduce import cycles. Rust has
// the same ownership problem while the planner crates are being split. The
// registry below keeps every Go injection point, but makes installation and
// lookup synchronized and reports missing or incorrectly typed callbacks.
//
// 打破循环依赖的回调槽位与 Plan Cache 克隆辅助。
//
// Go 侧用包级函数变量注入具体优化器实现，避免 planner 子包互相 import。
// Rust 迁移阶段保留全部注入点：经同步注册表安装/查找，并报告未安装或类型不匹配。
// Plan Cache（计划缓存）复用已优化的执行计划；克隆时仅复制会话不安全的表达式。

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::ops::Deref;
use std::sync::{Arc, OnceLock, RwLock};

use astersql_expression::{Column, Constant, ExprBox, ScalarFunction};

macro_rules! define_callback_slots {
    ($($name:ident),+ $(,)?) => {
        /// Stable identity of a callback exported by the Go package.
        /// 对应 Go 包导出的回调名称枚举，用作注册表键。
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum CallbackName {
            $($name),+
        }

        impl CallbackName {
            /// Returns the original Go variable name.
            /// 返回与 Go 包级变量同名的字符串。
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$name => stringify!($name)),+
                }
            }
        }

        /// All cycle-breaking callback injection points, in Go declaration order.
        /// 全部循环依赖打断注入点，顺序与 Go 声明一致。
        pub const ALL_CALLBACKS: &[CallbackName] = &[$(CallbackName::$name),+];

        $(
            #[doc = concat!("Callback slot corresponding to Go `", stringify!($name), "`.")]
            /// 与同名 Go 变量对应的静态回调槽。
            pub static $name: CallbackSlot = CallbackSlot::new(CallbackName::$name);
        )+
    };
}

// 下列名称覆盖逻辑/物理算子的选优、代价、附着 Task、ResolveIndices 等注入点。
define_callback_slots!(
    FindBestTask4BaseLogicalPlan,
    FindBestTask4LogicalDataSource,
    ExhaustPhysicalPlans4LogicalJoin,
    ExhaustPhysicalPlans4LogicalApply,
    DeriveStats4DataSource,
    DeriveStats4LogicalIndexScan,
    DeriveStats4LogicalTableScan,
    AddPrefix4ShardIndexes,
    GetEstimatedProbeCntFromProbeParents,
    GetActualProbeCntFromProbeParents,
    GetPlanCost,
    Attach2Task4PhysicalSort,
    GetCost4PhysicalSort,
    GetPlanCostVer14PhysicalSort,
    GetPlanCostVer24PhysicalSort,
    Attach2Task4NominalSort,
    Attach2Task4PhysicalUnionAll,
    GetPlanCostVer14PhysicalUnionAll,
    GetPlanCostVer24PhysicalUnionAll,
    GetPlanCostVer1PhysicalExchangeReceiver,
    GetPlanCostVer2PhysicalExchangeReceiver,
    ResolveIndices4PhysicalLimit,
    Attach2Task4PhysicalLimit,
    GetPlanCostVer14PhysicalTopN,
    GetPlanCostVer24PhysicalTopN,
    Attach2Task4PhysicalTopN,
    ResolveIndices4PhysicalTopN,
    Attach2Task4PhysicalExpand,
    ResolveIndices4PhysicalSelection,
    Attach2Task4PhysicalSelection,
    GetPlanCostVer14PhysicalSelection,
    GetPlanCostVer24PhysicalSelection,
    Attach2Task4PhysicalUnionScan,
    ResolveIndices4PhysicalUnionScan,
    ResolveIndices4PhysicalProjection,
    GetCost4PhysicalProjection,
    GetPlanCostVer14PhysicalProjection,
    GetPlanCostVer24PhysicalProjection,
    Attach2Task4PhysicalProjection,
    GetCost4PhysicalIndexJoin,
    GetPlanCostVer14PhysicalIndexJoin,
    GetIndexJoinCostVer24PhysicalIndexJoin,
    Attach2Task4PhysicalIndexJoin,
    Attach2Task4PhysicalWindow,
    GetCost4PhysicalMergeJoin,
    Attach2Task4PhysicalMergeJoin,
    GetPlanCostVer14PhysicalMergeJoin,
    GetPlanCostVer24PhysicalMergeJoin,
    GetPlanCostVer14PhysicalIndexScan,
    GetPlanCostVer24PhysicalIndexScan,
    GetPlanCostVer14PhysicalTableScan,
    GetPlanCostVer24PhysicalTableScan,
    GetPlanCostVer14PhysicalIndexReader,
    GetPlanCostVer24PhysicalIndexReader,
    GetCost4PhysicalHashJoin,
    GetPlanCostVer14PhysicalHashJoin,
    Attach2Task4PhysicalHashJoin,
    GetPlanCostVer14PhysicalIndexMergeReader,
    GetPlanCostVer24PhysicalIndexMergeReader,
    GetPlanCostVer24PhysicalHashJoin,
    GetCost4PhysicalIndexHashJoin,
    GetPlanCostVer1PhysicalIndexHashJoin,
    Attach2Task4PhysicalIndexHashJoin,
    GetCost4PhysicalHashAgg,
    GetPlanCostVer14PhysicalHashAgg,
    GetPlanCostVer24PhysicalHashAgg,
    Attach2Task4PhysicalHashAgg,
    GetCost4PhysicalStreamAgg,
    GetPlanCostVer14PhysicalStreamAgg,
    GetPlanCostVer24PhysicalStreamAgg,
    Attach2Task4PhysicalStreamAgg,
    Attach2Task4PhysicalApply,
    GetCost4PhysicalApply,
    GetPlanCostVer14PhysicalApply,
    GetPlanCostVer24PhysicalApply,
    GetCost4PhysicalIndexLookUpReader,
    GetPlanCostVer14PhysicalIndexLookUpReader,
    GetPlanCostVer24PhysicalIndexLookUpReader,
    ResolveIndices4PhysicalIndexLookUpReader,
    Attach2Task4PhysicalSequence,
    GetPlanCostVer24PhysicalCTE,
    Attach2Task4PhysicalCTEStorage,
    GetPlanCostVer24PhysicalTableReader,
    GetPlanCostVer14PhysicalTableReader,
    GetCost4PointGetPlan,
    GetPlanCostVer14PointGetPlan,
    GetPlanCostVer24PointGetPlan,
    GetCost4BatchPointGetPlan,
    GetPlanCostVer14BatchPointGetPlan,
    GetPlanCostVer24BatchPointGetPlan,
    GetCost4PhysicalIndexMergeJoin,
    GetPlanCostVer14PhysicalIndexMergeJoin,
    Attach2Task4PhysicalIndexMergeJoin,
    AttachPlan2Task,
    GetTaskPlanCost,
    CompareTaskCost,
    GetPossibleAccessPaths,
    DoOptimize,
);

/// 注册表中的单条回调：保存类型 ID、类型名与类型擦除后的值。
struct CallbackEntry {
    type_id: TypeId,
    type_name: &'static str,
    value: Box<dyn Any + Send + Sync>,
}

/// 全局回调注册表（读写锁保护），按需懒初始化。
fn callbacks() -> &'static RwLock<HashMap<CallbackName, CallbackEntry>> {
    static CALLBACKS: OnceLock<RwLock<HashMap<CallbackName, CallbackEntry>>> = OnceLock::new();
    CALLBACKS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// A named handle used to install and retrieve one concrete callback type.
/// 命名句柄：安装与取回某一具体类型的回调。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CallbackSlot {
    name: CallbackName,
}

impl CallbackSlot {
    /// 由回调名构造槽位（编译期常量）。
    const fn new(name: CallbackName) -> Self {
        Self { name }
    }

    /// Returns the matching Go package-level variable name.
    /// 返回对应的 Go 包级变量名枚举。
    pub const fn name(self) -> CallbackName {
        self.name
    }

    /// Installs a callback, returning the previous callback of the same type.
    ///
    /// Replacing a slot with a different concrete type is rejected so an
    /// initialization-order mistake cannot turn into an invalid invocation.
    /// 安装回调；若槽内已有不同类型则报 TypeMismatch，避免初始化顺序错误导致错误调用。
    pub fn install<T>(self, callback: T) -> Result<Option<T>, CallbackError>
    where
        T: Clone + Send + Sync + 'static,
    {
        let expected = TypeId::of::<T>();
        let mut entries = callbacks()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // 类型不一致时拒绝覆盖，防止后续 get 时强转失败。
        if let Some(previous) = entries.get(&self.name)
            && previous.type_id != expected
        {
            return Err(CallbackError::TypeMismatch {
                name: self.name,
                expected: type_name::<T>(),
                actual: previous.type_name,
            });
        }

        let previous = entries.insert(
            self.name,
            CallbackEntry {
                type_id: expected,
                type_name: type_name::<T>(),
                value: Box::new(callback),
            },
        );
        Ok(previous.and_then(|entry| entry.value.downcast::<T>().ok().map(|value| *value)))
    }

    /// Gets a clone of the installed callback.
    /// 克隆取出已安装的回调；未安装或类型不符时返回错误。
    pub fn get<T>(self) -> Result<T, CallbackError>
    where
        T: Clone + Send + Sync + 'static,
    {
        let entries = callbacks()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries
            .get(&self.name)
            .ok_or(CallbackError::NotInstalled { name: self.name })?;
        entry
            .value
            .downcast_ref::<T>()
            .cloned()
            .ok_or(CallbackError::TypeMismatch {
                name: self.name,
                expected: type_name::<T>(),
                actual: entry.type_name,
            })
    }

    /// Removes the callback currently installed in this slot.
    /// 移除本槽当前回调，返回是否原先已安装。
    pub fn remove(self) -> bool {
        callbacks()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.name)
            .is_some()
    }

    /// Returns whether this callback has been installed.
    /// 本槽是否已安装回调。
    pub fn is_installed(self) -> bool {
        callbacks()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&self.name)
    }
}

/// Failure to resolve or consistently type a callback slot.
/// 解析回调失败：未安装，或请求类型与已安装类型不一致。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallbackError {
    /// 该名称尚未 install。
    NotInstalled { name: CallbackName },
    /// 已安装类型与请求的泛型参数不符。
    TypeMismatch {
        name: CallbackName,
        expected: &'static str,
        actual: &'static str,
    },
}

impl Display for CallbackError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled { name } => {
                write!(formatter, "callback {} is not installed", name.as_str())
            }
            Self::TypeMismatch {
                name,
                expected,
                actual,
            } => write!(
                formatter,
                "callback {} has type {actual}, requested {expected}",
                name.as_str()
            ),
        }
    }
}

impl Error for CallbackError {}

/// Removes all registered callbacks. Intended for orderly planner teardown and tests.
/// 清空全部已注册回调，供优化器有序拆卸与测试隔离。
pub fn ClearCallbacks() {
    callbacks()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// Result of a plan-cache clone operation.
///
/// Go can return the original slice when every value is safe. This enum keeps
/// that zero-copy behavior explicit instead of always allocating a new vector.
/// Plan Cache 克隆结果：全部会话安全时可 Shared 零拷贝复用原切片，否则 Cloned。
#[derive(Debug)]
pub enum PlanCacheSlice<'a, T> {
    /// 全部元素可跨会话共享，引用原切片。
    Shared(&'a [T]),
    /// 存在不安全元素，已分配新向量。
    Cloned(Vec<T>),
}

impl<T> AsRef<[T]> for PlanCacheSlice<'_, T> {
    fn as_ref(&self) -> &[T] {
        self
    }
}

impl<T> Deref for PlanCacheSlice<'_, T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Shared(values) => values,
            Self::Cloned(values) => values,
        }
    }
}

fn reuse_or_new_vec<T>(cloned: Option<Vec<T>>, len: usize) -> Vec<T> {
    // 复用调用方传入的缓冲：清空后按需扩容，减少 Plan Cache 热路径分配。
    let mut cloned = cloned.unwrap_or_else(|| Vec::with_capacity(len));
    cloned.clear();
    if cloned.capacity() < len {
        cloned.reserve(len - cloned.capacity());
    }
    cloned
}

/// Clones only session-unsafe expressions and reuses `cloned` when supplied.
/// Corresponds to Go `CloneExpressionsForPlanCache`.
/// 仅克隆会话不安全的表达式；全部安全则 Shared。对应 Go `CloneExpressionsForPlanCache`。
pub fn CloneExpressionsForPlanCache<'a>(
    exprs: Option<&'a [ExprBox]>,
    cloned: Option<Vec<ExprBox>>,
) -> Option<PlanCacheSlice<'a, ExprBox>> {
    let exprs = exprs?;
    let mut all_safe = true;
    for e in exprs {
        if !e.SafeToShareAcrossSession() {
            all_safe = false;
            break;
        }
    }
    if all_safe {
        return Some(PlanCacheSlice::Shared(exprs));
    }
    let mut cloned = reuse_or_new_vec(cloned, exprs.len());
    for e in exprs {
        // Go keeps the same interface value when safe; ExprBox ownership requires a clone
        // into the new vector either way (Shared path above preserves zero-copy).
        // Go 在安全时可保留同一接口值；此处进入克隆路径后统一 CloneExpr。
        cloned.push(e.CloneExpr());
    }
    Some(PlanCacheSlice::Cloned(cloned))
}

/// Clones only session-unsafe columns while preserving null entries.
/// Corresponds to Go `CloneColumnsForPlanCache`.
/// 克隆列切片并保留 null 槽位。对应 Go `CloneColumnsForPlanCache`。
pub fn CloneColumnsForPlanCache<'a>(
    cols: Option<&'a [Option<Arc<Column>>]>,
    cloned: Option<Vec<Option<Arc<Column>>>>,
) -> Option<PlanCacheSlice<'a, Option<Arc<Column>>>> {
    let cols = cols?;
    // Go does not skip nil in the allSafe scan (nil would panic there).
    // Go 扫描 allSafe 时不跳过 nil（会 panic）；此处同样对 None 直接 unwrap。
    let mut all_safe = true;
    for c in cols {
        if !c.as_ref().unwrap().SafeToShareAcrossSession() {
            all_safe = false;
            break;
        }
    }
    if all_safe {
        return Some(PlanCacheSlice::Shared(cols));
    }
    let mut cloned = reuse_or_new_vec(cloned, cols.len());
    for c in cols {
        match c {
            None => cloned.push(None),
            Some(c) if c.SafeToShareAcrossSession() => cloned.push(Some(Arc::clone(c))),
            Some(c) => cloned.push(Some(Arc::new(c.Clone()))),
        }
    }
    Some(PlanCacheSlice::Cloned(cloned))
}

/// Clones only session-unsafe constants while preserving null entries.
/// Corresponds to Go `CloneConstantsForPlanCache` (issue #66265 nil-entry path).
/// 克隆常量并保留中间 nil（issue #66265）。对应 Go `CloneConstantsForPlanCache`。
pub fn CloneConstantsForPlanCache<'a>(
    constants: Option<&'a [Option<Arc<Constant>>]>,
    cloned: Option<Vec<Option<Arc<Constant>>>>,
) -> Option<PlanCacheSlice<'a, Option<Arc<Constant>>>> {
    let constants = constants?;
    let mut all_safe = true;
    for c in constants {
        // 扫描阶段跳过 None，避免对空槽 unwrap。
        let Some(c) = c else {
            continue;
        };
        if !c.SafeToShareAcrossSession() {
            all_safe = false;
            break;
        }
    }
    if all_safe {
        return Some(PlanCacheSlice::Shared(constants));
    }
    let mut cloned = reuse_or_new_vec(cloned, constants.len());
    for c in constants {
        match c {
            None => cloned.push(None),
            Some(c) if c.SafeToShareAcrossSession() => cloned.push(Some(Arc::clone(c))),
            Some(c) => cloned.push(Some(Arc::new(c.Clone()))),
        }
    }
    Some(PlanCacheSlice::Cloned(cloned))
}

/// Clones only session-unsafe scalar functions and reuses `cloned` when supplied.
/// Corresponds to Go `CloneScalarFunctionsForPlanCache`.
/// 克隆标量函数切片。对应 Go `CloneScalarFunctionsForPlanCache`。
pub fn CloneScalarFunctionsForPlanCache<'a>(
    scalar_funcs: Option<&'a [Arc<ScalarFunction>]>,
    cloned: Option<Vec<Arc<ScalarFunction>>>,
) -> Option<PlanCacheSlice<'a, Arc<ScalarFunction>>> {
    let scalar_funcs = scalar_funcs?;
    let mut all_safe = true;
    for f in scalar_funcs {
        if !f.SafeToShareAcrossSession() {
            all_safe = false;
            break;
        }
    }
    if all_safe {
        return Some(PlanCacheSlice::Shared(scalar_funcs));
    }
    let mut cloned = reuse_or_new_vec(cloned, scalar_funcs.len());
    for f in scalar_funcs {
        if f.SafeToShareAcrossSession() {
            cloned.push(Arc::clone(f));
        } else {
            cloned.push(Arc::new(f.clone_scalar()));
        }
    }
    Some(PlanCacheSlice::Cloned(cloned))
}

/// Clones each row of a two-dimensional expression slice.
/// Corresponds to Go `CloneExpression2DForPlanCache`.
/// 对二维表达式切片逐行调用 `CloneExpressionsForPlanCache`。
pub fn CloneExpression2DForPlanCache<'a>(
    exprs: Option<&'a [Option<Vec<ExprBox>>]>,
) -> Option<Vec<Option<PlanCacheSlice<'a, ExprBox>>>> {
    exprs.map(|rows| {
        rows.iter()
            .map(|row| CloneExpressionsForPlanCache(row.as_deref(), None))
            .collect()
    })
}
