# `pkg/objstore/ossstore/mock/api_mock.rs`

## 文件定位

本文件属于独立 Rust crate `astersql-objstore-ossstore-mock`。该 crate 由同目录的 `Cargo.toml` 声明，唯一直接依赖是 `mockall = "0.13"`；根 `Cargo.toml` 同时把 `pkg/objstore/ossstore/mock` 列为 workspace member，并以 `facade_objstore_ossstore_mock` 暴露路径依赖。模块入口 `pkg/objstore/ossstore/mock/lib.rs` 声明 `pub mod api_mock` 并 `pub use api_mock::*`，因此本文件的公开类型可从 crate 根访问。

它提供的是测试用 OSS API 替身：用简化的请求/响应数据结构描述 Go OSS SDK 风格的接口，再由 `mockall::mock!` 生成可配置调用期望的 `MockAPI`。它不发起网络请求，也没有对象存储实现。还需区分它与 `pkg/objstore/ossstore/interface.rs` 的生产 `API` trait：两者类型、方法命名和返回模型不同，当前搜索到的 Rust 使用者仅为本 mock crate 的独立测试，因此不能把这里的 `Api` 当作生产 OSS 客户端已经接线的 mock 实现。

## 核心职责

1. 定义测试可构造、可比较的上下文、选项、错误以及 OSS 请求/结果占位类型，例如 `Context`、`Options`、`OssError`、`PutObjectRequest` 和 `GetObjectResult`。
2. 用 `Api: Send + Sync` 列出 13 个 OSS 操作，包括对象读写/删除/拷贝、桶存在性检查以及分片上传生命周期。
3. 通过 `mockall::mock!` 生成 `MockAPI` 及各方法对应的 `expect_*` 配置入口，使测试可以匹配参数、限定调用次数并返回成功或错误。
4. 保留 Go mockgen 的表面命名：`NewMockAPI`、`MockAPIMockRecorder`、`EXPECT` 和 `ISGOMOCK`，降低 Go 测试迁移时的结构差异。

本文件的职责止于测试协议和调度。它不会应用 `OptionFn`、解释请求字段、维护对象内容、执行重试或负责 OSS 资源清理；这些行为必须由测试回调或真实实现承担。

## 主要符号

- `Context { request_id: String }`：简化的请求上下文，仅保留可用于断言的请求标识；它不承载 Go `context.Context` 的取消、截止时间或值传播语义。
- `Options { endpoint: Option<String> }` 与 `OptionFn(Arc<dyn Fn(&mut Options) + Send + Sync>)`：表示 functional option。`OptionFn::new` 封装线程安全的 `'static` 闭包，`apply` 显式调用闭包，`Default` 生成空操作选项。自定义 `Debug` 只输出 `OptionFn(..)`，避免要求闭包本身实现 `Debug`。
- `OssError(String)`：可克隆、可比较的轻量错误；`new` 接受任意 `Into<String>`，并实现 `Display` 和 `std::error::Error`。
- `request_with_key!`：生成 11 个只有 `key: String` 的请求类型；这些类型用于参数匹配，不等同于阿里云 SDK 的完整请求结构。
- `PutObjectRequest { key, body }` 与 `GetObjectResult { body }`：唯二带额外载荷的业务数据结构。其余 11 个结果由 `empty_result!` 生成为零大小占位类型。
- `Api`：公开、同步的 mock 协议。13 个方法均接收 `&self`、`&Context` 和 `&[OptionFn]`；除 `IsBucketExist` 接收桶名并返回 `bool` 外，其余方法接收对应请求并返回对应结果。全部失败路径统一为 `OssError`。
- `MockAPI`：由 `mockall::mock!` 生成并实现 `Api`。生成代码提供 `expect_AbortMultipartUpload`、`expect_GetObject`、`expect_PutObject` 等期望构造器，实际检查、调用计数和回调调度由 `mockall` 完成。
- `MockAPIMockRecorder = MockAPI`：类型别名，不是 Go 版本那种持有 mock 指针的独立 recorder。
- `NewMockAPI() -> MockAPI`：直接调用 `MockAPI::new()`；与 Go 构造函数不同，不接收 `gomock.Controller`。
- `MockAPI::EXPECT(&mut self)` 与 `ISGOMOCK(&self)`：前者返回自身可变引用以继续调用 `expect_*`，后者是无返回值的兼容标记。

文件没有条件编译项；测试模块的 `#[cfg(test)]` 接线位于 `lib.rs`。

## 执行流程

典型测试流程如下：

1. 测试调用 `NewMockAPI`，由 `MockAPI::new` 建立空的 mock 状态。
2. 测试调用 `mock.EXPECT()` 取得 `&mut MockAPIMockRecorder`。由于 recorder 只是 `MockAPI` 的别名，这一步实际仍在操作同一个 mock。
3. 测试调用生成的 `expect_<方法名>()`，再用 `withf` 描述参数谓词、用 `times` 指定次数、用 `return_once` 或其他 mockall 返回策略配置结果。
4. 被测代码或测试直接通过 `Api` 方法调用 `MockAPI`。mockall 按方法和参数寻找期望，执行匹配的返回回调，并更新调用次数。
5. 若返回回调给出 `Ok` 或 `Err(OssError)`，该结果原样从 trait 方法返回。`migration_aster_unit_test.rs` 分别验证了 `IsBucketExist`/`GetObject` 的参数与成功结果转发，以及 `PutObject` 错误传播。
6. mock 生命周期结束时，mockall 负责校验尚未满足的期望；显式的 `.times(1)` 使重复调用或漏调用成为测试失败。

`OptionFn` 不会在上述调度中自动应用。测试当前只断言 option slice 的长度；需要验证选项效果时，返回回调必须逐个调用 `OptionFn::apply`。

## 数据与状态

持久状态主要位于 mockall 生成的 `MockAPI` 内部，包括已登记的期望、匹配器、返回动作和调用次数；本文件没有全局变量或外部存储。请求和结果大都派生 `Clone`、`Debug`、`Default`、`PartialEq`、`Eq`，便于测试构造与精确断言。

`Context`、请求引用和 option slice 都只在一次调用期间借用，不由 mock API 保存。`PutObjectRequest.body` 与 `GetObjectResult.body` 使用拥有所有权的 `Vec<u8>`，适合内存测试但不能表达流式 body 或关闭语义。空结果类型只表示“操作成功且没有本测试关心的返回字段”。

`OptionFn` 通过 `Arc` 共享闭包，克隆不会复制闭包状态。闭包被要求 `Send + Sync`，但若其内部包含同步状态，该状态的语义和生命周期由闭包创建者负责。`OssError` 只保存文本，未携带服务错误码、来源链或重试分类。

## 依赖与调用关系

直接依赖关系为：`lib.rs` 装配并重新导出本模块；本文件依赖标准库的 `fmt`、`Arc`，以及 `mockall::mock!`。没有阿里云 OSS Rust SDK、生产 `astersql-objstore-ossstore` crate 或异步运行时依赖。

RustCodeGraph 将目标文件索引为 33 个符号，但对 `NewMockAPI`、`OptionFn::apply` 和宏生成方法的 `callers`/`callees` 查询没有输出静态边；宏展开后的期望调度也不在索引的显式源码调用图中。仓库文本搜索补充确认，Rust 侧 `NewMockAPI`/本模块 `MockAPI` 的直接使用位于：

- `pkg/objstore/ossstore/mock/api_mock_test.rs`：验证 `EXPECT` 返回公开 recorder 类型别名。
- `pkg/objstore/ossstore/mock/migration_aster_unit_test.rs`：验证参数转发、结果/错误返回、调用次数及多数方法可配置。

生产 Rust OSS 抽象位于 `pkg/objstore/ossstore/interface.rs::API`，使用 `storeapi::Context`、`anyhow::Result` 和更完整的输入/输出类型；本文件没有实现该 trait。因此当前可证实的上游是 mock crate 自身测试，而不是完整应用的 OSS 主链。

## 错误处理与边界

API 方法不在本文件内创建、包装或分类错误；测试配置的 `OssError` 被直接返回。未配置调用、参数不匹配、调用次数不符等属于 mockall 的测试失败/恐慌路径，而不是 `Result::Err`。`OssError` 的字符串比较适合迁移断言，但无法表达 Go SDK 的结构化 `ServiceError`、错误链或重试信息。

请求模型有意简化：由 `request_with_key!` 生成的类型只有 `key`，缺少 bucket、upload ID、part number、分页参数等真实 OSS 字段；空结果也缺少 ETag、分页令牌和 upload ID。`Context` 没有取消/超时；`GetObjectResult.body` 没有 `Read`/`Close` 生命周期。使用者不得据此推断真实客户端具备同样的边界行为。

Go `pkg/objstore/ossstore/interface.go::API` 和生成的 `api_mock.go` 均包含 `Presign`，而 Rust 本文件的 `Api` 与 `MockAPI` 没有该方法。`migration_aster_unit_test.rs::api_mock_covers_the_complete_go_method_set` 的名称声称覆盖完整方法集，但测试本身也未检查 `Presign`；所以当前事实是存在对齐缺口，而不是完整对齐。

## 并发与资源生命周期

`Api: Send + Sync` 以及 `OptionFn` 闭包的 `Send + Sync` 约束允许类型跨线程边界使用；不过配置期望需要 `&mut MockAPI`，正常模式是先单线程完成配置，再把 mock 交给调用方。具体并发调用是否安全以及并发时期望匹配的顺序保证由 mockall 0.13 的生成实现决定，本文件没有额外锁或顺序协议，现有测试也未验证并发调用。

本文件不创建线程、任务、通道、锁、事务、文件句柄或网络连接。`Arc` 在最后一个 `OptionFn` 克隆释放时销毁闭包。请求/结果按 Rust 所有权规则释放；内存 body 无需显式关闭。`MockAPI` 释放时会触发 mockall 的期望校验，因此测试应保证所有 `.times(...)` 约束在 drop 前满足，且不应依赖 Go `gomock.Controller::Finish` 的独立控制器生命周期。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/objstore/ossstore/mock/api_mock.go`，它由 MockGen 根据 `pkg/objstore/ossstore/interface.go::API` 生成。Rust 版本手写协议，再交给 mockall 生成 mock；对应关系如下：

- Go `MockAPI` 持有 `*gomock.Controller` 和独立 recorder；Rust `MockAPI` 内含 mockall 状态，`MockAPIMockRecorder` 只是类型别名。
- Go `NewMockAPI(ctrl)` 依赖测试控制器；Rust `NewMockAPI()` 无参数，期望校验随 mockall 对象生命周期进行。
- Go `EXPECT()` 返回独立 recorder 并使用同名方法登记调用；Rust `EXPECT()` 返回自身，随后使用 `expect_<方法名>`。因此只保留链式入口形状，不是逐字符相同的 API。
- Go variadic `...func(*oss.Options)` 对应 Rust 借用的 `&[OptionFn]`；Rust option 需显式 `apply`，且本文件不会自动执行。
- Go SDK 使用指针请求/结果并允许 `nil`；Rust使用引用输入和拥有所有权的结果，成功结果不能为 `None`。
- 13 个非预签名方法在名称层面相互对应；Rust 的请求/结果字段是测试最小模型。Go 的第 14 个方法 `Presign` 当前没有 Rust 对应项。

Go 测试 `pkg/objstore/ossstore/client_test.go` 把生成 mock 注入生产 `client.svc`，覆盖权限检查、读取和错误行为；预签名测试则另建 `presignAPI`。Rust mock 的测试没有展示把本文件类型注入 `pkg/objstore/ossstore/interface.rs::API`，且两套 trait 不兼容，说明 Rust mock 当前主要是迁移契约测试资产而非生产客户端测试替身。

## 扩展指南

新增或修改 OSS 操作时，应同步处理至少四处：`Api` trait 的签名、`mockall::mock!` 中的实现声明、相应请求/结果数据类型，以及独立测试 `migration_aster_unit_test.rs`。若需要继续保持 Go 对齐，还应逐项核对 `interface.go` 和生成的 `api_mock.go`，不要仅凭测试函数名称判断方法集完整。

补齐 `Presign` 时需要单独设计 `PresignOptions`、请求联合类型和 `PresignResult`，因为现有 `Options`/`OptionFn` 不能准确表示 Go 的 `func(*oss.PresignOptions)`。若目标是让该 mock 真正注入 Rust 生产客户端，更安全的方向是实现或直接 mock `pkg/objstore/ossstore/interface.rs::API`，统一 `storeapi::Context`、输入/输出和错误类型；这会跨 crate 边界，需评估依赖方向，不能只在本文件增加同名方法。

扩充请求字段时优先修改明确的结构体；若所有 key-only 请求不再同构，应从 `request_with_key!` 中拆出个别类型。涉及 body 流或关闭行为时，不能继续用 `Vec<u8>` 冒充资源生命周期，应增加独立可关闭抽象及相应测试。并发使用需要新增独立测试验证期望匹配、调用次数和 drop 行为，不要把测试代码内嵌到本生产文件。

兼容风险主要是公开类型/方法名、mockall 生成的 `expect_*` 名称和 Go 迁移测试调用形状；性能风险较低，因为该 crate 面向测试，但大 body 的 `Vec<u8>` 克隆与复杂 `withf` 匹配仍可能增加测试成本。

## 验证依据

- RustCodeGraph：`status` 显示索引含目标文件；`files --filter pkg/objstore/ossstore/mock` 列出 8 个 Go/Rust 文件；`node --file pkg/objstore/ossstore/mock/api_mock.rs --offset 1 --limit 500` 返回完整 283 行和 33 个符号；`query MockAPI`、`query MockAPIMockRecorder` 定位 Rust 别名/构造函数以及 Go 生成符号；精确 `callers`/`callees` 查询没有返回本文件宏生成调用边。
- Rust 源与装配：`pkg/objstore/ossstore/mock/api_mock.rs`、`pkg/objstore/ossstore/mock/lib.rs`、`pkg/objstore/ossstore/interface.rs`。
- Cargo 边界：`pkg/objstore/ossstore/mock/Cargo.toml` 与根 `Cargo.toml` 的 workspace member、`facade_objstore_ossstore_mock` 路径依赖。
- Rust 独立测试：`pkg/objstore/ossstore/mock/api_mock_test.rs` 和 `pkg/objstore/ossstore/mock/migration_aster_unit_test.rs`。
- Go 对照与实际测试用法：`pkg/objstore/ossstore/interface.go`、`pkg/objstore/ossstore/mock/api_mock.go`、`pkg/objstore/ossstore/client_test.go`。
- 仓库搜索：除上述两个 Rust 测试文件外，未发现本模块 `NewMockAPI`/`MockAPI` 在 Rust OSS 生产代码中的直接使用；生产 Rust 使用另一套 `interface.rs::API`。

本任务为纯文档分析，按计划不运行 Cargo。结构验收使用任务指定命令，要求本文恰有 11 个固定二级标题。
