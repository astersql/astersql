# `pkg/ddl/options.rs`

## 文件定位

`pkg/ddl/options.rs` 是 `astersql-ddl` crate 的构造参数门面：它不执行 DDL job，也不参与 owner 调度、schema 状态迁移、reorg/backfill 或元数据持久化，而是把创建 DDL 实例所需的依赖和参数收集为 `Options`。crate 根模块在 `pkg/ddl/lib.rs` 中以 `pub mod options` 暴露该模块；`pkg/ddl/Cargo.toml` 声明 crate 名为 `astersql-ddl`、库入口为 `lib.rs`，并通过 `ddl-systable = { package = "astersql-ddl-systable", path = "systable" }` 提供本文件使用的 `SchemaLoader` trait。

生产侧直接消费点是 `pkg/ddl/ddl.rs::Ddl::new`：构造函数接收 `IntoIterator<Item = OptionFn>`，调用 `apply_options` 后把结果保存到 `Ddl::options`。随后 `Ddl::start` 会检查 `options.store` 是否存在；本文件本身不启动任务、不打开连接，也不验证各字符串标识是否对应真实资源。

## 核心职责

1. 用 `Options` 集中保存 DDL 构造期的七项配置：etcd 客户端标识、底层存储标识、auto-ID 客户端标识、info cache 标识、schema lease、schema loader 和事件发布存储标识（`pkg/ddl/options.rs::Options`）。
2. 用 `OptionFn = Box<dyn FnOnce(&mut Options) + Send>` 实现 functional options：每个 `with_*` 函数捕获一个值，并返回一次性修改 `Options` 的闭包。
3. 由 `apply_options` 从 `Options::default()` 开始，按调用方提供的迭代顺序执行闭包，形成最终配置。
4. 为构造器提供依赖注入边界，尤其允许把 `Arc<dyn SchemaLoader>` 原样交给 DDL 实例；相关契约由 `pkg/ddl/schema_loader_contract_aster_unit_test.rs` 验证。

当前实现仍是部分迁移状态：除 `schema_loader` 外，多数 Go 侧具体依赖在 Rust 中只是 `Option<String>` 标识。它们能表达“是否设置”和覆盖顺序，但不能提供 Go 具体客户端/存储接口的运行能力。

## 主要符号

- `pub struct Options`：构造配置快照，派生 `Clone` 与 `Default`。六个可选依赖默认是 `None`，`lease: Duration` 默认是零时长；`schema_loader` 使用 `Option<Arc<dyn SchemaLoader>>`，其克隆只增加共享所有权计数。
- `pub type OptionFn = Box<dyn FnOnce(&mut Options) + Send>`：一次性、拥有捕获值的配置闭包。`FnOnce` 允许把 `String` 或 `Arc` 移入目标字段；`Send` 允许闭包在线程边界间转移，但类型没有 `Sync` 约束，也没有被并行执行的语义。
- `with_etcd_client`、`with_store`、`with_info_cache`、`with_auto_id_client`、`with_event_publish_store`：接收 `impl Into<String> + Send + 'static`，先转换成拥有所有权的 `String`，再把它写成对应字段的 `Some(value)`。
- `with_lease(Duration)`：把 lease 直接写入 `Options::lease`，不限制零值或上限。
- `with_schema_loader(Arc<dyn SchemaLoader>)`：把 trait object 写入 `Options::schema_loader`，不调用 `reload`，也不复制 loader 的内部状态。
- `apply_options(impl IntoIterator<Item = OptionFn>) -> Options`：本文件唯一的聚合入口；无选项时返回默认配置，同一字段被多次设置时最后执行的闭包生效。

全部符号均为公开 API；文件内没有模块级常量、额外 impl、条件编译项或私有辅助函数。

## 执行流程

典型构造流程如下：

1. 调用方通过若干 `with_*` 函数创建 `OptionFn`。字符串类构造器在此时完成 `Into<String>` 转换，并由闭包取得值的所有权。
2. `pkg/ddl/ddl.rs::Ddl::new` 接收这些闭包，把它们传给 `apply_options`。
3. `apply_options` 创建 `Options::default()`，随后按迭代器顺序逐个以 `&mut Options` 调用闭包。
4. 闭包消费自身并更新一个字段；若多个闭包更新同一字段，后执行者覆盖前值。`pkg/ddl/options_test.rs::options_are_applied_in_order` 用两个 `with_store` 验证结果为 `last-store`。
5. 完成后的 `Options` 被放入 `Ddl::options`。`pkg/ddl/ddl.rs::Ddl::start` 在首次启动前要求 `store.is_some()`，缺失时返回 `DDL store is not configured`；其他字段没有在该构造/启动路径中被强制校验。

`pkg/ddl/options_test.rs::test_options` 还验证了七个构造器可以组合，且 `schema_loader` 注入后仍与输入 `Arc` 指向同一个对象。

## 数据与状态

`Options` 是普通内存值，不关联事务或持久化记录。它的状态变化只发生在构造阶段对局部 `result` 的独占可变借用上：

- `etcd_client`、`store`、`auto_id_client`、`info_cache`、`event_publish_store` 是可选的拥有型字符串。
- `lease` 是 `std::time::Duration`，默认零值；本文件接受包括零在内的任意 `Duration`。
- `schema_loader` 是可选的原子引用计数 trait object。`Options::clone()` 会克隆 `Arc`，不会深拷贝 loader。
- `Default` 不表达“可运行”：默认 `store` 为 `None`，因此默认构造出的配置不能通过当前 `Ddl::start` 的存储检查。
- `apply_options` 不做合并、去重或来源追踪；覆盖后的旧字符串或旧 `Arc` 按 Rust 所有权规则立即释放/递减引用计数。

这些配置不包含 DDL job、schema version、状态机阶段或 system table 数据，因此不会直接改变集群元数据。

## 依赖与调用关系

上游关系：

- `pkg/ddl/ddl.rs::Ddl::new` 是 RustCodeGraph 找到的生产调用者，调用 `apply_options` 并持有返回的 `Options`。
- `pkg/ddl/options_test.rs::{test_options, options_are_applied_in_order}` 直接调用所有构造器与 `apply_options`。
- `pkg/ddl/schema_loader_contract_aster_unit_test.rs::schema_loader_mock_is_injectable_through_production_options` 调用 `with_schema_loader`，再经保存的 trait object 调用 `SchemaLoader::reload`。

下游关系：

- 本文件仅依赖标准库的 `Arc`、`Duration`，以及 `ddl_systable::SchemaLoader`。
- RustCodeGraph 的 `callees` 查询显示各 `with_*` 与 `pkg/ddl/options.rs::apply_options` 没有静态下游函数调用；其行为是字段赋值和顺序执行闭包。
- `pkg/ddl/ddl.rs::Ddl::start` 读取 `options.store` 并实施当前唯一明确的启动前置条件。

RustCodeGraph 对 `pkg/ddl/options.rs` 识别出 10 个符号，并显示该文件被 `pkg/ddl/ddl.rs`、`pkg/ddl/options_test.rs`、`pkg/ddl/schema_loader_contract_aster_unit_test.rs` 等文件使用。图查询对同名通用符号可能混入其他模块候选，因此本文只采用路径能定位到 `pkg/ddl` 的边。

## 错误处理与边界

本文件的 API 不返回 `Result`，也没有显式 panic、日志或恢复逻辑。边界行为如下：

- 空迭代器合法，结果是 `Options::default()`；可否运行由后续消费者决定。
- 重复设置合法且不会报错，严格遵循“最后一个闭包获胜”。
- 字符串构造器不拒绝空字符串，也不验证地址、资源名或连接可用性。
- `with_lease` 不拒绝零时长；是否有业务意义应由使用 lease 的下游负责。
- `with_schema_loader` 的参数不是 `Option`，因此调用构造器时必须提供一个 `Arc`；但调用方也可以完全不提供该选项，使最终字段保持 `None`。
- 选项闭包如果由未来扩展自行 panic，`apply_options` 不捕获 unwind，且可能只完成部分字段更新；当前七个内建闭包只有赋值操作。
- 当前可观察的生产校验在 `Ddl::start`：`store` 缺失会返回字符串错误，其余缺失字段不会在该路径报错。这不等于它们在完整目标架构中永远可选。

## 并发与资源生命周期

`apply_options` 串行执行闭包，并通过唯一的 `&mut Options` 修改局部值；本文件没有锁、通道、异步任务、线程创建或共享可变全局状态。`OptionFn: Send` 只保证未执行的闭包可转移到另一线程，不保证多个闭包并发执行，也不使 `Options` 自动具备额外的并发协议。

字符串值在创建选项时转为拥有型数据，并在闭包执行时移动进 `Options`；闭包未执行就被丢弃时，捕获值随闭包释放。`schema_loader` 由 `Arc` 管理共享生命周期：闭包先拥有传入的 `Arc`，执行后所有权进入 `Options`；克隆 `Options` 或调用方预先克隆 `Arc` 都只调整引用计数。trait 本身的线程安全上限由 `ddl_systable::SchemaLoader` 定义，本文件没有额外的 `Send + Sync` 标注或同步包装。

本文件不会创建或关闭 etcd/KV 客户端；当前这些字段只是字符串。Go 测试会显式关闭真实 etcd client，而 Rust 测试没有相应资源清理，是类型迁移差异而非资源泄漏证据。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/options.go`，测试对照是 `pkg/ddl/options_test.go::TestOptions`。

共同点：

- Go 的 `Option func(*Options)` 与 Rust 的 `OptionFn = Box<dyn FnOnce(&mut Options) + Send>` 都用闭包修改配置对象。
- `WithEtcdClient`、`WithStore`、`WithInfoCache`、`WithAutoIDClient`、`WithLease`、`WithSchemaLoader`、`WithEventPublishStore` 在 Rust 中都有命名和字段语义对应项。
- Go `TestOptions` 逐项应用选项并验证 etcd、lease、store、info cache；Rust `test_options` 保留这些意图，并额外覆盖 auto-ID、schema loader 与 event publish store。

当前差异：

- Go 字段使用真实类型（例如 `*clientv3.Client`、`kv.Storage`、`*infoschema.InfoCache`、`notifier.Store`）；Rust 除 `Arc<dyn SchemaLoader>` 与 `Duration` 外均用 `Option<String>`，所以是标识级占位，并非等价的运行时依赖移植。
- Go `Options` 还有 `ExtWorkloadMgr extworkload.Manager` 及 `WithExternalWorkloadManager`；当前 Rust 文件没有对应字段或构造器。
- Go 由调用方先创建 `&Options{}` 再循环应用；Rust 将默认创建与循环封装进 `apply_options`。
- Go `Option` 可重复调用；Rust 使用 `FnOnce`，每个具体 `OptionFn` 只能消费一次。两者对正常的一次构造流程结果一致，但复用单个选项闭包的能力不同。
- Rust 增加 `Send + 'static`（字符串输入）约束以拥有捕获值并允许跨线程转移；Go API 没有对应的类型系统约束。

因此，扩展时应以 Go 文件校对字段集合和语义，但不能宣称当前 Rust 已具备所有 Go 依赖的真实行为。

## 扩展指南

新增构造选项时应保持以下接入顺序：

1. 在 `Options` 添加字段，并明确 `Default` 是否代表有效缺省值；如为真实共享服务，优先使用已有 trait/客户端类型，而不是继续扩大字符串占位。
2. 添加对应 `with_*` 构造器；需要移动非 `Copy` 值时保持 `FnOnce` 所有权语义，只有确有跨线程需求时才改变 `Send` 边界。
3. 在实际消费者（通常是 `pkg/ddl/ddl.rs::Ddl::new`、`Ddl::start` 或后续装配层）接线。只增加字段与构造器而无人读取，不能算完成功能移植。
4. 在独立测试 `pkg/ddl/options_test.rs` 增加字段赋值、默认值、重复设置顺序和必要边界用例；不要把 Rust 测试嵌入生产源文件。若涉及 loader trait object，同步维护 `pkg/ddl/schema_loader_contract_aster_unit_test.rs`。
5. 与 `pkg/ddl/options.go` 对照；若移植 `WithExternalWorkloadManager`，还需确认 Rust 中真实 manager 接口、所有权与关闭责任，不能仅复制字段名。

兼容风险主要是公开 `Options` 字段类型变化和默认值变化；正确性风险是“选项已存入但未被消费者使用”；性能风险主要来自把重型依赖深拷贝或在选项应用期执行 I/O。当前实现仅移动字符串/`Duration`/`Arc`，应用开销与选项数量线性相关，且不执行 I/O。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件；`files --filter pkg/ddl/options.rs` 确认目标文件已索引并含 10 个符号。
- RustCodeGraph：`node --file pkg/ddl/options.rs --offset 1 --limit 400` 核对了 `Options`、`OptionFn`、七个 `with_*` 和 `apply_options` 的完整实现。
- RustCodeGraph：对 `apply_options` 及各 `with_*` 执行 `query`、`callers`、`callees`，并用 `explore`/`node` 消除同名符号歧义；确认 `apply_options` 的生产调用点为 `pkg/ddl/ddl.rs::Ddl::new`，各内建选项构造器无下游函数调用，测试调用点包括 `options_test.rs` 与 schema loader 契约测试。
- RustCodeGraph：读取 `pkg/ddl/ddl.rs` 的 `Ddl::new`/`Ddl::start`，确认配置保存和 `store` 启动校验；读取 `pkg/ddl/lib.rs`，确认 `pub mod options` 与独立测试模块 `mod options_test`。
- 源码/配置：读取 `pkg/ddl/Cargo.toml`，确认 crate 名、`lib.rs` 入口和 `ddl-systable` 路径依赖。
- Go 对照：读取 `pkg/ddl/options.go` 和 `pkg/ddl/options_test.go`，核对字段、functional-option 语义、真实依赖类型以及 Rust 尚缺的 external workload manager。
- Rust 测试：读取 `pkg/ddl/options_test.rs` 和 `pkg/ddl/schema_loader_contract_aster_unit_test.rs`，核对组合赋值、后写覆盖、`Arc::ptr_eq` 与 trait 调用契约。
- 本任务是纯文档分析，按任务约束未运行 Cargo；交付验证仅检查固定章节、文件范围和事实可追溯性。
