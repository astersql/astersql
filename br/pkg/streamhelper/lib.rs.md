# `br/pkg/streamhelper/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-streamhelper` 的 crate 根。[`Cargo.toml`](Cargo.toml) 以 `[lib] path = "lib.rs"` 指定该入口，并用 `package.metadata.porting.go-package = "br/pkg/streamhelper"` 标明 Go 对照包。它不是单一算法实现，而是日志备份 streamhelper 的编译期组装与兼容门面：挂载同目录的生产模块、在测试构建中挂载独立测试文件，并将公开项扁平再导出到 crate 根。

该门面让调用者既可用 `astersql_br_pkg_streamhelper::regioniter::Store` 这样的模块路径，也可用 `astersql_br_pkg_streamhelper::Store` 这样的 Go 包级风格路径。真实的 region 扫描、任务元数据、flush 订阅、checkpoint 收集和推进逻辑均在相邻实现文件中；不能把本文件的再导出误写成这些行为的实现。

## 核心职责

- 通过显式 `#[path] pub mod` 挂载 11 个生产模块：`stubs`、`models`、`regioniter`、`prefix_scanner`、`client`、`collector`、`flush_subscriber`、`advancer_env`、`advancer_cliext`、`advancer_daemon` 和 `advancer`。
- 通过具名 `pub use` 暴露 `advancer`、`advancer_env` 的选定 API，并对其余九个模块使用 glob 再导出，形成接近 Go `streamhelper` 包级命名空间的公共面。
- 仅在 `cfg(test)` 下挂载 15 个独立 `*_test.rs`/测试辅助模块；生产构建不包含这些模块，Rust 测试逻辑也没有内嵌在生产文件中。
- 用 crate 级 `allow` 接纳 Go 风格的导出命名和迁移期未使用项。该 lint 策略不产生运行时行为，也不能作为实现完整性的证据。
- 维持 crate 边界。相邻的 `config`、`spans` 是独立 Cargo 包；相邻的 `daemon` 也是独立 crate，且本文件挂载的是当前目录的 [`advancer_daemon.rs`](advancer_daemon.rs)，不是 `daemon/lib.rs`。

## 主要符号

本文件自身不声明常量、结构体、枚举、trait、函数或 `impl`；它的符号面完全由模块声明和再导出组成。

- `advancer` 的具名再导出包括 `CheckpointAdvancer`、三个构造入口 `NewCheckpointAdvancer`/`NewCommandCheckpointAdvancer`/`NewTiDBCheckpointAdvancer`，以及 resolve-lock 和 checkpoint 计算辅助函数。具名列表限制了该模块哪些 `pub` 项能从 crate 根访问。
- `advancer_env` 的具名再导出包括 `Env`、`StreamMeta`、`RegionLockResolver`、`LogBackupFlushIntervalGetter`、TiKV flush interval 读取/解析函数，以及 safepoint、服务 ID、超时等常量。
- `advancer_cliext::*`、`advancer_daemon::*`、`client::*`、`collector::*`、`flush_subscriber::*`、`models::*`、`prefix_scanner::*`、`regioniter::*`、`stubs::*` 会把对应模块所有公开项引入 crate 根。新增公开项可能自动扩大根 API 或造成名字冲突。
- 15 个 `mod ..._test` 均为私有条件编译模块。其中 [`parity_test.rs`](parity_test.rs) 从 `crate::{...}` 大量导入根级符号，直接验证扁平公共契约；[`export_test.rs`](export_test.rs) 对应 Go `export_test.go`，只为 crate 内测试暴露辅助能力。

## 执行流程

1. Cargo 读取本 crate 的 `lib.rs`，编译器按 `#[path]` 解析 11 个生产模块；测试构建再加入 15 个测试模块。
2. 编译器解析各模块公开项，并按本文件的具名或 glob `pub use` 建立 crate 根 API。这个阶段没有业务请求、I/O 或后台任务。
3. 上层调用者从根路径或公开模块路径取得模型、trait、构造器与算法入口。例如 [`br/pkg/utiltest/crr/pd_sim_service.rs`](../utiltest/crr/pd_sim_service.rs) 从根路径取得 `StreamMeta`、`LogBackupService`、`TaskEvent` 等契约并为模拟 PD 实现它们；[`br/pkg/stream/crr/internal/checkpoint/lib.rs`](../stream/crr/internal/checkpoint/lib.rs) 再导出根级 `Store`。
4. 真正运行时，调用者创建 `MetaDataClient`、`CheckpointAdvancer` 或 subscriber/collector 后，执行流才进入对应实现模块：任务和 pause 元数据落在 `client`/`models`，拓扑枚举在 `regioniter`，flush 事件在 `flush_subscriber`，检查点聚合在 `collector`，推进、GC safepoint 和 resolve-lock 在 `advancer` 及其环境/扩展模块。
5. 本文件不负责启动 BR 命令。当前 Rust [`br/cmd/br/stream.rs`](../../cmd/br/stream.rs) 只直接依赖 streamhelper 的 `config` 子 crate；不能据 Go 主链推断 Rust CLI 已从本门面构造并运行完整 advancer。

## 数据与状态

`lib.rs` 本身没有运行时数据、全局可变状态或配置值。它只决定类型和符号的可见性。运行时状态由下游模块拥有：`models` 定义任务、暂停、checkpoint 与路径数据；`client` 维护元数据客户端及 watcher；`regioniter` 表示 region/store 拓扑；`collector` 汇聚 store/region checkpoint；`flush_subscriber` 管理 flush 事件订阅；`CheckpointAdvancer` 组合配置、环境、当前 checkpoint、缓存和推进状态。

从门面角度最重要的不变量是公共路径稳定性：`Store`、`StreamMeta`、`NewMetaDataClient`、`CheckpointAdvancer` 等已被外部 crate 从根路径引用，移动真实定义时必须保留相应再导出，或同步迁移所有消费者。glob 再导出意味着子模块的 `pub` 可见性也是根 API 设计的一部分，不只是模块内部细节。

## 依赖与调用关系

本 crate 的 manifest 直接依赖两个仓库内 crate：`astersql-br-pkg-streamhelper-config` 和 `astersql-br-pkg-streamhelper-spans`；还依赖 `regex`、`serde`、`serde_json`、`uuid`。具体模块另外使用标准库的锁、原子量、线程、通道和时间类型，但门面本身不调用这些依赖。

已由 Cargo manifest 核实的 Rust 上游包括：

- [`br/pkg/stream/Cargo.toml`](../stream/Cargo.toml) 及 [`br/pkg/stream/crr/internal/checkpoint/Cargo.toml`](../stream/crr/internal/checkpoint/Cargo.toml)；当前直接生产接线证据是 checkpoint crate 消费并再导出 `Store`。
- [`br/pkg/utiltest/fakecluster/Cargo.toml`](../utiltest/fakecluster/Cargo.toml) 与 [`br/pkg/utiltest/crr/Cargo.toml`](../utiltest/crr/Cargo.toml)；它们消费 region/store、事件、服务 trait 和任务模型来构造内存测试集群与 CRR harness。

本文件的下游是所声明的 11 个模块。模块间主关系为：`advancer` 通过 `Env`/`StreamMeta` 等 trait 使用环境和元数据能力，通过 `collector`、`flush_subscriber`、`regioniter` 与 `stubs` 获取拓扑和检查点；`advancer_cliext` 扩展元数据客户端；`advancer_daemon` 把推进器适配为生命周期回调。RustCodeGraph 的宽泛 `explore` 对常见名称产生大量跨仓库同名结果，因此只采用精确 `query` 定位到 `advancer.rs::NewCheckpointAdvancer`、`client.rs::MetaDataClient`，并以 consumer manifest 和明确源码导入补足 crate 根边；没有把缺失的图边解释为不存在调用。

## 错误处理与边界

本文件没有可失败的运行时操作，不创建、捕获或转换错误。调用者观察到的错误行为由被再导出的实现决定，门面不会包装它们。例如元数据/扫描/订阅接口多以 `Result<_, String>` 或模块定义的结果类型传播失败，advancer 再按具体路径决定重试、暂停、降级轮询或记录错误。

公共面存在三个边界：第一，`cfg(test)` 模块在普通依赖构建中不可见，`export_test.rs` 的辅助函数不是生产 API；第二，具名再导出的 `advancer`/`advancer_env` 新公开符号不会自动出现在根路径，必须显式加入列表；第三，glob 再导出会自动暴露新 `pub` 项并可能重名，扩展时需要编译期冲突检查与公共 API 审查。

[`stubs.rs`](stubs.rs) 虽名为 stubs，却被生产门面公开挂载并提供当前 Rust 迁移所需的协议、存储和客户端替身类型。文档只能把它描述为当前参与编译的兼容/精简层，不能据名称断言其只用于测试，也不能把其中轻量实现当作真实 etcd、PD 或 TiKV 的完整故障语义。

## 并发与资源生命周期

本门面不启动线程、异步任务、定时器、通道、锁、事务或网络连接，也没有 Drop/关闭顺序。模块加载与再导出发生在编译期。

经门面可达的实现则包含并发和资源生命周期：元数据 watch、flush 订阅、collector 与 advancer 会使用线程/通道和共享状态；`Env`、`StreamMeta`、`RegionLockResolver`、`LogBackupService` 等 trait 把外部资源所有权与关闭责任留给具体实现；advancer 的周期运行还涉及 owner 生命周期、GC safepoint 和任务暂停。调用者必须遵守各实现文件的停止、取消、缓存清理和锁约束，不能因为根级构造器易于取得就假设资源会由 `lib.rs` 自动启动或回收。

测试模块通过 `cfg(test)` 与生产生命周期隔离。新增涉及全局状态、后台线程或通道的测试应继续放在独立测试文件中，使用现有测试辅助并显式恢复全局值、结束发送端或 join 后台任务，避免并行测试互相污染。

## 与 Go 版本的对应关系

Go `br/pkg/streamhelper` 没有与 `lib.rs` 一一对应的入口文件；同目录 16 个 `package streamhelper` 的 `.go` 文件天然共享包级命名空间。Rust 需要用模块声明与 `pub use` 人工重建该可见性，因此本文件主要对应 Go 的“包边界”，而各 Rust 子模块分别对应同名 Go 文件，例如 `advancer.rs`/`advancer.go`、`client.rs`/`client.go`、`collector.rs`/`collector.go`、`models.rs`/`models.go` 和 `regioniter.rs`/`regioniter.go`。

Rust 的 [`parity_test.rs`](parity_test.rs) 用内存假集群串联根级公开模型、扫描、元数据、暂停、flush/resolve-lock 辅助及一次 checkpoint 推进，验证的是包级公开契约；各 `*_test.rs` 则对照相邻 Go 测试覆盖模块行为。Go [`export_test.go`](export_test.go) 暴露测试专用方法，Rust [`export_test.rs`](export_test.rs) 以 trait 和辅助函数实现近似测试入口，但其中外部 checkpoint storage factory 明确是无真实存储的占位，不能声称完整等价。

当前接线也不完全对称：Go BR 的日志备份命令已直接使用 `streamhelper` 的完整生产能力；已检索到的 Rust 外部生产引用主要是 checkpoint 模块使用 `Store`，而较完整的 trait/模型消费来自 utiltest。本文因此把 crate 描述为已编译、已被部分消费的迁移门面，不把 Go 的全部应用主链写成 Rust 已接通事实。

## 扩展指南

- 新增生产实现文件时，在本文件添加明确的 `#[path] pub mod`；若需要 Go 包级兼容路径，再选择具名或 glob 再导出。具名方式应同步维护列表，glob 方式必须检查重名和意外 API 扩张。
- 修改或移动现有公开符号前，检查所有 consumer manifests 和 `astersql_br_pkg_streamhelper::{...}`/模块路径导入；至少关注 stream checkpoint、utiltest/fakecluster 和 utiltest/crr。保留兼容再导出通常比直接删除根路径安全。
- 新增测试必须使用独立 `*_test.rs` 文件，并在此处以 `#[cfg(test)] #[path = ...] mod ...` 挂载；同时对照最近的 Go 源与 Go 测试，不要为了通过 Rust 测试删减真实逻辑。
- 扩展 `CheckpointAdvancer` 主链时，应在真实归属模块同步环境 trait、配置、元数据、collector/subscriber 与错误策略；`lib.rs` 只负责暴露最终公共入口，不应承载业务实现。
- 引入真实 PD/TiKV/etcd 或存储后端时，不要继续扩大 `stubs` 来伪装生产完备性；应在正确的上游 crate 实现并通过 manifest 接线，补充取消、重连、幂等、资源关闭和并发测试。
- 兼容风险集中在公共路径和 Go/Rust 语义漂移；性能风险不在再导出本身，而在新 API 是否进入 advancer tick、region 扫描或订阅热路径。新增阻塞 I/O、长锁或无界通道前应在具体实现与独立测试中明确约束。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter br/pkg/streamhelper` 定位 77 个 Go/Rust 文件；`node --file br/pkg/streamhelper/lib.rs` 核对 crate 根结构；精确 `query NewCheckpointAdvancer`、`query MetaDataClient` 定位真实实现。宽泛 `explore` 的同名噪声未作为调用证据，精确 callers/callees 未提供可用 crate 根边。
- crate 边界与上游：[`Cargo.toml`](Cargo.toml)、[`br/pkg/stream/Cargo.toml`](../stream/Cargo.toml)、[`br/pkg/stream/crr/internal/checkpoint/Cargo.toml`](../stream/crr/internal/checkpoint/Cargo.toml)、[`br/pkg/utiltest/fakecluster/Cargo.toml`](../utiltest/fakecluster/Cargo.toml)、[`br/pkg/utiltest/crr/Cargo.toml`](../utiltest/crr/Cargo.toml)。
- Rust 生产源码：[`lib.rs`](lib.rs) 的 11 个生产模块、15 个测试模块及再导出；并核对 [`advancer.rs`](advancer.rs)、[`client.rs`](client.rs)、[`br/pkg/stream/crr/internal/checkpoint/lib.rs`](../stream/crr/internal/checkpoint/lib.rs)、[`br/pkg/utiltest/fakecluster/core.rs`](../utiltest/fakecluster/core.rs) 和 [`br/pkg/utiltest/crr/pd_sim_service.rs`](../utiltest/crr/pd_sim_service.rs) 的直接接线。
- Rust 独立测试：[`parity_test.rs`](parity_test.rs)、[`export_test.rs`](export_test.rs)，以及本文件挂载的各模块 `*_test.rs`；这些文件证明测试与生产文件分离，并展示根级和模块级 API 的实际消费。
- Go 对照：同目录 `advancer.go`、`client.go`、`collector.go`、`models.go`、`regioniter.go` 等 16 个 `package streamhelper` 文件，及 [`export_test.go`](export_test.go) 和相邻 Go 测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付使用任务指定的结构命令确认目标存在且恰有 11 个固定二级标题，并人工复核链接、模块计数、Cargo 消费者、Go/Rust 差异、迁移边界和扩展风险。
