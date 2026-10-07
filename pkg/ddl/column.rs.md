# `pkg/ddl/column.rs`

## 文件定位

[`column.rs`](column.rs) 是 `astersql-ddl` crate 中公开的列元数据与纯内存操作模块；`pkg/ddl/lib.rs` 通过 `pub mod column` 暴露它，crate 边界由 `pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"` 确定。它不直接提交 DDL job、写系统表、推进 owner/worker，也不进行行数据回填；它提供一套简化的 `TableInfo`/`ColumnInfo`/`IndexInfo` 模型和列操作原语，供更高层的 `add_column.rs`、`modify_column.rs` 以及测试调用。

从完整 DDL 链路看，本文件位于“DDL 动作已被解析后、持久化或执行前后的元数据计算”位置。真实的状态推进由上层模块组织：例如 `start_add_column` 调用本文件完成列数检查和初始挂载，`advance_add_column` 在列变为 `Public` 时调用本文件计算目标位置；`apply_modified_column` 调用本文件同步索引引用并移动列。`pkg/ddl/doc.go` 描述的全局 schema 版本同步、job 持久化和集群 owner 机制不在本文件内实现。

## 核心职责

1. 定义列 DDL 所需的最小元数据模型：`SchemaState`、`ColumnPosition`、`ColumnKind`、`FieldType`、`DefaultValue`、`ColumnInfo`、`IndexColumn`、`IndexInfo`、`ForeignKeyInfo` 和 `TableInfo`。
2. 维护表内结构不变量：列 ID 由 `max_column_id` 单调分配；`columns[i].offset == i`；新列先追加在末尾并进入 `SchemaState::None`。
3. 为 ADD/MODIFY/DROP COLUMN 提供位置、索引引用、可删除性和列数上限等校验或变换。
4. 提供 AUTO_RANDOM 位宽检查、外键查找、表达式索引隐藏列名还原等无 I/O 辅助函数。
5. 用 `ColumnError` 把本层能够判定的失败压缩成稳定的结构化错误，由上层映射成具体 DDL 错误。

本文件不是完整 Go `pkg/ddl/column.go` 的一比一移植：Go 文件还包含 worker、job 状态转换、事务、回填、错误码和持久化逻辑；Rust 文件只覆盖可独立表达的元数据子集。

## 主要符号

- `SchemaState`：按派生的 `Ord` 顺序表示 `None -> DeleteOnly -> WriteOnly -> WriteReorganization -> Public`。`column_test.rs::test_write_data_write_only_mode` 依赖“`WriteOnly` 及以后可接收新写入”的顺序比较。该枚举没有 Go `SchemaState` 的所有状态，因此不能作为完整 Go 状态集合使用。
- `ColumnPosition::{None, First, After(String)}`：表达未移动、首列和指定 Public 列之后三种位置。
- `FieldType` / `ColumnKind`：保存类型族、长度、精度、字符集、排序规则和标志。`FieldType::integer` 构造 `INT(11)` 风格默认值。
- `ColumnInfo::new`：将名字 ASCII 小写化，并初始化为 ID 0、offset 0、`State::None`、无默认值/生成列/变更依赖的列。
- `TableInfo::new`：创建默认 `utf8mb4`/`utf8mb4_bin` 的空表；`TableInfo::move_column_info` 检查源/目标边界，移动后重写全部 offset。
- `allocate_column_id`：递增 `max_column_id` 后返回新值，不负责溢出检测。
- `init_and_add_column_to_table`：分配 ID，强制新列回到 `None`，以表尾下标设置 offset，再追加到 `columns`。
- `check_after_position_exists` 与 `locate_offset_to_move`：前者只检查 `AFTER` 名称存在；后者要求目标列处于 `Public`，并处理“先移除当前列会使目标偏移左移”的差一逻辑。
- `update_index_column`：把索引列名称和 offset 指向 changing 列；非可做前缀索引的类型，或前缀长度大于等于新字段长度时，清除 `length`。可保留前缀的类型包括 `Varchar`、`String` 和四种 Blob 家族。
- `list_indices_with_column` / `ensure_column_droppable` / `remove_column_and_single_indices`：区分可随列删除的普通单列索引，与禁止直接删列的主键、列存索引和联合索引；实际删除后返回索引 ID 并压缩列 offset。
- `build_elements`：生成先列后索引的 `(id, "column"|"index")` 列表。RustCodeGraph/`rg` 未发现当前 Rust 生产调用者，属于已公开但尚未接入生产流程的辅助 API。
- `check_add_column_too_many_columns`：比较调用者传入的数量与限制；限制值不在本文件读取全局配置。
- `check_new_auto_random_bits`：以饱和减法计算 `64 - shard_bits - range_bits`，比较当前 ID 实际占用位数；当前 Rust 生产代码中未发现调用者。
- `get_column_foreign_key_info`：返回第一个按精确字符串匹配引用列名的外键；当前 Rust 生产代码中未发现调用者。
- `expression_index_origin_name`：去掉可选 `_V$_` 前缀，再去掉最后一个下划线后的后缀；当前 Rust 生产代码中未发现调用者。

## 执行流程

新增列主流程由 `pkg/ddl/add_column.rs` 串接：

1. `start_add_column` 先构造 `ColumnInfo`。
2. 它调用 `check_add_column_too_many_columns(table.columns.len() + 1, column_limit)`，超限时映射为 `AddColumnError::TooManyColumns`。
3. 它调用 `init_and_add_column_to_table`，把新列以新 ID、`None` 状态和表尾 offset 写入 `TableInfo`。
4. `advance_add_column` 在上层推进 `None -> DeleteOnly -> WriteOnly -> WriteReorganization -> Public`；只有即将进入 `Public` 时调用 `locate_offset_to_move`，随后由 `TableInfo::move_column_info` 落实 FIRST/AFTER 顺序并刷新所有 offset。

修改列的直接流程由 `pkg/ddl/modify_column.rs::apply_modified_column` 串接：先用 `locate_offset_to_move` 算最终位置，替换列定义，再遍历索引并对旧列名匹配项调用 `update_index_column`，同步外键列名，最后调用 `move_column_info`。重组型修改在 `initialize_changing_objects` 中创建隐藏 changing 列/索引时也调用 `update_index_column`。

删除辅助流程集中在 `remove_column_and_single_indices`：按 ID 找列，调用 `ensure_column_droppable`，移除引用该列的普通单列索引并收集 ID，移除列，最后重新编号剩余列 offset。当前搜索未发现该函数的 Rust 生产调用者；其行为由 `column_test.rs` 直接验证，不能据此推断生产 DROP COLUMN 已经接入它。

## 数据与状态

`TableInfo` 是本文件所有变换的聚合根。关键不变量是：`max_column_id` 至少不小于已分配列 ID；`columns` 的向量顺序就是逻辑列顺序；每个 `ColumnInfo.offset` 应与向量下标一致。`move_column_info` 和删除函数在变换后全量重写 offset，`init_and_add_column_to_table` 则利用追加前的长度设置新列 offset。

`ColumnInfo` 同时保存稳定属性和在线变更中间态：`state` 控制可见阶段；`prevent_null_insert` 支持 NULL 到 NOT NULL 的过渡；`origin_default_value` 保存变更期间旧数据语义；`change_dependency_offset` 记录 changing 列依赖的旧列位置；`generated_*` 与 `dependencies` 描述生成列。该结构是值对象，没有内部同步或惰性加载。

名称处理并不完全统一：`ColumnInfo::new` 仅执行 ASCII 小写，其他构造者可直接写入字符串；多数查找使用精确相等而非 `eq_ignore_ascii_case`。因此调用者若绕过 `new`，必须自行保持规范化，否则位置、索引或外键查找可能不匹配。

`check_new_auto_random_bits` 对 `current_id == 0` 得到 0 个已用位；当 `shard_bits + range_bits > 64` 时通过饱和减法把可用自增位降为 0。它只做算术判断，不像 Go 版本那样先从持久化 allocator 分配/读取最新 ID。

## 依赖与调用关系

本文件源码只直接依赖标准库 `std::collections::HashSet`，不直接使用 `pkg/ddl/Cargo.toml` 中列出的外部 crate。它通过本 crate 的公开模块边界被其他 DDL 文件使用。

已核实的生产调用边包括：

- `add_column.rs::start_add_column -> check_add_column_too_many_columns -> init_and_add_column_to_table -> allocate_column_id`。
- `add_column.rs::advance_add_column -> locate_offset_to_move -> TableInfo::move_column_info`。
- `modify_column.rs::apply_modified_column -> locate_offset_to_move / update_index_column / TableInfo::move_column_info`。
- `modify_column.rs::initialize_changing_objects -> update_index_column`。

RustCodeGraph `status` 显示索引包含 11,467 个文件、307,296 个节点，`files --filter pkg/ddl/column.rs` 识别该文件 64 个符号并报告被 41 个文件使用。其宽泛 `explore` 结果能列出上述 DDL 邻接符号，但对精确函数 ID 执行 `callers`/`callees` 未返回边，因此本文用索引源码视图与 `rg` 的精确引用交叉核验，不把缺失图边解释为“没有调用”。

## 错误处理与边界

`ColumnError` 覆盖同名/缺失列、唯一列不可删、受保护索引引用、offset 越界、列数超限和 AUTO_RANDOM 溢出。它只派生比较与调试能力，没有实现展示文本或标准错误 trait；上层模块通常把它映射为自己的领域错误。

重要边界如下：

- `move_column_info` 的 `to` 必须小于当前列数，不能用 `len()` 表示追加；源或目标越界返回 `InvalidOffset`，不修改表。
- `locate_offset_to_move` 的 `After` 只接受 `Public` 目标列；`check_after_position_exists` 则仅检查名称存在，两者验证强度不同。
- `ensure_column_droppable` 只阻止表中“当前恰有一列”的删除，并检查主键、列存或联合索引；外键、生成列依赖和 check constraint 必须由别处验证。
- `remove_column_and_single_indices` 先完成所有可失败检查，再开始修改，因此这些错误路径不会留下部分删除；成功后返回的索引 ID 供更高层清理物理数据。
- `update_index_column` 在 `flen <= prefix length` 时清除前缀，这与 Go `UpdateIndexCol` 的条件一致；相等长度也被视为无需前缀。
- `expression_index_origin_name` 对没有下划线后缀的输入原样返回；它不验证后缀是否为数字。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接。所有变更都通过 `&mut TableInfo` 或 `&mut IndexColumn` 在调用者持有的对象上同步完成，生命周期由 Rust 借用规则约束。

这不等于完整 DDL 无并发问题。Go `checkNewAutoRandomBits` 会先对 allocator 执行 `Inc(1)`，以避免 DML 并发更新造成检查过时；Rust `check_new_auto_random_bits` 只接收已给定的 `current_id`，其新鲜性与同步责任完全属于调用者。类似地，schema version、job checkpoint、owner failover 和回填资源都由上层 DDL 框架管理，本文件没有持久化点，进程失败后也没有独立恢复能力。

## 与 Go 版本的对应关系

- Rust `init_and_add_column_to_table` 对应 Go `InitAndAddColumnToTable`：都分配列 ID、强制 `StateNone`、以表尾 offset 追加；Rust 返回 ID，Go 返回列指针。
- Rust `check_after_position_exists`、`locate_offset_to_move` 分别对应 Go `CheckAfterPositionExists`、`LocateOffsetToMove`。FIRST/AFTER 和当前列位于目标前后的 offset 算法一致；Rust 不包含 Go 对未知位置枚举的 default 分支，因为 Rust enum 已封闭取值。
- Rust `update_index_column` 对应 Go `UpdateIndexCol`，保留了可前缀类型及 `flen <= length` 时清除前缀的核心语义。`column_test.rs::test_modify_column_with_index` 特别验证 Blob 的合法前缀仍被保留。
- Rust `list_indices_with_column` 与 `ensure_column_droppable` 分别对应 Go `listIndicesWithColumn` 和 `isColumnCanDropWithIndex` 的核心筛选；Rust 将“表至少一列”也纳入后者，并使用本地错误枚举。
- Rust `build_elements` 对应 Go `BuildElements` 的“列元素在前、相关索引在后”，但 Rust 用字符串类型标记，不是 Go `meta.Element`/`TypeKey`。
- Rust `get_column_foreign_key_info`、`allocate_column_id` 和列数检查分别对应 Go `GetColumnForeignKeyInfo`、`AllocateColumnID`、`checkAddColumnTooManyColumns`；Rust 的列数限制由参数传入，Go 从全局配置原子读取。
- Rust `check_new_auto_random_bits` 只移植最终位宽比较；Go 版本还选择 allocator、处理 AUTO_INCREMENT 转换、先分配 ID 防并发，并生成包含列名和可用 shard bits 的数据库错误。
- Rust 文件没有移植 Go 文件中的 `worker` 方法、job 解码/取消、回填事务、schema version 更新和数据扫描。扩展时应继续复用 Rust 现有上层状态机，而不是仅凭同名函数假定完整等价。

## 扩展指南

新增列元数据字段时，应先判断它属于稳定 schema 定义还是在线变更中间态，并同步检查 `ColumnInfo::new`、上层构造/克隆路径以及独立测试。新增列类型若影响前缀索引能力，必须修改 `ColumnKind` 和 `update_index_column` 的 allowlist，并在 `pkg/ddl/column_test.rs` 增加保留与清除前缀的成对用例。

修改列位置或删除行为时，要守住 `columns[i].offset == i`：优先复用 `TableInfo::move_column_info`，成功路径后检查所有 offset；不可把 Rust 单元测试嵌入生产文件。新增删除限制（外键、生成列、check constraint）应先确认现有专门模块是否已经校验，避免在本层重复或产生错误顺序差异。

若要接入当前未被生产代码调用的辅助函数，应同时建立真实上层调用边，并核对 Go 版本上下文而不只复制函数体。例如接入 AUTO_RANDOM 检查前必须解决 allocator 新鲜性和并发保护；接入 `build_elements` 前必须确认 Rust job 元素类型和持久化编码，而不能长期依赖 `&'static str` 占位表示。

建议同步测试位置：本文件的直接单元测试放在独立的 `pkg/ddl/column_test.rs`；ADD 状态流修改同步 `add_column_test.rs`，MODIFY/索引修改同步 `modify_column_test.rs`，更高层用户可见行为再选择现有 DDL 集成测试。兼容风险主要是 Go 错误语义、大小写规范化和状态集合差异；性能风险主要来自移动/删除后对全部列的 O(n) offset 重写，以及索引/外键线性扫描。

## 验证依据

- 源码与符号：RustCodeGraph `node --file pkg/ddl/column.rs --offset 1 --limit 260`、`--offset 261 --limit 260` 和 `--offset 521 --limit 260`，覆盖全部 522 行；`query <symbol> --kind function --json` 核实主要函数签名。
- 索引与关系：`rustcodegraph status`、`files --filter pkg/ddl/column.rs`、`explore "pkg/ddl/column.rs symbols callers callees column DDL"`；再用生产 Rust 文件的精确引用搜索确认 `add_column.rs` 与 `modify_column.rs` 的调用边。
- crate/模块边界：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`；目标源码只直接导入 `std::collections::HashSet`。
- DDL 契约：`pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`，并以实际 Rust/Go 源码验证其与本文件相关的部分。
- Rust 生产入口：`pkg/ddl/add_column.rs`、`pkg/ddl/modify_column.rs`。
- Rust 独立测试：`pkg/ddl/column_test.rs`；补充搜索到 `db_change_test.rs` 对 `ensure_column_droppable`、`db_integration_test.rs` 对 `move_column_info` 的使用。
- Go 对照：`pkg/ddl/column.go` 中 `InitAndAddColumnToTable`、`CheckAfterPositionExists`、`UpdateIndexCol`、`LocateOffsetToMove`、`BuildElements`、`checkNewAutoRandomBits`、`listIndicesWithColumn`、`GetColumnForeignKeyInfo`、`AllocateColumnID` 和 `checkAddColumnTooManyColumns`；`pkg/ddl/modify_column_test.go` 验证 Go `BuildElements` 的使用场景。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 个二级标题的结构命令和人工事实复核验收。
