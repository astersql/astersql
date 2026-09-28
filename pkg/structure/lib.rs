// Copyright 2026 AsterSQL.

// 结构化 KV 封装：在底层键值存储之上提供 String / List / Hash 等类 Redis 语义。
//
// 通过 `TxStructure` 把业务键编码成带类型前缀的内部 key，再经 `kv` 读写。
// 本 crate 用 `include!` 装配实现文件，并 re-export 依赖中的错误码与 KV 接口。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 错误类型与错误码 re-export。
pub mod errors {
    pub use dbterror_dependency::errors::*;
}
/// KV 存储抽象 re-export。
pub mod kv {
    pub use kv_crate::*;
}
/// 编解码工具 re-export。
pub mod codec {
    pub use codec_dependency::*;
}
/// 数据库错误构造 re-export。
pub mod dbterror {
    pub use dbterror_dependency::dbterror::*;
}
/// MySQL 错误号常量 re-export。
pub mod mysql {
    pub use dbterror_dependency::errno::*;
}

/// 核心结构体与公共 API（structure.rs）。
mod structure_impl {
    use crate::{dbterror, errors, kv, mysql};
    include!("structure.rs");
}
pub use structure_impl::*;
/// 键类型与编码相关实现（type.rs）。
mod type_impl {
    use crate::{codec, errors, kv, *};
    include!("type.rs");
}
pub use type_impl::*;
/// String 语义实现（string.rs）；方法经 structure_impl 的 TxStructure 暴露。
mod string_impl {
    use crate::{errors, kv, *};
    include!("string.rs");
}
/// List 语义实现（list.rs）。
mod list_impl {
    use crate::{errors, kv, *};
    include!("list.rs");
}
/// Hash 语义实现（hash.rs）。
mod hash_impl {
    use crate::{errors, kv, *};
    include!("hash.rs");
}
pub use hash_impl::{
    HashPair, NewHashReverseIter, NewHashReverseIterBeginWithField, ReverseHashIterator,
};

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "structure_test.rs"]
mod structure_test;
