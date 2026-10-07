# `br/pkg/conn/util/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-conn-util` 的 crate 根，而不是连接算法的实现文件。根工作区 [`Cargo.toml`](../../../../Cargo.toml) 将 `br/pkg/conn/util` 列为独立成员，本目录的 [`Cargo.toml`](Cargo.toml) 又以 `[lib] path = "lib.rs"` 指定本文件为库入口，并通过 `package.metadata.porting.go-package = "br/pkg/conn/util"` 标记它对应的 Go 包。

该 crate 位于 BR 的连接辅助层：真正的 TiKV/PD store 筛选、时间戳、HTTP 配置读取和地址规范化逻辑在 [`util.rs`](util.rs)，精简的 `kvproto::metapb` 数据模型在 [`stubs.rs`](stubs.rs)。本文件只决定这些实现以什么模块路径进入 crate，以及测试在何时参与编译。

## 核心职责

本文件有四项装配职责：

1. 用 crate 级 `#![allow(...)]` 暂时容纳 Go 到 Rust 迁移产生的命名和未使用项，包括 `non_snake_case`、`non_camel_case_types`、`non_upper_case_globals`、`dead_code`、`unused_imports` 与 `unused_variables`。
2. 通过 `#[path = "stubs.rs"] pub mod stubs` 把本地协议桩接入公开模块树。
3. 通过 `pub use stubs::kvproto` 把 `kvproto` 提升到 crate 根；因此 [`util.rs`](util.rs) 和测试可写 `crate::kvproto::metapb`，外部使用者也能沿 `<crate>::kvproto` 访问同一组桩类型。
4. 通过 `#[path = "util.rs"] pub mod util` 暴露真实辅助逻辑；两份测试模块则只在测试构建中装配，且保持私有。

它没有把 `util::*` 平铺重导出到 crate 根，所以公共函数的稳定路径是 `<crate>::util::GetAllTiKVStores` 一类路径，而不是 `<crate>::GetAllTiKVStores`。

## 主要符号

- `pub mod stubs`：公开 [`stubs.rs`](stubs.rs) 的完整命名空间。该文件定义 `kvproto::metapb::{StoreState, StoreLabel, Store}`，用于避免此精简 crate 在 darwin arm64 上引入 `kvproto/grpcio`；依据是本目录 [`Cargo.toml`](Cargo.toml) 的依赖说明。
- `pub use stubs::kvproto`：公开重导出，不复制数据或实现；它为 `util.rs` 中的 `use crate::kvproto::metapb::{self, Store}` 建立可解析路径。
- `pub mod util`：公开装配 [`util.rs`](util.rs)。该模块持有 `CancelContext`、`HttpClient`、`StoreMeta`、`PdClient` 等抽象，以及 store 筛选、PD TS 获取、重试、配置拉取和地址处理函数。
- `mod parity_test`：由 `#[cfg(test)]` 控制的私有契约测试模块，对应 [`parity_test.rs`](parity_test.rs)。
- `mod util_test`：由 `#[cfg(test)]` 控制的私有补充回归模块，对应 [`util_test.rs`](util_test.rs)。
- crate 级 `allow` 属性：仅改变 lint 诊断，不改变可见性、控制流或运行时行为。它的范围覆盖本文件声明的整个 crate，包括子模块。

本文件没有常量、类型、trait、函数或 `impl`，也没有 feature gate；条件编译仅用于两份测试模块。

## 执行流程

生产构建时，编译器先把本文件当作 crate 根应用 lint 配置，然后按显式 `path` 解析 `stubs.rs` 和 `util.rs`。`stubs` 先在模块树中可用，`pub use` 再建立根级 `kvproto` 别名；`util.rs` 随后可通过 `crate::kvproto` 使用桩协议类型。调用者进入 `<crate>::util` 后，运行的才是 `util.rs` 中的实际逻辑，本文件不会参与每次请求的动态调度。

测试构建多两个装配步骤：`cfg(test)` 成立后加载 `parity_test.rs` 和 `util_test.rs`。两者都能访问私有 crate 上下文，并分别经 `crate::util` 与 `crate::kvproto` 验证实现和协议桩；正常库构建不会编译这两个模块。

## 数据与状态

本文件自身不分配、缓存或变更任何运行时数据。它装配的状态分属下游文件：

- [`stubs.rs`](stubs.rs) 保存 `Store` 的 id、地址、状态、心跳与标签，以及 `StoreLabel` 的键值；这些是本地值类型，不是远端 PD 状态的实时镜像。
- [`util.rs`](util.rs) 主要接收调用者提供的 context、PD/store 接口和 HTTP client，并返回筛选结果、时间戳、配置内容或错误；crate 根不持有全局单例。
- [`parity_test.rs`](parity_test.rs) 的 mock 使用 `AtomicU32` 记录重试次数和响应关闭次数，属于测试观测状态，不会进入生产构建。

公开重导出只提供类型路径别名，不产生第二份 `kvproto` 状态，也不改变 `Store` 的所有权关系。

## 依赖与调用关系

上游边界首先是 Cargo：根 workspace 纳入此 crate，但仓库内其他 Cargo manifest 未直接声明 `astersql-br-pkg-conn-util` 依赖。因此当前可验证事实是它可作为独立 workspace 库构建和测试；不能据此声称 BR 主程序已经通过 Cargo 调用它。

本文件的直接下游关系为：

- `lib.rs -> stubs.rs`：模块装配；
- `lib.rs -> stubs::kvproto`：公开重导出；
- `lib.rs -> util.rs`：公共实现模块装配；
- `lib.rs -[cfg(test)]-> parity_test.rs / util_test.rs`：测试专用装配。

再下一层，[`util.rs`](util.rs) 依赖本 crate 的 `kvproto::metapb`，并使用 `astersql-br-pkg-errors`、`astersql-br-pkg-logutil`、`astersql-br-pkg-utils`、`astersql-util-engine`、`astersql-errors` 与 `tracing`；这些依赖均由本目录 [`Cargo.toml`](Cargo.toml) 声明。RustCodeGraph 对 `GetAllTiKVStores` 的调用边显示其直接被同文件的 `GetAllTiKVStoresWithRetry` 调用，并下调 `StoreMeta::GetAllStores`、TiFlash 判定及错误注解逻辑；这证明 crate 根装配的是可执行实现，而非空门面。

## 错误处理与边界

crate 根没有 `Result`、panic 或恢复分支；路径拼写和模块缺失属于编译期错误。运行时错误完全由 `util.rs` 传播，例如 PD 查询失败、活跃 TiFlash 与策略冲突、无 status address、HTTP 非成功状态、读取/回调失败及重试取消。

需要特别区分两个边界：

- `pub use stubs::kvproto` 表明当前协议类型来自本地精简桩，不等价于完整 `kvproto` API。扩展字段或方法时必须同时检查 `stubs.rs`、Go 的 `metapb` 使用方式及相关测试，不能假定生成 protobuf 的全部能力存在。
- crate 级宽泛 `allow` 会隐藏未使用代码和 Go 风格命名告警。它适合当前迁移兼容层，但新增 Rust 原生 API 不应依赖这些豁免来掩盖无效接线或不清晰命名。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或 I/O 资源，也没有初始化/销毁钩子。模块在编译期确定，公开重导出的生命周期与 crate 相同。

资源语义存在于被装配模块及测试：`util.rs` 的重试循环受 `CancelContext` 控制，HTTP 响应在每次回调后经 `HttpClient::CloseResponse` 关闭；`parity_test.rs::config_response_is_closed_after_callback_failure` 验证回调失败仍关闭响应，`cancellation_during_backoff_stops_before_another_pd_call` 验证取消后不再发起第二次 PD 调用。测试计数器使用原子类型，避免并发观测产生数据竞争；这些都不是 `lib.rs` 自己维护的状态。

## 与 Go 版本的对应关系

Go 的 [`util.go`](util.go) 是普通 `package util` 文件，类型与函数天然位于同一包命名空间；Rust 版本则用 `lib.rs` 人工建立 crate 边界，再把实现留在 `util` 子模块中。因此二者在公开路径上不是逐字同构：Go 调用 `util.GetAllTiKVStores`，Rust 当前路径是 `<crate>::util::GetAllTiKVStores`。

行为对应由下游实现和测试保证：`util.rs` 镜像 Go 的 `StoreBehavior`、store 过滤、PD 时间戳组合、带 aggressive backoff 的重试、逐 store 配置获取和 status/node host 修正；`parity_test.rs::go_rust_public_contract_matches` 覆盖正常、边界、错误与重试/资源场景，额外测试固定 HTTP 状态文本、响应关闭、取消和 URL 路径/查询保留；`util_test.rs` 固定 Go `url.JoinPath` 的路径清理语义和 `oracle.ComposeTS` 的加法进位语义。

迁移差异也必须保留在认知中：Go 直接依赖真实 `github.com/pingcap/kvproto/pkg/metapb`、`pd.Client` 与 `http.Client`；此 Rust crate 为精简构建使用本地 `metapb` 桩和自定义 trait。它验证的是所需可观察契约，不代表完整客户端栈已经在本 crate 接线。

## 扩展指南

- 新增通用连接逻辑时，优先放入 [`util.rs`](util.rs)，再从 `pub mod util` 的既有路径公开；只有确实需要缩短公共路径时才在 `lib.rs` 增加精确 `pub use`，并评估 API 兼容性。
- 新逻辑若需要新的 protobuf 字段，必须同步扩展 [`stubs.rs`](stubs.rs)，并核对 Go [`util.go`](util.go) 对真实 `metapb` 的用法；不要把完整外部依赖复制进本仓库或在 crate 根伪造行为。
- Go 对齐场景应扩充独立的 [`parity_test.rs`](parity_test.rs)；Rust 特有的 URL/数值边界可扩充 [`util_test.rs`](util_test.rs)。不要把测试写进 `lib.rs` 或 `util.rs`，以遵守生产代码与测试文件分离的仓库约定。
- 新测试文件必须同时在这里增加 `#[cfg(test)]` 和 `#[path = "..."] mod ...`；否则文件存在但不会进入该 crate 的测试编译图。
- 若要接入 BR 主链，除实现本 crate 外还需在真实调用方的 Cargo manifest 增加带路径或已发布版本的依赖并替换重复实现；当前仓库搜索未发现这种依赖，不能仅凭 workspace membership 认为接线完成。
- 性能风险主要来自下游重试次数、store 遍历、HTTP body 分配与桩/真实客户端差异；只改模块导出通常没有运行时成本，但扩大公共面会增加兼容维护成本。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 Rust 7,032 个；本目录列出 `lib.rs`、`stubs.rs`、`util.rs`、两份 Rust 测试和 Go `util.go`。
- RustCodeGraph `node --file br/pkg/conn/util/lib.rs --offset 1 --limit 200`：确认本文件共 37 行、公开模块/重导出与两个 `cfg(test)` 模块；`node` 对 `stubs.rs`、`util.rs` 确认协议桩和实际实现位置。
- RustCodeGraph `query/node GetAllTiKVStores`：确认 Rust 实现在 `util.rs:499`，其调用 `StoreMeta::GetAllStores`、TiFlash 判定和错误注解，并由 `GetAllTiKVStoresWithRetry` 调用。
- 已读源码与配置：[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、根 [`Cargo.toml`](../../../../Cargo.toml)、[`stubs.rs`](stubs.rs)、[`util.rs`](util.rs)、Go [`util.go`](util.go)。
- 已读独立测试：[`parity_test.rs`](parity_test.rs) 与 [`util_test.rs`](util_test.rs)；测试由本文件显式装配，没有内嵌在生产源文件中。
- 仓库搜索 `astersql-br-pkg-conn-util|astersql_br_pkg_conn_util`：仅发现本 crate 自身的包名声明；根 workspace 另以路径纳入该成员，据此将“尚无其他 Cargo crate 直接依赖”记录为当前接线边界。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰有 11 个固定二级章节，并人工复核链接、符号、调用边和“未接入主链”表述。
