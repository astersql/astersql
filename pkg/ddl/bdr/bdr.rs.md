# [`pkg/ddl/bdr/bdr.rs`](./bdr.rs)

## 文件定位

本文件是 `astersql-ddl-bdr` crate 的核心策略实现，回答“给定 BDR（Bidirectional Replication，双向复制）角色与 DDL 形态，是否应拒绝该操作”。crate 入口 `pkg/ddl/bdr/lib.rs` 将本模块的三个公开函数重新导出；`pkg/ddl/bdr/Cargo.toml` 表明其直接依赖仅是 parser AST、parser types 与 meta model 三个工作区 crate。

它位于 DDL job 持久化之前的策略边界，而不是 DDL job、schema state、reorg 或 schema-version 同步的执行器。Rust 生产链中，`pkg/session/runtime/normal_ddl_submit.rs`、`crossks_job_submit.rs` 和 `system_session.rs` 把字符串角色转换为 `ast::BDRRole`，再通过 `IsDenied` 决定 job submission 是否允许继续。因此这里不创建任务、不修改元数据，也不负责回滚。

## 核心职责

- `IsAddColumnDenied`：对 Primary 角色的新增列选项做更细粒度检查，只允许默认可空、显式 `NULL`、仅默认值、`NOT NULL + DEFAULT`，并把 `COMMENT` 与 `GENERATED` 视为不影响该组合计数的选项。
- `IsModifyColumnDenied`：对 Primary 角色的改列做检查，要求 `FieldType::Equal`，且选项只能是单独 `DEFAULT` 或 `DEFAULT + COMMENT`。
- `IsDenied`：依据 `model::ActionBDRMap` 执行动作级矩阵判定，并对 Primary 下新增唯一索引/主键追加参数级特判。
- 对 Primary/Secondary 的未知动作采取拒绝优先（fail closed）；对 `None` 和 Rust 为兼容 Go 未知字符串角色而增加的 `Unknown` 则完全绕过 BDR 拦截。

本文件只计算布尔结果，没有副作用。调用方负责把 `true` 转成用户可见错误并停止提交。

## 主要符号

### `pub fn IsAddColumnDenied(role, options) -> bool`

输入为 `ast::BDRRole` 与 `&[ast::ColumnOption]`。非 Primary 在第一个分支直接返回 `false`。Primary 路径扫描 `ColumnOption.Tp`，记录 `Null`、`NotNull`、`DefaultValue`，并用至多为 1 的 `comment`、`generated` 从选项总数中扣除这两类非约束选项。最终只有源码第 63–67 行列出的四种有效组合放行，其余返回 `true`。

### `pub fn IsModifyColumnDenied(role, newFieldType, oldFieldType, options) -> bool`

非 Primary 直接放行。Primary 首先调用 `types::FieldType::Equal`；该比较不仅比较 MySQL 类型，还覆盖长度（按类型规则可能忽略）、decimal、charset、collation、unsigned 标志和枚举元素，定义见 `pkg/parser/types/field_type.rs`。类型不等立即拒绝；类型相等时只放行一个 `DefaultValue`，或恰好两个且同时含 `DefaultValue`、`Comment` 的选项集合。

### `pub fn IsDenied(role, action, args) -> bool`

`action` 是 `model::ActionType`，当前为动作编号类型；`args: Option<&dyn Any>` 模拟 Go 的 `model.JobArgs` 动态参数。Primary/Secondary 都先查 `model::ActionBDRMap`：缺项即拒绝。Primary 放行 `SafeDDL` 和 `UnmanagementDDL`；Secondary 仅放行 `UnmanagementDDL`。Primary 的 `ACTION_ADD_INDEX` / `ACTION_ADD_PRIMARY_KEY` 在提供参数时会下转为 `model::ModifyIndexArgs`，若首个 `IndexArgs` 的 `Unique` 为真则拒绝。`None | Unknown` 不查表、不读参数，直接放行。

## 执行流程

1. session/job-submit 层读取 BDR 角色字符串，映射成 `Primary`、`Secondary`、`None` 或 `Unknown`。
2. 通用 job 提交路径把 job type 编号传给 `IsDenied`。普通路径见 `normal_ddl_submit.rs::Bdr::is_denied`，跨 keyspace 路径见 `crossks_job_submit.rs::CrossKSBDRPolicy::is_denied`，table-mode 路径见 `system_session.rs::TableModeBdrPolicy::is_denied`。
3. `IsDenied` 按角色选择策略：Primary 查动作类别并执行唯一索引特判；Secondary 只接受不受管理动作；未配置/未知角色直接通过。
4. 返回值交还 `jobsubmit::BdrPolicy`；本文件不负责后续 job 入表、owner 调度或 worker 执行。
5. 对 ADD COLUMN / MODIFY COLUMN，更细的两个函数应在构造 DDL job 前检查 AST 与字段类型。Go 生产链已分别在 `pkg/ddl/executor.go:2676` 和 `pkg/ddl/modify_column.go:2333` 接线；当前 Rust 全仓搜索只发现测试调用，尚未发现对应生产接线，不能把 Go 的这一层细粒度保护描述为 Rust 已完整启用。

## 数据与状态

- 角色来自 `parser_ast::misc::BDRRole`。`Primary` 和 `Secondary` 启用限制；`None` 表示未启用；`Unknown` 用于保留 Go 字符串角色对未知值走 default 分支的行为。
- 动作分类来自 `pkg/meta/model/bdr.rs` 的惰性静态表 `ActionBDRMap`，它由 `BDRActionMap` 展平生成，类别包括 `SafeDDL`、`UnsafeDDL`、`UnmanagementDDL` 与 `UnknownDDL`。本文件不缓存或复制分类。
- 列检查只读取 `ColumnOption.Tp`，不读取默认表达式、注释文本或生成列表达式内容。
- 改列检查读取完整 `FieldType` 等价语义，而不只是类型码。
- `IsDenied` 只有在 Primary、新增索引/主键且 `args` 为 `Some` 时读取 `ModifyIndexArgs.IndexArgs[0].Unique`；传 `None` 时仅执行动作类别判定。
- 三个函数都不修改输入，不持有跨调用状态。

## 依赖与调用关系

上游生产调用者（RustCodeGraph 文件级反向引用与 `rg` 共同核验）：

- `pkg/session/runtime/normal_ddl_submit.rs::Bdr::is_denied` → `IsDenied`
- `pkg/session/runtime/crossks_job_submit.rs::CrossKSBDRPolicy::is_denied` → `IsDenied`
- `pkg/session/runtime/system_session.rs::TableModeBdrPolicy::is_denied` → `IsDenied`

当前没有找到 Rust 生产代码调用 `IsAddColumnDenied` 或 `IsModifyColumnDenied`；它们由 `bdr_test.rs` 与 `migration_aster_unit_test.rs` 覆盖。Go 对应生产调用在 `pkg/ddl/executor.go` 与 `pkg/ddl/modify_column.go`。

下游依赖为：`parser-ast` 提供角色和列选项；`parser-types` 提供 `FieldType::Equal`；`meta-model` 提供动作编号、`ActionBDRMap`、类别常量及 `ModifyIndexArgs`。`std::any::Any` 是索引参数动态分派的唯一标准库机制。RustCodeGraph 能索引三个函数及文件引用，但本次对函数节点执行 `callers`/`callees` 返回空边，因此函数级调用关系由上述直接引用补证，而未把空图误写为“没有调用者”。

## 错误处理与边界

- API 以 `bool` 表达策略，不返回错误详情；具体错误构造属于调用层。
- Primary/Secondary 查询不到动作时返回 `true`，避免新动作未分类时被意外放行；`ActionBDRMap` 中 `UnsafeDDL`、`UnknownDDL` 也不会命中放行条件。
- `None`/`Unknown` 对所有动作返回 `false`，包括未知动作；这是与 Go 默认分支一致的兼容行为，不是保守拒绝。
- `IsDenied` 的索引参数存在强前置条件：若相关动作传入 `Some`，动态值必须确为 `ModifyIndexArgs`，否则 `expect` panic；其 `IndexArgs` 还必须非空，否则 `[0]` panic。调用方应只传经过对应 job 参数解码的非空结构。
- `IsAddColumnDenied` 按“选项数量 + 标志”判断。重复选项或额外未知选项会增加有效数量并趋向拒绝；`COMMENT`/`GENERATED` 各只扣一次，即使重复也不会全部忽略。
- `IsModifyColumnDenied` 要求选项数恰好匹配，重复 `DEFAULT`、重复 `COMMENT` 或任何额外选项均拒绝。
- `FieldType::Equal` 有自己的兼容规则，例如 `VARCHAR`/`VARSTRING` 可视为同类，不能把这里的“类型相等”简单理解为结构体逐字段完全相等。

## 并发与资源生命周期

三个入口都是同步、只读、无分配所有权转移的纯判定函数；借用仅持续到函数返回，不启动线程、任务或通道，不获取锁，也不接触事务和 I/O。

唯一共享状态是 meta-model 中通过 `std::sync::LazyLock` 构造的 `ActionBDRMap`。其初始化由标准库保证并发安全，初始化后只读。本文件没有清理阶段或资源释放责任。DDL job 的持久化、owner 生命周期、取消/回滚、schema version 同步与 reorg 均发生在本文件之外。

## 与 Go 版本的对应关系

Rust 三个函数逐一对应 `pkg/ddl/bdr/bdr.go` 的同名函数，主要控制流、允许组合、动作分类与唯一索引特判保持一致：

- Go `[]*ast.ColumnOption` 对应 Rust `&[ast::ColumnOption]`；Rust 消除了 nil 元素可能性，但判定仍只读 `Tp`。
- Go 按值传 `FieldType` 并对旧值取地址；Rust 直接借用新旧 `FieldType`，调用同语义的 `Equal`。
- Go `model.JobArgs` 通过类型断言取得 `*ModifyIndexArgs`；Rust 使用 `Option<&dyn Any>` 与 `downcast_ref`。两者类型错误都会失败，Rust 明确 panic 消息。
- Go 的字符串型 `BDRRole` 会让任意非 primary/secondary 值进入 default 放行；Rust 用显式 `Unknown` 复现该行为。
- Go 的细粒度 ADD/MODIFY COLUMN 检查已有生产接线；Rust 当前仅确认通用 `IsDenied` 已接入 session runtime。后续补齐 Rust DDL 构造链时必须复用这两个函数，而不是复制规则。

对照测试为 `pkg/ddl/bdr/bdr_test.go`；Rust 的 `pkg/ddl/bdr/bdr_test.rs` 复刻表驱动矩阵及 V1/V2 job 参数往返，`migration_aster_unit_test.rs` 还覆盖完整类别集合、未知动作和 `Unknown` 角色。

## 扩展指南

- 新增 DDL 动作时，先在 `pkg/meta/model/bdr.rs::BDRActionMap` 明确分类，并同步其独立测试；否则 Primary/Secondary 会因 `ActionBDRMap` 缺项而拒绝。随后扩充 `bdr_test.rs` 和 Go `bdr_test.go` 的角色矩阵。
- 新增参数相关例外时，优先在 `IsDenied` 的动作分支内做窄化检查；必须定义 `args` 的具体类型、缺失参数行为和空集合行为，避免新增 panic 路径。若 jobsubmit 生产调用需要触发参数级规则，还需把解码后的参数传入，而不是继续固定传 `None`。
- 调整 ADD COLUMN 规则应修改 `IsAddColumnDenied` 并同步 `ADD_COLUMN_DENIED_CASES`；调整 MODIFY COLUMN 规则应修改 `IsModifyColumnDenied` 并同步对应测试。测试逻辑继续放在独立 `bdr_test.rs`，不要内嵌到生产文件。
- 若要宣称 Rust 与 Go BDR 防护完整等价，必须先为两个细粒度函数补齐 Rust 生产调用点，并验证系统 schema 例外等调用层语义；本文件自身不能替代这些接线。
- 性能上三个函数为 O(n)：列函数按选项数扫描，动作查表为均摊 O(1)。扩展时应避免在每次提交上构造新映射或执行 I/O。

## 验证依据

- 目标源码：`pkg/ddl/bdr/bdr.rs`，RustCodeGraph `node --file` 确认 174 行及三个公开函数 `IsAddColumnDenied`、`IsModifyColumnDenied`、`IsDenied`。
- 图查询：RustCodeGraph `status` 显示索引含本文件；`files --filter pkg/ddl/bdr` 列出 Rust/Go 实现与测试；文件节点给出四个 Rust 引用文件。三个函数的 `callers`/`callees` 查询为空，故使用直接引用核验并在文中披露此限制。
- crate 与类型来源：`pkg/ddl/bdr/Cargo.toml`、`pkg/ddl/bdr/lib.rs`、`pkg/meta/model/bdr.rs`、`pkg/parser/types/field_type.rs`。
- Rust 上游：`pkg/session/runtime/normal_ddl_submit.rs`、`pkg/session/runtime/crossks_job_submit.rs`、`pkg/session/runtime/system_session.rs`。
- Go 对照与生产接线：`pkg/ddl/bdr/bdr.go`、`pkg/ddl/executor.go:2676`、`pkg/ddl/modify_column.go:2333`。
- 独立测试：`pkg/ddl/bdr/bdr_test.rs`、`pkg/ddl/bdr/migration_aster_unit_test.rs`、`pkg/ddl/bdr/bdr_test.go`。测试覆盖角色矩阵、安全/不安全/不受管控/未知动作、列选项组合、字段类型变化、唯一/非唯一索引、Job V1/V2 参数往返和未知角色。
- 本任务为纯文档分析，按计划不运行 Cargo；结构检查用于确认目标文件存在且恰有十一个固定二级标题。
