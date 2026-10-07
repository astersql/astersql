# `pkg/ddl/testargsv1/normal.rs`

## 文件定位

[`normal.rs`](normal.rs) 属于 `astersql-ddl-testargsv1` crate 的默认编译分支。该 crate 由 [`Cargo.toml`](Cargo.toml) 定义，默认不启用任何 feature；当 `ddlargsv1` feature 未启用时，[`lib.rs`](lib.rs) 通过 `pub use normal::*` 将本文件的常量再导出到 crate 根。工作区根 [`Cargo.toml`](../../../Cargo.toml) 又把根 crate 的同名 feature 转发给 `facade_ddl_testargsv1/ddlargsv1`，因此默认构建与强制 V1 构建在编译期互斥。

它是测试参数版本选择的兼容开关，不负责创建、持久化或执行 DDL Job。就 DDL 执行框架而言，本文件本身不涉及 owner、schema state、reorg、回滚、系统表或 schema version 同步。

## 核心职责

- `FORCE_V1 = false` 表示默认构建不强制测试使用 DDL Job V1 参数格式；源码注释指出 DDL args V2 自 TiDB 8.4.0 起是默认格式。
- `ForceV1` 为 `FORCE_V1` 提供与 Go 导出名一致的兼容别名，使机械迁移后的调用点可以保留原有名称。
- 两个常量均受 `#[cfg(not(feature = "ddlargsv1"))]` 约束，与 [`force_v1.rs`](force_v1.rs) 中 feature 开启时为 `true` 的定义组成互斥分支。

该文件不在运行时探测版本，也不修改任何全局状态；它只把构建配置编码为布尔常量。

## 主要符号

- `pub const FORCE_V1: bool = false`：Rust 风格的公开常量。仅在未启用 `ddlargsv1` 时存在，是默认分支的唯一事实来源。
- `pub const ForceV1: bool = FORCE_V1`：Go 兼容名称。它没有独立状态，值始终跟随 `FORCE_V1`；局部 `#[allow(non_upper_case_globals)]` 只豁免命名告警。
- `#[cfg(not(feature = "ddlargsv1"))]`：两个符号的编译边界。调用方不能假设 `normal::FORCE_V1` 在 feature 开启时仍存在；启用 feature 后应使用 crate 根再导出的 [`force_v1.rs`](force_v1.rs) 符号。

本文件没有类型、trait、函数、`impl`、可变静态量或运行时初始化代码。

## 执行流程

1. Cargo 在未指定 `ddlargsv1` 时使用 [`Cargo.toml`](Cargo.toml) 的空 `default` feature 集。
2. 编译器保留本文件两个带 `not(feature = "ddlargsv1")` 的常量定义。
3. [`lib.rs`](lib.rs) 的同一条件分支执行 `pub use normal::*`，使调用方可从 `astersql_ddl_testargsv1::FORCE_V1` 或兼容名 `ForceV1` 读取 `false`。
4. 读取常量只产生编译期已知的布尔值；本文件没有后续调用、I/O 或副作用。

启用 `ddlargsv1` 时，上述定义和再导出均不参与编译，crate 根改为再导出 `force_v1` 分支的 `true`。因此选择发生在编译阶段，不存在运行时在两个模块之间切换的路径。

## 数据与状态

本文件的数据仅是不可变的 `bool` 常量：`FORCE_V1` 保存默认选择，`ForceV1` 是对前者的常量表达式别名。它不分配堆内存，不拥有配置对象，不读取环境变量，也不缓存 DDL 状态。

需要保持的不变量是：默认分支的两个名称都为 `false`；feature 分支的两个名称都为 `true`；crate 根在任一构建配置下只公开其中一组定义。这个不变量由 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 分别在两个条件编译分支中断言。

## 依赖与调用关系

- 上游模块接线：[`lib.rs`](lib.rs) 无条件声明 `pub mod normal`，并在 `not(feature = "ddlargsv1")` 时再导出其公开符号。
- 工作区接线：根 [`Cargo.toml`](../../../Cargo.toml) 以 `facade_ddl_testargsv1` 依赖本 crate，并将根 feature `ddlargsv1` 转发给它；[`pkg/lib.rs`](../../lib.rs) 再通过 `pkg::ddl::testargsv1` 暴露该 façade。
- 测试调用方：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `default_build_uses_current_ddl_args` 同时读取 `crate::normal::{FORCE_V1, ForceV1}` 与 crate 根再导出，验证四个观察点均为 `false`。
- 下游依赖：`ForceV1` 只依赖同文件的 `FORCE_V1`；除此以外没有函数调用或外部 crate 依赖。

RustCodeGraph 能列出本目录的六个 Go/Rust 文件并显示 `normal.rs` 全部源码，但其按常量名查询把 `FORCE_V1` 解析到 Go/feature 分支，未为本文件生成可用的精确 callers/callees 节点。因此调用边以条件再导出源码和仓库级引用搜索交叉核验。仓库搜索未发现除独立单测、crate 接线和 façade 外的 Rust 使用点；不能据 Go 侧调用链宣称 Rust 生产路径已经接线。

## 错误处理与边界

本文件没有 `Result`、错误类型、panic 或输入校验。主要边界来自条件编译：直接引用 `normal::FORCE_V1` 的代码在启用 `ddlargsv1` 时会因符号不存在而无法编译，所以跨配置调用方应优先使用 crate 根再导出。

布尔值只表达“是否强制 V1”，不负责判断某个 Job 类型是否支持 V2，也不实现集群升级、降级或兼容性决策。把它解释为“所有 DDL Job 必然使用 V2”会超出本文件证据；准确语义只是默认测试配置不强制 V1。

## 并发与资源生命周期

两个符号均为编译期常量，不包含锁、原子量、线程、异步任务、通道、事务、文件句柄或网络连接。读取它们不发生初始化和竞争，生命周期等同于编译产物。

Go 生产代码可能根据对应开关修改进程级 Job 版本状态，但那是 [`pkg/ddl/ddl.go`](../ddl.go) 的责任，不是本 Rust 文件的资源生命周期。当前仓库也未找到等价的 Rust 生产消费路径。

## 与 Go 版本的对应关系

直接对照文件 [`normal.go`](normal.go) 使用 `//go:build !ddlargsv1` 并声明 `const ForceV1 = false`；其注释同样说明 V2 自 8.4.0 起成为默认格式，同时保留 V1 兼容测试轮次。Rust 的 `#[cfg(not(feature = "ddlargsv1"))]`、`ForceV1 = false` 与该语义一一对应，并额外提供惯用名称 `FORCE_V1`。

Go 的互补文件 [`force_v1.go`](force_v1.go) 在 `ddlargsv1` build tag 下返回 `true`，对应 Rust 的 [`force_v1.rs`](force_v1.rs)。Go 侧实际生产接线位于 [`pkg/ddl/ddl.go`](../ddl.go) 的 `detectAndUpdateJobVersion`：无 etcd 客户端的测试/uni-store 路径根据 `testargsv1.ForceV1` 选择 `model.JobVersion1` 或 `JobVersion2`；[`pkg/ddl/ddl_test.go`](../ddl_test.go) 的 `TestDetectAndUpdateJobVersion` 覆盖这个分支。

Rust 当前只有开关 crate、条件再导出与局部单元测试，仓库引用搜索没有发现 Rust 版 `detectAndUpdateJobVersion` 使用该常量。因此常量值和构建选择已对齐 Go，但完整运行时接线不能视为已移植。

## 扩展指南

- 若调整默认版本，必须同步检查 `normal.rs`、[`normal.go`](normal.go)、互补的 `force_v1` 文件，以及 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的双 feature 断言；不要只改兼容别名。
- 新增调用方时优先依赖 crate 根的 `FORCE_V1`，避免直接绑定某个条件模块；若为了 Go 移植兼容使用 `ForceV1`，应保持它只是别名。
- 若把开关接入 Rust DDL 运行时，应在实际版本选择模块新增独立测试，覆盖默认与 `ddlargsv1` 两种构建配置，并参照 Go 的 `TestDetectAndUpdateJobVersion` 核对状态变化；测试逻辑不要内嵌到 `normal.rs`。
- 不应在本文件加入运行时探测、全局可变状态或 Job 编解码逻辑。此处适合保留为小型配置边界，实际决策应放在拥有 DDL Job 版本状态的模块中。
- 兼容风险主要是 feature/build-tag 语义漂移和公开名称变化；性能风险可忽略，因为读取编译期常量没有运行时分配或 I/O。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`node --file pkg/ddl/testargsv1/normal.rs --offset 1 --limit 260` 读取了本文件完整 36 行；`files --filter pkg/ddl/testargsv1` 确认 crate 相关的 Rust/Go 文件集合。常量级 `query/node/callers/callees` 未能唯一定位本文件符号，限制已在“依赖与调用关系”记录。
- 源码与配置：[`normal.rs`](normal.rs)、[`force_v1.rs`](force_v1.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、根 [`Cargo.toml`](../../../Cargo.toml) 和 [`pkg/lib.rs`](../../lib.rs)。
- Go 对照与行为测试：[`normal.go`](normal.go)、[`force_v1.go`](force_v1.go)、[`pkg/ddl/ddl.go`](../ddl.go) 的 `detectAndUpdateJobVersion`、[`pkg/ddl/ddl_test.go`](../ddl_test.go) 的 `TestDetectAndUpdateJobVersion`。
- Rust 独立测试：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 `ddlargsv1_feature_forces_v1_jobs` 与 `default_build_uses_current_ddl_args`。
- 仓库引用搜索：`rg -n 'astersql-ddl-testargsv1|testargsv1|FORCE_V1|ForceV1' --glob '*.rs' --glob '*.toml' --glob '*.go' --glob '*.bazel' --glob 'BUILD*'`，用于核验 façade、Cargo feature、Go 消费点和 Rust 测试引用。

本任务为纯文档分析，按计划不运行 Cargo。结构验证另行执行，以确认目标文件存在且恰好包含规定的十一个二级章节。
