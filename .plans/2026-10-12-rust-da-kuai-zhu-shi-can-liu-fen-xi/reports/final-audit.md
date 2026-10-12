# Rust 大块注释残留最终审计

## 结论

本轮对全仓库 6,959 个 Rust 文件做了保守的词法扫描，得到 580 个候选块，分布在 279 个文件。七份分片报告已将每个唯一范围审计且仅审计一次：已审计 580，遗漏 0，重复 0。

审计不支持“大块注释都可直接删除”的结论。只有 70 块已有可执行实现与相邻证据，可进入删除前的来源映射和回归阶段；410 块仍保存未完整证明的行为（部分缺口 279、完全未实现 36、非执行测试档案 95）；其余 100 块是合法说明、生成模板、测试夹具、执行中的 Rust 或扫描器词法误报，不应当作残留代码删除。

## 发现规则与覆盖边界

扫描使用两层发现：第一层识别至少 20 行且具有足够代码特征的连续行注释、块注释和 Rust 原始字符串；第二层用 `_GO_DRAFT_ARCHIVE`、“机械迁移占位”、“非可执行 Rust”等迁移标记词补足短块。重叠区间按路径合并，纯许可证块被排除。详细规则、可复现命令和已知盲区见 `00-scan-method.md`。

载体分布为：纯行注释 440，纯块注释 18，纯原始字符串 119，行注释+块注释 2，行注释+原始字符串 1。顶层路径分布为 `pkg/` 518、`br/` 36、`lightning/` 20、`tools/` 3、`tests/` 2、`cmd/` 1。`dumpling/` 无候选只表示本规则未命中，不等于实现完整。

该方法是词法启发式而非 Rust token tree 分析；不覆盖非 `.rs` 文件、`include!` 外部内容或构建时才生成的文件，普通转义字符串只在命中迁移标记时进入。因此“候选集零遗漏”特指 `00-candidates.tsv` 与七份分片报告的集合一致，不代表词法规则能发现所有可能的迁移缺口。

## 统一分类、优先级与置信度

| 分类 | 数量 | 基线优先级 | 处置原则 |
| --- | ---: | --- | --- |
| 已实现重复档案 | 70 | P3 | 先建立 Go 差异/测试映射，运行聚焦回归，再删除档案；保留精炼的语义或安全不变量说明。 |
| 部分实现且注释仍描述缺口 | 279 | P1 | 按可观察行为分组补齐生产接线或回归，不以编译成功、局部内存模型或存在测试名代替对齐证据。 |
| 完全未实现占位 | 36 | P0 | 全部位于 DDL placement policy 测试空壳；优先恢复 create/alter/drop、exchange/reorg、rollback/GC 等真实 owner/job 链路验收。 |
| 非执行测试档案 | 95 | P2 | 将 Go 用例名、夹具、failpoint、SQL 和断言逐项映射为可执行测试；不保留仅检查 archive `contains` 的伪覆盖。 |
| 合法说明/模板/夹具/词法误报 | 100 | 无处置 | 保留正在被运行时、生成器或测试消费的内容；调整后续扫描器的排除规则。 |

这是可复算的初始排序，具体任务可根据数据正确性和外部依赖风险上调：Planner partition processor、Executor builder/dispatch、Statistics 持久化、MVCC、Parquet/SST 和 Lightning 数据面都是 P0/P1 审查热点。

分片的置信度字段并未全部统一到单候选，因此不伪造 580 项的精确置信分布。有显式字段的 142 项中，123 项为高置信，19 项为中高置信；其余 438 项由分片给出路径、实现和测试证据，但未单独记录置信度。删除档案前仍须根据来源提交和回归二次确认。

## `domain_sysvars.rs` 逐回调结论

`pkg/domain/domain_sysvars.rs:21-166` 归类为“已实现重复档案”，但当前 Rust 通过可观测状态边界而不是真实客户端对象表达部分副作用。逐项结论如下：

| Go 回调意图 | Rust 对应与结论 | 删除前证据 |
| --- | --- | --- |
| 统计缓存容量 | `DomainSysVars::set_stats_cache_capacity` 已执行，保留不额外 clamp 的行为。 | `domain_sysvars_test.rs` 容量回归。 |
| PD 动态选项（包括 RPC mode/duration） | `set_pd_client_dynamic_option` 已解析并更新可观测状态。 | duration 单位和 RPC mode 大小写回归；接入真实 PD client 时另建副作用计划。 |
| 外部时间戳 | `set_external_timestamp` 已执行。 | 保留现有状态可观测回归。 |
| 全局资源管控 | `set_global_resource_control` 已执行并保存可观测状态。 | 真实资源组客户端副作用不由旧草稿作为实现来源。 |
| 低分辨率 TSO | 由现有 `Domain` 配置路径承接，未发现只存在于注释的分支。 | 清理时保留边界说明。 |
| schema cache | 由现有 `Domain` 配置路径承接，未发现只存在于注释的分支。 | 清理时保留边界说明。 |
| 熔断错误率阈值 | `set_circuit_breaker_error_rate_ratio` 已执行。 | 现有测试证明比率不额外 clamp。 |
| 注册与初始化 | `Domain::init_domain_sys_vars` 已将上述设置器接入。 | 删除旧草稿前保留“外部客户端由边界适配”的普通说明。 |

## 后续实施批次

1. **P0 DDL placement policy 真实回归**：路径 `pkg/ddl/placement_policy_test.rs`；以 36 个空壳所对应的 create/alter/drop、exchange/reorg、rollback/GC 为验收；依赖 owner/job/schema-sync/TestKit 夹具。
2. **P0/P1 存储与数据格式对齐**：路径 `pkg/store/mockstore/unistore/tikv`、`pkg/store/copr`、`pkg/table/tables/test/partition`、`pkg/dumpformat/parquetfile`、`pkg/ingestor/simplesst`；验收 badger/region/latch、MVCC、partition DDL、Parquet page/row-group/rebase/IO 与 SST flush/close；依赖真实存储和对象存储夹具。
3. **P1 Planner 产品路径与规则对齐**：路径 `pkg/planner`；先逐函数核对 partition processor，再验证 plan clone/task base、TiFlash predicate pushdown、stats loading、index advisor 和 join-order hint/warning；依赖真实 session/planner 上下文。
4. **P1 Executor 统一 builder 与类型/失败矩阵**：路径 `pkg/executor`；验收 HashAgg、HashJoin V1/V2、Union、index/merge join 的统一 dispatch，并覆盖 datum/collation/SQL mode、取消、首错、spill、tracker 和清理。
5. **P1 Expression/Statistics 行为矩阵**：路径 `pkg/expression`、`pkg/statistics`；验收 string/time/regexp 逐行与向量化一致性、collation/typeinfer 端到端属性、priority queue DDL/failpoint/并发/冷却和 stats 系统表持久化；依赖 parser/planner/TestKit 和统计存储夹具。
6. **P1 外部服务边界**：路径 `br`、`lightning`；验收真实 PD/TiKV/gRPC、TLS、SQL、对象存储、checkpoint 原子性、远程部分成功后的幂等/取消/清理；外部 Rust 依赖须遵循独立上游仓库与已发布 tag 约束。
7. **P2 归档测试转可执行回归**：路径重点为 `pkg/ingestor`、`pkg/server`、`pkg/parser`、`pkg/session`、`pkg/executor/test`、`tests/realtikvtest`；按 Go 测试名建立一对一矩阵，将 raw string/注释中的 mock、failpoint、SQL 和断言变成真正测试。
8. **P3 已证明档案清理**：从 `pkg/domain`、`pkg/ttl/sqlbuilder`、Planner/Executor 低风险重复块开始；每批必须先记录 Go 差异或来源提交、对应 Rust 实现与聚焦回归，再删除档案和仅检查文本存在的测试。

顺序上先补 P0 的真实状态机回归，再补存储/格式与外部服务基础边界；Planner、Executor、Expression/Statistics 可在夹具就绪后并行。P2 应按相同夹具跟随对应 P1 任务，P3 只在行为证据已成立时进行。

## 可清理、应保留与待核查项

- 70 个已实现重复档案的删除前置证据是：精确的 Go 差异/来源提交、每个可观察分支的 Rust 实现映射、可执行回归与对应变更类型的 Ready 验证。仅有同名函数或测试不足以删除。
- 100 个合法说明/模板/夹具/词法误报应保留：它们包括 package 与算法契约、生成器输入、SQL/JSON/parser 测试数据、正在编译执行的 trait/impl，以及被普通 `"br"`/`.to_string()` 误扩展的文本。
- 没有“无法定位 Go 来源且疑似生产缺口”的未知项。仍需 Git 历史核查的是全部 P3/R 类的引入与替代提交、`pkg/privilege/privileges/privileges_test.rs:638-1582`、三个无同名 Go 文件的 `*_aster_unit_test.rs` 聚合来源，以及 `tools/gen-parquet/stubs.rs` 的上游依赖版本/tag。核查方式是比对完整 SHA 的差异、确认后续替代仍保留原行为，并运行聚焦回归。

## 验证与限制

- 候选全集 580，分片已审计 580，集合差异 0，重复 0；分片数量为 31 + 56 + 44 + 191 + 91 + 56 + 111 = 580。
- 五类统计为 70 + 279 + 36 + 95 + 100 = 580；优先级基线为 P0 36、P1 279、P2 95、P3 70、无处置 100。
- 本轮未修改、编译或运行生产代码；结论来自静态源码、RustCodeGraph 索引、调用关系、相邻测试源码、Go 来源和历史证据。“有测试证据”不表示本轮已实际运行该测试。
- 本轮只验证审计产物的集合一致性和 Markdown diff 质量；不运行 Cargo，不变更生产源码、测试行为或 `plan.md`。
