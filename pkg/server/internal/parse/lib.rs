// Copyright 2026 AsterSQL.

// server/internal/parse crate 根模块：握手与命令包解析。
//
// 聚合解析所需的 MySQL 客户端能力位、Response41、长度编码工具与
// 连接属性指标；对外再导出 `parse` 中的握手响应与 COM_STMT_FETCH 解析。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 解析器侧 MySQL 协议常量（客户端 capability 标志位）。
pub mod parser {
    pub mod mysql {
        pub use astersql_parser_mysql::r#const::{
            ClientConnectAtts, ClientConnectWithDB, ClientPluginAuth,
            ClientPluginAuthLenencClientData, ClientSecureConnection,
            ClientZstdCompressionAlgorithm,
        };
    }
}

/// server/internal 依赖：握手结构与长度编码解析工具。
pub mod server {
    pub mod internal {
        /// Response41：成功初始握手响应消息结构。
        pub mod handshake {
            pub use astersql_server_internal_handshake::Response41;
        }
        /// 长度编码整数/字节串解析（与 Go ParseLengthEncoded* 对齐）。
        pub mod util {
            pub use astersql_server_internal_util::{
                ParseLengthEncodedBytes, ParseLengthEncodedInt,
            };
        }
    }
}

/// 会话变量定义：连接属性大小限制与统计指标。
pub mod sessionctx {
    pub mod vardef {
        pub use astersql_sessionctx_vardef::{
            ConnectAttrsLongestSeen, ConnectAttrsLost, ConnectAttrsSize,
        };
    }
}

/// 日志工具再导出。
pub mod util {
    pub mod logutil {
        pub use astersql_util_logutil::*;
    }
}

/// 握手响应与语句 fetch 等包解析实现。
pub mod parse;
pub use parse::*;

#[cfg(test)]
#[path = "handshake_test.rs"]
mod handshake_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "parse_test.rs"]
mod parse_test;
