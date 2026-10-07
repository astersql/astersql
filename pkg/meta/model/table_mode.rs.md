# [`pkg/meta/model/table_mode.rs`](table_mode.rs)

## 文件定位

本文件定义表级运行模式元数据，以及提交 `AlterTableMode` 前用于描述目标表的参数结构。源码由 `pkg/meta/model/internal/group3/lib.rs` 的私有模块 `table_mode` 通过 `include!("../../table_mode.rs")` 纳入 `astersql-meta-model-group3`，随后经 `pub use table_mode::*`、根 crate 的 `group_3` 模块和根级再导出成为 `astersql_meta_model::TableMode` 等公共 API。根 `pkg/meta/model/Cargo.toml` 只聚合 group1–group4；本文件实际使用的 `serde_repr` 依赖声明在 `pkg/meta/model/internal/group3/Cargo.toml`。

它处在 DDL 表模式切换链的模型层，而不是执行层：`TableMode` 被 `pkg/meta/model/table.rs` 的 `TableInfo.Mode` 持久化，被 `pkg/meta/model/job_args.rs` 的 `AlterTableModeArgs.TableMode` 带入 DDL job；真正解析目标、构建和执行 job 的逻辑位于 domain/DDL 等上层模块。本文件自身不读取 catalog、不提交事务，也不阻止 DML/DDL。

## 核心职责

- `TableMode` 用三个稳定的数值判别值表达普通、导入和恢复状态，并提供人类可读名称。
- `TableMode::CanTransitionTo` 集中表达模式迁移的最小合法性规则：仅禁止 `Import -> Restore` 和 `Restore -> Import`，其余迁移（包括同模式迁移）均允许。
- `AlterTableModeTarget` 汇集调用方输入与 resolver 补全的元数据，供后续构建表模式 DDL job；它只是数据载体。

“非 Normal 时普通 DML/DDL 应受限”是该类型承载的业务语义，但具体拦截必须由使用 `TableInfo.Mode` 的上层实现完成，不能把源文件顶部注释误读为本文件已经实施访问控制。

## 主要符号

`pub enum TableMode` 使用 `#[repr(u8)]`，并派生 `Clone`、`Copy`、`Debug`、`Default`、`PartialEq`、`Eq`、`Serialize_repr` 和 `Deserialize_repr`。判别值与 Go `byte` 常量一致：`TableModeNormal = 0`（也是默认值）、`TableModeImport = 1`、`TableModeRestore = 2`。`serde_repr` 让序列化结果使用数值判别值，而不是枚举变体名称。

`pub fn String(self) -> &'static str` 分别返回 `"Normal"`、`"Import"` 和 `"Restore"`。返回值是静态字符串，不分配内存；命名保留 Go `fmt.Stringer` 风格，而不是 Rust 惯用的 `Display`。

`pub fn CanTransitionTo(self, target: TableMode) -> bool` 用一个负向匹配排除两个特殊模式之间的直接互转。它不检查调用来源，也不禁止同模式切换；Go 源码中的 TODO 同样说明未来可能为同模式修改补充“相同修改来源”校验。

`pub struct AlterTableModeTarget` 派生 `Clone` 和 `Debug`，包含 `SchemaID`、`SchemaName`、`TableID`、`TableName`、`CurrentMode`、`TargetMode`。ID 与目标模式是请求核心；名称和当前模式可由运行时 resolver 根据元数据补全。名称类型为 group1 正式 AST 身份再导出的 `ast::CIStr`，用于保留大小写不敏感标识语义。

## 执行流程

1. 调用方以 schema/table ID 和 `TargetMode` 描述切换意图；跨 keyspace 请求还需提供名称，本地请求可稍后补全。
2. 上层 resolver 查 catalog，补齐或校验 `SchemaName`、`TableName`，并把表当前的 `TableInfo.Mode` 写入 `CurrentMode`。本文件不执行这一步。
3. 上层在构建或应用 job 前调用 `CurrentMode.CanTransitionTo(TargetMode)`。例如 `pkg/domain/canonical_domain.rs` 的 `set_table_mode` 直接以旧表的 `Mode` 调用该方法。
4. 非法的 Import/Restore 互转由调用方转成错误；`canonical_domain::set_table_mode` 使用两端的 `String()` 形成错误文本。合法的同模式请求可被上层视为无变化，合法的不同模式请求则写回新的 `TableInfo.Mode`。
5. DDL job 参数通过 `AlterTableModeArgs.TableMode` 携带数值模式；本文件不负责 job 编解码、持久化、schema version 发布或回滚。

完整的 3×3 矩阵由 `pkg/meta/model/table_mode_test.rs::test_table_mode_can_transition_to` 验证：Normal 可到任意模式；Import、Restore 均可回 Normal 或保持自身；两个特殊模式不能直接互转。

## 数据与状态

`TableMode` 是无内部可变性的值类型。`0` 被定义为 Normal，并同时是 `Default`，这与 `TableInfo.Mode` 上的 `skip_serializing_if = "is_default"` 配合：普通模式可在表元数据 JSON 中省略；非默认模式按 `u8` 数值编码。修改判别值会影响已持久化元数据和 DDL job 的兼容性，因此只能追加兼容值，不能重排现有值。

`AlterTableModeTarget` 没有 `Default`、`PartialEq` 或 serde 派生，表明它是进程内请求/解析载体，不是本文件定义的持久化格式。其字段存在两个阶段：请求阶段以 ID 和目标模式为主，解析阶段具有经 catalog 核验的名称与当前模式。代码没有用类型系统区分这两个阶段，上层必须保证传给 job builder 的对象已解析完成。

## 依赖与调用关系

下游依赖只有 `serde_repr::{Serialize_repr, Deserialize_repr}` 与同 crate 的 `ast::CIStr`。`ast` 在 `internal/group3/lib.rs` 中从 group1 再导出，所以没有在 group3 Cargo manifest 中另建 parser 类型身份。

直接数据关系包括：

- `pkg/meta/model/table.rs::TableInfo.Mode` 保存当前模式，并将默认 Normal 从 JSON 省略。
- `pkg/meta/model/job_args.rs::AlterTableModeArgs.TableMode` 把目标模式纳入 DDL job 参数。
- `pkg/domain/canonical_domain.rs::set_table_mode` 是已核对的直接方法调用者：读取旧 `Mode`，调用 `CanTransitionTo`，用 `String` 生成非法迁移错误，并在合法变化时写入新模式。
- `pkg/domain/sqlsvrapi/server.rs` 与其 mock 以本文件的 `AlterTableModeTarget` 作为 API 参数；相关迁移测试直接构造该结构。

仓库中还存在 `pkg/domain/crossks/ddl_submit.rs`、`pkg/ddl/jobsubmit/table_mode.rs`、`pkg/ddl/table_mode.rs` 和 `pkg/util/dbutil/table.rs` 的局部 `TableMode` 类型。它们是各边界的适配模型，不等同于本文件类型；例如 cross-keyspace resolver 明确把 `astersql_meta_model::TableMode` 映射到自己的枚举。扩展时应显式同步这些映射，不能依赖同名即同一类型。

## 错误处理与边界

本文件没有 `Result` 返回值或自定义错误。`CanTransitionTo` 只返回布尔值，错误的类型、消息和恢复策略由调用方决定。当前规则不验证 schema/table 是否存在、名称是否匹配 ID、`CurrentMode` 是否新鲜，也不验证请求来源；这些都是 resolver、事务或 DDL 执行层职责。

Rust 枚举只可构造三个合法变体，因此 `String` 没有 Go `default => ""` 分支。反序列化数值 `0..=2` 之外的判别值时，`Deserialize_repr` 会返回反序列化错误；这与 Go 的 `type TableMode byte` 可保存未知 byte、并由 `String()` 对未知值返回空串不同。若未来需要前向兼容未知值，必须显式重新设计表示，不能假设当前派生行为会保留未知数值。

同模式迁移当前返回 `true`，但是否作为无操作处理由上层决定。`canonical_domain::set_table_mode` 在相同模式时返回未变更结果；该上层行为不是 `CanTransitionTo` 本身的保证。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部资源。`TableMode` 为 `Copy` 值，方法仅读取参数；`AlterTableModeTarget` 只拥有普通值和 `CIStr`，生命周期由调用栈或持有它的请求对象管理。

并发安全依赖上层在同一 catalog/事务快照内解析 `CurrentMode`、校验转换并提交修改。若先读取模式、随后脱离事务调用 `CanTransitionTo`，结果可能因并发 DDL 而过期；本类型没有版本号或 compare-and-swap 能力。同步 schema version、发布元数据以及失败回滚也全部属于上层 DDL 生命周期。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/meta/model/table_mode.go`，Rust 保留了三个常量的数值顺序、显示字符串、迁移矩阵以及 `AlterTableModeTarget` 的六个字段和语义。Go 测试 `pkg/meta/model/table_mode_test.go::TestTableModeCanTransitionTo` 与 Rust `pkg/meta/model/table_mode_test.rs::test_table_mode_can_transition_to` 使用相同九种组合；Rust 文件还包含一个精简矩阵测试。

主要语言差异有三点：Go 使用可容纳未知值的 `byte` 新类型，Rust 使用封闭的 `#[repr(u8)]` 枚举；Go `String()` 对未知值返回空串，而 Rust 不存在可安全构造的未知变体；Rust 额外派生数值 serde、默认值和复制/比较 trait。字段采用 Go 风格 PascalCase，以降低移植层的命名差异。

## 扩展指南

新增模式时，首先在 `TableMode` 末尾追加稳定判别值，并补齐 `String` 与 `CanTransitionTo` 的所有新旧组合；不要改变现有 `0/1/2`。随后同步 `pkg/meta/model/table_mode.go`、`table_mode_test.rs`、`table_mode_test.go`，并检查 `TableInfo.Mode` 和 `AlterTableModeArgs` 的序列化兼容性。

还必须搜索并更新所有边界枚举/映射，重点包括 cross-keyspace、DDL jobsubmit、DDL 执行和 dbutil 检查模块；否则新值可能在 `match` 处导致编译失败，或在数值转换处被当成非法模式。若改变同模式规则或引入“修改来源”，应把来源作为可验证数据贯穿 target/job 参数并在独立测试中覆盖，不能只在本方法里猜测调用者身份。

若调整 `AlterTableModeTarget` 的解析契约，应同步 `pkg/domain/sqlsvrapi/migration_aster_unit_test.rs` 及 resolver/job builder 的独立测试。测试逻辑应继续放在独立测试文件，不能内嵌到生产源文件。兼容风险主要是持久化判别值与 Go/Rust 跨边界一致性；性能风险很低，当前判断和字符串化均为常数时间且无分配。

## 验证依据

- RustCodeGraph：`status` 确认项目索引可用；`node --file pkg/meta/model/table_mode.rs --offset 1 --limit 260` 读取完整 73 行源码并报告 14 个引用文件；`query TableMode` 与 `query AlterTableModeTarget` 确认模型符号及上层同名适配符号。精确 `callers/callees` 未返回可用边，因而按技能规则使用局部源码搜索补足。
- 源码与装配：`pkg/meta/model/table_mode.rs`、`pkg/meta/model/internal/group3/lib.rs`、`pkg/meta/model/lib.rs`、`pkg/meta/model/table.rs`、`pkg/meta/model/job_args.rs`。
- Cargo：`pkg/meta/model/Cargo.toml` 与实际编译边界 `pkg/meta/model/internal/group3/Cargo.toml`。
- 直接调用证据：`pkg/domain/canonical_domain.rs::set_table_mode`；API 参数证据：`pkg/domain/sqlsvrapi/server.rs`。
- Go 对照与测试：`pkg/meta/model/table_mode.go`、`pkg/meta/model/table_mode_test.go`；Rust 独立测试：`pkg/meta/model/table_mode_test.rs`，另有 `pkg/meta/model/job_3_aster_unit_test.rs` 对显示名和关键迁移做冒烟验证。
- 本任务为纯文档分析，按计划不运行 Cargo；最终以固定的十一章节结构命令和人工事实复核验收。
