# `pkg/infoschema/validatorapi/interface.rs`

## 文件定位

该文件是 `astersql-infoschema-validatorapi` crate 的公共契约文件，定义 InfoSchema 版本校验的三态结果 `Result` 和实现方必须提供的 `Validator` trait。crate 根 `pkg/infoschema/validatorapi/lib.rs` 通过 `pub mod interface` 声明模块，并用 `pub use interface::*` 在 crate 根重导出这两个符号；`Cargo.toml` 将 `lib.rs` 指定为库入口，并以 `package.metadata.porting.go-package = "pkg/infoschema/validatorapi"` 记录对应 Go 包。

它本身不保存租约、schema 版本或变更历史，也不执行校验算法。真实 Rust 状态机位于 `pkg/infoschema/isvalidator/validator.rs`，其中 `impl ValidatorApi for Validator` 实现本文件的 trait。当前生产接线并不都经由 `dyn validatorapi::Validator`：例如 `pkg/session/runtime/schema_validation.rs` 直接包装具体的 `isvalidator::Validator` 并调用其固有 `check`，`pkg/infoschema/issyncer/syncer.rs` 也定义了本地 `SchemaValidator` 适配层。因此本文件的主要作用是固定 Go 兼容 API 和跨实现边界，而不是充当完整应用中的唯一动态分派入口。

## 核心职责

1. 用 `#[repr(i32)] enum Result` 固定检查结果的数值协议：`ResultSucc = 0`、`ResultFail = 1`、`ResultUnknown = 2`。`pkg/infoschema/validatorapi/migration_aster_unit_test.rs::result_values_match_go_iota_order` 直接验证这些值与 Go `iota` 顺序一致。
2. 用 `Validator::RelatedSchemaChange` 把“事务客户端提供的相关 schema 变更摘要”留给实现方选择，避免这个 API crate 直接依赖某个 Rust 事务客户端类型。Go 对照则直接使用 `*transaction.RelatedSchemaChange`。
3. 固定更新、检查和生命周期方法的参数顺序与返回形状，使实现可以登记租约窗口及 schema 增量，并让事务提交检查区分成功、确定失败和暂时未知。
4. 保留 Go 的 nil 语义：`Check` 的 `Option<&[i64]>` 区分 `None`（Go `nil` slice）和 `Some(&[])`（非 nil 空 slice）；`Update` 的 `Option<&RelatedSchemaChange>` 与 `Check` 返回的 `Option<RelatedSchemaChange>` 分别对应 Go 的可空指针输入和输出。

## 主要符号

- `pub enum Result`：`Clone + Copy + Debug + Eq + PartialEq` 的 C 风格枚举，并以 `#[repr(i32)]` 固定判别值。`ResultSucc` 表示当前证据足以继续，`ResultFail` 表示明确发现不兼容的 schema 变化，`ResultUnknown` 表示校验器停止或租约证据不足等无法判定情形。具体产生条件由实现决定；`pkg/infoschema/isvalidator/validator.rs::check` 是当前实现依据。
- `pub trait Validator`：schema 校验器契约。trait 未声明 `Send`、`Sync`、错误类型或异步方法；是否可跨线程共享由具体实现和上层容器决定。
- `type RelatedSchemaChange`：实现关联类型。当前实现将其设为 `pkg/infoschema/isvalidator/validator.rs::RelatedSchemaChange`；接口测试则使用 `()` 或本地记录结构，证明 API 不绑定具体表示。
- `Update(&self, leaseGrantTime, oldSchemaVer, newSchemaVer, change)`：刷新租约及最新版本，并携带从旧版本到新版本的可选变更摘要。当前实现只在版本变化时把 delta 入队。
- `Check(&self, txnTS, schemaVer, relatedPhysicalTableIDs, needCheckSchema)`：按事务时间戳、事务所见 schema 版本、相关物理表集合和 delta 检查开关进行判定，返回可选变更摘要与三态结果。
- `Stop` / `Restart(currSchemaVer)` / `Reset`：定义停止、以当前版本重启、恢复初始状态三种生命周期操作。它们语义不同，不能互换。
- `IsStarted` / `IsLeaseExpired`：分别查询运行状态和当前租约是否已过期。

## 执行流程

本文件只有类型和方法声明；以下流程是契约如何进入当前实现与应用链的可追溯路径，而不是本文件内部的函数体：

1. InfoSchema reload 路径在 `pkg/infoschema/issyncer/syncer.rs` 获得新旧 schema 版本和可选 `RelatedSchemaChange`，必要时先 `Reset`，随后用 reload 时间戳调用校验器的 `Update`。
2. `pkg/infoschema/isvalidator/validator.rs` 的 `impl ValidatorApi for Validator` 将本 trait 的大写方法逐一转发到同名语义的小写固有方法；`Update` 更新 `latest_schema_ver` 和 `latest_schema_expire`，版本前进时维护 delta 队列。
3. 事务提交检查由 `pkg/session/runtime/schema_validation.rs::SharedValidator::check` 调用具体实现的 `check`，再把 `ResultSucc`、`ResultUnknown`、`ResultFail` 映射为 domain 层的 `SchemaCheckResult`。
4. `pkg/domain/schema_checker.rs::SchemaChecker::check_by_schema_version` 对成功立即放行，对失败返回 `InfoSchemaChanged`，对未知按配置睡眠重试，耗尽次数后返回 `InfoSchemaExpired`。
5. 当前实现的主要判定顺序见 `pkg/infoschema/isvalidator/validator.rs::check`：停止状态返回 `Unknown`；事务版本早于重启下界返回 `Fail`；版本落后时按 nil/空表集合、MDL/delta 开关和变更历史判定；版本未落后但事务时间超过租约时返回 `Unknown`；其余返回 `Succ`。

## 数据与状态

接口文件自身没有全局变量、字段、缓存或可变状态。它传递的状态维度包括：

- `leaseGrantTime: u64` 与 `txnTS: u64`：均是时间戳边界值。当前实现用授予时间加 lease 再减 1ms 计算到期点，并把事务时间与到期点比较；单位和编码必须与上游时间戳转换一致。
- `oldSchemaVer`、`newSchemaVer`、`schemaVer`、`currSchemaVer: i64`：分别表示更新前后版本、事务使用版本和重启后的版本下界。负值并未被接口类型禁止，边界策略属于实现。
- `relatedPhysicalTableIDs: Option<&[i64]>`：借用调用方切片且不取得所有权。`None` 与 `Some(&[])` 是不同协议值；当前实现中，旧 schema 配合 `None` 会直接失败，而空切片可以代表只涉及临时表并继续走 delta 判定。
- `RelatedSchemaChange`：`Update` 只借用变更摘要，`Check` 则可按值返回摘要，因而实现若确实返回详情，需要决定克隆或所有权转移策略。当前 `isvalidator::check` 的各分支均返回 `None`，但接口与上层 domain 映射保留了 `Some` 的能力。
- `Result` 的整数表示是兼容边界；调整成员顺序或数值会破坏 Go 对齐及潜在 FFI/序列化假设。

## 依赖与调用关系

本文件只使用 Rust 核心语言能力，没有 `use` 导入；`pkg/infoschema/validatorapi/Cargo.toml` 也没有声明普通依赖。这一低耦合设计通过关联类型隔离事务客户端的 `RelatedSchemaChange`。

直接模块关系为 `lib.rs -> interface.rs`，且 crate 根重导出 `Result` 与 `Validator`。实现关系为 `pkg/infoschema/isvalidator/validator.rs::Validator -> validatorapi::Validator`；转发实现覆盖 `Update`、`Check`、`Stop`、`Restart`、`Reset`、`IsStarted` 和 `IsLeaseExpired` 全部方法。

RustCodeGraph 对 trait 节点 `pkg/infoschema/validatorapi/interface.rs::Validator` 的 `callers`/`callees` 没有返回方法级调用边，这是 trait 声明与动态/转发调用在当前索引中的限制。源码搜索补出的直接证据包括：`pkg/infoschema/isvalidator/validator.rs` 的 trait 实现、`pkg/infoschema/issyncer/syncer.rs` 的 reload 更新入口、`pkg/session/runtime/schema_validation.rs` 的提交检查适配，以及 `pkg/domain/schema_checker.rs` 的三态消费逻辑。

## 错误处理与边界

接口不返回 `Result<T, E>`，也不定义可传播的 Rust 错误；校验的不确定性通过业务枚举 `ResultUnknown` 表达，确定的不兼容通过 `ResultFail` 表达。调用方不能把 `Unknown` 当作成功：当前 domain 层会重试，最终转成 InfoSchema 过期错误。

必须保留以下边界：

- `None` 与 `Some(&[])` 不可通过 `unwrap_or_default` 等方式提前合并；`interface_test.rs::check_preserves_go_nil_and_empty_slice_distinction` 和迁移测试分别锁定该差异。
- `Update` 的 `change = None` 是合法输入，不能等同于 API 调用缺失；版本仍可刷新，当前实现会形成空变更摘要。
- `Stop` 后的 `Check` 应由实现走未知路径，而不是默认成功；`Restart` 还携带防止旧写事务提交的版本下界。
- `IsLeaseExpired` 是墙钟状态查询，`Check` 则使用传入的事务时间戳判断，两者不可互相替代。
- trait 没有约束版本单调性、时间戳合法性或关联类型内部不变量；例如当前实现要求表 ID 与 action type 等长，否则相关表扫描会触发 `expect`。这些属于实现/调用者必须共同维护的前置条件。

## 并发与资源生命周期

本接口所有方法都接收 `&self`，允许实现使用内部可变性，但 trait 本身没有 `Send + Sync` 上界，也未规定锁模型。当前 `pkg/infoschema/isvalidator/validator.rs::Validator` 使用 `RwLock<ValidatorState>`：查询持读锁，更新及生命周期切换持写锁；trait 的同步方法不会生成任务、通道或异步资源。

生命周期状态可概括为：构造后 started；`Stop` 标记停止并清空版本/delta；`Restart` 恢复 started 并记录重启版本下界；`Reset` 恢复 started、清零最新版本和重启下界并清空 delta。租约由后续 `Update` 刷新。实现和调用方必须避免在持有自身外部锁时进行可能形成反向锁序的扩展；若未来增加异步或阻塞工作，不应直接塞进这些同步 trait 方法而不重新审视上层 reload 和提交路径的延迟。

测试中的 `Cell`/`RefCell` 仅用于单线程记录桩，并不证明 trait 对象线程安全。生产实现可被 `Arc` 共享是具体类型能力，不能从本 trait 的声明单独推出。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/infoschema/validatorapi/interface.go`：Go 的 `type Result int` 与三个 `iota` 常量对应 Rust 的 `#[repr(i32)] Result`；Go `Validator` 的七个方法在 Rust trait 中逐项保留，参数顺序和 Go 风格方法名也一致。

主要类型映射如下：Go `*transaction.RelatedSchemaChange` 输入映射为 `Option<&Self::RelatedSchemaChange>`，返回指针映射为 `Option<Self::RelatedSchemaChange>`；Go `[]int64` 映射为 `Option<&[i64]>`，特意保留 nil slice 与非 nil 空 slice；Go `bool`、`uint64`、`int64` 分别映射为 Rust 同语义标量。

语义核验还来自 `pkg/infoschema/isvalidator/validator.go` 与 `validator_test.go`：Go 实现用 `sync.RWMutex` 保护 started、租约、版本和 delta 队列；测试覆盖正常租约、停止/重启、超时、nil 表集合、相关表变更和队列压缩。Rust 的实际状态机及独立测试位于相邻 `isvalidator` crate。本 API 文件不应复制这些算法，只应保持足以表达它们的稳定契约。

当前存在有意的 Rust 抽象差异：关联类型解除对 `client-go` 的直接依赖；trait 未强制 `Send + Sync`；Rust `Result` 明确固定为 `i32`，而 Go `int` 的机器字长依平台而定。数值 0/1/2 的语义相同，但不应据此宣称二进制 ABI 完全相同。

## 扩展指南

- 新增或修改结果状态时，先确认 Go `interface.go`、所有三态匹配点和数值兼容要求；至少同步 `migration_aster_unit_test.rs::result_values_match_go_iota_order`、`pkg/session/runtime/schema_validation.rs` 和 domain 层错误映射。新增枚举成员会使现有穷尽匹配编译失败，这是需要有意处理的 API 变更。
- 修改 `Validator` 方法签名时，同步更新 `pkg/infoschema/isvalidator/validator.rs` 的 `impl ValidatorApi for Validator`、两个 validatorapi 独立测试记录桩及所有下游适配。不要把实现状态字段或算法搬进本接口文件。
- 涉及相关表集合时必须继续测试 `None`、`Some(&[])` 和非空切片三种输入；Go nil/empty 的差别是事务 DDL 错误检查与临时表事务路径的实际行为边界。
- 若需要让 API 直接作为跨线程 trait object 使用，应基于真实调用路径评估在 trait 上增加 `Send + Sync`，并同步调整测试桩；不要仅因当前具体实现位于 `Arc` 中就假定该约束已经存在。
- 若要返回 `Some(RelatedSchemaChange)`，需要补充实现级测试和 domain 映射测试，验证所有权、action 类型转换及错误携带行为。
- 性能关注点不在本文件的零成本声明，而在实现的锁持有时间、delta 扫描和 `Unknown` 重试。扩展契约时应避免迫使高频 `Check` 分配；目前表 ID 使用借用切片正是这一边界的一部分。
- Rust 测试继续放在独立的 `interface_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产源文件。

## 验证依据

- RustCodeGraph `status`：项目索引包含 11,467 个文件，目标目录的 `interface.rs`、`interface_test.rs`、`migration_aster_unit_test.rs`、`lib.rs` 均已索引；`interface.rs` 识别出 10 个符号。
- RustCodeGraph `node --file pkg/infoschema/validatorapi/interface.rs`：核对 `Result`、`Validator`、关联类型和七个方法的完整声明；`node pkg/infoschema/validatorapi/interface.rs::Validator` 再次核对 trait 源码。
- RustCodeGraph `callers` / `callees` 查询 trait 节点未产生方法级边，已如实记录该索引限制；随后通过精确源码/符号搜索核对真实实现和接线，不把泛化的同名搜索结果当作调用证据。
- 读取的 crate 边界文件：`pkg/infoschema/validatorapi/Cargo.toml`、`pkg/infoschema/validatorapi/lib.rs`。
- 读取的直接 Go 对照：`pkg/infoschema/validatorapi/interface.go`；实现与 Go 测试证据：`pkg/infoschema/isvalidator/validator.go`、`pkg/infoschema/isvalidator/validator_test.go`。
- 读取的 Rust 实现/调用证据：`pkg/infoschema/isvalidator/validator.rs`、`pkg/infoschema/issyncer/syncer.rs`、`pkg/session/runtime/schema_validation.rs`、`pkg/domain/schema_checker.rs`。
- 读取的独立 Rust 测试：`pkg/infoschema/validatorapi/interface_test.rs`、`pkg/infoschema/validatorapi/migration_aster_unit_test.rs`；它们覆盖 nil/empty 区分、枚举值、全部 trait 参数与生命周期方法的可实现边界。
- `pkg/infoschema/doc.go` 不存在，因此没有可读取的目标包 Go 包级契约；本说明以最近的 crate 根、同路径 Go 文件、实际实现和测试为权威证据。
