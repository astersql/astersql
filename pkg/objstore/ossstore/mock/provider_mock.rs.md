# `pkg/objstore/ossstore/mock/provider_mock.rs`

## 文件定位

本文件属于独立 crate `astersql-objstore-ossstore-mock`，为阿里云 OSS 凭证提供者建立可配置期望的 Rust 测试替身。crate 入口 `pkg/objstore/ossstore/mock/lib.rs` 将 `provider_mock` 声明为公开模块并再导出其全部公开项；根工作区 `Cargo.toml` 把该 crate 纳入 members，并以 `facade_objstore_ossstore_mock` 注册，随后由 `pkg/lib.rs` 的 `objstore::ossstore::mock` 门面再次导出。

它当前是测试兼容层，而不是 OSS 生产凭证链的实现。生产代码使用 `pkg/objstore/ossstore/credential.rs::CredentialsProvider`（小写 `get_credentials`，返回 `ProviderCredentials`）；本文件另行定义 Go 风格的 `CredentialsProvider`（大写 `GetCredentials`/`GetProviderName`）。仓库中的精确符号检索只发现同一 mock crate 的测试直接构造该 mock，没有发现生产 OSS 路径把它注入 `CredentialRefresher`。

## 核心职责

- 用 `Credentials` 表示 mock 调用返回的 Access Key ID、Access Key Secret 与可选 STS token，三个字段均为拥有所有权的 `String`。
- 用 `CredentialsError` 提供最小、可比较且实现标准错误接口的失败值，便于期望闭包返回错误。
- 用 `CredentialsProvider: Send + Sync` 固定被模拟的方法集：取凭证以及报告提供者名称。
- 通过 `mockall::mock!` 生成 `MockCredentialsProvider` 及 `expect_GetCredentials`、`expect_GetProviderName` 等期望配置接口。
- 以 `NewMockCredentialsProvider`、`EXPECT`、`ISGOMOCK` 和 `MockCredentialsProviderMockRecorder` 保留 GoMock 可辨识的公开命名与调用形状；这些名称依赖 crate 根的 lint 放宽配置。

## 主要符号

- `Credentials`（第 26 行）：公开值类型，派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`。`security_token` 为空字符串表示没有临时令牌；类型本身不验证字段是否非空。
- `CredentialsError(String)`（第 37 行）：元组字段私有，只能由本模块或公开构造器 `CredentialsError::new(message)` 创建。它派生可克隆、可比较能力，`Display` 原样输出内部消息，并实现 `std::error::Error`。
- `CredentialsProvider`（第 55 行）：公开且要求实现者同时满足 `Send + Sync`。`GetCredentials(&self)` 返回 `Result<Credentials, CredentialsError>`，`GetProviderName(&self)` 返回拥有所有权的 `String`。
- `mockall::mock!`（第 63 行）：根据上述 trait 方法签名生成公开 `MockCredentialsProvider`。期望配置、调用计数检查和返回闭包执行由 `mockall` 生成代码承担，本文件不手写调度逻辑。
- `MockCredentialsProviderMockRecorder`（第 73 行）：不是独立 recorder，而是 `MockCredentialsProvider` 的公开类型别名。
- `NewMockCredentialsProvider()`（第 76 行）：零参数调用 `MockCredentialsProvider::new()`，无需 GoMock controller。
- `MockCredentialsProvider::EXPECT(&mut self)`（第 82 行）：直接返回同一个 mock 的可变引用，让调用方继续调用 mockall 生成的 `expect_*` 方法。
- `MockCredentialsProvider::ISGOMOCK(&self)`（第 87 行）：无返回值、无副作用的兼容标记方法。

## 执行流程

典型测试流程由 `pkg/objstore/ossstore/mock/migration_aster_unit_test.rs::credentials_provider_mock_matches_go_zero_argument_methods` 展示：

1. 调用 `NewMockCredentialsProvider()` 创建没有显式期望配置的 mock。
2. 通过 `mock.EXPECT()` 取得同一对象的可变引用；别名 `MockCredentialsProviderMockRecorder` 只维持 GoMock 风格的类型名。
3. 调用生成的 `expect_GetCredentials()`，设置 `.times(1)`，并用 `.return_once(...)` 返回一份 `Credentials`。
4. 调用生成的 `expect_GetProviderName()`，设置 `.times(1)`，并用 `.return_const(...)` 返回提供者名。
5. 经 trait 方法 `GetProviderName` 和 `GetCredentials` 触发 mockall 的匹配、计数与返回逻辑；测试断言名称和 Access Key ID。
6. 可调用 `ISGOMOCK()` 证明兼容方法存在；该步骤不改变状态。

另一个独立测试 `provider_mock_test.rs::expect_returns_the_named_go_mock_recorder_type` 只验证 `EXPECT()` 的返回值可以同时视为 `MockCredentialsProviderMockRecorder` 和 `MockCredentialsProvider`，从而固定类型别名契约。

## 数据与状态

业务数据只存在于按值返回的 `Credentials` 及 `CredentialsError` 字符串中，本文件没有全局变量、缓存或持久化状态。`Credentials::default()` 会得到三个空字符串，但这只是派生默认值，不代表云端可用凭证。

mock 的期望集合、剩余调用次数和返回行为属于 `mockall` 生成的 `MockCredentialsProvider` 内部状态。调用方必须持有 `&mut MockCredentialsProvider` 才能经过 `EXPECT()` 配置该状态；实际 trait 调用只需要 `&self`。`return_once` 会消费一次性返回闭包，`return_const` 则提供可重复克隆的常量返回值，当前迁移测试又用 `.times(1)` 将两者都约束为一次调用。

## 依赖与调用关系

- 上游导出链：`mock/lib.rs` 的 `pub mod provider_mock` 与 `pub use provider_mock::*` → 根 `pkg/lib.rs::objstore::ossstore::mock` facade。
- 直接构造者：`provider_mock_test.rs::expect_returns_the_named_go_mock_recorder_type` 和 `migration_aster_unit_test.rs::credentials_provider_mock_matches_go_zero_argument_methods` 调用 `NewMockCredentialsProvider`。
- 下游依赖：标准库 `std::fmt` 支撑错误显示；`mockall::mock!` 是 `mock/Cargo.toml` 声明的唯一外部依赖。
- crate 边界：`mock/Cargo.toml` 的 `[lib] path = "lib.rs"` 将本文件作为库模块编译，`package.metadata.porting.go-package` 指向 `pkg/objstore/ossstore/mock`。
- RustCodeGraph 将本文件识别为 13 个符号，并显示若干 crate 级“used by”文件；但对 `NewMockCredentialsProvider` 和两个 trait 方法执行 callers/callees 查询没有返回函数级边，原因是核心实现由宏展开生成。因此本文仅把精确符号检索确认的两份同 crate 测试记为直接调用者，不把 crate 依赖误写为函数调用。
- 与生产链的关系仅是语义对照：`credential.rs::CredentialRefresher` 接受其自身的 `Arc<dyn CredentialsProvider>`，其 trait 类型和方法签名均不同，本 mock 不能直接作为该参数使用。

## 错误处理与边界

`CredentialsError::new` 接受任何可转为 `String` 的消息；`Display` 不包装、不脱敏地输出该消息，`Error` 实现也没有自定义 source。调用方不应把 Access Key、Secret 或 token 写入错误文本。`GetCredentials` 的错误通过 `Result` 原样交给测试调用方，本文件没有重试、日志或错误转换。

本文件不检查空 Access Key、空 Secret、空 token 或提供者名，也没有到期时间字段；这些是测试替身的刻意边界，不应被解释为生产凭证校验策略。期望是否匹配、未配置方法被调用时的行为、调用次数校验时机均由 `mockall 0.13` 负责；仓库测试只验证了成功返回和一次调用，未直接覆盖错误返回、未匹配调用或多线程调用。

## 并发与资源生命周期

trait 的 `Send + Sync` 上界要求实现对象可跨线程传递和共享，但本文件不创建线程、锁、channel、异步任务或运行时。mock 的生命周期完全由测试作用域管理：构造后配置期望、执行调用，离开作用域时由生成类型负责清理并校验其约束。

`EXPECT` 需要独占可变借用，因此期望通常应在共享 mock 之前配置完成。与 Go 的 `credentialRefresher` 或 Rust 生产 `CredentialRefresher` 不同，本文件不负责周期刷新、原子快照发布或关闭后台 worker；相关并发生命周期分别位于 `pkg/objstore/ossstore/credential.go` 和 `pkg/objstore/ossstore/credential.rs`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/objstore/ossstore/mock/provider_mock.go`，它由 Go MockGen 针对阿里云 SDK 的 `providers.CredentialsProvider` 生成。

- 类型名称与方法名称基本对齐：两边都有 `MockCredentialsProvider`、`MockCredentialsProviderMockRecorder`、`NewMockCredentialsProvider`、`EXPECT`、`ISGOMOCK`、`GetCredentials` 和 `GetProviderName`。
- Go 构造器接收 `*gomock.Controller`，mock 和 recorder 是两个结构体；Rust 构造器零参数，recorder 名称是 mock 本身的别名，`EXPECT` 返回自身可变引用。
- Go `GetCredentials` 返回 `*providers.Credentials, error`，凭证类型来自阿里云 SDK且允许 nil 指针；Rust 返回本地 `Credentials` 值与本地 `CredentialsError`，没有 nil 凭证状态。
- GoMock 通过 controller 的 `Call`/`RecordCallWithMethodType` 记录调用；Rust 将匹配与计数委托给 mockall 生成的 `expect_*` API。
- Go 的 `ISGOMOCK` 返回空结构体，Rust 方法返回单元值 `()`；两者都只作为生成 mock 的识别标记。
- Go 集成测试 `pkg/objstore/ossstore/credential_test.go::TestCredentialRefresher` 把该 mock 注入 Go `credentialRefresher`，并验证初次刷新、周期刷新和关闭。Rust 的对应 mock 目前仅由 mock crate 自测使用；Rust 生产刷新器测试采用另外实现的 `CountingProvider`，所以不能宣称该 mock 已覆盖 Rust 刷新主链。

## 扩展指南

- 若阿里云 Go 接口新增方法，应同时更新 `CredentialsProvider` trait 与 `mockall::mock!` 内的 impl 声明，并在独立的 `provider_mock_test.rs` 或 `migration_aster_unit_test.rs` 中增加期望配置和实际调用；不要把测试内嵌进本生产文件。
- 若凭证模型增加字段，先核对 Go SDK 字段及实际消费路径，再更新 `Credentials`、成功/错误测试和文档；新增敏感字段时特别检查 `Debug` 派生与错误消息是否会泄露秘密。
- 若目标是让本 mock 注入 Rust 生产 `CredentialRefresher`，不能只改类型别名：需要先统一或适配本文件与 `credential.rs` 的两个不同 trait、返回类型和命名约定，并为直接接线增加独立回归测试。当前任务没有进行这种行为修改。
- 保留 Go 风格公开名称时，应继续依赖 `mock/lib.rs` 的 lint 允许项；若改成 Rust 命名，应评估 facade 使用者和迁移测试的兼容性。
- 并发测试若共享 mock，应先确认 mockall 生成类型在所用期望/返回闭包组合下满足 trait 的 `Send + Sync` 要求，并覆盖跨线程调用次数及析构校验；目前仓库没有这类证据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标文件已索引；`files --filter pkg/objstore/ossstore/mock` 列出本 crate 的 Rust/Go 源与测试；`node --file provider_mock.rs` 读取完整 88 行并报告 13 个符号；精确 `query` 确认 trait、错误、构造器和 Go/Rust 同名符号；对构造器及两个 trait 方法执行 `callers`/`callees` 未得到宏内函数边。
- 源码：`pkg/objstore/ossstore/mock/provider_mock.rs`（本文对象）；模块入口 `pkg/objstore/ossstore/mock/lib.rs`；生产边界 `pkg/objstore/ossstore/credential.rs`；根门面 `pkg/lib.rs`。
- Cargo：`pkg/objstore/ossstore/mock/Cargo.toml`；根 `Cargo.toml` 的 workspace member 与 `facade_objstore_ossstore_mock` 路径依赖。
- Go 对照：`pkg/objstore/ossstore/mock/provider_mock.go`；实际 Go 使用与生命周期测试 `pkg/objstore/ossstore/credential_test.go`；刷新器实现 `pkg/objstore/ossstore/credential.go`。
- Rust 测试：`pkg/objstore/ossstore/mock/provider_mock_test.rs`；`pkg/objstore/ossstore/mock/migration_aster_unit_test.rs::credentials_provider_mock_matches_go_zero_argument_methods`。这些测试证明命名 recorder 契约、成功凭证返回、提供者名称返回、一次调用约束和 `ISGOMOCK` 可调用；错误分支与并发行为未验证。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰好包含上述 11 个固定二级标题。
