# `pkg/expression/cache_snapshot.rs`

源文件：[`pkg/expression/cache_snapshot.rs`](cache_snapshot.rs)

## 文件定位

本文件属于 `astersql-expression` crate，是 Rust 物理计划实例缓存边界上的“拥有所有权、可跨线程传递”的表达式快照层。它不执行表达式，也不直接管理缓存；它把运行时的 `Expression` trait object、`Schema`、`NameSlice` 等对象转换为只含自有值的闭合表示，并在新的 `BuildContext` 中重建运行时对象。

模块由 `pkg/expression/lib.rs` 以私有模块 `cache_snapshot` 装配，再通过 `pub use cache_snapshot::*` 导出公开类型。实际生产上游集中在 `pkg/planner/core/operator/physicalop/cache_snapshot.rs`：物理计划快照捕获列、条件、函数参数和 schema，命中缓存后再用新的表达式上下文恢复它们。`Column::ToCacheSnapshot` 与 `Schema::ToCacheSnapshot`（分别位于 `column.rs`、`schema.rs`）是包内兼容命名的便捷入口。

`pkg/expression/Cargo.toml` 声明本 crate 名为 `astersql-expression`，并以 `package.metadata.porting.go-package = "pkg/expression"` 指向 Go 对照包；本文件本身没有外部 feature 门控，也不引入新的直接依赖。

## 核心职责

1. 为列、关联列、常量和标量函数提供封闭白名单快照 `CachedExpression`，拒绝无法安全脱离运行时状态的表达式类型。
2. 递归捕获虚拟列表达式、延迟常量表达式和标量函数参数，并在恢复时重新创建对象树，而不是保留原 trait object 或可变槽位。
3. 保存影响语义的类型、标识、参数序号、分组元数据和字符集/排序规则信息，同时刻意不保存可按需重建的内部缓存（例如列 hash cache）。
4. 通过版本化的 `CachedBuiltinId` 与核心 builtin 注册表重建标量函数，阻止扩展函数、未知函数以及伪造/过期 ID 穿过缓存边界。
5. 完整保存 `Schema` 的列顺序、主键/非空唯一键和可空唯一键形状，以及 `NameSlice` 中 `None` 的位置和字段名协议元数据。

## 主要符号

- `CacheSnapshotError(String)`：所有捕获/恢复失败的统一错误。`unsupported` 是本文件内部的构造助手；错误文本用于指出不支持的类型、无效 ID、缺失类型或锁中毒等边界原因。
- `CachedCollation`：私有的排序规则快照，保存可选 coercibility、repertoire、charset、collation 和显式字符集标记。`capture`/`restore` 通过 `CollationInfo` 接口搬运这些值。
- `CachedColumn`：保存 `Column` 的返回类型、ID/UniqueID、行位置、递归虚拟表达式、显示名、隐藏/前缀/IN-operand 标记、关联列 UniqueID 及排序规则。公开入口为 `try_from_column`、`restore_column`。
- `CachedSchema`：保存有序 `columns`、`pk_or_uk`、`nullable_uk`。每个 key 是独立的 `Vec<CachedColumn>`，因此键顺序、重复列和可空键类别均不会被集合化或去重。
- `CachedNameSlice` / `CachedFieldName`：保存字段名切片及协议显示元数据。元组结构保留 `None` 空位；恢复时为每个非空项创建新的 `Arc<FieldName>`。
- `CachedCorrelatedColumn`：由 `CachedColumn` 和关联 datum 值组成。快照只复制当前 datum 值，不持有原运行时锁槽。
- `CachedConstant`：保存 datum、可选返回类型、递归 deferred expression、prepared parameter 序号、subquery reference ID 与排序规则。
- `CachedBuiltinId(String)`：稳定、版本化的 builtin 身份，当前前缀是 `builtin:v1:`。`from_name` 先规范为 ASCII 小写并检查核心注册表；`registered_name` 在恢复前再次验证前缀和注册状态；`as_str` 供诊断/测试读取。
- `CachedScalarFunction`：保存 builtin ID、必需返回类型、递归参数、可选 `GROUPING` 模式及 marks、排序规则。
- `CachedExpression`：四分支枚举 `Column`、`CorrelatedColumn`、`Constant`、`ScalarFunction`。`try_from_expression` 是捕获总入口，`restore` 是恢复总入口。

测试专用条件编译仅出现在 `CachedBuiltinId::from_raw_for_test`，用于构造非法 ID 验证恢复拒绝路径；生产数据结构和主流程没有 `cfg` 分支。

## 执行流程

### 捕获表达式

`CachedExpression::try_from_expression` 按固定顺序对 `dyn Expression` 做运行时向下转型：

1. `Column`：调用 `CachedColumn::capture`，并递归捕获 `VirtualExpr`。
2. `CorrelatedColumn`：捕获内嵌列；若存在 `data`，取得读锁并克隆 datum。锁已中毒时立即失败。
3. `Constant`：克隆值和类型；递归捕获 `DeferredExpr`；把 `ParamMarker` 降为稳定的序号，同时保存 `SubqueryRefID` 和 collation。
4. `ScalarFunction`：先拒绝 extension function，再检查函数名是否属于核心快照白名单；若 `GROUPING` 元数据明确处于“尚未初始化”状态也拒绝。随后要求存在返回类型、生成 `CachedBuiltinId`、递归捕获全部参数，并保存已初始化的 grouping mode/marks。
5. 其他实现 `Expression` 的具体类型统一返回“不支持”错误，不做有损降级。

任一子节点失败都会通过迭代器的 `collect::<Result<...>>()` 或 `transpose()` 原样终止整个快照，不产生部分成功对象。

### 恢复表达式

`CachedExpression::restore(ctx)` 根据枚举分支重建：

1. 列由 `CachedColumn::restore` 从默认列开始逐字段赋值，并递归恢复虚拟表达式。
2. 关联列先恢复列，再用 `NewCorrelatedDatum` 为保存值创建新槽；因此新旧对象不会共享同一个 `Arc`/锁。
3. 常量要求 `ret_type` 存在，用 `Constant::with_type` 建立对象，随后恢复 deferred expression、参数序号、subquery ID 和 collation。
4. 标量函数先恢复参数，再校验 stable ID，通过 `rebuild_core_cache_snapshot_builtin` 调用核心构造路径。构造结果必须仍能向下转型为 `ScalarFunction`；之后克隆 scalar，恢复 grouping 元数据及 collation。

### 捕获和恢复 schema/名称

`CachedSchema::try_from_schema` 分别遍历输出列、`PKOrUK` 和 `NullableUK`；内部 `capture_keys` 保持二维向量形状。`restore` 用 `NewSchema` 创建输出列，再以 `SetKeys` 和 `SetUniqueKeys` 恢复两类键。

`CachedNameSlice::from_name_slice` 逐位置捕获 `Option<Arc<FieldName>>`。`restore` 保持切片长度和空位，并对每个名字新建 `Arc`。`CachedFieldName` 搬运五个 `CIStr` 字段和三个布尔标记。

## 数据与状态

快照是值对象而不是序列化格式：所有类型都实现 `Clone`，但没有实现 `Serialize`/`Deserialize`，`CachedBuiltinId` 的 `builtin:v1:` 只是进程内稳定身份协议，不意味着磁盘兼容承诺。

主要状态不变量如下：

- 表达式树只能由四种枚举分支构成；新增运行时表达式类型不会自动进入缓存白名单。
- scalar function 的返回类型在捕获时必须存在；constant/column 为贴合运行时类型仍保存 `Option<FieldType>`，但 constant 恢复时会拒绝缺失类型，column 则按原状态恢复可选值。
- builtin 名称在捕获时转换为 ASCII 小写，ID 必须具有当前版本前缀且仍存在于核心注册表。
- grouping metadata 的三态有区别：`Some(false)` 表示必需元数据未初始化并导致捕获失败；`Some(true)` 对应的 mode/marks 会被保存；不适用的函数保留 `None`。
- schema key 使用有序二维向量；不会去重、排序或把 `NullableUK` 合并进 `PKOrUK`。
- collation 的 coercibility 是否曾被设置由 `Option` 单独保存，恢复时只有 `Some` 才调用 `SetCoercibility`。
- hashcode、canonical hashcode、builtin 内部对象、原关联 datum 锁等运行时派生/共享状态不进入快照，恢复对象自行重建这些状态。

## 依赖与调用关系

上游实际接线：

- `pkg/planner/core/operator/physicalop/cache_snapshot.rs` 是主要生产调用者。其 `CachedPhysicalProperty`、各物理算子快照、`CachedPlanBase` 和 `CachedSchemaProducer` 使用 `CachedColumn`、`CachedExpression`、`CachedSchema`；辅助函数 `capture_expressions`/`restore_expressions`、`capture_columns`/`restore_columns`、`capture_scalar_functions`/`restore_scalar_functions` 批量传播本文件的失败。
- `pkg/expression/column.rs::Column::ToCacheSnapshot` 和 `pkg/expression/schema.rs::Schema::ToCacheSnapshot` 转发到本文件的捕获入口。
- `CachedNameSlice` 当前除 `cache_snapshot_test.rs` 外未检索到生产调用，属于已导出但尚未在物理计划快照主链接线的能力；不能据此宣称计划缓存已使用名称切片快照。

下游依赖：

- `crate::*` 提供表达式具体类型、`BuildContext`、`CollationInfo`、`ParamMarker`、`NewCorrelatedDatum`、`NewSchema` 和类型模块。
- `builtin_core.rs::is_core_cache_snapshot_builtin` 定义可捕获函数集合：正式 builtin 注册表，加构造器特殊处理的 `Cast`、`GetVar`、`InternalFuncFromBinary`、`InternalFuncToBinary`。
- `builtin_core.rs::rebuild_core_cache_snapshot_builtin` 在恢复时再次检查集合，并通过 `NewFunctionBase` 的标准工厂重建函数。
- `ScalarFunction` 的 builtin 对象提供 extension 判定、grouping 元数据读取与恢复接口。

应用链路可概括为：会话规划生成运行时物理计划 → 物理计划快照递归调用本文件捕获表达式/schema → 实例计划缓存保存自有快照 → 新会话/上下文命中后由物理计划快照调用本文件恢复 → 标准 builtin 工厂重建可执行表达式。

## 错误处理与边界

本文件不 panic，也不静默跳过字段；所有预期失败都返回 `CacheSnapshotError`。已明确覆盖的拒绝边界包括：

- 关联 datum 的 `RwLock` 读锁中毒。
- extension builtin、未进入核心快照白名单的 builtin、非四类白名单的表达式实现。
- `GROUPING` 元数据要求初始化但尚未初始化。
- scalar function 缺少返回类型；constant 在恢复时缺少返回类型。
- stable builtin ID 前缀错误、名称为空、注册项已不存在。
- builtin 工厂返回错误，或返回值不是 `ScalarFunction`。
- grouping mode/marks 恢复接口返回错误。

错误会在递归边界短路，并由物理计划快照层转换成其自身的 `CacheSnapshotError` 文本。错误类型只封装字符串，没有来源链或结构化错误码；若未来调用方需要分类处理，应扩展错误枚举，而不是依赖全文匹配。

本文件不验证所有字段组合的业务合法性，例如列 ID/index 是否对应当前 schema；职责是忠实捕获受支持对象，并把需要上下文验证的构造交给 `BuildContext` 和标准 builtin 工厂。

## 并发与资源生命周期

设计目标是让快照脱离会话态和运行时共享指针。独立测试以编译期泛型约束确认 `CachedExpression`、`CachedColumn`、`CachedConstant`、`CachedScalarFunction`、`CachedSchema`、`CachedNameSlice` 均满足 `Send + Sync`。

捕获关联列时只在克隆 datum 的短时间内持有读锁；快照不保存 guard。恢复时 `NewCorrelatedDatum` 创建全新的槽，测试还用 `Arc::ptr_eq` 证明恢复对象与原对象不共享关联 datum 指针。

字段名恢复会为每个非空项创建新 `Arc`；表达式、虚拟表达式、deferred expression 和参数向量也递归新建。`FieldType`、`Datum`、`CIStr` 等通过值克隆保存。文件中没有后台任务、通道、事务、I/O 或显式资源清理；生命周期完全由 Rust 所有权和智能指针管理。

需要注意，`Clone` 快照可能复制较大的表达式树、datum 和 key 列形状；本文件没有去重、共享子树或内存预算。扩展大型字段时应评估实例计划缓存的捕获成本与常驻内存。

## 与 Go 版本的对应关系

Go `pkg/expression` 下没有 `cache_snapshot.go` 或同名快照类型；因此本文件不是同路径逐文件翻译，而是 Rust 为跨上下文、跨线程的实例计划缓存新增的边界层。

所承载的语义字段与 Go 类型保持对应：

- `CachedColumn` 对应 `pkg/expression/column.go::Column` 的 `RetType`、`ID`、`UniqueID`、`Index`、`VirtualExpr`、`OrigName`、隐藏/前缀/operand 标记、collation 和 `CorrelatedColUniqueID`。Go 的 `hashcode` 是派生缓存，同 Rust 一样不应作为稳定快照状态。
- `CachedCorrelatedColumn` 对应 Go `CorrelatedColumn{Column, Data}`，但 Rust 恢复时主动建立新 datum 槽，从而满足跨线程/跨会话边界；Go 的普通 `Clone` 会保留 `Data` 指针，语义目的不同。
- `CachedConstant` 对应 `constant.go::Constant` 的值、返回类型、`DeferredExpr`、`ParamMarker.order`、`SubqueryRefID` 与 collation；这保留了 prepared statement 和延迟非确定函数在计划缓存中的语义。
- `CachedScalarFunction` 对应 `scalar_function.go::ScalarFunction` 的函数名/返回类型/builtin 语义，但不复制具体 builtin 实例，而是以稳定 ID 经 Rust 核心注册表重建。
- `CachedSchema` 对应 `schema.go::Schema`。Go `Clone` 同样分别复制 Columns、PKOrUK 和 NullableUK；Rust 快照额外把它们转换成 `Send + Sync` 的自有表示。
- `CachedFieldName` 对应 `pkg/types/field_name.go::FieldName` 的全部显示字段和标记；`CachedNameSlice` 保留 Go `NameSlice` 的位置语义。

Go 仍通过 `CloneForPlanCache` 等既有路径处理计划缓存；不能把 Rust 的 stable builtin ID 白名单机制描述为 Go 已存在的 API。两边应对齐的是恢复后的表达式行为、字段和值语义，而非内部快照实现形状。

## 扩展指南

新增能力时应从最窄入口修改，并同步独立测试文件 `pkg/expression/cache_snapshot_test.rs`：

- 支持新的表达式具体类型：为 `CachedExpression` 增加显式枚举分支及自有快照类型，在 `try_from_expression` 和 `restore` 两侧成对实现；确认不携带 session/context/锁 guard，并新增 round-trip、拒绝/错误和 `Send + Sync` 测试。不要用兜底序列化绕过白名单。
- 增加 builtin：若它已进入正式 `funcs` 注册表，`every_core_snapshot_builtin_has_a_stable_id` 会检查 stable ID；若由构造器特殊处理，必须同时更新 `is_core_cache_snapshot_builtin`、重建路径和全注册表测试。extension function 当前是明确禁止项，放宽会改变隔离与兼容边界。
- 修改表达式字段：对照 Go 原始类型和 Rust 运行时类型，判断字段是语义状态还是可重建缓存；语义字段必须捕获/恢复并加 round-trip 断言，派生缓存应保持排除。
- 修改 schema/key：保持列顺序、重复元素以及 `PKOrUK`/`NullableUK` 分类；新增测试覆盖重复 key、可空键和虚拟列。
- 修改字段名：保持 `NameSlice` 长度及 `None` 位置，恢复时继续避免共享原 `Arc`；生产接线前补充实际调用方测试，因为当前能力只在本文件单测中使用。
- 改 stable ID 格式：视为缓存协议版本变更，必须定义旧 ID 的处理策略并测试未知版本；当前实现会直接拒绝非 `builtin:v1:`。

相关测试必须继续放在独立的 `cache_snapshot_test.rs`，不要嵌入生产源文件。对主链接线的改变还应同步 `pkg/planner/core/operator/physicalop/cache_snapshot_test.rs` 以及会话计划缓存的针对性测试。

## 验证依据

事实核对使用了以下直接证据：

- RustCodeGraph `status`：索引包含本仓库 Rust/Go 文件；`node --file pkg/expression/cache_snapshot.rs` 读取了完整 453 行源码。
- RustCodeGraph `query`：定位 `CachedExpression`（第 167 行）、`CachedColumn`（第 56 行）、`CachedSchema`（第 72 行）及精确方法节点 `try_from_expression`、`try_from_schema`、`from_name_slice`。
- RustCodeGraph 文件节点给出本文件被 `pkg/expression/cache_snapshot_test.rs` 及物理计划/执行相关文件使用；精确 `callers` 查询在本次环境中长时间无输出后被中止，因此生产调用边又以仓库 `rg` 逐项核对，没有把模糊的同名 `snapshot` 结果当作证据。
- 源码与装配：`pkg/expression/cache_snapshot.rs`、`pkg/expression/lib.rs`、`pkg/expression/builtin_core.rs`、`pkg/expression/column.rs`、`pkg/expression/schema.rs`、`pkg/expression/Cargo.toml`。
- 生产调用：`pkg/planner/core/operator/physicalop/cache_snapshot.rs`，包括批量 expression/column 捕获恢复、`CachedPlanBase` 与 `CachedSchemaProducer`。
- Rust 独立测试：`pkg/expression/cache_snapshot_test.rs`，覆盖七个测试：快照类型 `Send + Sync`、四种表达式 round trip 与 hash 语义、嵌套 builtin/deferred constant、稳定 builtin ID、未知 ID 拒绝、所有核心 builtin ID，以及 schema/name slice 的列/key/空位行为（其中 schema/name 断言位于同一测试函数）。
- Go 语义对照：`pkg/expression/column.go`、`constant.go`、`scalar_function.go`、`schema.go` 和 `pkg/types/field_name.go`；未发现 Go 同名 `cache_snapshot` 文件。

本任务为纯文档分析，按计划未运行 Cargo 或代码测试。结构完整性由任务指定的 11 章节检查验证；行为结论来自上述源码、调用点和既有独立测试，不把测试未覆盖的场景表述为已验证行为。
