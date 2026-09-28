// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Lightning 配置默认常量与 gRPC keepalive 参数。
//
// 定义导入批大小、Region（数据分片）切分阈值、读块大小等默认值，
// 以及与 TiKV/PD 通信时的 gRPC keepalive 默认配置。

use std::time::Duration;

use crate::ByteSize;

/// 1 KiB（1024 字节），用于拼装更大容量单位。
const KIB: i64 = 1024;
/// 1 MiB。
const MIB: i64 = 1024 * KIB;
/// 1 GiB。
const GIB: i64 = 1024 * MIB;

/// 默认批导入比例：控制单次导入占用目标 Region 容量的比例上限。
pub const DEFAULT_BATCH_IMPORT_RATIO: f64 = 0.75;
/// 读取数据源时的默认块大小（64 KiB）。
pub const READ_BLOCK_SIZE: ByteSize = ByteSize(64 * KIB);
/// 触发 Region 预切分的默认数据量阈值（96 MiB）。
pub const SPLIT_REGION_SIZE: ByteSize = ByteSize(96 * MIB);
/// 触发 Region 预切分的默认键数量阈值。
pub const SPLIT_REGION_KEYS: i32 = 960_000;
/// Region 切分大小相对默认值的最大倍率上限。
pub const MAX_SPLIT_REGION_SIZE_RATIO: i32 = 10;
/// MySQL 协议 `max_allowed_packet` 默认值（64 MiB）。
pub(crate) const DEFAULT_MAX_ALLOWED_PACKET: u64 = 64 * MIB as u64;

/// gRPC keepalive 探测参数：空闲多久发 ping、等待超时、是否允许无活跃流时探测。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GrpcKeepaliveParams {
    /// 两次 keepalive ping 之间的空闲时间。
    pub time: Duration,
    /// 等待 ping 响应的超时时间。
    pub timeout: Duration,
    /// 无活跃 RPC 流时是否仍发送 keepalive。
    pub permit_without_stream: bool,
}

/// 默认 gRPC keepalive：60s 发 ping，120s 超时，不在无流时探测。
pub const DEFAULT_GRPC_KEEPALIVE_PARAMS: GrpcKeepaliveParams = GrpcKeepaliveParams {
    time: Duration::from_secs(60),
    timeout: Duration::from_secs(120),
    permit_without_stream: false,
};

/// 缓冲区相对批大小的缩放系数。
pub static BUFFER_SIZE_SCALE: i64 = 5;
/// 默认批处理数据量上限（100 GiB）。
pub static DEFAULT_BATCH_SIZE: ByteSize = ByteSize(100 * GIB);
/// 单个 Region 允许的最大数据量（256 MiB）。
pub static MAX_REGION_SIZE: ByteSize = ByteSize(256 * MIB);
