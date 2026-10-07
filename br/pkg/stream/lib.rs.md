# `br/pkg/stream/lib.rs`

## 文件定位

源码：[br/pkg/stream/lib.rs](lib.rs)。

`lib.rs` 是 Cargo 包 `astersql-br-pkg-stream` 的 crate 根，而不是流备份算法的实现文件。`br/pkg/stream/Cargo.toml` 的 `[lib] path = "lib.rs"` 将它指定为编译入口，根工作区 `Cargo.toml` 又把 `br/pkg/stream` 列为 workspace member。它对应 Go 包 `br/pkg/stream`，负责把同目录的 Rust 移植文件组织成一个可供恢复、任务编排和 CRR 测试工具使用的库。

该文件当前有三类内容：crate 级 lint 放宽，11 个生产子模块声明，以及 12 个仅测试构建启用的测试模块声明；末尾再把所有生产子模块的公开符号扁平再导出。它没有函数、类型、常量、trait 或业务状态，也不直接包含 `crr/` 与 `backupmetas/`：后两者在 workspace 中是独立 crate，其中 `backupmetas` 由本 crate 作为路径依赖使用。

## 核心职责

1. 用显式 `#[path = "..."]` 把 `stubs.rs`、`decode_kv.rs`、`logging_helper.rs`、`table_history.rs`、`meta_kv.rs`、`search.rs`、`stream_status.rs`、`table_mapping.rs`、`rewrite_meta_rawkv.rs`、`stream_metas.rs` 和 `stream_mgr.rs` 固定到 crate 模块树中（`lib.rs:19-61`）。
2. 在 `cfg(test)` 下挂载 12 个独立测试文件，使测试能够用 `crate::...` 访问内部模块，同时不把测试代码带入普通依赖构建（`lib.rs:63-109`）。这也遵守了 Rust 源码与测试逻辑分文件的仓库约定。
3. 通过 11 条 glob re-export 提供 `astersql_br_pkg_stream::X` 的兼容门面，同时仍保留 `astersql_br_pkg_stream::stream_mgr::X` 之类的具名模块路径（`lib.rs:111-122`）。
4. 以 `#![allow(...)]` 暂时容纳 Go 风格导出名、移植期未使用项和广泛 Clippy 告警（`lib.rs:8-17`）。这是一项 crate 全局策略，会覆盖所有由该根文件装入的子模块，扩展时不应误认为各模块已满足惯用 Rust lint。

## 主要符号

本文件不定义可执行符号；其公共契约由模块和再导出组成：

- `stubs`：移植期依赖边、错误、protobuf 风格结构、`Storage`/`MemStorage` 与 ID 映射结构。它被公开并被外部 crate 直接访问，例如 `astersql_br_pkg_stream::stubs::backuppb::Metadata`；因此不是纯测试模块。
- `decode_kv`：`Iterator`、`EventIterator`、`EncodeKVEntry`、`DecodeKVEntry` 等 KV 事件缓冲接口。
- `logging_helper`：`LogDBReplaceMap`，输出数据库/表替换映射。
- `table_history`：`LogBackupTableHistoryManager`、`TableLocationInfo` 和 `NewTableHistoryManager`，维护 DDL/表历史视图。
- `meta_kv`：`RawMetaKey`、`RawWriteCFValue`、`ParseTxnMetaKeyFrom` 和 WriteCF 类型常量，解析事务元数据键值。
- `search`：`Comparator`、`StreamBackupSearch`、`StreamKVInfo` 和 `NewStreamBackupSearch`，扫描流备份数据并合并 CF 条目。
- `stream_status`：`Checkpoint`、`TaskStatus`、`TaskPrinter` 等状态展示接口。
- `table_mapping`：`TableMappingManager`、`MetaInfoCollector`、`PiTRIdTrackerLookup` 等上游到下游 schema/ID 映射接口。
- `rewrite_meta_rawkv`：`SchemasReplace`、`NewSchemasReplace` 与删除范围记录结构，执行 RawKV 元数据改写。
- `stream_metas`：`StreamMetadataSet`、`Migrations`、`MigrationExt`、shift TS 与 migration 合并/截断辅助。
- `stream_mgr`：`MetadataHelper`、`StreamManager`、备份元数据前缀、路径过滤和快速反序列化入口。

每个 `pub mod` 与对应 `pub use module::*` 共同形成两种稳定访问方式。由于使用 glob re-export，新增同名公开符号可能在 crate 根造成名称冲突，这是修改公共面的首要兼容风险。

## 执行流程

普通构建时，Rust 从 `Cargo.toml` 指定的 `lib.rs` 进入：

1. 应用 crate 级 `allow` 属性。
2. 按 11 条 `#[path] pub mod` 解析并编译生产模块；模块之间可通过 `crate::...` 引用彼此。
3. 跳过所有 `#[cfg(test)]` 声明。
4. 处理末尾 `pub use`，把各模块的公开项加入 crate 根命名空间。
5. 下游 crate 随后可按根路径或具名模块路径调用实际实现。例如 `br/pkg/utiltest/crr/harness.rs` 从根路径导入 `MetadataHelper`，而 `br/pkg/restore/log_client/log_file_manager.rs` 使用 `stream_mgr::MetadataHelper` 和 `stream_metas::TryParseTaggedBackupMetaFileNameWrapper`。

测试构建多一步：12 个 `*_test.rs`/`parity_test.rs` 文件成为 crate 私有测试模块。`parity_test.rs` 同时引用 decode、meta、rewrite、search、metadata、history、mapping 和 stubs，用一个跨模块契约测试检查根模块装配后的协作；其余测试文件按实现模块细分边界行为。`export_test.rs` 是 Rust 测试辅助模块，不会像 Go 的 `export_test.go` 那样参与非测试包。

## 数据与状态

`lib.rs` 自身不分配、缓存或变更任何数据，也没有全局变量、锁、通道或单例。业务状态全部属于被装配模块，例如 `StreamMetadataSet` 保存已加载的 metadata/migration 视图，`StreamManager` 持有存储与备份元数据访问状态，`TableMappingManager` 保存上下游 ID 映射，`LogBackupTableHistoryManager` 保存表历史。

唯一可视为“静态配置”的内容是编译期模块图和 lint 策略：`cfg(test)` 决定测试模块是否存在，`pub use` 决定根命名空间的公开面，`#[path]` 决定源码文件的精确来源。修改这些声明会改变编译可达性或 API 路径，而不是运行时数据。

## 依赖与调用关系

上游依赖关系由 Cargo 和源码共同证明：

- `br/pkg/task/Cargo.toml` 依赖本 crate；`br/pkg/task/restore.rs` 使用 `table_history::{LogBackupTableHistoryManager, TableLocationInfo}`，其测试使用 `NewTableHistoryManager`。
- `br/pkg/restore/log_client/Cargo.toml` 依赖本 crate；`log_file_manager.rs` 调用 `stream_metas` 和 `stream_mgr` 中的 metadata 加载、路径过滤与解析入口。
- `br/pkg/utiltest/crr/Cargo.toml` 依赖本 crate；`flush_sim.rs` 通过根再导出调用 `GetStreamBackupMetaPrefix`，`harness.rs` 通过根再导出使用 `MetadataHelper`，并直接使用 `stubs::backuppb` 类型。

下游依赖在 `br/pkg/stream/Cargo.toml` 中声明。实际源码证据包括：`stream_mgr.rs` 使用 `astersql-br-pkg-encryption`、`astersql-br-pkg-stream-backupmetas` 和 `sha2`；`stream_metas.rs` 使用 `stream-backupmetas` 与 `utils-consts`；`search.rs` 使用 `utils-consts`、`base64` 和 `sha2`；`stream_status.rs` 使用 `regex`；`stubs.rs` 使用 `serde`。清单还声明 `streamhelper`、`utils-iter`、`serde_json`、`hex`、`zstd` 等供整个 crate 的装配模块使用。`lib.rs` 不直接调用这些依赖，因此 RustCodeGraph 对该文件本身不会形成业务 callees；调用边落在各实现模块。

RustCodeGraph 的直接证据还显示：`TryParseTaggedBackupMetaFileNameWrapper` 被 `restore/log_client/log_file_manager.rs` 调用，`NewTableHistoryManager` 被 `br/pkg/task/restore_test.rs` 和本 crate 测试调用，而 `NewStreamBackupSearch`、`GetStreamBackupMetaPrefix`、`MetadataHelper` 均有模块内或外部工具链使用者。这说明门面不是孤立声明，但不同消费者有意混用根再导出和具名模块路径。

## 错误处理与边界

本文件没有 `Result`、`Option`、panic 或错误转换逻辑。语法、模块路径、名称冲突和依赖缺失会在编译阶段失败；运行时错误由实际模块定义和传播，例如 KV 解码拒绝短缓冲、metadata 解析返回错误、存储操作传播 `Storage` 错误。

边界上需要注意：

- `stubs` 被生产态公开装配，当前 crate 仍包含移植期替代类型；不能把门面存在误述为所有 Go 依赖都已接入真实后端。
- `#![allow(dead_code, unused_*, clippy::all, non_*...)]` 会掩盖死代码、未使用导入和命名问题，结构验证不能代替功能验证。
- `cfg(test)` 只证明测试源码在测试构建中可达；本次纯文档任务按计划不运行 Cargo，因此没有重新证明这些测试当前可编译或通过。
- glob re-export 扩大根 API，符号重名会使装配失败或迫使消费者改路径；删除再导出则可能破坏现有根路径调用者。

## 并发与资源生命周期

crate 根没有并发原语、异步任务、线程、锁、通道、文件句柄或显式清理流程，也不会创建或销毁 `StreamManager`/`Storage`。资源生命周期由调用者和具体实现模块管理；`lib.rs` 只决定这些类型是否在编译期可见。

`cfg(test)` 是这里唯一的生命周期式边界：测试模块只存在于测试编译单元，生产依赖不会链接测试辅助逻辑。`stubs` 则相反，它没有 `cfg(test)`，且外部 crate 会引用其类型，因此其生命周期与生产 crate 相同。扩展时若辅助代码只服务测试，应放入独立 `*_test.rs` 并在此受 `cfg(test)` 控制，不能无意加入公开 `stubs`。

## 与 Go 版本的对应关系

Go 的 `br/pkg/stream` 没有与 `lib.rs` 一一对应的单个入口文件；同目录 16 个 `.go` 文件使用 `package stream`，由 Go 工具链自动聚合公开标识符。Rust 必须用 `lib.rs` 显式声明模块，再用 `pub use` 模拟 Go 包中 `stream.X` 的扁平访问方式。

主要文件保持直接映射：`decode_kv.rs`/`.go`、`logging_helper.rs`/`.go`、`table_history.rs`/`.go`、`meta_kv.rs`/`.go`、`search.rs`/`.go`、`stream_status.rs`/`.go`、`table_mapping.rs`/`.go`、`rewrite_meta_rawkv.rs`/`.go`、`stream_metas.rs`/`.go`、`stream_mgr.rs`/`.go`。Rust 额外有 `stubs.rs` 和本 `lib.rs`，反映迁移期的显式 crate 边界与未完全落地依赖。

测试也按文件对应：Rust 根挂载 `decode_kv_test.rs`、`meta_kv_test.rs`、`rewrite_meta_rawkv_test.rs`、`search_test.rs`、`stream_metas_test.rs`、`stream_mgr_fuzz_test.rs`、`stream_misc_test.rs`、`table_mapping_test.rs` 等；Go 同目录存在对应 `*_test.go`。Rust 还提供 `parity_test.rs` 验证跨模块 Go/Rust 公开契约，以及 `stream_mgr_test.rs`、`stream_status_test.rs` 等独立覆盖。两边文件数量和测试拆分并非完全相同，语义核对应以实际函数和边界用例为准，不能只按文件名判断等价。

## 扩展指南

- 新增生产实现文件时，在本文件增加显式 `#[path] pub mod`；只有确实需要兼容 Go 包式根 API 时才增加 `pub use`。先检查新公开名是否与现有 11 组 glob re-export 冲突。
- 新增或修改算法应在对应实现文件完成，不要把业务逻辑塞进 crate 根。测试放在独立 `*_test.rs` 文件，并用 `#[cfg(test)] #[path = "..."] mod ...;` 挂载；同时对照相应 Go 源码和 Go 测试保持行为与边界一致。
- 若扩展 metadata 读取/迁移，优先修改 `stream_mgr.rs` 或 `stream_metas.rs`，同步 `stream_mgr_test.rs`、`stream_misc_test.rs`、`stream_metas_test.rs` 和必要的 `parity_test.rs`。
- 若扩展 KV 编解码、搜索、映射或改写，分别进入 `decode_kv.rs`、`search.rs`、`table_mapping.rs`、`rewrite_meta_rawkv.rs`，同步同名独立测试及对应 Go 测试。
- 若要移除桩类型，先用 RustCodeGraph/`rg` 清点外部 `astersql_br_pkg_stream::stubs` 使用者并迁移到真实 crate；`br/pkg/restore/log_client` 与 `br/pkg/utiltest/crr` 当前都有直接引用，不能只从本 crate 内部判断可删。
- 修改公开模块或根再导出属于兼容性变更；需要检查 `br/pkg/task`、`br/pkg/restore/log_client`、`br/pkg/utiltest/crr` 等下游。性能风险通常不在门面本身，而在新接入实现是否引入额外复制、全量 metadata 扫描或阻塞存储 I/O。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖本仓库 7032 个 Rust 文件；`files --filter br/pkg/stream` 确认本 crate 的实现、Go 对照与独立测试集合；`node --file br/pkg/stream/lib.rs --offset 1 --limit 200` 核对 122 行完整模块图；`explore` 查询 `GetStreamBackupMetaPrefix MetadataHelper NewTableHistoryManager NewStreamBackupSearch`，核对模块内外调用者。
- Rust 源与清单：`br/pkg/stream/lib.rs`、`br/pkg/stream/Cargo.toml`、根 `Cargo.toml`；下游边界由 `br/pkg/task/Cargo.toml`、`br/pkg/restore/log_client/Cargo.toml`、`br/pkg/utiltest/crr/Cargo.toml` 核验。
- 直接调用证据：`br/pkg/task/restore.rs`、`br/pkg/restore/log_client/log_file_manager.rs`、`br/pkg/utiltest/crr/flush_sim.rs`、`br/pkg/utiltest/crr/harness.rs`。
- Go 对照：同目录 `decode_kv.go`、`logging_helper.go`、`table_history.go`、`meta_kv.go`、`search.go`、`stream_status.go`、`table_mapping.go`、`rewrite_meta_rawkv.go`、`stream_metas.go`、`stream_mgr.go`，以及对应 `*_test.go`。
- Rust 测试：`parity_test.rs`、`decode_kv_test.rs`、`export_test.rs`、`meta_kv_test.rs`、`rewrite_meta_rawkv_test.rs`、`search_test.rs`、`stream_metas_test.rs`、`stream_mgr_fuzz_test.rs`、`stream_mgr_test.rs`、`stream_misc_test.rs`、`stream_status_test.rs`、`table_mapping_test.rs`。其中 `export_test.rs` 是被 `lib.rs` 挂载的测试辅助模块，其余 11 个文件包含根契约及细分行为覆盖。
- 本任务仅新增说明文档，按计划不运行 Cargo；完成判据是上述事实复核、人工检查无“门面即完整实现”的臆测，以及任务规定的 11 章节结构命令退出码为 0。
