# `pkg/session/runtime/schema_validation.rs`

## 文件定位

本文件是 `astersql-session` crate 内部的事务提交期 schema 校验适配层。模块由 `pkg/session/runtime.rs` 以私有 `mod schema_validation` 装入，两个符号都只有 `pub(super)` 可见性，不构成 crate 的公开 API。它位于 session 提交流程、`astersql-domain` 的通用 `SchemaChecker`、`astersql-infoschema-isvalidator::Validator` 与 `astersql-kv::TransactionSchemaChecker` 之间，职责是把几套不同的数据和错误接口接成存储层可在 commit timestamp 上调用的闭包。

直接入口位于 `pkg/session/runtime/control.rs` 的提交路径：仅当事务不是只读、session 持有 `schema_validator` 且保留了事务起始 `InfoSchema` 时，才调用本文件的 `checker`，随后以 `kv::SchemaChecker` 事务选项安装到 transaction。表 ID 在进入本文件前已经从写 key 与加锁表收集、排除 ID 0 和临时表、排序并去重；本文件不重复承担这些策略。

## 核心职责

1. `SharedValidator::check` 将 InfoSchema validator 的 `(Option<RelatedSchemaChange>, Result)` 适配成 domain 层 `SchemaCheckResult` 三态：`ResultSucc`、`ResultUnknown`、`ResultFail` 分别对应 `Success`、`Unknown`、`Fail`。
2. 对失败携带的变更详情做类型转换：`phy_tbl_ids` 原样转成 `physical_table_ids`，数值型 `action_types` 逐项 `to_string()` 后交给 domain 层。
3. `checker` 固化事务开始时的 schema version、相关物理表 ID 和 delta 检查开关，构造 domain `SchemaChecker`，再包装为存储层认识的 `TransactionSchemaChecker`。
4. 将 domain 的内部错误稳定映射为 SQL/domain 标准错误：`InfoSchemaChanged` 生成 `ERR_INFO_SCHEMA_CHANGED`，`InfoSchemaExpired` 生成 `ERR_INFO_SCHEMA_EXPIRED`。变更详情参与 domain 判定，但当前 KV 闭包的成功/错误接口不向上传回详情。

本文件不决定哪些表“相关”、不维护 schema lease/delta 队列、不实现 Unknown 重试策略，也不分配 commit timestamp；这些分别属于 `runtime/control.rs`、`infoschema/isvalidator/validator.rs`、`domain/schema_checker.rs` 和具体存储事务实现。

## 主要符号

- `SharedValidator(pub Arc<validator::Validator>)`：私有元组结构体，持有共享 InfoSchema validator。它实现 `astersql_domain::schema_checker::SchemaValidator`，借此把具体 validator 注入通用 domain checker。`Arc` 允许 session、domain checker 和提交闭包共享同一实例。
- `impl SchemaValidator for SharedValidator::check(&self, ts, version, tables, delta)`：把 `tables: &[i64]` 作为 `Some(tables)` 传给底层 `Validator::check`。这里刻意不会产生 Go/Rust validator 中表示“只按版本检查”的 `None`；空切片仍是 `Some(&[])`，保留“事务没有相关普通表”的独立语义。
- `checker(validator, version, tables, delta) -> astersql_kv::TransactionSchemaChecker`：本文件唯一工厂入口。它先创建 `astersql_domain::schema_checker::SchemaChecker`，再返回一个捕获该 checker 的 `Arc<dyn Fn(u64) -> Result<_, _> + Send + Sync>` 新类型。闭包参数 `ts` 是存储提交路径给出的 commit timestamp。

文件没有模块级常量、枚举、trait 定义、条件编译项或内嵌测试。

## 执行流程

1. `pkg/session/runtime/control.rs` 准备提交。对写事务，它从写 key 解码表 ID，并合并悲观锁涉及的表 ID；ID 0、临时表被过滤，最终列表排序去重。schema 版本取自事务起始 `InfoSchema::SchemaMetaVersion()`；`delta` 取 `!TxnCtx.noNeedToRestore.EnableMDL`。
2. `control.rs` 调用 `schema_validation::checker`。工厂将具体 `validator::Validator` 包成 `SharedValidator`，再用 `SchemaChecker::new` 保存版本、表 ID 与 delta 开关。
3. 返回的 `TransactionSchemaChecker` 被写入 transaction 的 `kv::SchemaChecker` 选项。`pkg/store/mockstore/mockstorage/canonical_storage.rs` 会在真正分配的提交时间戳上执行该闭包；`pkg/store/driver/kv_adapter.rs` 则把它接到 `tikv_client::SchemaLeaseChecker`，由真实 TiKV commit 流程调用。
4. 闭包调用 domain `SchemaChecker::check(ts)`。该层把构造时保存的参数转发给 `SharedValidator::check`；若底层返回 `Unknown`，按默认 500 ms、最多 10 次同步退避重试；成功立即返回，明确失败立即结束，重试耗尽则判定 schema expired。
5. `SharedValidator` 调用 InfoSchema `Validator::check(ts, version, Some(tables), delta)`。底层在 validator 停止、lease 过期时返回 `Unknown`；版本早于 restart 下界或相关表 delta 已改变时返回 `Fail`；可证明安全时返回 `Succ`。
6. domain 成功结果成为 `Ok(())`。明确变更与过期分别转为标准 domain 错误，存储提交由此接受或拒绝事务。mockstore 的相关实现保证校验使用最终发布的 commit timestamp；校验拒绝时写入仍未提交。

## 数据与状态

- `version: i64`：事务开始时观察到的 schema 元版本，由 factory 捕获后保持不变。
- `tables: Vec<i64>`：事务涉及的普通物理表/分区 ID。所有权移入 `SchemaChecker`；筛选、排序、去重发生在调用方，而非本文件。
- `delta: bool`：是否必须基于 schema delta 判断相关表变化。session 调用方以事务启动时的 MDL 状态推导它；底层 validator 还会读取当前全局 MDL 状态，以覆盖事务期间开关切换。
- `ts: u64`：存储层在提交阶段传入的时间戳。它不是 schema version，主要用于判断本地 schema lease 能否覆盖该提交时刻。
- `RelatedSchemaChange`：适配器保留物理表 ID，并将底层 `u64` action type 转成十进制字符串。当前 `validator.rs::check` 的已实现失败分支均返回 `None` 详情，因此转换路径主要是兼容 domain 接口和未来/替代 validator 行为，文档不假定当前提交错误一定携带详情。

本文件自身没有可变全局状态或缓存。持久状态位于 InfoSchema validator 的锁保护状态中；重试参数、指标计数和保存的事务参数位于 domain `SchemaChecker`。

## 依赖与调用关系

上游：

- `pkg/session/runtime.rs` 声明私有模块。
- `pkg/session/runtime/control.rs` 是唯一生产调用点，负责构造参数并将结果安装为 `kv::SchemaChecker`。
- 间接运行者包括 `pkg/store/mockstore/mockstorage/canonical_storage.rs` 与 `pkg/store/driver/kv_adapter.rs`，它们在提交阶段从事务选项取出 `TransactionSchemaChecker` 并调用。

下游：

- `astersql-infoschema-isvalidator`（`pkg/infoschema/isvalidator/validator.rs`）提供共享 `Validator`、三态 `Result` 和底层 schema lease/delta 判定。
- `astersql-domain`（`pkg/domain/schema_checker.rs`）提供 `SchemaValidator` trait、参数持有、Unknown 重试、指标和 `SchemaCheckError`。
- `astersql-kv`（`pkg/kv/option.rs`）提供提交闭包新类型与 `SchemaChecker` option ID。
- `astersql-domain::domain::{ERR_INFO_SCHEMA_CHANGED, ERR_INFO_SCHEMA_EXPIRED}` 提供对外兼容错误。

`pkg/session/Cargo.toml` 直接声明了上述 `astersql-infoschema-isvalidator`、`astersql-domain`、`astersql-kv` 路径依赖；`nextgen` feature 没有改变本文件编译内容，本文件也没有自己的 feature gate。

## 错误处理与边界

- `ResultSucc` 无错误返回；`ResultFail` 不重试，映射为 schema changed；`ResultUnknown` 由 domain checker 重试，耗尽后映射为 schema expired。
- `SchemaCheckError::InfoSchemaChanged(_)` 中的可选详情在错误翻译时被有意丢弃，只生成 `ERR_INFO_SCHEMA_CHANGED.FastGenByArgs(&[])`；调用者看到稳定的 domain 错误码/文本，而不是适配层私有错误。
- `SharedValidator` 总传 `Some(tables)`。因此空列表表示“已知没有相关普通表”，不同于底层 API 的 `None`（任何版本前进都失败）。维护时不能把空 slice 简化为 `None`。
- validator 停止、commit timestamp 超出 lease 覆盖范围并非立即视为变更，而是 Unknown；同步重试会阻塞当前提交线程，最坏默认等待约 5 秒后返回 expired。
- 工厂不验证表 ID、版本或 timestamp 的取值，也不捕获 panic；这些值的合法性依赖调用方和下游契约。
- session 只为非只读事务且具备 validator/start schema 时安装 checker；mockstore 的 embedded-RPC 分支还只在存在写入时调用它。这些边界不由本文件控制。

## 并发与资源生命周期

`SharedValidator` 通过 `Arc` 共享底层 validator；domain trait 要求 `Send + Sync`，`TransactionSchemaChecker` 的闭包也要求 `Send + Sync`，因而可跨 session/storage 提交边界安全持有。底层 `Validator::check` 取得内部状态的读锁，在一次判定中观察一致快照；本文件不持有额外锁，也不启动线程、任务或通道。

`checker` 创建后拥有 `SchemaChecker` 及其 `Vec<i64>`。transaction option 持有闭包的 `Arc`，真实 driver 在 commit 前克隆 checker 并交给 TiKV client；当 transaction、client checker 及其他克隆全部释放时，捕获状态和 validator 引用自然释放。domain 的 Unknown 退避使用 `std::thread::sleep`，不是异步 timer；扩展时需要评估提交线程阻塞与重试上限，不能误认为它会让出 async runtime。

## 与 Go 版本的对应关系

- Go 的通用逻辑位于 `pkg/domain/schema_checker.go`：`NewSchemaChecker` 保存 validator、schema version、相关表与 delta 标志；`CheckBySchemaVer` 对 `ResultUnknown` 以 500 ms/10 次退避，对 Fail/耗尽分别返回 `ErrInfoSchemaChanged`/`ErrInfoSchemaExpired`。Rust 将这部分放在 `pkg/domain/schema_checker.rs`，本文件只负责类型与错误桥接。
- Go 的提交接线位于 `pkg/sessiontxn/isolation/base.go::SetOptionsBeforeCommit`：排除临时表后，将 `domain.NewSchemaChecker` 设为 `kv.SchemaChecker`。Rust 的对应接线当前位于 `pkg/session/runtime/control.rs`，同样排除临时表并根据 MDL 决定 delta，但 Rust 还从写 key 与 locking table IDs 构造列表。
- Go validator 的 `pkg/infoschema/isvalidator/validator.go::Check` 与 Rust `validator.rs::check` 保留相同关键分支：停止/lease 超界为 Unknown，restart 之前的版本为 Fail，schema 前进时区分 nil、空列表与相关表 delta，安全时 Succ。
- Go `transaction.RelatedSchemaChange` 的 action type 是数值；Rust domain 的通用结构选择字符串，因此本文件执行 `to_string()`。这是显式表示层差异，不应误写为丢弃 action type。
- `pkg/session/test/session_test.go::TestSchemaCheckerSQL` 覆盖相关表/分区变更拒绝、无关表变更允许；Rust 同路径 `session_test.rs` 中该大段测试目前仅为注释迁移材料，不能算可执行覆盖。可执行 Rust 会话级证据来自 `pkg/session/test/temporarytabletest/temporary_table_test.rs::global_temporary_table_schema_change_keeps_transaction_usable`，它验证临时表变化可提交，而混入普通表变化时 optimistic/pessimistic commit 都返回 `[domain:8028]`。

## 扩展指南

- 若新增底层 validator 结果种类或变更详情字段，首先修改 `SharedValidator::check` 的穷尽映射，并在独立测试文件中验证每一种映射；不要在本生产文件内嵌 `#[cfg(test)]` 测试。
- 若改变提交期错误契约，修改 `checker` 闭包中的 `SchemaCheckError` 映射，同时核对 domain 错误码、真实 driver 的原始错误保留和 mockstore 的拒绝提交语义。兼容风险主要是客户端依赖的 8028/8027 错误分类。
- 若改变表收集或临时表策略，应修改上游 `runtime/control.rs`，不是把过滤逻辑塞进本适配器；同步扩展 `pkg/session/test/temporarytabletest/temporary_table_test.rs`，并对照 Go 的 `TestSchemaCheckerSQL`/`TestSchemaCheckerTempTable`。
- 若调整 Unknown 重试、sleep 或指标，应修改 `pkg/domain/schema_checker.rs` 并扩展 `pkg/domain/schema_checker_test.rs`；该变化会影响提交延迟和线程占用，属于性能与可用性敏感项。
- 若改变 commit timestamp 的取得/调用时机，应在 `pkg/store/mockstore/mockstorage/canonical_storage_test.rs` 与真实 driver 相关测试中证明“检查的 timestamp 就是提交发布的 timestamp”以及拒绝时写入仍私有。
- 新增直接适配层测试时，按仓库约定放在独立 `*_test.rs` 文件并由模块入口声明，至少覆盖三态映射、`Some(&[])` 与 `None` 区别、action type 字符串化及两个标准错误映射。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点，目标文件已索引且识别出 6 个节点；`node --file pkg/session/runtime/schema_validation.rs` 读取了完整 51 行源码。
- RustCodeGraph `node SharedValidator`：确认 `checker` 实例化 `SharedValidator`；`node SchemaValidator` 和 `node --file pkg/domain/schema_checker.rs` 确认 trait 参数、三态结果、默认 500 ms/10 次重试及错误分支。
- RustCodeGraph `node --file pkg/session/runtime/control.rs`：确认唯一生产调用点的非只读门槛、表 ID 收集/过滤/去重、MDL 开关以及 `kv::SchemaChecker` 安装。
- RustCodeGraph `node --file pkg/infoschema/isvalidator/validator.rs`：确认停止、restart 版本、nil/空列表、delta/MDL、lease 与三态返回的真实分支。
- 读取 `pkg/session/Cargo.toml`、`pkg/session/runtime.rs`、`pkg/kv/option.rs`、`pkg/store/driver/kv_adapter.rs` 和 `pkg/store/mockstore/mockstorage/canonical_storage.rs`，核对 crate 依赖、模块边界、闭包类型及两个存储提交入口。`pkg/session/doc.go` 不存在，因此没有额外 package contract 可读。
- Rust 测试证据：`pkg/domain/schema_checker_test.rs`（参数转发、Unknown 重试、Fail 详情、耗尽/零重试）；`pkg/infoschema/isvalidator/validator_test.rs`（stop、lease、版本与相关表）；`pkg/session/test/temporarytabletest/temporary_table_test.rs`（临时表与普通表提交行为）；`pkg/store/mockstore/mockstorage/canonical_storage_test.rs`（提交 timestamp 与拒绝写入语义）。
- Go 对照：`pkg/domain/schema_checker.go`、`pkg/infoschema/isvalidator/validator.go`、`pkg/sessiontxn/isolation/base.go`、`pkg/session/test/session_test.go`、`pkg/session/test/temporarytabletest/temporary_table_test.go`。
- 本任务是只读代码分析与 Markdown 新增，按计划未运行 Cargo 或代码测试。最终结构检查要求文档存在且恰有本文所列 11 个固定二级标题。
