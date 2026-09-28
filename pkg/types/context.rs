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

// 类型转换 Context：标志位、时区与截断/告警策略。
//
// `Flags` 控制溢出截断、负转无符号、零日期与字符集跳过检查等行为；
// `Context` 将标志、时区与 WarnAppender（告警追加器）组合，供类型转换路径使用。

use std::fmt;
use std::ops::{BitAnd, BitOr, Not};
use std::sync::{Arc, LazyLock};

use chrono_tz::Tz;
use contextutil::WarnAppender;

/// 严格模式：不忽略任何截断/校验错误。
pub const StrictFlags: Flags = Flags(0);

/// 类型转换行为标志集合（位掩码）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Flags(pub u16);

/// 忽略截断错误，直接返回裁剪后的值。
pub const FlagIgnoreTruncateErr: Flags = Flags(1 << 0);
/// 截断时记为 warning（警告）而非硬错误。
pub const FlagTruncateAsWarning: Flags = Flags(1 << 1);
/// 允许负数强制转为无符号（按位 reinterpret）。
pub const FlagAllowNegativeToUnsigned: Flags = Flags(1 << 2);
/// 忽略零日期（如 0000-00-00）相关错误。
pub const FlagIgnoreZeroDateErr: Flags = Flags(1 << 3);
/// 忽略日期中零分量（如月/日为 0）相关错误。
pub const FlagIgnoreZeroInDateErr: Flags = Flags(1 << 4);
/// 忽略非法日期错误。
pub const FlagIgnoreInvalidDateErr: Flags = Flags(1 << 5);
/// 跳过 ASCII 字符集合法性检查。
pub const FlagSkipASCIICheck: Flags = Flags(1 << 6);
/// 跳过 UTF-8 合法性检查。
pub const FlagSkipUTF8Check: Flags = Flags(1 << 7);
/// 跳过 UTF8MB4 合法性检查。
pub const FlagSkipUTF8MB4Check: Flags = Flags(1 << 8);
/// CAST TIME 到 YEAR 时经字符串拼接路径（兼容特定 MySQL 行为）。
pub const FlagCastTimeToYearThroughConcat: Flags = Flags(1 << 9);

impl BitOr for Flags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitAnd for Flags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl Not for Flags {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

impl Flags {
    /// 是否包含指定标志位。
    fn contains(self, flag: Flags) -> bool {
        self.0 & flag.0 != 0
    }

    /// 开启或关闭指定标志，返回新的 Flags（不可变更新）。
    fn with(self, flag: Flags, enabled: bool) -> Flags {
        if enabled {
            Flags(self.0 | flag.0)
        } else {
            Flags(self.0 & !flag.0)
        }
    }

    /// 是否允许负值转无符号。
    pub fn AllowNegativeToUnsigned(self) -> bool {
        self.contains(FlagAllowNegativeToUnsigned)
    }

    /// 设置 AllowNegativeToUnsigned。
    pub fn WithAllowNegativeToUnsigned(self, enabled: bool) -> Flags {
        self.with(FlagAllowNegativeToUnsigned, enabled)
    }

    /// 是否跳过 ASCII 检查。
    pub fn SkipASCIICheck(self) -> bool {
        self.contains(FlagSkipASCIICheck)
    }

    // Keep the historical Go spelling for API parity.
    /// 设置 SkipASCIICheck（保留 Go 历史拼写 SACII）。
    pub fn WithSkipSACIICheck(self, enabled: bool) -> Flags {
        self.with(FlagSkipASCIICheck, enabled)
    }

    /// 是否跳过 UTF-8 检查。
    pub fn SkipUTF8Check(self) -> bool {
        self.contains(FlagSkipUTF8Check)
    }

    /// 设置 SkipUTF8Check。
    pub fn WithSkipUTF8Check(self, enabled: bool) -> Flags {
        self.with(FlagSkipUTF8Check, enabled)
    }

    /// 是否跳过 UTF8MB4 检查。
    pub fn SkipUTF8MB4Check(self) -> bool {
        self.contains(FlagSkipUTF8MB4Check)
    }

    /// 设置 SkipUTF8MB4Check。
    pub fn WithSkipUTF8MB4Check(self, enabled: bool) -> Flags {
        self.with(FlagSkipUTF8MB4Check, enabled)
    }

    /// 是否忽略截断错误。
    pub fn IgnoreTruncateErr(self) -> bool {
        self.contains(FlagIgnoreTruncateErr)
    }

    /// 设置 IgnoreTruncateErr。
    pub fn WithIgnoreTruncateErr(self, enabled: bool) -> Flags {
        self.with(FlagIgnoreTruncateErr, enabled)
    }

    /// 截断是否转为 warning。
    pub fn TruncateAsWarning(self) -> bool {
        self.contains(FlagTruncateAsWarning)
    }

    /// 设置 TruncateAsWarning。
    pub fn WithTruncateAsWarning(self, enabled: bool) -> Flags {
        self.with(FlagTruncateAsWarning, enabled)
    }

    /// 是否忽略日期中的零分量错误。
    pub fn IgnoreZeroInDate(self) -> bool {
        self.contains(FlagIgnoreZeroInDateErr)
    }

    /// 设置 IgnoreZeroInDate。
    pub fn WithIgnoreZeroInDate(self, enabled: bool) -> Flags {
        self.with(FlagIgnoreZeroInDateErr, enabled)
    }

    /// 是否忽略非法日期错误。
    pub fn IgnoreInvalidDateErr(self) -> bool {
        self.contains(FlagIgnoreInvalidDateErr)
    }

    /// 设置 IgnoreInvalidDateErr。
    pub fn WithIgnoreInvalidDateErr(self, enabled: bool) -> Flags {
        self.with(FlagIgnoreInvalidDateErr, enabled)
    }

    /// 是否忽略零日期错误。
    pub fn IgnoreZeroDateErr(self) -> bool {
        self.contains(FlagIgnoreZeroDateErr)
    }

    /// 设置 IgnoreZeroDateErr。
    pub fn WithIgnoreZeroDateErr(self, enabled: bool) -> Flags {
        self.with(FlagIgnoreZeroDateErr, enabled)
    }

    /// 是否经拼接路径将 TIME CAST 为 YEAR。
    pub fn CastTimeToYearThroughConcat(self) -> bool {
        self.contains(FlagCastTimeToYearThroughConcat)
    }

    /// 设置 CastTimeToYearThroughConcat。
    pub fn WithCastTimeToYearThroughConcat(self, enabled: bool) -> Flags {
        self.with(FlagCastTimeToYearThroughConcat, enabled)
    }
}

/// 携带部分转换结果值的错误，便于截断后仍返回裁剪值。
#[derive(Clone, Debug)]
pub struct ErrorWithValue<T> {
    /// 截断或溢出后的回退值。
    pub value: T,
    /// 共享错误对象。
    pub error: contextutil::errors::SharedError,
}

impl<T> ErrorWithValue<T> {
    /// 构造带值的错误。
    pub fn new(value: T, error: contextutil::errors::SharedError) -> Self {
        Self { value, error }
    }
}

impl<T> fmt::Display for ErrorWithValue<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, formatter)
    }
}

impl<T: fmt::Debug> std::error::Error for ErrorWithValue<T> {}

/// 成功为 T，失败为带裁剪值的 ErrorWithValue。
pub type ValueResult<T> = Result<T, ErrorWithValue<T>>;

/// 类型转换上下文：标志、时区与告警处理器。
#[derive(Clone)]
pub struct Context {
    flags: Flags,
    loc: Tz,
    warnHandler: Arc<dyn WarnAppender + Send + Sync>,
}

/// 构造新的类型转换 Context。
pub fn NewContext(flags: Flags, loc: Tz, handler: Arc<dyn WarnAppender + Send + Sync>) -> Context {
    Context {
        flags,
        loc,
        warnHandler: handler,
    }
}

impl Context {
    /// 当前标志位。
    pub fn Flags(&self) -> Flags {
        self.flags
    }

    /// 以新标志克隆 Context（原实例不变）。
    pub fn WithFlags(&self, flags: Flags) -> Context {
        let mut result = self.clone();
        result.flags = flags;
        result
    }

    /// 以新时区克隆 Context。
    pub fn WithLocation(&self, loc: Tz) -> Context {
        let mut result = self.clone();
        result.loc = loc;
        result
    }

    /// 当前时区。
    pub fn Location(&self) -> Tz {
        self.loc
    }

    /// 追加一条 warning。
    pub fn AppendWarning(&self, err: contextutil::errors::SharedError) {
        self.warnHandler.AppendWarning(err);
    }

    // Every caller supplies a truncation-class error, matching types.Context.HandleTruncate.
    /// 按标志处理截断：忽略、转 warning 或返回带值错误。
    pub fn HandleTruncate<T>(
        &self,
        value: T,
        err: contextutil::errors::SharedError,
    ) -> ValueResult<T> {
        if self.flags.IgnoreTruncateErr() {
            return Ok(value);
        }
        if self.flags.TruncateAsWarning() {
            self.AppendWarning(err);
            return Ok(value);
        }
        Err(ErrorWithValue::new(value, err))
    }
}

impl types_time::TimeContext for Context {
    fn flags(&self) -> types_time::TimeFlags {
        types_time::TimeFlags {
            ignore_zero_in_date: self.flags.IgnoreZeroInDate(),
            ignore_invalid_date: self.flags.IgnoreInvalidDateErr(),
            ignore_zero_date: self.flags.IgnoreZeroDateErr(),
            cast_time_to_year_through_concat: self.flags.CastTimeToYearThroughConcat(),
        }
    }

    fn location(&self) -> Tz {
        self.loc
    }

    fn append_warning(&self, warning: types_time::TimeError) {
        self.AppendWarning(contextutil::errors::SharedError::new(warning));
    }
}

/// 丢弃所有 warning/note 的空处理器。
struct IgnoreWarnings;

impl WarnAppender for IgnoreWarnings {
    fn AppendWarning(&self, _err: contextutil::errors::SharedError) {}

    fn AppendNote(&self, _err: contextutil::errors::SharedError) {}
}

/// 默认语句标志：允许负转无符号，并忽略零日期错误。
pub const DefaultStmtFlags: Flags =
    Flags(StrictFlags.0 | FlagAllowNegativeToUnsigned.0 | FlagIgnoreZeroDateErr.0);

/// 默认语句 Context：DefaultStmtFlags + UTC，不收集 warning。
pub static DefaultStmtNoWarningContext: LazyLock<Context> =
    LazyLock::new(|| NewContext(DefaultStmtFlags, chrono_tz::UTC, Arc::new(IgnoreWarnings)));

/// 严格 Context：StrictFlags + UTC，不收集 warning。
pub static StrictContext: LazyLock<Context> =
    LazyLock::new(|| NewContext(StrictFlags, chrono_tz::UTC, Arc::new(IgnoreWarnings)));
