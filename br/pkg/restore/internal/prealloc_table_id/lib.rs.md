# `br/pkg/restore/internal/prealloc_table_id/lib.rs`

## 文件定位

本文件是 Cargo library crate `astersql-br-pkg-restore-internal-prealloc-table-id` 的根入口。`br/pkg/restore/internal/prealloc_table_id/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，并在 `package.metadata.porting.go-package` 中把该 crate 对应到 Go 包 `br/pkg/restore/internal/prealloc_table_id`。crate 只直接依赖 `sha2 = "0.10"`，没有 feature 声明。

它不是预分配算法的实现文件，而是一个薄门面：`#[path = "alloc.rs"] pub mod alloc` 把同目录实现挂到公开模块 `alloc`，`pub use alloc::*` 再把实现中的公共常量、类型、trait 和函数提升到 crate 根。生产调用方因此可以写 `astersql_br_pkg_restore_internal_prealloc_table_id::PreallocIDs`，也可以显式走 `::alloc::PreallocIDs`。

在测试构建中，本文件还用 `#[cfg(test)]` 分别挂载 `parity_test.rs` 和 `alloc_test.rs`。测试逻辑保持在独立文件中，不进入生产构建。文件顶部的宽泛 `allow` 属性作用于整个 crate，用于容纳从 Go 迁移而保留的命名和当前尚未被所有生产路径消费的接口。

## 核心职责

1. 建立 crate 的公开模块边界，把 `alloc.rs` 作为唯一生产实现模块。
2. 通过通配再导出提供扁平公共 API，使依赖 crate 无需知道内部文件布局。
3. 仅在 `cfg(test)` 下注册两个独立测试模块，分别覆盖 Go 对齐用例和更细的契约/错误分支。
4. 在 crate 根统一接受 Go 风格标识符、未使用迁移接口和 Clippy 告警；这是一项迁移兼容策略，不是算法行为。

实际业务职责由 `alloc.rs` 承担：收集表及分区原 ID，向抽象全局 ID 分配器预占区间，建立旧 ID 到新 ID 的映射，重写表元数据，并创建或复用 checkpoint。`lib.rs` 本身不分配 ID、不持有状态，也不执行恢复流程。

## 主要符号

- `pub mod alloc`：以显式 `#[path = "alloc.rs"]` 声明的公开生产模块。模块路径稳定为 crate 的 `alloc` 子模块。
- `pub use alloc::*`：把 `alloc.rs` 的所有公开项再导出到 crate 根。当前主要 API 包括 `Allocator`、`PreallocIDs`、`New`、`NewAndPrealloc`、`ReuseCheckpoint`、`InsaneTableIDThreshold`、`Error`、`Result`，以及本地最小替身模块 `checkpoint`、`model`、`metautil`、`errors`、`berrors`。
- `mod parity_test`：测试构建专用的私有模块，来自 `parity_test.rs`；覆盖正常预分配、空输入、错误包装、重复分配保护、哈希与 checkpoint 复用。
- `mod alloc_test`：测试构建专用的私有模块，来自 `alloc_test.rs`；复刻 Go `TestAllocator` 和 `TestAllocatorBound` 的区间与重写断言。
- crate 级 `#![allow(...)]`：允许 `dead_code`、Go 风格大小写、未使用项及全部 Clippy lint。它影响该 crate 所挂载的实现和测试模块，但不改变运行时控制流。

本文件没有自定义常量、结构体、trait、函数或 `impl`，也没有 feature 条件；公共业务符号全部来自 `alloc::*`。

## 执行流程

编译阶段先把 `alloc.rs` 解析为公开模块，再把其中公共项放入 crate 根命名空间。下游 `internal/prealloc_db` 的 `db.rs` 从 crate 根导入 `PreallocIDs`，其 `rewrite_table_info` 把本地完整 `model::TableInfo` 转成预分配 crate 的精简模型，调用 `PreallocIDs::RewriteTableInfo`，然后把表 ID 与分区 ID 写回克隆结果。`DB::RegisterPreallocatedIDs` 保存映射，后续建表/DDL 路径使用它避免旧 ID 与目标集群已有全局 ID 冲突。

实现模块中的典型算法链为：`New(tables)` 收集并排序表/分区 ID，计算 SHA-256 哈希及可复用边界；`PreallocIDs::PreallocIDs(allocator)` 读取当前全局 ID 水位，保留仍安全的原 ID，对已占用或异常大的 ID 分配连续新 ID，并通过 `AdvanceGlobalIDs` 一次推进全局水位；`RewriteTableInfo` 克隆输入后依次改写表与分区 ID；`CreateCheckpoint` 保存区间、边界和哈希。便捷入口 `NewAndPrealloc` 组合前两步，`ReuseCheckpoint` 则验证表集合与旧 checkpoint 后重建映射。

当执行测试时，`cfg(test)` 额外编译两个测试模块；正常生产构建不会包含其中的夹具或测试辅助入口。`lib.rs` 不负责选择测试、初始化分配器或调度恢复任务。

## 数据与状态

`lib.rs` 自身没有全局变量、缓存、锁或可变状态。它只决定符号可见性和测试模块是否参与编译。

再导出的核心状态 `PreallocIDs` 位于 `alloc.rs`，包含半开区间 `start..end`、可复用边界 `reusable_border`、已排序原 ID 集合的 SHA-256 `hash`、分配前暂存的 `unalloced_ids`，以及旧 ID 到目标 ID 的 `alloc_rule`。`unalloced_ids = Some(...)` 表示尚未完成预分配；成功推进全局 ID 后设为 `None`。空表输入使用 `start = i64::MAX`、`end = 0` 表示空区间。

`Allocator` trait 只要求 `GetGlobalID` 与 `AdvanceGlobalIDs`，把持久化水位的所有权留给上层。`model::TableInfo`、`PartitionInfo`、`PartitionDefinition`、`metautil::Table` 和 `checkpoint::PreallocIDs` 都是该 crate 为当前迁移范围定义的最小本地数据形状，不等同于完整 TiDB 模型。

## 依赖与调用关系

crate 的唯一外部 Cargo 依赖是 `sha2`，供 `alloc.rs::computeSortedIDsHash` 对排序后的每个 `i64` 以大端字节计算 SHA-256。标准库依赖为 `HashMap`、格式化和错误 trait。`lib.rs` 不直接调用这些依赖，只公开承载实现的模块。

已核对的直接 Rust 依赖方是 `br/pkg/restore/internal/prealloc_db`：其 `Cargo.toml` 以路径 `../prealloc_table_id` 声明依赖；`db.rs` 使用 crate 根再导出的 `PreallocIDs`，并在 `rewrite_table_info` 中调用其重写方法。该 crate 的 `db_test.rs` 和 `parity_test.rs` 还从根导入 `Allocator`、`New`、`PreallocIDs` 验证集成语义。

`br/pkg/restore/snap_client/client.rs::AllocTableIDs` 展示了更上层的恢复主链，但当前该文件导入的是自身 `stubs.rs` 中的 `NewAndPreallocTableIDs`、`ReusePreallocatedTableIDs` 和另一套 `PreallocIDs`，并未通过 Cargo 依赖直接调用本 crate。因此它只能作为 Go 恢复流程和迁移目标的背景证据，不能算 `lib.rs` 当前已接线的直接调用边。

RustCodeGraph 对 `lib.rs` 的“used by”只报告 `tools/tazel/parity_test.rs`，而符号查询能定位 `prealloc_db` 的实际 crate 根导入；这说明文件级图对通配再导出和跨 crate 符号使用覆盖有限，调用关系以 Cargo manifest 与源码导入交叉核验。

## 错误处理与边界

门面文件没有可失败的运行时操作。编译边界上的主要风险是通配再导出：`alloc.rs` 新增任何 `pub` 项都会自动成为 crate 根 API，可能引入命名冲突或意外扩大兼容承诺。删除或改名公共项则会同时破坏 `::alloc::X` 与 crate 根 `::X` 两种访问路径。

再导出的算法会拒绝超出阈值约束的 ID 集合、未分配时调用 `AllocID`、映射缺失或超出预占区间、空 checkpoint、checkpoint 边界/哈希不匹配、空表信息，以及底层分配器失败。`NewAndPrealloc` 分别为收集阶段和分配阶段添加上下文；`RewriteTableInfo` 为表 ID 与分区 ID 失败添加原 ID 信息。异常大的单个 ID 会被重写，但用于确定可复用边界的正常最大 ID 必须保证加上待分配数量后不越过 `InsaneTableIDThreshold`。

宽泛的 `#![allow(clippy::all)]` 会隐藏新的静态告警。扩展时不能把“没有 lint”当成正确性证据，尤其要人工检查整数区间、`usize` 转换、通配导出和 Go 风格 API 的兼容性。

## 并发与资源生命周期

本文件没有并发原语或资源生命周期。模块在编译时静态组装，`pub use` 不复制状态；测试模块只在测试二进制中存在。

实现层的 `PreallocIDs` 也不包含 `Arc`、`Mutex`、通道或异步任务，其变更方法要求 `&mut self`，`Allocator` 也以 `&mut dyn Allocator` 传入，因此一次预分配在 Rust 类型层面是独占、同步操作。是否需要事务、锁或持久化原子性由具体 `Allocator` 实现和上层恢复会话保证；本地 trait 本身不提供回滚。如果 `AdvanceGlobalIDs` 失败，`start` 和 `alloc_rule` 已可能在内存中被更新而 `end` 尚未提交，调用方应把该对象视为失败结果并丢弃，而不是并发复用。

checkpoint 是按值复制的纯数据快照。复用时会重新收集当前表/分区 ID 并校验哈希和边界，避免把一个表集合的预留区间误用于另一个集合。

## 与 Go 版本的对应关系

Go 同路径没有 `lib.rs` 对等文件；Go 以目录自动形成 package，而 Rust 需要该 crate 根显式声明实现模块并导出 API。`Cargo.toml` 的 `go-package` 元数据和 `alloc.rs` 的注释共同确认对应关系。

`alloc.rs` 基本保持 `alloc.go` 的公共概念和流程：相同的 `InsaneTableIDThreshold`、`Allocator` 两个方法、`New`/`NewAndPrealloc`/`ReuseCheckpoint`、半开区间、旧 ID 重写规则、checkpoint 字段以及大端 ID 哈希。`alloc_test.rs` 对照 Go `alloc_test.go` 的 `TestAllocator` 与 `TestAllocatorBound`，`parity_test.rs` 补充错误与 checkpoint 契约。

当前 Rust 移植并非完整生产依赖复刻：错误、checkpoint、meta model 和 metautil table 都是本地最小替身，`Allocator` 也没有直接连接 Go 版的 `meta.Mutator`/事务设施。Rust `prealloc_db` 通过精简模型转换消费该 crate；更上层 `snap_client` 仍使用自己的 stub 预分配类型。因此可以确认算法和局部集成已经存在，但不能声称完整 BR 生产恢复链已统一接到这个 crate。

## 扩展指南

若只是新增算法能力，应优先修改 `alloc.rs`，并在独立的 `alloc_test.rs` 或 `parity_test.rs` 增加对应测试；不要把测试写进 `lib.rs`。新增公共项前应决定是否真的要通过 `pub use alloc::*` 暴露到 crate 根，并检查 `prealloc_db` 和潜在下游的名称冲突。

若增加新实现子模块，应在本文件显式声明其可见性，并避免无意中形成两套同名公共 API。若收窄现有通配导出为显式导出，需要保留当前调用方使用的 `PreallocIDs`、`Allocator`、`New` 以及模型模块，并把该变化视为 API 兼容性调整。

若要完成更上层生产接线，应统一 `snap_client/stubs.rs` 与本 crate 的数据模型和 checkpoint 类型，并提供真正连接目标集群全局 ID 元数据的 `Allocator` 实现。该工作涉及 Cargo 依赖、事务原子性和恢复 checkpoint 兼容，不应只在门面文件内完成；测试至少要同步覆盖 `prealloc_table_id` 两个独立测试文件、`prealloc_db/db_test.rs`、`prealloc_db/parity_test.rs` 及 `snap_client/client_test.rs` 的分配/复用场景。

修改阈值、排序/哈希格式、区间端点或复用判定会影响 checkpoint 跨版本兼容性；必须与 Go `alloc.go` 和 `alloc_test.go` 同步核对，并覆盖空输入、分区、重复/未知 ID、异常大 ID、边界水位、底层分配失败和 hash/border 篡改。

## 验证依据

- RustCodeGraph：运行 `status` 确认索引可用；`files --filter br/pkg/restore/internal/prealloc_table_id` 确认目录已索引；`node --file .../lib.rs` 读取完整 30 行门面；`node --file .../alloc.rs` 读取完整 498 行实现；`query NewAndPrealloc`、`query PreallocTableIDs`、`query Allocator` 定位 Go/Rust符号。对重名 Go/Rust 符号的 `callers/callees` 结果不够精确，因此未单独据此断言调用边。
- crate 与实现：`br/pkg/restore/internal/prealloc_table_id/Cargo.toml`、`lib.rs`、`alloc.rs`，核对 library 入口、唯一外部依赖、公开再导出、状态结构、分配/重写/checkpoint 流程。
- Go 对照：`br/pkg/restore/internal/prealloc_table_id/alloc.go` 与 `alloc_test.go`，核对包职责、算法分支、错误语义和测试意图。
- Rust 独立测试：`br/pkg/restore/internal/prealloc_table_id/alloc_test.rs` 与 `parity_test.rs`，核对正常矩阵、空输入、分配边界、错误传播、哈希及 checkpoint 复用。
- 直接集成：`br/pkg/restore/internal/prealloc_db/Cargo.toml`、`db.rs`、`db_test.rs`、`parity_test.rs`，核对路径依赖、根级 API 使用及表/分区信息转换。
- 上层背景：`br/pkg/restore/snap_client/client.rs` 与 `stubs.rs`，用于区分目标恢复流程与当前仍独立存在的 stub 类型，未将其误记为本 crate 的直接生产调用者。
- 本任务是纯文档分析，按计划未运行 Cargo。交付验证执行固定 11 章节结构命令，并人工确认唯一生产物、链接路径、门面/实现边界和未接线限制均有直接证据。
