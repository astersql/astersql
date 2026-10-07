# `pkg/parser/ast/model.rs`

## 文件定位

`model.rs` 属于 `astersql-parser-ast` crate；crate 根由 `pkg/parser/ast/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，并在 `pkg/parser/ast/lib.rs:4994-4996` 以公开子模块 `pub mod model` 接入。它不是 SQL 解析入口，而是 Go `pkg/parser/ast/model.go` 中一组跨层共享的、具有稳定数值语义的模型枚举在 Rust AST 边界上的实现。

该模块的直接上游包括 `pkg/meta/model/internal/group1/lib.rs:10-24`：后者将这里的类型、常量和 `PriorityValueToName` 再导出，供正式元数据模型及 DDL 等模块使用。典型落点有 `pkg/meta/model/table.rs:637-665` 的表锁元数据、`pkg/meta/model/resource_group.rs:37-63` 的失控查询设置，以及该文件第 120 行以后对优先级名称的格式化。`CIStr` 与 `NewCIStr` 并非在本文件重复实现，而是由 `pkg/parser/ast/lib.rs:325-375` 定义，再由本文件第 23 行公开转发。

## 核心职责

1. 用 `value_type!`（`model.rs:25-33`）统一生成透明整数包装类型，使 Rust 类型系统区分表锁、分区、索引、外键动作等不同概念，同时保留与 Go 常量完全一致的底层数值。
2. 为这些值类型定义公开常量和 `fmt::Display`，把持久化/协议数值转换成 SQL 或诊断文本；未知值的回退规则逐类型对齐 Go，而不是统一报错。
3. 暴露 `LowPriorityValue`、`MediumPriorityValue`、`HighPriorityValue` 与 `PriorityValueToName`，供资源组等元数据格式化路径使用。
4. 从 crate 根再导出大小写不敏感标识符 `CIStr` 及其构造函数，保持 Go `model.go` 的使用入口形状；其真正的 JSON、比较、哈希和内存估算逻辑位于 `pkg/parser/ast/lib.rs:325-375`。

本文件不负责构造 AST 节点、不做语法校验，也不执行表锁、分区路由、索引构建、外键动作或 runaway 策略；这些值由下游模块解释和执行。

## 主要符号

- `value_type!($name, $repr)`：生成 `#[repr(transparent)] pub struct Name(pub Repr)`，并派生 `Clone`、`Copy`、`Debug`、`Default`、`Eq`、`Hash`、`PartialEq`、`Serialize`、`Deserialize`。透明包装和 serde 派生意味着序列化形态是底层整数，而不同领域类型不能被无意混用。
- `TableLockType(u8)` 与 `TableLockNone`、`Read`、`ReadLocal`、`ReadOnly`、`Write`、`WriteLocal`：表锁类型；`Display` 的未知值为空串（`model.rs:35-65`）。
- `ViewAlgorithm(i32)`、`ViewSecurity(i32)`、`ViewCheckOption(i32)`：分别描述视图算法、执行权限和检查范围；同时提供 `Undefined/Merge/Temptable`、`Definer/Invoker`、`Local/Cascaded` 关联常量（`model.rs:67-137`）。
- `PartitionType(i32)`：`None/Range/Hash/List/Key/SystemTime` 六种值，字符串分别为 `NONE/RANGE/HASH/LIST/KEY/SYSTEM_TIME`（`model.rs:139-171`）。
- `PrimaryKeyType(i8)`：默认、聚簇、非聚簇三态；仅后两者产生非空展示文本（`model.rs:173-191`）。
- `IndexType(i32)`：从 `Invalid=0` 到 `Fulltext=8`，包含 BTREE、HASH、RTREE、假设索引、向量、倒排、HNSW 和全文索引（`model.rs:193-234`）。这些判别值必须与 Go 以及已持久化元数据兼容。
- `ReferOptionType(i32)`：外键的无动作、RESTRICT、CASCADE、SET NULL、NO ACTION、SET DEFAULT（`model.rs:236-266`）。
- `RunawayActionType(i32)`、`RunawayWatchType(i32)`、`RunawayOptionType(i32)`：分别表达触发后的动作、观察匹配粒度、以及选项类别（`model.rs:268-320`）。前两者有展示逻辑；选项类别仅提供数值常量。
- `ColumnChoice(u8)`：统计分析的默认、全部列、谓词列、显式列清单策略，并提供 `Default/All/Predicate/List` 关联常量（`model.rs:322-348`）。
- `PriorityValueToName(u64) -> &'static str`：`1 -> LOW`、`16 -> HIGH`，其余值（包括显式的 `8`）均为 `MEDIUM`（`model.rs:350-361`）。
- `CIStr`、`NewCIStr`：本模块的公开再导出；实际结构含原始串 `O` 与小写串 `L`，见 `pkg/parser/ast/lib.rs:325-375`。

## 执行流程

本文件没有单一运行时入口；其逻辑在值被构造、序列化或格式化时触发：

1. 解析器、DDL 或元数据代码选用某个公开常量，或从持久化整数反序列化为透明包装类型。
2. 下游结构保存该值。例如 `pkg/meta/model/table.rs:637-665` 将 `TableLockType` 放入表锁元数据，`pkg/meta/model/resource_group.rs:37-63` 保存 runaway 动作和观察类型。
3. 当恢复 SQL、生成诊断文本或构建资源组描述时，`Display` 对值进行模式匹配；已知值返回固定大写关键字，未知值按各类型的 Go 兼容规则回退。
4. 资源组格式化调用 `PriorityValueToName`；`pkg/meta/model/resource_group.rs:120-133` 将结果拼为 `PRIORITY=<name>`。
5. 需要标识符时，调用者可通过 `model::NewCIStr` 转发入口构造值；真正的构造过程在 crate 根保存原文并计算 Unicode 小写形式（`pkg/parser/ast/lib.rs:368-375`）。

这里的 `Display` 只负责投影文本，不修改值，也不验证下游操作是否合法。例如 `ReferOptionSetDefault` 能格式化为 `SET DEFAULT`，但外键执行能力由下游决定。

## 数据与状态

所有由 `value_type!` 生成的类型都是单字段整数新类型，且为 `Copy` 值语义；本文件没有可变全局状态。底层表示分别是：表锁与列选择为 `u8`，主键类型为 `i8`，其余模型枚举为 `i32`，优先级为裸 `u64`。

数值本身是兼容协议的一部分。尤其 Go `pkg/parser/ast/model.go:221-236` 明确警告 `IndexType` 同时被 TiFlash 使用并可能来自旧版本持久化的 `TableInfo`，所以只能在末尾追加兼容值，不能重排或复用已有判别值。其他类型也按 Go 的 `iota`/显式数值逐项对齐。

`Default` 由整数包装的零值派生，因此零通常表示无/未指定/默认：无表锁、未定义视图算法、无分区、默认主键、无效索引、无外键选项、runaway 的 none/rule，以及默认列选择。例外是零值的展示语义依各类型定义：例如 `ViewSecurity(0)` 为 `DEFINER`，`ViewCheckOption(0)` 为 `LOCAL`。

`CIStr` 的状态不存于本文件；crate 根的 `CIStr { O, L }` 保留原始文本和比较用小写文本。字符串 JSON 输入会重新计算 `L`，对象 JSON 输入则保留对象给出的 `O/L`（`pkg/parser/ast/lib.rs:332-345`）。

## 依赖与调用关系

直接依赖很窄：`serde::{Serialize, Deserialize}` 用于值类型编解码，`std::fmt` 用于 `Display`，`crate::{CIStr, NewCIStr}` 用于公开转发。`pkg/parser/ast/Cargo.toml` 证明 serde 开启 derive，并且该 crate 还依赖 parser-auth、parser-charset、parser-mysql、parser-types、serde_json 与 url；本文件自身只直接使用其中的 serde。

模块接线为 `pkg/parser/ast/lib.rs -> pub mod model`。主要跨 crate 路径为：

`parser_ast::model` → `pkg/meta/model/internal/group1/lib.rs::ast` 再导出 → `pkg/meta/model/table.rs`、`resource_group.rs`、`index.rs` 等正式元数据结构 → DDL、表、锁、备份导出等消费者。

直接代码证据包括：

- `pkg/meta/model/internal/group1/lib.rs:12-24` 再导出本文件的类型、常量和优先级函数；
- `pkg/meta/model/table.rs:640,664` 通过 `ast::model::TableLockType` 保存锁类型；
- `pkg/meta/model/resource_group.rs:53,59,131` 使用 runaway 类型和优先级函数；
- `dumpling/export/schema_projection.rs:167` 判断 `PartitionType::Key`；
- `dumpling/export/schema_projection_restore.rs:204-215,733-762` 将外键和分区类型恢复为 SQL；
- `pkg/ddl/index.rs:157-166` 在 parser AST 索引类型和 meta model 索引类型之间显式映射。

需要区分 crate 根的同名 AST 枚举与 `model` 子模块的新类型。例如 `pkg/parser/ast/lib.rs` 还定义了解析语句直接使用的 `ast::IndexType`、`ast::PartitionType` 等，而正式元数据常经 `parser_ast::model` 进入；边界转换应显式书写，不能依赖相同名称假定它们是同一类型。

## 错误处理与边界

本文件没有返回 `Result` 的函数，也不主动产生错误。无法识别的整数被保留在新类型中，格式化时按 Go 行为降级：

- `TableLockType`、`PartitionType`、`PrimaryKeyType`、`IndexType`、`ReferOptionType` 返回空串；
- `ViewAlgorithm` 返回 `UNDEFINED`，`ViewSecurity` 返回 `DEFINER`，`ViewCheckOption` 返回 `CASCADED`；
- `RunawayActionType` 返回 `DRYRUN`，`RunawayWatchType` 返回 `NONE`，`ColumnChoice` 返回 `DEFAULT`；
- 未知优先级返回 `MEDIUM`。

这种“可表示未知值、展示时回退”的边界用于兼容持久化数据和 Go 行为，但也可能掩盖上游非法输入；需要严格校验时，应由构造/解析或业务层在进入本模型前完成，而不能把 `Display` 当校验器。

`CIStr` 的 JSON 错误由 crate 根实现处理：对象或字符串可反序列化，数字等不匹配输入返回 serde 错误；证据见 `pkg/parser/ast/model_7_aster_unit_test.rs:54-64`。本文件只转发该 API。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。所有模型类型是拥有整数的 `Copy` 值，常量具有静态生命周期，`PriorityValueToName` 返回静态字符串；调用不会分配资源或形成清理义务。

`CIStr` 拥有两个 `String`，克隆和释放遵循 Rust 所有权；本文件的再导出不改变其生命周期。虽然 `TableLockType` 和 runaway 类型描述并发控制概念，它们只是元数据标签，实际锁获取、查询终止或资源组切换发生在下游执行模块，不能据此文件推断同步保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/ast/model.go`。Rust 对齐了 Go 中从 `TableLockType` 到 `PriorityValueToName` 的数值次序和主要字符串结果：表锁（Go 25-64）、视图三类属性（66-133）、分区（135-169）、主键（171-192）、索引（194-236）、外键选项（238-266）、runaway（334-395）、列选择（397-420）和优先级（422-441）。

实现形态存在以下差异：

- Go 使用具名整数类型与 `String()` 方法；Rust 使用 `#[repr(transparent)]` 新类型、公开常量和 `Display`，从而通过 `to_string()` 得到同样文本。
- Rust 为若干类型增加关联常量（如 `PartitionType::Range`），便于模式匹配；Go 只暴露包级常量。两套 Rust 名称指向同一数值。
- Go 的视图字符串方法使用指针接收者；Rust 新类型是 `Copy`，`Display` 借用 `&self`，没有 nil 接收者这一边界。
- Go 在本文件直接定义 `CIStr`、哈希/比较/内存估算和 JSON 兼容逻辑；Rust 将真实实现放在 crate 根 `lib.rs:325-375`，本文件仅再导出。Rust 对象 JSON 输入与 Go 一样保留已有 `L`，字符串输入计算小写。
- Go `IndexTypeHNSW` 注释说明其只用于 AST、预处理后改写为 Vector；Rust保留相同判别值，但本文件本身不执行该改写。

测试证据方面，`pkg/parser/ast/model_test.rs` 覆盖 `CIStr` 的原文/小写/展示及字符串与对象 JSON 往返；`pkg/parser/ast/model_7_aster_unit_test.rs:10-65` 覆盖表锁、索引、外键、优先级和 `CIStr` 的 Go 对齐行为。

## 扩展指南

新增或修改模型值时应遵循以下顺序：

1. 先核对 `pkg/parser/ast/model.go` 的数值、字符串与兼容注释；对持久化枚举只追加新值，不重排已有值，尤其不能破坏 `IndexType` 的 TiFlash/旧版本兼容性。
2. 在 `model.rs` 中更新相应常量、关联常量和 `Display` 分支；若新增全新值类型，使用 `value_type!` 保持 serde、默认值和透明表示一致。
3. 检查 `pkg/meta/model/internal/group1/lib.rs::ast` 是否需要再导出，并搜索 parser AST 与 meta model 之间的显式转换（例如 `pkg/ddl/index.rs:157-166`）。同名类型并不保证自动兼容。
4. 在独立测试文件中同步验证，不要把测试写入生产源文件。字符串/未知值/判别值测试优先补到 `pkg/parser/ast/model_7_aster_unit_test.rs`；`CIStr` 构造和 JSON 行为补到 `pkg/parser/ast/model_test.rs`。
5. 若修改 `CIStr`，真正实现位于 `pkg/parser/ast/lib.rs:325-375`；同时检查所有通过 `model` 再导出的用户，并保留字符串 JSON 的升级兼容行为。

主要风险是判别值兼容、未知值回退改变导致 SQL/诊断文本漂移、serde 形态变化，以及遗漏 meta model 再导出或边界转换。这里的格式化路径很短，性能风险主要来自扩大 `CIStr` 构造/克隆，而整数枚举本身无明显热点资源成本。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/ast/model.rs` 确认目标已索引，文件含 104 个符号。
- RustCodeGraph 源码读取：`node --file pkg/parser/ast/model.rs --offset 1 --limit 260` 与 `--offset 261 --limit 180` 覆盖全部 361 行；`node --file pkg/parser/ast/lib.rs` 核对 `CIStr` 实现、公开模块接线与独立测试接线。
- RustCodeGraph 符号查询：查询了 `PriorityValueToName`、`TableLockType`、`PartitionType`、`IndexType`、`RunawayActionType`、`ColumnChoice`、`CIStr`、`NewCIStr`，并用常量查询确认 `PartitionTypeSystemTime`、`IndexTypeHNSW`、`RunawayActionSwitchGroup`、`PredicateColumns` 的目标定义。精确 `callers/callees` 对这些宏生成新类型/常量未返回边，因此调用关系使用已索引文件的模块使用关系及仓库直接引用点交叉验证，未虚构静态调用边。
- 已读配置与入口：`pkg/parser/ast/Cargo.toml`、`pkg/parser/ast/lib.rs`、`pkg/meta/model/internal/group1/lib.rs`。
- 已读 Go 对照：`pkg/parser/ast/model.go` 全部 441 行。
- 已读 Rust 测试：`pkg/parser/ast/model_test.rs`、`pkg/parser/ast/model_7_aster_unit_test.rs`；crate 根 `lib.rs:5077-5082` 证明二者以 `#[cfg(test)]` 独立接线。
- 已读直接消费者：`pkg/meta/model/table.rs`、`pkg/meta/model/resource_group.rs`，并通过仓库引用搜索核对 `pkg/ddl/index.rs`、`dumpling/export/schema_projection.rs`、`dumpling/export/schema_projection_restore.rs` 等使用点。
- 人工复核结论：本文区分了定义、再导出、消费者与未由本文件执行的业务行为；所有未知值回退均逐项来自 Rust/Go 源码，没有把格式化能力描述为业务校验或执行能力。
