# `pkg/meta/model/bdr.rs`

## 文件定位

[`bdr.rs`](bdr.rs) 是 `pkg/meta/model` 的 BDR（Bidirectional Replication，双向复制）元数据定义文件，同时承载一个通用的 TSO 物理时间转换函数。它不是 DDL 拒绝策略本身，而是为策略提供“DDL 动作属于哪一安全类别”的静态事实表。

编译边界并不直接由顶层 `pkg/meta/model/lib.rs` 声明：`pkg/meta/model/internal/group1/lib.rs` 以 `#[path = "../../bdr.rs"] mod bdr; pub use bdr::*;` 纳入本文件，形成 `astersql-meta-model-group1` 的公开 API；随后 `pkg/meta/model/lib.rs` 通过 `pub use ::group_1::*` 再导出为 `astersql-meta-model` 的根级 API。`pkg/meta/model/Cargo.toml` 只依赖四个内部 group crate；本文件实际使用的 `chrono` 依赖声明在 `pkg/meta/model/internal/group1/Cargo.toml`。

本文件没有条件编译项、trait 或业务对象实例。生产代码由一个值类型、四个类别常量、两个进程级惰性映射和一个纯转换函数组成。

## 核心职责

1. 用 `DDLBDRType` 和 `UnsafeDDL`、`SafeDDL`、`UnmanagementDDL`、`UnknownDDL` 表达 BDR 对 DDL 动作的四类静态标签。
2. 在 `BDRActionMap` 中维护类别到动作列表的权威分组，并从它派生 `ActionBDRMap`，支持按 `ActionType` 做常数时间查找。
3. 用 `TSConvert2Time` 提取 TiDB/PD TSO 的高位物理毫秒，生成 UTC 时间。这个职责与 BDR 分类没有数据依赖，只是与 Go 同路径文件保持一致而共处一处。

本文件只描述类别，不结合当前集群角色做准入决策。真正的 Rust 决策入口是 `pkg/ddl/bdr/bdr.rs::IsDenied`：Primary 放行 `SafeDDL` 与 `UnmanagementDDL`，Secondary 只放行 `UnmanagementDDL`，未登记动作在启用 BDR 的角色下按拒绝处理。

## 主要符号

- `pub struct DDLBDRType(pub &'static str)`：静态字符串的新类型包装。派生 `Clone`、`Copy`、`Debug`、`Eq`、`Hash`、`PartialEq`，因此既能作为 `HashMap` 键，也能低成本复制比较；公开元组字段允许调用者读取底层标签。它没有 `Display`、解析或序列化实现。
- `UnsafeDDL`：值为 `"unsafe DDL"`，当前包含 57 个动作，覆盖删除、截断、恢复、重命名、约束与多数结构性变更。
- `SafeDDL`：值为 `"safe DDL"`，当前包含 23 个动作，例如创建 schema/表、部分索引和列变更、视图与 TTL 变更。
- `UnmanagementDDL`：值为 `"unmanagement DDL"`，当前包含 9 个放置策略、脱敏策略和资源组动作；Go 注释将其定义为 CDC 不同步的 DDL。
- `UnknownDDL`：值为 `"unknown DDL"`，当前仅包含 `DEPRECATED_ACTION_ALTER_TABLE_ALTER_PARTITION`。
- `pub static BDRActionMap: LazyLock<HashMap<DDLBDRType, Vec<ActionType>>>`：正向权威表，共登记 90 个动作。首次访问时构造，之后只通过共享引用读取。
- `pub static ActionBDRMap: LazyLock<HashMap<ActionType, DDLBDRType>>`：反向查询表。初始化时遍历 `BDRActionMap`，复制动作和类别后 `collect` 成表。
- `pub fn TSConvert2Time(ts: u64) -> DateTime<Utc>`：丢弃低 18 位逻辑部分，以 `(ts >> 18)` 作为 Unix epoch 后的毫秒数，再调用 `DateTime::from_timestamp_millis`。

## 执行流程

BDR 分类查询的主流程如下：

1. `astersql-meta-model-group1` 第一次读取 `ActionBDRMap` 时触发其 `LazyLock` 初始化。
2. 初始化闭包先访问 `BDRActionMap`；若正向表尚未初始化，则创建四个类别及其动作向量。
3. `flat_map` 将每个 `(类别, 动作列表)` 展开成 `(动作, 类别)`，再收集为反向 `HashMap`。
4. `pkg/ddl/bdr/bdr.rs::IsDenied` 以动作查询反向表。查不到时，Primary/Secondary 分支直接拒绝；查到后再结合 BDR 角色和少数索引参数特例决定是否拒绝。

TSO 转换是独立流程：`TSConvert2Time` 将输入右移 18 位以去除逻辑计数，按毫秒解释为 UTC 时间。直接 Rust 调用者包括 `pkg/meta/model/table.rs::TableInfo::GetUpdateTime`、`pkg/session/runtime/normal_ddl_service.rs`、`pkg/ddl/persistent_actions.rs` 和 `pkg/ddl/storage_class_transition.rs`，用于呈现模型更新时间或构造 DDL/存储级别迁移状态的开始时间。

## 数据与状态

`ActionType` 在 group1 中是 `u8` 持久化协议类型；本文件存放的是其分类关系，不拥有 DDL job 或复制会话状态。四个类别的字符串值与 Go 版本兼容，既是人类可读标签，也是 `DDLBDRType` 相等与哈希语义的一部分。

两个映射都是进程级只读静态值。`BDRActionMap` 是维护入口，`ActionBDRMap` 是派生索引。关键不变量是：

- 每个已登记动作必须只出现在一个类别中；若同一动作重复出现，反向 `HashMap` 会覆盖旧值。
- 正向表内动作总数必须等于反向表长度；该条件与逐项反查共同检测跨类别重复和丢失。
- 当前动作集合共 90 项，分组计数为 Safe 23、Unsafe 57、Unmanagement 9、Unknown 1。
- 未登记动作不会自动成为 `UnknownDDL`；反向查询结果是 `None`，具体调用者决定如何处理。

`TSConvert2Time` 不保存状态。低 18 位只表达 TSO 逻辑序号，不参与结果；因此物理部分相同而逻辑部分不同的 TSO 会得到相同时间。

## 依赖与调用关系

向下依赖包括：

- `use super::*`：取得 group1 中的 `ActionType` 及全部 `ACTION_*` 常量。由于本文件以子模块方式编译，`super` 指向 `pkg/meta/model/internal/group1/lib.rs` 的 crate 根。
- `std::collections::HashMap`：存放正向与反向分类关系。
- `std::sync::LazyLock`：提供线程安全的一次初始化。
- `chrono::{DateTime, Utc}`：表达 UTC 时间并按 Unix 毫秒构造；依赖位于 `pkg/meta/model/internal/group1/Cargo.toml`。

主要向上调用关系包括：

- `pkg/ddl/bdr/lib.rs` 从 `meta_model::group_1` 再导出 `ActionBDRMap`、`SafeDDL` 和 `UnmanagementDDL`；`pkg/ddl/bdr/bdr.rs::IsDenied` 据此实施角色相关准入。
- `pkg/meta/model/table.rs::TableInfo::GetUpdateTime` 调用 `TSConvert2Time(self.UpdateTS)`。
- `pkg/session/runtime/normal_ddl_service.rs` 用它恢复存储级别迁移状态的开始时间并计算持续时间。
- `pkg/ddl/persistent_actions.rs` 与 `pkg/ddl/storage_class_transition.rs` 用它构造迁移操作状态。

RustCodeGraph 的文件节点确认 `bdr.rs` 被 `pkg/ddl/persistent_actions.rs`、`pkg/ddl/storage_class_transition.rs`、`pkg/meta/model/bdr_test.rs`、`pkg/meta/model/table.rs`、`pkg/meta/reader.rs` 等文件使用。由于精确同名符号的 `callers/callees` 查询在本次会话中超时，以上直接边又通过限定 Rust 文件的精确符号检索核验；不把同名 Go 调用计入 Rust 调用边。

## 错误处理与边界

分类表的构造没有 `Result`：内存分配失败之外没有显式运行时错误路径。最重要的逻辑边界不是抛错，而是反向表的覆盖语义；重复动作会静默保留最后迭代到的类别，所以修改分组必须依靠独立测试维护唯一性和完整性。

`TSConvert2Time` 也不返回 `Result`。`DateTime::from_timestamp_millis` 返回 `Option`，实现用 `expect("TSO physical milliseconds must be a valid UTC timestamp")` 将无效时间变为 panic。对 `u64` 输入右移 18 位后的非负毫秒范围，这一约束落在 `chrono::DateTime` 可表示范围内；不过扩展函数签名、改变位移或改用有符号输入时必须重新审视该假设。函数不会校验输入是否确实来自 PD，也不会保留逻辑位或原始 TSO。

未登记的 `ActionType` 与显式的 `UnknownDDL` 不等价。当前 DDL 消费者 `IsDenied` 对 Primary/Secondary 的未登记值采用 fail-closed（拒绝），但该策略属于调用者，不是本文件强制的全局行为。

## 并发与资源生命周期

`LazyLock` 保证每张表在并发首次访问时只初始化一次，其他线程等待同一次初始化结果；初始化完成后，静态值存活到进程退出。公开 API 只暴露 `&HashMap`/`&Vec` 的共享访问路径，没有内部锁竞争、后台任务、通道、事务、I/O 或显式清理过程。

`ActionBDRMap` 初始化期间会嵌套读取 `BDRActionMap`，依赖方向是单向的；`BDRActionMap` 的初始化闭包不反向访问 `ActionBDRMap`，因而当前不存在循环初始化。`TSConvert2Time` 只计算局部值，可并发调用且不接触共享可变状态。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/meta/model/bdr.go`，测试对照为 `pkg/meta/model/bdr_test.go`。

- Go 的 `type DDLBDRType string` 对应 Rust 的静态字符串新类型 `DDLBDRType(&'static str)`；四个常量的字面值一致。
- 两边的四组动作内容与类别语义一致，Rust 当前为 23/57/9/1，共 90 项。
- Go 先创建空 `ActionBDRMap`，再在包 `init()` 中遍历 `BDRActionMap` 填充；Rust 用两个 `LazyLock`，在首次访问反向表时惰性派生，避免可变全局初始化。
- Go 重复键赋值和 Rust `collect::<HashMap<_, _>>()` 都会以后写值覆盖先写值，因此两边都依赖测试检测重复分类。
- Go `time.UnixMilli(int64(ts >> 18))` 对应 Rust `DateTime::from_timestamp_millis((ts >> 18) as i64)`；两边都忽略 18 个逻辑位。Rust 明确返回 `DateTime<Utc>`，Go 返回 `time.Time`。
- Go 注释对 `SafeDDL` 的定义是 Primary 可执行，对 `UnmanagementDDL` 的定义是 CDC 不同步；Rust 源码的概括性注释不是最终准入规则，完整角色行为应以 `pkg/ddl/bdr/bdr.rs::IsDenied` 及其 Go 对照为准。

## 扩展指南

新增或调整 DDL 动作时，最可能修改的是 `BDRActionMap` 中对应类别的向量；不要手工维护 `ActionBDRMap`，它应始终由正向表派生。安全步骤是：

1. 先确认 group1 中 `ActionType` 数值及 Go `pkg/meta/model/bdr.go` 的目标类别，保持持久化编号和分类语义一致。
2. 确保动作只加入一个类别；需要改变类别时从旧向量移除后再加入新向量。
3. 同步独立 Rust 测试 `pkg/meta/model/bdr_test.rs`，必要时补充明确的类别断言；不得把测试嵌入本生产文件。Go 语义变化时也应同步 `pkg/meta/model/bdr_test.go`。
4. 检查 `pkg/ddl/bdr/bdr.rs::IsDenied` 是否还需要参数级特例；“类别安全”不必然覆盖唯一索引等细粒度条件。
5. 若修改 TSO 位布局或返回类型，同步检查 `TableInfo::GetUpdateTime`、DDL/存储迁移状态调用者及 `pkg/meta/model/bdr_1_aster_unit_test.rs` 的毫秒转换断言。

兼容风险主要是动作误分类导致 Primary/Secondary 错误放行或拒绝，以及字符串标签变化破坏比较语义。性能风险较低：两张表只初始化一次，查询为哈希查找；若动作集合显著扩大，应仍避免在热路径重复构造分类表。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`node --file pkg/meta/model/bdr.rs --offset 1 --limit 260` 读取了目标文件全部 169 行；文件节点列出其直接使用文件。对 `DDLBDRType`、`BDRActionMap`、`ActionBDRMap`、`TSConvert2Time` 执行了查询；精确 `callers/callees` 因 Go/Rust 同名解析超时，调用边改由限定路径的 `rg` 结果交叉核验。
- 源码与装配：`pkg/meta/model/bdr.rs`、`pkg/meta/model/internal/group1/lib.rs`、`pkg/meta/model/internal/group1/Cargo.toml`、`pkg/meta/model/lib.rs`、`pkg/meta/model/Cargo.toml`。
- Rust 调用者：`pkg/ddl/bdr/bdr.rs`、`pkg/ddl/bdr/lib.rs`、`pkg/meta/model/table.rs`、`pkg/session/runtime/normal_ddl_service.rs`、`pkg/ddl/persistent_actions.rs`、`pkg/ddl/storage_class_transition.rs`。
- Rust 独立测试：`pkg/meta/model/bdr_test.rs` 验证动作全集长度、正反映射可逆及物化视图动作分类；`pkg/meta/model/bdr_1_aster_unit_test.rs` 另行抽查四类代表动作和 TSO 毫秒转换；`pkg/meta/model/table_test.rs` 验证表更新时间委托关系。
- Go 对照：`pkg/meta/model/bdr.go` 与 `pkg/meta/model/bdr_test.go`；直接消费分类的 Go 行为可在 `pkg/ddl/bdr/bdr.go` 核验。
- 人工复核：逐项统计 `BDRActionMap` 得到 90 项且未发现重复名称；文档区分了静态分类、调用者准入策略和独立的 TSO 转换职责，没有将未登记动作描述成已显式分类。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构检查，并检查只新增本说明、删除完成后的编号任务文件，未改动 Rust、Go、Cargo 或 `plan.md`。
