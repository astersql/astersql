# DDL、Table、Store 与基础存储残留审计

## 范围与结论

- 审计范围：`pkg/ddl`、`pkg/table`、`pkg/store`、`pkg/kv`、`pkg/structure`、`pkg/lock`；候选表在这些前缀下共 91 块、18 个文件（`pkg/structure` 与 `pkg/lock` 本次为零候选）。
- DDL 阅读契约：已阅读 `docs/agents/ddl/README.md` 与 `pkg/ddl/doc.go`；判断以 owner 驱动的持久化 job、schema state/version sync、reorg checkpoint、cancel/rollback 和 durable metadata 为行为证据，不以函数带有 `#[test]` 或编译成功代替。
- 分类计数：合法说明/测试夹具误报 4；已实现重复档案 7；部分实现且注释仍描述缺口 44；完全未实现占位 36。
- 来源核对：对应 Go 文件仍在同路径或相邻路径（例如 `pkg/ddl/placement_policy_test.go`、`pkg/ddl/tests/serial/serial_test.go`、`pkg/ddl/tests/tiflash/ddl_tiflash_test.go`、`pkg/store/copr/*_test.go`、`pkg/store/mockstore/unistore/tikv/mvcc_test.go`、`pkg/table/tables/test/partition/partition_test.go`）。

## 执行框架与跨候选证据

- DDL 文档描述 SQL executor → DDL executor → JobSubmitter → owner scheduler/worker → schema version sync → history 的链路；placement-policy 草稿没有进入这条链路，因此其空测试不能证明 create/alter/drop、exchange/reorg、回滚或 GC 行为。
- `pkg/ddl/placement/{bundle,constraints,meta_bundle}_test.rs` 的归档后存在真实 Rust 断言；这些可列为“已实现重复档案”，但删除前仍应逐断言确认 Go 数据表没有遗漏。
- `serial_test.rs` 和 `ddl_tiflash_test.rs` 把 Go 原体传给 `keep_go_source`，同时在同名测试中执行部分真实 TestKit 操作；故统一判为“部分实现”，不能把源字符串存在性或步骤记录当覆盖。
- coprocessor/MVCC/partition 文件已有真实单测，但大字符串仍包含外部 mockstore、failpoint、网络、持久化和完整 DDL 状态机路径；这些边界正是残余风险。
- 文档漂移：未发现 `docs/agents/ddl/README.md` 对 owner/job/schema-sync 主链的描述与本次查到的代码证据冲突；发现的是 Rust 测试迁移覆盖不足，不是该入口文档漂移。

## 逐候选记录

候选：`pkg/ddl/doc.rs:16-55`

- 分类：合法说明/测试夹具误报
- 片段：Package ddl is the core of TiDB DDL layer. It is used to manage the schema of
- 来源/意图：包级 Online DDL 不变量说明，复刻 `doc.go`；现状：纯文档，无待执行代码；测试：不适用；风险：删除会损失契约；建议：保留并从残留清单排除。

候选：`pkg/ddl/placement/bundle_test.rs:23-1598`

- 分类：已实现重复档案
- 片段：覆盖 Bundle 判空、克隆、对象 ID、leader DC 提取、字符串化、placement options 构造、Reset/Tidy 以及 range key 编解码测试。
- 来源/意图：Go 测试迁移档案；现状：后续真实测试覆盖 Empty/Clone/ObjectID、leader DC、options、Reset/Tidy、range key；缺口：本块所列核心行为已有可执行断言；风险：重复档案可能漂移；建议：逐项确认等价后删除档案，保留真实测试。

候选：`pkg/ddl/placement/constraints_test.rs:22-202`

- 分类：已实现重复档案
- 片段：覆盖一组 LabelConstraint 的构造、追加冲突检测与字符串还原测试。
- 来源/意图：Go 测试迁移档案；现状：后续真实测试覆盖构造、YAML、冲突追加、restore 与 fingerprint；缺口：本块所列核心行为已有可执行断言；风险：重复档案可能漂移；建议：逐项确认等价后删除档案，保留真实测试。

候选：`pkg/ddl/placement/meta_bundle_test.rs:22-431`

- 分类：已实现重复档案
- 片段：覆盖基于 meta/mockstore 的 table、partition、partition list 和 full table bundle 构造测试。
- 来源/意图：Go 测试迁移档案；现状：后续真实测试覆盖 table/partition/list/full bundles；缺口：本块所列核心行为已有可执行断言；风险：重复档案可能漂移；建议：逐项确认等价后删除档案，保留真实测试。

候选：`pkg/ddl/placement/rule_test.rs:54-77`

- 分类：已实现重复档案
- 片段：require 断言保持 Go 测试判断点；不接入 testify 或断言框架。
- 来源/意图：Go 测试迁移档案；现状：后续真实测试覆盖 Clone 与 NewRules 约束矩阵；缺口：本块所列核心行为已有可执行断言；风险：重复档案可能漂移；建议：逐项确认等价后删除档案，保留真实测试。

候选：`pkg/ddl/placement/rule_test.rs:79-203`

- 分类：已实现重复档案
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：Go 测试迁移档案；现状：后续真实测试覆盖 Clone 与 NewRules 约束矩阵；缺口：本块所列核心行为已有可执行断言；风险：重复档案可能漂移；建议：逐项确认等价后删除档案，保留真实测试。

候选：`pkg/ddl/placement_policy_test.rs:54-93`

- 分类：完全未实现占位
- 片段：infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:167-194`

- 分类：完全未实现占位
- 片段：infosync/PD bundle 调用属于外部依赖；这里只记录读取或校验 bundle 的意图。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:276-366`

- 分类：完全未实现占位
- 片段：Go: store := testkit.CreateMockStore(t)
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:377-478`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:529-553`

- 分类：完全未实现占位
- 片段：Go: var (
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:583-652`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:663-723`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:734-764`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:775-854`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:865-1008`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1061-1125`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1136-1197`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1208-1276`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1287-1374`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1406-1445`

- 分类：完全未实现占位
- 片段：Go: ids := make([]int64, 0, 32)
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1456-1538`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1549-1597`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1607-1628`

- 分类：完全未实现占位
- 片段：Go: dom := domain.GetDomain(ctx)
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1639-1765`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1776-1844`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1855-1931`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:1942-2026`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:2037-2108`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:2119-2237`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:2248-2349`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:2360-2435`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:2446-2618`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:2629-2775`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:2786-2852`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:2863-2987`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:2998-3087`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:3098-3212`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:3223-3323`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:3333-3358`

- 分类：完全未实现占位
- 片段：Go: OldLoop:
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:3369-3673`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/placement_policy_test.rs:3684-3860`

- 分类：完全未实现占位
- 片段：测试入口保留原始断言顺序，方便后续人工接入 Rust 测试框架时逐段迁移。
- 来源/意图：`placement_policy_test.go` 的 SQL/DDL/PD bundle 测试；现状：函数虽带 `#[test]`，主体仅为注释、空 helper 或 `go_step`，未提交 DDL job、未等待 schema sync、未读取真实 InfoSchema/PD；缺口：状态转换、失败回滚、GC/交换分区兼容路径和外部 bundle 返回均未验证；测试：仅空壳通过；风险高；建议：按 Go 测试逐场景接入 TestKit/failpoint/PD mock 后删除对应档案。

候选：`pkg/ddl/serverstate/syncer_test.rs:16-98`

- 分类：已实现重复档案
- 片段：server state syncer 在 etcd 中初始化、读取全局状态、watch 状态切换和 IsUpgradingState 缓存更新的语义。
- 来源/意图：Go 测试迁移档案；现状：后续真实测试覆盖 init/watch/cache、错误 JSON 与网络边界；缺口：本块所列核心行为已有可执行断言；风险：重复档案可能漂移；建议：逐项确认等价后删除档案，保留真实测试。

候选：`pkg/ddl/tests/serial/serial_test.rs:187-211`

- 分类：部分实现且注释仍描述缺口
- 片段：store := testkit.CreateMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:1483-1664`

- 分类：部分实现且注释仍描述缺口
- 片段：store := testkit.CreateMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:2009-2037`

- 分类：部分实现且注释仍描述缺口
- 片段：store := createMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:2194-2254`

- 分类：部分实现且注释仍描述缺口
- 片段：store := createMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:2523-2635`

- 分类：部分实现且注释仍描述缺口
- 片段：store := createMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:2827-2938`

- 分类：部分实现且注释仍描述缺口
- 片段：store := createMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:3157-3245`

- 分类：部分实现且注释仍描述缺口
- 片段：store := createMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:3454-3516`

- 分类：部分实现且注释仍描述缺口
- 片段：store := createMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:3647-3700`

- 分类：部分实现且注释仍描述缺口
- 片段：store := createMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:3942-3969`

- 分类：部分实现且注释仍描述缺口
- 片段：store := testkit.CreateMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:4224-4458`

- 分类：部分实现且注释仍描述缺口
- 片段：store := testkit.CreateMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:4833-4866`

- 分类：部分实现且注释仍描述缺口
- 片段：store := testkit.CreateMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:5074-5095`

- 分类：部分实现且注释仍描述缺口
- 片段：store := testkit.CreateMockStore(t)
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:5207-5269`

- 分类：部分实现且注释仍描述缺口
- 片段：var cluster testutils.Cluster
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/serial/serial_test.rs:5371-5402`

- 分类：部分实现且注释仍描述缺口
- 片段：if kerneltype.IsClassic() {
- 来源/意图：`serial_test.go` 单个测试函数原体；现状：同名 Rust `#[test]` 已运行部分 SQL/元数据断言，并通过 `keep_go_source` 保存剩余路径；缺口：字符串内未被相邻 Rust 断言覆盖的 failpoint、异步 job、GC/恢复边界仍不执行；风险中高；建议：逐函数做语句/断言差分，补齐后删除原体。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:121-163`

- 分类：部分实现且注释仍描述缺口
- 片段：s := &tiflashContext{}
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:249-275`

- 分类：部分实现且注释仍描述缺口
- 片段：If table is dropped after tikv_gc_safe_point, it can be recovered
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:455-539`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:729-764`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:990-1009`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:1122-1153`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:1504-1528`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:1603-1651`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:1837-1912`

- 分类：部分实现且注释仍描述缺口
- 片段：var maxTick ddl.TiFlashTick = 10
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:2091-2135`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:2323-2344`

- 分类：部分实现且注释仍描述缺口
- 片段：ctx, cancel := context.WithTimeout(context.Background(), to)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:2450-2526`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:2575-2597`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:2718-2757`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:2822-2861`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:2960-3007`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:3203-3225`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:3299-3356`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:3421-3444`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:3474-3500`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:3585-3616`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/ddl/tests/tiflash/ddl_tiflash_test.rs:3757-3831`

- 分类：部分实现且注释仍描述缺口
- 片段：s, teardown := createTiFlashContext(t)
- 来源/意图：`ddl_tiflash_test.go` 单个 helper/测试原体；现状：Rust 记录步骤并实现部分 TestKit 场景；缺口：真实 TiFlash mock、PD rule、轮询、GC/failpoint 与并发时序仍有字符串档案；风险高；建议：接入可控 TiFlash/PD 测试边界并逐块迁移。

候选：`pkg/kv/cachedb_test.rs:360-389`

- 分类：合法说明/测试夹具误报
- 片段：package freecache
- 来源/意图：freecache Go oracle，仅供 ignored 再生成/差分测试；现状：真实 Rust 缓存测试覆盖 union-get、并发、驱逐；缺口：oracle 本身不应执行；风险低；建议：保留为明确夹具。

候选：`pkg/store/copr/batch_coprocessor_test.rs:22-653`

- 分类：部分实现且注释仍描述缺口
- 片段：testing/require/mockstore/testutils/failpoint/backoff/context/goleak 等外部依赖均按 Go 语义保留为占位调用，后续需接入 Rust 测试基础设施。
- 来源/意图：Go batch coprocessor 全文件参考；现状：真实 Rust 测试已覆盖均衡、连续性、hash、RR、backoff、alive-store；缺口：完整拓扑/故障与 goroutine 泄漏场景未逐项证明；风险中；建议：建立 Go 场景映射表后拆除已覆盖段。

候选：`pkg/store/copr/coprocessor_cache_test.rs:23-293`

- 分类：部分实现且注释仍描述缺口
- 片段：testing/require/mockstore/testutils/failpoint/backoff/context/goleak 等外部依赖均按 Go 语义保留为占位调用，后续需接入 Rust 测试基础设施。
- 来源/意图：Go coprocessor cache 全文件参考；现状：真实 Rust 测试覆盖 key 布局、配置、admission、长度与驱逐；缺口：与 Go LRU/指标细节仍需逐项比对；风险中；建议：差分确认后删除归档。

候选：`pkg/store/copr/coprocessor_test.rs:23-1292`

- 分类：部分实现且注释仍描述缺口
- 片段：testing/require/mockstore/testutils/failpoint/backoff/context/goleak 等外部依赖均按 Go 语义保留为占位调用，后续需接入 Rust 测试基础设施。
- 来源/意图：Go coprocessor task/split/retry 全文件参考；现状：大量真实 Rust 测试覆盖 range、paging、retry、batch/lock/limiter；缺口：mockstore/failpoint/backoff/goleak 组合路径未由归档本身执行；风险高；建议：按 Go 测试名建立覆盖矩阵，补齐网络错误路径。

候选：`pkg/store/copr/mpp_probe_test.rs:22-232`

- 分类：部分实现且注释仍描述缺口
- 片段：这段逻辑覆盖 MPP failed store 探活流程、后台任务互斥、异常输入断言和 MPP server info LRU 缓存。
- 来源/意图：Go MPP failed-store probe/LRU 测试；现状：真实 Rust 测试覆盖失败恢复、探测周期、LRU、单后台 worker；缺口：Go client 异步/关闭语义未完整接线；风险中；建议：补真实 client 边界后删除档案。

候选：`pkg/store/copr/region_cache_test.rs:21-249`

- 分类：已实现重复档案
- 片段：对齐 Go 'region_cache_test.go' 的历史参考骨架。
- 来源/意图：Go 测试迁移档案；现状：后续真实测试覆盖 bucket split、coverage、fallback、overlap 与 send failure；缺口：本块所列核心行为已有可执行断言；风险：重复档案可能漂移；建议：逐项确认等价后删除档案，保留真实测试。

候选：`pkg/store/mockstore/unistore/tikv/detector_test.rs:1-34`

- 分类：合法说明/测试夹具误报
- 片段：Copyright 2026 AsterSQL.
- 来源/意图：许可证与测试模块说明连续命中；现状：后续真实测试覆盖环检测、清理、过期；缺口：无；风险低；建议：排除误报。

候选：`pkg/store/mockstore/unistore/tikv/mvcc_test.rs:22-898`

- 分类：部分实现且注释仍描述缺口
- 片段：UniStore MVCC 测试如何搭建 badger-backed test store、构造事务请求并断言锁/写入/回滚状态。
- 来源/意图：Go UniStore MVCC fixture 与场景；现状：内存实现真实测试覆盖 prewrite/commit/read/rollback/1PC/status/scan/GC/resolve-lock 原子性；缺口：注释明确 badger-backed DB、region/server/latch 路径仍占位；风险高；建议：另建上游存储依赖任务，不能以当前内存测试等同完整 Go 覆盖。

候选：`pkg/table/tables/test/partition/partition_test.rs:23-698`

- 分类：部分实现且注释仍描述缺口
- 片段：这段逻辑覆盖分区表 AddRecord、partition pruning、exchange partition、key partition、reorg partition 与 DDL 回滚测试。
- 来源/意图：Go 分区表 DML、pruning、exchange/reorg/rollback 测试；现状：真实 Rust 测试覆盖基础 range/hash/list 路由和 physical key；缺口：DDL job 状态机、exchange constraints、reorg 双写与取消回滚未真实执行；风险高；建议：走 owner/worker job 框架接入分区 DDL 回归。

候选：`pkg/table/tblsession/table_test.rs:18-18`

- 分类：合法说明/测试夹具误报
- 片段：Go 侧 'TestSessionMutateContextFields' 等逻辑的参考伪代码（非可执行 Rust）。
- 来源/意图：紧邻 Go 参考字符串的解释标记；现状：只说明下一候选非执行；缺口：由下一候选记录；风险低；建议：与下一块联合管理。

候选：`pkg/table/tblsession/table_test.rs:19-175`

- 分类：部分实现且注释仍描述缺口
- 片段：这段逻辑验证 tblsession.NewMutateContext 对 sessionctx.Context 各支持接口的转发语义。
- 来源/意图：Go tblsession mutate-context 转发测试；现状：仅保留 reference-presence 测试及少量 mock；缺口：sessionctx 各接口转发没有真实行为断言；风险中；建议：实现接口 mock 与逐字段转发测试后删除档案。

## 处置优先级

1. 高：`pkg/ddl/placement_policy_test.rs` 36 块空壳，以及 TiFlash、MVCC、partition 中涉及真实 PD/存储/DDL 状态机的缺口。
2. 中：serial 与 coprocessor 档案逐 Go 测试名建立行为矩阵，区分已执行断言和仅记录步骤。
3. 低：已实现重复档案在逐断言核对后删除；文档、oracle 和许可证误报保留并调整未来扫描规则。

## 验证记录

- 覆盖要求：本报告每个候选以唯一的 `候选：\`路径:起始-结束\`` 标识；覆盖命令应为空差异。
- 测试/调用证据：来自同文件真实 `#[test]`、对应 Go 文件，以及 DDL owner/job/schema-sync 入口契约。此任务只写审计报告，未运行 Cargo。
