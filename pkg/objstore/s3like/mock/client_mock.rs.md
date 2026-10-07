# `pkg/objstore/s3like/mock/client_mock.rs`

## 文件定位

本文件属于独立 crate `astersql-objstore-s3like-mock`，crate 入口为同目录的 `lib.rs`，并由其公开为 `client_mock` 模块。`pkg/objstore/s3like/mock/Cargo.toml` 表明该 crate 直接依赖 `anyhow`、`mockall`、`objectio`、`s3like` 和 `storeapi`；父 crate `pkg/objstore/s3like/Cargo.toml` 又把它作为开发依赖提供给 `permission_test.rs`。因此它位于对象存储实现的测试边界，不负责访问 S3，也不保存真实桶、对象或网络连接。

源码头保留了 Go MockGen 的来源信息，但 Rust 实现实际由 `mockall::mock!` 在编译时展开。它为 `s3like::PrefixClient` 提供可编程的测试替身，使 Rust 测试能按 GoMock 风格配置参数匹配、返回值和调用次数。

## 核心职责

1. 通过 `mockall::mock!` 生成公开类型 `MockPrefixClient`，并让它实现 `s3like::PrefixClient`。
2. 为接口中 14 个无默认实现的方法生成 `expect_<方法名>()` 配置入口和对应的调用分派；覆盖四个权限探测方法、对象 CRUD/列举/复制，以及 multipart writer/uploader 的创建。
3. 通过 `NewMockPrefixClient()` 提供与 Go 构造函数同名的便捷入口。
4. 通过 `EXPECT()` 保留 Go 测试常用的调用形状，通过 `ISGOMOCK()` 保留生成 mock 的标记方法。

`PrefixClient::PresignObject` 没有出现在宏声明中，因为 `pkg/objstore/s3like/interface.rs` 已为它提供默认的“不支持”实现；`MockPrefixClient` 继承该默认行为，而不是允许测试为其配置期望。

## 主要符号

- `mockall::mock! { pub PrefixClient {} ... }`：生成 `MockPrefixClient`、构造器 `new()`、各方法的期望配置 API 和调用校验状态。宏块内的 `impl PrefixClient for PrefixClient` 指生成类型实现外部 trait，并非递归定义。
- 权限探测方法：`CheckBucketExistence`、`CheckGetObject`、`CheckListObjects`、`CheckPutAndDeleteObject`，统一接收 `&storeapi::Context` 并返回 `anyhow::Result<()>`。
- 对象操作方法：`CopyObject`、`DeleteObject`、`DeleteObjects`、`PutObject`；参数分别保留 `CopyInput`、对象键、键切片和字节切片的借用语义。
- 查询方法：`GetObject`、`HeadObject`、`IsObjectExists`、`ListObjects`。可缺失的 Go 指针返回被表达为 `Result<Option<T>>`，列举上限使用 `isize` 以保留 Go `int` 的有符号值域。
- multipart 方法：`MultipartWriter` 返回 `Result<Option<Box<dyn objectio::Writer>>>`，`MultipartUploader` 返回 `Option<Box<dyn Uploader>>`；二者把 Go 的可空接口值显式映射为 `Option`。
- `NewMockPrefixClient() -> MockPrefixClient`：零参数调用宏生成的 `MockPrefixClient::new()`。与 Go 版不同，控制器由 `mockall` 内部管理，不由调用者传入。
- `MockPrefixClient::EXPECT(&mut self) -> &mut Self`：仅返回自身，使调用方可继续调用宏生成的 `expect_*` 方法；它不是 Go 版那种独立 recorder 对象。
- `MockPrefixClient::ISGOMOCK(&self)`：无状态、无返回值的兼容标记，运行时不执行校验或 I/O。

## 执行流程

典型测试流程如下：

1. 调用 `NewMockPrefixClient()` 创建一个尚未配置期望的 mock。
2. 以可变借用调用 `EXPECT()`，再调用宏生成的 `expect_<方法名>()`。测试可用 `withf` 匹配参数、`times` 限定调用次数，并以 `return_once` 注入成功值、`None` 或 `anyhow::Error`。
3. 将 `&MockPrefixClient` 作为 `&dyn PrefixClient` 传给被测逻辑，或直接调用 trait 方法。
4. trait 方法由 mockall 的生成代码查找匹配期望，执行配置的返回动作，并更新调用次数；文件本身不解释参数、不访问存储，也不包装业务错误。
5. 测试作用域结束时，mockall 负责检查尚未满足的期望。`pkg/objstore/s3like/permission_test.rs::test_check_permissions` 展示了真实主链：构造 mock，配置四个权限方法之一，再由 `s3like::CheckPermissions` 通过 `PrefixClient` 动态分派调用。

## 数据与状态

本文件没有模块级常量、静态变量或手写字段。可变状态均由 `mockall::mock!` 生成，核心是每个方法的期望集合、参数匹配器、返回动作和调用计数。

输入中的 `Context`、字符串、切片、`CopyInput` 均以借用传入，mock 不取得所有权；返回的 `GetResp`、`HeadObjectResp`、`ListResp`、`Writer` 和 `Uploader` 则由配置闭包构造并转交调用者。`return_once` 适合转移只能消费一次的资源，例如装箱 writer、uploader 或包含读取流的 `GetResp`。`migration_aster_unit_test.rs::prefix_client_mock_preserves_signed_and_nil_success_values` 证明 `None` 和负 `max_keys` 都是刻意保留的有效测试域。

## 依赖与调用关系

- 上游接口：`pkg/objstore/s3like/interface.rs::PrefixClient` 定义被模拟的能力集合，并要求实现者满足 `Send + Sync`。
- 直接生产依赖：`anyhow::Result` 表达方法错误；`storeapi::Context` 贯穿需要上下文的方法；`s3like::{CopyInput, GetResp, HeadObjectResp, ListResp, Uploader}` 和 `objectio::Writer` 构成参数及返回类型。
- 生成框架：`mockall` 生成 `MockPrefixClient` 及 `expect_*` API，承担匹配、返回动作与调用次数验证。
- 已确认调用者：RustCodeGraph 将 `pkg/objstore/s3like/permission_test.rs::test_check_permissions` 连接到 `NewMockPrefixClient`，该测试随后调用 `pkg/objstore/s3like/permission.rs::CheckPermissions`。
- crate 内覆盖：`pkg/objstore/s3like/mock/migration_aster_unit_test.rs` 通过 `lib.rs` 的 `#[cfg(test)]` 模块接线，直接覆盖全部 14 个可配置方法的代表性成功、失败、可空返回和资源对象行为。
- workspace 接线：根 `Cargo.toml` 以 `facade_objstore_s3like_mock` 引用该包；父 `s3like` crate 则以开发依赖名 `s3like_mock` 用于独立权限测试。

该文件没有下游网络、文件系统或异步运行时调用；实际执行效果完全取决于测试配置的闭包和返回对象。

## 错误处理与边界

宏声明中的 fallible 方法直接返回 `anyhow::Result`，不会在本层改写错误。错误上下文属于被测调用方：例如 `permission.rs::CheckPermissions` 为 mock 返回的错误增加 `permission <name>` 上下文，而 `permission_test.rs` 验证错误链仍保留原始错误。

需要特别维护的边界包括：

- `GetObject`、`HeadObject`、`ListObjects` 和 `MultipartWriter` 的 `Ok(None)` 表示成功但没有对象/组件，不能与 `Err` 合并。
- `MultipartUploader` 没有 `Result`，只能返回有或无 uploader；该签名严格跟随 `PrefixClient`。
- `ListObjects::max_keys` 接受负数；mock 层不做合法性校验。
- 未配置、参数不匹配、调用次数不足或超额属于 mockall 的测试失败，而不是本文件构造的业务错误。
- `PresignObject` 无可配置期望入口，调用时走 trait 默认错误。若接口将其改为必需方法，本文件也必须同步补充，否则 trait 实现将无法保持完整。

## 并发与资源生命周期

`PrefixClient: Send + Sync` 是接口级约束，因此生成的 mock 必须能作为该 trait 的实现使用；配置阶段需要 `&mut MockPrefixClient`，执行阶段的方法均接收 `&self`。本文件不创建线程、任务、锁或通道，也不保证测试配置闭包中的自定义状态天然线程安全；并发测试应使用线程安全的捕获值，并避免在调用开始后继续修改期望。

资源生命周期由返回值所有权决定。`Box<dyn objectio::Writer>` 和 `Box<dyn Uploader>` 在返回后归调用者管理；mock 不代替调用者执行 `Close` 或 `Upload`。迁移单测中的 `TestWriter`、`TestUploader` 和 `TestReadCloser` 明确验证了写入、上传与读取资源可穿过 mock 边界。期望状态随 `MockPrefixClient` 存活，并在 mock 被销毁前累计调用次数。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/objstore/s3like/mock/client_mock.go`，它是 MockGen 产物，使用 `gomock.Controller`、`MockPrefixClientMockRecorder` 和 `RecordCallWithMethodType`。Rust 文件保留同名接口方法和近似测试写法，但实现结构存在以下明确差异：

- Go 的 `NewMockPrefixClient(ctrl)` 接收外部 controller；Rust 的 `NewMockPrefixClient()` 直接调用 `MockPrefixClient::new()`。
- Go 的 `EXPECT()` 返回独立 recorder；Rust 的 `EXPECT()` 返回 `&mut Self`，随后使用 `expect_CheckBucketExistence()` 等 mockall 生成方法。
- Go 的可空指针或接口返回映射为 Rust 的 `Option`，同时与错误并存的返回映射为 `Result<Option<T>>`。
- Go `int` 在 `ListObjects` 中对应 Rust `isize`，而 multipart 并发度接口在 Rust trait 中使用 `i32`。
- Go `ISGOMOCK()` 返回空结构体；Rust `ISGOMOCK()` 返回单元值且为空操作。
- GoMock 的控制器显式提供 `Satisfied()`；Rust 测试主要通过 mockall 的 `times` 和生命周期结束时校验期望。

`permission_test.go::TestCheckPermissions` 与 Rust 的 `permission_test.rs::test_check_permissions` 具有相同测试意图：四种权限失败、未知权限拒绝和全部权限成功。Rust 独有的 `mock/migration_aster_unit_test.rs` 进一步覆盖了其余接口方法以及 Rust 所需的 `Option<Box<dyn Trait>>` 边界。

## 扩展指南

当 `PrefixClient` 新增必需方法或更改签名时，应同步修改 `mockall::mock!` 中的 trait 实现；先从 `pkg/objstore/s3like/interface.rs` 复制语义一致的签名，再更新 Go 对照生成文件对应的方法域。不要在 mock 中加入真实参数校验或存储逻辑，边界规则应由被测实现负责，mock 只负责匹配和返回配置结果。

新增或调整 mock 能力时，应把测试逻辑放在独立的 `pkg/objstore/s3like/mock/migration_aster_unit_test.rs`，不要内嵌到生产源文件。至少覆盖：参数匹配、成功值、错误传播、所有可空返回，以及新增资源对象的所有权/关闭行为。若变更用于权限主链，还应同步 `pkg/objstore/s3like/permission_test.rs`，并与 `permission_test.go` 的原始意图核对。

兼容风险主要来自方法名、参数类型或 `Option`/`Result` 形状漂移；测试行为风险来自放宽 `times` 或匹配器导致本应发现的调用错误漏报。性能不是该文件的主要约束，但测试不应在返回闭包中引入不必要的真实 I/O。生成宏代码不可被 RustCodeGraph 完整展开，因此导航时应同时核对宏声明和独立测试中的 `expect_*` 用法。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 7,032 个 Rust 文件；`files --filter pkg/objstore/s3like/mock` 识别 `client_mock.rs`、`lib.rs`、Go 对照和迁移单测。
- RustCodeGraph 源码与调用证据：`node --file pkg/objstore/s3like/mock/client_mock.rs` 显示全部 89 行并标记其被 `permission_test.rs` 使用；`node NewMockPrefixClient` 给出 Rust 构造函数及 `test_check_permissions` 调用边；`node test_check_permissions` 显示其同时调用 `NewMockPrefixClient` 与 `CheckPermissions`。
- 读取的 Rust 定义：`pkg/objstore/s3like/mock/client_mock.rs`、`pkg/objstore/s3like/mock/lib.rs`、`pkg/objstore/s3like/interface.rs`、`pkg/objstore/s3like/permission.rs`。
- 读取的 Cargo 接线：`pkg/objstore/s3like/mock/Cargo.toml`、`pkg/objstore/s3like/Cargo.toml` 和根 `Cargo.toml` 中的 facade 依赖。
- 读取的 Go 对照与测试：`pkg/objstore/s3like/mock/client_mock.go`、`pkg/objstore/s3like/permission_test.go`。
- 读取的独立 Rust 测试：`pkg/objstore/s3like/mock/migration_aster_unit_test.rs`、`pkg/objstore/s3like/permission_test.rs`。前者覆盖接口域与资源/空值边界，后者覆盖真实权限调用链。
- 本任务为纯文档分析，按计划不运行 Cargo；交付仅执行固定章节结构检查和 diff 自审。
