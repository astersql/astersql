// Copyright 2026 AsterSQL.

// autoid_service crate 的根模块（crate 入口）。
//
// 本 crate 提供“自增 ID 分配服务”（AutoID Service）的实现：在分布式数据库中，
// 表的自增列（AUTO_INCREMENT）需要一个全局唯一且单调递增的 ID 分配器。
// 为避免每次插入都访问底层存储，该服务通常以“号段”（batch/range）方式
// 预先批量申请一段 ID 缓存在内存中，再逐个分配给客户端，从而降低分配开销。
//
// 模块组织：
// - `autoid`：核心实现，包含 ID 分配逻辑、服务端接口以及与元数据存储的交互；
//   其全部公开项通过 `pub use` 在 crate 根重新导出。
// - `autoid_test` / `migration_aster_unit_test`：仅在测试构建（`#[cfg(test)]`）
//   下编译的单元测试模块。

/// 自增 ID 分配服务的核心实现模块。
mod autoid;
/// 将 `autoid` 模块的所有公开项重新导出到 crate 根，
/// 使外部使用方可以直接通过 `autoid_service::Xxx` 访问。
pub use autoid::*;

// 仅在运行测试时编译的单元测试模块。
#[cfg(test)]
mod autoid_test;

// 迁移自 Go 版本的单元测试：通过 include! 宏把外部文件内容
// 原样嵌入到该测试模块中，仅在测试构建下生效。
#[cfg(test)]
mod migration_aster_unit_test {
    include!("migration_aster_unit_test.rs");
}
