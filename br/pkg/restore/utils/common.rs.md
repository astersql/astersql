# `br/pkg/restore/utils/common.rs`

## 文件定位

`common.rs` 属于 Cargo 包 `astersql-br-pkg-restore-utils`；包入口 `br/pkg/restore/utils/lib.rs` 以 `pub mod common` 挂载该文件，并通过 `pub use common::*` 将其唯一公开类型 `CreatedTable` 扁平再导出。因此外部 crate 的设计使用路径是 `astersql_br_pkg_restore_utils::CreatedTable`，也可以在包内通过 `common::CreatedTable` 定位定义。

这个文件对应 Go 包 `br/pkg/restore/utils` 的 `common.go`，表达恢复流程中“下游表已经创建、数据尚未导入”的中间状态。它不是建表、键重写或数据导入算法的实现文件，而是把这些阶段共同需要的三组元数据放进同一个传递对象。

当前接线需要特别区分：RustCodeGraph 对 `br/pkg/restore/utils/common.rs:28` 的调用分析只得到结构体自身，仓库内 `rg` 也没有发现构造或消费 `astersql_br_pkg_restore_utils::CreatedTable` 的 Rust 代码。现有 Rust 快照恢复链实际使用 `br/pkg/restore/snap_client/stubs.rs:1278` 中另一份 `CreatedTable`。所以本文件目前是已公开但尚未接入主链的移植类型，不能据其存在推断 Rust 运行时已经使用它。

## 核心职责

`CreatedTable` 的职责是保持一次恢复建表前后的对应关系：

- `OldTable` 保存备份侧表信息，提供旧表、分区和文件元数据的来源。
- `Table` 保存目标集群中新建表的元数据，提供新的表、分区和索引 ID。
- `RewriteRule` 保存从旧键前缀到新键前缀的规则，使后续 SST 或日志导入能够把备份键空间映射到目标表键空间。

三者必须描述同一个逻辑表。这个关联约束没有由 Rust 类型系统或本文件内代码校验，而是由创建者负责建立、由消费者按约定读取。文件本身不生成规则、不验证新旧表匹配关系，也不执行 I/O。

## 主要符号

### `pub struct CreatedTable`

定义位于 `common.rs:28-32`，是文件内唯一的类型、唯一的公开 API；没有常量、trait、函数、`impl` 或条件编译项。

- `RewriteRule: Option<Box<RewriteRules>>`：可空、堆分配的键重写规则集合。`RewriteRules` 来自同一 crate 的 `rewrite_rule.rs`，通常包含一组底层前缀规则以及新表 ID 等辅助信息。`None` 表示调用方没有提供规则；本结构不会补默认规则。
- `Table: Option<Box<model::TableInfo>>`：可空的目标表元数据。这里的 `model::TableInfo` 是 `br/pkg/restore/utils/stubs.rs:145` 定义的精简类型，当前覆盖表 ID、分区和索引等本包算法所需字段，并不等同于完整生产 `pkg/meta/model.TableInfo`。
- `OldTable: Option<Box<metautil::Table>>`：可空的备份侧表。这里的 `metautil::Table` 是 `br/pkg/restore/utils/stubs.rs:156` 的空占位结构，尚不能承载 Go `metautil.Table` 的数据库、表和文件信息。

三个字段均为 `pub`，调用者可以直接构造和替换。该类型没有 `Clone`、`Debug`、`Default`、`PartialEq` 等派生实现，也没有构造函数或访问器；这与当前已接线的 `snap_client/stubs.rs::CreatedTable` 不同，后者派生了 `Clone`、`Debug`、`Default`，并使用非可空的 `Table`、`OldTable` 值。

## 执行流程

本文件没有可执行函数；它在完整恢复流程中的预期数据流由 Go 对照和当前 Rust 快照恢复实现共同说明：

1. 建表阶段根据备份侧 `metautil::Table` 在目标集群创建表。
2. 创建者读取目标集群生成的新 `TableInfo`，并检查新旧表模式是否兼容。当前 Rust 对应逻辑见 `br/pkg/restore/snap_client/client.rs:1077` 的 `buildCreatedTables`，其中会拒绝聚簇索引模式不一致的表。
3. 创建者根据旧、新表 ID 生成 `RewriteRules`，并把恢复时间戳写入每条规则；`client.rs:1091-1102` 展示了当前桩类型的构造过程。
4. 创建者把规则、新表和旧表组合为 `CreatedTable`，交给后续 placement、文件范围整理、恢复流水线、统计更新和校验阶段。
5. 消费者从 `OldTable` 读取旧物理 ID/文件，从 `Table` 读取新物理 ID，并用 `RewriteRule` 改写键前缀。例如当前 Rust `tikv_sender.rs` 及其测试使用 `snap_client::stubs::CreatedTable` 完成物理表排序和文件范围校验。

步骤 2-5 是该结构的业务语义，但目前并不经过本文件的 `CreatedTable`；要接入 canonical 类型，必须先消除与 `snap_client/stubs.rs` 的字段类型差异并迁移调用方，不能只替换 import。

## 数据与状态

`CreatedTable` 是拥有所有权的数据快照：三个成员都由结构体持有，`Box` 使具体值位于堆上，`Option` 将“字段缺失”编码为显式状态。它不含引用和生命周期参数，因此一旦成功构造，内容不依赖创建者栈帧。

该结构没有内部状态机。业务上可把完整的 `Some/Some/Some` 组合视为可供数据恢复消费的状态，但源码允许八种 `Option` 组合，也没有方法阻止缺少新表、旧表或规则的无效组合。尤其 `utils::stubs::metautil::Table` 目前为空，即使是 `Some` 也不代表具备恢复主链所需的旧表信息。

对象没有共享可变性、缓存、全局变量或持久化状态。字段命名保留 Go 的大写风格；crate 根的 `#![allow(non_snake_case)]` 允许这种移植期命名。

## 依赖与调用关系

直接源码依赖只有两条：

- `crate::rewrite_rule::RewriteRules`：键前缀重写规则的 canonical utils 实现。
- `crate::stubs::{metautil, model}`：为避免引入完整 TiDB/kvproto 依赖图而提供的本地精简类型。

crate 边界由 `br/pkg/restore/utils/Cargo.toml` 确认：它是库 crate，入口为 `lib.rs`，移植元数据声明 Go 包为 `br/pkg/restore/utils`；manifest 的直接依赖是 `astersql-br-pkg-errors`、`astersql-br-pkg-rtree` 和 `astersql-errors`。`common.rs` 自身没有直接引用这三个外部 crate。

公开关系是 `lib.rs -> common.rs -> rewrite_rule.rs/stubs.rs`。当前真实 Rust 调用关系则停在公开导出：RustCodeGraph 的 callers 结果为空，`impact` 对目标定义仅列出它自身。与之相对，Go `common.go::CreatedTable` 被 `snap_client/client.go`、`placement_rule_manager.go`、`tikv_sender.go`、`export_test.go` 和 `br/pkg/task/restore.go` 使用。Rust 中这些职责目前连接到 `snap_client/stubs.rs::CreatedTable`，例如 `client.rs::CreateTables/buildCreatedTables`、`tikv_sender.rs::getSortedPhysicalTables/SortAndValidateFileRanges` 和 `pipeline_items.rs`。

## 错误处理与边界

本文件没有 `Result`、错误类型、校验分支或 panic 点；构造、解引用和业务一致性检查全部留给调用者。主要边界如下：

- 任一字段都可能为 `None`。未来消费者必须决定缺失字段是合法的阶段性状态，还是应返回带上下文的错误，不能直接 `unwrap` 后假设完整。
- `RewriteRule` 可能存在但规则集合为空、旧前缀与旧表不匹配，或新前缀与目标表不匹配；本类型不验证这些不变量。
- `Table` 与 `OldTable` 可能指向不同逻辑表，分区数、名称、索引或聚簇索引模式也可能不兼容；当前 Rust 主链的部分检查在 `client.rs::buildCreatedTables`，不是此处保证。
- `utils::stubs::metautil::Table` 是空结构，无法支持需要旧表 ID、分区、数据库名或文件列表的消费者。这是当前迁移边界，而不是“空旧表也能恢复”的行为承诺。
- 类型没有 `Default`，因此不能像 `snap_client` 测试那样通过 `..Default::default()` 构造部分夹具；迁移测试时需要显式处理三个可空字段或补充经过论证的构造 API。

## 并发与资源生命周期

`CreatedTable` 不创建线程、异步任务、锁、通道、事务、文件或网络连接，也没有 `Drop` 实现。其资源生命周期完全由 Rust 所有权管理：结构体释放时，三个 `Option<Box<_>>` 中存在的堆对象会随之释放。

源码没有显式 `Send`/`Sync` 实现；是否可跨线程传递由三个成员类型的自动 trait 共同决定。当前字段由 `Vec`、整数、字符串等拥有型数据组成，理论上可获得相应自动 trait，但本文件没有把并发作为 API 契约，也没有编译期断言。Go 和当前 Rust 快照恢复流水线会在并发 handler 之间传递创建表对象；若未来把本类型接入该流水线，应以独立测试验证所需的所有权模型、失败取消和有界并发行为，而不是从本结构的无锁特征推断流水线安全。

大对象使用 `Box` 避免把完整规则和元数据内联在结构体中；同时每个存在字段产生一次独立堆分配。当前文件没有批量分配或复用策略，性能影响取决于恢复表数量和后续是否频繁移动/克隆。

## 与 Go 版本的对应关系

Go `br/pkg/restore/utils/common.go:24-28` 同样只定义 `CreatedTable`，字段一一对应：`RewriteRule *RewriteRules`、`Table *model.TableInfo`、`OldTable *metautil.Table`。Rust 的 `Option<Box<T>>` 对应 Go 可为 `nil` 的指针，并保留独占拥有语义；字段名称也逐字保留，便于对照移植。

存在两项关键语义差异：

1. Go 字段引用真实 `br/pkg/metautil.Table` 和 `pkg/meta/model.TableInfo`；本 Rust 文件引用 `utils/stubs.rs` 的精简类型，其中 `metautil::Table` 仍是空占位。因此当前 Rust 类型只完成了外形对齐，没有完成元数据能力对齐。
2. Go 类型直接贯穿建表和恢复调用链；本 Rust 类型尚无调用者。当前 Rust 主链的同名类型位于 `snap_client/stubs.rs`，字段为 `Option<RewriteRules>`、非可空 `model::TableInfo` 和非可空 `metautil::Table`，也没有 `Box`。两份 Rust 类型不可直接互换。

Go 的相关行为测试位于 `br/pkg/restore/snap_client/tikv_sender_test.go`、`placement_rule_manager_test.go` 和 `pipeline_items_test.go`。Rust 的对应测试文件 `tikv_sender_test.rs`、`placement_rule_manager_test.rs`、`pipeline_items_test.rs` 验证了相似的排序、规则、placement、并发和统计流程，但 import 明确来自 `crate::stubs::CreatedTable`，不构成本文件类型的直接覆盖。

## 扩展指南

若只需给中间对象增加附加元数据，应先确认 Go `utils.CreatedTable` 是否有同一字段或行为，再修改 `common.rs::CreatedTable`；对应测试必须放在独立 `*_test.rs` 文件中，不能内嵌到生产源文件。由于该类型目前没有直接测试，可在 `br/pkg/restore/utils/` 增加独立的 common 测试模块，并在 `lib.rs` 的 `#[cfg(test)]` 区域挂载。

若目标是让恢复主链真正使用本类型，推荐按以下接线顺序进行：

1. 先补足或替换 `utils::stubs::model::TableInfo` 与 `metautil::Table`，保证能表达 `snap_client` 消费的表名、分区、文件和 merge-option 等信息。
2. 决定 nullable 契约：若完整对象是主链不变量，提供返回 `Result<CreatedTable>` 的构造函数并在边界消除 `None`；若阶段性缺失是合法状态，则让每个消费者显式处理缺失分支。
3. 迁移 `snap_client/client.rs::buildCreatedTables` 的构造点，再逐个迁移 `tikv_sender`、placement manager、pipeline 和 checksum 等消费者，最后删除重复类型。不要在两份同名结构之间长期做无校验转换。
4. 同步独立 Rust 测试：至少覆盖正常的新旧表/分区 ID 映射、缺失字段、模式不匹配、空规则、文件范围排序，以及流水线并发失败取消；以对应 Go 测试的断言集合为基准，不做简化版验证。

兼容风险主要来自把非可空桩字段改成 `Option<Box<_>>` 后的调用方处理、stub 与真实元数据字段差异，以及公开结构体字段变更对外部 crate 的构造代码影响。性能风险主要是每表三次潜在堆分配以及迁移时不必要的深拷贝；正确性风险则是旧、新物理 ID 与重写规则不一致。

## 验证依据

本说明基于以下直接证据：

- `br/pkg/restore/utils/common.rs:16-32`：文件说明、两条 import、`CreatedTable` 及三个公开字段。
- `br/pkg/restore/utils/lib.rs:18-43`：模块挂载、公开再导出和 stub 边界；`lib.rs:45-59` 也证明现有测试模块没有 `common_test.rs`。
- `br/pkg/restore/utils/Cargo.toml`：crate 名称、`lib.rs` 入口、Go 包映射和直接依赖。
- `br/pkg/restore/utils/common.go:17-28`：Go 的真实依赖和一一对应结构。
- `br/pkg/restore/utils/stubs.rs:105-157`：本文件引用的 `model::TableInfo` 与空 `metautil::Table` 的实际能力边界。
- `br/pkg/restore/snap_client/stubs.rs:1276-1282`：当前 Rust 主链使用的重复 `CreatedTable` 定义及字段差异。
- `br/pkg/restore/snap_client/client.rs:986-1105`：创建表、模式检查、规则生成、时间戳写入以及中间对象构造流程。
- `br/pkg/restore/snap_client/tikv_sender_test.rs:41-110,198-275` 与 `pipeline_items_test.rs:68-190`：当前 Rust 桩类型的物理 ID 排序、文件范围和并发流水线测试证据。
- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；`node --file br/pkg/restore/utils/common.rs` 确认完整源码只有 32 行；`query CreatedTable --json` 区分了 utils、snap-client 与 Go 三个同名定义；对精确目标执行 `callers` 没有返回调用者，`impact` 对该定义只列出其自身。
- 仓库搜索 `rg -n --glob '*.rs' 'restore_utils|utils::common|common::CreatedTable|use .*CreatedTable|CreatedTable \{' br/pkg/restore br/pkg/task`：未发现本文件类型的 Rust 构造或消费点，发现的恢复主链构造点均属于 `snap_client::stubs::CreatedTable`。

本任务是纯文档分析，没有运行 Cargo 或代码测试。验收使用任务指定的结构命令，确认目标文档存在且恰好包含上述十一个固定二级标题；人工复核重点是区分 canonical 导出类型、当前实际接线类型和 Go 运行链，未把占位结构描述成已完整支持。
