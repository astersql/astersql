# `pkg/tablecodec/rowindexcodec/rowindexcodec.rs`

## 文件定位

本文件是 `astersql-tablecodec-rowindexcodec` crate 的实际分类逻辑，源码入口由同目录 `lib.rs` 的 `pub mod rowindexcodec` 引入并通过 `pub use rowindexcodec::*` 再导出。crate 在工作区根 `Cargo.toml` 中注册，`pkg/tablecodec/lib.rs` 又把它挂到 `tablecodec::rowindexcodec`，根门面 `pkg/lib.rs` 则通过 `facade_tablecodec_rowindexcodec` 暴露同一 API。

它位于 SQL 层与存储键格式之间，只做 TiDB 表键的轻量类别识别：不解码 table id、handle 或 index id，也不负责键的构造。当前可确认的生产调用者是 `pkg/util/resourcegrouptag/resource_group_tag.rs` 中的 `GetResourceGroupLabelByKey`，后者把分类结果转换为资源组的 Row、Index 或 Unknown 标签。

## 核心职责

- 用 `GetKeyKind(&[u8]) -> KeyKind` 判断输入是否具有 `t + 8 字节 table id + _r/_i` 的开头。
- 对合法位置上的 `_r` 返回 `KeyKindRow`，对 `_i` 返回 `KeyKindIndex`，其余情况统一返回 `KeyKindUnknown`。
- 以只读切片和前缀比较完成判断，不进行完整反序列化、分配或错误对象构造。
- 与 Go 的 `pkg/tablecodec/rowindexcodec/rowindexcodec.go` 保持分支顺序和最小长度语义一致。

这里的“识别”只是格式前缀分类，不是完整合法性校验。长度恰为 11 字节、且分隔符正确的键已经能被分类，即使没有后续 row id 或 index id；`migration_classification_ignores_bytes_after_the_separator` 也证明分隔符之后的内容不参与判断。

## 主要符号

- `pub type KeyKind = i32`：分类值的公开别名。它保留 Go `type KeyKind int` 的整数式接口，但 Rust 类型系统不会阻止调用者构造 0、1、2 之外的值。
- `KeyKindUnknown = 0`、`KeyKindRow = 1`、`KeyKindIndex = 2`：分别表示未知键、行记录键和二级索引键；数值顺序与 Go 的 `iota` 定义一致。
- `tablePrefix = b"t"`：表键的一字节全局前缀。
- `rowPrefix = b"_r"`、`indexPrefix = b"_i"`：位于 table id 之后的两字节类别分隔符。
- `pub fn GetKeyKind(mut key: &[u8]) -> KeyKind`：唯一的行为入口。参数只在函数内部重新绑定为后部切片，不修改底层数据。
- `#[cfg(test)] #[path = "rowindexcodec_test.rs"] mod tests`：仅测试构建时装入同目录的独立测试文件，测试逻辑没有内嵌在生产源文件中。

这些名称保留了 Go 风格大小写；同目录 `lib.rs` 用 crate 级 `allow(non_snake_case, non_upper_case_globals)` 接受这套迁移接口。

## 执行流程

`GetKeyKind` 按以下固定顺序执行：

1. 检查 `key.len() < 11`。11 来自 1 字节 `t`、8 字节 table id 和 2 字节类别分隔符；不足时立即返回 `KeyKindUnknown`，并保证后续切片安全。
2. 用 `starts_with(tablePrefix)` 检查首字节是否为 `t`；不是表键前缀时返回 `KeyKindUnknown`。
3. 执行 `key = &key[9..]`，跳过表前缀和固定宽度的 table id。此处不解析、也不验证这 8 个字节的编码。
4. 若剩余切片以 `_r` 开头，立即返回 `KeyKindRow`。
5. 否则若以 `_i` 开头，返回 `KeyKindIndex`。
6. 分隔符未知或错位时返回 `KeyKindUnknown`。

生产链中的直接使用流程是：`GetResourceGroupLabelByKey` 接收一个 KV key，调用 `rowindexcodec::GetKeyKind`，再把 Row/Index/其他值映射成 `ResourceGroupTagLabel`。因此本函数的 Unknown 回退也覆盖未来未知 `KeyKind` 整数值。

## 数据与状态

文件没有可变全局状态。三个前缀是指向静态字节数据的 `&'static [u8]`；三个类别是编译期整数常量。一次调用只持有调用者输入的共享借用以及由 `&key[9..]` 得到的子切片。

关键不变量如下：

- 判别位置固定为偏移 9，不能通过在 table id 区域放置 `_r` 或 `_i` 提前命中。
- 最短可分类输入为 11 字节。
- 后缀不影响分类，函数不会读取或解释分隔符后的字段。
- Unknown 同时代表短输入、非 `t` 前缀、未知/错位分隔符以及所有未识别情形，调用者无法仅凭返回值区分具体原因。

## 依赖与调用关系

该实现只使用 Rust 核心切片能力：`len`、`starts_with` 和范围切片，没有运行时第三方依赖。`pkg/tablecodec/rowindexcodec/Cargo.toml` 的普通依赖为空，只有测试用的 `astersql-testkit-testsetup` dev-dependency；`[lib] path = "lib.rs"` 确定 crate 入口，`package.metadata.porting.go-package` 指向 Go 对照包。

已核对的模块与调用关系为：

- 下游：`GetKeyKind` 只调用切片内建操作，不调用仓库内其他业务函数。
- 上游：`pkg/util/resourcegrouptag/lib.rs` 从 `astersql_tablecodec_rowindexcodec` 再导出 API；`resource_group_tag.rs::GetResourceGroupLabelByKey` 调用 `GetKeyKind` 并匹配 `KeyKindRow`/`KeyKindIndex`。
- 聚合入口：`pkg/tablecodec/lib.rs` 以路径模块接入该子 crate 的源码布局；`pkg/lib.rs` 提供工作区总门面。
- 测试入口：生产文件的 `tests` 模块加载 `rowindexcodec_test.rs`；crate 根另加载 `migration_aster_unit_test.rs`，`main_test.rs` 提供公共测试初始化。

RustCodeGraph 将目标文件标记为被 `pkg/util/resourcegrouptag/resource_group_tag.rs` 使用，但当前索引对该函数的 `callers`/`callees` 查询返回空集合；因此上述函数级边同时由调用点 `rowindexcodec::GetKeyKind(key)` 和 Cargo 依赖声明直接核实，而非把空图结果解释成“没有调用者”。

## 错误处理与边界

函数没有 `Result`、panic 分支或日志。所有格式不匹配都收敛为 `KeyKindUnknown`。先检查 11 字节最小长度，再取 `[9..]`，所以对任意输入切片都不会因该切片操作越界；空切片同样安全返回 Unknown。

边界行为包括：

- 恰好 10 字节及以下：Unknown。
- 至少 11 字节但首字节不是 `t`：Unknown。
- `t` 和 8 字节占位之后不是 `_r`/`_i`：Unknown。
- `_r`/`_i` 出现在错误偏移：不会命中。
- 正确分隔符之后是空、任意数据或尚未验证的编码：仍按分隔符分类。

因此调用者不能把 Row/Index 结果当作“完整键一定可解码”的证明。若需要验证 table id、handle、index id 或尾部编码，应交给相应完整 codec，而不是扩大本函数的隐含保证。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务或外部资源。公开数据均为不可变常量/静态切片；`GetKeyKind` 是无副作用纯读取函数，同一输入在并发调用中得到相同结果。

输入借用只在调用期间存在，返回值是复制的 `i32`，不会保存对输入的引用。函数不分配堆内存、不复制 key，也没有需要释放或回滚的资源；时间复杂度对固定的 1 字节和 2 字节前缀比较而言为常数，额外空间为常数。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/tablecodec/rowindexcodec/rowindexcodec.go`。两版共同采用 0/1/2 三个类别值、`t`/`_r`/`_i` 三个字节前缀，以及“先查长度、再查表前缀、跳过 9 字节、依次查 row/index”的完全相同控制流。Rust 的 `starts_with` 对应 Go 的 `bytes.HasPrefix`，共享切片 `&key[9..]` 对应 Go 的 `key = key[9:]`。

语言层面的差异是：Go 前缀变量为包内私有可变切片，Rust 前缀目前声明为公开的不可变 `static`；Go `KeyKind` 是独立命名类型，Rust 是 `i32` 类型别名。因此 Rust 对非法整数值的静态隔离较弱，但现有生产调用点用通配分支把其他值映射为 Unknown。

`rowindexcodec_test.go::TestGetKeyKind` 的典型 row、index、空切片和 nil 用例已迁移到 `rowindexcodec_test.rs::TestGetKeyKind`；Go 的 nil 字节切片在 Rust 中由空切片表达。额外的 `migration_aster_unit_test.rs` 明确验证全部 0..10 长度、非表前缀、错误位置/未知分隔符和任意后缀，这些是对相同生产语义的边界加固，并未改变 Go 算法。

## 扩展指南

新增键类别时，最可能需要同步修改 `KeyKind` 常量集合、对应前缀和 `GetKeyKind` 的判别分支，并同步审查 `resource_group_tag.rs::GetResourceGroupLabelByKey` 的映射。还应在独立的 `rowindexcodec_test.rs` 或 `migration_aster_unit_test.rs` 增加正确偏移、最短长度、错位前缀和后缀无关性用例；不要把测试函数放回生产源文件。

若改变现有前缀、判别顺序或最短长度，必须先核对 Go `rowindexcodec.go` 及其测试，因为这会改变跨语言兼容行为。若目标是完整校验键结构，建议新增语义清晰的严格 API，而不是悄悄收紧 `GetKeyKind`：当前资源标签调用者依赖它的低成本、宽松分类特征。

性能方面应继续保持无分配和固定前缀检查；不要为类别识别引入完整 table id/handle 解码。类型安全方面，未来可评估把别名升级为 enum，但这会影响数值兼容、公开 API、通配回退和门面再导出，不能作为局部重构处理。

## 验证依据

本说明基于以下直接证据：

- 生产实现：`pkg/tablecodec/rowindexcodec/rowindexcodec.rs`，重点符号为 `KeyKind`、三个 `KeyKind*` 常量、三个 `*Prefix` 静态量和 `GetKeyKind`。
- crate/模块边界：`pkg/tablecodec/rowindexcodec/Cargo.toml`、同目录 `lib.rs`、`pkg/tablecodec/lib.rs`、根 `pkg/lib.rs`，以及工作区根 `Cargo.toml`。
- 生产调用点：`pkg/util/resourcegrouptag/lib.rs` 的再导出和 `pkg/util/resourcegrouptag/resource_group_tag.rs::GetResourceGroupLabelByKey`。
- Go 语义：`pkg/tablecodec/rowindexcodec/rowindexcodec.go::GetKeyKind` 与 `rowindexcodec_test.go::TestGetKeyKind`。
- Rust 测试：`rowindexcodec_test.rs::TestGetKeyKind`、`migration_aster_unit_test.rs` 的四个迁移回归，以及 `main_test.rs::TestMain`。
- RustCodeGraph：`status` 显示项目索引含目标文件；`files --filter pkg/tablecodec/rowindexcodec` 列出相关 Rust/Go 文件；`node --file` 核对目标实现、测试、Go 对照、门面和生产调用点；目标函数的 `callers`、`callees`、`impact` 查询均为空，已用精确调用点与 Cargo 依赖补证。

人工复核结论：文档区分了轻量分类与完整解码，说明了实际接线、所有返回分支、状态/资源特性、Go 差异和安全扩展位置；没有把索引缺失的调用边或未校验的键后缀描述为已验证能力。
