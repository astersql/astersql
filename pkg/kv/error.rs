// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// KV 层预定义错误与可重试判定。
//
// 集中声明事务（Transaction）读写路径常见错误原型：键不存在、写冲突、
// 条目过大、锁过期等。部分错误消息追加 `TxnRetryableMark`，提示客户端稍后重试。
// 写冲突（Write Conflict）指并发事务修改同一键，乐观事务提交时被拒绝。

use std::sync::LazyLock;

/// 本模块统一使用的共享错误别名。
pub type Error = errors::SharedError;

/// 可重试错误消息后缀，提示客户端稍后再试。
pub const TxnRetryableMark: &str = "[try again later]";

/// 键不存在（对应 SQL/KV 层 Not Found）。
pub static ErrNotExist: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassKV.NewStd(errno::ErrNotExist));

/// 事务可重试错误；消息拼接原始 errno 文本与 TxnRetryableMark。
pub static ErrTxnRetryable: LazyLock<Box<errors::Error>> = LazyLock::new(|| {
    // 从 MySQL 错误名表取原始模板，再附加可重试标记。
    let source = &errno::MySQLErrName[&errno::ErrTxnRetryable];
    let message = parser_mysql::Message(
        &format!("{}{}", source.Raw, TxnRetryableMark),
        &source.RedactArgPos,
    );
    dbterror::ClassKV.NewStdErr(errno::ErrTxnRetryable, &message)
});

/// 禁止将 nil/空值写入 KV（Set 必须提供有效字节）。
pub static ErrCannotSetNilValue: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassKV.NewStd(errno::ErrCannotSetNilValue));
/// 当前事务无效（已回滚或未正确开启）。
pub static ErrInvalidTxn: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassKV.NewStd(errno::ErrInvalidTxn));
/// 事务总体写入体积超过 TxnTotalSizeLimit。
pub static ErrTxnTooLarge: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassKV.NewStd(errno::ErrTxnTooLarge));
/// 单条键值条目超过 TxnEntrySizeLimit。
pub static ErrEntryTooLarge: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassKV.NewStd(errno::ErrEntryTooLarge));
/// 键本身字节长度过大。
pub static ErrKeyTooLarge: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassKV.NewStd(errno::ErrKeyTooLarge));
/// 唯一键冲突（映射到 MySQL ErrDupEntry）。
pub static ErrKeyExists: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassKV.NewStd(errno::ErrDupEntry));
/// 功能尚未实现。
pub static ErrNotImplemented: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassKV.NewStd(errno::ErrNotImplemented));

/// 存储层写冲突；可重试，消息带 TxnRetryableMark。
pub static ErrWriteConflict: LazyLock<Box<errors::Error>> = LazyLock::new(|| {
    let source = &errno::MySQLErrName[&errno::ErrWriteConflict];
    let message = parser_mysql::Message(
        &format!("{} {}", source.Raw, TxnRetryableMark),
        &source.RedactArgPos,
    );
    dbterror::ClassKV.NewStdErr(errno::ErrWriteConflict, &message)
});

/// TiDB 层检测到的写冲突；同样视为可重试。
pub static ErrWriteConflictInTiDB: LazyLock<Box<errors::Error>> = LazyLock::new(|| {
    let source = &errno::MySQLErrName[&errno::ErrWriteConflictInTiDB];
    let message = parser_mysql::Message(
        &format!("{} {}", source.Raw, TxnRetryableMark),
        &source.RedactArgPos,
    );
    dbterror::ClassKV.NewStdErr(errno::ErrWriteConflictInTiDB, &message)
});

/// Shared-lock ownership may have been lost while upgrading; the transaction must abort.
pub static ErrSharedLockLost: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrSharedLockLost));

/// 悲观锁（Pessimistic Lock）过期：持锁时间超过 TTL。
pub static ErrLockExpire: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrLockExpire));
/// 键存在性断言失败（Assert Exist/NotExist 与实际不符）。
pub static ErrAssertionFailed: LazyLock<Box<errors::Error>> =
    LazyLock::new(|| dbterror::ClassTiKV.NewStd(errno::ErrAssertionFailed));

/// 判断错误是否为事务可重试类型（含写冲突）。
pub fn IsTxnRetryableError(err: Option<&errors::SharedError>) -> bool {
    let Some(err) = err else {
        return false;
    };
    // 三类原型均带可重试标记，客户端可安全重放事务。
    ErrTxnRetryable.Equal(Some(err))
        || ErrWriteConflict.Equal(Some(err))
        || ErrWriteConflictInTiDB.Equal(Some(err))
}

/// 判断是否为“键不存在”错误。
pub fn IsErrNotFound<'a>(err: impl Into<Option<&'a errors::SharedError>>) -> bool {
    ErrNotExist.Equal(err.into())
}

/// 根据列值与索引名生成唯一键冲突错误。
pub fn GenKeyExistsErr(keyCols: &[String], keyName: &str) -> errors::SharedError {
    ErrKeyExists.FastGenByArgs(&[keyCols.join("-").into(), keyName.into()])
}
