# `br/pkg/mock/mockid/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-mock-mockid` 的 crate 根。`br/pkg/mock/mockid/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，根 `Cargo.toml` 又把 `br/pkg/mock/mockid` 列为 workspace member。该文件自身不实现分配算法，而是装配 [`mockid.rs`](mockid.rs)、平铺再导出其公开 API，并在测试构建中接入独立的 [`parity_test.rs`](parity_test.rs)。

这是迁移期的独立对等 crate，而不是当前 Rust BR 调试主链中的实际实现：仓库内没有其他 Cargo manifest 依赖 `astersql-br-pkg-mock-mockid`；`br/cmd/br/debug.rs` 使用的是 `br/cmd/br/stubs.rs::mockid` 中的同名本地桩。Go 侧则由 `br/cmd/br/debug.go` 直接导入同路径 Go 包。

## 核心职责

- 用 `#[path = "mockid.rs"] pub mod mockid` 把实现文件注册成公开子模块。
- 用 `pub use mockid::*` 在 crate 根重新导出 `IDAllocator` 和 `NewIDAllocator`，使调用方既可走 `crate::mockid::...`，也可走 `crate::...`。
- 用 `#[cfg(test)]` 和 `#[path = "parity_test.rs"] mod parity_test` 保证对等测试只进入测试构建，不成为发布 API。
- 用 crate 级 `#![allow(...)]` 容纳 Go 移植保留的 `NewIDAllocator`、`Alloc`、`Rebase` 等命名，以及当前迁移阶段可能存在的未使用项。

因此，这个文件的职责是“边界与导出”，原子计数语义位于 `mockid.rs`，业务使用场景的 Go 依据位于 `br/cmd/br/debug.go`。

## 主要符号

- `pub mod mockid`：公开实现模块。其主要生产符号为 `pub struct IDAllocator { base: AtomicU64 }`、`pub fn NewIDAllocator() -> IDAllocator`、`IDAllocator::Alloc(&self) -> Result<u64, Infallible>` 和 `IDAllocator::Rebase(&self) -> Result<(), Infallible>`。
- `pub use mockid::*`：通配再导出。当前会导出上述公开生产符号；`new_id_allocator_with_base` 是 `pub(crate)` 且受 `cfg(test)` 限制，不会成为外部 API。
- `mod parity_test`：私有测试模块，只在 `cfg(test)` 下存在。它包含溢出回绕测试 `alloc_wraps_after_u64_max_like_go_atomic_add` 和综合契约测试 `go_rust_public_contract_matches`。
- crate 级 `allow`：允许 `dead_code`、三种 Go 风格命名、未使用导入和变量。它不改变运行行为，但会降低这些类别的编译告警强度，扩展时不能把它当成忽略类型或并发错误的机制。

## 执行流程

1. Cargo 以 `lib.rs` 为 crate 根编译包；该文件按显式路径加载 `mockid.rs`。
2. `pub use mockid::*` 把实现模块的公开项暴露到 crate 根。调用 `NewIDAllocator` 时，实际执行 `mockid.rs` 中的构造函数，以 `AtomicU64::new(0)` 建立实例。
3. 调用 `Alloc` 时，实际实现对 `base` 执行 `fetch_add(1, Ordering::SeqCst)`，再用 `wrapping_add(1)` 返回自增后的值；新实例依次得到 `1、2、3……`，从 `u64::MAX` 增加时回绕到 `0`。
4. 调用 `Rebase` 时直接返回 `Ok(())`，不读取或修改 `base`。
5. 测试构建额外加载 `parity_test.rs`：测试从 crate 根使用再导出的 API，并通过 `crate::mockid::new_id_allocator_with_base` 覆盖溢出边界。

当前应用接线需单独看待：Go 的 `br/cmd/br/debug.go` 用该包为表、索引和分区模拟新 ID；Rust 的 `br/cmd/br/debug.rs` 执行相似流程，但解析到 `br/cmd/br/stubs.rs` 的本地 `mockid` 模块，并不经过本 crate 的 `lib.rs`。

## 数据与状态

`lib.rs` 不持有全局状态，也不定义常量。唯一业务状态来自其再导出的 `IDAllocator.base: AtomicU64`：

- 每个 `NewIDAllocator()` 都创建独立、从 `0` 开始的计数器，第一次 `Alloc()` 返回 `1`。
- 状态只在该实例内变化；`Rebase()` 是空操作，多个实例之间没有共享基数或协调机制。
- 返回值是 `u64`；溢出遵循模 `2^64` 回绕，与 Go 的 `atomic.AddUint64` 对齐，而不是报错或饱和。
- 没有持久化、租约、远端服务、ID 预留区间或进程间唯一性保证，所以它只适合测试和模拟。

## 依赖与调用关系

上游与入口：

- `br/pkg/mock/mockid/Cargo.toml` 定义本 crate，未声明普通依赖或开发依赖；根 `Cargo.toml` 仅把它纳入 workspace。
- 仓库 Cargo manifests 中未发现其他 crate 对包名 `astersql-br-pkg-mock-mockid` 的依赖，因此当前 Rust 上游只有本 crate 的 `parity_test.rs`。
- RustCodeGraph 显示 `lib.rs` 的模块/再导出关系，并识别 `new_id_allocator_with_base -> alloc_wraps_after_u64_max_like_go_atomic_add`；对常见同名符号的宽泛查询会同时命中 `br/cmd/br/stubs.rs`，不能据此认定存在跨 crate 调用。

下游：

- `lib.rs -> mockid.rs`：模块装配和公开再导出。
- `mockid.rs -> std::sync::atomic::{AtomicU64, Ordering}`：唯一运行时依赖，使用 `SeqCst` 原子操作。
- 测试模式下 `lib.rs -> parity_test.rs`，测试再调用 crate 根导出和测试专用构造器。

Go 对照链路为 `br/cmd/br/debug.go -> br/pkg/mock/mockid/mockid.go`。当前 Rust 对应链路则为 `br/cmd/br/debug.rs -> br/cmd/br/stubs.rs::mockid`；若未来要让应用复用本 crate，需要显式新增 Cargo 依赖和导入，不能仅依赖同名符号。

## 错误处理与边界

- `Alloc` 和 `Rebase` 的错误类型都是 `std::convert::Infallible`，所以正常类型系统下不会产生 `Err`；这对齐 Go 实现恒定返回 `nil` error 的行为。
- `Alloc` 明确采用回绕加法，边界 `u64::MAX -> 0` 已由独立测试锁定。返回 `0` 因而不是错误信号；调用方若要求非零 ID，必须另行限制分配规模。
- `Rebase` 不校准计数器。依赖真实 allocator rebase 语义的代码不能用该 mock 验证。
- crate 根的通配再导出会自动暴露未来在 `mockid.rs` 新增的所有 `pub` 项；新增公开符号时应把这视为 crate API 变更并检查命名冲突。
- `cfg(test)` 测试辅助构造器与测试模块不会进入非测试构建，生产调用方不能依赖它们。

## 并发与资源生命周期

`IDAllocator::Alloc(&self)` 通过 `AtomicU64` 支持共享引用上的并发更新；`Ordering::SeqCst` 给所有该原子操作提供全序。`parity_test.rs` 把分配器放入 `Arc`，由 8 个线程各分配 100 个 ID，并验证 800 个结果无重复，直接覆盖线程共享场景。

该类型没有锁、通道、后台任务、文件句柄、网络连接或自定义 `Drop`。实例析构仅释放内存；最后一个 `Arc` 被释放后没有额外清理义务。原子性只保证单实例计数更新不竞争，不提供跨实例、跨进程或重启后的唯一性。

## 与 Go 版本的对应关系

`br/pkg/mock/mockid/mockid.go` 是直接语义来源：

- Go `IDAllocator.base uint64` 对应 Rust `AtomicU64` 字段；Go 通过 `atomic.AddUint64` 操作普通字段，Rust 把原子性封装进字段类型。
- Go `NewIDAllocator() *IDAllocator` 返回指针，Rust `NewIDAllocator() -> IDAllocator` 返回拥有所有权的值；需要共享时由调用方包入 `Arc`。
- Go `Alloc() (uint64, error)` 对应 Rust `Result<u64, Infallible>`。两者都返回自增后的值且不会产生实际错误；Rust 通过不可构造的错误类型表达这一点。
- Go `atomic.AddUint64` 的溢出回绕由 Rust 的 `fetch_add(...).wrapping_add(1)` 显式复现。
- Go `Rebase() error` 恒返 `nil`，Rust `Rebase()` 恒返 `Ok(())`，两者都不改变状态。

Go 调试命令在 `br/cmd/br/debug.go` 中真实使用此包模拟表、索引和分区 ID。Rust 的相似代码当前依赖独立的 `br/cmd/br/stubs.rs::mockid`；两份 Rust 实现目前首个 ID 都是 `1`，但类型签名和接线路径不同，不能视为同一个实现。

## 扩展指南

- 修改导出边界时优先在 `lib.rs` 调整 `pub mod`/`pub use`，并确认是否仍需兼容 crate 根的 Go 风格符号；避免无意中通过通配再导出扩大 API。
- 修改分配、溢出或 `Rebase` 行为时应改 `mockid.rs`，并同步独立的 `parity_test.rs`。测试逻辑不要内嵌到生产源文件。
- 新增边界测试可使用测试专用 `new_id_allocator_with_base`；它应继续保持 `cfg(test)` 和 `pub(crate)`，不要为测试便利扩大生产 API。
- 若要把本 crate 接入 `br/cmd/br`，需要在 `br/cmd/br/Cargo.toml` 添加明确依赖、替换 `stubs.rs::mockid` 导入，并验证 `Alloc` 返回类型差异；这属于应用接线变更，不应在本文件的文档任务中暗示已经完成。
- 若要支持真实持久化、跨进程唯一性或有效 `Rebase`，应引入新的实现抽象和独立测试，而不是继续扩大这个测试 mock。兼容风险主要在 Go 风格 API 和回绕语义；性能风险主要来自把 `SeqCst` 改成其他内存序时可能造成的并发语义变化。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter br/pkg/mock/mockid` 确认 `lib.rs`、`mockid.rs`、`parity_test.rs` 和 Go 对照文件；`node --file` 读取上述文件；`query NewIDAllocator`、`query IDAllocator` 与精确 `explore` 用于区分本 crate、Go 实现和 `br/cmd/br/stubs.rs` 的同名符号。
- 源码：`br/pkg/mock/mockid/lib.rs`（模块、再导出、测试门控）、`br/pkg/mock/mockid/mockid.rs`（原子实现）、`br/pkg/mock/mockid/parity_test.rs`（顺序、回绕、空操作、并发唯一性和资源生命周期测试）。
- crate/构建边界：`br/pkg/mock/mockid/Cargo.toml`、根 `Cargo.toml`、`br/pkg/mock/mockid/BUILD.bazel`；Cargo manifests 的包名搜索未发现其他 Rust crate 依赖该包。
- Go 与应用对照：`br/pkg/mock/mockid/mockid.go`、`br/cmd/br/debug.go`；Rust 当前应用路径：`br/cmd/br/debug.rs`、`br/cmd/br/stubs.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令验证目标文档存在且恰好包含 11 个固定二级章节，并人工复核唯一新增生产物为本说明文件。
