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

// 错误处理上下文（errctx）：按错误组配置 Error/Warn/Ignore 级别并统一处理。
//
// SQL 执行中部分错误（截断、重复键、除零等）可按会话 SQL Mode 降级为警告或忽略。
// `Context` 持有各组的 `Level` 映射与警告追加器；`HandleError` /
// `HandleErrorWithAlias` 按 errno 归组后决定返回、告警或丢弃。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::{Arc, LazyLock};

use crate::{contextutil, errno, errors};

/// Level defines the behavior for each error.
///
/// 错误处理级别：返回调用方、降级为警告，或忽略。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Level {
    /// The error is returned to the caller.
    ///
    /// 将错误返回给调用方（严格模式）。
    #[default]
    LevelError = 0,
    /// The error is appended as a warning.
    ///
    /// 追加为警告（Warning），不中断当前语句。
    LevelWarn = 1,
    /// The error is ignored.
    ///
    /// 完全忽略该错误。
    LevelIgnore = 2,
}

/// ErrGroup groups errors that share handling behavior.
///
/// 共享同一处理策略的错误分组（与 SQL Mode 相关开关对应）。
#[repr(usize)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrGroup {
    /// 数值/字符串/日期截断或越界类错误。
    ErrGroupTruncate = 0,
    /// 重复键（Dup Entry）。
    ErrGroupDupKey = 1,
    /// 非法 NULL（NOT NULL 列写入 NULL）。
    ErrGroupBadNull = 2,
    /// 缺少默认值。
    ErrGroupNoDefault = 3,
    /// 除以零。
    ErrGroupDividedByZero = 4,
    /// 自增列读失败（AUTO_INCREMENT 耗尽等）。
    ErrGroupAutoIncReadFailed = 5,
    /// 行值无法匹配任何分区。
    ErrGroupNoMatchedPartition = 6,
}

/// Keep this in sync with the last [`ErrGroup`] variant.
///
/// 与最后一个 ErrGroup 变体保持同步的分组数量。
pub const errGroupCount: usize = 7;
/// 各组当前 Level 的定长映射表。
pub type LevelMap = [Level; errGroupCount];
/// 警告追加器的共享引用（可将错误记入会话 Warning 列表）。
pub type WarnAppenderRef = Arc<dyn contextutil::WarnAppender + Send + Sync>;

/// Context defines how errors are returned, downgraded to warnings, or ignored.
///
/// 描述错误如何被返回、降级为警告或忽略的处理上下文。
#[derive(Clone)]
pub struct Context {
    /// 各 ErrGroup 对应的处理级别。
    levelMap: LevelMap,
    /// 警告/备注追加回调。
    warnHandler: WarnAppenderRef,
}

impl Context {
    /// 返回当前 Level 映射的副本。
    pub fn LevelMap(&self) -> LevelMap {
        self.levelMap
    }

    /// 查询指定错误组的处理级别。
    pub fn LevelForGroup(&self, errGroup: ErrGroup) -> Level {
        self.levelMap[errGroup as usize]
    }

    /// Returns a derived strict context without changing this context.
    ///
    /// 派生严格上下文：所有组均为 LevelError，不修改原上下文。
    pub fn WithStrictErrGroupLevel(&self) -> Context {
        Context {
            levelMap: [Level::LevelError; errGroupCount],
            warnHandler: Arc::clone(&self.warnHandler),
        }
    }

    /// 派生上下文：仅将指定错误组的级别改为 `level`。
    pub fn WithErrGroupLevel(&self, eg: ErrGroup, level: Level) -> Context {
        let mut levels = self.levelMap;
        levels[eg as usize] = level;
        Context {
            levelMap: levels,
            warnHandler: Arc::clone(&self.warnHandler),
        }
    }

    /// 派生上下文：整体替换 Level 映射。
    pub fn WithErrGroupLevels(&self, levels: LevelMap) -> Context {
        Context {
            levelMap: levels,
            warnHandler: Arc::clone(&self.warnHandler),
        }
    }

    /// 通过 warnHandler 追加一条警告。
    pub fn AppendWarning(&self, err: errors::SharedError) {
        self.warnHandler.AppendWarning(err);
    }

    /// 通过 warnHandler 追加一条 Note（提示）。
    pub fn AppendNote(&self, err: errors::SharedError) {
        self.warnHandler.AppendNote(err);
    }

    /// Handles a single error or each child of an ErrorGroup in order.
    /// Processing stops as soon as one child must be returned, matching Go.
    ///
    /// 处理单个错误，或按序处理 ErrorGroup 的每个子错误；
    /// 一旦某个子错误必须返回给调用方则立即停止，行为对齐 Go。
    pub fn HandleError(&self, err: Option<errors::SharedError>) -> Option<errors::SharedError> {
        let err = err?;
        let children = errors::Errors(&err);
        // 子错误不止一个，或唯一子错误与自身不是同一指针时，视为错误组。
        let isGroup = children.len() != 1 || !children[0].ptr_eq(&err);
        if isGroup {
            for singleErr in children {
                if let Some(returned) = self.HandleError(Some(singleErr)) {
                    return Some(returned);
                }
            }
            return None;
        }

        self.HandleErrorWithAlias(Some(&err), err.clone(), err.clone())
    }

    /// Handles an internal error while allowing distinct returned and warning aliases.
    ///
    /// 处理内部错误，允许返回错误与警告错误使用不同别名（便于对外文案）。
    pub fn HandleErrorWithAlias(
        &self,
        internalErr: Option<&errors::SharedError>,
        err: errors::SharedError,
        warnErr: errors::SharedError,
    ) -> Option<errors::SharedError> {
        let cause = errors::Cause(internalErr)?;
        let Some(sqlErr) = cause.downcast_ref::<errors::Error>() else {
            // 非 SQL Error 类型：原样返回。
            return Some(err);
        };
        let Ok(code) = u16::try_from(sqlErr.Code()) else {
            return Some(err);
        };
        let Some(group) = Self::ErrGroupForCode(code) else {
            // 未归组的 errno：不降级，直接返回。
            return Some(err);
        };

        match self.levelMap[group as usize] {
            Level::LevelError => Some(err),
            Level::LevelWarn => {
                self.AppendWarning(warnErr);
                None
            }
            Level::LevelIgnore => None,
        }
    }

    /// Maps every errno listed by the Go `init` function to its error group.
    ///
    /// 将 Go `init` 中登记的 errno 映射到对应 ErrGroup；未登记返回 None。
    pub fn ErrGroupForCode(code: u16) -> Option<ErrGroup> {
        match code {
            errno::ErrTruncatedWrongValue
            | errno::ErrDataTooLong
            | errno::ErrTruncatedWrongValueForField
            | errno::ErrWarnDataOutOfRange
            | errno::ErrDataOutOfRange
            | errno::ErrBadNumber
            | errno::ErrWrongValueForType
            | errno::ErrDatetimeFunctionOverflow
            | errno::WarnDataTruncated
            | errno::ErrIncorrectDatetimeValue => Some(ErrGroup::ErrGroupTruncate),
            errno::ErrBadNull | errno::ErrWarnNullToNotnull => Some(ErrGroup::ErrGroupBadNull),
            errno::ErrNoDefaultForField => Some(ErrGroup::ErrGroupNoDefault),
            errno::ErrDivisionByZero => Some(ErrGroup::ErrGroupDividedByZero),
            errno::ErrAutoincReadFailed => Some(ErrGroup::ErrGroupAutoIncReadFailed),
            errno::ErrNoPartitionForGivenValue | errno::ErrRowDoesNotMatchGivenPartitionSet => {
                Some(ErrGroup::ErrGroupNoMatchedPartition)
            }
            errno::ErrDupEntry => Some(ErrGroup::ErrGroupDupKey),
            _ => None,
        }
    }
}

/// 以全 Error 级别与给定警告追加器构造上下文。
pub fn NewContext(handler: WarnAppenderRef) -> Context {
    NewContextWithLevels([Level::LevelError; errGroupCount], handler)
}

/// 以显式 Level 映射构造上下文。
pub fn NewContextWithLevels(levels: LevelMap, handler: WarnAppenderRef) -> Context {
    Context {
        levelMap: levels,
        warnHandler: handler,
    }
}

/// A strict context whose warning sink intentionally discards all input.
///
/// 严格上下文：警告接收端丢弃全部输入（无会话 Warning 列表时使用）。
pub static StrictNoWarningContext: LazyLock<Context> = LazyLock::new(|| {
    let handler: WarnAppenderRef = Arc::new(contextutil::ignoreWarn {});
    NewContext(handler)
});

/// Resolves an error level from the Go `ignore` and `warn` flags.
///
/// 由 Go 侧 `ignore`/`warn` 标志解析出 Level：ignore 优先于 warn。
pub fn ResolveErrLevel(ignore: bool, warn: bool) -> Level {
    if ignore {
        Level::LevelIgnore
    } else if warn {
        Level::LevelWarn
    } else {
        Level::LevelError
    }
}
