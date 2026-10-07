# `pkg/expression/expropt/kvstore.rs`

## 文件定位

本文件属于 `astersql-expression-expropt` crate，是表达式“可选求值属性”机制中 KV Store 属性的专用适配层。模块入口 `pkg/expression/expropt/lib.rs` 以 `mod kvstore` 装入本文件并通过 `pub use kvstore::*` 对外再导出。它位于会话上下文与需要存储句柄的表达式之间：`pkg/expression/sessionexpr/sessionctx.rs` 在构造求值上下文时注册 `KVStorePropProvider<C::Store>`，Reader 再按 `exprctx::OptPropKVStore` 从上下文取回句柄。

本文件本身不读写键值、不发起 TiKV RPC，也没有事务逻辑；它只声明依赖键、保存一个取句柄的闭包，并完成按键和具体 Rust 类型的安全取回。当前全仓 Rust 引用搜索未发现生产代码调用 `get_kv_store`，除注册点外，显式读取均在独立测试中；Go 对照代码则已有内置时间函数和 MVCC 信息函数消费该属性。

## 核心职责

1. `KVStorePropProvider<T>` 将“如何取得当前存储句柄”封装成可注册的 `Fn() -> Arc<T>`，避免把具体存储实现耦合进 `expropt` crate。
2. `OptionalEvalPropProvider` 实现把该 Provider 固定绑定到 `exprctx::OptPropKVStore`，并通过 `as_any` 支持后续具体类型向下转换。
3. `KVStorePropReader` 通过 `RequireOptionalEvalProps` 声明使用者需要 KV Store 位；组合多个 Reader 的表达式可据此合并所需属性集合。
4. `get_kv_store` 委托公共的 `get_prop_provider` 完成缺失、键不一致和类型不匹配校验，成功后调用 Provider 并返回共享的 `Arc<T>`。

这里的泛型 `T` 仅受 `Send + Sync + 'static` 约束。与 Go 版直接约束为 `kv.Storage` 不同，Rust 文件不依赖具体 KV trait/crate；调用方以关联类型 `C::Store` 或测试类型实例化它。因此不能仅凭本文件断言 `T` 具备任何 KV 操作能力，具体能力由下游调用位置的类型约束决定。

## 主要符号

- `pub struct KVStorePropProvider<T>`：公开 Provider。内部唯一字段是私有的 `Box<dyn Fn() -> Arc<T> + Send + Sync>`；`T: Send + Sync + 'static` 使 Provider 可作为类型擦除的共享上下文属性保存。
- `KVStorePropProvider::new<F>(provider: F) -> Self`：把一个 `'static`、线程安全、可重复调用的闭包装箱。构造时不执行闭包，也不持有独立的 `Arc<T>` 字段。
- `KVStorePropProvider::call(&self) -> Arc<T>`：同步调用闭包，每次返回值由闭包决定。会话接线使用 `move || store_session.store()`，测试使用 `move || Arc::clone(&store)`。
- `impl OptionalEvalPropProvider for KVStorePropProvider<T>`：`Desc` 返回静态的 `OptPropKVStore` 描述；`as_any` 返回 `Some(self)`，供公共读取函数安全地 `downcast_ref` 到完全一致的 `KVStorePropProvider<T>`。
- `pub struct KVStorePropReader`：无字段的公开 Reader，可直接以 `KVStorePropReader` 值使用。
- `required_optional_eval_props(&self)`：返回 `OptPropKVStore.AsPropKeySet()`。在 `pkg/expression/exprctx/optional.rs` 中该键的索引为 4，因此集合只包含 KV Store 对应的一位。
- `get_kv_store<T, C>(&self, ctx: &C) -> anyhow::Result<Arc<T>>`：公开读取入口。`C` 只需实现窄接口 `OptionalEvalPropContext`，完整 `exprctx::EvalContext` 由 blanket impl 自动满足该接口。

本文件没有模块级常量、枚举、条件编译分支或本地测试模块。

## 执行流程

注册流程如下：

1. `pkg/expression/sessionexpr/sessionctx.rs` 克隆会话引用为 `store_session`。
2. 它用 `KVStorePropProvider::<C::Store>::new(move || store_session.store())` 创建 Provider。闭包捕获的会话引用使存储句柄按读取时刻取得。
3. 求值上下文的 `set_optional_prop` 接收类型擦除后的 Provider；公共注册表依据 `provider.Desc().Key()` 把它放入 `OptPropKVStore` 对应槽位。

读取流程如下：

1. 表达式或测试通过 `required_optional_eval_props` 声明 KV Store 属性需求。
2. `get_kv_store::<T, _>(ctx)` 用键 `OptPropKVStore` 调用 `get_prop_provider::<KVStorePropProvider<T>, _>`。
3. `pkg/expression/expropt/optional.rs::get_prop_provider` 先检查上下文槽位存在，再核对 Provider 自描述键，最后通过 `Any` 检查具体类型（包括泛型参数 `T`）一致。
4. 校验成功后，Reader 调用 `provider.call()`；闭包返回 `Arc<T>`，Reader 原样返回，不复制 `T`。

测试 `optional_test.rs::verify_kv_store` 同时覆盖注册前错误、注册后键集合、直接调用 Provider，以及 Reader 返回同一个 `Arc` 分配对象。`migration_aster_unit_test.rs::infoschema_and_kv_readers_choose_and_preserve_provider_objects` 再次验证对象身份保持。

## 数据与状态

持久状态只有 `KVStorePropProvider<T>::provider` 闭包。其捕获状态完全由构造者决定；本类型不缓存闭包结果，也不保证不同调用返回同一实例。当前会话接线和测试均返回已有句柄的 `Arc` 克隆，所以这些路径共享同一底层对象，但这是闭包的行为，不是 `call` 的固有保证。

`KVStorePropReader` 是零大小、无状态类型。属性键和描述位于 `pkg/expression/exprctx/optional.rs`：`OptPropKVStore` 为索引 4，描述表字符串为 `"OptPropKVStore"`。实际 Provider 存放在 `pkg/expression/expropt/optional.rs::OptionalEvalPropProviders` 的定长槽位向量中；本文件不拥有注册表。

所有权边界以 `Arc<T>` 表达：返回 Reader 后，调用者增加一个强引用；Provider 闭包以及会话捕获对象何时释放，取决于外层求值上下文和闭包的生命周期。

## 依赖与调用关系

- crate 边界：`pkg/expression/expropt/Cargo.toml` 定义 crate `astersql-expression-expropt`，`lib.rs` 为入口、`autotests = false`、`doctest = false`。本文件直接使用标准库 `Arc`、crate 内公共契约和 `anyhow::Result`；Cargo 清单通过 `exprctx-crate` 引入 `astersql-expression-exprctx`，并直接依赖 `anyhow`。
- 上游装配：`pkg/expression/expropt/lib.rs` 再导出本文件符号；`pkg/expression/sessionexpr/sessionctx.rs` 是已找到的 Rust 生产注册点，将会话的 `C::Store` 注入求值上下文。
- 下游公共逻辑：`get_kv_store` 调用 `pkg/expression/expropt/optional.rs::get_prop_provider`；Provider 的 `Desc`、Reader 的键集合分别调用 `OptPropKVStore.Desc()` 和 `OptPropKVStore.AsPropKeySet()`。
- Rust 验证调用者：`pkg/expression/expropt/optional_test.rs`、`pkg/expression/expropt/migration_aster_unit_test.rs` 和 `pkg/expression/sessionexpr/migration_aster_unit_test.rs`。
- Go 生产调用者：`pkg/expression/builtin_time.go::builtinTiDBBoundedStalenessSig` 取 Store 以取得时间戳 oracle；`pkg/expression/builtin_info.go::builtinTiDBMVCCInfoSig` 取 Store 后要求其还能转换成 `helper.Storage`，再查询编码键的 MVCC 信息。它们说明该属性为何存在，但不能证明对应 Rust 内置函数已接线。

RustCodeGraph 对 `KVStorePropProvider` 的结构节点给出了 Go 测试实例化边；对泛型 Rust 方法的 callers/callees 索引覆盖有限，因此全仓 `rg` 搜索用于补齐上述 Rust 调用点，并明确区分注册、读取和 Go 对照。

## 错误处理与边界

`get_kv_store` 自身只用 `?` 传播 `get_prop_provider` 的 `anyhow::Error`，Provider 闭包签名不返回 `Result`，因此没有可恢复的 Provider 执行错误通道。公共函数区分三类失败：请求键未注册、Provider 描述键与请求键不一致、Provider 无法向下转换成所请求的 `KVStorePropProvider<T>`。其中类型参数也是运行时类型身份的一部分；用错误的 `T` 读取会得到类型转换错误，而不是错误的 `Arc<T>`。

边界条件包括：

- 缺少 `OptPropKVStore` 时不会调用闭包；`optional_test.rs` 和 `migration_aster_unit_test.rs` 均验证错误文本包含 `not exists in EvalContext`。
- Reader 只接受实现 `OptionalEvalPropContext` 的上下文；这让聚焦测试无需实现完整 EvalContext。
- `Desc()` 依赖描述表索引有效；键表的一致性由 `exprctx::validateOptionalProperties` 负责，不由本文件重复检查。
- 闭包若 panic，`call` 不捕获 panic；闭包若阻塞，Reader 也同步阻塞。
- 本文件不验证 `T` 是否为真正的 KV 存储，也不检查其支持 MVCC、oracle 或事务；这些能力必须在消费端收窄。

## 并发与资源生命周期

Provider 和具体类型都要求 `Send + Sync + 'static`，闭包 trait object 同样要求 `Send + Sync`。这允许它被放入可跨线程使用的求值上下文对象，但本文件没有创建线程、任务、锁、通道或异步 Future，也没有提供额外同步。

资源共享使用 `Arc`：调用 `call` 通常只增加引用计数，不克隆底层 Store。会话注册闭包捕获一个 `Arc` 会话对象，因此只要 Provider 存活，该会话捕获对象就不会释放；每个返回的 `Arc<T>` 则独立延长 Store 的生命周期。是否存在引用环取决于会话和 Store 的外部结构，本文件没有打破环的 `Weak` 策略。

闭包为 `Fn` 而非 `FnMut`/`FnOnce`，所以必须可重复、共享调用；如扩展实现需要内部可变状态，必须由捕获对象自行提供线程安全的内部可变性，并评估并发调用的开销和锁竞争。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/expropt/kvstore.go`：

- Go `type KVStorePropProvider func() kv.Storage` 对应 Rust 的泛型结构体加装箱闭包。两者都延迟取得 Store；Rust 用 `Arc<T>` 明确共享所有权，并用 `T` 保留具体类型。
- Go `Desc` 与 Rust `Desc` 都返回 `exprctx.OptPropKVStore.Desc()`。
- Go `KVStorePropReader{}` 与 Rust 单元结构体均无状态；两者所需属性集合都只有 `OptPropKVStore`。
- Go `GetKVStore` 与 Rust `get_kv_store` 都先通过公共 helper 取 Provider，再调用它。Go 返回接口 `kv.Storage`，Rust 返回 `Arc<T>`；Go 用运行时接口断言，Rust 公共 helper 用 `Any` 向下转换。
- Go `optional_test.go` 验证缺失时报错、Provider 和 Reader 返回同一 mock Store；Rust 的 `optional_test.rs::verify_kv_store` 保留相同意图并用 `Arc::ptr_eq` 验证对象身份。

迁移差异是 Rust 没有在本文件中表达 Go `kv.Storage` 接口约束，也没有生产读取调用点证据；这使适配层可复用，但把能力校验留给消费者。另一个已存在的 Go 注释称 `GetKVStore returns a SequenceOperator`，与函数实际返回 `kv.Storage` 不符，应视为上游注释笔误，而不是 Rust 行为依据。

## 扩展指南

- 若只新增一个需要 Store 的 Rust 表达式，应组合/持有 `KVStorePropReader`，把 `required_optional_eval_props()` 的结果并入表达式属性集合，并在执行入口调用 `get_kv_store::<实际 Store 类型, _>`；不要绕过公共 helper 直接访问注册表。
- 若需要 Store 的新能力，应优先在消费端对 `T` 添加最小 trait 约束。直接给本文件的 `T` 加全局 KV trait 约束会改变所有注册者和测试的类型边界，必须先确认 Go 语义和 crate 依赖方向。
- 若取 Store 过程可能失败，需要有意修改 Provider 闭包签名与 `call` 返回类型，并同步 Go 语义、所有构造点、公共错误传播和独立测试；不能仅在 Reader 中吞掉错误。
- 若增加新的可选属性，不应复用 `OptPropKVStore`；应在 `exprctx` 描述表、计数、注册表和相应 Provider/Reader 中建立新键，并验证位集合布局。
- 测试必须继续放在独立文件。KV Store 适配层修改至少同步 `pkg/expression/expropt/optional_test.rs`；会话接线修改同步 `pkg/expression/sessionexpr/migration_aster_unit_test.rs`；Go 对齐语义变化还应核对 `pkg/expression/expropt/optional_test.go`。重点覆盖缺失 Provider、错误具体类型、对象身份、重复调用语义，以及闭包捕获资源的生命周期。
- 兼容风险主要是属性键/描述不一致与泛型类型不匹配；性能风险主要是每次读取的动态查找、`Any` 向下转换、闭包调用及 `Arc` 原子引用计数。当前逻辑没有 Store I/O，不能把下游 I/O 成本归因于本文件。

## 验证依据

- 目标源码：`pkg/expression/expropt/kvstore.rs`，核对 `KVStorePropProvider`、`new`、`call`、`OptionalEvalPropProvider` 实现、`KVStorePropReader`、`required_optional_eval_props`、`get_kv_store` 的完整定义。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/expression/expropt` 确认 Rust/Go 对照文件与独立测试均已索引；`explore "pkg/expression/expropt/kvstore.rs KVStorePropProvider KVStorePropReader get_kv_store"` 和 `node KVStorePropProvider` 核对源定义及已识别引用。
- 模块与公共契约：`pkg/expression/expropt/lib.rs`、`pkg/expression/expropt/optional.rs`、`pkg/expression/exprctx/optional.rs`。
- crate 清单：`pkg/expression/expropt/Cargo.toml`。
- Rust 生产接线：`pkg/expression/sessionexpr/sessionctx.rs` 第 410—413 行附近。
- Rust 独立测试：`pkg/expression/expropt/optional_test.rs::verify_kv_store`、`pkg/expression/expropt/migration_aster_unit_test.rs::registry_and_missing_reader_paths_match_go`、`infoschema_and_kv_readers_choose_and_preserve_provider_objects`，以及 `pkg/expression/sessionexpr/migration_aster_unit_test.rs` 的完整会话属性读取验证。
- Go 对照与测试：`pkg/expression/expropt/kvstore.go`、`pkg/expression/expropt/optional_test.go`；生产消费证据为 `pkg/expression/builtin_time.go::builtinTiDBBoundedStalenessSig` 和 `pkg/expression/builtin_info.go::builtinTiDBMVCCInfoSig`。
- 全仓引用核验：`rg -n "get_kv_store|KVStorePropProvider|KVStorePropReader|OptPropKVStore" --glob '*.rs' --glob '*.go'`，用于补足 RustCodeGraph 对泛型方法调用边的索引限制。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认本文恰有十一个固定二级章节，并人工复核没有把 Go 生产调用误写成 Rust 已接线事实。
