# `pkg/planner/core/operator/physicalop/index_join_probe.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate，是 Rust 物理计划枚举中为 IndexJoin 构造并选择内侧 probe（逐外表行查找）访问计划的专用实现。模块由 `pkg/planner/core/operator/physicalop/lib.rs` 私有声明，主要入口 `best_probe` 也是 `pub(crate)`，因此它不是跨 crate API。

当前唯一生产调用点在 `base_physical_plan.rs` 的物理 Join 候选构造路径：当 IndexJoin 的内侧逻辑子树已有可构造的 lookup scan，且内侧连接键多于一列时，调用者以“连接输出行数 / 外侧行数”计算每次 probe 的平均输出行数，再调用 `best_probe`。成功返回的候选会替换默认 lookup plan，并把动态范围、可用连接键映射和尾列比较器回填到 `PhysicalIndexJoin`；没有候选时仍沿用原有 `build_index_join_lookup_scan` 路径。

## 核心职责

文件同时承担三组紧密相关的职责：

1. 从 `DataSource`、`LogicalTableScan`、`TiKVSingleGather` 或其单子节点包装中找到原始数据源，遍历可用于 IndexJoin 的 TiKV 索引路径。
2. 把连接等值条件转换为 ranger 可消费的占位模板，保留运行时真正需要的相关外表列表达式；必要时从 `OtherConditions` 提取索引下一列的动态不等式范围，并据此构造 `PhysicalTableReader`、`PhysicalIndexReader` 或 `PhysicalIndexLookUpReader`。
3. 修正 probe 侧基数估算并选择最佳候选：对只使用部分连接键的等值前缀设置 rows-after-access 下界，对 Fix44855 控制的 NDV 估算设置上界，恢复残余过滤条件之前的行数，最后结合计划成本、连接键覆盖数和等值列 NDV 决定胜者。

它生成的是优化期的物理计划与范围模板，不执行真实 KV 读取，也不负责 IndexJoin 运行期把每一行外侧值写入模板。

## 主要符号

- `ProbePathResult`：一个候选访问路径的结构化结果。`scan` 保存索引扫描及其范围/条件；`used_cols` 是 ranger 实际使用的索引前缀宽度；`eq_ndv` 是其中纯等值前缀的估算 NDV；`last_col_is_range` 与 `last_col_manager` 描述尾列是否由范围条件扩展；`key_offsets[index_offset]` 记录该索引列对应的内侧连接键位置，`-1` 表示不是可用连接键。
- `ProbeCandidate`：把最终可挂接的 `Box<dyn PhysicalPlan>` 与构造该计划所依据的 `ProbePathResult` 绑定，供调用者同时消费计划和 IndexJoin 元数据。
- `best_probe(logical, join, avg_rows)`：crate 内入口。识别数据源边界；对单子节点逻辑包装递归下钻；多子节点或无法识别数据源时返回 `Ok(None)`。
- `source_probe(source, join, avg_rows)`：核心实现。枚举、构造、估算并比较所有合格访问路径，返回成本最优候选。
- `access_rows_floor(ctx, stats, result, join_keys)`：当索引等值前缀只覆盖部分连接键时，按 `RowCount / eq_ndv` 给出每次 probe 扫描行数下界。缺少统计/结果、NDV 非正、尾列为范围、存在动态比较管理器、或所有连接键均已覆盖时返回 `0`。Fix44855 在此处默认开启，并被记录为相关优化器修复项。
- `apply_index_floor(access, index, floor, unique)`：对非唯一路径提高 access 行数到下界，同时按原 `index / access` 比例同步提高 after-index 行数；原 access 为零时使用比例 `1`。唯一访问路径或下界不更大时保持原值。
- `ndv_lower_bound(columns, histograms)`：为 Fix44855 的行数上界寻找 NDV 下界，顺序是单列统计、列集合完全匹配的索引统计、已初始化列统计中的最大 NDV；没有可靠值时返回 `-1.0`。
- `fix_map`、`ndv_is_close`：前者读取并解析会话变量 `tidb_opt_fix_control`；后者判断两个 NDV 是否足够接近，供候选比较决定优先按成本还是按 NDV 排序。

## 执行流程

1. `best_probe` 先尝试直接把逻辑节点降型为 `DataSource`；随后处理持有 `Source` 的 `LogicalTableScan` 和 `TiKVSingleGather`；再对恰有一个子节点的包装递归。只有找到原始数据源后才进入 `source_probe`。
2. `source_probe` 取得计划上下文和 Join 的内外连接键。访问路径优先来自 `PossibleAccessPaths`，为空时回退到 `AllPossibleAccessPaths`；随后按“索引 ID + 是否表路径”去重，并排除 `IsIndexJoinUnapplicable` 或非 TiKV 路径。普通二级索引直接取 `path.Index`，common handle 表路径则可使用主索引元数据。
3. 对每条路径调用 `IndexInfo2PrefixCols` 得到索引列和前缀长度。数据源条件先被克隆；凡引用动态内侧连接键的静态条件都移入 `key_filters`，避免占位常量被误当成真实 probe 值参与求交。
4. 对每个索引列建立 `key_offsets`。若列对应某个内侧连接键，则向 ranger 条件加入“内侧列 = 整数 0”的临时模板，同时单独保存“内侧列 = 相关外侧列”的真实访问表达式。`DetachCondAndBuildRangeForIndex` 用模板与剩余静态条件构造范围；没有可用前缀或前缀中没有连接键的路径立即淘汰，未被 range 使用的后缀映射被重置为 `-1`。
5. 如果 ranger 的最后一列还不是范围列，并且索引仍有下一列，代码扫描 `OtherConditions` 中的二元 `lt/le/gt/ge`。只有一侧是该索引列、另一侧引用外部 schema 列时才加入 `ColWithCmpFuncManager`；索引列位于右侧时会反转比较方向。随后尝试给每个点范围追加带 `binary` collator 的空 Datum 槽位；超过 `RangeMaxSize` 时放弃该动态尾列，否则增加 `used_cols` 并标记尾列为范围。
6. 用纯等值部分（排除尾部范围列）估算 `eq_ndv`。非伪统计调用 `EstimateColsNDVWithMatchedLen`，伪统计则记为零。之后初始化 `PhysicalIndexScan`，复制表、索引、schema、直方图和标识信息，把模板条件从 `AccessCondition`/`FilterCondition` 中移除，并将范围中真正由连接键占据的位置恢复为空 Datum；真实相关列等式则重新加入访问条件。
7. 路径只有在唯一索引全部列均被等值条件完整覆盖时才标记 `unique`。基于 `avg_rows` 得到初始输出行数：主键路径在非正估算时使用 `1`；随后应用 NDV 上界和唯一性上限。残余过滤被拆成只引用索引列的 `index_filters` 与需要回表的 `table_filters`，选择率估算失败、非正或缺失时使用 `0.8`，逐层反推 access 与 after-index 行数。
8. `access_rows_floor` 给出部分连接键路径的扫描下界。主键路径直接用总体残余选择率恢复 access 行数；二级索引通过 `apply_index_floor` 保持索引过滤比例。随后分别设置 IndexScan 和 TableScan 的统计信息。
9. 按路径形态包装最终计划：主键/common-handle 表路径生成 `PhysicalTableReader`；覆盖所需列的单读二级索引生成 `PhysicalIndexReader`；需回表时生成 `PhysicalIndexLookUpReader`。非空残余条件通过 `PhysicalSelection` 包装，并设置 `FromDataSource = true`。
10. 对候选调用 `get_plan_cost_ver2`。通常选择成本更低者；若新旧候选覆盖相同数量的连接键且 NDV 明显不接近，则优先选择 `eq_ndv` 更大的路径，避免较低 NDV 路径因失真的 post-join 基数而获胜。

## 数据与状态

核心状态是三个不同视角的数量：`key_offsets` 表示索引列到 Join key 的映射，`used_cols` 表示实际进入范围的索引前缀，`eq_ndv` 只覆盖其中非范围的等值部分。三者不能互换；例如动态尾部范围会增加 `used_cols`，但不会进入 `eq_ndv`，同时会令 rows-after-access 下界不适用。

范围中的空 `Datum` 是运行时填充值的模板槽，而不是 SQL 常量。临时整数零只用于让 ranger 推导范围形状，构造完成后会从扫描条件中剔除；真正的相关外表列等式被恢复到 `AccessCondition`。每个 range 的 `LowVal`、`HighVal` 和 `Collators` 必须保持同宽，动态尾列分支显式追加三者以维持该不变量。

统计行数按执行层级分别维护：`access` 是索引/主键范围后读取量，`after_index` 是索引残余条件之后的数量，`output_rows` 是全部过滤后的目标输出量。`scan_stats`、`table_stats` 以及 reader/selection 的 stats 都是从 `source.TableStats` 克隆后仅修改 `RowCount`，不会原地改写数据源统计。

Fix44855 有两个独立默认值：部分连接键的扫描下界默认开启；二级索引基于 NDV 的行数上界默认关闭。显式 `44855:OFF` 会关闭两者。两条路径均调用 `RecordRelevantOptFix`，使计划缓存/诊断能知道该计划依赖此修复开关。

## 依赖与调用关系

上游调用链为 `base_physical_plan.rs` 的 Join 物理候选枚举 → `best_probe` → `source_probe`。返回后，上游从 `ProbePathResult` 重写 `OuterJoinKeys`、`InnerJoinKeys`、`KeyOff2IdxOff`、`Ranges`、`IdxColLens` 和可克隆的 `CompareFilters`，并把 `ProbeCandidate.plan` 设为 `PhysicalIndexJoin.InnerPlan`。

主要下游依赖如下：

- `logicalop` 提供 `DataSource`、逻辑包装节点、schema、访问路径和表统计入口。
- `planner_util::IndexInfo2PrefixCols` 提供与索引元数据对齐的列/前缀长度；`ranger::DetachCondAndBuildRangeForIndex` 与 `AppendRanges2PointRanges` 负责静态模板范围和动态尾槽构造。
- `expression` 构造临时/相关等式、克隆条件、提取列并格式化范围信息；`ColWithCmpFuncManager` 保存运行时非等值比较表达式。
- `cardinality` 估算等值列 NDV 与残余过滤选择率；`statistics::HistColl` 为 Fix44855 上界提供列/索引 NDV。
- `PhysicalIndexScan`、`PhysicalTableScan`、三个 reader 和 `PhysicalSelection` 组成最终 probe 子计划；`costusage` 与 `get_plan_cost_ver2` 负责候选成本比较。
- `Cargo.toml` 将本模块归入 `astersql-planner-core-operator-physicalop`，并以本地 crate 依赖连接 `base`、`logicalop`、`property`、`planner_util`、`ranger`、`statistics`、`cardinality`、`costusage`、`expression`、`kv`、`fixcontrol` 等边界；该 manifest 没有为本模块定义条件 feature。

RustCodeGraph 将本文件标记为被 `base_physical_plan.rs` 使用；图查询也确认 `best_probe` 调用 `source_probe`，而 `source_probe` 调用本文件的四个估算/比较辅助函数并实例化两个结果结构。

## 错误处理与边界

正常的“不适用”统一表现为 `Ok(None)` 或跳过当前路径，包括：没有计划上下文、逻辑树不是支持的数据源/单子节点包装、无路径、非 TiKV、无索引元数据、索引列为空、ranger 未使用任何前缀、或已用前缀没有任何连接键。这些情况不是优化器错误，允许调用者回退到默认 lookup scan。

真正可能失败的操作通过 `Result<_, expression::Error>` 向上传播：`expression::NewFunction` 构造表达式时使用 `?`；ranger 错误被转成 `expression::errors::New(e.to_string())`；克隆扫描计划和 `get_plan_cost_ver2` 的错误也使用 `?`。选择率估算错误不会中止枚举，而是退化到 `0.8`；FixControl 文本解析失败则退化为空 map。

范围大小受 `SessionVars.RangeMaxSize` 约束。追加动态尾列若触发 fallback，就保留原范围与原 `used_cols`，不会生成部分扩展的模板。NDV 缺失、未初始化或伪统计会禁止对应上下/下界，而不是伪造确定值。唯一性的一行上限只在完整等值覆盖唯一索引时应用，common-handle 只匹配主键前缀时不会错误地压成一行。

条件分类还有两个重要边界：引用动态连接键的 datasource 条件必须作为残余过滤保留；动态不等式的另一侧必须至少引用一列且不能引用内侧 schema，否则不会建立 `ColWithCmpFuncManager`。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、显式锁或后台资源。所有候选状态都在一次 `best_probe` 调用栈内拥有：访问路径借用 `DataSource`，条件、范围、统计和计划节点在需要跨阶段保存时显式克隆，落选候选随循环迭代释放，最终只返回一个拥有所有权的 boxed 物理计划。

`LogicalTableScan.Source` 和 `TiKVSingleGather.Source` 通过 `borrow()` 做短生命周期只读借用，并在递归 `source_probe` 返回前保持有效；代码不跨线程保存该借用。`PlanContext`、schema 和计划上下文以共享引用或克隆的 context handle 传递，本文件不改变其同步模型。唯一可观察的会话级副作用是记录 Fix44855 为相关优化器修复项。

资源方面，主要风险是候选枚举时的表达式/范围/计划克隆和成本计算；`RangeMaxSize` 限制动态范围扩张，但文件本身没有额外缓存或手动释放要求。

## 与 Go 版本的对应关系

Rust 实现不是对某个同路径 Go 文件的逐函数翻译；对应逻辑分布在 `pkg/planner/core/index_join_path.go` 与 `pkg/planner/core/exhaust_physical_plans.go`。

- Rust `ProbePathResult` 对应 Go `indexJoinPathResult` 中的 `usedColsLen`、`eqUsedColsNDV`、`lastColIsRange`、`idxOff2KeyOff`、`lastColManager` 以及已选路径/范围信息。
- Rust 的路径枚举、mapping 和最佳结果选择对应 Go `getBestIndexJoinPathResultByProp`、`indexJoinPathBuild` 与 `indexJoinPathCompare`；Go 按 `IndexJoinRuntimeProp` 向下构造任务，Rust 在 `base_physical_plan.rs` 中直接为已枚举的物理 Join 生成 boxed 内侧计划，因此装配层级不同。
- Rust `access_rows_floor` 对应 Go `indexJoinProbeAccessRowsFloor`：都只在范围使用部分连接键、纯等值前缀 NDV 有效且没有尾列范围/比较管理器时返回 `TableStats.RowCount / eqUsedColsNDV`，并共享默认开启的 Fix44855 门控。
- Rust `ndv_lower_bound` 对应 Go `getColsNDVLowerBoundFromHistColl`；两者都按单列、完全匹配索引列集合、已初始化列最大值的顺序寻找下界。Rust 返回 `f64` 以直接进入本地行数计算，Go 返回 `int64` NDV。
- Rust 对二级索引的过滤拆分、NDV 上界和行数反推对应 Go `constructDS2IndexScanTask`；主键/表路径下界对应 `constructDS2TableScanTask`。Rust 在同一函数内直接建立三类 reader，Go 则先形成 `CopTask` 再转换/挂接。
- Go 回归 `pkg/planner/core/casetest/join/join_test.go::TestIssue69974IndexJoinProbeAccessRows` 验证 Fix44855 开启时选择覆盖两个连接键的 `idx_k1_k2`，关闭时恢复只使用 clustered PK 前缀的旧估算。Rust 独立测试用更小的结构化输入验证相同下界开关和路径元数据，不等同于完整 SQL 集成覆盖。

需要注意的已验证差异是：Go probe 构造还包含大 IN-list 残余过滤的 root/coprocessor 拆分等任务层逻辑；本 Rust 文件没有对应分支，不能据此声称该策略已由此模块支持。

## 扩展指南

新增访问路径资格或路径排序规则时，应优先修改 `source_probe` 的路径过滤、构造或 `current_is_better`，并确认不会改变 `key_offsets` 的“索引列 → 连接键”方向。任何新增 range 形态都必须同步维护 `used_cols`、`last_col_is_range`、`eq_ndv` 的边界，并保证每个 range 的 low/high/collator 等宽。

新增动态比较运算符时，应修改提取 `OtherConditions` 的运算符白名单与左右交换映射，同时验证表达式另一侧只能引用外表列；相应测试应放在独立的 `index_join_probe_test.rs`，不要内嵌到生产文件。扩展过滤分类或 reader 计划形态时，需要同步检查 `PhysicalSelection.FromDataSource`、各层 `RowCount` 和 `DataSourceSchema`，否则成本或回表 schema 可能失真。

调整 Fix44855 时必须同时审查 `access_rows_floor` 的默认 ON 语义、`ndv_lower_bound` 上界的默认 OFF 语义、`RecordRelevantOptFix` 调用，以及 Go 的 `indexJoinProbeAccessRowsFloor`、`getColsNDVLowerBoundFromHistColl` 和相关构造函数。性能验证应关注候选数、范围内存、表达式克隆、选择率调用和每个候选的 `get_plan_cost_ver2`。

最低测试同步面是 `pkg/planner/core/operator/physicalop/index_join_probe_test.rs`：为纯行数函数增加表驱动边界，为范围模板增加 collator/槽位/manager 断言，为候选选择增加至少两个竞争路径。若改动影响最终 SQL 计划选择，还应同步 Go 对照回归或 Rust 对应的计划集成测试；现有 Go 证据包括 Issue 69974 场景。

## 验证依据

- 源文件：`pkg/planner/core/operator/physicalop/index_join_probe.rs`，RustCodeGraph 分段读取完整 734 行；主要符号为 `ProbePathResult`、`ProbeCandidate`、`best_probe`、`source_probe`、`access_rows_floor`、`apply_index_floor`、`ndv_lower_bound`、`ndv_is_close`。
- 调用图：`rustcodegraph query` 分别定位 `best_probe`、`access_rows_floor`、`apply_index_floor`、`ndv_lower_bound`；`callees best_probe` 给出 `source_probe`，`callees source_probe` 给出四个辅助函数、两个结果结构以及计划/表达式依赖。文件级索引显示生产使用者为 `pkg/planner/core/operator/physicalop/base_physical_plan.rs`。
- 上游接线：`base_physical_plan.rs` 约 10459--10549 行，验证多连接键门控、`avg_rows` 计算、键/范围/比较器回填和默认 lookup 回退。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml` 与 `lib.rs`，验证 crate 名、私有模块声明、独立测试模块以及直接依赖；manifest 未声明 feature。
- Rust 独立测试：`pkg/planner/core/operator/physicalop/index_join_probe_test.rs`。`index_floor_preserves_filter_ratio_and_unique_limit` 覆盖比例与唯一限制；`usable_keys_floor_guards_and_fix_control_recording` 覆盖所有下界门控及 Fix44855；`index_upper_bound_uses_initialized_column_and_matching_index_ndv` 覆盖 NDV 查找顺序；`dynamic_tail_range_keeps_typed_collators_and_comparison_manager` 覆盖 common-handle 前缀、动态尾列、collator 等宽和下界禁用。
- Go 对照：`pkg/planner/core/index_join_path.go` 的 `indexJoinPathResult`、`getBestIndexJoinPathResultByProp`；`pkg/planner/core/exhaust_physical_plans.go` 的 `indexJoinProbeAccessRowsFloor`、`constructDS2TableScanTask`、`getColsNDVLowerBoundFromHistColl`、`constructDS2IndexScanTask`；集成回归 `pkg/planner/core/casetest/join/join_test.go::TestIssue69974IndexJoinProbeAccessRows`。
- 本任务是纯文档分析，按总计划不运行 Cargo。交付结构检查要求文档存在且恰有上述 11 个固定二级标题。
