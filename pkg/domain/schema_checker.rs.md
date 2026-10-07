# `pkg/domain/schema_checker.rs`

## 文件定位

本文件属于 `astersql-domain` crate；crate 根由 [`pkg/domain/Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 指定，[`pkg/domain/lib.rs`](lib.rs) 以 `pub mod schema_checker` 对外暴露本模块。它位于 SQL 会话与存储提交之间：会话根据事务起始 InfoSchema 版本和写入表集合构造检查器，存储层取得提交时间戳后调用检查器，避免事务在不兼容的 schema 变化后提交。

当前有效实现位于 [`schema_checker.rs`](schema_checker.rs) 第 120—250 行。第 23—119 行是整段注释掉的早期迁移草稿，不参与编译、符号导出或运行流程，阅读和扩展时不能把其中的 Go 风格 API 当成现有 Rust API。

## 核心职责

- 用 `SchemaChecker` 保存事务开始时的 schema 版本、涉及的物理表 ID、是否按 delta 检查，以及 Unknown 结果的重试策略。
- 用 `SchemaValidator` trait 隔离具体 InfoSchema validator；本文件只规定检查输入和三态结果，不维护 schema 历史。
- 将 validator 的 `Success`、`Fail`、`Unknown` 转换为提交路径可消费的 `Result<(), SchemaCheckError>`：成功立即放行，确定变更立即失败，暂时未知则同步退避并重试。
- 在确定变更与重试耗尽时分别递增 `SchemaLeaseErrorCounter{type="changed"}` 和 `SchemaLeaseErrorCounter{type="outdated"}`；指标未初始化时允许跳过计数。

这些职责集中在 `SchemaChecker::check_by_schema_version`；构造、适配真实 validator、挂载事务选项和实际提交均由其他模块完成。

## 主要符号

- `RelatedSchemaChange`：失败详情值对象，包含 `physical_table_ids: Vec<i64>` 与字符串化的 `action_types: Vec<String>`。它可克隆、比较，供错误携带与测试断言使用。
- `SchemaCheckResult`：validator 返回的三态枚举。`Success` 表示版本有效；`Fail(Option<RelatedSchemaChange>)` 表示确定不兼容并可选附带变更；`Unknown` 表示当前无法判断。
- `SchemaValidator: Send + Sync`：线程安全的校验抽象。`check(&self, txn_ts, schema_version, related_table_ids, check_by_delta)` 接收提交时间戳、待验证版本、相关表切片和 delta 策略。
- `SchemaCheckError`：对外错误枚举。`InfoSchemaChanged` 保留可选变更详情；`InfoSchemaExpired` 表示所有允许的尝试仍为 Unknown，或重试次数为零。
- `increment_schema_lease_error`：内部指标辅助函数。它访问 `astersql_metrics::SchemaLeaseErrorCounter` 的惰性全局句柄，句柄存在时按标签递增，否则无操作。
- `SchemaChecker`：核心状态对象。`validator` 由 `Arc<dyn SchemaValidator>` 共享；其余字段记录版本、表集合、delta 标志以及默认 `500ms × 10` 的重试配置。
- `SchemaChecker::new`：保存调用方输入并设置默认重试配置，不执行校验，也不复制 validator 内部状态。
- `SchemaChecker::with_retry`：消费并返回 `Self` 的 builder，用于测试或调优覆盖间隔和次数。
- `SchemaChecker::check`：使用构造时保存的 `schema_version`，转发到 `check_by_schema_version`。
- `SchemaChecker::check_by_schema_version`：允许调用方临时指定版本，是唯一实现三态循环、指标和错误映射的方法。

## 执行流程

1. 会话提交逻辑 [`pkg/session/runtime/control.rs`](../session/runtime/control.rs) 仅为非只读事务准备检查器；它从写键和锁表 ID 提取表号，排除零值与临时表，再排序去重。
2. [`pkg/session/runtime/schema_validation.rs`](../session/runtime/schema_validation.rs) 的 `checker` 用事务起始 `SchemaMetaVersion`、相关表集合和 MDL/delta 标志调用 `SchemaChecker::new`，再把 `checker.check(commit_ts)` 包装为 `astersql_kv::TransactionSchemaChecker` 闭包。
3. 会话通过 `transaction.SetOption(kv::SchemaChecker, ...)` 挂载闭包。真实 TiKV 适配器 [`pkg/store/driver/kv_adapter.rs`](../store/driver/kv_adapter.rs) 将它转换为 `tikv_client::SchemaLeaseChecker`，由 client-rust 提交流程传入提交时间戳；mock 存储的 [`pkg/store/mockstore/mockstorage/canonical_storage.rs`](../store/mockstore/mockstorage/canonical_storage.rs) 也在有写集的提交路径调用它。
4. `SchemaChecker::check` 把保存的版本交给 `check_by_schema_version`。后者最多循环 `retry_times` 次，每次以相同的提交时间戳、schema 版本、相关表切片和 delta 标志调用 `SchemaValidator::check`。
5. `Success` 立即返回 `Ok(())`；`Fail(change)` 记录 `changed` 指标并返回 `InfoSchemaChanged(change)`；`Unknown` 在每一次尝试后（包括最后一次）调用 `thread::sleep(retry_interval)`。
6. 循环结束仍未成功或明确失败时，记录 `outdated` 指标并返回 `InfoSchemaExpired`。当 `retry_times == 0` 时不会调用 validator，直接走这一分支。

## 数据与状态

`SchemaChecker` 在构造后没有内部可变字段；`check` 和 `check_by_schema_version` 都只借用 `&self`。`related_table_ids` 的所有权在构造时移入检查器，每次校验只向 validator 提供切片，不在重试间复制或修改。显式传给 `check_by_schema_version` 的版本优先于对象保存的 `schema_version`，而 `check` 始终使用保存值。

状态演进由外部 validator 管理。本文件既不缓存 validator 结果，也不更新 schema lease；同一组输入在不同尝试中可以从 `Unknown` 变为 `Success` 或 `Fail`。`RelatedSchemaChange` 只在 `Fail` 路径上传递，成功与过期路径不返回变更详情。

默认重试上界是 10 次、每次 Unknown 后睡眠 500ms，因此纯 Unknown 最多引入约 5 秒的本地同步等待（不含 validator 本身耗时和调度误差）。`with_retry` 可把次数设为零，也可把间隔设为零。

## 依赖与调用关系

上游主链为 `ConcreteSession` 提交逻辑 → `schema_validation::checker` → `SchemaChecker::new` / `SchemaChecker::check` → `SchemaChecker::check_by_schema_version`。`schema_validation::SharedValidator` 是生产适配器：它调用 `astersql_infoschema_isvalidator::Validator::check`，把其三态结果映射成本模块枚举，并把数值 action type 转成字符串。

下游依赖包括：

- 标准库 `Arc`：允许会话闭包与 validator 安全共享所有权；`Duration` 和 `thread::sleep`：实现同步退避。
- `astersql-metrics`：提供可选的全局 `SchemaLeaseErrorCounter`；该依赖在 [`pkg/domain/Cargo.toml`](Cargo.toml) 中以路径依赖声明。
- `astersql-kv::TransactionSchemaChecker` 与 `kv::SchemaChecker` 选项：它们不由本文件直接引用，而是由 session 适配层将本文件 API 接入存储提交 ABI。
- `tikv_client::SchemaLeaseChecker`：真实存储驱动中的最终 client-rust 接点；本文件自身不依赖具体 TiKV transaction 类型。

RustCodeGraph 显示 `check` 调用 `check_by_schema_version`，后者调用 trait 方法 `SchemaValidator::check` 和内部指标函数；直接生产构造点是 `pkg/session/runtime/schema_validation.rs`，独立单元测试构造点位于 `pkg/domain/schema_checker_test.rs`。

## 错误处理与边界

本模块没有通用字符串错误：只有可判别的 `InfoSchemaChanged` 与 `InfoSchemaExpired`。生产 session 适配层把它们分别映射到 Domain 的 `ERR_INFO_SCHEMA_CHANGED` 和 `ERR_INFO_SCHEMA_EXPIRED`，因此新增错误变体时必须同步更新该穷尽匹配。

重要边界如下：

- `Fail(None)` 仍是确定的 schema 变化错误，只是没有详细变更对象；指标照常记录。
- `Unknown` 不是立即错误；每次 Unknown 都睡眠，最后一次也不例外，这一点刻意保持 Go 行为。
- `retry_times == 0` 时 validator 完全不被调用，结果直接为 `InfoSchemaExpired`。
- 空 `related_table_ids` 被原样传给 validator，其语义由具体 validator 决定，本文件不把它改写成“跳过检查”。
- 指标句柄尚未初始化不会影响业务结果；`increment_schema_lease_error` 通过 `unsafe` 读取可变静态量，并用 `#[allow(static_mut_refs)]` 局部允许该兼容模式。
- trait 没有返回 `Result`，所以 validator 内部故障必须被实现方编码为三态之一；本文件不会传播其他底层错误。

## 并发与资源生命周期

`SchemaValidator` 要求 `Send + Sync`，并由 `Arc` 持有，因此一个检查器可以被包装进要求 `Send + Sync` 的提交闭包。`SchemaChecker` 自身只含不可变配置和共享 validator，校验时不加锁；具体 validator 若维护可变状态，必须自行实现同步。

退避使用 `std::thread::sleep`，会阻塞当前提交线程，不会创建后台任务、通道或异步计时器。检查器的生命周期由 `TransactionSchemaChecker` 闭包捕获，并随事务选项/底层提交检查器释放。`RelatedSchemaChange` 的所有权在 `Fail` 时从 validator 结果移动进错误，无额外资源清理。

全局指标读取位于短小的 `unsafe` 区域；它只在 collector 存在时递增，不负责初始化或销毁 collector。对该区域的并发安全假设来自 metrics crate 的包级惰性句柄约定，若指标实现改为别的初始化机制，应优先消除这里的 `static mut` 访问。

## 与 Go 版本的对应关系

Go 对照文件是 [`pkg/domain/schema_checker.go`](schema_checker.go)。两版核心决策一致：默认间隔 500ms、默认 10 次；将事务时间戳、schema 版本、相关表和 delta 标志传给 validator；Success 放行、Fail 记录 `changed` 并报 schema changed、Unknown 每次睡眠，耗尽后记录 `outdated` 并报 expired。

主要 API/表示差异为：

- Go 通过嵌入 `validatorapi.Validator` 实现接口复用；Rust 用 `Arc<dyn SchemaValidator>` 显式组合并要求线程安全。
- Go 的 `intSchemaVer` 实现 tikv `SchemaVer` 接口；Rust 直接使用 `i64`，不保留该包装类型。
- Go 的重试参数是可运行期修改的包级 atomic；Rust 将参数保存在每个 `SchemaChecker` 内，并通过 `with_retry` 覆盖，实例之间互不影响。
- Go 返回 `(*transaction.RelatedSchemaChange, error)`；Rust 成功返回 `()`，只在 `SchemaCheckError::InfoSchemaChanged` 内携带可选的本地 `RelatedSchemaChange`。
- Go 指标预期总是存在；Rust 兼容层允许全局 collector 尚未初始化并静默跳过计数。
- Rust 生产适配器把 Go/InfoSchema 风格的数值 action type 字符串化，因而当前错误详情不保留原始数值类型。

Go 测试 [`pkg/domain/schema_checker_test.go`](schema_checker_test.go) 还用真实 isvalidator 验证 schema 历史、相关表命中和 lease 过期语义；Rust 独立测试聚焦本文件的编排契约，没有在此文件中重复 isvalidator 的完整集成场景。

## 扩展指南

- 新增 validator 输入或检查策略时，先修改 `SchemaValidator::check` 与 `SchemaChecker` 字段/构造器，再同步生产适配器 `pkg/session/runtime/schema_validation.rs`、构造点 `pkg/session/runtime/control.rs` 和独立测试 `pkg/domain/schema_checker_test.rs`。不要把测试嵌入生产源文件。
- 新增结果或错误变体时，必须更新 `check_by_schema_version` 的穷尽分支、session 错误映射以及真实与 mock 提交路径的预期；同时评估 KV 共享错误类型的兼容性。
- 修改重试策略时，应明确是否仍保留“最后一次 Unknown 后也睡眠”的 Go 语义，并为次数为零、单次 Unknown、Unknown 后 Success/Fail 增加独立回归测试。异步化退避会改变当前同步提交契约，不能只替换 `thread::sleep`。
- 修改相关变更表示时，需要同步 `SharedValidator` 的 `phy_tbl_ids`/`action_types` 转换，并确认调用方是否需要保留原始 action 数值而非字符串。
- 修改指标时应保持 `changed`、`outdated` 标签兼容，检查 collector 初始化时序，并尽量将 `unsafe static mut` 访问收敛到 metrics crate 的安全 API。
- 性能评审应关注重试总等待时间、提交线程阻塞和大表 ID 集合；表 ID 的排序去重目前发生在 session 层，本文件不应暗中重复处理或改变顺序。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；目标文件识别出 20 个符号。通过 `node --file pkg/domain/schema_checker.rs` 阅读全部 250 行，并用 `query SchemaChecker`、`query SchemaValidator`、`callees SchemaChecker::check_by_schema_version` 核对符号与内部调用边。
- 生产调用证据：`pkg/session/runtime/control.rs:1060`—`1100` 构造并挂载事务检查器；`pkg/session/runtime/schema_validation.rs:8`—`50` 适配 validator 和错误；`pkg/store/driver/kv_adapter.rs:1326`—`1365` 在真实提交路径注册 client-rust lease checker；`pkg/store/mockstore/mockstorage/canonical_storage.rs:632`—`691` 在 mock 提交路径执行同一事务选项。
- crate 证据：`pkg/domain/Cargo.toml` 确认 crate 名、`lib.rs` 入口以及 `astersql-metrics` 路径依赖；`pkg/domain/lib.rs` 确认公开模块和独立 `schema_checker_test` 测试模块。
- Go 对照证据：`pkg/domain/schema_checker.go` 的 `NewSchemaChecker`、`Check`、`CheckBySchemaVer`、两项 atomic 重试参数和指标标签。
- Rust 测试证据：`pkg/domain/schema_checker_test.rs` 的四个测试分别验证保存版本及全部参数透传、Unknown 后重试并返回变更详情、最后一次 Unknown 后仍退避、零重试不调用 validator。Go 测试 `pkg/domain/schema_checker_test.go` 补充真实 isvalidator 的表变更与 lease 行为证据。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定的 `rg` 命令验证恰有 11 个固定二级章节，并人工复查所有“已支持”陈述都有上述源码、调用边、Cargo 或测试依据。
