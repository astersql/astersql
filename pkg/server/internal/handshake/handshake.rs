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

// MySQL 握手成功响应（Response41）数据结构。
//
// 对齐客户端认证完成后的初始握手应答字段布局，供连接层解析 capability、
// 用户、默认库、认证插件与可选连接属性。

/// Response41 对应 Go 的成功初始握手响应消息。
/// 字段顺序、集合形状和整数宽度与 Go 定义一致。
#[derive(Default)]
pub struct Response41 {
    /// 连接属性键值（如 _client_name）；未提供时为空表。
    pub attrs: std::collections::HashMap<Vec<u8>, Vec<u8>>,
    /// 认证用户名。
    pub user: Vec<u8>,
    /// 默认数据库名（可为空）。
    pub db_name: Vec<u8>,
    /// 选用的认证插件名（如 mysql_native_password）。
    pub auth_plugin: Vec<u8>,
    /// 认证载荷字节（插件相关，可能是 scramble 响应）。
    pub auth: Vec<u8>,
    /// zstd 压缩等级（capability 含压缩时有效）。
    pub zstd_level: isize,
    /// 客户端能力标志位（capability flags）。
    pub capability: u32,
    /// 连接校对集 ID。
    pub collation: u8,
}
