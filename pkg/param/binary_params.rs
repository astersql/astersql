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

// MySQL 二进制协议参数载体与相关错误。
//
// 二进制协议（Binary Protocol）在 COM_STMT_EXECUTE 等报文中以类型编号、
// null-bitmap 与原始字节传递绑定参数；本模块只保存解码结果，不做网络读入。
use crate::{dbterror, errno, terror};

// ErrUnknownFieldType 对应 Go 包级错误：二进制参数携带未知数据类型时，由 server 错误类生成标准错误。
pub static ErrUnknownFieldType: std::sync::LazyLock<Box<terror::Error>> =
    std::sync::LazyLock::new(|| dbterror::ClassServer.NewStd(errno::ErrUnknownFieldType));
// 保留迁移代码使用的 Rust 风格名称，并确保两个名称指向同一个标准错误。
pub use ErrUnknownFieldType as ERR_UNKNOWN_FIELD_TYPE;

/// BinaryParam 对应 Go 的同名结构，保存从 MySQL 二进制协议解码出的单个参数。
/// 上层 ExecArgs 会继续把这些原始字节转换为表达式；本类型自身不读取网络，也不触发解析。
#[derive(Default)]
pub struct BinaryParam {
    /// Tp 是协议中的单字节 MySQL 类型编号。
    pub Tp: u8,
    /// IsUnsigned 保留类型标志中的无符号语义，影响后续数值解码。
    pub IsUnsigned: bool,
    /// IsNull 表示协议 null-bitmap 已将该参数标记为 NULL，此时 Val 不应被解释为值。
    pub IsNull: bool,
    /// Val 保存协议解码后的原始字节；Vec<u8> 对应 Go 可变长度的 []byte 所有权形状。
    pub Val: Vec<u8>,
}
