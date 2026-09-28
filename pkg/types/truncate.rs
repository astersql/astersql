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

// 截断（truncate）错误处理：按会话标志决定忽略、转警告或直接返回错误。
//
// 对齐 Go `types.Context.HandleTruncate`：先剥到根因（Cause），再按 errno
// 判断是否属于截断类错误，最后按 IgnoreTruncateErr / TruncateAsWarning 优先级处置。

use crate::{Context, errno, errors};

/// 判断错误是否为 MySQL 语义下的截断/越界类 SQL 错误码。
fn is_truncate_error(error: &errors::SharedError) -> bool {
    let Some(sql_error) = error.downcast_ref::<errors::Error>() else {
        return false;
    };
    // 与 Go 侧截断相关 errno 列表保持一致
    const TRUNCATE_CODES: [i32; 10] = [
        errno::ErrTruncatedWrongValue as i32,
        errno::ErrDataTooLong as i32,
        errno::ErrTruncatedWrongValueForField as i32,
        errno::ErrWarnDataOutOfRange as i32,
        errno::ErrDataOutOfRange as i32,
        errno::ErrBadNumber as i32,
        errno::ErrWrongValueForType as i32,
        errno::ErrDatetimeFunctionOverflow as i32,
        errno::WarnDataTruncated as i32,
        errno::ErrIncorrectDatetimeValue as i32,
    ];
    TRUNCATE_CODES.contains(&sql_error.Code())
}

impl Context {
    /// 处理可选的截断错误：无错误、非截断错误、忽略、警告或向上返回。
    pub fn HandleTruncate(
        &mut self,
        error: Option<errors::SharedError>,
    ) -> Result<(), errors::SharedError> {
        let Some(error) = error else {
            return Ok(());
        };

        // 剥到根因后再分类，避免包装层掩盖真实 errno
        let cause = errors::Cause(Some(&error)).unwrap_or(error);
        if !is_truncate_error(&cause) {
            return Err(cause);
        }
        // IgnoreTruncateErr 优先于 TruncateAsWarning
        if self.Flags().IgnoreTruncateErr() {
            return Ok(());
        }
        if self.Flags().TruncateAsWarning() {
            self.AppendWarning(cause);
            return Ok(());
        }
        Err(cause)
    }
}
