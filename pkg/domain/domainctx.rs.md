# `pkg/domain/domainctx.rs`

## 文件定位

本文件位于 `astersql-domain` crate 的上下文适配边界。`pkg/domain/lib.rs` 以公开模块 `pub mod domainctx` 暴露它；它引用同一 crate 的 `crate::domain::Domain`，没有自行创建、初始化或关闭 Domain。

它解决的是“调用方持有会话类上下文，但 Domain 可能不存在”这一接口问题。跨 keyspace 会话不归属于单个 Domain，因此这里把缺失状态作为正常返回值表达，而不是把 Domain 当成必然存在的全局对象。当前 Rust 实现只有接口和一次转发；仓库搜索未发现生产代码中的 `DomainContext` 实现或 `get_domain` 调用，现阶段属于已经定义、已有独立测试、但尚未接入 Rust 生产主链的兼容适配层。

## 核心职责

- `DomainContext` 约束上下文提供者如何暴露可选 Domain，并把具体会话类型与 `Domain` 使用方解耦。
- `get_domain` 提供与 Go `GetDomain` 同用途的统一入口，把动态 trait 对象上的查询原样转发给实现者。
- `Option<Arc<Domain>>` 同时表达两个契约：Domain 允许缺失；存在时通过原子引用计数共享所有权，而非借用上下文内部的短生命周期引用。
- 本文件不负责校验 Domain 是否已初始化、选择 keyspace、刷新 InfoSchema、处理 DDL，也不管理 Domain 的关闭流程；这些职责属于 `pkg/domain/domain.rs` 等实现文件。

## 主要符号

### `pub trait DomainContext`

这是公开 trait，只有一个必需方法：

```text
fn domain(&self) -> Option<Arc<Domain>>
```

方法只借用上下文自身，但返回一个可独立持有的 `Arc<Domain>`。trait 没有默认实现、关联类型、条件编译或额外约束；实现者必须明确决定返回克隆后的共享 Domain，还是返回 `None`。

### `pub fn get_domain`

签名为 `pub fn get_domain(ctx: &dyn DomainContext) -> Option<Arc<Domain>>`。参数使用 trait object，允许调用者在运行时传入任意 `DomainContext` 实现；函数体仅调用 `ctx.domain()`，不改变返回值、不缓存、不重试，也不把 `None` 转成错误。

### 导入

- `crate::domain::Domain`：Domain 服务容器类型，实际定义在 `pkg/domain/domain.rs` 的 `Domain` 结构体中。
- `std::sync::Arc`：为返回的 Domain 提供线程安全的共享所有权计数。

本文件没有模块级常量、结构体、枚举、宏、`impl` 块或 `cfg` 条件项。

## 执行流程

1. 上游把某个实现了 `DomainContext` 的对象以 `&dyn DomainContext` 传给 `get_domain`。
2. `get_domain` 动态分派到该对象的 `domain()` 实现。
3. 实现者返回 `Some(Arc<Domain>)` 时，调用方取得一个共享 Domain 句柄；返回 `None` 时，调用方必须把它视为合法的未绑定或跨 keyspace 状态。
4. `get_domain` 不增加任何业务分支；实际选择逻辑完全由具体 `DomainContext` 实现负责。

独立 Rust 测试 `pkg/domain/domainctx_test.rs::canonical_domain_context_preserves_absent_cross_keyspace_domain` 定义 `Detached` 上下文，其 `domain()` 返回 `None`，并确认 `get_domain(&Detached)` 仍为 `None`。这验证了缺失值不会在适配层中被替换、报错或触发 panic。

## 数据与状态

文件自身没有全局变量、缓存或可变状态。唯一经过接口的数据是 `Option<Arc<Domain>>`：

- `None`：表示上下文没有绑定单一 Domain；注释明确把跨 keyspace 会话列为典型场景。
- `Some(Arc<Domain>)`：表示可共享的 Domain 句柄。`Arc` 的引用计数变化由标准库管理，但底层 Domain 的业务状态与生命周期仍由 `pkg/domain/domain.rs` 管理。

该接口不携带 keyspace ID，也不验证返回的 Domain 是否与调用会话匹配。因此，“上下文与 Domain 绑定正确”是实现者必须维持的不变量，而不是 `get_domain` 能检查的不变量。

## 依赖与调用关系

crate 边界由 `pkg/domain/Cargo.toml` 确认：包名为 `astersql-domain`，库入口为 `lib.rs`；`domainctx.rs` 只直接使用 crate 内部 `Domain` 与标准库 `Arc`，没有引入 Cargo 中列出的其他外部 crate。

RustCodeGraph 对 `get_domain` 给出的直接下游边为：

```text
get_domain -> DomainContext::domain
```

文件级索引显示该模块会被多个 Rust 文件纳入依赖图，但对全仓 Rust 源码精确搜索后，实际符号使用仅出现在 `pkg/domain/domainctx_test.rs`；没有发现生产 `DomainContext` 实现或对此处 `get_domain` 的调用。`pkg/executor/importer/table_import_testkit_test.rs` 中的 `session::get_domain`，以及 `pkg/server/handler/*` 内同名私有函数，属于不同符号，不能作为本文件的调用者。

Go 版本的直接生产调用者包括 `pkg/domain/domain.go::init`、`pkg/domain/extract.go` 的提取流程、`pkg/domain/historical_stats.go::DumpHistoricalStats` 和 `pkg/domain/plan_replayer_dump.go` 的回放包生成流程。它们说明该适配概念在 Go 主链中用于从会话上下文取得 InfoSchema 或 Domain 服务，但不能反推 Rust 已经接入这些流程。

## 错误处理与边界

该 API 没有 `Result`，不会直接产生可传播错误。Domain 缺失由 `None` 表示，是公开契约的一部分；安全调用方应显式匹配、使用 `?` 传播 `None`，或按自身业务规则转换成有上下文的错误，不能无条件 `unwrap`。

边界限制如下：

- `get_domain` 不区分“跨 keyspace”“尚未绑定”“实现者主动隐藏”等不同缺失原因。
- 它不捕获 `DomainContext::domain` 实现内部的 panic。
- 它不检查 Domain 的可用状态，也不替调用方执行关闭或资源清理。
- trait object 没有要求 `Send` 或 `Sync`；能否跨线程传递上下文本身取决于具体实现及上层类型约束。
- 当前测试只覆盖 `None` 路径；Rust 侧尚无 `Some(Arc<Domain>)` 的本文件专属测试和生产接线证据。

## 并发与资源生命周期

返回类型选择 `Arc<Domain>`，使调用方可以在原上下文借用结束后继续持有 Domain，并支持原子引用计数意义上的跨线程共享。每次 `domain()` 是否克隆既有 `Arc` 由实现者决定；本文件没有锁、通道、异步任务或事务，也不会等待后台 worker。

`Arc` 只管理内存所有权，不等同于 Domain 业务资源自动关闭。`pkg/domain/domain.rs` 中的 `Domain::close` 才承担 Domain 运行资源的显式关闭工作；持有额外 `Arc` 还可能延长对象内存生命周期。因此新增实现时应避免引用环，并遵守 Domain 既有关闭顺序，不能把引用计数归零当作业务清理协议。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/domain/domainctx.go`。其 `GetDomain(ctx contextutil.ValueStoreContext) *Domain` 调用 `ctx.GetDomain()`，执行 `*Domain` 类型断言：断言成功返回指针，类型不匹配或空值返回 `nil`。`pkg/domain/domainctx_test.go::TestDomainCtx` 分别验证绑定 `nil` 时为 `nil`、绑定 `&Domain{}` 时非 `nil`。

Rust 版本保留“可能没有 Domain”的核心语义，但实现机制不同：

- Go 接受通用值存储，并在本函数内做运行时类型断言；Rust 把类型契约前移为 `DomainContext` trait，不需要 `Any` 下转型。
- Go 用裸指针是否为 `nil` 表达缺失；Rust 用 `Option` 强制调用方处理缺失。
- Go 返回共享指针；Rust 明确返回 `Arc<Domain>`，把共享所有权计入类型。
- Go 测试覆盖有值和无值两条路径；Rust 独立测试当前只覆盖 `None`。
- Go 函数已有多个生产调用者；Rust 适配层目前未发现生产实现者或调用者，因此只能认定接口语义已部分移植，不能认定调用链已经对齐。

Go 测试中的 `TestGetRUVersionWithoutController` 测试的是 `Domain::GetRUVersion` 默认行为，不直接覆盖 `GetDomain`，不应算作本文件的边界证据。

## 扩展指南

若要把该接口接入新的会话上下文，最可能的修改点是为该上下文类型实现 `DomainContext::domain`，并在需要可选 Domain 的入口调用 `get_domain`。实现时应：

1. 从上下文已有的 Domain 存储中克隆 `Arc`，不要制造第二个独立 Domain。
2. 对跨 keyspace 或未绑定上下文保留 `None`，不要为了方便而创建占位 Domain。
3. 在独立测试文件中同时覆盖 `Some`、`None`，并在有 keyspace 绑定逻辑时覆盖错误绑定；遵守仓库约定，不把测试嵌入 `domainctx.rs`。
4. 若试图对齐 Go 生产调用点，应逐个确认 Rust 对应流程是否已经存在，只做必要接线，不能因为本适配层简单就假设 InfoSchema、历史统计或 Plan Replayer 整条链均已移植。
5. 若需要区分缺失原因，应评估新增错误枚举或更丰富返回类型的兼容影响；直接把现有 `Option` 改成 `Result` 会破坏所有实现者和调用者。

主要风险是：把跨 keyspace 的合法 `None` 当成异常导致用户路径失败；返回错误 keyspace 的 Domain 导致元数据隔离被破坏；长期保存 `Arc` 延长资源生命周期。单次动态分派和 `Arc` 克隆通常开销有限，但高频热路径接线前仍应确认是否已有更直接的上下文访问方式。

## 验证依据

- 源文件：`pkg/domain/domainctx.rs`，确认唯一 trait、方法、转发函数、导入和注释契约。
- crate 装配：`pkg/domain/lib.rs` 的 `pub mod domainctx` 与 `pub use domain::Domain`；`pkg/domain/Cargo.toml` 的包名、库入口和依赖边界。
- Rust 测试：`pkg/domain/domainctx_test.rs::canonical_domain_context_preserves_absent_cross_keyspace_domain`。
- Go 对照：`pkg/domain/domainctx.go::GetDomain`、`pkg/domain/domainctx_test.go::TestDomainCtx`；生产调用位置由 RustCodeGraph 的 Go 调用边及源码搜索核对。
- RustCodeGraph：`status` 显示索引包含 `pkg/domain/domainctx.rs`；`node --file pkg/domain/domainctx.rs` 核对全文件；`node pkg/domain/domainctx.rs::get_domain` 确认 `get_domain -> domain`；`query DomainContext` 核对 trait、方法和测试实现；`node pkg/domain/domainctx.go::GetDomain` 核对 Go 生产调用者。
- 全仓精确搜索：`rg -n --glob '*.rs' '\b(get_domain|DomainContext)\b' pkg cmd br` 用于区分本符号与同名函数，并确认当前 Rust 生产接线缺失。
- 本任务只新增说明文档；没有运行 Cargo 或代码测试。交付结构检查要求文档存在，且恰好包含任务指定的十一个二级标题。
