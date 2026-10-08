# `pkg/server/handler/auto_id_owner_handler.rs`

源文件：[`auto_id_owner_handler.rs`](./auto_id_owner_handler.rs)

## 文件定位

本文件属于独立库 crate `astersql-server-handler`，crate 根由 `pkg/server/handler/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，模块入口 `pkg/server/handler/lib.rs` 通过 `pub mod auto_id_owner_handler` 对外暴露它。它把“实例健康检查”和“Auto ID 服务 owner 身份查询”收敛成一个很小的、与具体 HTTP 框架解耦的处理核心。

需要区分 Rust 与 Go 的接线状态：Go 生产服务器在 `pkg/server/http_status.go` 中仅于 `deploymode.IsStarter()` 为真时把 `/owner_manager/auto_id_service` 注册到 `handler.NewAutoIDOwnerHandler(s)`；Rust 仓库搜索到的直接使用者是测试，尚未发现 Rust 生产状态服务器注册该路由。因此，本文件当前提供了可复用处理逻辑和抽象边界，但不能单凭它断言 Rust 服务器已经对外提供该端点。

## 核心职责

文件承担三项职责：

1. 用 `AutoIDOwnerChecker` 描述处理器向服务器运行时索取的最小能力：健康状态 `Health()` 与 owner 状态 `IsAutoIDOwner()`。
2. 用 `AutoIDOwnerResponse` 描述响应端的最小写出能力，将 HTTP 状态码、JSON 序列化和具体 Web 框架留给适配器。
3. 在 `AutoIDOwnerHandler::ServeHTTP` 中执行固定顺序的判定：先检查健康状态；不健康时只写 500，健康时才查询 owner 状态并写出 `autoIDOwnerStatus`。

这一顺序是安全边界而不只是实现细节：`pkg/server/handler/tests/http_handler_test.rs` 的 `auto_id_owner_handler_returns_http_500_when_unhealthy` 令 `IsAutoIDOwner()` 直接 panic，用以证明健康检查失败后绝不能继续读取 owner 状态。

## 主要符号

- `pub trait AutoIDOwnerChecker`：输入侧能力接口。`Health(&self) -> bool` 表示实例是否可服务；`IsAutoIDOwner(&self) -> bool` 表示本实例当前是否持有 Auto ID owner 身份。方法只借用 `self`，处理器不负责改变这两种状态。
- `pub trait AutoIDOwnerResponse`：输出侧适配接口。关联类型 `Error` 让具体 writer 自行决定错误类型；`write_status(u16)` 用于仅写状态码，`write_owner_status(autoIDOwnerStatus)` 用于写 owner 载荷。
- `pub struct AutoIDOwnerHandler<C>`：泛型处理器，私有字段 `checker: C` 独占保存检查器。泛型约束在构造函数和实现块上要求 `C: AutoIDOwnerChecker`。
- `pub struct autoIDOwnerStatus`：响应数据对象，仅含公开布尔字段 `IsOwner`。它派生 `Debug`、`Clone`、`Copy`、`PartialEq`、`Eq`，但本文件本身不实现 serde；JSON 字段命名由 `AutoIDOwnerResponse` 实现负责。
- `pub fn NewAutoIDOwnerHandler<C>(checker: C) -> AutoIDOwnerHandler<C>`：按值接收检查器并构造处理器。Rust 测试既传入拥有值，也通过为 `&FakeAutoIDOwnerChecker` 实现 trait 来共享可变测试状态。
- `AutoIDOwnerHandler::ServeHTTP<W>(&self, writer: &mut W) -> Result<(), W::Error>`：核心入口；只读借用处理器、可变借用响应 writer，并原样传播 writer 错误。
- 包级 `pub fn ServeHTTP(...)`：薄转发入口，调用同名实例方法；当前仓库未搜索到该自由函数的直接使用者。

文件级 `#![allow(dead_code, non_snake_case, non_camel_case_types)]` 允许保留 Go 移植命名，例如 `Health`、`IsOwner`、`NewAutoIDOwnerHandler` 和 `autoIDOwnerStatus`。

## 执行流程

`AutoIDOwnerHandler::ServeHTTP` 的完整流程如下：

1. 调用 `self.checker.Health()`。
2. 若返回 `false`，立即调用 `writer.write_status(500)` 并返回其 `Result`；不会调用 `IsAutoIDOwner()`，也不会写 owner 载荷。
3. 若健康，调用 `self.checker.IsAutoIDOwner()` 取得瞬时布尔值。
4. 用该值构造 `autoIDOwnerStatus { IsOwner: ... }`，再调用 `writer.write_owner_status(...)`。
5. 将响应适配器的成功或错误不加包装地返回给调用者。

本文件不读取请求方法、路径、头或 body，也不自己选择健康路径的 HTTP 200。测试中的 `Recorder::write_owner_status` 将状态设为 200 并编码 `{"is_owner": ...}`，说明这些行为属于响应适配层。包级 `ServeHTTP` 不增加任何分支，只转发到实例方法。

## 数据与状态

处理器唯一的持久字段是 `checker: C`。构造时检查器按值移入，之后每个请求仅通过共享借用读取它；该文件没有全局变量、缓存、计数器或可变静态状态。

一次调用中的临时数据只有健康布尔值和健康分支创建的 `autoIDOwnerStatus`。状态对象为 `Copy` 值，写给响应后不再由处理器保留。owner 结果是查询时刻的快照；本文件不锁定 owner 租约，也不保证写出完成前身份不会变化。

Go 生产实现的真实数据源位于 `pkg/server/server.go`：`Server.Health()` 读取 `s.health.Load()`，`Server.IsAutoIDOwner()` 在 `s.autoIDService != nil` 时调用 `IsOwner()`，否则返回 `false`。这些是 Go 端 `Server` 满足 checker 接口的依据，不是当前 Rust handler 内部拥有的状态。

## 依赖与调用关系

内部调用边很短：`NewAutoIDOwnerHandler` 构造 `AutoIDOwnerHandler`；实例 `ServeHTTP` 依次可能调用 `AutoIDOwnerChecker::Health`、`AutoIDOwnerChecker::IsAutoIDOwner`、`AutoIDOwnerResponse::write_status` 或 `write_owner_status`；包级 `ServeHTTP` 调用实例 `ServeHTTP`。

RustCodeGraph 将目标文件列为由 `pkg/server/handler/tests/http_handler_test.rs` 使用；仓库引用搜索还找到：

- `pkg/server/handler/auto_id_owner_handler_test.rs`：crate 内独立单元测试，覆盖健康非 owner、健康 owner、不健康三条路径。
- `pkg/server/handler/tests/http_handler_test.rs`：外部测试 crate 验证不健康时写 500 且不查询/写 owner。
- `pkg/server/handler/tests/http_handler_serial_test.rs`：验证健康 owner 载荷，同时明确当前 Rust 测试服务器访问真实路径得到 404，记录了尚未接线的现状。

`pkg/server/handler/Cargo.toml` 没有为本文件引入 Web 框架或序列化依赖；该文件仅使用 Rust 语言本身。测试 crate 在 `pkg/server/handler/tests/Cargo.toml` 通过路径 dev-dependency `astersql-server-handler = { path = ".." }` 使用它。Go 上游路由和运行时 checker 分别位于 `pkg/server/http_status.go` 与 `pkg/server/server.go`。

## 错误处理与边界

本文件可返回的错误只来自 `AutoIDOwnerResponse`。不健康路径返回 `write_status(500)` 的结果；健康路径返回 `write_owner_status(...)` 的结果。处理器不吞掉、不转换、不记录错误，也没有重试。

明确边界包括：

- 健康失败是短路条件，500 是硬编码状态码。
- 健康成功并不等于本实例是 owner；`IsOwner: false` 是正常成功载荷，不是错误。
- checker 方法不能返回错误，因此底层健康/选主查询失败必须由 checker 实现折叠为布尔值，或在进入本处理器前处理。
- 本文件不负责验证 HTTP method、路由是否只在 Starter 模式暴露、Content-Type、JSON 字段名或响应码 200；这些均属于路由和 writer 适配层。
- 若 `write_status` 或 `write_owner_status` 发生部分写入后返回错误，本文件没有回滚能力。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或后台资源。`ServeHTTP` 使用 `&self`，所以 API 允许多个调用共享同一个 handler；但能否安全跨线程共享取决于外层是否要求且 `C` 是否实现相应的 `Send`/`Sync`，本文件没有添加这些 trait 约束。

writer 在一次调用期间以 `&mut W` 独占借用，防止同一响应对象在该调用内被并发写入。checker 从构造开始由 handler 持有，随 handler 一同销毁；响应载荷按值传给 writer。对于需要动态更新状态的 checker，实现方应使用自身的原子量、锁或其他同步机制，本文件不会替其同步。Go 对照中的健康状态由原子读取支持，而 Auto ID owner 生命周期由 `autoIDService` 管理。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/server/handler/auto_id_owner_handler.go`：checker 接口的两个方法、持有 checker 的 handler、含 `IsOwner` 的状态对象、构造函数，以及“先健康检查、后 owner 查询”的处理顺序均一致。`pkg/server/handler/auto_id_owner_handler_test.go` 与 Rust 独立测试都覆盖 `false -> true -> unhealthy` 三种状态。

关键差异如下：

- Go handler 直接实现 `net/http` 的 `ServeHTTP(http.ResponseWriter, *http.Request)`；Rust 用 `AutoIDOwnerResponse` 抽象 writer，并完全省略未使用的 request 参数。
- Go 的 `autoIDOwnerStatus.IsOwner` 带 `json:"is_owner"` 标签，并通过 `WriteData` 完成 JSON 写出；Rust 状态类型不依赖 serde，具体序列化由响应适配器实现。
- Go 构造函数返回指针，接口值支持动态分派；Rust 返回拥有值，并以泛型对 checker 和 writer 静态分派。
- Go 生产路由在 Starter 模式注册，且 `Server` 提供真实 `Health`/`IsAutoIDOwner`；当前 Rust 证据只证明处理核心和测试存在，真实 Rust 路由测试仍断言 404。因此移植尚未包含等价生产接线。

## 扩展指南

若只改变 owner 判定流程，应优先修改 `AutoIDOwnerHandler::ServeHTTP`，并同步扩展独立测试 `pkg/server/handler/auto_id_owner_handler_test.rs`；必须保留“不健康时不调用 `IsAutoIDOwner`”这一不变量。若新增可失败的 checker 查询，需谨慎修改 `AutoIDOwnerChecker` 的返回类型，并决定如何映射为 HTTP 状态，同时会影响所有测试假实现。

若新增响应字段，应修改 `autoIDOwnerStatus`、`AutoIDOwnerResponse::write_owner_status` 的实现及 Go 对照语义，并覆盖字段命名和兼容性测试。由于 Rust 类型本身没有序列化标签，不能只增加字段而假设 JSON 自动正确。

若要完成 Rust 生产接线，需要在状态服务器路由层实现 `AutoIDOwnerChecker` 与 `AutoIDOwnerResponse` 适配器，并复现 Go 的 Starter 模式注册条件；随后应把 `pkg/server/handler/tests/http_handler_serial_test.rs` 中当前 404 断言更新为针对实际模式的端到端断言。这属于本文件之外的接线工作。性能风险主要在 checker 实现：请求路径应保持无阻塞或使用廉价的原子/租约状态读取；本处理器本身没有分配集合或循环。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/server/handler` 找到目标、Go 对照和相关测试；`node --file pkg/server/handler/auto_id_owner_handler.rs` 核对 76 行源码及“由 `pkg/server/handler/tests/http_handler_test.rs` 使用”的关系；`query AutoIDOwnerHandler`、`query NewAutoIDOwnerHandler`、`query IsAutoIDOwner` 核对 Rust/Go 对应符号。
- 生产源码：`pkg/server/handler/auto_id_owner_handler.rs`、`pkg/server/handler/lib.rs`、`pkg/server/handler/Cargo.toml`。
- Go 对照与接线：`pkg/server/handler/auto_id_owner_handler.go`、`pkg/server/http_status.go`、`pkg/server/server.go`。
- 独立测试：`pkg/server/handler/auto_id_owner_handler_test.rs`、`pkg/server/handler/auto_id_owner_handler_test.go`、`pkg/server/handler/tests/http_handler_test.rs`、`pkg/server/handler/tests/http_handler_serial_test.rs`，以及测试 crate 声明 `pkg/server/handler/tests/Cargo.toml`。
- 仓库引用搜索确认包级自由函数没有直接调用者，并确认 Rust 生产代码中未找到 `/owner_manager/auto_id_service` 路由注册；这些结论仅描述当前检出的仓库状态。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务给定命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核未把测试或 Go 路由误写为 Rust 生产接线。
