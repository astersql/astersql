# `br/pkg/common/lib.rs`

源码：[lib.rs](./lib.rs)；crate 清单：[Cargo.toml](./Cargo.toml)。

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-common` 的库根。根工作区的 `Cargo.toml` 将 `br/pkg/common` 列为 workspace member，而本目录 `Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定该文件为编译入口，并用 `package.metadata.porting.go-package = "br/pkg/common"` 声明其 Go 对照包。这个 crate 当前不是业务流程入口，而是 BR 子系统共享常量的公开门面。

文件本身只有 30 行：声明 `consts` 子模块、把子模块公开项重导出到 crate 根，并在测试构建中挂载两个独立测试模块。真实常量定义位于 [`consts.rs`](./consts.rs)，因此阅读该文件时不能把门面声明误认为连接池或任务调度实现。

## 核心职责

1. 以 `#[path = "consts.rs"] pub mod consts` 将同目录常量文件纳入 crate，并保留 `astersql_br_pkg_common::consts::...` 这一显式模块访问路径。
2. 以 `pub use consts::*` 扁平重导出公开项，使消费者可直接写 `astersql_br_pkg_common::MaxStoreConcurrency`。目前被重导出的生产符号只有 `consts::MaxStoreConcurrency: usize = 128`。
3. 在 `cfg(test)` 下挂载 [`consts_test.rs`](./consts_test.rs) 与 [`parity_test.rs`](./parity_test.rs)，把实现测试与 Go/Rust 公共契约测试放在独立文件中，而不是内嵌到生产源文件。
4. 在 crate 级 `#![allow(...)]` 中容忍迁移期 Go 风格命名、未使用项和 Clippy 告警。该设置覆盖整个 crate，应视为迁移兼容措施，而非新代码无需遵守 Rust 惯例的许可。

## 主要符号

- `pub mod consts`：公开模块声明，源文件由 `#[path = "consts.rs"]` 明确指定。它建立编译期模块边，不执行运行时逻辑。
- `pub use consts::*`：公开 glob 重导出。当前让 `MaxStoreConcurrency` 同时可从 `consts` 模块和 crate 根访问；未来 `consts.rs` 新增的任何 `pub` 项也会自动进入 crate 根 API，因此新增符号可能扩大兼容面。
- `mod consts_test`：仅在 `cfg(test)` 为真时编译的私有测试模块，验证根级重导出的常量能作为 `usize` 使用且值为 `128`。
- `mod parity_test`：仅在测试构建中编译的私有契约测试，验证值、正数边界、约定上限以及无分配的 `Copy` 使用方式。
- `MaxStoreConcurrency` 并非在本文件定义；其 canonical Rust 定义是 `consts.rs` 中的 `pub const MaxStoreConcurrency: usize = 128`。

本文件没有函数、类型、trait、`impl`、可变静态量、宏或条件 feature；也不存在可供调用图表达的运行时 caller/callee。

## 执行流程

该文件只参与编译和名称解析，流程如下：

1. Cargo 选择 `lib.rs` 作为 `astersql-br-pkg-common` 的库根。
2. 编译器处理 crate 级 lint allow 列表。
3. `#[path = "consts.rs"]` 装载 `consts` 模块，编译 `MaxStoreConcurrency` 常量。
4. `pub use consts::*` 把常量加入 crate 根的公开命名空间。
5. 下游 crate 解析 `astersql_br_pkg_common::MaxStoreConcurrency` 时取得该编译期常量；没有初始化函数、动态注册或运行时分派。
6. 仅在此 crate 的测试构建中，编译器继续装载两个独立测试文件；普通生产构建不包含它们。

当前可见生产消费者是 [`br/pkg/task/backup_ebs.rs`](../task/backup_ebs.rs)：它导入根级 `MaxStoreConcurrency`，但现实现仅在 `RunBackupEBS` 中以 `let _ = MaxStoreConcurrency` 触碰符号，尚未用它计算并发。Go 对照流程则在 `waitUntilAllScheduleStopped` 中以 `min(len(allStores), common.MaxStoreConcurrency)` 限制 worker pool 大小。

## 数据与状态

本文件不拥有可变状态。唯一通过门面暴露的数据是编译期常量 `MaxStoreConcurrency: usize = 128`：

- `usize` 与 Rust 容器长度、并发计数的类型自然兼容；`consts_test.rs` 用接受 `usize` 的局部函数锁定这一类型契约。
- 常量为 `Copy`，读取不会分配、加锁或转移所有权；`parity_test.rs` 通过连续赋值验证可复制使用。
- `128` 是“每个 BR 进程对 store 的经验并发上限”，不是当前 store 数，也不是运行时可调配置。
- `lib.rs` 不缓存 store 列表，不保存连接池，不维护计数器；实际消费者必须自行使用 `min(实际数量, MaxStoreConcurrency)` 等规则施加上限。

## 依赖与调用关系

向下依赖只有本地模块边 `lib.rs -> consts.rs`，以及测试构建中的 `lib.rs -> consts_test.rs`、`lib.rs -> parity_test.rs`。本 crate 的 `Cargo.toml` 没有 `[dependencies]`，因此门面和常量不依赖外部 crate。

向上依赖经 Cargo 和导入关系确认：

- `br/pkg/task/Cargo.toml` 以路径依赖 `../common` 引入 `astersql-br-pkg-common`。
- `br/pkg/task/backup_ebs.rs` 导入 `astersql_br_pkg_common::MaxStoreConcurrency`；当前仅保持符号接线，未形成实际并发控制。
- RustCodeGraph 的精确文件读取确认 `lib.rs` 声明/重导出结构；由于本文件没有函数，`callers`/`callees` 不适用于这些编译期模块边。全局同名查询还会命中 `br/pkg/restore/data/stubs.rs` 的独立 `MaxStoreConcurrency`，不能将那个桩常量误算为本 crate 的符号。

Go 侧真实调用包括 `br/pkg/task/backup_ebs.go::waitUntilAllScheduleStopped`、`br/pkg/restore/data/data.go::ReadRegionMeta` 和 `RecoverRegions`，都以 `min(store 数, common.MaxStoreConcurrency)` 限制 worker pool。Rust restore/data 当前使用自身 `stubs.rs` 中数值相同的常量，并未通过本门面复用；这是现状边界，不代表已经完成统一接线。

## 错误处理与边界

`lib.rs` 没有返回值、错误类型、`Result` 或 panic 路径。模块文件缺失、符号冲突和下游类型不匹配属于编译错误，而非运行时错误。

需要保持的边界包括：

- `MaxStoreConcurrency` 必须保持可作为 `usize` 使用，否则现有 Rust 类型契约测试失败。
- Go 与 Rust 的公共值应保持 `128`；修改时必须评估连接数、RPC 压力和资源占用，不能只让契约测试接受新值。
- `pub use consts::*` 会把所有新增公开常量自动提升到 crate 根；若出现同名项会造成编译冲突，若移除或重命名则会破坏下游导入。
- `#![allow(clippy::all)]` 等宽泛豁免会隐藏告警；扩展时应尽量缩小新增豁免范围，但本分析不改变现有迁移策略。
- `parity_test.rs` 的 `<= 1024` 只是测试中的安全区间断言，不是生产运行时校验，也不能替代容量评估。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、连接、文件句柄或堆资源，因而没有启动、取消、关闭或回收阶段。`MaxStoreConcurrency` 只是供调用方构造并发资源时采用的上限；生命周期完全由调用方的 worker pool、error group 或连接对象管理。

Go 对照表明预期资源语义是先取 `min(store 数, 128)`，再按这个容量建立 worker pool，并等待 error group 完成。当前 Rust `backup_ebs.rs` 尚未实现这一消费语义，只读取常量以维持依赖接线；因此不能声称本 crate 已在 Rust EBS 路径实际限制连接并发。常量本身是编译期值，可跨线程复制且无需同步。

## 与 Go 版本的对应关系

Go canonical 文件是 [`consts.go`](./consts.go)，包名为 `common`，仅导出无类型常量 `MaxStoreConcurrency = 128`。Rust 对应关系为：

- Go 包 `br/pkg/common` ↔ Cargo crate `astersql-br-pkg-common`，由 `package.metadata.porting.go-package` 明示。
- Go `const MaxStoreConcurrency = 128` ↔ Rust `pub const MaxStoreConcurrency: usize = 128`。
- Go 直接通过包名访问 ↔ Rust 由 `lib.rs` 根级重导出后通过 crate 名访问。

语义差异是 Rust 显式固定为 `usize`，而 Go 常量本身无类型、在使用点转换；Rust 的类型选择服务于 `len` 和并发计数。更重要的迁移差异是：Go 的 EBS 与 restore/data 生产路径都真实使用该常量裁剪 worker 数，Rust EBS 路径目前仅占位引用，Rust restore/data 则保留了本地桩常量。文档据此只确认公共常量和门面契约已移植，不把全部 Go 消费路径描述为已完成。

## 扩展指南

新增或修改公共常量时，建议按以下接入点处理：

1. 在 `consts.rs` 修改 canonical 定义；只有确需成为稳定公共 API 的项才声明为 `pub`，因为 glob 重导出会自动暴露它。
2. 同步核对 `consts.go` 及其真实 Go 使用点，记录类型、单位、默认值和边界语义；不要只按名称机械翻译。
3. 在独立的 `consts_test.rs` 增加 Rust 类型/值/边界测试，在 `parity_test.rs` 增加 Go/Rust 公共契约测试；不要把测试逻辑塞入 `lib.rs` 或 `consts.rs`。
4. 用全仓库引用搜索区分本 crate 符号与同名桩。若把 `br/pkg/restore/data/stubs.rs` 的重复常量改为依赖本 crate，需单独评估 crate 依赖方向和对应 restore 测试，不能在门面修改中静默扩大范围。
5. 若让 `backup_ebs.rs` 真正使用该上限，应增加针对 store 数小于、等于和大于 `128` 的独立测试，并验证零 store、取消和 worker 生命周期；这属于消费者行为修改，不应只靠本 crate 的常量测试证明。
6. 公共值变化存在兼容和性能风险：降低可能压缩吞吐，提高可能放大 TiKV 连接/RPC 压力。应同步 Go/Rust 行为并以真实调用路径验证。

## 验证依据

本说明基于以下可复核证据：

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`files --filter br/pkg/common` 确认 `lib.rs`、`consts.rs`、`consts_test.rs`、`parity_test.rs` 均已索引。
- RustCodeGraph `node --file br/pkg/common/lib.rs`：确认 crate 级 allow、`consts` 模块、根级重导出和两个 `cfg(test)` 测试模块。
- RustCodeGraph 对 `consts.rs`、`consts_test.rs`、`parity_test.rs` 的精确文件节点：确认常量签名和值，以及 `usize`、Go parity、正数/上界和 `Copy` 测试。
- RustCodeGraph 对 `br/pkg/task/backup_ebs.rs` 的精确文件节点：确认生产导入和 `RunBackupEBS` 中仅有 `let _ = MaxStoreConcurrency`；全局同名查询的 restore 桩歧义已通过路径限定排除。
- Cargo 文件：根 `Cargo.toml`、`br/pkg/common/Cargo.toml`、`br/pkg/task/Cargo.toml`，分别证明 workspace 成员、库根/Go 映射/零外部依赖，以及 task crate 的路径依赖。
- Go 对照与调用点：`br/pkg/common/consts.go`、`br/pkg/task/backup_ebs.go::waitUntilAllScheduleStopped`、`br/pkg/restore/data/data.go::ReadRegionMeta`、`RecoverRegions`。
- 独立 Rust 测试：`br/pkg/common/consts_test.rs` 与 `br/pkg/common/parity_test.rs`。本任务是纯文档分析，按计划不运行 Cargo；结构验证另按任务指定命令执行。
