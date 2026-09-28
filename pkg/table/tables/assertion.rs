// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 事务内存缓冲（mem buffer）上的键断言（assertion）标志辅助。
//
// TiDB 在悲观/乐观事务预写前可为某个 KV 键设置「应存在 / 应不存在 / 未知」等断言；
// 本模块提供与 Go `tables` 包一致的「已有断言优先、缺失才写入」语义，
// 通过 `AssertionBuffer` 抽象对接具体的 mem buffer 实现。

/// 键断言操作类型，对应 Go 的 `kv.AssertionOp`。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssertionOp {
    /// 不附加任何断言标志。
    None,
    /// 断言该键在存储中应已存在。
    Exist,
    /// 断言该键在存储中应不存在。
    NotExist,
    /// 断言状态未知（例如仅提示需要加锁检查）。
    Unknown,
}

/// 单个键在 mem buffer 上携带的标志位视图。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyFlags {
    /// 当前键上已记录的断言操作；`None` 表示尚无断言。
    pub assertion: Option<AssertionOp>,
}

/// 可读写键标志的缓冲抽象，供 `set_assertion` 操作。
pub trait AssertionBuffer {
    /// 缓冲操作返回的错误类型。
    type Error;
    /// 读取指定键的当前标志；键不存在时应返回可由 `is_not_found` 识别的错误。
    fn get_flags(&self, key: &[u8]) -> Result<KeyFlags, Self::Error>;
    /// 判断错误是否表示「键标志尚未建立」（可安全初始化）。
    fn is_not_found(error: &Self::Error) -> bool;
    /// 将断言写入（或更新到）指定键的标志中。
    fn update_assertion_flags(&mut self, key: &[u8], assertion: AssertionOp);
}

/// Existing assertion flags win. Missing flags are initialized, while every
/// other lookup error is propagated unchanged.
///
/// 设置键断言：若已有断言则保持不变；标志缺失或键不存在时写入新断言；
/// 其它查找错误原样向上传播。
pub fn set_assertion<B: AssertionBuffer>(
    buffer: &mut B,
    key: &[u8],
    assertion: AssertionOp,
) -> Result<(), B::Error> {
    // 已有断言优先；仅在无断言或键不存在时进入写入分支。
    match buffer.get_flags(key) {
        Ok(flags) if flags.assertion.is_some() => return Ok(()),
        Ok(_) => {}
        Err(error) if B::is_not_found(&error) => {}
        Err(error) => return Err(error),
    }
    buffer.update_assertion_flags(key, assertion);
    Ok(())
}
