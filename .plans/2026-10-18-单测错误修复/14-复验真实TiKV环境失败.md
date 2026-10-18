# 任务 14: 复验真实TiKV环境失败

批次：【批次 14】依赖批次 13。线性顺序 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 13 → 14 → 15。本批次仅此任务，不与其他批次并行。

状态：未开始

目的：同一真实 TiKV 集群上五个 PiTR 加索引场景和真实 split_file 均实际执行成功，并完整清理资源。

来源任务：用户指定 `target/rust-test.R6H1p5`，对应失败清单见下方；不是逐 Go commit 移植任务。

预计会话范围：只围绕本项行为的复现、最小修复和聚焦验证；共享执行链的其他问题不并入。若调查发现多个独立根因，记录证据并请求拆分，不扩大到完整子系统。

## 文件

- 调查/按证据最小修改：`tests/realtikvtest/addindextest/pitr_test.rs`。
- 调查/按证据最小修改：`tests/realtikvtest/importintotest4/split_file_test.rs`。
- 调查/按证据最小修改：`tests/realtikvtest/testutils/lib.rs`。
- 调查/按证据最小修改：`tests/run-rust-tests.sh`。

若新增最小回归，优先加入列出的相邻独立测试文件；不得将 Rust 单元测试写入生产源文件。

## 上下文

五个 PiTR 用例因 ASTERSQL_REALTIKV_TEST_PATH 未设置立即 panic；split_file 从 REAL_TIKV_PD（默认 127.0.0.1:12379）连接 PD 失败。这只证明环境前置条件缺失，未证明数据库行为错误。

- 历史 FAIL，`target/rust-test.R6H1p5:18970`：`astersql-tests-realtikvtest-addindextest::pitr_test test_pitr_create_gen_col_index`。
- 历史 FAIL，`target/rust-test.R6H1p5:18995`：`astersql-tests-realtikvtest-addindextest::pitr_test test_pitr_create_multi_cols_index`。
- 历史 FAIL，`target/rust-test.R6H1p5:19020`：`astersql-tests-realtikvtest-addindextest::pitr_test test_pitr_create_non_unique_index`。
- 历史 FAIL，`target/rust-test.R6H1p5:19045`：`astersql-tests-realtikvtest-addindextest::pitr_test test_pitr_create_primary_key`。
- 历史 FAIL，`target/rust-test.R6H1p5:19070`：`astersql-tests-realtikvtest-addindextest::pitr_test test_pitr_create_unique_index`。
- 历史 FAIL，`target/rust-test.R6H1p5:19363`：`astersql-tests-realtikvtest-importintotest4::split_file_test test_split_file`。

同时阅读 `.plans/2026-10-18-单测错误修复/证据索引.md`。验证入口为根 Makefile 的 rust-test / rust-test-diagnose-one 对应 nextest 命令；`.config/nextest.toml` 默认 10 秒，diagnostic 串行 60 秒。以下精确 filter 避免同名用例和独立 test target 漏跑。

## Cargo 共享槽位规则

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

### 本仓库配置补充

`.cargo/config.toml` 同时配置 target-dir 和 build-dir；领取槽位后除 `CARGO_TARGET_DIR` 外再设置 `export CARGO_BUILD_BUILD_DIR="$CARGO_TARGET_DIR"`，使中间产物也留在领取槽位内，禁止继续使用全仓共享 build-dir。正常/失败/INT/TERM 用清理 trap 回收自己的 owner 记录及空锁目录；先等待所有使用该槽位的 Cargo 和测试子进程退出，保留缓存。不启动新的后台测试后立即退出持锁 shell。

## 测试计划

- 行为：同一真实 TiKV 集群上五个 PiTR 加索引场景和真实 split_file 均实际执行成功，并完整清理资源。
- 失败验证：优先原用例；涉及生产修改时先在列出的独立测试文件补一个固定输入回归，再使用该 crate 中实际新增测试全名精确过滤执行，必须记录真实全名和非零测试数。
- 预期失败原因：原日志已证明环境前置条件失败；本项以环境修复前后对照代替新增生产行为失败测试。
- 失败/慢测诊断命令（先领取槽位；最多单用例 60 秒）：

    env -u LDFLAGS cargo nextest run --locked -p astersql-tests-realtikvtest-addindextest -p astersql-tests-realtikvtest-importintotest4 --profile diagnostic --test-threads 1 --no-fail-fast -E '(binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_gen_col_index)) | (binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_multi_cols_index)) | (binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_non_unique_index)) | (binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_primary_key)) | (binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_unique_index)) | (binary_id(=astersql-tests-realtikvtest-importintotest4::split_file_test) & test(=test_split_file))'

- 通过验证命令（仍须默认预算通过，diagnostic 不替代交付）：

    env -u LDFLAGS cargo nextest run --locked -p astersql-tests-realtikvtest-addindextest -p astersql-tests-realtikvtest-importintotest4 --profile default --test-threads 1 --no-fail-fast -E '(binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_gen_col_index)) | (binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_multi_cols_index)) | (binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_non_unique_index)) | (binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_primary_key)) | (binary_id(=astersql-tests-realtikvtest-addindextest::pitr_test) & test(=test_pitr_create_unique_index)) | (binary_id(=astersql-tests-realtikvtest-importintotest4::split_file_test) & test(=test_split_file))'

- 模拟策略：使用原真实 mock-KV / Domain / Session 执行；仅复用现有明确边界的 StoreHelper 或 DDL barrier 模拟，不伪造 SQL 结果。真实 TiKV 项必须使用真实集群。

## 步骤

1. 记录当前 HEAD、工作区状态，阅读对应 Go 实现及测试，确认前序批次已退出构建与测试阶段。
2. 依 docs/agents/testing-flow.md 后台启动独占 playground、等待健康、设置两个入口变量并逐一重跑。测试路径为 tikv://127.0.0.1:12379?disableGC=true，REAL_TIKV_PD 只填 host:port。不得删断言、加 ignore、无环境静默 return 或用 mock 冒充真实通过。环境恢复后若暴露独立生产缺陷，记录具体证据并请求新增任务，不能吞并外部客户端移植。
3. 涉及行为修复时先保存新增或加强测试的预期失败证据，再做最小修改；若为已有修复确认/环境复验，按本任务说明记录不适用原因。
4. Rust 修改后先 `cargo fmt --all`，再执行通过命令和相邻验证；只在保持通过时重构。
5. 按完成条件交付，不修改只读 plan.md。

## 验证

所需证据：本项原集合至少 6 个有效测试、零失败/超时、退出码 0；新增行为回归必须先失败后通过。记录每项用例耗时、profile 和测试数。若原用例已有修复，记录当前通过和已有代码依据；不得宣称本任务修复了未修改的生产逻辑。

### 真实 TiKV 生命周期

本阶段独占新 playground 和 canonical catalog；不得复用/停止他人的集群。先确认 12379 未占用；若占用，选择空闲端口并一致更新以下变量与命令。使用唯一 tag，禁止删除别人的 data。先领取 Cargo 槽位，在同一 shell 内注册 EXIT/INT/TERM 清理：测试进程结束后停止并 wait 自己的 playground，再删除其 tag data，最后释放槽位。

    tiup playground --mode tikv-slim --tag astersql-r6h1p5-recheck --pd.port 12379 > target/r6h1p5-playground.log 2>&1 &
    PLAYGROUND_PID=$!
    export REAL_TIKV_PD=127.0.0.1:12379
    export ASTERSQL_REALTIKV_TEST_PATH="tikv://127.0.0.1:12379?disableGC=true"
    curl -f "http://${REAL_TIKV_PD}/pd/api/v1/version"

健康检查失败时在有界时间内重试，确认启动失败就收集日志并执行清理，不能无限等待。然后执行上文通过命令。五项 PiTR 都要检查真实 SQL/索引校验事件，split_file 不能由 split_file_with_canonical_kv 替代。

    kill "$PLAYGROUND_PID"
    wait "$PLAYGROUND_PID"
    rm -rf "${HOME}/.tiup/data/astersql-r6h1p5-recheck"
    ! curl -sf "http://${REAL_TIKV_PD}/pd/api/v1/version"

kill/wait 返回非零也须继续执行本阶段拥有资源的清理。保留 playground 日志。若环境不可用，只记录环境阻塞，不制造生产代码修复；真实验证缺失不得标记全部通过。

## 执行约束与交付检查

所有命令从仓库根目录执行，features 使用当前 manifest 默认集合，不添加 `--all-features`、`nextgen` 或 `intest` 改变本轮语义。根 workspace 的 default=[]；本轮日志没有证明启用了附加 feature。库用例由 nextest 精确 binary/test 过滤，RealTiKV 的 pitr_test/split_file_test 是独立 Cargo test target，不能改用 `--lib` 漏跑。

实现前使用 `skills/rustcodegraph/SKILL.md` 验证假设，读取目标包 `doc.go`（若存在）；DDL 必须读 `docs/agents/ddl/README.md`。Rust 测试与生产代码分文件，修复时一起维护对应测试，保留 PingCAP 许可证；真正修好后为受改 Rust 文件添加或保留顶部 `// Copyright 2026 AsterSQL.`。

迭代使用 WIP：仅本任务失败复现、回归和受影响相邻测试。交付使用 Ready：Rust 变更先 `cargo fmt --all`，再执行本任务通过命令、相邻检查和 `make lint`，并运行 `git diff --check`。格式化影响全仓，必须先协调源码占用，只保留本任务相关变化，不覆盖其他工作。`.agents/skills/tidb-verify-profile` 当前缺失，若仍缺失直接依 AGENTS.md 执行上述适用检查并如实报告。

本计划只新增文档，无需运行构建检查。实施阶段若只改 Rust，且不是 fresh clone、未遇 Bazel 工具链问题，则不因本任务自动运行 bazel_prepare；若触发 AGENTS.md 的任一完整条件（含新增 Go Test、Go imports/deps/source 或 Bazel metadata），先 `make bazel_prepare` 并纳入元数据。禁止自行运行 `make bazel_lint_changed`。

Rust testfailpoint 使用既有 RAII guard，并在成功、失败和中断退出时恢复；没有 manifest failpoint feature 的包不可臆造 feature。若补跑 Go failpoint 测试，使用 docs/agents/testing-flow.md 的包装脚本并恢复状态。禁止复制外部 Rust 依赖、本地 patch 或重写外部移植；确实需移植时必须独立上游提交/tag、统一 Git tag 依赖并清理临时副本，范围变化先请用户确认。

## 完成条件

每项生产修改都在本文件记录已读 Go 实现/测试位置或必不可少的本地接线理由，写明符号和预期行为。不得删断言、减少线程/循环/数据规模、吞错或增加空实现。先使用当前代码复现；已有修复通过时记录证据，不为制造变更撤销它。若当前未复现，保留未确认结论并记录复现范围，不能仅凭旧日志认定修复完成。

按 PLANS.md 在本文件维护进度、发现、设计决策、结果及提交 SHA；plan.md 始终只读。证据不足或缺环境时只在本文件记具体阻塞与命令；只有范围内实现已完成、无关原因阻止验证时，才可标记 `已完成，待回归` 并保留本文件。不因范围外缺口递归重建子系统。

达到证据要求后标记 `已完成`；有实际代码、测试、配置或文档修改时使用 `$git-commit` 只提交本任务变更，排除其他任务文件和工作区无关改动。只有本编号任务状态变更/删除则不提交。最终报告文件、WIP/Ready 选择、正确性/兼容性/性能风险、确切命令、退出码、有效测试数、槽位、CARGO_TARGET_DIR、未验证项及必要下一步。完成后可删除本编号文件，完整证据保留在最终回复和实际变更提交中；不删除其他任务或证据索引。

## 进度

- [ ] 当前复现和范围确认。
- [ ] 范围内修复或已有修复确认。
- [ ] 聚焦通过验证及适用 Ready 检查。

## 发现与设计决策

实施时仅追加本任务经代码或验证支持的结论；历史诊断见上下文，候选根因不是事实。

## 结果与回顾

当前仅完成规划，尚无新运行证据。执行者在本节记录实际命令、退出码、测试数、耗时、槽位、风险、提交及未验证边界。
