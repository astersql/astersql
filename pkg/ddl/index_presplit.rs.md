# `pkg/ddl/index_presplit.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/ddl/lib.rs` 以公开模块 `pub mod index_presplit` 挂载。它提供 add-index 前的索引 Region 预切分原语：把手工或统计规划得到的索引值转成切分 key，并提供一个抽象的 split/scatter 执行包装器。

当前真实接线中，`pkg/session/runtime/ddl.rs` 的 add-index 路径调用 `get_split_index_keys`；`pkg/ddl/index_auto_presplit.rs::plan_auto_pre_split` 和 `run_pre_split` 调用 `get_split_keys_from_value_list`。该文件不创建或持久化 DDL job，不推进 schema state，也不执行回填；它位于 DDL job 的 add-index 准备阶段，是预切分辅助层，而不是 DDL 状态机本身。

## 核心职责

- `get_split_index_keys` 在两种输入模式间分派：`SplitArguments.value_lists` 非空时采用显式切分点，否则采用 `[lower, upper)` 范围和 `num` 计算内部切分点。
- `get_split_keys_from_value_list` 逐组调用 `encode_index_key`，保持输入顺序和重复项。这一特性由 `pkg/ddl/index_presplit_test.rs::explicit_value_list_preserves_go_order_and_duplicates` 验证。
- `get_split_keys_from_bound` 对已编码的上下界求最长公共前缀，将其后的至多八字节解释为大端 `u64`，按 `count` 等分，并生成 `count - 1` 个内部 key。
- `split_index_region_and_wait` 把真正的存储操作隔离为两个回调：一次批量 split，然后逐个判断 region 是否 scatter 完成。
- `eval_split_datums` 将一组 `Option<Datum>` 收集为完整值列表；任一项为 `None` 时整体失败。

这里的 Rust 实现是局部、简化的移植层，不等同于 Go 文件的全部能力：它没有表/索引元数据、分区枚举、statement context、真实 `tables.Index.GenIndexKey`、超时 context 或存储能力探测。

## 主要符号

- `SplitError`：公开错误枚举。`InvalidCount` 表示范围切分数量为零；`InvalidBounds` 表示编码后下界不小于上界；`Evaluation` 表示表达式结果缺失；`Split(String)` 原样承载底层 split 回调的文字错误。
- `SplitArguments`：公开输入结构。`value_lists` 与 `lower`/`upper`/`num` 构成互斥使用模式，但类型本身不强制这一不变量；入口以 `value_lists` 非空为最高优先级。
- `get_split_index_keys(table_id, index_id, args)`：生产主入口，返回 `Result<Vec<Key>, SplitError>`；`Key` 是 `pkg/ddl/backfilling.rs` 中的 `Vec<u8>` 别名。
- `get_split_keys_from_value_list(...)`：公开的显式值编码入口，同时被 AUTO 预切分规划复用。
- `get_split_keys_from_bound(...)`：公开的区间插值入口。
- `encode_index_key(...)`：私有简化编码器，布局为字节 `t`、八字节大端 table ID、字节 `i`、八字节大端 index ID，再接每个 `Datum` 的 `Debug` 文本和 `0` 分隔符。
- `padded_u64(key, pad)`：私有工具，只读取切片前八字节，不足时用指定字节补齐并按大端解释。
- `split_index_region_and_wait(...)`、`eval_split_datums(...)`：公开但当前仓库搜索未发现 Rust 生产调用者的独立辅助 API；不能据其存在推断完整存储链已经接通。

文件没有 trait、`impl`、模块级可变状态、条件编译项或异步函数。

## 执行流程

手工 add-index 的当前 Rust 主链是：`pkg/session/runtime/ddl.rs` 从索引选项构造 `SplitArguments`，为普通表、global index 或每个本地分区选定 physical table ID，并根据 fast reorg 决定是否使用临时 index ID；随后逐个 physical table 调用 `get_split_index_keys`。返回 key 后，该调用点目前只注入 `beforePresplitIndex` 和 `mockSplitIndexRegionAndWaitErr` failpoint，没有调用本文件的 `split_index_region_and_wait` 去操作真实存储。

显式值模式中，`get_split_index_keys` 直接转入 `get_split_keys_from_value_list`；每一行值独立编码，结果不排序、不去重。AUTO 模式则由 `pkg/ddl/index_auto_presplit.rs::plan_auto_pre_split` 先采样边界行，再调用同一函数，之后在 AUTO 层过滤空 key、排序和去重。

范围模式中，入口先拒绝 `count == 0`，再编码 lower/upper 并要求 `lower < upper`。算法保留两端完整最长公共前缀；下界余部以 `0x00` 补齐、上界余部以 `0xff` 补齐到八字节，计算 `step = (hi - lo) / count`，然后累计 `step` 生成第 1 到第 `count - 1` 个切分点。`count == 1` 合法并返回空列表。

若单独调用 `split_index_region_and_wait`，空 key 会短路为 `Ok(0)`；否则先调用一次 `split(keys)` 获取 region IDs，再对每个 ID 调用 `scatter_finished`，最终只返回布尔判断为真的数量。

## 数据与状态

本文件处理的核心状态都是函数局部值，没有持久化状态。`SplitArguments` 借助 `Vec<Vec<Datum>>` 表示多列索引的一组或多组运行时值；`Datum` 来自 `pkg/ddl/index_cop.rs`。输出 key 是拥有所有权的字节向量，因此调用者可在之后进行临时索引 key 转换、排序或交给存储层。

范围插值的重要不变量是：输入编码 key 必须严格递增；输出数量固定为 `count - 1`；输出保留 lower/upper 的最长公共前缀。`pkg/ddl/index_nokit_test.rs::test_bounded_index_presplit_keys_retain_decodable_prefix` 验证 table/index 路由前缀不会丢失，且输出单调递增。

需要注意两个当前限制。第一，`encode_index_key` 使用 `Datum` 的 `Debug` 文本，并非 TiDB tablecodec 的生产索引编码；排序语义不保证对所有类型、排序规则、时区和复合索引与 Go 相同。第二，插值只观察公共前缀后的八字节；当 `hi - lo < count` 时 `step` 为零，函数仍会返回重复切分点，而不会像 Go 完整链路那样应用所有范围/region-size 校验。

## 依赖与调用关系

直接内部依赖只有 `crate::backfilling::Key` 和 `crate::index_cop::Datum`，因此本文件没有直接使用 `pkg/ddl/Cargo.toml` 中的外部 crate。crate 边界由 `pkg/ddl/lib.rs` 暴露，其他 crate 可通过 `astersql_ddl::index_presplit` 访问公开符号。

已验证的上游调用边包括：

- `pkg/session/runtime/ddl.rs` → `get_split_index_keys`：add-index 运行路径按 physical table 生成预切分 key。
- `pkg/ddl/index_auto_presplit.rs::plan_auto_pre_split` → `get_split_keys_from_value_list`：把统计分位边界编码成 key。
- `pkg/ddl/index_auto_presplit.rs::run_pre_split` → `get_split_keys_from_value_list`：手工模式的可测试执行门面。

已验证的下游边为 `get_split_index_keys` → `get_split_keys_from_value_list` 或 `get_split_keys_from_bound`，二者再调用 `encode_index_key`；范围路径另外调用 `padded_u64`。RustCodeGraph 能定位目标文件的 17 个符号和主要签名，但对这些查询未返回调用边，因此以上调用关系以仓库精确引用搜索和源码读取交叉核验。

## 错误处理与边界

- 显式值编码当前没有可失败步骤，因此 `get_split_keys_from_value_list` 虽返回 `Result`，现实现总是 `Ok`；空输入返回空列表。
- 范围模式只显式校验 `count != 0` 和编码后的 `lower < upper`。`count == 1` 返回零个内部点；等值或逆序边界返回 `InvalidBounds`。
- `get_split_index_keys` 在 `value_lists` 非空时完全忽略 `lower`、`upper` 和 `num`，调用者必须避免同时提供相互矛盾的两类参数。
- `split_index_region_and_wait` 只把 split 回调的 `Err(String)` 映射为 `SplitError::Split`；scatter 回调不能返回错误，未完成与查询失败无法区分，也没有重试、等待间隔或超时概念。
- `eval_split_datums` 只识别缺失结果，不解析表达式、不做列数检查或目标列类型转换。

因此，调用者不能把这些错误当作 Go 用户可见错误码/消息的完整等价物。实际 SQL 参数上限（如 region 数上限）和表达式检查还依赖调用链其他层。

## 并发与资源生命周期

本文件自身不创建线程、任务、锁、channel、事务或 context。所有输入通过借用传入，输出为新分配的 `Vec`；回调仅在函数调用期间同步执行。`split_index_region_and_wait` 按 region ID 顺序串行调用 `scatter_finished`，不保存回调或 region 状态。

Go 对照中的 `splitIndexRegionAndWait` 会派生带 `tidb_split_region_timeout` 的 context、调用 `kv.SplittableStore.SplitRegions`、注入 failpoint，并在超时后仍以短 backoff 检查剩余 region；Rust 帮助函数没有这些生命周期保证。当前 `pkg/session/runtime/ddl.rs` 的 Rust 接线也只生成 key 和触发 failpoint，所以不能据本文件推断真实 Region split/scatter 已发生。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/index_presplit.go`。命名和大体分工对应如下：`get_split_index_keys` 对应 `getSplitIdxKeys`，值列表与范围函数分别对应 `getSplitIdxKeysFromValueList`/`getSplitIdxPhysicalKeysFromValueList` 和 `getSplitIdxKeysFromBound`/`getSplitIdxPhysicalKeysFromBound`，执行包装器对应 `splitIndexRegionAndWait`，求值帮助器对应 `evalSplitDatumFromArgs`/`evalConstExprNodes`。

已对齐的局部语义包括：显式行按给定顺序生成 key；范围 lower 必须小于 upper；`regions = 1` 不产生内部点；范围插值采用与 Go `util.GetValuesList` 相同风格的公共前缀和前八字节大端等分算法。`pkg/ddl/index_presplit_test.rs` 用本地 `go_values_list` 复算该算法。

尚未对齐的关键能力包括：Go 使用 `tables.NewIndex(...).GenIndexKey` 和 statement context 生成真实可路由索引 key，并加入索引起止边界；按普通表、global index 与本地分区扩展 physical IDs；校验值数量和列类型；支持默认 MinNotNull/MaxValue 边界；区分不支持 split 的存储；带超时执行 split 并等待 scatter；保留具体 TiDB 错误。Rust 的 `SplitError`、简化编码和回调 API 只覆盖这些行为的一个小子集。

从 DDL 框架视角，本功能是 add-index job 内的优化准备动作，不独立修改 schema version，不产生额外 schema state，不负责 reorg checkpoint，也没有自身 rollback/delete-range 语义；job 的暂停或取消仍应由上层 job/context 链处理。

## 扩展指南

若要扩展为真实生产编码，首要修改点是 `encode_index_key`：应复用 table/index codec、statement context、时区、collation 和句柄规则，而不是继续扩充 `Debug` 文本格式；这通常要求调整公开函数参数，并同步 `pkg/session/runtime/ddl.rs`、`pkg/ddl/index_auto_presplit.rs` 及其调用者。

若要补齐分区/global index、首尾边界或临时索引语义，应先决定职责位于调用者还是本模块，避免两层重复展开 physical table 或重复转换 temp index key。若要让 `split_index_region_and_wait` 接入存储，应加入 context/超时、unsupported-store 表达、可诊断 scatter 错误及取消语义，并与 Go 手工模式“严格失败”、AUTO 模式“best effort”的策略保持一致。

所有行为修改都应放在独立测试文件中，不把测试嵌入本生产文件。至少同步 `pkg/ddl/index_presplit_test.rs`；路由前缀行为同步 `pkg/ddl/index_nokit_test.rs`；SQL 接线和错误语义同步 `tests/realtikvtest/addindextest3/functional_test.rs`；AUTO 复用变化同步 `pkg/ddl/index_auto_presplit_test.rs`。兼容风险集中在 key 字节格式和已有调用签名，正确性风险集中在排序、重复点、分区/global/temp index 路由，性能风险集中在大量切分点分配以及串行 scatter 等待。

## 验证依据

- 源码与边界：`pkg/ddl/index_presplit.rs`；`pkg/ddl/backfilling.rs::Key`；`pkg/ddl/index_cop.rs::Datum`。
- crate 与模块接线：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`。
- Rust 上游：`pkg/session/runtime/ddl.rs`、`pkg/ddl/index_auto_presplit.rs`。
- 独立 Rust 测试：`pkg/ddl/index_presplit_test.rs`、`pkg/ddl/index_nokit_test.rs`；SQL/RealTiKV 覆盖位于 `tests/realtikvtest/addindextest3/functional_test.rs`（本纯文档任务未运行测试）。
- Go 对照：`pkg/ddl/index_presplit.go`；其中 `preSplitIndexRegions`、`getSplitIdxKeys*`、`splitIndexRegionAndWait`、`evalSplitDatumFromArgs`、`evalConstExprNodes` 用于核对当前移植范围与差异。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件，`files --filter pkg/ddl/index_presplit.rs` 显示目标文件有 17 个符号；`query` 定位了 `get_split_index_keys`、`get_split_keys_from_value_list`、`split_index_region_and_wait`、`eval_split_datums` 的签名。`callers`/`callees` 未返回边，故调用边另用精确仓库搜索验证。
- 人工复核结论：本文件存在是为了集中索引预切分 key 计算和抽象执行；当前真正运行的主路径是 add-index 参数到 key 生成，真实存储 split/scatter 仍未在该 Rust 路径中接通；安全扩展必须优先维护 key 编码、physical ID 和手工/AUTO 错误策略的一致性。
