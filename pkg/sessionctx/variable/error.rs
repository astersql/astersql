// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 会话变量子系统的错误描述符与 errno 表。
//
// 对齐 Go `dbterror`：错误由 class、MySQL/TiDB 错误码与消息模板组成；
// 栈捕获由调用方负责。`format` 填充 `%s`/`%d` 及精度限定占位符。

#![allow(non_upper_case_globals)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 错误所属子系统分类（对应 Go 错误类）。
pub enum ErrorClass {
    /// 系统变量相关错误。
    Variable,
    /// 执行器侧错误（如密码策略）。
    Executor,
}

/// The stable portion of a TiDB/MySQL error. Stack capture belongs to the
/// caller; keeping the code, class and message together mirrors dbterror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 稳定的错误描述：class、errno 码与消息模板。
///
/// 栈信息由调用方捕获；三者捆绑以镜像 dbterror 的错误对象。
pub struct ErrorDescriptor {
    /// 错误所属类。
    pub class: ErrorClass,
    /// MySQL/TiDB 错误码（errno）。
    pub code: u16,
    /// 可含 printf 风格占位符的消息模板。
    pub message: &'static str,
}

impl ErrorDescriptor {
    /// 构造常量错误描述符。
    pub const fn new(class: ErrorClass, code: u16, message: &'static str) -> Self {
        Self {
            class,
            code,
            message,
        }
    }

    /// Formats the `%s`, `%d` and MySQL precision-qualified placeholders used
    /// by errno messages. Extra arguments are ignored, as in the Go call sites.
    /// 按消息模板填充参数，生成最终错误字符串。
    ///
    /// 支持 `%s`、`%d` 及 MySQL 精度限定形式（如 `'%-.64s'`）；多余参数忽略。
    pub fn format(&self, args: &[&str]) -> String {
        // 手工扫描模板：遇 `%%` 输出字面 `%`；遇 `%...[sd]` 用下一个参数替换。
        // Go fmt 对字符串精度按 Unicode 字符而非 UTF-8 字节计数。
        let bytes = self.message.as_bytes();
        let mut out = String::with_capacity(self.message.len() + 16);
        let mut i = 0;
        let mut arg = 0;
        while i < bytes.len() {
            let Some(percent_offset) = self.message[i..].find('%') else {
                out.push_str(&self.message[i..]);
                break;
            };
            let percent = i + percent_offset;
            out.push_str(&self.message[i..percent]);
            if percent + 1 < bytes.len() && bytes[percent + 1] == b'%' {
                out.push('%');
                i = percent + 2;
                continue;
            }
            let mut end = percent + 1;
            while end < bytes.len() && !matches!(bytes[end], b's' | b'd') {
                end += 1;
            }
            if end < bytes.len() {
                if let Some(value) = args.get(arg) {
                    if bytes[end] == b's' {
                        let spec = &self.message[percent + 1..end];
                        let precision = spec
                            .split_once('.')
                            .and_then(|(_, digits)| digits.parse::<usize>().ok());
                        match precision {
                            Some(limit) => out.extend(value.chars().take(limit)),
                            None => out.push_str(value),
                        }
                    } else {
                        out.push_str(value);
                    }
                } else {
                    out.push_str(&self.message[percent..=end]);
                }
                arg += 1;
                i = end + 1;
            } else {
                out.push('%');
                i = percent + 1;
            }
        }
        out
    }
}

// 便捷别名，缩短下方静态错误表初始化。
const VARIABLE: ErrorClass = ErrorClass::Variable;
const EXECUTOR: ErrorClass = ErrorClass::Executor;

// —— 变量子系统 errno 表（静态 ErrorDescriptor）——
/// 弃用语法警告：建议改用替代写法。
pub static errWarnDeprecatedSyntax: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1287,
    "'%s' is deprecated and will be removed in a future release. Please use %s instead",
);
/// 快照（snapshot）早于 GC 安全点，无法按历史时间戳读。
///
/// GC safe point：存储层已回收更早版本数据的水位线。
pub static ErrSnapshotTooOld: ErrorDescriptor =
    ErrorDescriptor::new(VARIABLE, 8055, "snapshot is older than GC safe point %s");
/// 变量尚不支持所给取值。
pub static ErrUnsupportedValueForVar: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    8047,
    "variable '%s' does not yet support value: %s",
);
/// 未知系统变量名。
pub static ErrUnknownSystemVar: ErrorDescriptor =
    ErrorDescriptor::new(VARIABLE, 1193, "Unknown system variable '%-.64s'");
/// 作用域（scope）不匹配：例如对 SESSION 变量使用了 GLOBAL，或反之。
pub static ErrIncorrectScope: ErrorDescriptor =
    ErrorDescriptor::new(VARIABLE, 1238, "Variable '%-.192s' is a %s variable");
/// 未知或非法时区名。
pub static ErrUnknownTimeZone: ErrorDescriptor =
    ErrorDescriptor::new(VARIABLE, 1298, "Unknown or incorrect time zone: '%-.64s'");
/// 变量只读，需用指定 SET 形式赋值。
pub static ErrReadOnly: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1621,
    "%s variable '%s' is read-only. Use SET %s to assign the value",
);
/// 变量取值非法。
pub static ErrWrongValueForVar: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1231,
    "Variable '%-.64s' can't be set to the value of '%-.200s'",
);
/// 变量参数类型不正确。
pub static ErrWrongTypeForVar: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1232,
    "Incorrect argument type to variable '%-.64s'",
);
/// 值被截断且不正确。
pub static ErrTruncatedWrongValue: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1292,
    "Truncated incorrect %-.64s value: '%-.128s'",
);
/// 已达 max_prepared_stmt_count 上限，无法再创建预处理语句。
pub static ErrMaxPreparedStmtCountReached: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1461,
    "Can't create more than maxPreparedStmtCount statements (current value: %d)",
);
/// 不支持的事务隔离级别；可通过 `tidb_skip_isolation_level_check` 跳过检查。
pub static ErrUnsupportedIsolationLevel: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    8048,
    "The isolation level '%s' is not supported. Set tidb_skip_isolation_level_check=1 to skip this error",
);
/// 未知系统变量（内部/小写别名条目，errno 与 ErrUnknownSystemVar 相同）。
pub static errUnknownSystemVariable: ErrorDescriptor =
    ErrorDescriptor::new(VARIABLE, 1193, "Unknown system variable '%-.64s'");
/// 该变量为 GLOBAL，必须用 SET GLOBAL。
pub static errGlobalVariable: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1229,
    "Variable '%-.64s' is a GLOBAL variable and should be set with SET GLOBAL",
);
/// 该变量为 SESSION，不能用 SET GLOBAL。
pub static errLocalVariable: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1228,
    "Variable '%-.64s' is a SESSION variable and can't be used with SET GLOBAL",
);
/// 在另一变量为 ON 时，本变量不能为 OFF。
pub static errValueNotSupportedWhen: ErrorDescriptor =
    ErrorDescriptor::new(VARIABLE, 1235, "%s = OFF is not supported when %s = ON");
/// starter 部署模式不支持 SET GLOBAL max_allowed_packet。
pub static errSetGlobalMaxAllowedPacket: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1235,
    "SET GLOBAL max_allowed_packet is not supported in starter deployment mode",
);
/// NextGen 内核不支持该功能/变量。
pub static ErrNotSupportedInNextGen: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1235,
    "%s is not supported in the next generation of TiDB",
);
/// 密码不满足当前策略要求。
pub static ErrNotValidPassword: ErrorDescriptor = ErrorDescriptor::new(
    EXECUTOR,
    1819,
    "Your password does not satisfy the current policy requirements (%s)",
);
/// 函数目前仅为 noop 实现，需开启 `tidb_enable_noop_functions`。
pub static ErrFunctionsNoopImpl: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1235,
    "function %s has only noop implementation in tidb now, use tidb_enable_noop_functions to enable these functions",
);
/// 选项已不再支持。
pub static ErrVariableNoLongerSupported: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    8136,
    "option '%s' is no longer supported. Reason: %s",
);
/// utf8mb4 默认排序规则非法。
pub static ErrInvalidDefaultUTF8MB4Collation: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    3721,
    "Invalid default collation %s: utf8mb4_0900_ai_ci or utf8mb4_general_ci or utf8mb4_bin expected",
);
/// 更新该变量已弃用，未来将变为只读。
pub static ErrWarnDeprecatedSyntaxNoReplacement: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1681,
    "Updating '%s' is deprecated. It will be made read-only in a future release.",
);
/// 简单弃用警告：将在未来版本移除。
pub static ErrWarnDeprecatedSyntaxSimpleMsg: ErrorDescriptor = ErrorDescriptor::new(
    VARIABLE,
    1681,
    "%s is deprecated and will be removed in a future release.",
);

/// 本模块全部错误描述符列表，便于枚举与测试。
pub static ALL_ERRORS: &[&ErrorDescriptor] = &[
    &errWarnDeprecatedSyntax,
    &ErrSnapshotTooOld,
    &ErrUnsupportedValueForVar,
    &ErrUnknownSystemVar,
    &ErrIncorrectScope,
    &ErrUnknownTimeZone,
    &ErrReadOnly,
    &ErrWrongValueForVar,
    &ErrWrongTypeForVar,
    &ErrTruncatedWrongValue,
    &ErrMaxPreparedStmtCountReached,
    &ErrUnsupportedIsolationLevel,
    &errUnknownSystemVariable,
    &errGlobalVariable,
    &errLocalVariable,
    &errValueNotSupportedWhen,
    &errSetGlobalMaxAllowedPacket,
    &ErrNotSupportedInNextGen,
    &ErrNotValidPassword,
    &ErrFunctionsNoopImpl,
    &ErrVariableNoLongerSupported,
    &ErrInvalidDefaultUTF8MB4Collation,
    &ErrWarnDeprecatedSyntaxNoReplacement,
    &ErrWarnDeprecatedSyntaxSimpleMsg,
];
