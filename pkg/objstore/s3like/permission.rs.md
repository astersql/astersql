# `pkg/objstore/s3like/permission.rs`

## 文件定位

位于后端构造流程与 `PrefixClient` 实现之间：`pkg/objstore/ossstore/store.rs::NewOSSStorage` 和 `pkg/objstore/s3store/store.rs::NewS3Storage` 在客户端创建后、返回可用 `Storage` 前调用 `CheckPermissions`。

## 核心职责

- `CheckPermissions` 将 `storeapi::Permission` 抽象值映射到 `PrefixClient` 的四个权限探测方法，而不关心具体云厂商如何发出请求。
- 它严格按照 `perms` 切片顺序逐项执行。任一探测失败即通过 `?` 短路，后续权限不会再检查；空切片则直接成功。
- 它只接受当前可独立或组合探测的四类权限：`AccessBuckets`、`ListObjects`、`GetObject`、`PutAndDeleteObject`。`PutObject` 虽然是合法的 `storeapi::Permission` 枚举值，但在本入口中属于未知/不支持分支，因为仓库用组合探测验证写入和删除能力。
- 它在下游错误上附加 `permission <权限名>: <原错误>` 上下文，同时保留原始错误链，供更外层构造函数继续添加 OSS 或 S3 级上下文。

## 主要符号

`pub fn CheckPermissions(ctx: &storeapi::Context, cli: &dyn PrefixClient, perms: &[storeapi::Permission]) -> anyhow::Result<()>` 是本文件唯一的公开函数，也是唯一业务符号：

- `ctx` 是借用的 `storeapi::Context`，原样传给具体探测方法；函数不创建、克隆或取消上下文。
- `cli` 是借用的 `dyn PrefixClient` trait 对象。trait 定义在 `pkg/objstore/s3like/interface.rs`，并要求实现者为 `Send + Sync`，但本函数自身只做同步、顺序调用。
- `perms` 是借用的权限切片，不会被修改或缓存。
- 返回值为 `anyhow::Result<()>`：全部检查通过（包括无检查项）返回 `Ok(())`，第一个失败返回带上下文的错误。

文件级 `#![allow(non_snake_case)]` 仅用于保留与 Go 导出函数 `CheckPermissions` 一致的命名；本文件没有常量、类型定义、trait、`impl` 或条件编译项。

## 执行流程

1. `CheckPermissions` 以输入顺序遍历 `perms`。
2. 对每个元素执行穷举式业务映射：`AccessBuckets` 调用 `PrefixClient::CheckBucketExistence`，`ListObjects` 调用 `CheckListObjects`，`GetObject` 调用 `CheckGetObject`，`PutAndDeleteObject` 调用 `CheckPutAndDeleteObject`。
3. 其他枚举值立即返回 `anyhow!("unknown permission: {}", perm.as_str())`；当前直接证据是 `PutObject` 测试用例。
4. 已选择的探测方法若失败，闭包使用权限的 `as_str()` 值形成上下文，再由 `anyhow::Context::context` 包装原错误；随后的 `?` 立即把该错误返回。
5. 当前项成功才进入下一项；遍历结束后返回 `Ok(())`。

生产链中，`NewOSSStorage` 会再包装为 `check permission failed due to ...`，`NewS3Storage` 会再附加 `check S3 permissions`。两条路径都在权限检查成功后才继续凭证刷新、对象锁检查或高层 `s3like::Storage` 构造。

## 数据与状态

本文件没有全局、静态或可变持久状态。循环中唯一的临时数据是当前 `perm`、具体探测返回的 `result`，以及失败时创建的上下文字符串。

权限的稳定显示值来自 `pkg/objstore/storeapi/storage.rs::Permission::as_str`：`AccessBuckets` 特别对应 Go 字面量 `AccessBucket`，其余相关值分别为 `ListObjects`、`GetObject`、`PutAndDeleteObject`。错误文本和跨语言兼容依赖这些字符串，扩展枚举时不能仅凭 Rust variant 名称推断外部展示值。

函数不拥有 `ctx`、`cli` 或 `perms`，返回后不保留任何引用。真正的 SDK 客户端状态、临时权限检查对象和网络资源均由各 `PrefixClient` 实现管理。

## 依赖与调用关系

上游关系：

- `pkg/objstore/s3like/lib.rs` 再导出 `CheckPermissions`。
- `pkg/objstore/ossstore/store.rs::NewOSSStorage` 用 `opts.CheckPermissions` 调用它，随后才启动可选的凭证刷新器并组装 `OSSStore`。
- `pkg/objstore/s3store/store.rs::NewS3Storage` 用 `options.CheckPermissions` 调用它，随后才检查对象锁配置并构造高层存储。
- 独立测试入口包括 `pkg/objstore/s3like/permission_test.rs::test_check_permissions` 和 S3 客户端测试 `pkg/objstore/s3store/client_test.rs::test_client_permission`；crate 内迁移对齐测试还在 `pkg/objstore/s3like/migration_aster_unit_test.rs::permissions_preserve_go_order_short_circuit_and_error_context` 覆盖顺序与错误文本。

下游关系：

- 抽象边界是 `pkg/objstore/s3like/interface.rs::PrefixClient` 的四个 `Check*` 方法。
- `anyhow::{anyhow, Context, Result}` 提供未知权限错误、上下文包装和统一结果类型。
- `storeapi::{Context, Permission}` 来自 `astersql-objstore-storeapi` 路径依赖；本 crate 的 Cargo 清单没有为权限逻辑设置 feature 开关。

RustCodeGraph 对 `permission.rs::CheckPermissions` 的结果确认了 OSS/S3 构造入口和测试调用点；图中同名 Go 符号是平行实现，不是 Rust 调用链的一部分。

## 错误处理与边界

- 下游探测错误：增加包含权限名及原错误显示文本的上下文，同时保留原错误为 error chain 的下一层。`permission_test.rs` 明确断言顶层文本包含 `permission <name>: some error`，且下一层仍为 `some error`。
- 未支持权限：不调用任何 `PrefixClient` 方法，直接返回 `unknown permission: <name>`。当前 `PutObject` 分支由 Rust、Go 两侧测试共同覆盖。
- 顺序与短路：函数不汇总多个错误，也不尝试剩余权限。这使错误稳定指向输入顺序中的首个失败项，但意味着调用方一次只能看到一个缺失权限。
- 空输入：循环体不执行并返回成功；这使权限检查由调用方的 `CheckPermissions` 配置显式控制。
- 函数没有重试、超时注入、错误分类或清理逻辑；这些责任属于传入上下文、具体 `PrefixClient::Check*` 实现或外层构造函数。

## 并发与资源生命周期

`CheckPermissions` 本身不创建线程、异步任务、锁、通道或事务，也不会并行探测权限。即使 `PrefixClient: Send + Sync` 允许客户端跨线程使用，本函数仍在调用线程上逐项同步执行，因此总耗时是已执行探测耗时之和，并在首个失败处截止。

所有参数均为共享借用，生命周期只覆盖本次调用。具体网络响应、临时对象的创建/删除以及 SDK 连接的释放由各后端 `Check*` 实现负责；本文件既不能替它们清理资源，也不会在失败后补偿已经成功的前序探测。扩展实现时应继续保持这种所有权边界，避免在通用分派层引入后端特有资源状态。

## 与 Go 版本的对应关系

Rust 版本保留了 Go 的关键语义：按切片顺序遍历、四种权限到同名客户端方法的一一映射、首错短路、未知权限立即报错，以及用权限名包装底层错误。

语言层差异如下：Go 接受 `context.Context` 与 `PrefixClient` 接口值，Rust 接受 `&storeapi::Context` 与 `&dyn PrefixClient`；Go 用 `errors.Annotatef`，Rust 用 `anyhow::Context` 保留错误链；Go 权限是字符串别名并通过 `default` 接纳任意未知字符串，Rust 权限是枚举，因此当前未知分支主要覆盖已存在但不被本函数支持的 `PutObject`，并为未来新增 variant 提供拒绝路径。

Rust 测试额外检查了 `anyhow` 错误链，从而验证其确实对齐 Go `Annotatef` 的“增加上下文但保留原因”语义。

## 扩展指南

- 新增可检查权限时，应同时更新 `storeapi::Permission` 及其 `as_str()`、`PrefixClient` trait、所有真实与 mock 客户端实现、`CheckPermissions` 映射，并同步独立测试 `pkg/objstore/s3like/permission_test.rs`；Go 仍作为对照实现时也应同步 `permission.go` 和 `permission_test.go`。
- 如果新权限只能通过组合操作验证，应像 `PutAndDeleteObject` 一样把组合语义放在 `PrefixClient` 方法契约及后端实现中，通用分派层只负责选择与传播结果。
- 不应把测试嵌入 `permission.rs`。仓库约束要求 Rust 源与测试分离，最近的专用测试文件是 `permission_test.rs`，更具体的 SDK 行为应放到对应后端的独立 `client_test.rs`。
- 修改检查顺序、改为并行或改为错误聚合会改变首错、调用次数和可能产生副作用的探测顺序；必须先更新 Go 对照意图，并新增明确的顺序、短路和资源清理回归测试。
- 错误文案可能被上层包装和测试匹配；变更 `Permission::as_str` 或上下文格式时需要同时评估 OSS/S3 构造错误的兼容性。性能风险主要来自新增网络探测次数，而不是本文件的循环开销。
