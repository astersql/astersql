# [`pkg/ddl/testargsv1/force_v1.rs`](force_v1.rs)

## 文件定位

`force_v1.rs` 属于独立 crate `astersql-ddl-testargsv1`，crate 边界由
`pkg/ddl/testargsv1/Cargo.toml` 定义。它不是 DDL 作业执行器，也不参与元数据持久化、
schema state 转换或回填；它只在编译期 feature `ddlargsv1` 启用时提供一个“测试必须使用
DDL Job V1 参数格式”的布尔开关。

模块入口 `pkg/ddl/testargsv1/lib.rs` 始终声明 `force_v1` 和 `normal` 两个模块，但通过
互斥的 `cfg` 条件选择 crate 根的公开再导出：启用 `ddlargsv1` 时再导出本文件，未启用时
再导出 `normal.rs`。工作区根 `Cargo.toml` 又把根包 feature `ddlargsv1` 转发为
`facade_ddl_testargsv1/ddlargsv1`，`pkg/lib.rs` 则把该 crate 暴露在
`ddl::testargsv1` 门面下。

## 核心职责

本文件只有一项职责：在 `ddlargsv1` 构建中把“强制 V1”表达为编译期常量 `true`，并同时
提供 Rust 风格的 `FORCE_V1` 与 Go 兼容名称 `ForceV1`。两个名称指向相同值，防止机械移植
的调用方因命名差异额外改写逻辑。

该开关的意图由同路径 Go 文件 `force_v1.go` 直接佐证：Go build tag
`ddlargsv1` 下的 `ForceV1` 同样恒为 `true`。本文件本身不选择某一种参数编解码器；真正的
消费者必须读取公开常量并据此分支。

## 主要符号

- `pub const FORCE_V1: bool = true`：仅在 `feature = "ddlargsv1"` 时存在，是 Rust
  规范命名的公开常量。
- `pub const ForceV1: bool = FORCE_V1`：同样只在该 feature 下存在；它是 Go 名称兼容
  别名，`#[allow(non_upper_case_globals)]` 只抑制命名 lint，不改变可见性或取值。

文件没有类型、trait、函数、`impl`、宏、可变静态量或运行时初始化代码。

## 执行流程

1. 构建方在工作区根启用 `ddlargsv1`；根 `Cargo.toml` 将 feature 转发给
   `astersql-ddl-testargsv1`。
2. 编译器满足本文件两个常量上的 `#[cfg(feature = "ddlargsv1")]`，因此生成
   `FORCE_V1 = true` 和 `ForceV1 = FORCE_V1`。
3. `pkg/ddl/testargsv1/lib.rs` 在同一条件下执行 `pub use force_v1::*`，使两个名称也可从
   crate 根访问；`normal.rs` 的对应常量因相反的 `cfg` 条件不参与该构建。
4. 消费者读取常量并选择 V1 兼容路径。当前仓库搜索只发现
   `migration_aster_unit_test.rs` 在 Rust 中直接读取它们；没有发现 Rust 生产逻辑消费该
   开关，因此不能把 Go 侧已经存在的运行时接线推断为 Rust 侧也已接线。

整个过程由条件编译和常量折叠完成，没有运行时调用栈。

## 数据与状态

本文件的数据模型仅是不可变的 `bool` 常量。`ForceV1` 由 `FORCE_V1` 初始化，两者在启用
feature 的构建中保持值相等这一不变量。文件不保存全局可变状态，不读取环境变量或配置，
也不修改 DDL 全局 Job 版本。

feature 关闭时，这两个定义不会被编译；crate 根改为再导出 `normal.rs` 中值为 `false`
的同名常量。因此调用方从 crate 根读取时可保持统一 API，而取值由构建配置决定。

## 依赖与调用关系

上游装配关系为：工作区根 `Cargo.toml` 的 `ddlargsv1` feature →
`astersql-ddl-testargsv1/ddlargsv1` → `lib.rs` 的条件再导出 → 本文件的两个常量。
`pkg/lib.rs` 进一步通过 `ddl::testargsv1` facade 再导出该 crate。

本文件没有 `use` 声明和下游函数调用。RustCodeGraph 将 `force_v1.rs` 识别为一个含 1 个
文件级符号的索引文件并报告 `used by 0 files`；对 `FORCE_V1`、`ForceV1` 的精确
`query/callers/callees` 未建立独立定义节点。局部 `rg` 补充确认，Rust 直接引用仅位于
`pkg/ddl/testargsv1/migration_aster_unit_test.rs`，而 `pkg/lib.rs` 只引用 facade crate。

Go 侧的行为消费者位于 `pkg/ddl/ddl.go`：`(*ddl).detectAndUpdateJobVersion` 在无 etcd
客户端时读取 `testargsv1.ForceV1`，据此将全局 Job 版本设为 V1 或 V2。这个调用边用于解释
移植意图，不代表当前 Rust 生产路径已经连接到该函数。

## 错误处理与边界

本文件没有返回值、`Result`、panic 或 I/O，因而没有运行时错误传播。主要边界来自编译
配置：直接依赖 `force_v1::FORCE_V1` 的代码只能在 `ddlargsv1` feature 启用时编译；需要
同时支持两种构建的调用方应读取 crate 根再导出的 `FORCE_V1`/`ForceV1`，而不是绕过
`lib.rs` 的条件选择。

常量只表达“是否强制 V1”，不保证任意具体 DDL Job 类型已经支持 V1，也不处理集群版本
探测、升级兼容或 NextGen 限制。这些规则属于实际 DDL 版本选择逻辑，不能由此文件推导。

## 并发与资源生命周期

没有锁、原子量、线程、异步任务、通道、事务、句柄或堆资源。常量在编译期确定，所有读取
都是无副作用的值读取，不存在初始化顺序、释放、取消或竞争问题。

若未来消费者据此修改全局 Job 版本，其并发与恢复语义应由消费者自身负责；本文件不提供
同步保证。

## 与 Go 版本的对应关系

`force_v1.rs` 对应 `pkg/ddl/testargsv1/force_v1.go`：Go 使用
`//go:build ddlargsv1` 选择文件，并声明 `const ForceV1 = true`；Rust 使用
`#[cfg(feature = "ddlargsv1")]` 和 `lib.rs` 的条件再导出实现等价的构建选择。
Rust 额外提供 `FORCE_V1` 作为惯用名称，再以 `ForceV1` 保留 Go 公共标识符。

默认分支也保持对称：Go 的 `normal.go` 在 `!ddlargsv1` 下令 `ForceV1 = false`，Rust 的
`normal.rs` 在相反 feature 条件下提供两个值为 `false` 的名称。Go 生产代码已经在
`pkg/ddl/ddl.go` 的 `detectAndUpdateJobVersion` 中消费该值；仓库内尚未找到对应的 Rust
生产消费者，这是当前迁移状态的明确限制。

## 扩展指南

- 若只需新增消费者，优先从 `astersql-ddl-testargsv1` crate 根或
  `ddl::testargsv1` facade 读取统一名称，避免直接耦合 `force_v1` 模块。
- 若修改 feature 名称或转发方式，必须同步检查根 `Cargo.toml`、本 crate 的
  `Cargo.toml`、`lib.rs` 以及 `normal.rs`，保证两个构建分支互斥且均提供相同公开 API。
- 若改变常量语义，应同步 Go 的 `force_v1.go`/`normal.go` 和真实版本选择消费者；不能只
  修改测试开关而假定 DDL Job 编解码路径会自动改变。
- Rust 测试应继续放在独立文件 `migration_aster_unit_test.rs`，覆盖 feature 开启与关闭
  两种构建下模块内名称、crate 根再导出名称及值的一致性，不要把测试嵌入本源文件。
- 若将开关接入 Rust 生产 DDL 逻辑，需要另行验证 Job V1/V2 编解码、升级兼容和全局状态
  并发；这些均超出本常量文件的职责。

## 验证依据

- 源码：`pkg/ddl/testargsv1/force_v1.rs`（`FORCE_V1`、`ForceV1` 及其 `cfg` 条件）。
- crate 与 feature：`pkg/ddl/testargsv1/Cargo.toml`、`pkg/ddl/testargsv1/lib.rs`、根
  `Cargo.toml`、`pkg/lib.rs`。
- Rust 独立测试：`pkg/ddl/testargsv1/migration_aster_unit_test.rs`；其中
  `ddlargsv1_feature_forces_v1_jobs` 同时断言模块路径与 crate 根再导出均为 `true`，
  `default_build_uses_current_ddl_args` 验证默认分支均为 `false`。
- Go 对照与行为证据：`pkg/ddl/testargsv1/force_v1.go`、`normal.go`、
  `pkg/ddl/ddl.go` 的 `detectAndUpdateJobVersion`、`pkg/ddl/ddl_test.go` 的
  `TestDetectAndUpdateJobVersion`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter
  pkg/ddl/testargsv1` 列出本 crate 的 6 个 Go/Rust 文件；文件节点读取确认本文件 33 行且
  `used by 0 files`。精确常量调用图未被索引，因此以局部 `rg` 核验引用，未发现 Rust
  生产消费者。
- 本任务是纯文档分析，按计划不运行 Cargo；结构检查应确保本文恰有规定的 11 个二级标题。
