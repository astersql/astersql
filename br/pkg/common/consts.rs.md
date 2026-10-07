# `br/pkg/common/consts.rs`

## 文件定位

`br/pkg/common/consts.rs` 是 `astersql-br-pkg-common` 库 crate 的常量实现文件。crate 根 `br/pkg/common/lib.rs` 通过 `#[path = "consts.rs"] pub mod consts` 挂载它，并用 `pub use consts::*` 将其内容扁平重导出；因此外部 crate 使用的公开路径是 `astersql_br_pkg_common::MaxStoreConcurrency`，而不必经过 `consts` 子模块。

`br/pkg/common/Cargo.toml` 将库入口指定为 `lib.rs`，并以 `package.metadata.porting.go-package = "br/pkg/common"` 记录对应 Go 包。该清单没有普通依赖或 feature，说明本文件位于 BR 子系统的低层公共契约边界，不依赖其他 Rust 业务模块。

## 核心职责

本文件当前只有一项职责：以 `pub const MaxStoreConcurrency: usize = 128` 保存单个 BR 进程面向 TiKV store 时使用的经验并发上限。这个值用于在 store 数量增长时限制连接或工作任务的扇出规模，避免并发度无条件随集群 store 数增长。

它只提供上限值，不创建连接池、线程、异步任务或信号量，也不负责把实际并发度裁剪到该值。调用方仍需显式计算 `min(store_count, MaxStoreConcurrency)` 并将结果交给自己的 worker pool 或 error group；这一使用方式可由 Go 调用点 `br/pkg/restore/data/data.go` 和 `br/pkg/task/backup_ebs.go` 直接验证。

## 主要符号

- `pub const MaxStoreConcurrency: usize = 128`：本文件唯一的模块级符号，也是唯一公开 API。`pub` 允许 `br/pkg/common/lib.rs` 重导出；`const` 表示值在编译期内联、没有可变存储；`usize` 使其能直接和 Rust 集合的 `len()` 结果参与 `std::cmp::min`，并固定测试中的 store-count 用法。
- 文件中没有类型、trait、函数、`impl`、宏或条件编译项。测试不内嵌在源文件中，而由 `br/pkg/common/lib.rs` 在 `cfg(test)` 下分别挂载 `consts_test.rs` 与 `parity_test.rs`。

命名沿用 Go 的导出标识符，而不是 Rust 通常使用的全大写常量风格；crate 根的 `#![allow(non_upper_case_globals)]` 明确容纳了这种迁移期 API 对齐方式。

## 执行流程

该文件没有运行时控制流。其生效链路是：

1. Cargo 按 `br/pkg/common/Cargo.toml` 的 `[lib] path = "lib.rs"` 编译 crate。
2. `br/pkg/common/lib.rs` 声明 `consts` 模块，编译器读取本文件并建立常量。
3. `pub use consts::*` 将常量提升到 crate 根。
4. 依赖该 crate 的代码可导入 `astersql_br_pkg_common::MaxStoreConcurrency`，再自行将 store 数裁剪到不超过 128。

当前 Rust 接线必须区分两种情况：`br/pkg/task/Cargo.toml` 声明了到 `../common` 的路径依赖，`br/pkg/task/backup_ebs.rs` 也从公共 crate 导入常量，但函数 `RunBackupEBS` 目前仅执行 `let _ = MaxStoreConcurrency`，尚未用它计算实际并发度；`br/pkg/restore/data/data.rs` 的限流表达式则从该 crate 自己的 `stubs.rs` 导入同名常量，而 `br/pkg/restore/data/Cargo.toml` 没有依赖公共 crate。因此不能据此声称本文件已控制所有 Rust 恢复流程的运行时并发。

## 数据与状态

常量值为正整数 `128usize`。它代表每个 BR 进程的 store 并发上限，而不是集群 store 总数、线程池的当前大小、全局连接数计数器或可动态调整的配置值。

本文件没有可变状态、堆分配、缓存、持久化数据或环境配置。`usize` 的平台宽度不影响数值 128，但会影响 API 类型：消费方若需要 `u32` 等类型，应在自己的边界显式、安全地转换。正常的裁剪不变量是 `effective_concurrency = min(store_count, 128)`；当 `store_count` 为 0 时结果也为 0，本常量本身不会替调用方决定零 worker 是否有效。

## 依赖与调用关系

上游声明与导出关系如下：

- `br/pkg/common/Cargo.toml` 定义 crate `astersql-br-pkg-common`，入口为 `lib.rs`。
- `br/pkg/common/lib.rs` 挂载 `consts.rs` 并扁平重导出全部公开符号。
- `br/pkg/task/Cargo.toml` 通过路径依赖引用该公共 crate；`br/pkg/task/backup_ebs.rs` 是当前 `rg` 能确认的 Rust 外部导入者，但只有占位读取。
- `br/pkg/common/consts_test.rs` 和 `br/pkg/common/parity_test.rs` 通过 crate 根导入该常量，验证类型、值和基本契约。

本文件没有下游函数调用，也没有第三方依赖。RustCodeGraph 的文件节点显示 `consts.rs` 全部 26 行并识别一个符号，但报告 `used by 0 files`；精确 `query MaxStoreConcurrency` 又只返回 `br/pkg/restore/data/stubs.rs` 的同名常量等歧义结果。因此调用关系以 crate/Cargo 声明与精确文本引用交叉核验，不能把图索引缺失解释为“没有消费者”。

Go 侧的实际消费者是 `br/pkg/restore/data/data.go` 的 `ReadRegionMeta`、`RecoverRegions` 路径，以及 `br/pkg/task/backup_ebs.go` 的 `waitUntilAllScheduleStopped`：三处都以 store 集合长度和本常量取最小值。Rust 的 `br/pkg/restore/data/data.rs` 保留了对应限流表达式，但目前依赖的是 `br/pkg/restore/data/stubs.rs` 中复制的常量，而非本文档所述公共符号。

## 错误处理与边界

常量求值和读取不会返回错误、panic 或 `Result`。真正的错误处理位于消费方创建连接、调度 worker、等待 error group 或执行 RPC 的代码中，本文件不包装也不传播那些错误。

需要由消费方处理的边界包括：空 store 集合导致有效并发度为 0；store 数超过 128 时只应启动至多 128 个并发工作；不同整数类型之间的转换；以及 worker pool 是否接受 0。独立测试 `br/pkg/common/parity_test.rs` 断言值大于 0 且不超过 1024，但上界 1024 是测试给出的安全区间，不是本文件公开的第二项业务常量。

修改数值会改变面向大量 store 的资源压力和吞吐，应视为兼容的 API 值变更而非普通重构；必须同步评估连接数、RPC 压力、内存、调度公平性以及 Go/Rust 行为一致性。

## 并发与资源生命周期

名称描述并发策略，但常量本身不拥有任何并发原语或资源。它是 `Copy` 的编译期值，可被多个线程或任务读取，不需要锁、原子操作、初始化、析构或关闭流程。`br/pkg/common/parity_test.rs` 通过重复赋值验证了这种无资源所有权的使用方式。

连接、worker、取消上下文和 error group 的生命周期均属于调用方。尤其不能把值 128 理解为已经存在一个全局信号量；若新增调用点只导入常量却没有把它用于 worker/error-group 限流，就不会产生任何并发保护。Go 的恢复路径会在局部构造 worker pool 并绑定 error group，而当前 Rust 恢复移植在自己的 crate 内借助同名桩值实现相似裁剪。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/common/consts.go`。两侧都导出 `MaxStoreConcurrency`，值均为 128，注释都将其解释为面向 TiKV 连接池的经验上限。Go 使用无类型整数常量；Rust 显式选择 `usize`，以便与 `Vec`/slice 的 `len()` 结果直接比较。这是类型系统层面的差异，不是数值语义差异。

Go 侧已经在 `br/pkg/restore/data/data.go` 两处和 `br/pkg/task/backup_ebs.go` 一处将该公共常量用于真实并发计算。Rust 公共 crate 的 API 和测试已建立，但迁移状态并不完全相同：`backup_ebs.rs` 只有占位引用，`restore/data` 仍复制同值桩常量。后续接线时应复用公共 crate，而不是继续增加副本；不过消除现有桩属于消费 crate 的迁移任务，不是本文件的职责。

## 扩展指南

- 调整数值时，应同时修改 `br/pkg/common/consts.rs` 与 Go 对照 `br/pkg/common/consts.go`，并更新独立测试 `br/pkg/common/consts_test.rs`、`br/pkg/common/parity_test.rs`；还要复查所有 `min(store_count, ...)` 调用点的容量类型和资源预算。
- 新增同类公共常量时，可继续放在本模块并依靠 `lib.rs` 重导出。若常量只服务单一实现细节，应留在所属 crate，避免扩大公共 API。
- 为现有 Rust 恢复/备份逻辑接线时，优先在消费 crate 的 `Cargo.toml` 声明 `astersql-br-pkg-common` 并从 crate 根导入；不要复制第三份同名常量。应在对应消费模块的独立 `*_test.rs` 或 `parity_test.rs` 中验证实际限流行为，而不是把测试写入 `consts.rs`。
- 如果并发上限未来需要按配置、store 类型或运行时负载变化，它就不再适合作为单一 `const`；应由配置层计算有效值，同时保留清晰默认值，并测试 0、1、128、超过上限和整数转换等边界。
- 性能风险主要来自上调造成连接/RPC/内存峰值增加，或下调造成大集群任务变慢；兼容风险来自 Rust 和 Go 数值漂移，以及公共常量与 `restore/data/stubs.rs` 副本漂移。

## 验证依据

本说明核对了以下直接证据：

- 源码与入口：`br/pkg/common/consts.rs`、`br/pkg/common/lib.rs`。
- crate 边界：`br/pkg/common/Cargo.toml`；外部依赖示例：`br/pkg/task/Cargo.toml`。
- Rust 测试：`br/pkg/common/consts_test.rs`、`br/pkg/common/parity_test.rs`。
- Go 对照与调用：`br/pkg/common/consts.go`、`br/pkg/restore/data/data.go`、`br/pkg/task/backup_ebs.go`。
- Rust 当前消费状态：`br/pkg/task/backup_ebs.rs`、`br/pkg/restore/data/data.rs`、`br/pkg/restore/data/stubs.rs`、`br/pkg/restore/data/Cargo.toml`。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/common` 找到 `consts.rs`、`consts_test.rs`、`lib.rs`、`parity_test.rs` 与 Go 对照；`node --file br/pkg/common/consts.rs --offset 1 --limit 240` 返回完整 26 行和单一常量。`query MaxStoreConcurrency` 存在同名桩歧义，`callers/callees` 查询未在时限内完成，因此以上调用边由 Cargo 清单和 `rg` 精确引用补证。

任务是纯文档分析，按计划不运行 Cargo。交付前以指定结构命令验证本文档存在且恰好包含十一个固定二级标题，并人工复核所有“已接线”描述均与当前源码一致。
