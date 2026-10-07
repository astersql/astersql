# `pkg/planner/core/access/access_obj.rs`

## 文件定位

该文件属于 `astersql-planner-core-access` crate，定义规划器“访问对象”的 Rust 数据模型、EXPLAIN 文本格式和 `tipb::ExplainOperator` protobuf 写入逻辑。访问对象描述物理算子触达的表、索引和分区，是 EXPLAIN access object 列与二进制计划之间的表示边界。crate 入口 `pkg/planner/core/access/lib.rs` 公开 `access_obj` 模块并再导出全部类型；工作区根 `Cargo.toml` 以 `facade_planner_core_access` 引入该 crate，`pkg/lib.rs` 再通过 facade 导出。

当前接线仍处于迁移阶段：`pkg/planner/core/base/misc_base.rs` 声明了 Rust `AccessObject` trait，但本文件只有同名风格的固有方法，没有为任何类型实现该 trait。仓库内 Rust 实例化点也只在 `pkg/planner/core/access/migration_aster_unit_test.rs` 中；例如 `physical_mem_table.rs::AccessObject` 当前直接返回 `String`，并未构造本文件的 `ScanAccessObject`。因此本文件已经具备与 Go 对齐的表示和转换能力，但不能据此认定 Rust 物理计划主链已普遍使用这些对象。

## 核心职责

1. `ScanAccessObject` 将表、索引和分区组合成普通或归一化 EXPLAIN 文本；归一化输出只隐藏具体分区名，不隐藏表名、索引名或索引列。
2. `IndexAccess` 保存索引名称、列顺序和聚簇属性，并转换为 `tipb::IndexAccess`。
3. `OtherAccessObject` 承载无法归入扫描或动态分区对象的自由文本，空文本写 protobuf 时是 no-op。
4. `DynamicPartitionAccessObject(s)` 表示动态分区裁剪结果，处理裁剪错误、全分区、空结果（`dual`）、具体分区及多表组合展示。
5. 三类可序列化对象的 `SetIntoPB` 都以替换方式设置 `ExplainOperator.access_objects`，而不是向已有列表追加；这一行为由迁移测试直接验证。

本文件不负责选择访问路径、执行分区裁剪、构建物理算子或发送 protobuf；它只格式化调用方已经准备好的状态并写入消息对象。

## 主要符号

- `ScanAccessObject { Database, Table, Indexes, Partitions }`：表扫描访问描述。`Database` 不出现在文本中，只进入 protobuf；`Table`、`Partitions` 和 `Indexes` 依次组成文本。
- `ScanAccessObject::String(&self) -> String`：保留真实分区名。表名非空时先输出 `table:<name>`，分区以逗号紧凑连接，索引列以 `, ` 连接；空表名不会抑制后续片段，所以可能产生前导 `, `。
- `ScanAccessObject::NormalizedString(&self) -> String`：与普通输出同序，但任意非空分区集合统一输出 `partition:?`。
- `ScanAccessObject::SetIntoPB(&self, Option<&mut tipb::ExplainOperator>)`：目标为 `None` 时返回；否则构造一个 scan access object 并整体替换目标列表。
- `IndexAccess { Name, Cols, IsClusteredIndex }` 与 `IndexAccess::ToPB`：保持字段值和列顺序；聚簇索引在文本中使用 `clustered index:`，普通索引使用 `index:`。
- `OtherAccessObject(pub String)`：字符串 newtype。`String` 与 `NormalizedString` 相同；`SetIntoPB` 对 `None` 或空字符串不修改目标。
- `DynamicPartitionAccessObject { Database, Table, AllPartitions, Partitions, Err }`：单个动态裁剪结果。`String` 的优先级为 `Err`、`AllPartitions`、空分区、具体分区。
- `DynamicPartitionAccessObjects(pub Vec<Box<DynamicPartitionAccessObject>>)`：对象集合。单元素直接返回元素文本，多元素追加 ` of <Table>` 以消除表归属歧义；归一化输出与普通输出相同。
- `DynamicPartitionAccessObjects::SetIntoPB`：空集合或空目标不写入；非空时保留输入长度和顺序，错误元素对应 protobuf 中的零值槽位。

文件没有模块级常量、trait、泛型、条件编译项或异步函数。所有类型及方法均为公开 API，字段也全部公开；命名保留 Go 风格，crate 根在 `lib.rs` 中允许相应 Rust lint。

## 执行流程

扫描文本流程如下：先在表名非空时写 `table:`；再根据模式写真实分区列表或单个 `?`；最后按 `Indexes` 原顺序逐项选择普通/聚簇前缀，并拼接名称与列列表。它不排序、不去重，也不补全数据库名，因此输出完全取决于调用方输入顺序。

扫描 protobuf 流程为：检查可选目标，克隆数据库、表和分区，逐项调用 `IndexAccess::ToPB`，用所得 `tipb::ScanAccessObject` 包装为一个 `tipb::AccessObject`，最后用仅含该元素的 `RepeatedField` 替换 `ExplainOperator.access_objects`。

动态分区文本流程先处理每个单对象：错误文本优先；无错误时 `AllPartitions` 胜过分区列表；否则空列表表示 `partition:dual`，非空列表表示实际分区。集合为空返回空字符串，单元素省略表名，多元素按输入顺序生成 `<分区描述> of <表名>` 并用 `, ` 分隔。

动态分区 protobuf 流程先创建与输入等长的零值对象数组。遍历时，带 `Err` 的元素直接 `continue`，无错误元素才填充字段；之后整数组装进 `DynamicPartitionAccessObjects` 并替换目标 access object 列表。因此错误不会令序列化失败，也不会缩短或重排输出。

## 数据与状态

所有业务状态均由结构体拥有：字符串和向量使用 `String`/`Vec`，动态对象集合通过 `Box` 拥有各元素。格式化方法只读取状态并返回新字符串；protobuf 方法克隆字符串和向量，不把借用保存到消息中，也不修改源对象。

重要不变量与表示约定包括：索引和分区的输入顺序必须保留；`AllPartitions` 为真时文本忽略 `Partitions`；非空 `Err` 覆盖单对象的所有正常文本状态；动态集合中的错误项在 protobuf 中占一个零值槽位；所有成功写入都把 `access_objects` 收敛为恰好一个外层对象。数据库名只用于 protobuf，动态多对象文本只使用表名。

这些结构没有自定义构造器或校验器，允许空名称、重复分区、`AllPartitions` 与非空分区并存等组合。调用方负责提供语义一致的数据，本文件只执行上述确定性优先级。

## 依赖与调用关系

直接外部依赖只有 `protobuf::RepeatedField` 和 `tipb`。`pkg/planner/core/access/Cargo.toml` 将 crate 定义为 `astersql-planner-core-access`，库入口为 `lib.rs`；`protobuf` 固定为 `2.8.0`，`tipb` 固定到提交 `07f0ea6b6bffa9d8ac100d81ee51dbbfe4dda3bf`，关闭默认 feature 并启用 `protobuf-codec`。这些依赖决定本文件使用 protobuf 2.x 的 `RepeatedField` API，而不是普通 `Vec` 直接赋值。

内部调用边很短：`ScanAccessObject::SetIntoPB -> IndexAccess::ToPB`；`OtherAccessObject::NormalizedString -> OtherAccessObject::String`；`DynamicPartitionAccessObjects::{String, NormalizedString} -> DynamicPartitionAccessObject::String`（归一化方法先委托集合 `String`）。RustCodeGraph 对目标文件识别出 17 个符号，并确认同目录测试调用各文本和 protobuf 方法。

上游边界目前主要是 crate 再导出与迁移测试，而非完整运行时：`access/lib.rs` 再导出类型，根 facade 再公开 crate；`migration_aster_unit_test.rs` 构造对象并验证行为。`base/misc_base.rs::AccessObject`、`DataAccesser` 和 `PartitionAccesser` 描述预期抽象边界，但本文件尚未实现这些 trait。未来接入物理算子时，应通过真实 trait 实现建立调用边，而不是继续复制字符串拼接逻辑。

## 错误处理与边界

本文件没有 `Result`、panic 分支或日志。可缺失的 protobuf 目标用 `Option<&mut ExplainOperator>` 表示，`None` 一律安全返回。`OtherAccessObject` 空字符串和空动态对象集合也保持目标消息原样；相反，空字段的 `ScanAccessObject` 仍会写入一个零字段 scan 对象。

动态裁剪错误是数据而不是 Rust 错误：文本直接返回 `Err`，protobuf 则保留相同位置的零值对象。消费者必须知道零值槽位可能代表裁剪错误，不能仅凭对象数量或空字段断言有效访问。该选择严格复刻 Go 的定长切片加 `continue` 行为。

类型自身没有校验 `Table`、`Database`、索引名和列名。测试确认空表名配合分区/索引会保留前导分隔符，这是兼容行为，不应在未同步 Go 输出和黄金结果的情况下“美化”。同样，集合单元素与多元素的文本格式不同，新增消费者不应假定始终带 `of <table>`。

## 并发与资源生命周期

该文件不创建线程、任务、锁、通道、事务、文件或网络资源。所有方法同步执行；文本方法只读借用 `&self`，protobuf 方法独占借用目标 `&mut ExplainOperator`，由 Rust 借用规则防止同一调用期间并发改写消息。

对象拥有其字符串和向量，返回值或 protobuf 都通过克隆获得独立所有权，方法结束后没有悬垂引用或后台生命周期。主要资源成本是按输出大小分配字符串、克隆字段以及构造 protobuf 数组；`SetIntoPB` 会丢弃目标中先前的 access object 列表。若在热路径优化分配，必须保持字段顺序、替换语义和错误零值槽位不变。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/access/access_obj.go`。Rust 的 5 个公开类型与 Go 的 `ScanAccessObject`、`IndexAccess`、`OtherAccessObject`、`DynamicPartitionAccessObject`、`DynamicPartitionAccessObjects` 字段和分支逐项对应；文本常量、分隔符、判断顺序和 protobuf oneof 选择保持一致。

语言层差异主要有三点。第一，Go 方法接收者或索引指针可为 `nil`；Rust 的有效 `&self` 不可为空，所以 `ScanAccessObject`/`IndexAccess` 无需对应的 nil-self 分支，但 protobuf 目标仍以 `Option` 保留 nil-target no-op。第二，Go 动态集合是对象指针切片，Rust 以 `Vec<Box<_>>` 表达非空所有权元素；Rust 不支持集合中 nil 元素，因此没有复刻 Go 对 nil 元素解引用的行为。第三，Go 类型通过方法集满足 `base.AccessObject` 接口，Rust 当前只提供 PascalCase 固有方法，尚未实现 `misc_base.rs::AccessObject` 的 snake_case trait 方法。

Go 生产调用者包括物理批量点查和索引扫描等文件，RustCodeGraph 也能看到 Go 的调用边；但 Rust 仓库检索没有发现同等生产构造点。因此 Go 版本说明了设计语义和目标接线，不能作为 Rust 已接入运行时的证据。

## 扩展指南

新增扫描字段时，应同时修改 `ScanAccessObject`、普通/归一化文本策略、`SetIntoPB`、Go 对照实现和 `migration_aster_unit_test.rs`；如果 tipb schema 没有对应字段，应先明确兼容降级方式。新增索引属性则优先落在 `IndexAccess::ToPB`，并验证普通与聚簇索引文本仍保持现有顺序。

调整动态分区行为时，必须保留或有意迁移以下兼容点：`Err > all > dual > list` 的文本优先级、单/多对象格式差异、输入顺序、错误项的零值槽位以及覆盖而非追加的 protobuf 语义。任何改变都应同步 Go 文件或明确记录迁移偏差，并扩展同目录独立测试，不能把测试嵌入生产源文件。

要把本文件接入 Rust 主链，最小合理入口是为相应类型实现 `pkg/planner/core/base/misc_base.rs::AccessObject`，将 snake_case trait 方法委托给现有固有方法，再由物理算子的 `DataAccesser`/`PartitionAccesser` 返回 trait 对象。接线前还需解决 trait 的 `&mut ExplainOperator` 与现有 `Option<&mut ExplainOperator>` 签名差异，并为具体物理算子增加独立回归测试；不要在调用点重复实现格式化。

性能方面，当前每次输出都会分配，protobuf 写入还会克隆全部名称和列表。若改为缓存或借用表示，需要特别评估计划对象变更后的缓存失效、tipb 所有权要求和 EXPLAIN 并发读取安全。兼容风险集中在用户可见字符串和 protobuf 字段布局，这两类变更都可能影响计划摘要、EXPLAIN 黄金文件或远端消费者。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 Rust/Go 源文件和迁移测试均已索引。
- RustCodeGraph `files --filter pkg/planner/core/access`：确认 `access_obj.rs`、`access_obj.go`、`lib.rs`、`migration_aster_unit_test.rs` 四个代码证据文件。
- RustCodeGraph `node --file pkg/planner/core/access/access_obj.rs`：读取完整 233 行，核对所有 5 个类型、12 个方法、分支和 protobuf 写入逻辑。
- RustCodeGraph `query AccessObject`、`query DynamicPartitionAccessObject` 及 callers/callees 查询：确认 Rust trait 位于 `base/misc_base.rs`，目标文件内部调用边，以及调用方名称歧义；精确 callers 对结构体无结果，因此结合仓库 `rg` 复核生产接线。
- `rg` 对 Rust 构造、trait impl、方法调用和 facade 依赖的检索：只发现同目录迁移测试实例化目标类型，没有发现这些类型的 `AccessObject` trait impl；发现 `physical_cte.rs` 中只有注释掉的拟议用法。
- `pkg/planner/core/access/Cargo.toml`、`pkg/planner/core/access/lib.rs`、根 `Cargo.toml` 和 `pkg/lib.rs`：核对 crate 名称、入口、依赖版本、再导出链和迁移元数据。
- `pkg/planner/core/access/access_obj.go`：核对全部 Go 字段、文本格式、nil 防御、动态错误零值槽位和 protobuf oneof 语义。
- `pkg/planner/core/access/migration_aster_unit_test.rs`：核对真实分区与 `?`、前导分隔符、普通/聚簇索引、覆盖语义、空值 no-op、动态 error/all/dual/list、多对象格式和错误槽位。
- 人工复核结论：文件存在的目的、每条执行路径、当前未完成的 trait/生产接线和安全扩展位置均有上述源码或检索证据；没有把 Go 生产调用关系误写成 Rust 已接入事实。
