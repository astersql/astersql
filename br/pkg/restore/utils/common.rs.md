# `br/pkg/restore/utils/common.rs`

## 文件定位

属于 `astersql-br-pkg-restore-utils`；本目录 `lib.rs` 通过 `pub mod common` 和 `pub use common::*` 导出。本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准；增量比较为 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`，不采用工作区或索引中的后续代码。

## 核心职责

表示不可变备份源与恢复目标的分离计划，保存建表后旧表、新表和 rewrite rule 的对应，并提供目标库名称回退、库设置兼容与确定性源库优先比较。

## 主要符号

`DatabaseRestorePlan { Source, Target, Reused }`；`TableRestorePlan { Source, TargetDB, TargetInfo }` 及 `TargetTable`；`CreatedTable { RewriteRule, Table, OldTable, TargetDB }` 及 `TargetDBName`；`DatabaseSettingsCompatible`、`SourceDBPrecedes`。两个计划派生 Clone/Debug/Default，CreatedTable 的前三个字段仍为 Option<Box<_>>。

## 执行流程

调用者填入源与克隆目标元数据；`TargetTable` 仅克隆 TargetDB/TargetInfo 组合成目标表。CreatedTable 由调用方关联旧表、目标表及规则；`TargetDBName` 优先返回非空 TargetDB.O，否则返回 OldTable 的库名，没有旧表则返回原空 TargetDB。比较函数不修改输入。

## 数据与状态

`Reused` 属于目标库，不属于源库；同一源库可能拆到多个目标库，不能把复用标记错误地附着到源。库兼容要求 charset/collate ASCII 忽略大小写相等，placement policy 的可选 `.L` 值相等。源库优先按 Name.L 字典序，再按 ID 比较，相同键不优先。

## 依赖与调用关系

使用本 crate `rewrite_rule` 与 `stubs::{metautil,model}`。冻结 `git grep` 显示公共兼容/优先比较函数没有生产调用。快照 `client.rs` 使用 `snap_client::stubs` 内另外的 DatabaseRestorePlan/TableRestorePlan/CreatedTable，不能将其实际建表调用归到本文件类型。

## 错误处理与边界

本文件无 Result，也不校验计划是否源目标匹配。可空 CreatedTable 字段由消费者负责处理。`TargetTable` 不携带 Go 源表的文件、checksum、统计等源专属数据；不能把目标 DDL 元数据作为备份源替代。

## 并发与资源生命周期

不创建线程、锁或 I/O。计划拥有元数据，TargetTable 返回克隆；CreatedTable 用 Box/Option 表达可选所有权。外部连接和恢复生命周期均不由此文件管理。

## 与 Go 版本的对应关系

对应 `common.go` 新增计划、TargetDBName 及两个比较函数。Go 计划使用指针和真实 metautil/model，Rust 使用本地 stub 值和克隆；Go policy 取 `.Name.L`，本地 stub 直接存 CIStr 并取 `.L`。Rust snap-client 仍使用另一组同名类型，公开导出不等于主链已迁移。

## 扩展指南

修改源目标字段时需同时审计 snap-client 同名类型及消费者，避免丢失备份 ID 或文件。新增用例应放独立测试文件；不要内嵌测试，也不要仅凭同名结构将两个 crate 的类型混用。

## 验证依据

本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准；增量比较为 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`，不采用工作区或索引中的后续代码。 已审计本文件增量与完整源码、`common.go`、`lib.rs`/`Cargo.toml` 和 `stubs.rs` 对应元数据定义；冻结引用搜索检查 `DatabaseSettingsCompatible`/`SourceDBPrecedes` 及快照类型导入。`snap_client/client_test.rs::test_create_tables_with_plan_preserves_source_metadata` 只覆盖快照 stub 计划，不能算本类型直接测试。本目录无 common 专用测试挂载，未运行 Cargo；图仅检查状态，冻结 git blob 为事实依据。
