// Copyright 2026 AsterSQL.

// 将上级 lockstore 实现挂入本 MVCC crate。
//
// 通过 `#[path]` 引用 `unistore/lockstore` 的 arena、迭代器、加载转储与
// MemStore，再统一 `pub use`，避免重复实现锁键值存储。

// 路径挂载 lockstore 各子模块源文件。
#[path = "../../lockstore/arena.rs"]
mod arena;
#[path = "../../lockstore/iterator.rs"]
mod iterator;
#[path = "../../lockstore/load_dump.rs"]
mod load_dump;
#[path = "../../lockstore/lockstore.rs"]
mod lockstore;

/// 再导出 arena / 迭代器 / 加载转储 / MemStore 全部公开项。
pub use arena::*;
pub use iterator::*;
pub use load_dump::*;
pub use lockstore::*;
