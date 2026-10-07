# `pkg/expression/expropt/current_user.rs`

## 文件定位

本文件属于 `astersql-expression-expropt` crate；crate 入口 `pkg/expression/expropt/lib.rs` 通过私有模块 `current_user` 加载它，再以 `pub use current_user::*` 导出其公开类型。`pkg/expression/expropt/Cargo.toml` 的 `package.metadata.porting.go-package` 将该 crate 对应到 Go 包 `pkg/expression/expropt`，本文件的直接 Go 对照是 `pkg/expression/expropt/current_user.go`。

它位于表达式求值上下文和会话身份之间：`pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 把会话的当前认证用户和活跃角色包装成 `CurrentUserPropProvider`，注册到可选属性表；`CurrentUserPropReader` 则声明并读取 `exprctx::OptPropCurrentUser`。仓库当前非测试 Rust 代码已接线 provider，但未发现 reader 在表达式内建函数中的直接调用；reader 的直接使用目前由 `expropt` 和 `sessionexpr` 的独立测试覆盖。因而它是已接入求值上下文的类型安全属性边界，不应把 `pkg/expression/builtin_info.rs` 中基于 `SessionInfo` 的 `CURRENT_USER()`/`CURRENT_ROLE()` 格式化逻辑误认为本文件的直接下游。

## 核心职责

- `CurrentUserPropProvider` 保存一个惰性回调，在读取发生时提供一对值：`Arc<auth::UserIdentity>` 和 `Vec<Arc<auth::RoleIdentity>>`。
- `OptionalEvalPropProvider` 实现把 provider 固定标识为 `OptPropCurrentUser`，并通过 `as_any` 支持安全的运行时具体类型检查。
- `CurrentUserPropReader` 通过 `RequireOptionalEvalProps` 声明对该键的依赖，并分别暴露当前用户与活跃角色读取方法。
- 私有 `get_provider` 复用 `optional.rs::get_prop_provider`，统一处理属性缺失、描述键不一致和具体类型不匹配。

本文件只传递身份对象，不负责认证、角色展开、权限判断，也不负责把身份格式化为 SQL 函数结果。

## 主要符号

- `CurrentUserPropProvider`：公开 provider 结构体。唯一字段是装箱闭包 `Box<dyn Fn() -> (Arc<UserIdentity>, Vec<Arc<RoleIdentity>>) + Send + Sync>`；字段本身不公开。
- `CurrentUserPropProvider::new<F>`：公开泛型构造函数，要求闭包满足 `Fn + Send + Sync + 'static`。
- `CurrentUserPropProvider::call`：公开同步调用入口，原样返回闭包当次产生的用户和角色。
- `CurrentUserPropProvider::Desc`：返回静态的 `exprctx::OptPropCurrentUser.Desc()`；该描述的键在 `pkg/expression/exprctx/optional.rs` 中定义为编号 0。
- `CurrentUserPropProvider::as_any`：返回 `Some(self)`，使公共提取逻辑能够用 `Any::downcast_ref` 校验具体 provider 类型。
- `CurrentUserPropReader`：无字段的公开单元结构体，不持有用户或上下文状态。
- `CurrentUserPropReader::required_optional_eval_props`：返回仅含 `OptPropCurrentUser` 的位集合。
- `CurrentUserPropReader::{current_user, active_roles}`：公开读取入口，接受任何实现 `OptionalEvalPropContext` 的上下文。
- `CurrentUserPropReader::get_provider`：私有辅助函数，按固定键取得 `&CurrentUserPropProvider`。

本文件没有模块级常量、条件编译项或自定义错误类型。

## 执行流程

1. 会话层在 `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 中克隆会话 `Arc`，构造 `CurrentUserPropProvider::new(move || (session.current_user(), session.active_roles()))`。
2. `EvalContext::set_optional_prop` 将 provider 放进 `OptionalEvalPropProviders` 中由其 `Desc().Key()` 决定的槽位，即 `OptPropCurrentUser`。
3. 需要身份信息的组件用 `CurrentUserPropReader` 声明 `required_optional_eval_props()`；结果是该键对应的单比特集合。
4. 调用 `current_user(ctx)` 或 `active_roles(ctx)` 时，reader 先进入私有 `get_provider`，再调用 `optional.rs::get_prop_provider(ctx, OptPropCurrentUser)`。
5. 公共提取逻辑检查槽位存在、provider 自描述键一致，并把类型擦除对象安全地 downcast 为 `CurrentUserPropProvider`。
6. reader 调用 `provider.call()`。`current_user` 丢弃角色并返回用户；`active_roles` 丢弃用户并返回角色列表。两种读取是两次独立回调调用，不共享一次结果。

## 数据与状态

provider 持有闭包而不是身份快照，因此实际数据新鲜度由闭包决定。当前生产接线捕获 `Arc<SessionContext>`，每次调用时重新执行 `current_user()` 和 `active_roles()`；这允许会话状态变化反映到后续读取中。

用户与每个角色用 `Arc` 共享所有权，reader 不复制身份对象；角色容器按值返回一个 `Vec`。在当前 `NewEvalContext` 闭包中，角色列表由会话接口当次返回。`CurrentUserPropReader` 自身是零状态对象，所有可变状态均在上下文、会话或闭包捕获值中。

关键不变量是 provider 的 `Desc().Key()`、注册槽位和 reader 请求键都必须是 `OptPropCurrentUser`。`required_optional_eval_props` 也必须保持同一键，否则依赖收集与运行时读取会分离。

## 依赖与调用关系

直接依赖如下：

- `std::sync::Arc`：共享用户、角色以及生产闭包捕获的会话对象。
- `auth`：由 `lib.rs` 从 `astersql-parser-auth` 再导出的 `UserIdentity`、`RoleIdentity`。
- `exprctx`：由 `lib.rs` 从 `astersql-expression-exprctx` 再导出的属性键、描述、键集合和 provider trait。
- `optional.rs`：提供 `RequireOptionalEvalProps`、`OptionalEvalPropContext` 与 `get_prop_provider`。
- `anyhow`：reader 的可失败读取结果与公共提取错误。

已验证的关键调用边为：`CurrentUserPropReader::current_user -> get_provider -> get_prop_provider`、`current_user -> CurrentUserPropProvider::call`，以及 `active_roles` 的同构路径。上游生产接线是 `sessionexpr::NewEvalContext -> CurrentUserPropProvider::new`。RustCodeGraph 未给出精确的生产 reader caller；全仓 `rg` 也只发现 reader 在独立测试中直接调用，这限制了目前能确认的运行时消费范围。

## 错误处理与边界

`new` 和 `call` 本身不返回 `Result`；闭包签名也不表达可恢复错误。若闭包 panic，panic 会穿过本文件传播。本文件不检查用户或角色内容，不排序、去重或展开角色，也不允许用 `None` 表示没有当前用户；这些语义由会话接口和调用者负责。

reader 的显式错误全部来自 `get_prop_provider`：未注册时返回包含 `not exists in EvalContext` 的错误；自描述键与请求键不一致时报键不匹配；相同键绑定了错误具体类型时返回 `cannot cast OptionalEvalPropProvider`。`pkg/expression/expropt/migration_aster_unit_test.rs::provider_type_mismatch_is_an_error_instead_of_an_unsafe_cast` 验证最后一种情况，说明 Rust 移植避免了不安全强转。

`current_user` 和 `active_roles` 每次都会取得用户与角色二元组后舍弃其中一半。若提供者计算昂贵、依赖瞬时状态，连续调用两者可能观察到不同会话时刻；当前实现没有原子快照或缓存保证。

## 并发与资源生命周期

构造参数必须是 `Send + Sync + 'static`，所以 provider 可安全嵌入需要跨线程所有权的 trait object；`Arc` 负责身份对象的引用计数生命周期。`CurrentUserPropProvider` 没有内部锁、异步任务、通道、文件句柄或显式清理逻辑，销毁时仅按 Rust 所有权规则释放闭包和捕获值。

`Send + Sync` 只约束闭包可在线程间安全共享，不保证两次调用获得一致快照，也不替闭包捕获的业务状态加锁。新增 provider 实现若捕获可变状态，必须自行采用线程安全同步，并避免在回调中制造死锁或长时间阻塞表达式求值。

## 与 Go 版本的对应关系

Rust 与 `pkg/expression/expropt/current_user.go` 保持相同的核心协议：一个 provider 同时返回当前用户和活跃角色；provider 描述固定为 `OptPropCurrentUser`；reader 声明同一键，并通过 `CurrentUser`/`ActiveRoles` 两条路径拆分二元组；缺失 provider 返回错误。

主要语言层差异是：Go 直接把命名函数类型断言为 `CurrentUserPropProvider`，Rust 用持有装箱闭包的结构体，并通过 `as_any` 与 `downcast_ref` 安全恢复具体类型。Go 返回指针和指针切片，Rust 对应使用 `Arc<UserIdentity>` 与 `Vec<Arc<RoleIdentity>>`。Rust 还显式要求回调 `Send + Sync + 'static`，并让 reader 接受较窄的 `OptionalEvalPropContext`，便于不实现完整 `exprctx::EvalContext` 的聚焦测试。

Go 测试 `pkg/expression/expropt/optional_test.go::TestOptionalEvalPropProviders` 验证缺失错误、注册、返回值相等和键集合；Rust 的 `optional_test.rs::verify_current_user` 除同等语义外还验证 `Arc` 指针同一性，`migration_aster_unit_test.rs` 另覆盖类型错配错误。

## 扩展指南

- 若只调整身份取得方式，优先修改 `sessionexpr::NewEvalContext` 传入的闭包，不要改变属性键或 reader 契约。
- 若用户和角色必须来自同一原子快照，应新增一次读取二元组的明确 API，或让调用者复用单次 `call()` 结果；不能假设先后调用 `current_user` 与 `active_roles` 会命中同一快照。
- 若 provider 需要报告可恢复错误，必须同步设计 Rust 与 Go 的 provider 签名，并更新两侧公共提取/reader 路径；不要用 panic 代替错误协议。
- 若新增字段或身份种类，要同步 `CurrentUserPropProvider` 回调返回值、两个 reader、`sessionexpr::NewEvalContext` 接线和 Go 对照文件，并评估所有权与克隆成本。
- Rust 测试应继续放在独立文件中：核心 provider/reader 契约更新 `pkg/expression/expropt/optional_test.rs` 和 `migration_aster_unit_test.rs`；真实会话接线更新 `pkg/expression/sessionexpr/sessionctx_test.rs`。不要把测试内嵌进 `current_user.rs`。
- 兼容风险集中在键编号/描述一致性、Go/Rust 签名对齐和错误文本依赖；性能风险集中在每次 reader 调用都会执行完整闭包并构造角色 `Vec`。

## 验证依据

- 源文件：`pkg/expression/expropt/current_user.rs`，确认两个公开类型、trait 实现、读取路径及无条件编译项。
- crate 与模块：`pkg/expression/expropt/Cargo.toml`、`pkg/expression/expropt/lib.rs`，确认 crate 名、Go 包映射、依赖和再导出关系；目标目录没有 `doc.go`。
- 公共注册表：`pkg/expression/expropt/optional.rs`、`pkg/expression/exprctx/optional.rs`，确认定长槽位、安全 downcast、错误分支，以及 `OptPropCurrentUser` 为键 0。
- 生产接线：`pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext`，确认闭包从同一个会话取得当前用户和活跃角色并注册 provider。
- Rust 独立测试：`pkg/expression/expropt/optional_test.rs::verify_current_user`、`migration_aster_unit_test.rs::{registry_and_missing_reader_paths_match_go,current_user_and_ddl_owner_readers_preserve_provider_values,provider_type_mismatch_is_an_error_instead_of_an_unsafe_cast}`、`pkg/expression/sessionexpr/sessionctx_test.rs::test_session_eval_context_opt_props`。
- Go 对照与测试：`pkg/expression/expropt/current_user.go`、`pkg/expression/expropt/optional_test.go::TestOptionalEvalPropProviders`。
- RustCodeGraph：`status` 确认索引含 11,467 个文件；文件节点确认 `current_user.rs` 共 93 行；调用边确认两个 reader 均调用 `get_provider` 和 `call`，而 `get_provider` 调用 `optional.rs::get_prop_provider`。全仓 `rg` 用于补足图中歧义符号的精确 caller 核验。
- 结构验证按任务指定命令执行；本任务是纯文档分析，按计划不运行 Cargo。
