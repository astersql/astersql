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

// Server 标准错误到 errno 的绑定表。
//
// 本文件由 `pkg/server/err/error.go` 迁移而来，保留 Go 错误声明顺序；
// 每个静态值通过 ClassServer.NewStd 绑定对应 MySQL/TiDB 错误码与消息模板。

// 本文件由 pkg/server/err/error.go 迁移而来，保留 Go 错误声明顺序。
// Server error to errno bindings.

use std::sync::LazyLock;

use crate::{dbterror, errno};

// LazyLock 对应 Go 包初始化期只创建一次的标准错误值。
/// 声明一个 ClassServer 标准错误静态项，名称与 errno 码标识一致。
macro_rules! server_error {
    ($(#[$meta:meta])* $name:ident, $code:ident) => {
        $(#[$meta])*
        pub static $name: LazyLock<dbterror::Error> =
            LazyLock::new(|| dbterror::ClassServer.NewStd(errno::$code));
    };
}

server_error!(/// 非法类型。
    ErrInvalidType, ErrInvalidType);
server_error!(/// 非法协议序号（packet sequence）。
    ErrInvalidSequence, ErrInvalidSequence);
server_error!(/// 当前连接状态下不允许该命令。
    ErrNotAllowedCommand, ErrNotAllowedCommand);
server_error!(/// 访问被拒绝（含密码场景）。
    ErrAccessDenied, ErrAccessDenied);
server_error!(/// 访问被拒绝（无密码变体，错误码与消息与上者不同）。
    ErrAccessDeniedNoPassword, ErrAccessDeniedNoPassword);
server_error!(/// 连接数已达上限。
    ErrConCount, ErrConCount);
server_error!(/// 单用户连接数过多。
    ErrTooManyUserConnections, ErrTooManyUserConnections);
server_error!(/// 要求使用安全传输（如 TLS）。
    ErrSecureTransportRequired, ErrSecureTransportRequired);
server_error!(/// 用户名前缀不匹配。
    ErrUserPrefixMismatch, ErrUserPrefixMismatch);
server_error!(/// 多语句执行被禁用。
    ErrMultiStatementDisabled, ErrMultiStatementDisabled);
server_error!(/// 新建连接被中止。
    ErrNewAbortingConnection, ErrNewAbortingConnection);
server_error!(/// 不支持的认证模式。
    ErrNotSupportedAuthMode, ErrNotSupportedAuthMode);
server_error!(/// 网络包过大。
    ErrNetPacketTooLarge, ErrNetPacketTooLarge);
server_error!(/// 必须先修改密码（沙箱/过期密码策略）。
    ErrMustChangePassword, ErrMustChangePassword);
server_error!(/// 服务器正在关闭。
    ErrServerShutdown, ErrServerShutdown);

/// 按 Go 包变量声明顺序，在错误注册表冻结前构造全部 Server 标准错误。
pub(crate) fn initialize_server_errors() {
    LazyLock::force(&ErrInvalidType);
    LazyLock::force(&ErrInvalidSequence);
    LazyLock::force(&ErrNotAllowedCommand);
    LazyLock::force(&ErrAccessDenied);
    LazyLock::force(&ErrAccessDeniedNoPassword);
    LazyLock::force(&ErrConCount);
    LazyLock::force(&ErrTooManyUserConnections);
    LazyLock::force(&ErrSecureTransportRequired);
    LazyLock::force(&ErrUserPrefixMismatch);
    LazyLock::force(&ErrMultiStatementDisabled);
    LazyLock::force(&ErrNewAbortingConnection);
    LazyLock::force(&ErrNotSupportedAuthMode);
    LazyLock::force(&ErrNetPacketTooLarge);
    LazyLock::force(&ErrMustChangePassword);
    LazyLock::force(&ErrServerShutdown);
}
