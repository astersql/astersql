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

// SQL 错误消息扩展：按配置中的正则为匹配错误追加文档后缀。
//
// 对应 Go `pkg/util/errmsg`。常用于云端/受限特性场景，把原始 MySQL 风格
// `SQLError.Message` 补上文档链接等说明。只应用第一个匹配规则。

// 本文件由 pkg/util/errmsg/errmsg.go 迁移而来，保留 Go 实现结构。
//

// Extend 对应 Go 的同名函数：按配置中的 regexp 快照为选中的 SQL 错误追加后缀。
// Go 通过 *mysql.SQLError 原地修改 Message；这里用 Option<&mut ...> 表达 nil 防御。
use crate::{config, parser::mysql};

/// 按全局配置中的错误消息扩展规则，为匹配的 `SQLError` 追加后缀。
///
/// `None` 对应 Go 的 nil，直接返回；空后缀规则跳过。
pub fn Extend(m: Option<&mut mysql::SQLError>) {
    let Some(m) = m else {
        // Go 中 nil 错误直接返回，避免访问 m.Message。
        return;
    };

    let extensions = config::get_error_message_extensions();
    if extensions.is_empty() {
        return;
    }

    for extension in extensions {
        if extension.suffix.is_empty() {
            continue;
        }

        // Go 只应用第一个匹配的 regexp；成功追加后立刻返回。
        if extension.matches(&m.Message) {
            extendErrorMessage(m, &extension.suffix);
            return;
        }
    }
}

// extendErrorMessage 对应 Go 的私有辅助函数：去掉原消息和后缀尾部句点，再按固定格式补句点。
/// 去掉原消息与后缀尾部句点后，按 `"{msg}, {suffix}."` 格式写回 Message。
fn extendErrorMessage(m: &mut mysql::SQLError, msg: &str) {
    m.Message = format!(
        "{}, {}.",
        m.Message.trim_end_matches('.'),
        msg.trim_end_matches('.')
    );
}
