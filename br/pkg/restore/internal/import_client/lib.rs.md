# `br/pkg/restore/internal/import_client/lib.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-restore-internal-import-client` 的 crate 根。`br/pkg/restore/internal/import_client/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向它，根 `Cargo.toml` 又把该目录列为 workspace member。文件本身不是 ImportSST 业务实现，而是一个很薄的门面：用 `#[path = "import_client.rs"]` 挂载实现模块，在测试构建中挂载两个独立测试模块，并用 `pub use import_client::*` 把实现模块的公开符号提升到 crate 根。

当前接线要如实区分：仓库内没有其他 Cargo manifest 依赖这个包；`br/pkg/restore/snap_client` 等 Rust 迁移代码仍从各自 `stubs` 模块取得 `ImporterClient`。因此，这个 crate 已能独立表达并验证 Go 包契约，但尚不能据此声称完整 Rust restore 主链正在使用它。

## 核心职责

1. 以明确的 `#[path]` 将 `import_client.rs` 固定为公开实现模块，保持与同目录 Go 包布局一一对应。
2. 以 `pub use import_client::*` 建立扁平公开 API，使 crate 使用者可以直接访问 `ImporterClient`、`ImportClient`、`NewImportClient`、`NewImportClientWithDialer`、`Context`、`Error`、`import_sstpb` 等公开符号，而不需要再写一层 `import_client::`。
3. 只在 `cfg(test)` 下编译 `parity_test.rs` 与 `import_client_test.rs`。这既保持生产库不携带测试实现，也满足“Rust 源文件与测试逻辑分离”的仓库约束。
4. 在 crate 根统一放宽迁移代码暂时触发的命名、未使用项和 Clippy 警告。该属性覆盖整个 crate，作用是容纳 Go/protobuf 风格名称，不代表这些警告对其他 crate 普遍可忽略。

## 主要符号

- `pub mod import_client`：唯一的生产模块声明，源码由 `import_client.rs` 提供。它是公开模块，因此调用者既可经 crate 根的再导出访问符号，也可显式走模块路径。
- `mod parity_test`：仅测试配置可见的私有模块，验证 Rust 与 Go 的公开契约，包括地址选择、双连接池、能力探测、强制分区 RPC 和关闭后重拨。
- `mod import_client_test`：仅测试配置可见的私有模块，是 `import_client_test.go` 的 Rust 对照，使用可注入 dialer 而非真实 gRPC 网络验证调用编排。
- `pub use import_client::*`：glob 再导出。它不会复制状态或执行代码，但决定了 crate 的实际公共命名空间；`import_client.rs` 中任何新增 `pub` 项会自动成为 crate 根 API。
- crate 级 `#![allow(...)]`：覆盖 `dead_code`、Go 风格大小写、未使用项及 `clippy::all`。它是编译策略，不是运行时开关，也没有 Cargo feature 与之对应。

本文件没有常量、结构体、trait、函数或 `impl`；所有业务符号均来自再导出的实现模块。

## 执行流程

编译生产库时，Rust 从 `lib.rs` 进入，应用 crate 级 lint 设置，解析 `import_client.rs`，然后把其中的公开项再导出到 crate 根；两个带 `cfg(test)` 的模块不会进入生产构建。编译测试时，流程额外解析 `parity_test.rs` 和 `import_client_test.rs`，二者通过 `use crate::{...}` 使用门面再导出的 API，所以同时验证了 `pub use` 是否完整可用。

真正调用 `NewImportClient` 后的运行链位于 `import_client.rs`：构造 `ImportClient`，按 store ID 查询元数据和选择 `PeerAddress`/`Address`，经 dialer 建立连接，再由 `ImporterClient` 方法转发 Download、Apply、MultiIngest 等 RPC。普通请求与 MultiIngest 使用两个连接缓存；能力探测根据 `Unimplemented` 与其他错误采取不同分支。上述行为是门面暴露的实现，不是 `lib.rs` 自身执行的逻辑。

## 数据与状态

`lib.rs` 自身不持有数据、全局变量、连接或缓存。其唯一“状态形状”是编译期模块图和公开命名空间。

经门面暴露的实际状态集中在 `import_client.rs::ImportClient`：`meta_client` 提供 store 元数据，`Mutex<ConnCaches>` 内分别保存普通连接 `conns` 与 ingest 连接 `ingest_conns`，另有 TLS、keepalive 和可注入 dialer 配置。连接缓存以 `storeID` 为键；地址变化不会自动刷新已有连接，必须先关闭并在后续请求中重建。`Context` 以 `Arc<Mutex<Option<Error>>>` 表达取消状态。这里列出这些状态是为了说明门面公开 API 的语义边界，修改它们应发生在实现文件而非本文件。

## 依赖与调用关系

- 上游构建入口：根 `Cargo.toml` workspace member → 本目录 `Cargo.toml` 的 `[lib]` → `lib.rs`。
- 直接下游：`lib.rs` → `import_client.rs`；测试配置下还指向 `parity_test.rs` 和 `import_client_test.rs`。
- 测试调用：两个测试模块均从 `crate` 根导入 `NewImportClientWithDialer`、`ImporterClient`、请求类型及错误类型，证明扁平再导出是测试入口。
- 实现下游：`NewImportClientWithDialer` 实例化 `ImportClient`；各 RPC 方法经 `GetImportClient` 或私有 `GetIngestClient` 进入 `cachedConnectionFrom`，未命中缓存时调用 `createGrpcConn`，最终委派给本地 `ClientConn`/`ImportSSTClient` trait。
- 应用主链限制：代码搜索未发现其他 Cargo 包依赖 `astersql-br-pkg-restore-internal-import-client`。`snap_client/client.rs`、`snap_client/import.rs` 等当前引用的是 `crate::stubs::ImporterClient`，不是本 crate 的再导出类型；两者不能视为同一类型或已完成接线。

## 错误处理与边界

门面不创建、转换或吞掉错误，编译期主要边界是模块文件存在、路径正确以及再导出无名称冲突。由于使用 glob 再导出，实现模块新增同名公开项时可能扩大或冲突 crate API，需要在修改后检查公共符号集合。

业务错误边界由实现模块负责：拨号或元数据错误通过 `Error::Trace` 传播，能力探测把 gRPC `Unimplemented` 与其他错误分开处理，其他失败使用 `Annotatef` 附加 store ID；锁中毒使用 `expect`，会 panic 而非返回 `Result`。默认 dialer 在当前 local-trait 模式下明确返回“不可拨号”错误，真实测试行为依赖 `NewImportClientWithDialer` 注入 mock。`Cargo.toml` 没有外部依赖且注释明确不使用 kvproto/grpcio，因此不可把本地 trait 当成真实网络客户端实现。

## 并发与资源生命周期

`lib.rs` 没有线程、异步任务、锁、通道或资源清理逻辑；`cfg(test)` 仅决定测试模块是否参与编译。

再导出的 `ImportClient` 使用 `Mutex<ConnCaches>` 串行化连接缓存访问，并以 `Arc` 共享元数据客户端、dialer 和 RPC client。`cachedConnectionFrom` 持锁完成查找、拨号和插入，因此同一时刻不会为同一缓存并发插入重复连接，但慢拨号也会延长其他缓存操作等待时间。`CloseGrpcClient` 同样持锁，先调用 `Close`，仅在成功后删除对应缓存项；若中途失败，当前及后续尚未处理的连接仍保留。普通池与 ingest 池分别关闭，关闭后下一次 RPC 会重新拨号。这些生命周期由 `parity_test.rs` 的关闭计数和重拨断言覆盖。

## 与 Go 版本的对应关系

Go 对照文件是 `br/pkg/restore/internal/import_client/import_client.go`，其 package 声明天然形成包级公开命名空间；Rust 没有等价机制，因此 `lib.rs` 用“模块挂载 + glob 再导出”模拟调用者可直接访问包 API 的形状。`Cargo.toml` 的 `package.metadata.porting.go-package` 也明确指向该 Go 包。

主要语义对应由 `import_client.rs` 完成：三秒拨号退避上限、PeerAddress 优先、普通/ingest 双连接池、成功关闭后删除、RPC 透传和能力探测均对照 Go 实现。Rust 为无 kvproto/grpcio 的本地迁移环境定义了本地 trait、请求/响应占位类型与可注入 dialer，因此验证的是编排与契约，不是真实 gRPC 字节交互。另一个可见差异是当前 Go `ImporterClient` 还公开 `IsBatchDownloadLatestMVCCSupported`，而 Rust trait/实现中未出现该方法；文档不得把它写成 Rust 已支持能力。

Go 测试 `import_client_test.go::TestImportClient` 使用真实 TCP gRPC mock；Rust `import_client_test.rs::test_import_client` 用注入式 mock 保留字段回显、故障预算和关闭行为。Rust 额外的 `dial_options_match_go`、`test_latest_mvcc_boolean_probe_and_strict_probe` 与 `parity_test.rs::go_rust_public_contract_matches` 扩展覆盖拨号参数、探测分支、缓存复用及资源释放。

## 扩展指南

- 新增 ImportSST RPC 时，主要修改点是 `import_client.rs` 中的 `import_sstpb::ImportSSTClient`、公开 `ImporterClient` trait 及 `impl ImporterClient for ImportClient`；同步扩展 `import_client_test.rs` 与 `parity_test.rs`，并先核对 Go 接口和实现。通常无需修改 `lib.rs`，因为 glob 会自动再导出新的公开项。
- 若新增独立生产模块，应在 `lib.rs` 显式声明其可见性，并决定是否精确再导出；不要仅依赖 glob 形成难以审计的 API。新增测试仍放在独立 `*_test.rs`/`parity_test.rs` 文件，用 `cfg(test)` 挂载，不能把测试逻辑写进生产源文件。
- 若把本 crate 接入 restore 主链，必须先处理它与 `snap_client::stubs::ImporterClient` 的类型边界，并在相应 Cargo manifest 增加显式依赖；仅在源码中出现同名 trait 不构成接线。
- 引入真实网络层会改变当前 Darwin/local-trait 边界、依赖规模、TLS 与错误类型，应作为单独移植任务验证，不能用测试 dialer 代替生产实现。
- 调整 crate 级 `allow` 前先评估整个实现模块的 Go/protobuf 命名；缩小 lint 豁免宜落到具体模块或符号，避免一次性产生大范围无关告警。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/restore/internal/import_client` 找到 `lib.rs`、实现文件、两个 Rust 测试及 Go 对照/测试。
- RustCodeGraph `node --file br/pkg/restore/internal/import_client/lib.rs`：核对全部 31 行，确认唯一生产模块声明、两个 `cfg(test)` 测试模块和 `pub use import_client::*`。
- RustCodeGraph `node --file br/pkg/restore/internal/import_client/import_client.rs`：核对 `ImporterClient`、`ImportClient`、`NewImportClientWithDialer`、`createGrpcConn`、`cachedConnectionFrom`、双连接池、能力探测与关闭流程。
- RustCodeGraph 对 `import_client_test.rs` 和 `parity_test.rs` 的文件节点读取，以及测试符号搜索：确认独立测试为 `dial_options_match_go`、`test_import_client`、`test_latest_mvcc_boolean_probe_and_strict_probe` 和 `go_rust_public_contract_matches`。
- 配置与源码读取：根 `Cargo.toml`、本目录 `Cargo.toml`、`import_client.go`、`import_client_test.go`；另用 `rg` 检查 Cargo 依赖和 restore Rust 调用点，确认当前无外部 Cargo 依赖者且 snap client 仍使用本地 stubs。
- 未运行 Cargo：任务是纯文档分析，计划明确排除 Cargo 验证。交付前仅运行任务指定的 11 章节结构检查，并人工复核链接路径、当前接线限制和 Go/Rust 差异均有上述源码依据。
