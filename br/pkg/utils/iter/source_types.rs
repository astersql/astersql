// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 迭代器数据源适配：切片、区间、空流、失败流与闭包生成器。
//! 对应 Go `iter` 包中的基础 Source；统一实现 `TryNextor` 供组合算子消费。
//! 工厂函数 `new_*` 仅 crate 内可见，对外通过上层 API 构造具体源。

use std::collections::VecDeque;

use crate::iter::{Context, Done, Emit, IterResult, Throw, TryNextor};

/// 基于 `VecDeque` 的有限序列源；按 FIFO 弹出直至耗尽。
pub struct SliceIter<T> {
    items: VecDeque<T>,
}

impl<T: Send> TryNextor<T> for SliceIter<T> {
    fn TryNext(&mut self, _ctx: &Context) -> IterResult<T> {
        // 空队列返回 Done，与 Go slice 迭代器耗尽语义一致。
        match self.items.pop_front() {
            Some(item) => Emit(item),
            None => Done(),
        }
    }
}

/// 数值闭区间/半开区间迭代器；`endExclusive` 控制是否包含上界。
pub struct OfRangeIter<T> {
    pub end: T,
    /// true 时为 `[current, end)`，false 时为 `[current, end]`。
    pub endExclusive: bool,
    pub current: T,
}

/// 为整型实现步进 +1；越界或触及排他上界时 Done。
macro_rules! impl_range {
    ($t:ty) => {
        impl TryNextor<$t> for OfRangeIter<$t> {
            fn TryNext(&mut self, _ctx: &Context) -> IterResult<$t> {
                // 先判终止再 Emit，保证不会越过 end。
                if self.current > self.end || (self.current == self.end && self.endExclusive) {
                    return Done();
                }
                let result = Emit(self.current);
                self.current += 1;
                result
            }
        }
    };
}

impl_range!(i32);
impl_range!(i64);
impl_range!(u32);
impl_range!(u64);
impl_range!(usize);
impl_range!(isize);

/// 永不产出元素的空源；用于组合算子边界与短路场景。
pub struct EmptyIter<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T: Send> TryNextor<T> for EmptyIter<T> {
    fn TryNext(&mut self, _ctx: &Context) -> IterResult<T> {
        Done()
    }
}

/// 装箱空源，便于作为 `dyn TryNextor` 传入管道。
pub fn empty<T: Send + 'static>() -> Box<dyn TryNextor<T>> {
    Box::new(EmptyIter {
        _marker: std::marker::PhantomData,
    })
}

/// 每次 `TryNext` 都抛出固定错误；用于测试或显式失败注入。
pub struct FailureIter<T> {
    error: String,
    _marker: std::marker::PhantomData<T>,
}

impl<T: Send> TryNextor<T> for FailureIter<T> {
    fn TryNext(&mut self, _ctx: &Context) -> IterResult<T> {
        // 错误字符串按次克隆，对齐 Go 侧每次返回同一 message 的行为。
        Throw(self.error.clone())
    }
}

/// 闭包驱动的惰性源；由调用方决定 Emit/Done/Throw。
pub struct FuncIter<T> {
    generator: Box<dyn FnMut(&Context) -> IterResult<T> + Send>,
}

impl<T: Send> TryNextor<T> for FuncIter<T> {
    fn TryNext(&mut self, ctx: &Context) -> IterResult<T> {
        // 透传 Context，便于闭包内检查取消。
        (self.generator)(ctx)
    }
}

/// 构造失败源；仅 crate 内部工厂使用。
pub(crate) fn new_failure<T: Send + 'static>(error: String) -> FailureIter<T> {
    FailureIter {
        error,
        _marker: std::marker::PhantomData,
    }
}

/// 将可变闭包装箱为 `FuncIter`。
pub(crate) fn new_func<T: Send + 'static>(
    g: impl FnMut(&Context) -> IterResult<T> + Send + 'static,
) -> FuncIter<T> {
    FuncIter {
        generator: Box::new(g),
    }
}

/// 由 `Vec` 构建切片源；内部转为双端队列以 O(1) 弹出头部。
pub(crate) fn new_slice<T: Send + 'static>(s: Vec<T>) -> SliceIter<T> {
    SliceIter {
        items: VecDeque::from(s),
    }
}

/// 默认半开区间 `[begin, end)`，与 Go `OfRange` 默认排他上界一致。
pub(crate) fn new_range<T>(begin: T, end: T) -> OfRangeIter<T> {
    OfRangeIter {
        end,
        endExclusive: true,
        current: begin,
    }
}
