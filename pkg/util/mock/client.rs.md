# `pkg/util/mock/client.rs` 逻辑说明

## 文件定位

`pkg/util/mock/client.rs` 属于 `astersql-util-mock` crate。该 crate 由 `pkg/util/mock/Cargo.toml` 定义，并以 `pkg/util/mock/lib.rs` 为入口；入口通过私有 `mod client` 装载本文件，再以 `pub use client::*` 导出其中的 `Client` 和 `SharedResponse`。它是测试辅助实现，不发送真实 TiKV/DistSQL 请求，而是把预先注入的 `kv::Response` 作为 `kv::Client::Send` 的结果返回。

本文件直接依赖同 crate 入口再导出的 `kv`（实际 Cargo 包为路径依赖 `astersql-kv`，位于 `pkg/kv`）。当前仓库内对本文件具体类型的直接 Rust 使用集中在 `pkg/util/mock/migration_aster_unit_test.rs`；`pkg/util/mock/store.rs` 则提供可保存 `Arc<dyn kv::Client + Send + Sync>` 的相邻装配点。没有条件编译项、模块级常量或独立同名测试文件。

## 核心职责

- `Client` 实现 `kv::Client`，忽略 `Send` 的上下文、请求、会话变量和发送选项，固定返回配置的 mock 响应；未配置时返回 `None`。依据：`pkg/util/mock/client.rs::Client`、`Client::Send`。
- `SharedResponse` 把单个可变响应放进 `Arc<Mutex<_>>`，使每次发送都能返回新的 trait-object 句柄，同时所有句柄仍访问同一底层响应状态。依据：`ResponseInner`、`SharedResponse::new`、`SharedResponse::{Next,Close}`。
- `Client::IsRequestTypeSupported` 不在本文件复制能力白名单，而是委托 `pkg/kv/checker.rs::RequestTypeSupportedChecker::IsRequestTypeSupported`。这使 mock 客户端与 KV crate 的请求类型判断保持一致。

## 主要符号

- `type ResponseInner = Arc<Mutex<Box<dyn kv::Response + Send>>>`：私有类型别名。`Send` 约束允许底层响应在持锁条件下跨线程共享；未要求底层响应实现 `Sync`，因为互斥锁提供同步边界。
- `pub struct SharedResponse { inner: ResponseInner }`：可克隆的公开响应包装；`inner` 私有，外部不能绕过锁直接访问响应。
- `SharedResponse::new(response)`：取得底层 `Box<dyn kv::Response + Send>` 的所有权，并建立唯一的初始 `Arc<Mutex<_>>`。
- `SharedResponse::ptr_eq(&self, other)`：用 `Arc::ptr_eq` 判断两个包装是否共享同一分配；它不比较响应内容，也不加锁。
- `impl kv::Response for SharedResponse`：`Next` 和 `Close` 都先取得同一把锁，再原样返回底层实现的结果。
- `pub struct Client`：公开字段 `RequestTypeSupportedChecker` 保存无状态能力检查器；`MockResponse: Option<SharedResponse>` 表达 Go `nil` 响应。字段名保留 Go 风格，crate 入口通过 `#![allow(non_snake_case)]` 接受这种命名。
- `Client::default()`：构造能力检查器，并把 `MockResponse` 设为 `None`。
- `Client::new(response)`：构造能力检查器，并用 `SharedResponse::new` 保存给定响应。
- `Client::SendMockResponse()`：克隆可选的 `SharedResponse`，再擦除为 `Box<dyn kv::Response>`；克隆的是句柄，不是底层响应。
- `impl kv::Client for Client`：公开行为由 trait 的 `Send` 与 `IsRequestTypeSupported` 两个方法构成。对应 trait 契约见 `pkg/kv/kv.rs::Client`。

## 执行流程

1. 测试以 `Client::new(Box<dyn kv::Response + Send>)` 注入响应；构造函数将它放入 `Arc<Mutex<_>>`。也可以用 `Client::default()` 得到没有响应的客户端。
2. 上层通过 `kv::Client::Send` 发起请求。该实现刻意不读取四个输入参数，只调用 `Client::SendMockResponse`；因此请求内容不会改变返回值，也不会发生 I/O。
3. `SendMockResponse` 对 `MockResponse` 执行 `as_ref().cloned()`：有值时新建一个装有 `SharedResponse` 克隆的 `Box<dyn kv::Response>`，无值时保留 `None`。
4. 消费方对返回响应调用 `Next` 或 `Close` 时，`SharedResponse` 锁住公共底层对象并在锁内调用同名方法；底层返回的结果或错误不经转换地向上传播。
5. 上层查询请求能力时，`Client::IsRequestTypeSupported` 直接调用字段中的检查器。实际 Select/Index/DAG/Analyze 分支和表达式白名单位于 `pkg/kv/checker.rs`，不属于本文件逻辑。

RustCodeGraph 的 `node pkg/util/mock/client.rs::SendMockResponse` 给出的直接调用边为：`Client::Send -> Client::SendMockResponse`；节点源码同时确认 `SendMockResponse` 的包装路径。索引对按符号 ID 发起的通用 `callers` 查询未在限定时间内返回，而 `callees` 将 ID 误作名称产生无关候选，因此调用关系以精确 `node` trail、目标源码和 `rg` 引用结果交叉确认，不把该异常输出作为调用证据。

## 数据与状态

持久状态只有两个层次：`Client::MockResponse` 决定客户端是否有响应；存在响应时，`SharedResponse::inner` 持有唯一底层响应及其可变游标/关闭状态。`Arc::clone` 只增加强引用计数，所有由一次 `Client::new` 派生出的响应句柄共享底层状态。不同的 `Client::new` 调用创建不同的 `Arc`，互不共享。

`RequestTypeSupportedChecker` 是 `pkg/kv/checker.rs` 中的零字段结构体，本文件不为能力检查保存额外状态。`Send` 的请求参数完全不进入状态，因而该 mock 不记录请求、不按请求生成响应，也不复制响应流。

## 依赖与调用关系

- 上游契约：`pkg/kv/kv.rs::Client` 要求 `Send -> Option<Box<dyn Response>>` 和 `IsRequestTypeSupported`；`pkg/kv/kv.rs::Response` 要求可变的 `Next` 与 `Close`。
- crate 接线：`pkg/util/mock/lib.rs` 再导出 `kv_crate as kv` 并导出 `client::*`；`pkg/util/mock/Cargo.toml` 对 `../../kv` 声明 `kv-crate` 路径依赖，没有与本文件相关的 feature 开关。
- 直接下游：`Client::Send` 调用 `Client::SendMockResponse`；`Client::new` 调用 `SharedResponse::new`；`SharedResponse::{Next,Close}` 调用所包装的 `kv::Response`；能力查询调用 `RequestTypeSupportedChecker::IsRequestTypeSupported`。
- 相邻装配：`pkg/util/mock/store.rs::Store::Client` 能保存 `Arc<dyn kv::Client + Send + Sync>` 并由 `Store::GetClient` 克隆返回。这是 mock crate 中客户端进入存储抽象的容器，但全仓精确引用搜索没有发现生产 Rust 路径直接构造本文件 `Client`；已确认的直接构造位于回归测试。
- 测试调用：`pkg/util/mock/migration_aster_unit_test.rs::client_returns_handles_to_the_same_configured_response` 调用 `Client::new` 和两次 `SendMockResponse`；`default_client_send_returns_none` 通过 trait 签名约束 `kv::Client::Send`，并断言默认响应为空。

## 错误处理与边界

- `Client::Send` 本身没有错误通道；未配置响应由 `None` 表示，与 Go 的 `nil` 对齐。输入参数不做校验，因为其职责只是固定响应注入。
- `SharedResponse::Next` 与 `Close` 原样传播底层 `kv::errors::SharedError`，不吞掉、不包装错误。
- 若另一个持锁调用发生 panic 导致互斥锁中毒，两个委托方法使用 `poisoned.into_inner()` 继续访问底层响应，而不是再次 panic。此选择只能恢复锁中数据的访问权，不能保证发生 panic 后底层响应的业务状态仍一致。
- `Close` 没有幂等保护；多个句柄可重复调用，具体是否成功完全由底层响应决定。`Next` 与 `Close` 也共享同一锁和同一状态。
- `SendMockResponse` 返回的 trait 对象不暴露 `SharedResponse::ptr_eq`，如需检查指针同一性，应在类型擦除前比较 `SharedResponse`；现有测试改用共享计数行为证明状态同一。

## 并发与资源生命周期

底层响应的所有权在 `SharedResponse::new` 时移入 `Arc<Mutex<Box<_>>>`，最后一个 `SharedResponse`（包括装在返回 trait object 中的克隆）销毁后，`Arc` 才释放响应。`Client` 被销毁不会立即释放仍由调用方持有的响应句柄。

`Mutex` 将每次完整的 `Next` 或 `Close` 串行化，因此同一响应不会被两个句柄同时可变访问；锁覆盖底层方法的全部执行时间，慢调用会阻塞其他句柄。源码没有异步任务、通道或显式线程创建。底层类型必须是 `Send`，而 `Arc<Mutex<T>>` 在此约束下提供跨线程共享能力；不过并发调用的业务顺序取决于抢锁顺序，源码不提供公平性或“关闭后禁止 Next”的额外保证。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/mock/client.go`。两端都有 `Client`、嵌入/持有的 `RequestTypeSupportedChecker`、可空 mock 响应，以及忽略全部参数并返回固定响应的 `Send`。

Rust 为适应所有权与 trait object 增加了 `SharedResponse`、`Client::new` 和 `SendMockResponse`：Go 的接口值可以在多次 `Send` 中直接返回同一对象，Rust 的 `Box<dyn Response>` 不能复制，因此通过 `Arc<Mutex<_>>` 为同一对象生成多个独立句柄。Rust 的 `Option` 对应 Go 的 `nil`；`Default` 对应 Go `Client{}` 的零值。Go 依靠匿名嵌入获得 `IsRequestTypeSupported`，Rust 则在 `impl kv::Client` 中显式转发。

值得注意的语义收紧是：Rust 构造函数只接受 `Box<dyn kv::Response + Send>`，并对共享访问进行串行化；Go 文件自身没有写出等价锁。除此之外，Rust 没有按请求复制响应或加入真实网络行为。Go 同目录测试未直接覆盖 `Client`；迁移后的专门证据位于 Rust 的 `migration_aster_unit_test.rs`。

## 扩展指南

- 若要按请求选择响应或记录请求，应修改 `Client::Send` 及 `Client` 状态，而不是绕过 `kv::Client`；同时在独立测试文件中覆盖参数分支、空响应和多次发送。此改动会偏离当前 Go 的“忽略参数、固定返回”语义，必须先核对对应 Go 增量。
- 若新增响应队列、错误脚本或工厂，应明确每次 `Send` 是共享一个响应、移动一个响应，还是新建响应。当前不变量是同一 `Client` 的每次发送共享底层对象；改变它会影响流游标、关闭和错误注入行为。
- 若修改 `SharedResponse::{Next,Close}` 的锁范围或中毒策略，应增加 `pkg/util/mock/migration_aster_unit_test.rs` 中的并发与 panic 后行为测试，并评估死锁、锁竞争及底层回调重入风险。
- 若扩展请求类型支持，只应修改权威实现 `pkg/kv/checker.rs::RequestTypeSupportedChecker` 及其独立测试；本文件保持转发，避免白名单漂移。
- 测试逻辑应继续放在独立的 `*_test.rs` 文件，不能内嵌进 `client.rs`。最接近的现有落点是 `pkg/util/mock/migration_aster_unit_test.rs`，并由 `pkg/util/mock/lib.rs` 的 `#[cfg(test)]` 模块声明接入。
- 兼容风险主要是 Go/Rust 空值和共享状态语义漂移；性能风险主要是全响应级互斥造成的串行化。mock 默认不进入真实 I/O 主链，因此不要把这里的行为外推为真实 TiKV 客户端性能特征。

## 验证依据

- RustCodeGraph：`status`（索引覆盖 11,467 个文件，`pkg/util/mock/client.rs` 被识别为 12 个符号）；`files --filter pkg/util/mock`；`query SharedResponse`、`query SendMockResponse`、`query RequestTypeSupportedChecker`、`query Response --kind trait`、`query IsRequestTypeSupported`、`query GetClient`；`node SharedResponse`；`node pkg/util/mock/client.rs::SendMockResponse`；`node --file pkg/util/mock/client.rs --offset 20 --limit 110`。精确节点 trail 证实 `Send` 调用 `SendMockResponse`。
- 源码与配置：`pkg/util/mock/client.rs`、`pkg/util/mock/lib.rs`、`pkg/util/mock/Cargo.toml`、`pkg/kv/kv.rs`、`pkg/kv/checker.rs`、`pkg/util/mock/store.rs`、`pkg/util/mock/context.rs`。
- Go 对照：`pkg/util/mock/client.go`，并用 `pkg/util/mock/store.go`、`pkg/util/mock/context.go` 核对相邻装配关系。
- 独立测试：`pkg/util/mock/migration_aster_unit_test.rs` 中的 `CountingResponse`、`client_returns_handles_to_the_same_configured_response` 和 `default_client_send_returns_none`。前者证明多个句柄共享底层可变响应，后者证明默认客户端为空；同目录 Go 测试未找到直接 Client 用例。
- 引用复核：`rg` 对 `Client`、`SharedResponse`、`SendMockResponse` 的精确搜索确认目标模块内的直接使用范围。本文只记录已由上述代码、索引和测试支持的当前事实；纯文档任务未运行 Cargo。
