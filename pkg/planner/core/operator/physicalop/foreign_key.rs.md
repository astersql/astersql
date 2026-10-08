# `pkg/planner/core/operator/physicalop/foreign_key.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate。该 crate 的 [`Cargo.toml`](Cargo.toml) 将 `lib.rs` 指定为根模块并声明 `autotests = false`；[`lib.rs`](lib.rs) 以 `pub mod foreign_key` 公开本模块，并在 `#[cfg(test)]` 下单独装配 `foreign_key_test.rs`。因此这里定义的类型可被同 crate 的 DML 物理计划持有，也可由上层 crate 通过模块路径访问。

文件位于 INSERT/UPDATE/DELETE 规划与外键执行之间：它把表和外键元数据转换成两类规划期描述——`FkCheck` 表示父行必须存在或子行必须不存在，`FkCascade` 表示 `ON DELETE`/`ON UPDATE` 的 `CASCADE` 或 `SET NULL` 动作。[`physical_common_plans.rs`](physical_common_plans.rs) 的 `Insert`、`Update`、`Delete` 已有 `fk_checks` 与 `fk_cascades` 字段承接这些结果；[`plan_clone_generated.rs`](plan_clone_generated.rs) 会拒绝缓存携带任一外键描述的 DML 计划。

当前 Rust 接线必须与 Go 主线明确区分。RustCodeGraph 对 `build_on_insert_fk_triggers`、`build_on_update_fk_triggers`、`build_on_delete_fk_triggers` 和 `get_update_columns_info` 未返回生产调用者，仓库搜索只找到独立 Rust 测试直接调用前两个入口。也就是说，文件后半是可编译、可测试、公开的简化移植模型，但尚不能据此声称完整 Rust DML 规划主链已经生成这些触发器。文件前半约 660 行的 `//` 注释保留了更完整的拟移植控制流，注释不是可执行实现。

## 核心职责

- 用 `ReferentialAction`、`CascadeType`、`ForeignKeyInfo`、`TableInfo` 表达生成触发器所需的最小外键和表元数据。
- 用 `FkCheck` 描述存在性检查方向、目标表/列和失败文本，并提供 EXPLAIN 风格的访问对象、算子信息与内存估算。
- 用 `FkCascade` 描述级联作用的子表、父子列映射、触发 DML 类型和引用动作，并提供同类展示与内存估算。
- 为 INSERT/REPLACE、UPDATE、DELETE 分别筛选适用外键，再将父表侧动作分流为限制检查或级联，将子表侧修改转换为父行存在性检查。
- 提供 `get_update_columns_info`，按多个表在联合 Schema 中的半开位置区间回填列名。

本文件不读取 InfoSchema、不解析真实 `model::FKInfo`、不寻找支持索引、不创建 `BasePhysicalPlan`、不执行检查或级联，也不管理执行期生成的级联子计划。这些都是同路径 Go 实现已承担、但当前可执行 Rust 部分尚未承担的职责；文件顶部的大段注释只能作为迁移意图和 Go 控制流索引，不能替代现状证据。

## 主要符号

- `ReferentialAction`：公开枚举，含 `Restrict`、`NoAction`、`Cascade`、`SetNull`、`SetDefault`；默认值是 `Restrict`。私有 `as_sql` 只为算子说明生成稳定的大写 SQL 文本。
- `CascadeType::{OnDelete, OnUpdate}`：标记级联由父行删除还是被引用列更新触发，没有默认值。
- `ForeignKeyInfo`：最小外键元数据。`child_table_id`/`parent_table_id` 与两组列建立映射；`on_delete`/`on_update` 保存动作；`public` 决定动作是否生效；`enabled` 是三个公开构造入口的统一过滤门槛。
- `TableInfo`：最小表元数据。`foreign_keys` 表示本表作为子表的外键，`referred_foreign_keys` 表示本表作为父表时被其他表引用的外键；`columns` 当前未被触发器构造函数读取。
- `FkCheck`：检查描述。`check_exist = true` 表示修改子表时父表对应行必须存在；`false` 表示修改父表时子表对应行必须不存在。`access_object` 只显示数值表 ID；`operator_info` 显示外键名和方向；`memory_usage` 计算结构体浅层大小与 `columns` 中字符串的 capacity，不含 `name`、`failed_error` 等其他动态分配。
- `FkCascade`：级联描述。`operator_info` 由 `cascade_type` 选择 `on_delete`/`on_update`，由 `action.as_sql()` 输出动作；`memory_usage` 仅返回结构体浅层大小。
- `build_on_insert_fk_triggers(&TableInfo, bool)`：为所有已启用的本表外键生成子表侧检查；`replacing = true` 时还按删除父行语义处理所有已启用的被引用外键。
- `build_on_update_fk_triggers(&TableInfo, &BTreeSet<String>)`：仅处理与更新列有交集的外键；先生成父表侧触发器，再追加子表侧存在性检查。
- `build_on_delete_fk_triggers(&TableInfo)`：对所有已启用的被引用外键按删除动作生成限制检查或级联。
- `Trigger` 与 `build_referred_trigger`：私有分流层，保证一个父表侧外键恰好生成 `Check` 或 `Cascade` 之一。
- `build_child_check`：私有子表侧转换，目标是 `parent_table_id`/`parent_columns`，固定 `check_exist = true`。
- `contains_any`：私有、ASCII 大小写不敏感的列集合交集判断。
- `get_update_columns_info`：公开位置映射辅助函数，把 `BTreeMap<table_id, columns>` 填入固定长度的 `Vec<Option<String>>`。

源码没有宏、trait 实现、模块级常量或条件编译项；测试的条件编译发生在 `lib.rs`，不是本文件内部。

## 执行流程

INSERT/REPLACE 路径如下：

1. `build_on_insert_fk_triggers` 遍历 `table.foreign_keys`，跳过 `enabled = false` 的项，对每个其余项调用 `build_child_check`。
2. 子表检查指向父表 ID 和父列，要求对应父行存在。
3. 普通 INSERT 到此结束。REPLACE 还会模拟“先删除冲突父行”的影响，遍历 `referred_foreign_keys` 并调用 `build_referred_trigger(..., OnDelete)`。
4. 私有 `Trigger` 将每个结果分别追加进 checks 或 cascades，二者不会同时生成。

UPDATE 路径如下：

1. 先遍历 `referred_foreign_keys`；外键必须启用，且 `updated` 与 `parent_columns` 经 `contains_any` 判断有交集。
2. 命中的父表侧外键按 `OnUpdate` 进入 `build_referred_trigger`。
3. 再遍历 `foreign_keys`；外键必须启用，且更新列与 `child_columns` 有交集，命中后调用 `build_child_check`。
4. 因此同一 UPDATE 可以同时产生父表侧限制/级联和子表侧存在性检查，返回顺序固定为“父表侧结果在前、子表侧检查在后”。

DELETE 路径只遍历已启用的 `referred_foreign_keys`，统一按 `OnDelete` 分流。

`build_referred_trigger` 先根据 `CascadeType` 选择配置的 `on_delete` 或 `on_update`；若 `public = false`，无条件覆盖成 `Restrict`。`Restrict`、`NoAction`、`SetDefault` 生成 `check_exist = false` 的 `FkCheck`；`Cascade`、`SetNull` 生成携带父子列映射的 `FkCascade`。该分组与 Go `buildOnDeleteOrUpdateFKTrigger` 的默认限制分支一致。

`get_update_columns_info` 先创建 `size` 个 `None`。每个 `(table_id, start, end)` 查找表列，最多取 `end.saturating_sub(start)` 列，并在 `start + offset < size` 时写入克隆列名；未知表、空/逆序区间和越过结果上界的部分都会被静默忽略。

## 数据与状态

所有 API 都是同步的纯值转换：输入通过共享引用读取，结果拥有克隆出的 `String` 和 `Vec`，没有全局可变状态或对输入的回写。`BTreeSet`/`BTreeMap` 带来确定的集合和映射遍历顺序，但三个触发器构造函数的主要输出顺序仍继承输入 `Vec` 中的外键顺序。

关键不变量包括：

- 一个启用的子表外键在 INSERT 中总生成一个存在性检查；当前模型不检查外键元数据版本、父表是否存在或支持索引是否存在。
- 一个父表侧外键只生成检查或级联之一。非 Public 外键即使配置 `Cascade`/`SetNull` 也按 `Restrict` 生成检查。
- `SetDefault` 在 Rust 枚举中可表达，但按 Go 的 default 分支生成限制检查，并未实现设置默认值的级联。
- 父表侧检查指向 `child_table_id`/`child_columns` 且 `check_exist = false`；子表侧检查指向 `parent_table_id`/`parent_columns` 且 `check_exist = true`。
- `contains_any` 使用 `eq_ignore_ascii_case`，只提供 ASCII 不区分大小写语义；它不是完整的 TiDB `CIStr.L` Unicode/规范化模型。
- `get_update_columns_info` 不验证区间重叠；后处理的位置可覆盖先前写入值。它也不验证 `end <= size` 或列数是否恰好等于区间宽度。

`ForeignKeyInfo`、`TableInfo`、`FkCheck` 均可 `Default`；默认外键 `enabled = false`、`public = false`，因此不会被公开入口处理。`FkCascade` 没有 `Default`，因为其 `CascadeType` 没有默认语义。

## 依赖与调用关系

模块直接依赖只有标准库 `BTreeMap` 和 `BTreeSet`；虽然 crate 的 `Cargo.toml` 声明了 `base`、`model`、`mysql`、`table`、`property`、`plancodec` 等规划依赖，当前可执行代码没有直接使用它们。更完整的 Go 对应实现正是通过这些概念查询元数据、索引和表对象，顶部注释也保留了该拟移植结构。

当前已验证的 Rust 上游关系为：

- `lib.rs` 公开 `foreign_key` 模块，并接入独立 `foreign_key_test.rs`。
- `physical_common_plans.rs::{Insert, Update, Delete}` 持有 `Vec<FkCheck>` 与 `Vec<FkCascade>`，构成结果的数据接线。
- `plan_clone_generated.rs::{Insert, Update, Delete}::clone_for_plan_cache` 在任一外键向量非空时返回 `None`；`plan_clone_generated_test.rs` 覆盖检查与级联两类情况。
- `foreign_key_test.rs` 直接调用 `build_on_insert_fk_triggers` 与 `build_on_update_fk_triggers`。RustCodeGraph 和仓库文本搜索均未发现三个构造入口的生产调用点；`build_on_delete_fk_triggers`、`get_update_columns_info` 也没有当前外部调用证据。

文件内下游调用边由 RustCodeGraph 验证：INSERT 调用 `build_child_check` 和 `build_referred_trigger`；UPDATE 还调用 `contains_any`；DELETE 调用 `build_referred_trigger`；后者构造 `FkCheck` 或 `FkCascade`。当前实现不调用 executor、InfoSchema、存储层或事务层。

Go 生产主链则将同名方法实现为 `Insert`/`Update`/`Delete` 的方法，由规划阶段填充真实 DML 计划对象。该 Go 接线只能作为目标语义对照，不是 Rust 已接入主链的证据。

## 错误处理与边界

三个公开构造函数返回 `Result<..., String>`，但当前实际可执行分支都只构造 `Ok`；`build_referred_trigger` 也没有返回 `Err` 的分支。因此签名预留了可恢复错误通道，现状却不会验证元数据一致性或索引条件，也不会产生错误。

当前边界和缺口包括：

- `enabled = false` 静默跳过；这压缩了 Go 的 `FKInfo.Version < 1` 规则，但两者不是同一个字段，调用方必须正确映射。
- `public = false` 强制限制检查，防止非 Public 外键提前级联；独立测试已有覆盖。
- 空列向量仍会生成检查或级联；父子列数量不相等、列不存在、表 ID 无效也不会报错。
- 普通 INSERT 不处理被引用外键；REPLACE 才按删除语义处理。简化入口没有 Go 的 `ON DUPLICATE KEY UPDATE` 列集合参数，因此不能表达该分支。
- UPDATE 只按列名交集过滤，不传播存储生成列依赖；顶部注释与 Go `buildTbl2UpdateColumns` 展示了完整语义，但可执行代码尚未实现。
- Go 会在被引用表/子表缺失时静默跳过，会校验表模式、主键 handle、支持索引和外键列，并返回数据库/索引/列错误；Rust 当前模型既没有这些输入，也没有这些错误。
- `access_object` 只输出表 ID，不含 Go 的表名或索引名；`failed_error` 是普通字符串，不是带错误码和参数的 planner error。
- `get_update_columns_info` 用边界检查避免 panic，并对不存在的表静默跳过；Go 版本则按上游不变量直接索引表和目标切片，错误输入可能 panic。二者的失败行为不同。

## 并发与资源生命周期

本模块不创建线程、异步任务、锁、原子变量、通道、事务、网络请求或文件资源。所有函数只在调用栈上构造容器，返回值离开作用域后由 Rust 自动释放；没有显式清理或取消阶段。

不可变借用使同一 `TableInfo` 可被多个线程并发读取，前提是调用方所持类型满足 Rust 自动推导的 `Sync`；函数自身没有共享可变状态。返回结果与输入完全分离，因为外键名和列名都会克隆。

主要资源成本是线性遍历和字符串克隆。INSERT/DELETE 对相关外键向量各遍历一次；UPDATE 对两类外键各遍历一次，但 `contains_any` 对每个候选列又线性扫描 `BTreeSet` 并执行 ASCII 大小写比较，最坏复杂度约为“外键列数 × 更新列数”，没有利用 `BTreeSet` 的对数查找。`memory_usage` 只是与 Go 对齐的部分估算，不代表完整堆内存占用：`FkCheck` 漏计名称和错误字符串，`FkCascade` 漏计所有字符串/向量容量。

## 与 Go 版本的对应关系

直接对照文件是 [`foreign_key.go`](foreign_key.go)。主要对应关系如下：

- Rust `FkCheck` / `FkCascade` 对应 Go `FKCheck` / `FKCascade` 的规划意图，但 Rust 不嵌入 `BasePhysicalPlan`，不持有真实 `table.Table`、`table.Index`、`model.FKInfo`、`ReferredFKInfo`、列元数据或执行期 `CascadePlans`。
- Rust `access_object`/`operator_info` 保留展示格式的核心含义；Go 使用表名和可选索引名，Rust 只使用表 ID。Rust 未提供组合二者的 `explain_info`。
- `FkCheck::memory_usage` 保留“浅层加检查列动态占用”的方向，但以 `String::capacity` 近似 CIStr 占用；`FkCascade::memory_usage` 与 Go 一样只计算浅层结构。由于两侧结构不同，数值不能直接比较。
- Rust 三个 `build_on_*_fk_triggers` 对应 Go 的 DML 方法，但没有会话 `ForeignKeyChecks` 开关、InfoSchema、schema 名、多表 map，也不会直接写回 DML 计划。Rust UPDATE 只处理单个 `TableInfo`，返回平坦向量；Go UPDATE/DELETE 按 table ID 返回/保存映射。
- Rust `build_referred_trigger` 保留 Go 的核心分流：非 Public 强制 `RESTRICT`，`CASCADE`/`SET NULL` 走级联，其余动作走“不应存在子行”的检查。Rust 没有 Go 的陈旧元数据跳过、表模式检查、索引与列校验。
- Rust `build_child_check` 压缩了 Go `buildFKCheckOnModifyChildTable`、`buildFKCheck` 的结果形状，但没有父表查询、主键 handle 快路、索引选择、exclusive/primary 标记或标准错误。
- Rust `contains_any` 对应 Go `isMapContainAnyCols`，但 Rust 为调用者兼容大写输入而逐项执行 ASCII 不区分大小写比较；Go 依赖 `CIStr.L` 和已小写的 map key。
- Rust `get_update_columns_info` 对应 Go `GetUpdateColumnsInfo`，但把真实表/列对象替换为 ID/字符串，并额外进行缺表、区间宽度和结果上界保护。Rust 尚未实现 Go `buildTbl2UpdateColumns` 对 assignment 下标的映射以及存储生成列依赖传播。

文件顶部被注释的 Rust 草案比后半实现更接近 Go 全貌，包括 InfoSchema、索引选择、错误和 DML 方法；但它不参与编译，本文所有“当前支持”结论均以后半第 676 行起的可执行代码为准。

## 扩展指南

若只扩展当前简化模型，应按职责选择接入点：新增引用动作分流放在 `build_referred_trigger`；新增 DML 过滤条件放在对应 `build_on_*_fk_triggers`；修改子表存在性目标放在 `build_child_check`；修改联合 Schema 位置语义放在 `get_update_columns_info`。保持“一个父表侧外键只产生检查或级联之一”、非 Public 不提前级联、检查方向与目标表相配三项不变量。

若目标是接入真实 Rust DML 规划主链，不应继续用字符串和表 ID 桩代替完整语义。需要沿 Go 增量补齐会话外键开关、InfoSchema 查询、schema/table/column/index 类型、旧版本外键跳过、表模式校验、主键 handle 与支持索引选择、标准 planner error、多表 UPDATE/DELETE 映射、ON DUPLICATE 分支和生成列依赖传播，并在真实 planner 调用点填充 `Insert`/`Update`/`Delete`。同时应评估是否替换而不是并存顶部注释草案，避免形成两套漂移的逻辑说明。

测试必须继续放在独立文件，不能写入生产 `.rs`。最低同步面是 [`foreign_key_test.rs`](foreign_key_test.rs)：补充 DELETE、禁用外键、`NoAction`/`SetNull`、空列、父子列不匹配、结果顺序和 `get_update_columns_info` 的缺表/重叠/越界用例。若改变 DML 持有或缓存规则，还要同步 [`plan_clone_generated_test.rs`](plan_clone_generated_test.rs)。接入真实主链后必须增加能证明调用点实际生成并消费这些节点的规划/SQL 集成测试，不能只保留对公开辅助函数的直接单测。

兼容性风险集中在 `enabled` 与 Go `Version` 的映射、ASCII 大小写与 `CIStr.L`、错误类型/文本及 `SET DEFAULT` 默认分支；正确性风险集中在遗漏索引与列校验、生成列传播和多表 UPDATE 分组；性能风险集中在 UPDATE 的嵌套大小写扫描、字符串克隆和不完整内存统计。

## 验证依据

本说明使用了以下直接证据：

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter .../foreign_key.rs` 确认目标文件；`node --file ... --offset 1/500` 读取完整 938 行；`query` 定位四个公开入口与 `build_referred_trigger`；`callees` 证明 INSERT → `build_referred_trigger`/`build_child_check`，UPDATE → 前两者及 `contains_any`，DELETE → `build_referred_trigger`，而 callers 查询未返回生产调用边。
- Rust 生产源码：[`foreign_key.rs`](foreign_key.rs) 的可执行部分；[`physical_common_plans.rs`](physical_common_plans.rs) 的 `Insert`、`Update`、`Delete` 外键字段；[`plan_clone_generated.rs`](plan_clone_generated.rs) 的三类 DML 缓存拒绝；[`lib.rs`](lib.rs) 的公开模块和独立测试装配。
- crate 与仓库契约：[`Cargo.toml`](Cargo.toml) 的 crate 名、根模块、依赖和 `package.metadata.porting.go-package`；最近的 `pkg/planner/core/base/doc.go` 要求 planner base 抽象避免依赖具体实现，本文件当前没有修改 base 接口。
- Rust 独立测试：[`foreign_key_test.rs`](foreign_key_test.rs) 覆盖子表修改检查父行存在、UPDATE 对父列的大小写不敏感匹配和级联、`SET DEFAULT` 走限制检查、非 Public `CASCADE` 强制限制；[`plan_clone_generated_test.rs`](plan_clone_generated_test.rs) 覆盖携带检查或级联的 DML 计划不可缓存。
- Go 对照：[`foreign_key.go`](foreign_key.go) 的 `FKCheck`、`FKCascade`、三个 `BuildOn*FKTriggers`、`GetUpdateColumnsInfo`、`buildTbl2UpdateColumns`、`buildOnDeleteOrUpdateFKTrigger`、`buildFKCheck`、`buildFKCascade`。同目录 Go 测试搜索未找到这些符号的直接单元测试，因此本文没有声称存在对应 Go 单测覆盖。

本任务是纯文档分析，按计划未运行 Cargo。结构验收使用任务指定命令确认本文存在且恰含十一个固定二级标题；人工复核重点是明确区分注释草案、当前可执行简化实现、DML 数据接线和尚未存在的生产调用链。
