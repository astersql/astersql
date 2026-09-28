// Copyright 2026 AsterSQL.

// 数据一致性（consistency）诊断包入口。
//
// 对应 Go 侧一致性报告辅助：在索引回查 / admin check 发现表行与索引
// 不一致时，采集 MVCC（多版本并发控制）快照并写入诊断日志。
// 本 crate 以 `reporter` 为实现主体，测试通过 `include!` 引入迁移补充用例。

/// 一致性报告器与 MVCC 解码实现。
mod reporter;
pub use reporter::*;

/// 迁移补充单元测试：对齐 Go 的 MVCC JSON、脱敏与截断行为。
#[cfg(test)]
mod migration_aster_unit_test {
    include!("migration_aster_unit_test.rs");
}
