# [`br/pkg/mock/lib.rs`](./lib.rs)

## 文件定位

`br/pkg/mock/lib.rs` 是 Cargo 包 `astersql-br-pkg-mock` 的 crate 根。`br/pkg/mock/Cargo.toml` 的 `[lib] path = "lib.rs"` 把它指定为库入口，根 `Cargo.toml` 又把 `br/pkg/mock` 列为 workspace member。该文件位于 BR 测试支撑层：它把同目录的 mockgen 风格 Rust 移植、集群组合桩和本地替身装配成一个统一 crate，而不实现备份、恢复或导入的生产算法。

文件头注释明确约束“生产代码路径不应依赖这些 mock”。当前可确认的 Rust 下游是测试辅助 crate：`br/pkg/mock/mocklocal/Cargo.toml` 与 `br/pkg/utiltest/Cargo.toml` 都依赖 `astersql-br-pkg-mock`；其中 `mocklocal/local.rs` 使用 `Call`、`Context`、`Controller`，`br/pkg/utiltest/suite.rs` 使用根级 `Cluster`、`NewCluster`。

## 核心职责

该入口只有装配职责，没有函数体或运行时分支：

1. 通过 `#[path = "..."] pub mod ...` 装入 `stubs`、`backend`、`common`、`encode`、`importer`、`mock_cluster`、`task_register` 七个公开子模块。
2. 通过 `pub use` 把各 mock 构造器、mock 类型、集群帮助函数以及选定的桩类型平铺到 crate 根，使 Rust 调用方获得接近 Go 单一 `package mock` 的导入体验。
3. 通过 `#[cfg(test)]` 仅在本 crate 的测试构建中挂入 `parity_test.rs`、`mock_cluster_test.rs`、`importer_test.rs`，保持测试逻辑与生产入口分文件。
4. 通过 crate 级 `#![allow(...)]` 容纳 Go/mockgen 风格的公开命名和机械移植代码，包括非 snake case、非 camel case、未使用参数和 Clippy 告警。

## 主要符号

- `pub mod stubs`：最先装入的本地替身层。它提供后续模块签名需要的 `Controller`、`Call`、`Context`、`Error`、`Result`、请求/响应结构、PD/TiKV/SQL Server 句柄等轻量类型。
- `pub mod backend`：对照 `backend.go` 的 Backend、EngineWriter、TargetInfoGetter mock；其公开项经 `pub use backend::*` 全量提升。
- `pub mod common`：对照 `common.go` 的 ChunkFlushStatus mock；经通配再导出提升。
- `pub mod encode`：对照 `encode.go` 的 Encoder、EncodingBuilder、Rows、Row mock；经通配再导出提升。
- `pub mod importer`：对照 `importer.go` 的 ImportKV 客户端和 WriteEngine 流 mock；经通配再导出提升。
- `pub mod mock_cluster`：提供 `Cluster`、`NewCluster`、`getDSN`、`waitUntilServerOnline` 等组合桩和生命周期帮助函数；经通配再导出提升。
- `pub mod task_register`：对照 `task_register.go` 的 TaskRegister mock；经通配再导出提升。
- `pub use stubs::{...}`：这是有意收窄的根级桩类型白名单，而不是 `stubs::*`。白名单包含控制器/调用记录、公共错误与结果、编码/后端接口类型、ImportKV 请求响应、集群资源句柄等；其他桩实现仍可通过 `crate::stubs` 访问。
- `mod parity_test`、`mod mock_cluster_test`、`mod importer_test`：三个私有测试模块，均受 `#[cfg(test)]` 保护，不属于普通依赖者的公共 API。

## 执行流程

作为 crate 根，它的“执行”发生在编译和名称解析阶段，而非运行时：

1. 编译器先应用 crate 级 lint 宽免。
2. 按声明顺序解析七个子模块；`stubs` 在最前，使其他移植模块可以通过 `crate::stubs` 使用公共替身类型。
3. `pub use backend::*` 等声明把各模块公开项加入 crate 根命名空间；`stubs` 只提升显式列表中的类型，以控制根级 API 面。
4. 外部 crate 可以写 `use astersql_br_pkg_mock::{Cluster, NewCluster}`，也可以使用模块限定路径，例如 `astersql_br_pkg_mock::stubs::take_one`。
5. 仅当本 crate 以测试配置编译时，编译器再纳入三个独立测试文件。测试从 `crate::...` 访问根级再导出，从而同时验证入口装配是否完整。

实际运行时行为由被装入的子模块决定。例如 `NewCluster` 在 `mock_cluster.rs` 中创建 Storage、Domain 和 PD 客户端，`Cluster::Start` 建立 Server/DSN，`Cluster::Stop` 关闭资源；`lib.rs` 只让这些入口可从 crate 根到达。

## 数据与状态

本文件自身不定义常量、结构体、trait、函数、静态变量、锁或通道，也不持有运行时状态。它暴露的数据与状态全部归属于子模块：

- mock 调用期望和剩余调用数由 `stubs::Controller`/`Call` 及各 mock 对象维护。
- 集群资源状态由 `mock_cluster::Cluster` 持有，包括 `Server`、`Storage`、`Domain`、PD 客户端、`DSN` 与可选 `HttpServer`。
- SQL/HTTP 探针、sleep、online 状态等测试钩子位于 `stubs`，不在入口中初始化。
- importer 请求/响应、编码配置和句柄等只是经根级白名单转出的类型；入口不复制或转换它们。

因此，修改再导出会改变名称可见性和下游编译契约，但不会直接改变对象布局或运行时状态机。

## 依赖与调用关系

向下依赖由七条模块声明构成：`lib.rs → stubs/backend/common/encode/importer/mock_cluster/task_register`。其中 mockgen 风格模块依赖 `stubs` 提供的控制器、参数占位和接口替身；`mock_cluster` 也依赖 `stubs` 提供 mock Storage、Server、PD 客户端以及测试钩子。`br/pkg/mock/Cargo.toml` 的 `[dependencies]` 为空，说明这一 crate 当前刻意以本地桩隔离重型 TiDB、grpcio、kvproto 等依赖；Cargo 注释也明确这是 darwin arm64 的精简边界。

向上关系有两类直接证据：

- `br/pkg/mock/mocklocal/Cargo.toml` 依赖父 crate，`mocklocal/local.rs` 使用 `astersql_br_pkg_mock::{Call, Context, Controller}`，并从 `astersql_br_pkg_mock::stubs` 取辅助函数。
- `br/pkg/utiltest/Cargo.toml` 依赖该 crate，`br/pkg/utiltest/suite.rs` 使用 `astersql_br_pkg_mock::{Cluster, NewCluster}`，`parity_test.rs` 也直接构造根级 `Cluster`。

RustCodeGraph 的文件节点把 `lib.rs` 标记为被 75 个文件关联，并确认该文件只有模块/导出装配符号；精确 `NewCluster` 查询定位到 `br/pkg/mock/mock_cluster.rs:76`，其签名为 `() -> Result<Cluster>`。由于 crate 根没有可执行函数，不能把子模块内部的 calls/callees 边误写成 `lib.rs` 自身的调用边。

Go/Bazel 侧的 `br/pkg/mock/BUILD.bazel` 将六个 `.go` 文件合成公开 `go_library(name = "mock")`，并被 backup、restore、checksum、task、Lightning 等多个 Go 测试目标依赖；这些 Bazel 边说明 Go 包用途，但不等同于 Rust Cargo 直接依赖边。

## 错误处理与边界

入口本身不产生或传播 `Result`，也没有 panic、重试或 I/O。错误边界由再导出的子模块 API 决定：根级白名单公开 `Error` 与 `Result`，mock 方法把录制的错误返回给调用者；`NewCluster` 等组合入口在其实现文件中传播初始化错误。

需要注意以下装配边界：

- 通配再导出可能因子模块新增同名公开项而在 crate 根产生名称冲突或意外扩大 API；扩展时必须做根级可见性检查。
- `stubs` 使用显式白名单。新增桩类型即使在 `stubs.rs` 中为 `pub`，也不会自动成为根级符号；调用方仍需走 `stubs::Type`，除非同步更新该列表。
- 三个测试模块只在 `cfg(test)` 下存在，外部依赖者不能引用其中的辅助函数。
- crate 级 lint 宽免是移植兼容措施，会隐藏未使用项和命名告警；它不能作为忽略语义缺失或错误处理缺口的理由。
- 当前 Cargo 无外部依赖且子模块使用轻量桩。不能据此宣称它启动真实 TiDB/PD/TiKV；`parity_test.rs` 明确只验证形状、返回传播和资源关闭副作用。

## 并发与资源生命周期

`lib.rs` 不创建线程、不持锁、不创建通道，也没有 Drop/关闭逻辑。它只暴露含并发或资源生命周期的子模块 API：

- `mock_cluster::NewCluster`/`Cluster::Start`/`Cluster::Stop` 构成两阶段生命周期；`Start` 后必须调用 `Stop`。`mock_cluster_test.rs::test_smoke` 验证 Domain、Storage、Server 和可选 HttpServer 在停止后已关闭。
- `mock_cluster` 内的进程级 `Once` 让 pprof 初始化只发生一次；`pprof_server_is_not_shared_by_later_clusters` 验证后续 Cluster 不共享首次实例的 server 句柄。
- `parity_test.rs` 使用原子计数、可注入钩子并在用例前后 reset，防止全局状态泄漏；这些同步机制属于测试与桩实现，不属于入口本身。
- importer 流 mock 的 `Send`、`Header`、`CloseAndRecv` 生命周期由录制期望驱动；`importer_test.rs` 验证 Header/CloseAndRecv 的错误原样传播和期望被消费。

安全扩展入口时，应保持“生命周期实现留在独立模块、入口只负责声明和导出”的边界，不要把线程启动或资源清理塞入 `lib.rs`。

## 与 Go 版本的对应关系

Go 没有对应的单一 `lib.go`：同目录 `backend.go`、`common.go`、`encode.go`、`importer.go`、`mock_cluster.go`、`task_register.go` 共享 `package mock`，Go 编译器天然把它们合并为一个包。Rust 必须通过 `lib.rs` 的 `mod` 和 `pub use` 显式重建这种包级可见性。

对应关系如下：

- `backend/common/encode/importer/task_register` 的 Rust 模块分别对照同名 Go MockGen 文件，保留 `NewMock...`、`EXPECT` 和方法名等 Go 风格 API。
- `mock_cluster` 对照手写 `mock_cluster.go` 的 `Cluster`、`NewCluster`、`Start`、`Stop`、DSN 与在线探针流程。
- `stubs` 没有单一 Go 文件对应；它是 Rust 为避免直接链接 Go 侧重型依赖而建立的本地类型/行为边界。
- Go 的公开性由标识符首字母和同包访问决定；Rust 入口通过公开模块、通配再导出和桩白名单近似这一体验，但并非所有 `stubs` 公开项都位于 crate 根。
- Go `mock_cluster_test.go` 使用外部包 `mock_test` 做生命周期冒烟测试；Rust 的 `mock_cluster_test.rs` 由入口以内嵌测试模块方式编译。二者装配方式不同，但都检查 `NewCluster → Start → Stop` 主路径。

## 扩展指南

新增或修改 mock 能力时按最小边界接入：

1. 若扩展现有 Go 对照模块，优先修改相应独立 `*.rs` 实现和同目录独立测试，不要把逻辑写入 `lib.rs`。
2. 若新增子模块，在入口增加显式 `#[path] pub mod`；只有确实需要 Go 包式根级访问时才增加 `pub use`，并先检查同名冲突。
3. 若新增公共桩类型，先判断调用方应使用 `stubs::Type` 还是稳定根级 `Type`。只有后者才加入 `pub use stubs::{...}` 白名单。
4. 新增 mockgen 风格 API 时同步更新 `parity_test.rs`，覆盖构造器、EXPECT/Call、成功返回与错误传播；Importer 流边界同步更新 `importer_test.rs`；Cluster 生命周期同步更新 `mock_cluster_test.rs` 和必要的 Go 对照测试。
5. 保持 Rust 测试逻辑在独立测试文件中，并由 `#[cfg(test)]` 挂接；不要把单元测试内嵌进 `lib.rs`。
6. 保持 Cargo 依赖精简。若真实功能必须引入外部 Rust 依赖，需遵守仓库的上游移植、提交、打 tag 和统一 Git tag 引用规则，不能用本地 `[patch]` 或 vendor 副本。

兼容风险主要是根级符号移除/重命名导致下游编译失败、通配导出冲突，以及桩签名偏离 Go 接口；性能风险通常不在入口本身，而在新增模块是否意外引入重型依赖或把真实 I/O 带入测试路径。

## 验证依据

- `br/pkg/mock/lib.rs:13-21`：crate 级 lint 宽免；`23-42`：七个公开子模块；`44-61`：根级再导出；`63-73`：三个独立测试模块及 `cfg(test)`。
- `br/pkg/mock/Cargo.toml`：包名、`[lib] path`、Go 包映射、空依赖集和精简边界说明；根 `Cargo.toml:25`：workspace 成员关系。
- RustCodeGraph：`status` 显示索引覆盖本仓库 7032 个 Rust 文件；`files --filter br/pkg/mock` 列出本 crate 的入口、实现与测试；`node --file br/pkg/mock/lib.rs` 返回完整 73 行及 75 个关联文件；精确查询把 Rust `NewCluster` 定位到 `mock_cluster.rs:76`，签名为 `() -> Result<Cluster>`。
- Rust 直接调用证据：`br/pkg/mock/mocklocal/local.rs`、`br/pkg/mock/mocklocal/stubs.rs`、`br/pkg/mock/mocklocal/parity_test.rs`、`br/pkg/utiltest/suite.rs`、`br/pkg/utiltest/parity_test.rs`；对应 Cargo 边位于 `br/pkg/mock/mocklocal/Cargo.toml` 和 `br/pkg/utiltest/Cargo.toml`。
- 独立 Rust 测试：`br/pkg/mock/parity_test.rs` 覆盖公开 mock 契约、错误传播、钩子复位和 Cluster 形状；`br/pkg/mock/mock_cluster_test.rs` 覆盖启动/停止、资源关闭与 `Once` 所有权；`br/pkg/mock/importer_test.rs` 覆盖流创建、Header、CloseAndRecv 错误传播。
- Go 对照：`br/pkg/mock/backend.go`、`common.go`、`encode.go`、`importer.go`、`task_register.go` 是同一 `package mock` 的 MockGen 产物；`mock_cluster.go` 是组合集群实现；`mock_cluster_test.go` 是对应生命周期测试；`br/pkg/mock/BUILD.bazel` 给出 Go 包文件集合和 Go 测试目标。
- 本任务只新增说明文档，未运行 Cargo。结构验证命令见任务验收记录；人工复核确认本文区分了 crate 装配事实、子模块运行时行为、Rust Cargo 依赖边和 Go/Bazel 使用边。
