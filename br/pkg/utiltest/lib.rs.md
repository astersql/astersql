# `br/pkg/utiltest/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-utiltest` 的 crate 根文件，也是 Go 包 `br/pkg/utiltest` 在 Rust 迁移树中的公开门面。`br/pkg/utiltest/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它声明为库入口，并用 `package.metadata.porting.go-package = "br/pkg/utiltest"` 标出 Go 对照包。文件本身不实现测试夹具逻辑，只装配 `stubs.rs` 与 `suite.rs`，再把常用类型和工厂提升到 crate 根路径。

这是测试基础设施而非 BR 生产请求链的一环。当前 Cargo 清单搜索未发现其他 crate 对 `astersql-br-pkg-utiltest` 的依赖；RustCodeGraph 也只把本 crate 的 `parity_test.rs` 识别为公开工厂的直接调用者。因此，它目前提供的是可复用的 Rust 测试 API，并由自身契约测试验证，而不能据此声称 Go 测试消费者都已经改接 Rust crate。

## 核心职责

该文件承担四项边界职责：

1. 以 `pub mod stubs` 暴露本地对象存储兼容层，其实现位于 `br/pkg/utiltest/stubs.rs`。
2. 以 `pub mod suite` 暴露 restore-schema 测试套件，其实现位于 `br/pkg/utiltest/suite.rs`。
3. 通过两个 `pub use` 列表提供扁平 API，使调用方可以直接从 crate 根取得存储接口、选项、错误类型和套件工厂，而不必依赖内部模块路径。
4. 仅在 `cfg(test)` 下装入独立的 `br/pkg/utiltest/parity_test.rs`，保持生产源与测试源分离。

文件顶部的 crate 级 `allow` 放宽未使用项、Go 风格命名以及 Clippy 检查。这是机械迁移兼容措施；它扩大了 crate 内部的告警豁免范围，不代表这些命名适合新建的非对照 API。

## 主要符号

- `pub mod stubs`：用显式 `#[path = "stubs.rs"]` 声明存储适配模块。
- `pub mod suite`：用显式 `#[path = "suite.rs"]` 声明测试套件模块。
- 从 `stubs` 再导出的 `Context`、`Error`、`Result`：分别是无取消语义的上下文占位、带 `is_not_exist` 分类的错误，以及该适配层的结果别名。
- 从 `stubs` 再导出的 `Storage`、`Reader`、`Writer`：本地对象存储及流式读写的 trait 表面；trait 具体方法在 `stubs.rs` 定义。
- 从 `stubs` 再导出的 `LocalStorage`、`NewLocalStorage`：文件系统实现及返回 `Arc<dyn Storage>` 的工厂。
- 从 `stubs` 再导出的 `WalkOption`、`ReaderOption`、`WriterOption`：目录遍历、范围读取与写入的选项对象。
- 从 `suite` 再导出的 `TestRestoreSchemaSuite`：聚合 `Cluster`、`MockGlue`、`Arc<dyn Storage>` 与临时目录所有权的夹具类型。
- `RestoreSchemaSuite`：`TestRestoreSchemaSuite` 的 Rust 兼容别名，供机械迁移调用方使用。
- `CreateRestoreSchemaSuite`：按 Glue、mock cluster、临时目录、本地存储、cluster start 的顺序构造套件。
- `mod parity_test`：只在测试构建启用的私有测试模块，不进入库的公开 API。

`lib.rs` 没有自有常量、结构体、trait、函数或 `impl`；所有业务语义均来自上述声明和再导出目标。

## 执行流程

编译库时，编译器先处理 crate 级兼容属性，再按显式路径加载 `stubs.rs` 和 `suite.rs`，最后建立 crate 根再导出。普通链接或导入 `astersql_br_pkg_utiltest` 不会自动创建集群、目录或文件，也不会启动后台任务。

典型调用从根路径的 `CreateRestoreSchemaSuite()` 开始。实际实现位于 `suite.rs::CreateRestoreSchemaSuite`：创建默认 `MockGlue`，调用 `NewCluster`，创建 `TempDir`，用 `stubs.rs::NewLocalStorage` 建立本地存储，启动 mock cluster，然后返回 `TestRestoreSchemaSuite`。调用方可通过 `suite.Mock` 使用 mock Domain/Server，通过 `suite.MockGlue` 提供 glue 状态，通过 `suite.Storage` 执行对象存储式 I/O。

若只需要存储边界，调用方可直接使用根再导出的 `NewLocalStorage`。该函数委托 `LocalStorage::new` 创建根目录，并包装为 `Arc<dyn Storage>`；后续读写、遍历、范围读取、重命名和关闭语义都由 `stubs.rs` 实现，而非 `lib.rs`。

测试构建还会加载 `parity_test.rs`。其中 `go_rust_public_contract_matches` 通过 crate 根 API 验证套件构造、存储往返、缺失文件分类、cluster 启停与幂等清理；`local_storage_matches_go_error_walk_and_lifecycle_contracts` 验证删除、遍历、预签名占位、范围读取、流关闭和 storage close 后继续读取。

## 数据与状态

`lib.rs` 自身没有可变状态或单例。它暴露的关键状态由实现模块持有：

- `TestRestoreSchemaSuite` 持有 `Mock`、`MockGlue`、`Storage`、私有 `_temp_dir` 和 `AtomicBool stopped`。`_temp_dir` 保证存储路径至少活到套件析构，`stopped` 保证显式 `Stop` 与析构清理幂等。
- `LocalStorage` 持有根 `PathBuf`；逻辑对象名在访问时拼接到该根路径。
- `Error` 保存可读消息和 `is_not_exist` 分类标记；调用方据此区分缺失对象与其他 I/O 错误。
- `ReaderOption` 的起止偏移控制半开范围读取；`WalkOption` 的 `SubDir`、`ObjPrefix`、`SkipSubDir` 控制遍历范围。

所有这些类型被根模块再导出后仍是原类型，并不会产生复制、包装或附加状态。

## 依赖与调用关系

向下依赖由 `br/pkg/utiltest/Cargo.toml` 限定为 `astersql-br-pkg-gluetidb-mock`、`astersql-br-pkg-mock` 和 `tempfile`。其中 `suite.rs` 使用前两个 crate 构造 mock glue 与 cluster，并用 `tempfile::TempDir` 管理临时目录；`stubs.rs` 使用标准库文件 I/O 和 `tempfile::NamedTempFile` 实现本地存储。Cargo 注释明确说明该裁剪避免 arm64 Darwin 上引入 kv、domain、kvproto、grpcio 和真实 objstore 边。

RustCodeGraph 的文件关系显示 `lib.rs` 被 `tools/tazel/parity_test.rs` 作为文件结构证据引用；符号调用查询显示 `CreateRestoreSchemaSuite` 的 Rust 直接调用者为 `br/pkg/utiltest/parity_test.rs::go_rust_public_contract_matches`，`NewLocalStorage` 的直接调用者为该测试及 `local_storage_matches_go_error_walk_and_lifecycle_contracts`。Cargo 全仓搜索没有找到其他 manifest 对此 crate 的依赖，所以当前 Rust 上游接线应描述为“本 crate 契约测试”，而不是 Go 消费面的完整替代。

Go 上游范围更广：例如 `br/pkg/checkpoint/checkpoint_test.go` 用 `utiltest.CreateRestoreSchemaSuite(t)` 为表后端 checkpoint 测试提供 Domain；`br/pkg/restore/ingestrec/ingest_recorder_test.go` 用其构造 infoschema 场景；`br/pkg/task/restore_test.go` 同时使用 mock cluster、glue 与 storage 验证 DDL 过滤。它们证明 Go 包的设计角色，但不是 Rust crate 已被这些测试链接的证据。

## 错误处理与边界

门面文件不捕获或转换错误。`NewLocalStorage` 返回 `Result<Arc<dyn Storage>>`，保留目录创建等 I/O 失败；`Storage`、`Reader` 和 `Writer` 方法也通过同一 `Result` 传播错误。缺失文件在 `ReadFile`、`Open`、`DeleteFile` 中被标为 `Error.is_not_exist = true`，而负的读取起点、重复关闭 reader/writer 等产生普通错误。

`CreateRestoreSchemaSuite` 的返回类型不是 `Result`。`suite.rs` 对 cluster 创建、临时目录创建、本地存储创建和 cluster 启动失败执行 `panic!`，以对应 Go `require.NoError(t, err)` 的快速终止测试语义。这一行为适合测试夹具，不应直接当成生产错误处理模式复用。

本地 storage 是为测试裁剪的适配层：`Context` 没有取消/超时传播，`PresignFile` 只返回文件名占位，`Close` 是 no-op，并未实现真实云对象存储的认证、远端一致性或预签名 URL 行为。路径拼接也面向受控测试输入；新增不可信路径场景前应单独评估路径穿越约束。

## 并发与资源生命周期

`lib.rs` 不启动线程、异步任务或通道。再导出的 `Storage` 要求 `Send + Sync`，返回值由 `Arc` 共享；`Reader`、`Writer` 要求 `Send`，但各自的文件位置和关闭状态仍由可变借用串行操作。

套件生命周期由 `TestRestoreSchemaSuite` 管理。构造时先建立存储再启动 cluster；显式 `Stop` 通过 `AtomicBool::compare_exchange` 保证只调用一次 `Mock.Stop`；`Drop` 再调用 `Stop`，因此显式停止后离开作用域不会重复清理。原子顺序使用 `SeqCst`，但该设计的主要目标是幂等资源边界，并不承诺可以从多个线程同时以 `&mut self` 调用 `Stop`。

私有 `_temp_dir` 必须与 suite 同寿命，否则 `Storage` 仍持有路径但目录可能已被删除。清理只停止 mock cluster；`Storage::Close` 本身不禁用 I/O，这一点由 `parity_test.rs` 的 stop 后读写和 close 后读取断言锁定。

## 与 Go 版本的对应关系

Go 的直接对照文件是 `br/pkg/utiltest/suite.go`。Go `TestRestoreSchemaSuite` 公开 `Mock`、`MockGlue`、`Storage` 三个字段；Rust 同名结构额外持有 `_temp_dir` 与 `stopped`，用于显式表达 Go `t.TempDir()` 和 `t.Cleanup()` 隐式管理的生命周期。

Go `CreateRestoreSchemaSuite(t *testing.T) *TestRestoreSchemaSuite` 依赖 `testing.T` 创建临时目录、通过 `require.NoError` 报告构造失败，并注册 cleanup。Rust `CreateRestoreSchemaSuite() -> TestRestoreSchemaSuite` 不接收测试句柄：它用 `tempfile::TempDir` 保存目录所有权、用 panic 对齐快速失败、用 `Drop` 模拟 cleanup。Rust 还提供 Go 中没有的 `RestoreSchemaSuite` 类型别名，以便利机械迁移。

Go suite 直接依赖 `pkg/objstore/storeapi.Storage` 与 `objstore.NewLocalStorage`；Rust 因 Cargo 平台裁剪，改用 `stubs.rs` 的局部 trait 和本地实现。因此公开概念和测试边界对应，但具体类型并非跨语言 ABI 等价，云存储能力也不是完整复刻。

`br/pkg/utiltest/BUILD.bazel` 只描述 Go `suite.go` 的 Bazel 库及其依赖；Rust crate 边界以 `Cargo.toml` 为准。Go 消费者仍按 `CreateRestoreSchemaSuite(t)` 使用 testing cleanup，Rust 消费者必须依赖值的作用域或显式调用 `Stop`。

## 扩展指南

若新增公开存储能力，应先在 `stubs.rs` 的 `Storage` trait 和 `LocalStorage` 实现中完成语义，再决定是否在 `lib.rs` 的 `pub use stubs::{...}` 中提升到根路径。同步扩展 `br/pkg/utiltest/parity_test.rs`，覆盖正常、缺失对象、非法参数、关闭后行为以及 Go 对照差异；不要把单元测试内嵌回 `lib.rs`。

若新增套件字段或构造步骤，应修改 `suite.rs::TestRestoreSchemaSuite` 与 `CreateRestoreSchemaSuite`，保持资源创建、启动和逆向清理关系清晰，并对照 `suite.go` 判断是 Go 增量还是 Rust 平台接线。涉及生命周期的字段要明确所有权；新增清理动作应纳入 `Stop`/`Drop` 的幂等测试。

只有需要成为稳定便捷 API 的符号才应加入根再导出。仅供实现内部使用的辅助项应留在 `stubs` 或 `suite` 模块，避免无意扩大兼容面。若开始让其他 Rust crate 使用此包，还必须在消费者 `Cargo.toml` 增加显式依赖并添加消费者侧独立测试；不能仅凭 `lib.rs` 已公开符号就认定完成接线。

兼容风险主要来自 Go/Rust 签名差异、局部 storage trait 与真实 objstore 能力差异，以及 crate 级宽泛 `allow` 掩盖新警告。性能风险主要在本地文件 I/O、目录递归遍历和 `SeqCst` 原子操作；当前测试夹具规模下不是主链瓶颈，但新增大数据或高并发测试时应重新评估。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；目标文件在索引内。
- RustCodeGraph `files --filter br/pkg/utiltest`：确认 crate 根、实现模块、独立 parity 测试以及 crr/fakecluster/syncpoint 子包边界。
- RustCodeGraph `node --file br/pkg/utiltest/lib.rs`：确认目标文件共 33 行，只含兼容属性、两个公开模块、两组再导出和条件测试模块。
- RustCodeGraph `node --file br/pkg/utiltest/suite.rs` 与 `stubs.rs`：核对工厂顺序、套件字段、`Stop`/`Drop`、storage trait、本地实现及错误分类。
- RustCodeGraph `callers CreateRestoreSchemaSuite`：直接 Rust 调用者为 `br/pkg/utiltest/parity_test.rs::go_rust_public_contract_matches`。
- RustCodeGraph `callers NewLocalStorage`：直接 Rust 调用者为 `go_rust_public_contract_matches` 和 `local_storage_matches_go_error_walk_and_lifecycle_contracts`。
- `br/pkg/utiltest/Cargo.toml`：核对 crate 名、`lib.rs` 入口、Go 包映射、依赖与平台裁剪说明；全仓 Cargo 搜索未发现该 crate 的外部消费者。
- `br/pkg/utiltest/suite.go` 与 `br/pkg/utiltest/BUILD.bazel`：核对 Go 结构、工厂顺序、cleanup 语义及 Go 依赖边界。
- `br/pkg/utiltest/parity_test.rs`：核对构造、错误、遍历、范围读取和资源清理的真实断言；测试与生产源分离。
- Go 使用证据包括 `br/pkg/checkpoint/checkpoint_test.go`、`br/pkg/restore/ingestrec/ingest_recorder_test.go`、`br/pkg/task/restore_test.go`；它们只用于说明 Go 包角色，不作为 Rust 已接线证明。
- 按任务约束未运行 Cargo；最终仅执行 Markdown 固定章节结构检查，并人工复核本文没有把裁剪桩描述成完整对象存储实现。
