// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TopSQL 资源标签校验工具：在启用 TopSQL 时检查请求是否携带 resource tag。
//
// 从各类 RPC Request 中提取起始键，解码 TiDB 表键前缀中的 table_id；
// 对有表 ID 却缺失标签的请求返回带调用栈的错误，便于排查。

use crate::rpc::Request;
use std::backtrace::Backtrace;

/// TopSQL 资源标签缺失或请求类型未知时的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceTagError(pub String);

impl std::fmt::Display for ResourceTagError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for ResourceTagError {}

/// 若 TopSQL 已启用且标签为空，则对带表 ID 的请求报错。
pub fn check_resource_tag_for_top_sql(
    request: &Request,
    resource_group_tag: &[u8],
    top_sql_enabled: bool,
) -> Result<(), ResourceTagError> {
    // 未启用 TopSQL 或已设置标签时直接通过。
    if !top_sql_enabled || !resource_group_tag.is_empty() {
        return Ok(());
    }
    let Some(start_key) = get_request_start_key(request)? else {
        return Ok(());
    };
    let table_id = decode_table_id(start_key).unwrap_or_default();
    if table_id > 0 {
        return Err(ResourceTagError(format!(
            "{:?} req does not set the resource tag, tid: {}, stack: {}",
            request.command_type(),
            table_id,
            get_stack()
        )));
    }
    Ok(())
}

/// 从 Request 变体中提取可用于定位表的起始键；部分命令无键则返回 None。
pub fn get_request_start_key(request: &Request) -> Result<Option<&[u8]>, ResourceTagError> {
    Ok(match request {
        Request::Get { key, .. } | Request::Cleanup { key, .. } => Some(key),
        Request::Scan { start, .. } | Request::ScanLock { start, .. } => Some(start),
        Request::Prewrite { request, .. } | Request::Flush { request, .. } => request
            .mutations
            .first()
            .map(|mutation| mutation.key.as_slice()),
        Request::Commit { keys, .. }
        | Request::BatchGet { keys, .. }
        | Request::BatchRollback { keys, .. }
        | Request::CheckSecondaryLocks { keys, .. }
        | Request::BufferBatchGet { keys, .. } => keys.first().map(Vec::as_slice),
        Request::PessimisticLock { request, .. } => Some(&request.primary_lock),
        Request::Cop { start_key, .. } | Request::CopStream { start_key, .. } => Some(start_key),
        // 无明确表键或属 Raw/MPP/元数据类命令，跳过标签检查。
        Request::ResolveLock { .. }
        | Request::CheckTxnStatus { .. }
        | Request::PessimisticRollback { .. }
        | Request::TxnHeartBeat { .. }
        | Request::Gc { .. }
        | Request::DeleteRange { .. }
        | Request::RawGet { .. }
        | Request::RawBatchGet { .. }
        | Request::RawPut { .. }
        | Request::RawBatchPut { .. }
        | Request::RawDelete { .. }
        | Request::RawBatchDelete { .. }
        | Request::RawDeleteRange { .. }
        | Request::RawScan { .. }
        | Request::BatchCop { .. }
        | Request::MppConn { .. }
        | Request::MppTask { .. }
        | Request::MppCancel { .. }
        | Request::MppAlive
        | Request::MvccGetByKey { .. }
        | Request::MvccGetByStartTs { .. }
        | Request::SplitRegion { .. }
        | Request::DebugGetRegionProperties { .. }
        | Request::StoreSafeTs
        | Request::UnsafeDestroyRange
        | Request::Empty => None,
    })
}

/// Record and index keys both begin with `t` plus TiDB's comparable int64.
/// The suffix is `_r` for records and `_i` for indexes.
///
/// 解码 TiDB 编码表键：`t` + 可比较 int64 table_id + `_r`/`_i` 后缀。
fn decode_table_id(key: &[u8]) -> Option<i64> {
    if key.len() < 11 || key[0] != b't' || (&key[9..11] != b"_r" && &key[9..11] != b"_i") {
        return None;
    }
    let mut encoded = [0u8; 8];
    encoded.copy_from_slice(&key[1..9]);
    // 翻转最高位，还原可比较编码的有符号 int64。
    encoded[0] ^= 0x80;
    Some(i64::from_be_bytes(encoded))
}

/// 捕获当前调用栈字符串，便于 TopSQL 标签缺失诊断。
pub fn get_stack() -> String {
    Backtrace::force_capture().to_string()
}

/// Go 风格别名：check_resource_tag_for_top_sql。
pub fn checkResourceTagForTopSQL(
    request: &Request,
    resource_group_tag: &[u8],
    top_sql_enabled: bool,
) -> Result<(), ResourceTagError> {
    check_resource_tag_for_top_sql(request, resource_group_tag, top_sql_enabled)
}

/// Go 风格别名：get_request_start_key。
pub fn getReqStartKey(request: &Request) -> Result<Option<&[u8]>, ResourceTagError> {
    get_request_start_key(request)
}

/// Go 风格别名：返回栈帧的 UTF-8 字节。
pub fn getStack() -> Vec<u8> {
    get_stack().into_bytes()
}
