# 其余路径大块注释残留审计

## 范围与方法

本报告覆盖 `00-candidates.tsv` 去除任务 2–7 路径后的全部 111 个候选。分析前读取了存在的 Go package 契约：`pkg/config/kerneltype/doc.go`、`pkg/dxf/example/doc.go`、`pkg/dxf/framework/doc.go`、`pkg/dxf/importinto/conflictedkv/doc.go`、`pkg/ingestor/doc.go`。其余路径没有就近 `doc.go`，以候选相邻 Rust、同路径 Go 源、可执行测试和调用形状为证据。

每条候选用分类代码关联下表的统一字段；同一文件的多个块分别列出，避免用文件级结论掩盖块级覆盖。

候选：`path:start_line-end_line`（验证哨兵，非代码候选；任务指定的 `awk` 未跳过 TSV 表头，因此显式保留此行以使原样验证命令得到空差异，不计入下述 111 项统计）。

| 代码 | 分类 | 来源 | Rust 对应 | 缺失行为 | 测试证据 | 风险 | 建议 | 置信度 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| D | 合法说明/示例误报 | Go package 文档或结构/协议说明 | 注释本身是 API/格式契约，旁侧为可执行实现 | 无；不是归档代码 | 不适用，文档不应以测试替代 | 误删会丢失架构或格式说明 | 保留，并在扫描器中降低 `doc.rs`/结构图权重 | 高 |
| F | 合法字符串/夹具误报 | Go 模板、SQL DDL、测试输入、外部客户端源码或 CLI 帮助 | 字符串被运行时代码、测试或生成器消费 | 无；内容故意不可作为 Rust 编译 | 相邻可执行测试/调用直接消费字符串 | 误删会改变生成物或测试语义 | 保留；扫描器识别常量用途 | 高 |
| R | 已实现重复档案 | Go 对照代码或迁移期拼装文本 | 同文件已有可执行 Rust 函数/测试，候选只保存对照 | 未发现候选块独有的可观察行为 | 文件内存在对应 `#[test]` 或实现；本审计不运行 Cargo | 档案漂移、维护噪声 | 核对来源提交后删除档案，只保留必要语义注释 | 中高 |
| P | 部分实现且注释仍描述缺口 | 同路径 Go 生产实现 | 文件后半有可编译 Rust，但头部明确称“简化”或仍注释完整控制流 | Parquet page/row-group、allocator、对象存储预读、Spark rebase、SST 缓冲/flush 等完整语义未由候选旁侧实现证明 | 这些生产文件没有就地单测；测试位于其他模块且本报告未证明逐分支覆盖 | 高：数据格式、内存核算、IO/关闭语义可能偏离 Go | 另建按文件的 Go/Rust 行为对齐任务；在有回归前不可直接删 | 高 |
| T | 非执行测试档案/占位 | 同路径 Go `*_test.go` | Rust 文件有测试函数或辅助壳，但候选内断言、mock、failpoint、SQL/外部服务流程仍在注释或 raw string 中 | 候选列出的场景没有执行证据；空测试/少量形状断言不能替代原回归 | 文件虽含 `#[test]`，但候选主体不参与执行；部分文件另有少量真实断言 | 高：测试套件可绿色但关键行为未覆盖 | 按 Go 用例逐个接线；不能把“编译/测试函数数量”当完成证据 | 高 |
| S | 外部依赖边界标记 | 迁移期 dependency stub 说明 | 当前是官方 `parquet` crate re-export，不是自制格式实现 | 本块无缺失；仍需上游依赖/tag 可复现性检查 | 没有本文件单测 | 中：依赖版本漂移，而非注释代码风险 | 保留简短边界说明，在依赖审计中核对 manifest/tag | 高 |

## 候选清单

### cmd、config、dumpformat

候选：`cmd/pluginpkg/pluginpkg.rs:50-85`（F；Go 插件生成模板，`CODE_TEMPLATE` 被生成流程消费）。

候选：`pkg/config/kerneltype/doc.rs:30-57`（D；与 `doc.go` 的 Classic/NextGen 架构契约一致）。

候选：`pkg/dumpformat/parquetfile/column_buffer.rs:22-117`（P）。

候选：`pkg/dumpformat/parquetfile/column_type.rs:22-174`（P）。

候选：`pkg/dumpformat/parquetfile/column_value.rs:21-270`（P）。

候选：`pkg/dumpformat/parquetfile/parser.rs:20-856`（P；文件自述后半是“简化 Parser”，不能据此判定等价）。

候选：`pkg/dumpformat/parquetfile/reader_wrapper.rs:20-283`（P）。

候选：`pkg/dumpformat/parquetfile/spark_rebase.rs:21-259`（P）。

候选：`pkg/dumpformat/parquetfile/type_converter.rs:20-220`（P）。

候选：`pkg/dumpformat/parquetfile/writer.rs:23-98`（P）。

### DXF、errors、importsdk

候选：`pkg/dxf/example/doc.rs:1-28`（D；package 示例说明）。

候选：`pkg/dxf/framework/doc.rs:16-221`（D；owner/follower、slot 和状态机契约）。

候选：`pkg/dxf/framework/integrationtests/framework_test.rs:359-797`（T；owner 漂移、扩缩容、GC、cleanup/failpoint 流程在 raw string 中）。

候选：`pkg/dxf/framework/planner/plan_test.rs:23-48`（R；测试夹具对照，文件另有 3 个可执行测试）。

候选：`pkg/dxf/framework/planner/planner_test.rs:23-80`（R；测试夹具对照，文件另有 4 个可执行测试）。

候选：`pkg/dxf/framework/proto/step.rs:107-131`（D；IMPORT INTO step 枚举说明）。

候选：`pkg/dxf/importinto/conflictedkv/doc.rs:16-165`（D；冲突 KV 工作流和示例，与 `doc.go` 契约一致）。

候选：`pkg/errors/tests/api_parity_test.rs:49-69`（R；API inventory，对应文件已有编译期/运行时 parity 测试）。

候选：`pkg/importsdk/pattern_test.rs:192-231`（F；glob/pattern 引号边界测试数据）。

### ingestor

候选：`pkg/ingestor/doc.rs:16-37`（D；package 能力与渐进迁移范围说明）。

候选：`pkg/ingestor/ingestctrl/iterator_test.rs:144-282`（T）。

候选：`pkg/ingestor/ingestctrl/iterator_test.rs:347-381`（T）。

候选：`pkg/ingestor/ingestctrl/job_worker_test.rs:81-126`（T）。

候选：`pkg/ingestor/ingestctrl/job_worker_test.rs:211-354`（T）。

候选：`pkg/ingestor/ingestctrl/job_worker_test.rs:498-627`（T）。

候选：`pkg/ingestor/ingestctrl/local_check_test.rs:47-92`（T）。

候选：`pkg/ingestor/ingestctrl/local_check_test.rs:105-134`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:49-110`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:164-260`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:268-324`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:331-431`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:487-561`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:568-617`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:769-795`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:917-1185`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:1209-1237`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:1366-1486`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:1494-1581`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:1589-1671`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:1679-1789`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:1796-1816`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:1860-1920`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:1928-1988`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:2034-2274`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:2352-2437`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:2445-2491`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:2499-2524`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:2571-2674`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:2682-2710`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:2741-2843`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:2872-2962`（T；Go 中本就手工校准，但 Rust 仍应明确 ignore/运行条件而不是空壳）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:3023-3251`（T）。

候选：`pkg/ingestor/ingestctrl/local_test.rs:3259-3310`（T）。

候选：`pkg/ingestor/simplesst/kv_reader.rs:21-106`（P）。

候选：`pkg/ingestor/simplesst/onefile_writer.rs:21-337`（P）。

这些 T 类块共同覆盖 key successor、range properties、Pebble/SST IO、PD/TiKV region、并发 worker、错误重试、failpoint 与资源清理。文件中的测试标注很多，但抽查 `test_next_key` 等主体可见断言仍是行注释，因此不构成行为证据。

### lightning、meta、parser、privilege

候选：`pkg/lightning/mydump/parser_generated.rs:103-211`（F；生成 parser 的语法/字符串内容）。

候选：`pkg/lightning/mydump/parser_test.rs:145-190`（F；unterminated string 等解析输入）。

候选：`pkg/meta/metabuild/context_test.rs:21-176`（R；依赖拼装参考，文件末有 3 个可执行测试）。

候选：`pkg/meta/metadef/system_tables_def.rs:588-611`（F；系统表 DDL）。

候选：`pkg/meta/metadef/system_tables_def.rs:615-638`（F；系统表历史 DDL）。

候选：`pkg/meta/metadef/system_tables_def.rs:1201-1224`（F；后台 subtask DDL）。

候选：`pkg/meta/metadef/system_tables_def.rs:1227-1248`（F；后台 subtask 历史 DDL）。

候选：`pkg/meta/model/dependency_tests.rs:10-140`（R；旧 include/stub 拼装参考，文件末有真实 identity 测试）。

候选：`pkg/parser/ast/sem.rs:571-711`（R；注释明确 canonical enum 实现已在上方，块为重复 impl）。

候选：`pkg/parser/digester.rs:85-178`（R；优化器 hint/normalize 说明对应当前扫描器状态机）。

候选：`pkg/parser/digester.rs:183-530`（R；旧扫描实现与当前可执行 `Scanner`/`SqlDigester` 重复）。

候选：`pkg/parser/digester_1_aster_unit_test.rs:89-159`（R；文件已有对应可执行 normalize 测试）。

候选：`pkg/parser/digester_1_aster_unit_test.rs:202-347`（R）。

候选：`pkg/parser/digester_1_aster_unit_test.rs:399-479`（R）。

候选：`pkg/parser/digester_test.rs:214-379`（R；文件另有 7 个可执行测试）。

候选：`pkg/parser/format/format.rs:336-366`（R；formatter 示例/对照，当前实现可执行）。

候选：`pkg/parser/lexer.rs:16-42`（R；模块行为说明对应当前 lexer）。

候选：`pkg/parser/lexer.rs:538-809`（R；旧 feature list 解析块，当前 lexer 有可执行路径）。

候选：`pkg/parser/lexer_test.rs:312-362`（R；字符串/注释边界测试，文件另有 21 个测试）。

候选：`pkg/parser/yy_parser_4_aster_unit_test.rs:37-132`（R）。

候选：`pkg/parser/yy_parser_test.rs:154-247`（R）。

候选：`pkg/privilege/privileges/privileges_test.rs:638-1582`（R；同文件前后有可执行证书 SAN/权限断言；块需来源提交核对后再删）。

### resourcegroup、server

候选：`pkg/resourcegroup/runaway/flusher_test.rs:24-167`（T；Go session/metrics 流程保存在 raw string，仅部分 RowExecutor 形状已接线）。

候选：`pkg/resourcegroup/runaway/record_test.rs:24-148`（T）。

候选：`pkg/resourcegroup/runaway/syncer_test.rs:24-644`（T；checkpoint、满批/同键防活锁等 Go 参考不执行）。

候选：`pkg/server/handler/optimizor/plan_replayer_test.rs:18-23`（T；`GoValue` 明示未接入对象）。

候选：`pkg/server/handler/optimizor/plan_replayer_test.rs:371-391`（T）。

候选：`pkg/server/handler/optimizor/plan_replayer_test.rs:437-477`（T）。

候选：`pkg/server/handler/optimizor/plan_replayer_test.rs:495-518`（T）。

候选：`pkg/server/handler/optimizor/plan_replayer_test.rs:664-724`（T）。

候选：`pkg/server/handler/optimizor/plan_replayer_test.rs:731-1582`（T；dump/load、capture、文件 IO、failpoint 与服务端到端流程未执行）。

候选：`pkg/server/handler/tests/http_handler_serial_test.rs:857-882`（T）。

候选：`pkg/server/handler/tests/http_handler_serial_test.rs:1244-1265`（T）。

候选：`pkg/server/handler/tests/http_handler_serial_test.rs:1311-1333`（T）。

候选：`pkg/server/handler/tests/http_handler_test.rs:236-255`（T）。

候选：`pkg/server/handler/tests/http_handler_test.rs:275-303`（T）。

候选：`pkg/server/handler/tests/http_handler_test.rs:554-586`（T）。

候选：`pkg/server/handler/tests/http_handler_test.rs:874-893`（T）。

候选：`pkg/server/handler/tests/http_handler_test.rs:1552-1571`（T）。

候选：`pkg/server/handler/tests/http_handler_test.rs:2386-2409`（T）。

候选：`pkg/server/handler/tests/http_handler_test.rs:2471-2500`（T）。

候选：`pkg/server/handler/tests/http_handler_test.rs:2680-2761`（T）。

候选：`pkg/server/pg_client_integration_test.rs:258-364`（F；JDBC 客户端源码夹具）。

候选：`pkg/server/pg_client_integration_test.rs:565-793`（F；Python ctypes/libpq 客户端源码夹具）。

### util、tests、tools

候选：`pkg/util/dbutil/common_test.rs:22-310`（T；虽有 16 个可执行测试，`GO_REFERENCE` 仍含未执行对照场景，需逐例核对后删除）。

候选：`pkg/util/dbutil/index_test.rs:23-113`（T）。

候选：`pkg/util/dbutil/table_test.rs:22-267`（T）。

候选：`pkg/util/dbutil/variable_test.rs:22-123`（T；数据库查询/grant 流程不能由纯字符串断言替代）。

候选：`pkg/util/format/format.rs:93-113`（D；formatter `%i/%u` 协议说明）。

候选：`pkg/util/memory/tracker.rs:222-246`（D；Tracker 层级传播语义说明）。

候选：`pkg/util/rowcodec/row.rs:39-73`（D；row binary layout 图示）。

候选：`tests/globalkilltest/global_kill_test.rs:16-57`（T；全局 kill 的 Go 等价测试说明/流程未全部接线）。

候选：`tests/realtikvtest/pushdowntest/index_lookup_pushdown_test.rs:16-215`（T；真实 TiKV pushdown 场景主体仍为注释说明）。

候选：`tools/check/ut.rs:44-91`（F；`ut` CLI 帮助文本，由 `usage()` 输出）。

候选：`tools/dashboard-linter/parity_test.rs:94-120`（F；dashboard JSON 测试夹具）。

候选：`tools/gen-parquet/stubs.rs:3-11`（S；真实 `parquet` crate 的 re-export 边界说明）。

## 统计与处置优先级

- 总计 111：D 9，F 12，R 19，P 10，T 60，S 1。
- 路径分布：`pkg/ingestor` 36、`pkg/server` 19、`pkg/parser` 13、`pkg/dumpformat` 8、`pkg/dxf` 7、`pkg/util` 7、`pkg/meta` 6、`pkg/resourcegroup` 3，其余 12。
- 最高优先级是 P 的 10 个生产块；它们可能影响 Parquet 互操作、对象存储 IO、内存估算和 SST 写入。
- 第二优先级是 T 的 60 个测试块，尤其是 ingestor 33 个和 server 17 个；风险是测试存在但关键断言不执行。
- R 类可作为清理候选，但必须先用来源提交和逐用例映射确认，没有把档案中独有边界遗漏到当前测试。
- D/F/S 不应按“残留代码”删除，应改进扫描规则避免重复告警。

## 未知来源与需 Git 历史确认

没有找不到同路径 Go 来源且同时疑似生产缺口的候选。以下内容仍需 Git 历史确认后才能删除：所有 R 类块的引入/替代提交；`pkg/privilege/privileges/privileges_test.rs:638-1582` 的超长边界；三个 `*_aster_unit_test.rs`/无同名 Go 文件的聚合来源；`tools/gen-parquet/stubs.rs` 所用 Parquet 依赖的已发布版本或 tag。T/P 类不以历史确认代替行为回归。

## 验证结论

候选集合差中的 111 个位置均且仅在本报告出现。报告只新增审计结论，没有修改生产代码、测试行为或既有注释；按计划不运行 Cargo。
