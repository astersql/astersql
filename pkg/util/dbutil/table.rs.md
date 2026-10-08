# `pkg/util/dbutil/table.rs`

## 文件定位

本文件属于 `astersql-util-dbutil` crate 的表级元数据辅助模块，由 [`pkg/util/dbutil/lib.rs`](lib.rs) 以 `pub mod table` 暴露。crate 清单 [`pkg/util/dbutil/Cargo.toml`](Cargo.toml) 将 Go 包对应关系声明为 `pkg/util/dbutil`，而本文件直接依赖 `astersql-infoschema` 提供的 `CiString` 与 `ColumnInfo`。

它只实现两个小型、无状态的查询/校验原语：按不区分大小写的名称查列，以及拒绝对非 Normal 模式表执行受保护操作。RustCodeGraph 将本文件识别为 8 个符号，并显示索引中的使用文件为 `pkg/util/dbutil/table_test.rs` 与 `pkg/planner/core/expression_rewriter.rs`；进一步核查后，后者调用的是 `IndexInfo::FindColumnByName`（定义于 `pkg/meta/model/index.rs`），不是本文件函数。因此当前 Rust 生产代码中没有找到本模块 API 的真实调用者，确定的 Rust 调用仅来自独立测试。

## 核心职责

- `FindColumnByName`：在线性列元数据切片中查找名称匹配的第一列。输入名称先转为小写，再与每个 `ColumnInfo.name.lower` 比较；命中时借用返回原切片中的列，未命中返回 `None`（`table.rs:35-40`）。
- `CheckTableModeIsNormal`：建立表模式保护门槛。`Normal` 返回 `Ok(())`；`Import` 和 `Restore` 返回包含表原始名称与模式名称的错误字符串（`table.rs:42-56`）。
- `TableMode`：为上述校验提供本地的三态枚举，并把 `Normal` 设为默认值（`table.rs:23-33`）。这个枚举不是 `astersql_meta_model::TableMode`；调用方接线时不能假定二者自动互换。

## 主要符号

- `pub enum TableMode { Normal, Import, Restore }`：`Copy + Eq` 的轻量模式值。`Default` 通过 `#[default]` 选择 `Normal`，调试输出与分支中使用的模式文本一致。
- `pub fn FindColumnByName<'a>(columns: &'a [ColumnInfo], name: &str) -> Option<&'a ColumnInfo>`：公开查找函数。显式生命周期说明返回引用来自 `columns`，不会创建、缓存或拥有列对象。
- `pub fn CheckTableModeIsNormal(table_name: &CiString, table_mode: TableMode) -> Result<(), String>`：公开保护校验。成功值不携带数据；失败值当前是普通 `String`，不是 infoschema 的结构化错误类型。
- 文件没有模块级常量、trait、struct、`impl`、条件编译项或私有辅助函数。公开符号通过 `table` 子模块访问；`lib.rs` 没有把它们再导出到 crate 根。

## 执行流程

`FindColumnByName` 的流程为：

1. 对查询参数调用 Unicode `str::to_lowercase`，得到拥有所有权的规范化字符串。
2. 按输入切片顺序调用 `iter().find(...)`。
3. 将规范化查询值与预先存放在 `CiString.lower` 中的列名比较。
4. 返回第一项匹配列的共享引用；遍历结束仍未命中则返回 `None`。

`CheckTableModeIsNormal` 的流程为：

1. 将传入模式与 `TableMode::Normal` 比较。
2. 若为 `Normal`，立即返回 `Ok(())`。
3. 否则用穷尽 `match` 把枚举映射为 `Normal`、`Import` 或 `Restore` 文本；实际错误分支只会得到后两者。
4. 使用 `table_name.original` 保留调用者提供的表名大小写，拼出 `ErrProtectedTableMode: Table <name> is in mode <mode>` 并返回 `Err(String)`。

两个函数均为同步的单次调用，不解析 SQL，也不读取数据库或全局模式状态；调用者必须先提供列快照、表名和当前模式。

## 数据与状态

本文件不保存可变状态。`TableMode` 是按值传递的三态标记；`ColumnInfo` 和 `CiString` 均来自 `astersql-infoschema`。名称对象同时保存 `original` 与 `lower`：查列读取 `lower`，错误消息读取 `original`，因此匹配语义与展示语义被刻意分开。

`FindColumnByName` 的时间复杂度是 O(n)，其中 n 为列数；查询字符串规范化还需要与名称长度成正比的时间和一次临时字符串分配。额外空间不随列数增长。若切片中意外含有多个相同的折叠名称，函数返回第一项，并不负责检测元数据冲突。

## 依赖与调用关系

下游依赖只有 `astersql_infoschema::{CiString, ColumnInfo}` 和 Rust 标准库的字符串、切片迭代与格式化能力。`Cargo.toml` 对非 Windows 目标唯一声明的普通依赖正是 `astersql-infoschema`；Windows 条件依赖属于同 crate 的其他 dbutil 模块，不是本文件逻辑所需。

RustCodeGraph 的 `query` 能定位 `TableMode`、`FindColumnByName`、`CheckTableModeIsNormal`，但对两个同名跨语言函数运行 `callers`/`callees` 没有返回边。精确引用核查确认：

- `pkg/util/dbutil/table_test.rs` 直接导入并执行三个公开符号，是当前可确认的 Rust 调用者。
- `pkg/planner/core/expression_rewriter.rs` 的同名调用接收者是索引元数据，解析到 `pkg/meta/model/index.rs` 的方法，不属于本模块。
- Go 侧 `FindColumnByName` 被 `pkg/util/dbutil/index.go` 的索引列选择逻辑及 planner 统计测试使用；Go 侧 `CheckTableModeIsNormal` 被 `pkg/ddl/executor.go`、`pkg/planner/core/optimizer.go` 与 `pkg/planner/core/operator/physicalop/foreign_key.go` 使用。这些路径说明原实现位于规划/DDL 的保护边界，但不证明 Rust 版本已经接入相同主链。

## 错误处理与边界

- 空列切片、未知名称以及大小写折叠后仍不匹配均返回 `None`，不产生错误。
- 查列依赖 `ColumnInfo.name.lower` 已由元数据构造过程正确维护。本函数只规范化查询侧，不重新规范化列侧，也不校验 `original` 与 `lower` 是否一致。
- `to_lowercase` 是 Unicode 小写转换；文档不应把它夸大为完整的 MySQL 标识符排序规则或数据库 collation 比较。现有测试只覆盖 ASCII 大小写。
- 模式校验只判断调用者传入的枚举，不负责读取或锁定表模式，因此检查结果只是传入快照时刻的结论。
- 错误当前没有错误码类型、错误源或可分类字段。其文本格式由独立 Rust 测试精确断言，修改大小写、空格、表名来源或模式文本会造成兼容变化。
- 枚举是封闭的，当前 `match` 穷尽三种模式；新增模式时编译器会要求更新文本映射，但同时还必须决定新模式是否允许操作。

## 并发与资源生命周期

本文件没有锁、原子量、任务、线程、通道、事务、文件句柄或网络连接。两个函数只读借用输入，因此本身可在并发上下文调用；一致性保证取决于调用者如何取得并发布元数据快照。

`FindColumnByName` 返回引用的生命周期绑定到列切片，Rust 类型系统阻止该引用活得比切片更久。函数只分配临时的小写查询字符串，该字符串在返回前释放；返回值不持有它。`CheckTableModeIsNormal` 在成功路径无堆分配，在失败路径创建并把错误字符串所有权交给调用者。

## 与 Go 版本的对应关系

[`pkg/util/dbutil/table.go`](table.go) 是直接对照文件。`FindColumnByName` 的 Rust 实现保持了 Go 版的核心语义：查询名先转小写、顺序扫描列、比较折叠名称、返回第一项或缺失值。类型差异是 Go 接受 `[]*model.ColumnInfo` 并返回可空指针，Rust 接受 `&[ColumnInfo]` 并返回 `Option<&ColumnInfo>`，从类型上排除了切片元素为空指针的情况。

`CheckTableModeIsNormal` 同样保留“仅 Normal 放行”的判断和包含表名/模式的错误意图，但并非完全等价移植：Go 使用 `ast.CIStr`、`model.TableMode` 与 `infoschema.ErrProtectedTableMode.FastGenByArgs` 生成结构化 TiDB 错误；Rust 使用 `astersql_infoschema::CiString`、本地 `TableMode` 与普通字符串。这意味着错误码、类型识别、统一格式化，以及与元数据层模式枚举的直接连接目前没有在本文件中体现。

Go 注释说明该检查用于优化阶段、外键触发器构建以及某些未进入 `ResolveContext` 的 DDL 执行路径；仓库引用也与此一致。Rust 当前缺少这些生产调用边，因此应把本模块视为已实现并测试的局部移植，而不是已完成全链路替换。

## 扩展指南

- 增加列名匹配规则时，优先修改 `FindColumnByName`，并在 `pkg/util/dbutil/table_test.rs` 增加 ASCII、非 ASCII、空名称、重复折叠名称等独立用例；需要与 MySQL collation 对齐时，应先确认 `CiString.lower` 的生成契约，避免查询侧和元数据侧采用不同规范化算法。
- 增加表模式时，同步修改 `TableMode`、`CheckTableModeIsNormal` 的放行策略和错误文本分支，并覆盖每种模式。还应与 `pkg/meta/model/table_mode.rs` 的模式集合和转换规则核对，避免产生第二套漂移的枚举。
- 若把 Rust 校验接入规划器、DDL 或外键路径，应优先复用真实元数据 `TableMode` 或增加显式、可测试的转换，而不是依赖整数或同名枚举的隐式对应；同时保留检查发生位置与 Go 调用点的语义一致性。
- 若调用方需要按错误码判断受保护表，应把 `String` 升级为项目统一错误类型，并同步错误兼容测试。不能只改文案来模拟结构化错误。
- 性能敏感且重复查列的场景可由上层建立名称索引；本函数的职责是简单线性查询，不应在这里引入全局缓存或延长元数据生命周期。
- 测试必须继续放在独立的 `pkg/util/dbutil/table_test.rs`，不要内嵌进生产源文件；Go 行为变更时同步审查 `pkg/util/dbutil/table.go` 与 `pkg/util/dbutil/table_test.go`。

## 验证依据

- 源码与边界：`pkg/util/dbutil/table.rs`（完整 57 行）、`pkg/util/dbutil/lib.rs`、`pkg/util/dbutil/Cargo.toml`。
- Go 对照：`pkg/util/dbutil/table.go`；生产引用通过精确检索确认于 `pkg/ddl/executor.go`、`pkg/planner/core/optimizer.go`、`pkg/planner/core/operator/physicalop/foreign_key.go` 和 `pkg/util/dbutil/index.go`。
- 测试依据：`pkg/util/dbutil/table_test.rs` 的 `protected_table_modes_are_rejected_with_table_identity` 覆盖 Normal/Import/Restore 与精确错误文本；`find_column_by_name_is_case_insensitive_and_preserves_missing_semantics` 覆盖大小写不敏感、保留原名与缺失返回 `None`。`pkg/util/dbutil/table_test.go` 的 `TestTable` 证明 Go 版在解析所得列集合上的命中/缺失意图；Go 文件没有针对模式校验的同目录测试。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳 `1791342965170`；`files --filter pkg/util/dbutil` 确认目标、模块、Go 对照和测试均已索引；`node --file pkg/util/dbutil/table.rs` 返回完整源码；`query` 定位三个主要符号；`callers`/`callees` 未给出同名函数边，因此调用关系以精确引用检索补证并剔除了 `IndexInfo::FindColumnByName` 的同名误关联。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰有 11 个固定二级标题，并人工复核没有把 Go 侧调用链描述为 Rust 已接线事实。
