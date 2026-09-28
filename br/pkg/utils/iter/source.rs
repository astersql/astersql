// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 迭代源工厂：从切片/区间/失败/闭包构造 `TryNextor`。
//! 对应 Go `br/pkg/utils/iter` 的 Source 辅助，供组合子测试与流水线使用。

use crate::iter::{Context, Done, Emit, IterResult, TryNextor};
use crate::source_types::{OfRangeIter, SliceIter, new_failure, new_func, new_range, new_slice};

// Go's constraints.Integer also includes these fixed-width integer types. Keep
// their state machine identical to the implementations in `source_types.rs`.
macro_rules! impl_missing_go_integer_range {
    ($($t:ty),+ $(,)?) => {
        $(
            impl TryNextor<$t> for OfRangeIter<$t> {
                fn TryNext(&mut self, _ctx: &Context) -> IterResult<$t> {
                    if self.current > self.end
                        || (self.current == self.end && self.endExclusive)
                    {
                        return Done();
                    }
                    let result = Emit(self.current);
                    self.current += 1;
                    result
                }
            }
        )+
    };
}

impl_missing_go_integer_range!(i8, i16, u8, u16);

/// 以拥有的切片为有限源；耗尽后结束。
pub fn FromSlice<T: Send + 'static>(s: Vec<T>) -> Box<dyn TryNextor<T>> {
    Box::new(new_slice(s))
}

/// 半开区间 `[begin, end)` 源；元素类型需支持 OfRangeIter。
pub fn OfRange<T>(begin: T, end: T) -> Box<dyn TryNextor<T>>
where
    OfRangeIter<T>: TryNextor<T> + 'static,
    T: Send + 'static,
{
    Box::new(new_range(begin, end))
}

/// 立即失败的源，用于错误路径单测。
pub fn Fail<T: Send + 'static>(err: impl Into<String>) -> Box<dyn TryNextor<T>> {
    Box::new(new_failure(err.into()))
}

/// 由闭包驱动的惰性源；每次 Next 调用 `g`。
pub fn Func<T: Send + 'static>(
    g: impl FnMut(&Context) -> IterResult<T> + Send + 'static,
) -> Box<dyn TryNextor<T>> {
    Box::new(new_func(g))
}

// Re-export concrete types for tests that need downcasting-free construction.
/// 具体切片源类型别名，便于测试避免 dyn 转型。
pub type SliceSource<T> = SliceIter<T>;
