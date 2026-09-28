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

// 校验 errcode / errname 与 Go 协议常量、共享消息表的一致性。
//
// 覆盖错误码边界值、Message 构造保留原始模板与脱敏下标，以及
// MySQLErrName 懒加载后表长度与代表性条目内容。

use super::errcode::*;
use super::errname::{Message, MySQLErrName};

/// 固定 MySQL 经典区间首尾、MariaDB 与 TiDB 扩展码，防止迁移时被“纠正”。
#[test]
fn error_code_boundaries_match_go_protocol_values() {
    // ErrErrorFirst/ErrHashchk 均为 1000；ErrErrorLast 与最后一条经典码同值 1863。
    assert_eq!(ErrErrorFirst, 1000);
    assert_eq!(ErrHashchk, ErrErrorFirst);
    assert_eq!(ErrErrorLast, 1863);
    // MariaDB 分区相关码与 TiDB 优化器 hint 码落在经典区间之外。
    assert_eq!(ErrOnlyOneDefaultPartionAllowed, 4030);
    assert_eq!(ErrWarnOptimizerHintWrongPos, 8066);
}

/// Message 只拷贝模板与脱敏位置，不执行格式化。
#[test]
fn message_preserves_raw_template_and_redaction_positions() {
    let message = Message("secret %s at %d", &[0, 2]);

    assert_eq!(message.Raw, "secret %s at %d");
    assert_eq!(message.RedactArgPos, vec![0, 2]);
}

/// MySQLErrName 必须是单例共享 map，条目数与抽样消息与 Go 一致。
#[test]
fn mysql_error_names_are_complete_and_shared() {
    let first: &'static _ = MySQLErrName();
    let second: &'static _ = MySQLErrName();

    assert_eq!(first.len(), 952);
    assert!(
        std::ptr::eq(first, second),
        "the Go package exposes one shared map"
    );
    // DupEntry / NoDB / TiDB hint 代表经典码、状态相关码与扩展码三类消息。
    assert_eq!(
        first[&ErrDupEntry].Raw,
        "Duplicate entry '%-.64s' for key '%-.192s'"
    );
    assert_eq!(first[&ErrNoDB].Raw, "No database selected");
    assert_eq!(
        first[&ErrWarnOptimizerHintWrongPos].Raw,
        "Optimizer hint can only be followed by certain keywords like SELECT, INSERT, etc."
    );
    assert!(first[&ErrDupEntry].RedactArgPos.is_empty());
}
