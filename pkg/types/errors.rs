// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `types` 包使用的标准错误与时间类型名常量。
//
// 多数错误经 `standard_error!` 绑定到 `dbterror::ClassTypes` 的 MySQL errno；
// 少数（如 ErrSyntax/ErrWrongValue*）用 NewStdErr 显式指定码与文案。
// ErrTimestampInDSTTransition 属 ClassExecutor（夏令时跳变）。

use std::sync::LazyLock;

/// 日期时间类型名字符串，供错误文案等使用。
pub const DateTimeStr: &str = "datetime";
/// 日期类型名。
pub const DateStr: &str = "date";
/// 时间类型名。
pub const TimeStr: &str = "time";
/// 时间戳类型名。
pub const TimestampStr: &str = "timestamp";

/// 生成 LazyLock 包装的标准 ClassTypes 错误静态量。
macro_rules! standard_error {
    ($name:ident, $class:ident, $code:ident) => {
        pub static $name: LazyLock<Box<dbterror::terror::Error>> =
            LazyLock::new(|| dbterror::$class.NewStd(dbterror::errno::$code));
    };
}

// Go 直接复用 parser/types 的包级实例；重导出可保留错误身份与初始化副作用。
pub use parser_types::types::ErrInvalidDefault;

// 以下为类型系统常用截断/溢出/精度/年份等标准错误绑定
standard_error!(ErrDataTooLong, ClassTypes, ErrDataTooLong);
standard_error!(ErrIllegalValueForType, ClassTypes, ErrIllegalValueForType);
standard_error!(ErrTruncated, ClassTypes, WarnDataTruncated);
standard_error!(ErrOverflow, ClassTypes, ErrDataOutOfRange);
standard_error!(ErrDivByZero, ClassTypes, ErrDivisionByZero);
standard_error!(ErrTooBigDisplayWidth, ClassTypes, ErrTooBigDisplaywidth);
standard_error!(ErrTooBigFieldLength, ClassTypes, ErrTooBigFieldlength);
standard_error!(ErrTooBigSet, ClassTypes, ErrTooBigSet);
standard_error!(ErrTooBigScale, ClassTypes, ErrTooBigScale);
standard_error!(ErrTooBigPrecision, ClassTypes, ErrTooBigPrecision);
standard_error!(ErrBadNumber, ClassTypes, ErrBadNumber);
standard_error!(ErrInvalidFieldSize, ClassTypes, ErrInvalidFieldSize);
standard_error!(ErrMBiggerThanD, ClassTypes, ErrMBiggerThanD);
standard_error!(ErrWarnDataOutOfRange, ClassTypes, ErrWarnDataOutOfRange);
standard_error!(
    ErrDuplicatedValueInType,
    ClassTypes,
    ErrDuplicatedValueInType
);
standard_error!(
    ErrDatetimeFunctionOverflow,
    ClassTypes,
    ErrDatetimeFunctionOverflow
);
standard_error!(ErrCastAsSignedOverflow, ClassTypes, ErrCastAsSignedOverflow);
standard_error!(ErrCastNegIntAsUnsigned, ClassTypes, ErrCastNegIntAsUnsigned);
standard_error!(ErrInvalidYearFormat, ClassTypes, ErrInvalidYearFormat);
standard_error!(ErrInvalidYear, ClassTypes, ErrInvalidYear);
standard_error!(ErrTruncatedWrongVal, ClassTypes, ErrTruncatedWrongValue);
standard_error!(
    ErrInvalidWeekModeFormat,
    ClassTypes,
    ErrInvalidWeekModeFormat
);
standard_error!(ErrWrongFieldSpec, ClassTypes, ErrWrongFieldSpec);

/// 语法/解析错误：errno 用 ErrParse，文案取 ErrSyntax。
pub static ErrSyntax: LazyLock<Box<dbterror::terror::Error>> = LazyLock::new(|| {
    dbterror::ClassTypes.NewStdErr(
        dbterror::errno::ErrParse,
        &dbterror::errno::MySQLErrName[&dbterror::errno::ErrSyntax],
    )
});
/// 错误值：码为 TruncatedWrongValue，文案取 ErrWrongValue。
pub static ErrWrongValue: LazyLock<Box<dbterror::terror::Error>> = LazyLock::new(|| {
    dbterror::ClassTypes.NewStdErr(
        dbterror::errno::ErrTruncatedWrongValue,
        &dbterror::errno::MySQLErrName[&dbterror::errno::ErrWrongValue],
    )
});
/// 错误值变体：码与文案均取 ErrWrongValue。
pub static ErrWrongValue2: LazyLock<Box<dbterror::terror::Error>> = LazyLock::new(|| {
    dbterror::ClassTypes.NewStdErr(
        dbterror::errno::ErrWrongValue,
        &dbterror::errno::MySQLErrName[&dbterror::errno::ErrWrongValue],
    )
});
/// 类型不匹配的错误值。
pub static ErrWrongValueForType: LazyLock<Box<dbterror::terror::Error>> = LazyLock::new(|| {
    dbterror::ClassTypes.NewStdErr(
        dbterror::errno::ErrWrongValueForType,
        &dbterror::errno::MySQLErrName[&dbterror::errno::ErrWrongValueForType],
    )
});

standard_error!(
    ErrPartitionStatsMissing,
    ClassTypes,
    ErrPartitionStatsMissing
);
standard_error!(
    ErrPartitionColumnStatsMissing,
    ClassTypes,
    ErrPartitionColumnStatsMissing
);
standard_error!(
    ErrIncorrectDatetimeValue,
    ClassTypes,
    ErrIncorrectDatetimeValue
);
standard_error!(ErrJSONBadOneOrAllArg, ClassTypes, ErrJSONBadOneOrAllArg);
standard_error!(ErrJSONVacuousPath, ClassTypes, ErrJSONVacuousPath);
standard_error!(
    ErrTimestampInDSTTransition,
    ClassExecutor,
    ErrTimeStampInDSTTransition
);
