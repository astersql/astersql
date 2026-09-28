// Copyright 2022 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 组合测试（fork / Pick）驱动。
//
// 对应 Go `testfork`：用 [`pickStack`] 枚举多层 [`Pick`] / [`PickEnum`] 取值组合，
// [`RunTest`] 反复运行子测试，失败时打印当前选中值以便定位。

use std::any::Any;
use std::error::Error;
use std::fmt::{self, Debug};

/// A picked value together with the text Go's `fmt.Sprintf("%v")` would
/// contribute to the failed-values diagnostic.
///
/// 类型擦除后的候选值，并携带失败诊断用的文本（对齐 Go `fmt.Sprintf("%v")`）。
pub struct AnyValue {
    /// 运行时类型擦除后的实际取值。
    value: Box<dyn Any>,
    /// 用于失败日志的展示文本。
    text: String,
}

impl AnyValue {
    /// 由可 Debug 的值构造；字符串类型额外加双引号以匹配 Go 诊断格式。
    pub fn new<E>(value: E) -> Self
    where
        E: Debug + 'static,
    {
        // 字符串单独加引号，其它类型走 Debug 格式。
        let text = if let Some(value) = (&value as &dyn Any).downcast_ref::<String>() {
            format!("\"{value}\"")
        } else if let Some(value) = (&value as &dyn Any).downcast_ref::<&str>() {
            format!("\"{value}\"")
        } else {
            format!("{value:?}")
        };
        Self {
            value: Box::new(value),
            text,
        }
    }

    /// 向下转型为原始泛型类型引用。
    fn downcast_ref<E: 'static>(&self) -> Option<&E> {
        self.value.downcast_ref::<E>()
    }
}

impl Debug for AnyValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

/// [`pickStack::PickValue`] 可能返回的错误。
#[derive(Debug, PartialEq, Eq)]
pub enum PickError {
    /// 候选值列表为空。
    EmptyValues,
    /// `pos` 超过当前栈深度，状态非法。
    IllegalState { pos: usize, stack_len: usize },
}

impl fmt::Display for PickError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValues => formatter.write_str("values should not be empty"),
            Self::IllegalState { pos, stack_len } => {
                write!(formatter, "illegal state {pos} > {stack_len}")
            }
        }
    }
}

impl Error for PickError {}

// pickStack 对应 Go 的同名结构体：stack 保存每一层可选值，pos 表示当前 Pick 深度。
/// 多层 Pick 的选择栈：每层一组候选值，`pos` 为当前深度，`valid` 表示是否还有组合。
pub struct pickStack {
    /// 各层剩余候选值；每层 `[0]` 为当前选中值。
    pub stack: Vec<Vec<AnyValue>>,
    /// 当前 Pick 深度（进入下一层前递增）。
    pub pos: usize,
    /// 是否仍有未枚举完的组合。
    pub valid: bool,
}

// newPickStack 对应 Go 构造函数；初始状态 valid=true，驱动 RunTest 至少跑一轮。
/// 构造空的选择栈；`valid=true` 使 [`RunTest`] 至少执行一轮。
pub fn newPickStack() -> Box<pickStack> {
    Box::new(pickStack {
        stack: Vec::new(),
        pos: 0,
        valid: true,
    })
}

impl pickStack {
    // NextStack 对应 Go 的进位逻辑：从最后一个 Pick 层开始丢弃已使用值，空层继续向前进位。
    /// 进位到下一组组合：消费最深层当前值，空层弹出，并重置 `pos`。
    pub fn NextStack(&mut self) {
        while !self.stack.is_empty() {
            let lastIndex = self.stack.len() - 1;
            // Go 使用切片 s.stack[lastIndex][1:]；这里用 remove(0) 表达“消费当前候选”。
            self.stack[lastIndex].remove(0);
            if !self.stack[lastIndex].is_empty() {
                break;
            }
            self.stack.truncate(lastIndex);
        }

        self.pos = 0;
        self.valid = !self.stack.is_empty();
    }

    // PickValue 对应 Go 的取值方法：首次进入某层时压入候选值，随后返回当前层第一个元素。
    /// 在当前深度取值：首次进入时压入候选列表，返回该层当前选中元素。
    pub fn PickValue(&mut self, values: Vec<AnyValue>) -> Result<&AnyValue, PickError> {
        if values.is_empty() {
            return Err(PickError::EmptyValues);
        }

        let stackLen = self.stack.len();
        if self.pos > stackLen {
            return Err(PickError::IllegalState {
                pos: self.pos,
                stack_len: stackLen,
            });
        }

        let currentPos = self.pos;
        // Go 通过 defer 在返回前递增 pos；显式保存当前层后再递增。
        self.pos += 1;

        if currentPos == stackLen {
            self.stack.push(values);
        }
        Ok(&self.stack[currentPos][0])
    }

    // Values 对应 Go 的 Values：取出每一层当前选中的第一个值，用于失败日志。
    /// 收集每一层当前选中值，供失败诊断使用。
    pub fn Values(&self) -> Vec<&AnyValue> {
        let mut values = Vec::new();
        for v in &self.stack {
            values.push(&v[0]);
        }
        values
    }

    // ValuesText 对应 Go 的字符串化逻辑；字符串值额外补双引号以匹配 fmt.Sprintf(`"%s"`).
    /// 将当前选中值格式化为 `[v1 v2 ...]` 文本。
    pub fn ValuesText(&self) -> String {
        let values = self.Values();
        let mut strValues = Vec::with_capacity(values.len());
        for value in values {
            strValues.push(value.text.clone());
        }
        format!("[{}]", strValues.join(" "))
    }

    // Valid 对应 Go 的状态查询；RunTest 用它决定是否继续枚举组合。
    /// 是否仍有未枚举组合（[`RunTest`] 循环条件）。
    pub fn Valid(&self) -> bool {
        self.valid
    }
}

// T is used by for test
// T 对应 Go 中嵌入 *testing.T 的测试上下文；stack 让 Pick 系列函数共享同一轮选择状态。
/// 单轮组合测试上下文：持有可变的 [`pickStack`] 供 Pick 系列函数共享。
pub struct T<'a> {
    /// 本轮选择栈。
    pub stack: &'a mut pickStack,
}

// RunTest runs the test function `f` multiple times util all the values in `Pick` are tested.
// RunTest 对应 Go 的组合测试驱动：每轮运行子测试，失败时把当前 Pick 值打印到 stderr。
/// 枚举所有 Pick 组合并反复运行 `f`；捕获 panic，首个失败在全部组合跑完后重新抛出。
pub fn RunTest<F>(mut f: F)
where
    F: FnMut(&mut T),
{
    let mut idx = 0;
    let mut first_failure = None;
    let mut stack = newPickStack();
    while stack.Valid() {
        // 捕获子测试 panic，记录首个失败以便枚举完后再 resume_unwind。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut wrapped = T { stack: &mut stack };
            f(&mut wrapped);
        }));
        if let Err(failure) = result {
            eprintln!(
                "SubTest #{idx} failed, failed values: {}",
                stack.ValuesText()
            );
            if first_failure.is_none() {
                first_failure = Some(failure);
            }
        }
        idx += 1;
        stack.NextStack();
    }
    if let Some(failure) = first_failure {
        std::panic::resume_unwind(failure);
    }
}

// Pick returns a value from the values list
// Pick 对应 Go 的泛型函数：把强类型切片转为 any 切片，再从当前 pickStack 层取回原类型。
/// 从候选列表中按当前栈状态取一个值（强类型进出，内部经 [`AnyValue`] 擦除）。
pub fn Pick<E>(t: &mut T, values: Vec<E>) -> E
where
    E: Clone + Debug + 'static,
{
    let mut slice: Vec<AnyValue> = Vec::with_capacity(values.len());
    for item in values {
        slice.push(AnyValue::new(item));
    }
    t.stack
        .PickValue(slice)
        .expect("Pick requires a non-empty value list and a valid pick stack")
        .downcast_ref::<E>()
        .expect("Pick value should keep the original Go generic type")
        .clone()
}

// PickEnum returns a value from the value enums
// PickEnum 对应 Go 的变参包装：把首个值和其它候选合并后委托给 Pick。
/// 变参风格包装：将首个值与其余候选合并后委托给 [`Pick`]。
pub fn PickEnum<E>(t: &mut T, item: E, mut other: Vec<E>) -> E
where
    E: Clone + Debug + 'static,
{
    let mut values = Vec::with_capacity(other.len() + 1);
    values.push(item);
    values.append(&mut other);
    Pick(t, values)
}
