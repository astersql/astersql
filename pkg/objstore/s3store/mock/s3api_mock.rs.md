# `pkg/objstore/s3store/mock/s3api_mock.rs`

## 文件定位

[源文件 `s3api_mock.rs`](./s3api_mock.rs) 是独立 crate `astersql-objstore-s3store-mock` 的主要实现文件，由同目录 `lib.rs` 声明为 `s3api_mock` 子模块并整体重导出。`Cargo.toml` 表明该 crate 只直接依赖 `aws-sdk-s3 = "1"` 和 `mockall = "0.13"`：前者提供真实 S3 操作的输入/输出类型，后者生成可编程的 mock 实现。

它的直接用途是对齐 Go 生成文件 `s3api_mock.go` 的方法集和期望录制体验，并由独立 Rust 测试 `migration_aster_unit_test.rs` 验证。它不是 `pkg/objstore/s3store/interface.rs` 中生产 trait `S3API` 的实现：两者名称相似，但本文件定义的是大写 Go 风格方法的 `S3Api`，而生产 trait 使用 snake_case 方法、`storeapi::Context`、`RequestOptions` 和 `anyhow::Result`。因此，当前 Rust 生产 `Client`/`Store` 请求链不会注入这个 `MockS3API`。

## 核心职责

- 以 `pub use aws_sdk_s3::operation::...` 重导出 14 组 S3 operation 的 Input/Output 类型，使 mock 签名与 AWS SDK for Rust 的数据模型一致。
- 用 `Context`、`S3Options`、`OptionFn` 和 `S3Error` 表达 Go 签名中 `context.Context`、可变参 option 回调和 `error` 的最小 Rust 对应物。
- 用 `S3Api` trait 列出 Go `pkg/objstore/s3store/interface.go::S3API` 的 14 个操作，覆盖桶/对象探测、读写、单个/批量删除、v1/v2 列举、服务端复制、分片上传生命周期与对象锁查询。
- 通过 `mockall::mock!` 生成 `MockS3API` 及其 `expect_<Method>` 期望 API，再用 `NewMockS3API`、`EXPECT`、`ISGOMOCK` 保留 GoMock 命名兼容层。

## 主要符号

- `Context { request_id: String }`：可克隆的请求标识容器。`Context::new` 接受任意 `Into<String>`；`Default` 得到空 request id。它没有取消、deadline 或 value 传递语义。
- `S3Options { force_path_style: bool }`：当前 option 模型唯一的可观测状态，默认为 `false`。
- `OptionFn(Arc<dyn Fn(&mut S3Options) + Send + Sync>)`：用 `OptionFn::new` 包装可在线程间安全共享的回调，`apply` 才会对传入 `S3Options` 执行该回调。手写 `Debug` 固定输出 `OptionFn(..)`，避免要求闭包自身实现 `Debug`。
- `S3Error { message: String }`：可比较、可克隆的测试错误；`S3Error::new` 存储消息，`Display` 原样输出消息，并实现标准 `Error`。
- `S3Api`：14 个同步 `&self` 方法。每个方法接受 `&Context`、对应 SDK input 的共享引用和 `&[OptionFn]`，返回按值持有的 SDK output 或 `S3Error`。
- `MockS3API`：由 `mockall::mock!` 生成，实现 `S3Api`。其实际状态和方法分派由 `mockall` 宏展开提供，源文件未手写存储布局。
- `MockS3APIMockRecorder = MockS3API`：命名兼容别名。GoMock 中 recorder 是独立类型；此处 mockall 的期望方法直接挂在 mock 值上，因而别名不代表第二个对象。
- `NewMockS3API() -> MockS3API`：无控制器参数的 Rust 构造包装，内部调用 `MockS3API::new()`。
- `MockS3API::EXPECT(&mut self) -> &mut MockS3APIMockRecorder`：返回 `self`，使用方可继续调用 mockall 生成的 `expect_GetObject` 等方法。`ISGOMOCK(&self)` 是无状态、无返回值的兼容标记。

## 执行流程

1. 测试通过 `NewMockS3API` 创建一个空 mock。
2. 测试对 `mock.EXPECT()` 返回的同一可变对象调用 `expect_<Method>()`，并以 `withf`、`times`、`return_once` 等 mockall API 定义参数匹配、调用次数与结果。
3. 被测代码或测试直接通过 `S3Api` 大写方法发起调用。`Context`、SDK input 和 option 切片均以共享引用传入；mockall 在调用期间检查已注册期望。
4. 命中期望后，mockall 执行已配置的返回闭包，把 SDK output 或 `S3Error` 作为 `Result` 传回调用方；未命中期望或调用次数不符合时，mockall 按其测试失败语义报告问题。
5. `OptionFn` 不会被 `S3Api` 方法或生成 mock 自动 `apply`。它们在当前测试中主要是可匹配的参数；若返回闭包要验证 option 实际效果，需显式创建 `S3Options` 并调用 `apply`。

`migration_aster_unit_test.rs` 提供三条直接流程证据：`mock_forwards_arguments_options_and_configured_output` 验证 Get 的 context/input/options 匹配和 output 返回；`mock_propagates_errors_and_checks_call_count` 验证 Put 错误值和一次调用限制；`mock_covers_complete_go_method_set` 对其余 12 个方法各注册一次成功期望并逐个调用。

## 数据与状态

手写状态都很小：`Context` 只持有 request id，`S3Options` 只持有 path-style 开关，`S3Error` 只持有消息字符串。`OptionFn` 使用 `Arc` 共享不可变回调对象，克隆只增加强引用计数，不复制闭包捕获状态。

`MockS3API` 内部保存期望、调用计数和返回闭包，但这些结构由 mockall 宏生成，不是本文件稳定的公开数据协议。SDK Input 以引用传入，Output 按值返回；文件自身不缓存对象数据，不管理 bucket/key 集合，也不连接网络。

## 依赖与调用关系

- 模块装配：`mock/lib.rs` 通过 `pub mod s3api_mock` 编译本文件，并以 `pub use s3api_mock::*` 重导出公开符号。上一层 `s3store/lib.rs` 用 `#[path = "mock/lib.rs"] pub mod mock` 将其暴露为 `s3store::mock`。
- 上游 Rust 使用点：全仓搜索显示，本文件特有的 `NewMockS3API`、`S3Api`、`S3Error` 和 `OptionFn` 在排除本文件后只由 `mock/migration_aster_unit_test.rs` 直接使用。RustCodeGraph 的精确 `callers` 查询未产生这些宏生成调用边，因此上述边由源码引用与 `rg` 结果核对。
- 下游依赖：SDK operation 模块提供 28 个重导出 Input/Output 类型；`std::sync::Arc` 支撑 `OptionFn` 共享；`std::fmt` 支撑 `Debug`/`Display`；mockall 根据 `S3Api` 签名生成 mock。
- Go 调用链：`main_test.go` 和 `s3_test.go` 会将 Go 版 `mock.MockS3API` 注入 `NewS3StorageForTest`，用于读取、Head、列举和远程锁等测试。这些是 Go `s3api_mock.go` 的使用边，不是 Rust `MockS3API` 的运行时调用边。
- 生产边界：Rust `s3store/client.rs` 和 `store.rs` 依赖 `interface.rs::S3API`；它们与本文件的 `S3Api` 类型不同，当前没有 adapter 把两者接通。

## 错误处理与边界

所有 14 个 trait 方法都把业务失败表示为 `Result<Output, S3Error>`，不使用 `anyhow`、AWS SDK error 枚举或错误源链。`S3Error` 只保留文本，因而无法表达 HTTP 状态、S3 error code、可重试性或 source error；它适合精确断言 mock 结果，不应被当作生产错误模型。

Input 的必填字段约束由 AWS SDK builder 在构建时处理，本 trait 和 mock 不再验证 bucket/key/upload id。`Context` 没有取消语义；`OptionFn` 也不会自动执行。期望未配置、参数不匹配或调用次数违约属于 mockall 测试框架失败，而不会转换成 `S3Error`。

本文件签名与 Go 的一个重要差异是 Rust 返回具体 Output，而 Go 返回 `*Output`：Rust 边界不能用 `nil, nil` 表示空成功输出。此外，Rust options 是借用切片而非 Go 可变参，期望匹配时应按切片长度和内容处理。

## 并发与资源生命周期

`OptionFn` 的回调约束为 `Send + Sync + 'static`，并被 `Arc` 持有，所以 option 值可以安全克隆并跨线程传递。但 `S3Api` trait 本身没有声明 `Send + Sync` 超约束，方法也是同步函数；不应仅根据 `OptionFn` 的约束推断 `MockS3API` 可被无锁并发调用。

本文件不创建 async task、线程、channel、锁、文件句柄、网络连接或事务。`Context`、Input 和 options 只在方法调用期间被借用；Output 和 Error 的所有权移交给调用者。`OptionFn` 最后一个克隆释放时，其闭包捕获资源随 `Arc` 一起释放。mockall 期望则跟随 `MockS3API` 实例生命周期，测试应在实例存活期间完成期望配置和调用。

## 与 Go 版本的对应关系

Go 的契约源是 `pkg/objstore/s3store/interface.go::S3API`，生成 mock 是同目录 `s3api_mock.go`。Rust `S3Api` 按相同顺序和名称覆盖 14 个操作，没有删减分片上传、批量删除、ListObjects v1 或对象锁查询。Rust `mock_covers_complete_go_method_set` 对除 Get/Put 之外的每个方法都有直接调用，Get/Put 由前两个测试覆盖。

映射关系为：Go `context.Context` → Rust 最小 `Context`；Go `*s3.XInput` → Rust `&XInput`；Go `...func(*s3.Options)` → Rust `&[OptionFn]`；Go `(*s3.XOutput, error)` → Rust `Result<XOutput, S3Error>`；Go `gomock.Controller` + 独立 recorder → mockall 生成的 mock 内部期望系统。`NewMockS3API` 因此不接受 controller，`EXPECT` 返回 `self`的可变引用，`ISGOMOCK` 也不返回 Go 的空 struct。

这是“方法集与测试体验对齐”，不是完整语义等价：Rust `Context` 不支持 Go context 的取消/deadline/value，`S3Options` 也只建模了 `force_path_style`。Go 的 `main_test.go`/`s3_test.go` 展示生成 mock 能注入实际 `S3Storage`；当前 Rust 对应 mock 未实现生产 `interface.rs::S3API`，所以不具备同样的注入位置。

## 扩展指南

- Go `interface.go::S3API` 增加或删除操作时，应同步修改 Rust `S3Api` trait 和 `mockall::mock!` 中的实现声明，并在独立 `migration_aster_unit_test.rs` 中添加该方法的期望与实际调用。不要把测试内嵌到本生产源文件。
- 若要新增 SDK operation，需同时重导出对应 Input/Output，保持 trait 与 mock 签名一致，并验证 builder 必填字段、成功 output 和 `S3Error` 路径。
- 若扩展 option 语义，先向 `S3Options` 添加状态，再用 `OptionFn::apply` 的独立测试证明回调效果。不要默认 mock 方法会自动执行 options。
- 若目标是让 Rust 生产 `Client`/`Store` 测试使用此 mock，需显式解决它与 `interface.rs::S3API` 的 trait、context、options、命名和错误类型差异；这是新的接线设计，不是在本文件内加一个方法即可完成。兼容风险包括同名 trait 混淆、同步/异步边界、output 空值语义与生产错误信息丢失。
- 并发场景需求应先通过 mockall 的具体线程安全契约验证，必要时再为 `S3Api` 增加 `Send + Sync` 超约束；不要由 `OptionFn: Send + Sync` 间接推导整个 mock 的并发安全性。
- 性能方面，每个 `OptionFn` 克隆仅克隆 `Arc`，但 SDK Input/Output 的构建和按值返回仍有成本。该 mock 应留在测试路径，不应成为生产请求的转发层。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件，目标文件可由 `node --file pkg/objstore/s3store/mock/s3api_mock.rs` 完整读取。
- RustCodeGraph `query S3Api --kind trait`、`query NewMockS3API --kind function`、`query OptionFn --kind struct` 定位到本文件的主要符号，同时显示 `interface.rs::S3API` 是另一个 trait。对 `NewMockS3API`、`AbortMultipartUpload` 等运行 `callers`/`callees` 未返回边，因此未把宏生成边当作已验证图证据。
- 源码：`pkg/objstore/s3store/mock/s3api_mock.rs`（类型、trait、mock 宏、兼容函数）；`mock/lib.rs`（模块声明与重导出）；`s3store/lib.rs`（上层模块装配）；`mock/Cargo.toml`（crate 边界与依赖）。
- Go 对照：`pkg/objstore/s3store/interface.go`（14 个生产接口方法）；`mock/s3api_mock.go`（MockGen 生成的 mock、recorder、构造函数和期望方法）；`main_test.go` 与 `s3_test.go`（Go mock 的实际注入和期望使用）。
- Rust 测试：`pkg/objstore/s3store/mock/migration_aster_unit_test.rs` 的三个独立测试覆盖参数传递、输出、错误、调用次数和完整 Go 方法集。全仓 `rg` 未发现这些本文件特有符号的其他 Rust 消费者。
- 按任务约束，本次是纯文档分析，不运行 Cargo。结构验证使用任务文件指定的 `test -f` 与 11 个固定标题计数命令。
