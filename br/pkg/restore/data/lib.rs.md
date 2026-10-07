# `br/pkg/restore/data/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-restore-data` 的 crate 根。`br/pkg/restore/data/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，根 `Cargo.toml` 又把 `br/pkg/restore/data` 列为 workspace member；包元数据把对应 Go package 明确标为 `br/pkg/restore/data`，类型为 `library`。

该文件是模块装配和公开 API 门面，而不是恢复算法的实现文件。生产构建中它声明 `stubs`、`key`、`recover`、`data` 四个公开模块，并用四条 `pub use ...::*` 把这些模块的公开项提升到 crate 根。`#[cfg(test)]` 下另行挂载 `parity_test.rs`、`data_test.rs`、`key_test.rs`、`recover_test.rs`；测试逻辑保持在独立文件中，不进入普通库构建。

## 核心职责

1. 用 `#[path = "..."] pub mod ...` 固定四个生产模块的源文件位置，使本目录无需 `src/` 布局也能形成一个 Cargo library。
2. 用 glob re-export 提供统一门面。调用方既可写模块路径（如 `crate::data::RecoverData`），也可从 crate 根取得公开项；因此该文件定义的是可见性边界，而非新的业务抽象。
3. 仅在测试构建时注册四个独立测试模块。其中 `parity_test` 验证 Go/Rust 公共行为，`data_test` 覆盖恢复状态与计划，`key_test` 覆盖键和 Region 算法，`recover_test` 固定空 peer 集合违反不变量时的 panic 行为。
4. 通过 crate 级 `#![allow(...)]` 暂时放宽迁移代码的命名、未使用项和 Clippy 检查，使保留 Go 风格名称及尚未完全接线的接口可以共存。该 allow 列表是迁移期兼容措施，不表示这些 lint 对整个 workspace 都被关闭。

## 主要符号

- `pub mod stubs`：公开本地边界类型，包括 `Context`、`Error`/`Result`、`Mgr`、`Progress`、恢复 RPC/流 trait、`metapb`/`recovpb` 最小结构及重试、worker pool 等。`stubs.rs` 明确说明这些是 Darwin-safe stand-in，不是真实 kvproto、grpcio、PD 或 TiKV 客户端。
- `pub mod key`：公开 `keyEq`、`keyCmp`、`keyCmpInterface`、`PrefixStartKey`、`PrefixEndKey`。其中起始键加字节 `b'z'`，空结束键编码成 `b'z' + 1`，为恢复区间提供统一的全范围排序语义。
- `pub mod recover`：公开 `RecoverRegion`、`RecoverRegionInfo` 以及 `SortRecoverRegions`、`CheckConsistencyAndValidPeer`、`LeaderCandidates`、`SelectRegionLeader`。这些函数执行纯内存的 peer 排序、重叠消解、连续性校验和 leader 负载选择。
- `pub mod data`：公开 `RecoveryStage`、`RecoverData`、`StoreMeta`、`Recovery`、`NewStoreMeta`、`NewRecovery` 等恢复编排接口。`RecoverData` 驱动收集元数据、生成计划、恢复 PD 分配 ID、下发 Region 计划及两阶段 flashback。
- `pub use data::*`、`pub use key::*`、`pub use recover::*`、`pub use stubs::*`：构成 crate 根公开面。新增同名公开项时可能引入 glob re-export 冲突，编译器报错或下游名称解析变化都应视为门面兼容性问题。
- `mod parity_test`、`mod data_test`、`mod key_test`、`mod recover_test`：均受 `#[cfg(test)]` 保护且不使用 `pub`，只参与本 crate 测试编译，不成为下游 API。

## 执行流程

`lib.rs` 自身没有函数调用、循环、网络访问或状态机；它在编译期形成如下装配关系：

1. 编译器读取 crate 级 lint allow。
2. 按显式 `#[path]` 加载 `stubs.rs`、`key.rs`、`recover.rs`、`data.rs`。模块之间的实际依赖为：`recover` 使用 `key` 和 `stubs`，`data` 使用 `recover` 与 `stubs`。
3. 四个 glob re-export 把各模块公开符号汇聚到 crate 根。
4. 若启用测试构建，再加载四个独立测试模块；这些测试仍可通过 `crate::data`、`crate::key`、`crate::recover`、`crate::stubs` 访问模块内项目。

由门面进入的代表性运行主链是 `RecoverData -> doRecoveryData -> Recovery::{ReadRegionMeta, MakeRecoveryPlan, RecoverRegions, PrepareFlashbackToVersion, FlashbackToVersion}`。其中 `MakeRecoveryPlan` 进一步调用 `SortRecoverRegions -> CheckConsistencyAndValidPeer -> LeaderCandidates -> SelectRegionLeader`。这条链实际定义在 `data.rs` 和 `recover.rs`，不可归因于 `lib.rs` 自身。

当前索引只把 `tools/tazel/parity_test.rs` 识别为 `lib.rs` 文件级使用者，不能据此推断生产调用已接线。更可靠的事实是 Cargo workspace 会编译该 crate，而本包测试直接使用上述模块路径和公开符号；仓库内 RustCodeGraph 对 `RecoverData` 的直接 Rust 调用证据是 `br/pkg/restore/data/parity_test.rs` 的 `contract_resource_cleanup_on_recover_data`。

## 数据与状态

该文件不声明常量、结构体、枚举、trait、函数或运行时全局变量，因此不拥有业务数据，也没有初始化顺序依赖。它影响的“状态”只有编译期模块图、符号可见性和 lint 策略。

经门面暴露但由子模块持有的关键状态包括：

- `Recovery`：保存 `allStores`、每个 store 的 `StoreMetas`、按 store 分组的 `RecoveryPlan`、`MaxAllocID`、管理器、进度器、并发度及 watcher 测试开关。
- `RecoveryStage`：把错误分类为收集元数据、生成恢复计划、重置 PD 分配 ID、恢复 Region、flashback 或未知阶段；前四阶段可重试，flashback 和未知阶段不可重试。
- `RecoverRegion`/`RecoverRegionInfo`：分别表达带 store 归属的 peer 元数据和经排序、前缀规范化后的 Region 候选。
- `stubs::Context`、`Error`、`MemProgress` 等：用于当前 Rust 移植和测试的本地状态模型。它们是接口替身，不能等同于 Go 生产实现中的 `context.Context`、gRPC connection 或真实 PD/TiKV client。

## 依赖与调用关系

`br/pkg/restore/data/Cargo.toml` 的 `[dependencies]` 为空，说明此 crate 当前没有声明外部或 workspace Rust 依赖；跨系统能力被收敛进 `stubs.rs` 的本地 trait/结构。Cargo 注释还明确标注 arm64 Darwin 环境不接 `kv/domain/kvproto/grpcio`，仅保留 local traits/stubs。

模块内依赖方向是单向的：

- `key` 不依赖其他本包模块，只做字节键处理。
- `recover` 依赖 `key::{PrefixStartKey, PrefixEndKey, keyCmp, keyEq}` 和 `stubs` 的错误、日志、proto 替身。
- `data` 依赖 `recover` 的计划算法以及 `stubs` 的上下文、重试、并发、RPC、PD、flashback 和进度边界。
- `lib.rs` 只装配和重导出，不向子模块注入运行时行为。

RustCodeGraph 的精确文件查询确认 `key.rs` 被 `recover.rs` 与 `parity_test.rs` 使用，`recover.rs` 被 `parity_test.rs` 与 `recover_test.rs` 使用；`RecoverRegions` 的直接被调用边来自 `doRecoveryData`。Go 侧没有对应 `lib.go`：Go 通过同一 `package data` 自动聚合 `data.go`、`key.go`、`recover.go`，Rust 才需要本文件显式复现包边界。

## 错误处理与边界

`lib.rs` 不创建、捕获或传播运行时错误。错误语义全部来自其重导出的子模块：`stubs::Error` 保存消息、可选稳定错误码和可选恢复阶段；`data.rs` 在每个恢复阶段用 `stage_err` 标记错误，从而决定整体重试；`recover.rs` 对 tombstone 有效 peer、非连续 Region 范围和空 leader 候选返回分类错误。

需要特别保留的边界如下：

- `SortRecoverRegions` 直接索引每个 Region 的首个 peer；`recover_test.rs::sort_recover_regions_rejects_region_without_peers` 用 `#[should_panic]` 固定“空 peer 列表违反收集不变量”，而不是静默跳过。
- `PrefixEndKey([])` 表示无界上界，编码为 `[b'z' + 1]`；若改成普通 `PrefixStartKey`，全键空间连续性判断会改变。
- `CheckConsistencyAndValidPeer` 拒绝范围缺口及最终有效集合中的 tombstone peer。
- `SelectRegionLeader` 假定输入非空；空集合应先由 `LeaderCandidates` 拒绝。
- 门面采用 glob re-export。扩展模块或新增公开项前必须检查同名冲突，不能只验证模块内路径可用。
- `#[path]` 是相对 `lib.rs` 的文件绑定；移动或重命名任一子模块/测试文件时必须同步修改此处，否则 crate 在解析阶段失败。

## 并发与资源生命周期

本文件本身不创建线程、锁、channel、连接、ticker 或事务，也没有 `Drop` 实现。它只决定承载这些行为的模块是否被编译和公开。

经门面进入的恢复流程中，`data.rs` 使用 `ErrorGroup`/`WorkerPool` 并行收集各 store 元数据及下发恢复计划，用 `Arc<Mutex<...>>`、`Condvar` 和集合协调结果；`SpawnTiKVShutDownWatchers` 可启动后台线程观察 store 重启。`CancelOnDrop` 在主恢复流程退出时取消派生 context，`ConnCloser::drop` 保证每条恢复连接关闭。`RecoverData` 的 parity 测试使用原子计数验证连接关闭和恢复调用，且通过 `spawn_watcher = false` 避免测试遗留 30 秒 watcher。

这些生命周期保证属于 `data.rs`/`stubs.rs`，不是 `lib.rs` 的副作用。删除某个 `mod` 或 `pub use` 虽不会直接释放资源，却可能让相应实现或测试不再编译，从而失去上述保护。

## 与 Go 版本的对应关系

Go 目录以 `package data` 隐式合并源码，没有单独的 `lib.go`。本文件对应的是 Go 包级命名空间和导出面：

- Rust `key.rs` 对照 `key.go`，保留 `keyEq`、`keyCmp`、`PrefixStartKey`、`PrefixEndKey` 的字节序及空结束键语义。
- Rust `recover.rs` 对照 `recover.go`，保留 peer 的 term/index/commit 降序规则、Region version 优先、重叠消解、连续性检查和按 store score 选主。
- Rust `data.rs` 对照 `data.go`，保留六阶段恢复顺序、阶段化重试判断、计划生成、PD allocate ID 恢复、Region 下发、watcher 和 flashback 流程。
- Rust `data_test.rs` 与 `key_test.rs` 对照 Go 同名测试；额外的 `parity_test.rs` 扩展覆盖正常、边界、错误和资源清理契约，`recover_test.rs` 明确固定空 peer 的 panic 不变量。

实现状态存在关键差异：Go 文件导入真实 `kvproto`、PD/TiKV client、gRPC、glue、worker pool 与 range task；Rust Cargo 当前无依赖，并通过 `stubs.rs` 模拟这些边界。因此可以认为纯算法和受测编排语义正在对齐，但不能声称 Rust crate 已具备连接真实集群的生产能力。`lib.rs` 顶部也明确提示门面应区分重导出与真实实现位置。

## 扩展指南

- 新增恢复算法时，优先放进职责对应的 `key.rs`、`recover.rs` 或 `data.rs`，不要把业务逻辑堆入 crate 根；`lib.rs` 只负责必要的模块声明和有意的重导出。
- 新增 Rust 源模块后，在这里增加明确的 `#[path] mod`，并决定它应为 `pub mod`、仅 crate 内可见，还是仅重导出少量 API。不要默认继续扩大 glob 门面。
- 若新增公开符号，检查四组 `pub use ...::*` 是否产生同名项，并同步 `parity_test.rs` 的公共契约覆盖。
- 若修改键编码或恢复计划算法，同步更新独立的 `key_test.rs`/`recover_test.rs`，并与 `key_test.go` 及 `recover.go` 的规则逐项核对。
- 若修改恢复阶段、错误重试或资源清理，同步更新 `data_test.rs` 和 `parity_test.rs`；应覆盖可重试/不可重试阶段、连接关闭、context 取消、流结束与 watcher 退出。
- 若接入真实 PD/TiKV/gRPC，不应把实现继续塞进 `stubs.rs`。应先在 Cargo manifest 声明可复现依赖，再以 trait 实现替换当前 ClientFactory/Conn/stream stand-in，并保留内存实现供独立测试。
- 新测试必须继续放在独立 `*_test.rs` 文件，通过 `#[cfg(test)]` 挂载；不要把测试内嵌回 `lib.rs` 或业务源文件。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；本次查询时数据库可用。
- RustCodeGraph `node --file br/pkg/restore/data/lib.rs --offset 1 --limit 260`：确认文件共 77 行、四个生产模块、四个条件测试模块、四组 glob re-export 及 crate lint allow。
- RustCodeGraph 对 `key.rs`、`recover.rs`、`data.rs`、`stubs.rs` 的文件节点查询，以及对 `RecoverData`、`SortRecoverRegions` 等入口的 `explore`/`callers`/`callees` 查询：确认模块职责、`doRecoveryData -> RecoverRegions` 等调用边和本地桩边界。
- Cargo 证据：`br/pkg/restore/data/Cargo.toml` 确认 crate 名、`lib.rs` 入口、Go package 映射、空依赖及 Darwin-safe 注释；根 `Cargo.toml` 确认 workspace membership。
- Go 对照：`br/pkg/restore/data/data.go`、`key.go`、`recover.go`、`data_test.go`、`key_test.go`。
- Rust 独立测试：`br/pkg/restore/data/parity_test.rs`、`data_test.rs`、`key_test.rs`、`recover_test.rs`。已核对它们分别覆盖公共契约/资源清理、Region 总数与计划、键及选主算法、空 peer 不变量。
- 本任务是只新增说明文档的静态分析，按任务约束未运行 Cargo。交付前使用任务文件规定的命令检查目标文档存在且恰好包含十一个固定二级章节，并人工复核所有相对链接指向当前目录真实文件。
