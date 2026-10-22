# PostgreSQL 18 核心兼容路线图

本路线图用于定义 AsterSQL 的 PostgreSQL 核心兼容目标及后续验收契约，组织为下列八个工作流。路线图是文档交付，不表示相应兼容功能、差分平台或运行环境已经实现。

总范围涵盖兼容契约、方言与解析、类型与转换、函数与运算符、SQL 会话与事务、系统目录、线协议以及客户端和双方言共存验收。各项均为待定义和验证的目标，不构成当前支持声明；本轮交付八章骨架和第 1 章的兼容契约与差分验证要求。

## 1. 兼容契约与差分验证

### 模块

本章建立契约及取证要求，模块定位从 [历史 PG 边界说明](postgresql-protocol-first-phase.md)、[连接模块说明](../pkg/server/pg_conn.rs.md) 和 [真实客户端测试入口](../pkg/server/pg_client_integration_test.rs) 开始。PG 专属连接与协议边界入口为 `pkg/server/pg_conn.rs`；共享会话和执行接入的调研入口为 `pkg/server/conn.rs`、`pkg/server/driver.rs`，沿现有内核继续定位，不另建存储、优化器或执行器。模块说明仅供导航，后续取证须以能力 ID 关联实际源码符号、独立测试和提交 SHA；这些入口及其中的历史测试记录不作为本轮当前运行证据。

### 目标行为

#### 范围与非目标

核心兼容目标是为应用使用 PostgreSQL SQL 与客户端接口时可观察的行为建立明确契约，并在后续任务中定义差分验证要求。契约应区分共享语义与方言专属语义，覆盖查询结果、类型及元数据、NULL、错误、会话与事务状态和客户端交互；本小节划定范围，能力矩阵、证据规则和用例格式分别由下列同名小节定义。

PostgreSQL **18.6** 是来源计划指定的目标基线；本轮未核实该补丁版本的发布状态，也未运行真实 PostgreSQL 对照。握手中报告的版本、协议协商成功或客户端可连接，都不能据此宣称当前具备 PostgreSQL 18.6 的 SQL、事务或完整功能兼容性。

兼容工作共享现有规范执行内核，通过明确的方言边界承接 PostgreSQL 目标行为，并保持 MySQL 方言既有契约。这里的共享内核指复用现有执行能力，不另建存储、优化器或执行器；共享实现本身也不证明两种方言的可观察语义相同。

本路线图明确排除扩展加载生态：CREATE EXTENSION、PostGIS、FDW、自定义过程语言、C 扩展、自定义类型/运算符类及第三方扩展加载生态均为非目标，不列为核心兼容交付或验收承诺。

本轮文档任务不实现生产代码、差分执行器、数据库环境或 CI；不扩展兼容能力，不撰写第 2–8 章正文。后续目标必须有相应实现和当前验证证据后，才能成为当前支持结论；既有说明或历史测试不能替代该证据。

#### 能力矩阵

矩阵以可独立验收的方言契约为单位，先记录目标，再逐项补齐当前证据。能力 ID 稳定标识契约，不随测试名称或实现位置改变；示例 ID 仅用于说明模板，不代表已经登记的支持能力。

| 字段 | 填写约定 |
| --- | --- |
| 能力 ID | 唯一标识场景与方言契约；同一场景有双方言契约时，用 `/pg`、`/mysql` 分别标识并关联。 |
| SQL/客户端场景 | 写明方言、输入 SQL 或客户端操作、参数、编码及必要会话前提，避免仅用函数名表达范围。 |
| 所属工作流 | 使用本路线图第 2–8 章的编号与标题指定责任归属；跨工作流时列主责及协作方。 |
| 分类 | 仅取共享语义、PG 专属、MySQL 专属、明确不支持；表示目标归属，不表示当前已经支持。 |
| 当前证据 | 未采集当前运行证据时填“未验证”；历史文档与测试路径只能注明为调研入口，不能当作实测结论。 |
| 目标结果 | 分方言记录期望的值、行或拒绝行为及适用边界；成功与拒绝均可成为验收目标。 |
| 类型与元数据 | 分方言记录预期结果类型、列名、可空性及协议元数据；PG OID 与 MySQL 类型标志各自验收，不要求相等。 |
| SQLSTATE | 记录各方言预期错误码及触发条件；成功场景填“不适用（成功）”，不能用无错误码表示未验证。 |
| 依赖 | 写明完成契约所需的工作流、能力 ID 或环境前提；无前置依赖时明确填“无”。 |
| 验收入口 | 标明后续用例的定位入口、双方对照或拒绝检查方式；计划入口须注明“待新增”，不能冒充已存在的测试。 |

分类中的“共享语义”要求列明范围内的可观察行为一致，仍分别核对两种方言的类型及协议元数据。“PG 专属”和“MySQL 专属”各自保留方言契约，不因共用执行内核就合并；“明确不支持”要求记录拒绝范围及错误目标，不能把尚未实现或未验证的目标自动归入这一类。

“同名异义分流”不是第五种分类：同一名字在两种方言下语义不同时，分别建立 PG 专属与 MySQL 专属契约及验收条件，并关联两个能力 ID。下表示意将这两个契约并列在一行以便比较；后续登记与验收分别追踪，不将其标为共享语义，也不以共享实现推导结果一致。

字段不得以空白或裸 `null` 隐去信息。“待定义”表示目标尚未细化，“未验证”表示缺少当前运行证据，“不适用（原因）”表示该字段在此场景无含义；这些状态不能相互替代。若目标要求返回 SQL NULL，应明确写“SQL NULL”及对应的类型、可空性；它是结果值，不是矩阵缺项。

以下仅有四条说明性示例，分别演示共享函数、同名异义、PG 专属与明确拒绝。所有结果和 SQLSTATE 都是待验收目标；类型细节标为待定义的条目须在后续实施前补齐。四条当前证据均为未验证，本轮没有运行 AsterSQL、PostgreSQL 或 MySQL 对照，示意行不能充当支持证明。

| 能力 ID | SQL/客户端场景 | 所属工作流 | 分类 | 当前证据 | 目标结果 | 类型与元数据 | SQLSTATE | 依赖 | 验收入口 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 示例 ABS-POS | PG 与 MySQL：`SELECT abs(1) AS v`，限定正整数字面量 | 4. 方言函数与运算符契约；协作 3. 共享类型与转换契约 | 共享语义 | 未验证 | 两方言均返回一行值 1；不由此推广到溢出、NULL 或其他重载 | 列名 v、整数结果；精确类型、可空性、PG OID 与 MySQL 类型标志待定义并分别核对 | 不适用（成功） | 工作流 2、3 的字面量解析与类型契约 | 待新增：工作流 4 的双方言函数用例，分别对照参考端的值及元数据 |
| 示例 LENGTH/pg；示例 LENGTH/mysql | 各方言 `SELECT length('é') AS v`，输入编码均为 UTF-8 | 4. 方言函数与运算符契约；协作 3. 共享类型与转换契约 | PG 专属；MySQL 专属（两个契约） | 两个契约均未验证 | PG 契约按字符数，目标 1；MySQL 契约按字节数，目标 2；后续分别采集对照证据 | 两端列名 v、整数结果；精确类型、可空性及各自协议元数据待定义 | 两个契约均不适用（成功） | 工作流 2、3 的方言分流与字符串编码契约 | 待新增：工作流 4 的两条关联用例，分别对照对应方言参考端，禁止跨方言要求值相等 |
| 示例 SEARCH-PATH/pg | PG 同一会话依次 `SET search_path TO public`、`SHOW search_path` | 5. SQL会话与事务语义；协作 2. 方言身份与解析边界 | PG 专属 | 未验证；历史说明仅为调研入口 | 设置后 SHOW 反映 public 搜索路径；此示例不承诺完整 schema 解析或 MySQL 同名操作 | SET 无结果列；SHOW 的文本结果类型、列名、可空性及 PG 元数据待定义 | 不适用（成功） | 工作流 2 的 PG 会话与名称边界 | 待新增：工作流 5 的同会话设置与读取用例，对照 PG 参考端 |
| 示例 EXTENSION-REJECT/pg | PG：`CREATE EXTENSION postgis`，假定连接及 SQL 传输有效 | 2. 方言身份与解析边界 | 明确不支持 | 未验证 | 按扩展加载非目标拒绝；不得创建扩展对象，不扩展为扩展生态实现任务 | 不适用（拒绝命令，无成功结果集） | PG 目标 0A000（不支持功能）；当前映射未验证 | 工作流 2 的拒绝边界及工作流 7 的错误响应 | 待新增：工作流 2 的拒绝用例，核对错误码与无对象副作用；不要求和可加载扩展的 PG 参考端结果相等 |

#### 证据规则

兼容结论必须绑定能力 ID、具体场景及证据范围。证据按用途分级，不能仅按文档数量或测试名称升级结论；下表定义后续取证要求，本轮没有采集当前客户端或差分运行记录。

| 证据类别 | 可证明的范围 | 结论边界 |
| --- | --- | --- |
| 官方 PostgreSQL 18 行为来源 | 为目标行为提供规范依据，记录官方手册的具体版本、章节与链接；存在歧义时补充对应版本的参考端复现 | 定义预期，不能证明 AsterSQL 已实现；大版本手册不能单独证明某个补丁版本的实测结果。 |
| 源码/独立测试定位 | 以仓库提交 SHA、源码路径及符号、独立测试路径及用例名定位实现与断言意图；入口包括 [连接模块说明](../pkg/server/pg_conn.rs.md) 和 [客户端测试文件](../pkg/server/pg_client_integration_test.rs) | 阅读源码或找到测试不等于测试已执行；模块说明只作导航，运行结论仍需真实源码、独立测试及当前记录相互对应。 |
| 历史测试记录 | 追溯过去在已知版本、环境和命令下报告的行为；[历史 PG 边界说明](postgresql-protocol-first-phase.md) 是调研入口 | 没有本轮复现时统一标注“历史记录，未复测”；缺失字段照实标明，不能据此填入矩阵的当前通过证据。 |
| 当前真实客户端记录 | 证明本轮指定服务端、客户端及配置下，实际执行的连接或 SQL 场景得到所记录结果 | 仅覆盖执行及断言的内容；连接成功不证明 SQL，SQL 执行成功也不证明与 PostgreSQL 语义相同。 |
| 当前差分记录 | 对同一能力契约，在 AsterSQL 与指定 PostgreSQL 参考端上执行相同场景，逐项比较预期与实际结果 | 仅覆盖所运行的输入、类型、元数据、NULL、SQLSTATE、会话/事务状态及副作用等适用断言；需列出未比较项与规范化规则，不能推广为全部 PostgreSQL 兼容。 |

每条证据记录至少包含下列可追溯字段；不适用项写明原因，缺失项不得用空白掩盖：

| 字段 | 记录要求 |
| --- | --- |
| 关联与来源位置 | 能力 ID、场景/用例 ID、记录日期、证据类别、官方来源版本/章节/链接、源码及独立测试路径/符号、日志或产物位置；本地来源绑定仓库提交 SHA 及影响结果的未提交改动。 |
| 版本 | AsterSQL 构建与提交、PostgreSQL 参考端精确版本、客户端/驱动及依赖版本；未使用参考端须明示，不得由 server_version 推断实际构建或参考端版本。 |
| 环境 | 操作系统、部署方式、连接与协议模式、数据库初始状态、编码、时区、会话参数、必要配置及数据准备/清理步骤；记录不得暴露凭证。 |
| 命令与退出码 | 可复现的完整命令、筛选条件、执行时间及每个必要步骤的实际退出码；注明跳过、忽略、失败和环境缺项，命令退出 0 不代替断言结果。 |
| 有效用例数量 | 实际执行并完成适用断言的用例数，另列发现、跳过、忽略与失败数量；重试不得重复计入有效覆盖，零测试不得记为通过。 |
| 预期与实际结果 | 预期行为及依据、双方实际值与适用类型/元数据、错误和状态等断言、比较结果；保留原始输出位置，解释规范化或排除项及其影响，不能只写“通过”。 |
| 结论与限制 | 可连接、可解析、SQL 子集或语义兼容中的具体结论，覆盖条件及未验证项；缺证、版本不明或有效用例数量为零时记录“未验证”及原因。 |

结论用词有以下不同的验收条件，不构成自动递进关系：

- **可连接**：当前客户端完成所声明的握手、认证及连接就绪断言；只覆盖指定客户端、协议与配置。
- **可解析**：具体输入通过指定解析入口及 AST/拒绝边界断言；不能由握手或命令返回成功推定，也不说明执行结果正确。
- **SQL 子集**：列明实际执行并验收的语句、参数、数据和边界用例，说明检查了哪些结果；不能将少量成功语句概括为整个语法或功能兼容。
- **语义兼容**：仅对明确的能力契约和覆盖条件作结论，要求当前差分记录及官方行为依据，适用的值、类型、元数据、NULL、错误、状态与副作用断言完整通过；未覆盖重载、边界或其他客户端必须单列为未验证。明确不支持项按能力矩阵的拒绝目标验收，不因参考端可执行而要求结果相同，也不称为该功能已支持。

反例（仅示意，非本轮实测）：某记录只证明一个指定客户端完成握手，收到 server_version 和 ReadyForQuery，未发送 SQL。错误表述是“PostgreSQL 18 SQL 及事务语义兼容”；正确表述是“在该记录的版本、环境和配置下可连接；SQL 解析、执行、事务及语义兼容未验证”。若该记录来自历史说明，还必须加注“历史记录，未复测”，不能写作当前可连接结论。server_version 是服务端报告值，模块说明文件是调研资料，两者均不能单独证明功能通过。

证据缺失、版本不明、零测试或缺少适用断言的条目，相关当前结论统一标“未验证”，注明缺口；历史记录继续保留其历史身份。新证据只能支撑明确覆盖的契约，经核对来源、运行记录和断言后再更新对应当前证据，不得自动升级支持状态，也不得覆盖既有记录的限制。本轮只建立这些规则，不变更已有支持结论。

#### 差分用例清单

清单使用 JSON 对象描述单个用例，以 `case_id` 关联能力矩阵中的 `capability_id`。`case_id` 在整个清单内唯一，`capability_id` 引用唯一的能力契约，允许多个用例覆盖同一能力；两者均不随文件排列、执行顺序或重试变化。双方言不同契约分别建用例，通过 `related_case_ids` 关联。以下沿用矩阵的说明性能力 ID，不登记新的支持能力。

本轮选择文档内 JSON 示例：仅用 Markdown 表格无法直接作为结构化运行输入；独立 JSON Schema 会引入额外文件、维护和实现约束，因此留待运行器任务决定。这里仅定义格式契约，没有新增清单文件、校验器、执行器或 CI。

| 字段 | 含义与约束 |
| --- | --- |
| `format_version`、`case_id`、`capability_id`、`related_case_ids` | 格式版本与稳定关联；关联列表可为空，表示没有关联用例。格式演进不能重用旧 ID 表示另一契约。 |
| `dialects.pg`、`dialects.mysql` | 两个方言分别声明适用状态和原因。适用项各自记录 `sql`、`setup_sql`（准备）、`cleanup_sql`（清理）、`parameters`、`session`、`expected`。SQL 可不同，但须说明映射和覆盖边界；不能直接把 PG SQL 发送给 MySQL 并忽略错误。 |
| `parameters` | 按绑定位置记录参数值、逻辑类型、方言类型及文本/二进制格式；PG OID、MySQL 类型标志分别记录。参数值使用下述带标签的值表示。无参数用空数组；需要但尚未知的类型填“待定义”，不能执行时猜测。 |
| `session`、`setup_sql`、`cleanup_sql` | 分方言明确编码、时区、会话参数、连接/协议模式、初始数据和事务条件；准备/清理数组按顺序执行，空数组表示无需 SQL 操作。清理失败、准备失败及会话不满足须进入运行记录，不能算有效通过。 |
| `expected` | 目标依据、成功或预期拒绝、结果值及顺序、类型与元数据、SQLSTATE、会话/事务状态与副作用断言。`output`、`sqlstate`、`state_and_effects` 分别承载结果、错误及状态/副作用断言；`output` 沿用观测结果的行、列及顺序结构。目标未细化时标“待定义”；真实参考端的观测结果独立存放，禁止用目标值填充观测值。 |
| `comparison` | 每方言 AsterSQL 与对应真实参考端分别比较；规定行顺序、重复行、列顺序和规范化。无 ORDER BY 时若契约不要求行序可按多重集合比较，仍保留重复行及原始输出；列序和类型不得顺便忽略。每个排除项需说明理由和限制，未知规则标“待定义”。 |
| `observations` | 分别保存 `aster_pg`、`reference_pg`、`aster_mysql`、`reference_mysql` 的实际版本、客户端信息、输出、SQLSTATE、状态及副作用和证据引用。每个必需观测缺失时显式标“未采集”，不允许以空数组冒充零行成功。 |
| `evidence_refs`、`verdict` | 引用上一节证据记录的稳定 ID 或可定位产物（附提交 SHA），并能追溯日期、环境、完整命令及退出码、有效/跳过/失败数量、原始输出和限制。`verdict` 仅在全部适用断言及证据齐全后可记通过；否则记“未验证”或实际失败，缺证不能通过。 |

输入及采集后的每个结果单元用 `{"kind":"value","value":"1"}` 表示非 NULL 值，用 `{"kind":"sql_null"}` 表示 SQL NULL；具体类型由参数或结果列元数据提供。字符串 `"NULL"`、空字符串、零行和 SQL NULL 必须区分。JSON 的裸 `null` 不用作缺证或 SQL NULL。采集前整个字段用 `{"status":"未采集"}`，目标字段未细化用 `{"status":"待定义"}`，不适用用 `{"status":"不适用","reason":"具体原因"}`；这些状态均不等于空的真实结果。

`observations` 中每个采集完成的对象至少包含 `status`、`version`（实际服务端精确版本及 AsterSQL 构建/提交）、`client`（客户端/驱动及依赖版本、协议模式）、`output`、`sqlstate`、`state_and_effects`、`evidence_refs`。`output` 内含 `rows`（行数组，单元按上述标签编码）、`columns`（按列序记录列名、类型、可空性及 PG OID/MySQL 类型标志等元数据）和 `row_order`；命令无结果集需明确说明原因。成功的 `sqlstate` 记“不适用（成功）”，错误时保留实际码和原始错误输出。实际状态、副作用和客户端信息同样不得由预期推断。版本字段中另记录计划基线及参考端实际版本，二者不同需说明对结论的限制；不能把握手报告版本当作构建版本。

下面是唯一的格式示例，SQL 和参数只是待执行输入。PG/MySQL SQL 分别使用自身的参数占位符；示例限定正整数输入，尚不覆盖 NULL 输入或其他重载。两个方言均需采集 AsterSQL 与真实参考端，所有真实对照值、类型、SQLSTATE 和版本尚未采集，目标断言也须在后续实施前细化，不构成有效用例或支持证据。PostgreSQL 18.6 仅沿用来源计划的目标，本轮未核实其发布状态。

```json
{
  "format_version": "1",
  "case_id": "example.abs-pos.bound-int",
  "capability_id": "示例 ABS-POS",
  "related_case_ids": [],
  "mapping": "分别绑定正整数 1；仅覆盖此输入，不推广到其他重载",
  "dialects": {
    "pg": {
      "status": "适用",
      "sql": "SELECT abs($1) AS v",
      "setup_sql": [],
      "cleanup_sql": [],
      "parameters": [
        {
          "position": 1,
          "logical_type": "有符号 32 位整数",
          "dialect_type": "int4",
          "protocol_type": {"status": "待定义"},
          "format": "text",
          "value": {"kind": "value", "value": "1"}
        }
      ],
      "session": {
        "encoding": "UTF-8",
        "timezone": "UTC",
        "protocol": "扩展查询",
        "initial_state": "独立空闲会话，无需表数据",
        "settings": {"status": "待定义"}
      },
      "expected": {
        "outcome": "成功",
        "basis": "能力矩阵示例 ABS-POS；官方行为来源待补齐",
        "output": {"status": "待定义"},
        "sqlstate": {"status": "待定义"},
        "state_and_effects": {"status": "待定义"}
      }
    },
    "mysql": {
      "status": "适用",
      "sql": "SELECT abs(?) AS v",
      "setup_sql": [],
      "cleanup_sql": [],
      "parameters": [
        {
          "position": 1,
          "logical_type": "有符号 32 位整数",
          "dialect_type": {"status": "待定义"},
          "protocol_type": {"status": "待定义"},
          "format": "binary",
          "value": {"kind": "value", "value": "1"}
        }
      ],
      "session": {
        "encoding": "UTF-8",
        "timezone": "UTC",
        "protocol": "预处理语句",
        "initial_state": "独立空闲会话，无需表数据",
        "settings": {"status": "待定义"}
      },
      "expected": {
        "outcome": "成功",
        "basis": "能力矩阵示例 ABS-POS；参考端行为依据待补齐",
        "output": {"status": "待定义"},
        "sqlstate": {"status": "待定义"},
        "state_and_effects": {"status": "待定义"}
      }
    }
  },
  "comparison": {
    "pairs": [["aster_pg", "reference_pg"], ["aster_mysql", "reference_mysql"]],
    "row_order": "按返回顺序逐行比较",
    "column_order": "按返回顺序逐列比较",
    "normalization": [],
    "excluded_assertions": []
  },
  "observations": {
    "aster_pg": {"status": "未采集"},
    "reference_pg": {"status": "未采集"},
    "aster_mysql": {"status": "未采集"},
    "reference_mysql": {"status": "未采集"}
  },
  "planned_reference_versions": {"pg": "18.6（来源计划目标，未核实）", "mysql": "待定义"},
  "evidence_refs": {"status": "未采集"},
  "verdict": {"status": "未验证", "reason": "目标及参数协议类型待细化，尚无任何真实对照记录"}
}
```

PG 专属或 MySQL 专属场景中，另一方言对象用 `status: 不适用` 并记录原因；这是覆盖边界，不能计为该方言通过。明确不支持项使用 `expected.outcome: 预期拒绝`，记录拒绝目标 SQLSTATE、状态恢复及无副作用要求；目标码未知时标“待定义”。若参考端允许操作，保留其真实成功结果，并将比较方式明确为 AsterSQL 的拒绝契约验收，不能强求成功与拒绝相等，也不能忽略任一错误。普通成功用例遇到“不支持”错误是失败或阻塞证据，不能临时转成预期拒绝使其通过。类型、元数据或其他适用断言未采集时仍为未验证，只有完整符合拒绝契约的当前证据才能支持“拒绝边界已验收”，不代表功能已支持。

#### 依赖与验收交接

本轮八个工作流的文档交接顺序为：契约→方言→类型→函数→SQL/会话/事务→目录→协议→客户端验收。下表将此依赖图对应到章节和交接内容；它是路线图编写顺序，实际实现须按真实源码、行为依赖、测试及资源约束重新核实批次，不能据此假定类型或 SQL 实现必须等待协议实现。

| 工作流 | 前序输入与交接产物 |
| --- | --- |
| 1. 兼容契约与差分验证 | 建立范围、能力矩阵、证据规则和 JSON 用例契约，为后续工作流提供统一验收语言。 |
| 2. 方言身份与解析边界 | 读取工作流 1，明确方言身份、语法及名称解析责任，给类型与后续工作流提供可定位的输入边界。 |
| 3. 共享类型与转换契约 | 读取工作流 2 的解析边界，明确类型、转换、NULL 与元数据目标，交给函数及 SQL 语义契约。 |
| 4. 方言函数与运算符契约 | 读取工作流 3，区分共享语义与同名异义契约，为 SQL 表达式验收提供边界。 |
| 5. SQL会话与事务语义 | 读取工作流 4 及方言、类型契约，描述会话/事务状态与副作用目标，交给目录可见性和协议状态验收。 |
| 6. 系统目录与内省契约 | 读取工作流 5 及名称、类型契约，明确目录对象、可见性和内省输出目标，交给协议及客户端用例。 |
| 7. 线协议认证与数据通道 | 读取工作流 6 及类型、错误、会话契约，明确协议承载和错误恢复目标，交给真实客户端验收。 |
| 8. 客户端与双方言共存验收 | 汇合前七项契约，定义客户端场景、方言隔离和当前运行证据要求；只对覆盖的能力作结论。 |

模块定位使用本章“模块”的 PG 边界、共享内核和独立客户端测试入口；模块路径存在或历史测试被记录，均不等于该能力已有当前证据。验收按“能力矩阵”的能力 ID 与目标，关联“差分用例清单”的 `case_id`、`capability_id` 和“证据规则”的版本、环境、命令、退出码、有效用例数量、预期与实际结果等字段；每项适用断言均须能回溯来源，缺项保持未验证。

交接给源蓝图任务 2“方言身份与解析边界”的只读输入是本路线图第 1 章及第 2–8 章标题骨架、四类矩阵示例、证据分级与唯一 JSON 格式示例，以及本章列出的调研入口。任务 2 的定义和范围见 `.plans/2026-10-22-postgresql18-核心兼容蓝图/2-方言身份与解析边界.md`；此处只定义交接，不执行该任务。

文档交接通过条件：范围排除完整，八章顺序及第一章五个责任标题齐全；矩阵的四类示例明确目标与未验证状态；JSON 的能力 ID 可对应矩阵，用例 ID 唯一，SQL NULL、未采集、待定义与不适用含义一致；证据规则不将历史记录或可连接升级为语义兼容；引用路径存在且计划产物标明待新增；任务 2 可据此继续描述方言责任，无需假定兼容能力或运行器已实现。目标中的待定义字段由对应后续能力任务细化，不能视作已验收。

后续细分只按单一能力的可观察契约、单一用例族或独立差分运行器拆分。各任务须声明输入、依赖、适用断言、证据缺口与通过条件；运行器单独立项，本轮不创建执行平台、环境或 CI。文档交付验收只表示契约可交接；所有尚无当前运行记录的能力仍为未验证。

### 范围外

核心非目标统一以“范围与非目标”小节为准。本章不实现生产行为、运行器、参考数据库环境或 CI，不撰写第 2–8 章正文；交接顺序不扩展为各子系统实现计划。

### 验收

文档验收以“依赖与验收交接”的通过条件为准，逐项检查范围、矩阵、证据字段、JSON 关联、章节结构和路径。行为验收则须按矩阵目标及证据规则另行采集当前记录，不能用本章完成、JSON 语法通过或零测试替代。

### 后续细分

按“依赖与验收交接”先交给源蓝图任务 2，再沿后续工作流细化；实施任务限定单一能力、单一用例族或独立运行器。各项在实现前补齐目标及验证入口，按实际依赖重新排批次；本章不展开这些实现任务。

## 2. 方言身份与解析边界

### 模块

[方言身份传递](#方言身份传递)负责连接到共享内核的身份边界。实际源码入口为 [pg_conn.rs](../pkg/server/pg_conn.rs) 的 `PgService::start` / `negotiate`、[pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::new` / `handle` / `process`、[pg_session.rs](../pkg/server/pg_session.rs) 的 `PgSession` / `SessionQuery::parse`，以及 [conn.rs](../pkg/server/conn.rs) 的 `ClientConn::dispatch`、`SessionDriver::open_ctx` 和 `TiDBContext`。另核对 [driver.rs](../pkg/server/driver.rs) 的 `IDriver::OpenCtx`：它是另一层生命周期抽象，不能冒充 PG 当前调用的 `SessionDriver`。本块不指定尚未取证的内核类型或接口名。

[类型转换语法边界](#类型转换语法边界)的调研入口为 [yy_parser.rs](../pkg/parser/yy_parser.rs)、[expressions.rs](../pkg/parser/ast/expressions.rs) 和上述扩展查询入口；实际共享表达式转换载体另定位到 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `ExprKind::Cast`。只核对本块必要的 AST 与参数接线，不展开类型执行子系统。

[大小写匹配语法边界](#大小写匹配语法边界)的源码入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `PatternLikeOrIlikeExpr`、[parser.y](../pkg/parser/parser.y) 的 `PredicateExpr` / `IlikeOrNotOp` / `LikeOrIlikeEscapeOpt` 与 [pg_name.rs](../pkg/server/pg_name.rs) 的 `rewrite` / `adapt`。语法文件与 Rust 载体分别取证，不以 Go 语义动作推断 Rust 运行接线。

[返回子句语法边界](#返回子句语法边界)的指定入口为 [ast/dml.rs](../pkg/parser/ast/dml.rs)、[parser.y](../pkg/parser/parser.y) 与 [pg_result.rs](../pkg/server/pg_result.rs)；实际共享 DML/投影载体在 [ast/lib.rs](../pkg/parser/ast/lib.rs)，以 `Returning`、`SelectField`、`command` / `encode_formats` 为必要局部交接点。

[冲突子句语法边界](#冲突子句语法边界)沿用 [ast/dml.rs](../pkg/parser/ast/dml.rs)、[parser.y](../pkg/parser/parser.y) 与 [pg_extended.rs](../pkg/server/pg_extended.rs)；实际 INSERT 载体为 [ast/lib.rs](../pkg/parser/ast/lib.rs) 的 `InsertStmt` / `Assignment`，只补充本块必要的局部取证。

[数组构造语法边界](#数组构造语法边界)的入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `Expr` / `RowExpr` / `ParamMarkerExpr`、[parser.y](../pkg/parser/parser.y) 的 `SimpleExpr` / `ArrayKwdOpt` 和 [pg_extended.rs](../pkg/server/pg_extended.rs) 的 `Extended::process` / `markers`，仅取证构造与类型/参数交接。

[行构造语法边界](#行构造语法边界)的入口为 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `RowExpr` / `Expr::restore`、[parser.y](../pkg/parser/parser.y) 的 `SimpleExpr`、[pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::close` / `scope` / `relation`；独立测试入口为 parser_test.rs 与 pg_name_test.rs，只作静态取证。

### 目标行为

#### 方言身份传递

本块承接第 1 章的共享内核、双方言独立契约与证据规则，对应源工作流 2 步骤 1；只定义身份传递，语法与名称规则留给本章后续文档块。连接建立时应由所属入口确定 PG 或 MySQL 身份，由该连接的会话持有，贯穿简单查询、扩展查询及后续执行上下文。身份在连接生命周期内稳定；会话设置、事务结束、预处理语句关闭和错误恢复不得把另一连接或另一入口切换方言。可变进程全局开关不能承担此职责。以下“应”均为目标契约，当前源码定位不等于全链路实现或运行通过。

责任链为 `listener → session → parser → resolver（名称绑定阶段） → planner → expression context`；箭头表示需要交接的身份责任，不表示已存在同名字段或静态调用边。

| 阶段 | 当前入口与实际边界 | 状态拥有者、生命周期与目标缺口 |
| --- | --- | --- |
| listener | PG 的 `PgService::start` 接受连接，`negotiate` 调用 `driver.open_ctx(id, 0, 45, "", None)`；MySQL 的 `ClientConn::dispatch` 经 `openSession` 取得上下文并分派 Query / Stmt 命令。 | PG service 拥有监听与工作线程，`ClientConn` 拥有 MySQL 连接状态；目标由连接所属入口确定身份，不能把 service 级共享对象变成可变当前方言。启动协议版本和握手版本字符串不是 SQL 方言身份或兼容证据。 |
| session | PG 每个连接循环创建一个 `Extended`，其 `session: PgSession` 被 Q 与扩展查询路径共用；`PgSession` 当前仅有 `path`、`failed_transaction`。`SessionDriver::open_ctx` 返回共享 `TiDBContext`。 | `Extended`、其语句及 portal 映射属于当前连接，连接循环结束后释放；`PgSession` 是真实局部状态入口，但没有全链路方言标记。目标将连接身份与可变 search_path/事务状态分开，关闭连接后不得被后续连接继承。 |
| parser | Q 先尝试 `SessionQuery::parse` 与 `CatalogQuery::parse_session`，普通路径经 `pg_name::adapt`、`pg_sql::adapt_with_context` 等适配后交给 `TiDBContext::execute_query`；扩展 Parse 在 `Extended::process` 做相应识别、适配与 `markers` 映射，再进入 `prepare_statement`。 | 当前受控识别及文本适配位于 PG 连接边界，不是完整 PG parser。目标每次解析显式获得本连接身份，生成的语句与预处理对象保留身份来源；解析器重用不能继承上一连接身份。共享 parser 内部的实际承载接口尚未在本块定位，后续需局部取证，不能从 SQL 文本或占位符反推身份。 |
| resolver | 当前可定位的名称入口是普通 SQL 的 `pg_name::adapt` 与目录查询的 `parse_session`，均收到当前 `PgSession`；共享 `TiDBContext` 接口没有显式方言参数。 | resolver 表示名称绑定阶段，不假定存在同名模块。目标名称绑定读取语句所属会话身份及适用的名称环境；该环境在解析/绑定/执行间的生效时点由后续规则定义。本块没有证明身份已传入共享名称绑定阶段，也不把边界名称适配当作该证明。 |
| planner | 普通 Q 的可定位共享入口为 `TiDBContext::execute_query`；普通扩展查询为 `prepare_statement`、`execute_prepared_statement`，参数是 SQL 或 statement ID/绑定值等，没有显式方言参数。 | 目标规划及计划重用保留产生该语句的方言语义；跨连接缓存不得混用两方言语义。当前只核实上述交接入口，未核实 planner 内部接线、缓存归属或键设计，不强定新增字段、类型及缓存接口。 |
| expression context | 普通执行仍由上述 `TiDBContext` 接口承接；`driver.rs::Expression` 是协议参数/结果值枚举，不是内核表达式求值上下文。 | 目标每条语句及其执行/重执行期间获得所属连接身份，函数分流和求值不得读取其他连接的可变状态。尚无共享表达式上下文接收 PG 身份的取证；具体落点交给后续局部实现调研，函数、转换的语义契约归工作流 3、4。 |

扩展查询目标要求 Parse、Bind、Execute、Describe、Close 与 Sync 始终属于同一连接身份；命名语句与 portal 不得跨连接解析、绑定或执行。简单查询也必须使用同一身份源。身份稳定不意味着冻结所有会话设置：现有 `pg_introspection_namespace_session_state` 的断言意图包括 Parse 后 SET，再执行 `current_schema()` 时观察当前会话值；search_path 状态执行和完整时点规则由工作流 5 承接，目录可见性由工作流 6 承接，本块不推广这条断言到全部名称绑定或计划失效行为。

当前证据固定为源码/独立测试阅读（基于仓库提交 `d55446b4a3`，本轮无行为执行）：[pg_session_test.rs](../pkg/server/pg_session_test.rs) 的 `pg_introspection_namespace_parser_boundaries` 断言有界会话语句的接受/拒绝；[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `pg_introspection_namespace_session_state` 断言两 PG 连接 search_path 交错隔离及简单/扩展查询局部状态承接，`postgres_create_table_simple_query` 提供普通 Q 接入的调研入口。这些测试没有证明 PG/MySQL 同时连接时的 parser→resolver→planner→expression context 身份隔离，本轮也没有运行它们。`pkg/server/doc.go` 不存在，已读包内模块说明及源码；不以历史通过记录补当前证据。

以下为待新增的隔离验收用例族，沿第 1 章分别登记能力 ID、case ID、结果元数据和观测记录；表内是目标输入及结果，不是执行结果。成功项 SQLSTATE 均记“不适用（成功）”；没有身份专用 SQLSTATE，不凭空创造错误码。

| 场景与正反例 | PG 适用性、目标结果 | MySQL 适用性、目标结果 | 错误与证据责任 |
| --- | --- | --- | --- |
| DIALECT-IDENTITY-COEXIST：同时保持 PG A、MySQL M 活跃，交错执行 `SELECT 1 AS v`；PG 分别经 Q 及零参数 Parse/Bind/Execute，M 经普通查询及自身预处理协议。反例是 PG 查询改变 M 的后续解析身份。 | 适用；每次返回一行 v=1，所属连接身份始终 PG。 | 适用；每次返回一行 v=1，所属连接身份始终 MySQL；精确类型与协议元数据分别按工作流 3、7 补齐。 | 无错误位置（成功）；相同值本身不足证明身份隔离，后续待新增连接/语句上下文断言，覆盖六段责任链且不引入产品可见调试接口。全部未验证。 |
| DIALECT-IDENTITY-PG-PAIR：A、B 两条 PG 连接；A 设置 `search_path TO pg_catalog, public`，B 显式设置 `search_path TO public`，交错 `SHOW search_path`，Q 和扩展协议交叉使用，再各自关闭并重连。反例是 A 的设置、Sync 或断开改变 B。 | 适用；各自 SHOW 反映本连接显式设置，稳定身份均 PG；重连重新初始化，不能继承 A 的状态。具体默认值另按工作流 5 契约核对，不以现有 public 默认值代表 PG 完整默认行为。 | 不适用这些 PG 设置语句；保持 M 的 `SELECT 1 AS v` 作为旁路隔离检查，不向 M 投递 PG 设置并忽略错误。 | 成功项无错误位置；状态取值归工作流 5，身份隔离归本块。已有测试仅有局部断言意图；完整场景未验证。 |
| DIALECT-IDENTITY-ERROR：A 通过扩展 Parse 发送 `SELECT )`，随后 Sync，再执行 `SELECT 1 AS v`；B、M 同时继续各自正例。反例是 A 的失败恢复令 B/M 切换身份或执行 A 的语句。 | 适用；A 语法错误目标 42601，恢复后可继续，B 保持自己的状态与身份。 | 不向 M 发送此 PG 协议负例；M 的正例应继续返回 1，MySQL 语法错误码及恢复不由本块猜定。 | parser 负责原始 SQL 的定位，PG 协议边界输出 SQLSTATE 和错误字段；位置目标指向 `)`，精确参考端位置须补采集。`write_error` 当前只写 S/V/C/M，没有 P 字段；不能宣称已支持错误位置。恢复契约协作工作流 7，全场景未验证。 |

外部目标依据为 PostgreSQL **18** 官方手册：§54.2.2/§54.2.3 [简单与扩展查询消息流](https://www.postgresql.org/docs/18/protocol-flow.html)，用于区分查询生命周期、预处理语句与 portal 的责任；§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)，规定 P 若提供须基于原始查询、从 1 起按字符计数，不能直接返回改写后字节偏移；[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 给出 42601 的语法错误分类。文档于 2026-10-23 查阅，未运行参考端；18.6 仍仅为来源指定目标，无该补丁版本实测结论。连接私有身份及禁止可变全局切换是 AsterSQL 双入口架构目标，不把它表述为 PostgreSQL 手册中的内部接口要求。

#### 类型转换语法边界

本块对应源工作流 2 步骤 2 的 `::` 分类，继承[方言身份传递](#方言身份传递)的连接私有身份契约。目标只确定表达式输入、优先级、嵌套、参数位置和诊断保真；类型解析后的转换可行性、舍入、溢出、NULL、结果类型与元数据由工作流 3 定义。本轮仅交付文档，以下用例均未运行，不能据此宣称已经支持。

当前源码证据基于提交 `a19b8cba62`：`Parser::ParseSQL` 在 `yy_parser.rs` 中让 `Scanner` / `yyParse` 决定语法接受与错误，`ParseOneStmt` 限定单语句，`yySetOffset` 向表达式记录原始偏移；现有 MySQL SQL mode 不能当作已存在的 PG 身份入口。`expressions.rs` 的 `ParenthesesExpr`、`BinaryOperationExpr`、`ParamMarkerExpr` 提供括号、运算树和参数顺序/偏移的调研入口；不能把该文件名当成 PG cast 节点定义。实际 `ast/lib.rs::ExprKind::Cast` 已有 `Expr`、`Tp: FieldType`、`FunctionType`、`ExplicitCharSet`，同一枚举有 `Parentheses` 和带 `Offset` 的 `ParamMarker`；这些字段说明可复用候选，不证明已能保留 PG 原始类型拼写、引用规则或 `::` 源位置。

`Extended::process` 的普通 Parse 路径先做有界适配，再调用 `markers` 和共享 `prepare_statement`；`markers` 仅将引号/注释之外的 `$n` 映射到 `?` 并返回零基参数索引序列，不解析转换语法。当前普通扩展路径在参数 OID 缺省/为零时返回 0A000，要求显式 OID；因此 `($1)::int4` 的语法目标不能自动升级为已支持从转换推断参数类型。目录/会话专用识别分支也不能证明通用 SELECT 转换支持。已阅读独立测试 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_parser_err_msg`（MySQL near/line 与注释诊断），以及 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 `marker_mapping_preserves_literals_comments_and_index_order`（字面量、注释、重复/乱序参数保护及拒绝边界）、`canonical_prepare_parameter_syntax_probe`（共享入口 `?` 与 `$1` 的断言意图）；本轮未执行测试，所读断言不构成 `::` 的通过证据。`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 均不存在。

承载方案留待后续最小语法取证：优先评估复用现有 Cast；若需要独立受控 PG 解析入口，其输出仍须可交给共享内核。两种方案都应保留以下信息，未定位的新接口不在此命名或假定存在：

- 操作数表达式树、显式括号及每层转换的顺序；`::` 左结合，不能把链式转换合并为最后一个目标类型，也不能把 `1 + 2::int4` 解析成 `(1 + 2)::int4`。
- 目标类型的原始拼写、限定名分段、引号与大小写信息，以及出现时的类型修饰参数、数组维度等语法信息；这是保真需求，不承诺本块实现完整类型语法、数组或自定义类型。支持集合及类型绑定交工作流 3，不能直接将任意 PG 类型名映射为 MySQL `FieldType` 后丢弃来源。
- 原始 SQL 中操作数、`::`、类型及参数的跨度、原始参数编号与出现顺序，并继承所属连接身份；改写时需有可回溯原 SQL 的位置映射。重复 `$1` 仍绑定同一个外部参数，不能因转换嵌套重排、复制绑定值或将值拼入 SQL。
- 词法层保护字符串、带引号标识符及注释中的 `::` / `$1`。不能用全局文本替换把 `::` 通用翻译为 CAST，也不能删除字符串/注释中的片段；现有 `markers` 拒绝 dollar quote 且不承诺 PG 嵌套块注释，后续要单独取证并明确拒绝边界，不能静默按 MySQL 词法解释。

下表是待新增验收用例族的输入契约，后续按第 1 章登记能力 ID 与 case ID。PG 成功项 SQLSTATE 为“不适用（成功）”，无错误位置；结果值只是目标，精确类型/OID及列元数据交工作流 3、7 补齐。所有实际输出与参考端错误位置均待采集。

| 输入与正反例 | PG 目标 | MySQL 对照与适用性 | 错误位置、责任与证据状态 |
| --- | --- | --- | --- |
| 正例 `SELECT 1::int4` | 解析为整数常量的显式转换，目标值 1；类型名 int4 是 PG 既有名称。 | 原 SQL 作为方言隔离负例，应拒绝 `::`；自身正例 `SELECT CAST(1 AS SIGNED)` 目标值 1，不要求与 PG 类型元数据相同。 | PG 成功；MySQL 负例定位冒号附近，精确诊断/码待参考端及本地取证；双方未验证。 |
| 正例 `SELECT ($1)::int4`，扩展 Parse/Bind/Execute，绑定整数 1 | 保留括号、参数编号 1、转换目标，目标值 1。分别覆盖显式 OID 与省略 OID 后由类型上下文推断；后者为目标缺口。 | 原 SQL 不作 MySQL 正例；自身预处理正例 `SELECT CAST((?) AS SIGNED)` 绑定 1，目标值 1。 | 参数不在普通 Q 文本中直接绑定；PG Parse 语法与类型推断分责，Bind 值解码另验收。当前 OID 限制如上，两协议均未运行。 |
| 优先级正例 `SELECT 1 + 2::int4` 与 `SELECT (1 + 2)::int4` | 分别为右操作数转换与括号和式转换，值均目标为 3；必须检查不同树形，不能仅比较相同结果。 | `::` 均为负例；自身正例分别为 `SELECT 1 + CAST(2 AS SIGNED)`、`SELECT CAST((1 + 2) AS SIGNED)`。 | 语法树责任归本块，求值归工作流 3、4；无当前 AST 或运行证据。 |
| 嵌套正例 `SELECT 1::int4::int8` 与 `SELECT ((1)::int4)::int8` | 前者按左结合，均保留两层转换，目标值 1；不能直接消去内层转换。 | `::` 均为负例；自身正例 `SELECT CAST(CAST(1 AS SIGNED) AS SIGNED)` 仅对照嵌套形态，不宣称两层类型等价。 | 类型链执行及元数据归工作流 3；树形与位置待新增断言，未验证。 |
| 参数位置正例 `SELECT ($2)::int4, ($1)::int4, ($2)::int4`，绑定 1、2 | 目标行 `(2,1,2)`，映射顺序为 `[1,0,1]`，各转换保留来源位置。 | 自身正例 `SELECT CAST(? AS SIGNED), CAST(? AS SIGNED), CAST(? AS SIGNED)` 按出现顺序绑定 2、1、2；不把 MySQL 的三个位置冒充 PG 两个参数。 | 现有 marker 测试仅覆盖一般映射意图，带转换的完整 Parse/Bind 仍未验证。 |
| 缺目标类型负例 `SELECT 1::`、`SELECT ($1)::`（后者走扩展 Parse） | 拒绝不完整转换，语法错误目标 42601；不能忽略 `::` 或退化为普通表达式。 | 同样不是 MySQL 合法转换；自身缺类型负例 `SELECT CAST(1 AS)` 应拒绝。 | PG 定位缺失类型的输入末尾，精确字符位置待参考端采集；MySQL 可更早在冒号处拒绝，不能强求错误位置相等。parser 保留原 SQL 定位，PG 协议边界输出错误字段。 |
| 词法保护正例 `SELECT '1::int4 $1' /* :: $2 */`，以及扩展输入 `SELECT ($1) /* :: $2 */ ::int4` 绑定 1 | 前者字符串值原样保留且无参数；后者仅绑定一个参数，目标值 1，注释不产生转换/参数节点。 | 前者可作 MySQL 自身正例；后者 `::` 是负例，对照用 `SELECT CAST((?) /* :: $2 */ AS SIGNED)`。 | PG/MySQL 仅按各自词法解释；成功无位置。dollar quote、嵌套注释另建边界负例/支持用例前须取证，本轮未验证。 |

官方依据（2026-10-23 查阅）为 PostgreSQL **18** §4.2.9 [Type Casts](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-TYPE-CASTS)（CAST 与 `::` 两种形式）、§4.2.2 [位置参数](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-EXPRESSIONS-PARAMETERS-POSITIONAL)、§4.1.6 [运算优先级](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-PRECEDENCE)（`::` 左结合且高于加法）、§8.1 [数值类型](https://www.postgresql.org/docs/18/datatype-numeric.html)（int4/int8）、§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)（P 基于原查询、从 1 起按字符定位）与[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)（42601）。MySQL 对照采用 **8.4** §14.10 [Cast Functions and Operators](https://dev.mysql.com/doc/refman/8.4/en/cast-functions.html)，仅作自身 CAST 形式的文档依据，不替代实际参考版本选择或运行记录。语法错误、未知类型和转换失败须按阶段区分，不能全映射为 42601；后两者具体目标码由工作流 3 按能力取证。本轮未运行真实数据库，18.6 仍为来源指定目标，无该补丁版本实测结论。

#### 大小写匹配语法边界

本块对应源工作流 2 步骤 2 的 ILIKE 分类，继承[方言身份传递](#方言身份传递)的连接身份契约；只定义 `ILIKE`、`NOT ILIKE`、`ESCAPE` 的语法承载、优先级和诊断责任。匹配算法、locale/collation、NULL、类型转换及运算符重载交工作流 3、4；名称大小写折叠由后续名称规则块定义。以下目标与当前静态证据分开登记，所有运行结果均未验证。

当前源码证据基于提交 `5780b7041ac54cf696b55a9a43fd170e47a4cfc0`：Rust `PatternLikeOrIlikeExpr` 已有 `expr`、`pattern`、`not`、`is_like`、`escape: u8`、`escape_explicit` 及模式缓存字段；这是可复用载体，不证明 PG 输入已完整接线。未纳入图索引的 `parser.y` 有 `ilike` token、`IlikeOrNotOp` 和 `PredicateExpr` 分支，以 `IsLike: false`、`Not` 区分操作类别和否定；`LikeOrIlikeEscapeOpt` 只接收字符串字面量，缺省为反斜杠且 explicit=false，显式空串产生零 escape，超过一个字节时报 ESCAPE 参数错误。该文件含 Go 语义动作，不能冒充 Rust parser 的当前通过证据；字节转义载体也不能直接宣称覆盖 PG 的字符/表达式转义边界。优先级声明将 like/ilike 与比较操作符放在同层，而 PG 18 表 4.2 将模式匹配置于比较之上，因此不能照搬共享声明即宣称 PG 优先级已对齐。

`pg_name::source_tokens` / `rewrite` 保留非编辑区的原 SQL，主要改写关系名和引号；`adapt` 无原生关系时可直接返回改写结果，不是 ILIKE 的完整解析器或求值器。已阅读独立测试 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_high_not_precedence_mode`（用 BETWEEN 检查 MySQL SQL mode 下 NOT 树形）和 `test_parser_err_msg`（near/line 与注释诊断），以及 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`（字面量/注释保护及名称改写）、`pg_introspection_names_rejection_boundaries`（引号、嵌套注释等拒绝边界）。这些用例没有给本块 ILIKE 提供专项通过证据，本轮未执行测试；`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 均不存在。

承载选择留待最小局部取证：优先比较复用共享 AST 与增加受控 PG 解析入口；前者须核对 Rust parser 实际构造、访问/恢复及共享消费者是否保真，后者须将相同信息交给共享内核，并保持原 SQL 位置。两方案都必须保留操作符类别、左操作数、右模式表达式、否定标志、ESCAPE 是否显式出现及其原始表达式/文本与跨度；省略 ESCAPE 与 `ESCAPE ''` 不得合并。现有 u8 字段不足时只规划必要的表示扩展，不在此规定新接口或补建匹配系统。不能把 ILIKE 无条件替换为 LIKE，也不能全局改写字符串/注释中的关键字，不能用 lower() 包装替代 PG 匹配契约。

PG 目标按模式匹配高于比较、IS、NOT、AND、OR 的顺序保留树形；`NOT ILIKE` 的 NOT 属于该谓词，前置逻辑 NOT 仍需保留其外层位置，括号可显式改变组合。MySQL SQL mode 的 NOT 优先级不能泄漏到 PG 连接。ESCAPE 只关联所属匹配谓词，表达式转义的完整允许集合由后续对照取证；当前字面量规则不能被当成完整 PG 规则。`ILIKE ANY`、符号别名 `~~*` / `!~~*` 及其他匹配家族不纳入本块承诺，后续单独登记，不能借本小节推导已支持。

下表为待新增用例族，按第 1 章分别登记 PG/MySQL 能力 ID 与 case ID。PG 成功项 SQLSTATE 均为“不适用（成功）”，无错误位置；PG 布尔类型与协议元数据交工作流 3、7。MySQL 原 ILIKE 输入作为隔离负例，目标拒绝；共享语法已有扩展候选，实施前须核对现有 MySQL 入口基线并显式解决差异，不在此改动它。MySQL 自身 LIKE 对照不视为 ILIKE 的等价翻译，大小写结果依赖所选 collation，未指定时不猜值或错误码。所有场景均未实测。

| 输入与正反例 | PG 目标 | MySQL 适用性与目标 | 错误位置、责任与证据状态 |
| --- | --- | --- | --- |
| 正例 `SELECT 'Ab' ILIKE 'a%'` | 在支持 ASCII 大小写折叠的确定性 locale/collation 前提下，接受匹配谓词，目标 true；前提需在后续参考环境明确记录。 | 原 SQL 为隔离负例；自身正例 `SELECT 'Ab' LIKE 'a%'` 接受，值待选定 collation 后按工作流 4 取证。 | PG 成功；MySQL 拒绝目标在 ILIKE 附近，精确 SQLSTATE/位置待参考端及本地核实。双方未验证。 |
| 正例 `SELECT 'Ab' NOT ILIKE 'a%'` 与 `SELECT NOT ('Ab' ILIKE 'a%')` | 同上述前提，均目标 false；前者保留谓词否定，后者保留外层 NOT 与括号，不能只凭相同结果证明树形。 | 原 SQL 均为负例；自身 `SELECT 'Ab' NOT LIKE 'a%'` 与 `SELECT NOT ('Ab' LIKE 'a%')` 接受，结果按自身 collation 核实。 | PG 成功；MySQL 原输入的精确拒绝码/位置未验证。 |
| 优先级正例 `SELECT 'Ab' ILIKE 'a%' = TRUE`、`SELECT NOT 'Ab' ILIKE 'a%' AND TRUE` | 分别为匹配结果再比较、外层 NOT 作用于匹配后再 AND；上述前提下目标 true、false。需树形断言，不能把比较纳入模式操作数或将 NOT 作用于左字符串。 | 原 SQL 为负例；MySQL 自身组合以 LIKE 取代关键字单独登记，保留其 SQL mode 语义，不强求 PG 树形。 | 语法组合归本块，求值归工作流 4；当前没有专项 AST 或运行证据。 |
| ESCAPE 正例 `SELECT 'A_b' ILIKE 'a!_b' ESCAPE '!'`，及 `SELECT 'Ab' NOT ILIKE 'a%' ESCAPE '!'` | 保留显式转义及否定；上述前提下目标 true、false。 | 原 SQL 为负例；自身 LIKE / NOT LIKE 配合 `ESCAPE '!'` 接受，值按自身 collation 验收。 | PG 成功；两方言分别核对转义原文与结果，未验证。 |
| 区分省略与空串：`SELECT 'a_b' ILIKE 'a!_b'` 和 `SELECT 'a_b' ILIKE 'a!_b' ESCAPE ''` | 两者都接受但 ESCAPE 出现状态不同；显式空串表示禁用转义。本行仅验收语法信息，不扩写匹配算法。 | 原 SQL 为负例；自身 LIKE 的省略/空串受 MySQL SQL mode 约束，需记录 NO_BACKSLASH_ESCAPES 后另取目标，不直接套用 PG 默认值。 | PG 成功，MySQL 自身形式的具体值/拒绝码待取证；均未验证。 |
| 缺右操作数负例 `SELECT 'Ab' ILIKE`、`SELECT 'Ab' NOT ILIKE`；缺转义负例 `SELECT 'Ab' ILIKE 'a%' ESCAPE` | 拒绝不完整谓词，目标语法错误 42601；不得吞掉 ILIKE、NOT 或 ESCAPE 后执行剩余表达式。 | 原 SQL 应拒绝，可早在 ILIKE 附近失败；自身负例 `SELECT 'Ab' LIKE`、`SELECT 'Ab' LIKE 'a%' ESCAPE` 同样拒绝。 | PG 定位缺失表达式的输入末尾，精确字符位置待参考端采集；MySQL 不要求同位置/同码。parser 保留原 SQL 跨度，协议工作流 7 输出 SQLSTATE/P，当前未验证。 |
| 词法保护正例 `SELECT 'ILIKE NOT ILIKE ESCAPE' /* ILIKE */` | 返回原字符串，字符串/注释不产生匹配节点。 | 自身正例，返回原字符串；无 ILIKE 运算符。 | 双方言成功、SQLSTATE 不适用；现有名称测试只覆盖类似保护意图，本输入仍未验证。 |

官方依据（2026-10-23 查阅）为 PostgreSQL **18** §9.7.1 [LIKE 与 ILIKE](https://www.postgresql.org/docs/18/functions-matching.html#FUNCTIONS-LIKE)（ILIKE、否定及 ESCAPE 规则）、§4.1.6 [操作符优先级](https://www.postgresql.org/docs/18/sql-syntax-lexical.html#SQL-PRECEDENCE)、§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html)（P 从 1 起按原始 SQL 字符定位）与[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)（42601）。MySQL 自身形式参考 **8.4** §14.8.1 [字符串比较操作符](https://dev.mysql.com/doc/refman/8.4/en/string-comparison-functions.html)，不以其替代本地扩展基线取证。多字符或非字面量 ESCAPE 的合法性、类型/求值错误及其精确 SQLSTATE 尚未取证，由工作流 3、4 补齐；不能统一按 42601 处理。本轮没有数据库或该输入的行为执行证据，18.6 仍仅为来源指定目标，无补丁版本实测结论。

#### 返回子句语法边界

本块对应源工作流 2 步骤 2 的 RETURNING 分类，继承[方言身份传递](#方言身份传递)的连接身份；只定义 INSERT/UPDATE/DELETE 的子句位置、投影保真、语法拒绝及 planner/结果描述交接。真实写入、返回行求值与事务副作用归工作流 5，类型归工作流 3，wire 编码归工作流 7。以下是待实现、待验收契约，不能以删除 RETURNING 后执行原语句、追加 SELECT 或只返回受影响行数冒充成功。

当前静态证据基于提交 `2ca07d8705eeaba2fda8b2f7823468147b7c2e1d`：指定入口 `ast/dml.rs` 是 DML 基础类型，实际共享 `InsertStmt`、`UpdateStmt`、`DeleteStmt` 在 [ast/lib.rs](../pkg/parser/ast/lib.rs)，三者均有 `Returning: Vec<SelectField>`。`SelectField` 提供 `Expr`、`WildCard`、`AsName`、`Offset`、`OriginalText`，可作为复用候选，但不能由这些字段推导 PG 原文、跨度或所有消费者已保真。未索引的 `parser.y` 中 `ReturningClause` 为省略或 `RETURNING FieldList`，`InsertIntoStmt` 将其放在 `OnDuplicateKeyUpdate` 后，`UpdateStmtNoWith` 与单表 `DeleteWithoutUsingStmt` 在尾部承接；这些是带 Go 动作的共享语法，不能直接证明 PG 的 ON CONFLICT、UPDATE FROM、DELETE USING 或 Rust 接线已支持。

`pg_result::command` 在共享 ParseSQL 后，对上述三类 AST 的非空 Returning 明确返回 `0A000`；语法解析错误在该入口映射为 `42601`。这表示现有 PG 命令门禁有拒绝分支，不表示本轮已运行验证。`encode_formats` 以 `QueryResult.columns` / `native_types` 描述列并编码行，DML completion 使用 affected_rows；目前源表 OID/属性号填零、typmod 填 -1，不能据此声称完整 RETURNING 元数据已实现。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_parser_err_msg`（near/line 诊断）、[pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `simple_query_roundtrip`（普通 DML completion 与 SELECT 元数据）和 `simple_query_encoding_rejects_unrepresentable_results`（编码及命令拒绝意图）；它们不是本块三类 RETURNING 的专项通过证据。局部补读 [parser_actions/dml_test.rs](../pkg/parser/parser_actions/dml_test.rs) 的 `dml_delete_single_table_keeps_go_modifiers_and_returning`、`dml_insert_and_replace_keep_go_hints_and_returning`，仅有 Returning 长度等断言，未覆盖本契约的完整投影、UPDATE、PG 隔离及副作用。本轮未运行任何测试；`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 均不存在。

PG 目标位置为 INSERT 的数据源及可选 ON CONFLICT 之后，UPDATE 的 SET、可选 FROM/WHERE 之后，DELETE 的可选 USING/WHERE 之后；不能照搬共享 MySQL ORDER BY/LIMIT 等扩展顺序。ON CONFLICT 本身由下一块定义，本块只保留衔接位置。投影按顺序保留表达式树、`*`/限定星号、显式或省略 AS 的输出别名及其原始拼写/引号、原 SQL 字符跨度；区分没有子句和出现 RETURNING 但投影为空。字符串和注释中的关键字不产生子句，不以文本截断或全局替换接入。

承载方案留待后续最小取证，比较共享 AST 与受控 PG 入口，不能凭候选字段定新接口。解析负责子句及字段保真；resolver 负责列、限定名、星号绑定，planner 负责保留投影及列顺序，绑定/类型阶段产生输出列名、类型、可空性及可用的来源信息，交工作流 3、7 描述。Describe 不得为获取元数据而执行写入；零行命中也有已确定的列结构。执行阶段由工作流 5 验证 INSERT/UPDATE 默认返回新行、DELETE 默认返回删除前行，并分别验证提交/回滚与受影响行数。PG 18 还含 OLD/NEW 引用及 `RETURNING WITH (OLD AS o, NEW AS n)`；本块不承诺其实现，后续须单独取证及明确支持集合，未支持合法形式以 0A000 在副作用之前拒绝，不能误称 PG 语法非法。MERGE、触发器和规则的实现不在本块范围。

下表为待新增验收用例族；各行独立初始化普通表 `t(id integer)`，无触发器、规则或默认值。UPDATE/DELETE 初始仅一行 id=1，INSERT 初始为空；具体 schema/连接前提由后续用例记录。PG 正例成功时 SQLSTATE 不适用、无错误位置；精确 OID/类型修饰等交工作流 3、7。MySQL 原 RETURNING 输入均作为隔离负例，拒绝目标在 RETURNING 附近，精确码/位置待参考端采集；自身去掉子句的 DML 是合法对照，仅验证自身副作用，不能作为 PG 返回行验收。共享语法已有 RETURNING 扩展候选，实施前须取证现有 MySQL 入口基线并解决差异，不能从 MySQL 官方语法推导本地已拒绝。所有行为均未实测。

| 输入与正反例 | PG 解析与列描述目标 | 执行交接及 MySQL 适用性 | 错误位置、责任与证据状态 |
| --- | --- | --- | --- |
| INSERT 正例 `INSERT INTO t(id) VALUES (1) RETURNING id` | 保留一个列投影，输出列名 id，类型取自绑定列。 | 工作流 5 目标返回 `(1)` 且仅插入一行；MySQL 原输入为负例，自身 INSERT 无返回投影。 | PG 成功目标；当前存在 0A000 门禁，未验证。 |
| UPDATE 正例 `UPDATE t SET id = id + 1 WHERE id = 1 RETURNING id + 10 AS next_id` | 保留算术表达式与别名 next_id，不能误取 SET 表达式或丢弃 WHERE。 | 工作流 5 目标返回 `(12)`，存储为 id=2；MySQL 原输入为负例，自身 UPDATE 后独立查询验证修改。 | PG 成功目标；列名、绑定类型及真实写入分别验收，未验证。 |
| DELETE 正例 `DELETE FROM t WHERE id = 1 RETURNING *` | 保留星号，由绑定阶段按表列顺序展开为 id；不得由返回行推断列数。 | 工作流 5 目标返回删除前 `(1)` 且表为空；MySQL 原输入为负例，自身 DELETE 验证删除。 | PG 成功目标；当前无全链路通过证据。 |
| 多投影/别名正例 `INSERT INTO t(id) VALUES (1) RETURNING id, id + 1 AS next_id, id label` | 三项有序，保留表达式、显式 AS 与省略 AS；输出列名 id、next_id、label。 | 工作流 5 目标行 `(1,2,1)` 且仅写入一次；MySQL 原输入为负例。 | PG 成功目标；树形、列顺序及副作用须独立断言，未验证。 |
| 零行正例 `UPDATE t SET id = 2 WHERE id = 9 RETURNING id` | 仍有 id 列结构，不能退化成无列命令。 | 工作流 5 目标返回零行、原表不变；MySQL 原输入为负例，自身 UPDATE 为零行命中对照。 | PG 成功目标；Describe/Execute 一致性待取证。 |
| 空投影负例 `INSERT INTO t(id) VALUES (1) RETURNING`、`UPDATE t SET id = 2 RETURNING`、`DELETE FROM t RETURNING` | 拒绝缺失投影，PG 目标 42601；不得当作省略子句。 | 解析失败不得执行写入；MySQL 原输入同为负例，可更早在 RETURNING 拒绝。 | PG 指向缺失投影的输入末尾，精确 P 待参考端采集；parser 保留原 SQL 定位，工作流 7 输出错误字段。 |
| 错位负例 `UPDATE t SET id = 2 RETURNING id WHERE id = 1` | 拒绝 RETURNING 后错置的 WHERE，PG 目标 42601。 | 不得删去尾部或子句后执行 UPDATE；MySQL 原输入为负例。 | PG 定位 WHERE 附近，精确字符位置待取证；未验证。 |
| 词法保护正例 `INSERT INTO t(id) VALUES (1) /* RETURNING */` 与 `SELECT 'RETURNING'` | 前者无返回投影，后者普通字符串投影；不制造 Returning 节点。 | 两方言自身正例；前者只插入，后者返回原字符串。 | 成功时 SQLSTATE 不适用；专项保护与无误改写证据待新增。 |

官方依据（2026-10-23 查阅）为 PostgreSQL **18** §6.4 [Returning Data from Modified Rows](https://www.postgresql.org/docs/18/dml-returning.html) 及 SQL 命令 [INSERT](https://www.postgresql.org/docs/18/sql-insert.html)、[UPDATE](https://www.postgresql.org/docs/18/sql-update.html)、[DELETE](https://www.postgresql.org/docs/18/sql-delete.html) 的 Synopsis/输出参数；错误依据为 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 的 42601/0A000 与 §54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 的原查询字符位置 P（从 1 起）。MySQL 自身语法依据为 **8.4** §15.2.7 [INSERT](https://dev.mysql.com/doc/refman/8.4/en/insert.html)、§15.2.17 [UPDATE](https://dev.mysql.com/doc/refman/8.4/en/update.html)、§15.2.2 [DELETE](https://dev.mysql.com/doc/refman/8.4/en/delete.html)，其语法未列 RETURNING，不替代本地扩展及实际参考端取证。合法但未支持、语法非法、列绑定/类型失败必须分阶段，列绑定/类型失败的具体错误码待相关工作流采集，不能全部映射为 42601。18.6 仍仅为来源指定目标，本轮无该补丁版本或真实数据库实测结论。

#### 冲突子句语法边界

本块对应源工作流 2 步骤 2 的 ON CONFLICT 分类，只定义语法信息与名称绑定交接。PG 的冲突目标、动作及谓词不能直接等同于 MySQL `ON DUPLICATE KEY UPDATE`，也不能改写为 `INSERT IGNORE`、`REPLACE` 或空赋值列表。唯一性推断、并发、锁、更新执行及事务副作用归工作流 5；本块没有开放 upsert 能力。

当前证据为基于提交 `391d3895b8` 的源码和独立测试阅读，本轮没有运行行为验证。`ast/dml.rs` 的 `TableName`、`Join::restore` 是共享表/连接基础类型；实际 `InsertStmt` 位于 `ast/lib.rs`，其 `OnDuplicate: Vec<Assignment>` 仅提供赋值列表，`Assignment` 包含 `Column` / `Expr`，不能区分“无冲突子句”与 DO NOTHING，更没有 PG 冲突目标、目标谓词和更新条件的独立字段。`parser.y::InsertIntoStmt` / `OnDuplicateKeyUpdate` 将 MySQL 子句写入 OnDuplicate；这是 Go 语义动作的语法证据，不是 Rust PG 解析接线或成功证据。该语法文件未入图，直接读取核对。`pg_extended.rs::Extended::process` 的 Parse 分支经会话/目录识别、名称和 SQL 适配、`markers` 后调用 `prepare_statement`，未在此提供 PG 冲突 AST 或原 SQL 跨度映射；`markers` 的参数映射不能替代诊断位置映射。

已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_insert_statement_memory_allocation`，断言普通 INSERT 的 1001 行及分配上限；已读 [pg_query_test.rs](../pkg/server/pg_query_test.rs) 的 `simple_query_roundtrip` 普通 INSERT/UPDATE/DELETE 命令完成及结果断言、`postgres_create_table_simple_query` 的建表入口。这些断言不覆盖 ON CONFLICT，不能作为本块通过证据。`pkg/parser/doc.go`、`pkg/server/doc.go` 均不存在；当前未证明 PG 冲突子句的接受、拒绝、错误码或位置，不从缺字段推断实际报错。

目标承载方案仍待本章解析方案任务比较共享 AST 扩展与受控 PG 入口；不预设新增类型或方法名。无论选择哪条路径，交接至少保留：

| 语法信息 | 目标保真及交接责任 |
| --- | --- |
| 子句及动作 | 区分未出现、DO NOTHING、DO UPDATE；保留 ON CONFLICT、DO、动作和 SET 的原 SQL 跨度。不能用空 OnDuplicate 隐去 DO NOTHING。 |
| 冲突目标 | 区分省略、索引推断列表、ON CONSTRAINT 名称；列表保留列与带括号表达式、顺序、显式 COLLATE/opclass 及各自跨度。DO NOTHING 可省略目标，DO UPDATE 必须指定目标；目标可解析不表示约束存在或可用。 |
| 两类谓词 | 推断列表后的 WHERE 属于目标谓词，SET 后的 WHERE 属于更新条件，分别保留表达式树、出现性、括号及跨度；不能合并为 INSERT/SELECT 的过滤条件。 |
| 更新与名称 | 保留有序赋值、单列/多列目标、DEFAULT/表达式/子查询形态；保留 `excluded.v` 与目标表或别名引用的限定名和引号，交名称绑定区分拟插入行与现有行，不能文本替换成 MySQL VALUES 或普通表。目标表名为 excluded 时显式别名的遮蔽关系须交后续名称任务核对。 |
| 原查询与协议 | Q 与扩展 Parse 使用同一 PG 契约；保留 `$n` 的身份、重复/乱序映射及原查询字符跨度。合法但暂不支持的形态须明确拒绝，不能丢字段后执行；错误传输和恢复交工作流 7。 |

下列均为待新增的 `CONFLICT-SYNTAX` 用例族，按第 1 章登记能力 ID/case ID；不是已实现测试。各例假设 `t(id, v, active)` 可绑定；需要执行冲突时再由工作流 5 创建合适唯一约束/索引和初始行。所有成功项 SQLSTATE 为“不适用（成功）”、无错误位置，均尚未运行。

| 正反例输入 | PG 目标结果与 AST 验收 | MySQL 适用性、错误及证据责任 |
| --- | --- | --- |
| 正例：`INSERT INTO t(id,v) VALUES (1,2) ON CONFLICT DO NOTHING`；再比较显式 `(id)` 与 `ON CONSTRAINT t_pkey` 目标 | 接受相应语法；分别保留省略、索引列、约束名及 DO NOTHING，不能等同于无子句。执行时忽略何种冲突由工作流 5 验收。 | PG 写法在 MySQL 契约下应拒绝；具体 MySQL SQLSTATE/位置待实际参考端与本地基线取证，不套用 PG 码。 |
| 正例：`INSERT INTO t AS dst(id,v,active) VALUES (1,2,true) ON CONFLICT (id) WHERE active DO UPDATE SET v=excluded.v WHERE dst.v<>excluded.v` | 接受语法；目标 WHERE 和更新 WHERE 分属两棵树，SET 中 excluded 与 dst 引用保持可绑定身份；冲突条件成立/不成立的真实写入与不更新结果另交工作流 5。 | 此 PG 形式应拒绝；MySQL 诊断仍待取证。不能仅删除目标或 WHERE 后转成 MySQL 更新。 |
| 正例形态：`ON CONFLICT ((lower(v)) COLLATE "C" text_ops) WHERE active DO NOTHING`（接在相应 INSERT 后） | 保留表达式括号、collation/opclass 名称及目标谓词；只验收解析保真，名称是否存在、类型及索引可用性另行取证，不承诺执行成功。 | PG 目标形式不适用 MySQL；目标名字有效性和相关错误码由名称/类型及工作流 5 补齐。 |
| 反例：`INSERT INTO t(id,v) VALUES (1,2) ON CONFLICT (id)`；以及尾部 `DO`、`DO UPDATE SET` | 缺少动作或赋值，PG 语法拒绝目标 42601；定位缺失项后的结束位置（或实际非法 token），解析器记录本子句跨度，精确 P 值待参考端采集。 | 同样不是合法 MySQL 冲突更新形式；MySQL 码与位置独立取证。禁止静默退化为普通 INSERT。 |
| 反例：`INSERT INTO t(id,v) VALUES (1,2) ON CONFLICT DO UPDATE SET v=excluded.v` | 缺少 DO UPDATE 所需冲突目标，应拒绝；具体参考端诊断阶段、SQLSTATE 与 P 是否提供须后续采集，不能只凭 Synopsis 的可选目标猜码。 | 不适用 MySQL；拒绝和定位单独验收，不作为 MySQL upsert 正例。 |
| MySQL 正例/PG 反例：`INSERT INTO t(id,v) VALUES (1,2) ON DUPLICATE KEY UPDATE v=3` | PG 方言目标语法拒绝 42601，错误跨度指向不属于 PG 子句的关键字，精确 P 待采集。 | MySQL 原生语法接受目标，OnDuplicate 保留赋值；真实冲突更新与 affected rows 交工作流 5，不能要求与 PG DO UPDATE 的结果天然一致。 |
| 词法及参数保护：字符串/注释中出现 `ON CONFLICT`、`excluded.v`；扩展 Parse 中 VALUES/SET/两类谓词含重复或乱序 `$1`、`$2` | 字符串/注释不产生冲突节点，实际参数绑定位置不因子句改动错位；按合法上下文构造完整 SQL，断言树、参数身份与原 SQL 跨度。 | MySQL 仅使用自身合法 INSERT/冲突更新及 `?` 参数，交错连接不继承 PG 解析身份；双方均未验证。 |

官方规则（2026-10-23 查阅）依据 PostgreSQL **18** SQL 命令 [INSERT 的 Synopsis 与 ON CONFLICT Clause](https://www.postgresql.org/docs/18/sql-insert.html)：目标、动作、两类谓词及 excluded 的责任分别保留。错误类别依据 [附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html)，错误位置依据 §54.8 [Error and Notice Message Fields](https://www.postgresql.org/docs/18/protocol-error-fields.html) 的 P：原查询从 1 起按字符计数，不直接使用改写后字节偏移。合法未支持路径的目标拒绝码为 0A000，不能冒充参考 PG 语法错误；本轮未证明当前门禁。MySQL 对照依据 **8.4** §15.2.7.2 [INSERT ... ON DUPLICATE KEY UPDATE](https://dev.mysql.com/doc/refman/8.4/en/insert-on-duplicate.html)，独立保留原生契约。18.6 仍仅为来源目标，本轮没有该补丁版本的实测证据。

#### 数组构造语法边界

本块对应源工作流 2 步骤 2 的数组构造分类，只定义 `ARRAY[...]` 的解析结构与类型交接。当前证据基于提交 `59c3c7c0535b3b54a1764401d0fccb244bfbcd8a` 的源码/独立测试阅读，没有行为实测。`expressions.rs::Expr` 有 Value、Raw、Row 等变体，`RowExpr::values` 是行字段列表，该枚举没有专门的数组构造变体；不能把 Raw 文本或 Row 列表当作数组结构保真的证据。未入图的 `parser.y` 已直接核对 `SimpleExpr` / `ArrayKwdOpt`：现有 ARRAY 关键字用于 CAST 等路径，`SetArray` 标志不等于 PG 的方括号构造语法，也不能从这些 Go 语义动作推断 Rust 实际接线或当前拒绝结果。

`pg_extended.rs::Extended::process` 的 Parse 分支经过会话/目录识别、名称及 SQL 适配和 `markers` 后准备语句；普通非目录路径的 OID 白名单没有数组 OID，未知参数类型另有 0A000 门禁。这只说明该参数入口的局部限制，不证明无参数数组 SQL 的接受/拒绝。`markers` 保存重复/乱序参数的映射，不保存原 SQL 诊断跨度。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_expression` 注册项和 `run_contract_table_with_options`：它们检查用例表接受性与顶层 AST 类型，不能证明数组嵌套树和类型保真；已读 [pg_extended_test.rs](../pkg/server/pg_extended_test.rs) 的 `parse_bind_execute_sync` 中 current_catalog 的 Parse/Bind/Describe/Execute/Sync 断言，该段不是数组用例。本轮未执行这些测试，亦未证明其他用例是否覆盖数组。`pkg/parser/doc.go`、`pkg/server/doc.go` 均不存在。

承载方案交本章后续解析方案任务比较共享 AST 扩展与受控 PG 入口，不预设新接口。解析层必须保留 ARRAY 构造类别、有序元素表达式树、每层方括号及逗号跨度、空列表、内层 ARRAY 的显式性、显式转换的类型名/数组标记及其作用层级、参数身份与原 SQL 字符位置。不能扁平化嵌套、按文本逗号切割或替换为 ROW、JSON_ARRAY、字符串字面量。语法嵌套不等于最终值维数：解析保留树，矩形形状约束和表达式求值得到的维度交后续语义验收；数组类型、共同元素类型、空数组定型、转换、NULL 与 OID 身份归工作流 3，参数/结果文本和二进制编码、错误字段和恢复归工作流 7。Q 与扩展 Parse 使用同一 PG 语法契约，不以 Parse 成功代表可 Bind 或可返回数组。

下表为待新增 `ARRAY-SYNTAX` 用例族，按第 1 章登记能力 ID/case ID。无表依赖；参数场景另记录参数类型、值及编码。所有行为均未验证，成功目标 SQLSTATE 为“不适用（成功）”、无错误位置。表中数组值用逻辑元素/形状表达，不承诺 wire 文本格式、列名或 OID；结果类型由工作流 3 细化，协议描述及编码由工作流 7 验收。

| 正反例输入 | PG 目标结构与类型边界 | MySQL 适用性、诊断及证据责任 |
| --- | --- | --- |
| 正例 `SELECT ARRAY[1,2]` | 保留两项整数表达式及顺序，目标逻辑数组元素为 1、2；共同元素类型交工作流 3，不能用两列行值替代。 | PG 写法在 MySQL 为隔离负例，应按自身语法拒绝；MySQL SQLSTATE/位置须采集自身参考端和本地基线，不套用 PG 码。 |
| 正例 `SELECT ARRAY[ARRAY[1,2],ARRAY[3,4]]` 与 `SELECT ARRAY[[1,2],[3,4]]` | 保留两种原始写法及相同嵌套分组；目标为 2×2 矩形、元素按上述顺序。不能因省略内层 ARRAY 而丢层级。 | 两种 PG 写法均为 MySQL 隔离负例；诊断仍待取证。矩形形状和值的运行验证另行实施。 |
| 语法正例/定型反例 `SELECT ARRAY[]`；带类型正例 `SELECT ARRAY[]::integer[]` | 空列表须可表示且不等于 NULL、零列或语法残缺；独立无类型上下文的前者不能得到无类型数组成功结果，后者交工作流 3 定型为空整数数组。裸空数组的参考 SQLSTATE、阶段及精确位置待 PG 18 参考端采集，不猜码；解析层不得擅自默认 text[]。 | 原 PG 输入在 MySQL 为负例；自身空值/JSON 语法按自身契约验收，不能要求得到 PG 空整数数组。 |
| 混合元素边界 `SELECT ARRAY[1,2.5]`、`SELECT ARRAY[1,true]` | 均保留元素树和各自类型信息；前者目标共同数值类型，后者无法按共同类型规则统一，交工作流 3 判定并拒绝。精确类型、转换结果及类型失败 SQLSTATE/位置由该工作流补参考端证据，不能一律当 42601。 | MySQL 对原输入按语法拒绝，不能先按 PG 元素共同类型规则求值；当前未采集诊断。 |
| 形状反例 `SELECT ARRAY[[1,2],[3]]` | 保留不等长内层结构，不伪造缺失元素；矩形约束失败由后续语义阶段拒绝，错误阶段、SQLSTATE 与 P 是否出现待参考端取证，不能因方括号闭合就声称执行成功。 | 原输入不是 MySQL 数组构造正例；其自身 JSON 嵌套不等长规则不参与 PG 矩形验收。 |
| 语法反例 `SELECT ARRAY[1,2` | 未闭合方括号，PG 拒绝目标 42601；解析器定位输入末尾的缺失闭合处，精确 P 值待参考端采集，禁止补括号后执行。 | MySQL 可更早拒绝该 PG 构造；具体错误与位置独立采集。工作流 7 传输诊断，不把改写后字节偏移当原 SQL 字符位置。 |
| 参数/词法保护 `SELECT ARRAY[$2,$1,$2]`；`SELECT 'ARRAY[1,2]' /* ARRAY[] */` | 前者保留重复/乱序身份与元素位置，类型及 Bind 另验收；后者仅为普通字符串，不能制造构造节点。前者有充分类型上下文后才是完整成功用例；后者为双方言自身正例，目标原字符串。 | MySQL 不接受前者 PG 构造及 $n 参数契约，自身参数用 ?；交错连接验证不泄露 PG 解析身份。后者成功时无错误位置，双方均未实测。 |
| MySQL 自身正例 `SELECT JSON_ARRAY(1,2)` | 不作为 PG 数组构造正例或等价翻译；该函数在 PG 下的存在性/行为另行取证。 | 按 MySQL JSON 构造规则目标为 JSON 数组，成功时无错误码/位置；其 JSON 类型及编码按自身契约，不比作 PG integer[]。本轮未实测。 |

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.2.12 [Array Constructors](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-ARRAY-CONSTRUCTORS) 定义方括号、嵌套、省略内层 ARRAY、矩形约束及空数组显式定型；§10.5 [共同类型判定](https://www.postgresql.org/docs/18/typeconv-union-case.html) 交接工作流 3。[附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 区分语法错误 42601 与功能未支持 0A000；合法但暂未支持的形态目标明确以 0A000 拒绝，不能冒充参考 PG 语法非法，本轮未证明当前门禁。§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 规定 P 指向原查询、从 1 起按字符计数。MySQL **8.4** §11.5 [Expressions](https://dev.mysql.com/doc/refman/8.4/en/expressions.html) 与 §14.17.2 [JSON_ARRAY](https://dev.mysql.com/doc/refman/8.4/en/json-creation-functions.html#function_json-array) 仅定义自身语法对照，不证明本地入口已拒绝 PG 构造。18.6 仍仅为来源目标，无该补丁版本实测结论。

#### 行构造语法边界

本块对应源工作流 2 步骤 2 的行构造分类，只定义 `ROW(...)`、多字段括号行和单表达式括号的解析边界。当前静态证据基于提交 `d3995a985309c6393975d051cb90b98be5176bb9`，不是运行通过证据。已核对 [expressions.rs](../pkg/parser/ast/expressions.rs) 的 `RowExpr::values: Vec<Expr>` 与 `Expr::restore` 的 Row 分支：可保存有序字段树，恢复时统一输出 ROW，载体本身没有显式 ROW 与隐式括号的来源标记或逐项跨度。共享载体是候选，不能据其存在推断 PG 解析已接线或匿名行类型已可执行。未入图的 [parser.y](../pkg/parser/parser.y) 的 `SimpleExpr` 分别将 `(Expression)` 建为 ParenthesesExpr，将 `(ExpressionList, Expression)` 和 `ROW(ExpressionList, Expression)` 建为 RowExpr；后两条要求至少两项，且含 Go 语义动作，不证明 Rust 当前接受范围。

已读 [pg_name.rs](../pkg/server/pg_name.rs) 的 `Resolver::close` / `scope` / `relation`：close 配对括号，scope 对扫描遇到的括号递归，并不建立行表达式树；relation 在关系位置只允许以 SELECT/WITH 开头的括号，否则返回 0A000 的关系分组拒绝。这是局部名称适配限制，不能推广到 SELECT 列表中的行构造，更不能把所有括号当子查询或关系。已读 [parser_test.rs](../pkg/parser/parser_test.rs) 的 `test_expression` 注册与 `run_contract_table_with_options`，其接受性及顶层类型对照没有直接断言本块的字段嵌套、原始括号和错误位置；已读 [pg_name_test.rs](../pkg/server/pg_name_test.rs) 的 `pg_introspection_names_token_scopes`（含子查询/CTE、extract 内 FROM 及字面量/注释）和 `pg_introspection_names_rejection_boundaries`（引号、注释及名称拒绝）。这些不是本块新增专项断言，亦未执行；`pkg/parser/doc.go`、`pkg/parser/ast/doc.go`、`pkg/server/doc.go` 不存在。

目标由共享 AST 扩展或受控 PG 解析入口承载，选择交后续解析方案任务取证，不预设新接口。两种方案都须区分行构造、分组表达式、标量子查询和 FROM 关系括号；保留显式 ROW 的出现性、字段数量与顺序、逐字段表达式树、嵌套层级、括号/逗号及原 SQL 字符跨度，不能按逗号文本切分、扁平化或改写成 SELECT 列表。字段顺序不承诺求值顺序；字符串、注释及引号内的 ROW/括号不制造节点。行类型、字段类型、转换及参数定型交工作流 3，行比较、NULL 及运算符语义交工作流 3、4；工作流 7 负责复合值元数据/编码、错误传输与恢复。Q 与扩展 Parse 共用 PG 契约，保留 `$n` 身份；解析接受不能替代 Bind、Describe 或结果编码证据。

下表是待新增 `ROW-SYNTAX` 用例族，按第 1 章登记能力 ID/case ID；除关系例外无表依赖，所有行为均未验证。PG 成功目标 SQLSTATE 为“不适用（成功）”、无错误位置；行值以逻辑字段描述，不预定 wire 文本、列名或 OID。类型可判定是目标分析结论，不表示本地实现已具备该能力。

| 输入与正反例 | PG 语法、类型与执行目标 | MySQL 适用性及诊断责任 |
| --- | --- | --- |
| 正例 `SELECT ROW(1,2)` | 语法合法；保留显式 ROW 和有序两字段；目标可定为匿名 record，字段为两个整数，精确字段类型/OID 交工作流 3。目标执行返回一列复合值，字段依次为 1、2，不能返回两列。 | ROW 多字段也是 MySQL 行语法；不能标为 PG 专属负例。但该 SELECT 投影的合法语法不保证 MySQL 可作为单列复合值返回；适用上下文、标量限制及 SQLSTATE/位置须 MySQL 参考端取证，不承诺同结果。 |
| 正例 `SELECT (1,2)` | 语法合法的隐式两字段行，与前例同逻辑字段和匿名行类型目标；保留隐式形式及跨度，不当子查询。执行目标仍是一列复合值，尚未验证。 | 多字段括号行也是 MySQL 语法，投影执行限制独立验收，同样不能仅因语法接受声称可执行。 |
| 分组正例/单字段行反例 `SELECT (1)`；对比 `SELECT ROW(1)` | 前者语法合法、类型为标量整数、目标值 1，不能建成单字段行；后者合法且为含整数的单字段匿名 record，目标一列复合值。两者类型可区分，执行和协议仍待证。 | `(1)` 是自身分组正例，目标标量 1；MySQL ROW 形式要求至少两项，`ROW(1)` 为语法负例。具体 MySQL 错误码/位置另取证，不套 PG 码。 |
| 语法反例 `SELECT ROW(1,` | 缺少下一字段及闭合括号，拒绝目标 42601；无合法完整树，不进入类型判定或执行，不能自动补齐。定位输入末尾，精确 P 待 PG 18 参考端采集。 | 同样是不完整输入，应拒绝；MySQL 码和位置独立取证。解析层负责原 SQL 定位，PG 协议层传递错误字段。 |
| 嵌套正例 `SELECT ROW(1,ROW(2,3))` | 语法合法；外层两字段，第二字段仍为两字段行，不能扁平化成三字段。匿名行及内层字段类型交工作流 3，目标执行保持嵌套值；复合结果编码须工作流 7 补证。 | 只作为自身行表达式上下文候选；嵌套可执行范围、类型及诊断须取证，不推导与 PG 复合值等价。 |
| 参数正例 `SELECT ROW($2,$1,$2)`（扩展 Parse，显式整数参数类型，绑定 1、2） | 保留参数编号、重复身份及字段位置，目标逻辑字段 2、1、2；语法完整，依给定参数类型定型，Parse/Bind/结果仍须分别验证。无类型上下文的参数不能直接声称可定型。 | MySQL 使用自身 `?` 和行语法上下文，不能继承 `$n` 或 PG record 类型；具体执行用例待 MySQL 基线取证。 |
| 名称交互正例 `SELECT ROW(t.id,(SELECT 2)) FROM public.t AS t`；对比关系分组 `SELECT * FROM (public.t) AS t` | 假定 public.t 有整数 id；前者行字段含标量子查询，外层关系名照常绑定，类型依赖绑定/子查询结果，执行待证。后者属于关系括号，当前名称适配有独立拒绝边界，不能用该拒绝否定表达式 ROW；关系组支持不由本块扩写。 | MySQL 用自身已建 t 和合法限定名验证，public 不自动视为 PG schema；关系组诊断独立取证，不要求双方言名称改写相同。 |
| 词法保护正例 `SELECT 'ROW(1,2)' /* (1,2) */` | 只产生普通字符串，目标原字符串值；没有行节点、无行类型需求，语法/定型/执行目标明确但未验证。 | 自身正例，目标相同字符串；双方言交错连接的行/分组正反例验证身份不泄露，仍须分别记录类型与诊断。 |

官方依据（2026-10-23 查阅）：PostgreSQL **18** §4.2.13 [Row Constructors](https://www.postgresql.org/docs/18/sql-expressions.html#SQL-SYNTAX-ROW-CONSTRUCTORS) 定义显式 ROW 与多字段省略形式及匿名 record；§4.2 的表达式分类及 §4.2.11 区分分组与标量子查询。同页还定义零字段 ROW 和 `rowvalue.*` 形式，它们应由后续任务独立定义支持集合，不在本块四个核心输入之外擅自宣称支持。MySQL **8.4** §11.5 [Expressions](https://dev.mysql.com/doc/refman/8.4/en/expressions.html) 提供分组、ROW 至少两项及子查询的自身语法依据，投影/嵌套的实际诊断仍缺参考端证据。[PG 附录 A](https://www.postgresql.org/docs/18/errcodes-appendix.html) 区分 42601 与合法未支持目标拒绝码 0A000；不能把类型/上下文错误一律作为语法错误，本轮未证明当前门禁。§54.8 [错误字段](https://www.postgresql.org/docs/18/protocol-error-fields.html) 要求 P 若提供须基于原查询、从 1 起按字符定位，精确值及是否提供由参考端补证。18.6 仍仅为来源目标，没有该补丁版本或本地数据库实测。

### 范围外

[方言身份传递](#方言身份传递)本轮不实现方言类型、解析原型、共享 AST、缓存接线或运行测试平台；不扩建 resolver、planner、表达式子系统。六类语法、引号/大小写、限定名、CTE 及 schema/search_path 名称规则由后续文档块定义；会话设置执行归工作流 5，目录实现归工作流 6，错误传输与恢复协作工作流 7。现有文本适配只作为有界入口证据，不承担通用 SQL 翻译方案，也不据其推断完整 PG 支持。

[类型转换语法边界](#类型转换语法边界)不实现 PG parser、转换执行、类型推断或元数据；不扩建类型目录、数组/行子系统。转换语义归工作流 3，协议 OID、错误传输与恢复协作工作流 7。

[大小写匹配语法边界](#大小写匹配语法边界)不实现 parser/AST 接线、匹配算法、locale/collation、NULL、重载或错误响应；求值语义交工作流 3、4，协议诊断交工作流 7，不扩展其他匹配家族。

[返回子句语法边界](#返回子句语法边界)不实现解析器、共享 AST 接线、投影求值、事务或 wire 编码；真实写入/返回行交工作流 5，类型和结果描述协作工作流 3、7。ON CONFLICT 本身及 OLD/NEW、MERGE 等组合不由本块扩写实现。

[冲突子句语法边界](#冲突子句语法边界)不实现 upsert、唯一性/索引推断、锁、并发、更新求值和事务；执行与副作用交工作流 5，名称/类型协作本章后续规则及工作流 3，错误响应交工作流 7。

[数组构造语法边界](#数组构造语法边界)不实现数组类型、下标、运算符、存储或 wire codec；ARRAY(子查询) 是另一个合法构造形式，留待单独定义支持集合，不能当作本块已覆盖。类型、转换及 OID 归工作流 3，参数/结果编码归工作流 7。

[行构造语法边界](#行构造语法边界)不实现 parser/AST、复合类型、行比较、NULL 求值或协议编码，不补建关系分组解析；类型与转换交工作流 3，比较/NULL 交工作流 3、4，元数据和错误响应交工作流 7。

### 验收

[方言身份传递](#方言身份传递)的文档通过条件：六段责任链均区分实际入口、状态拥有者/生命周期、目标与缺口；简单/扩展查询身份一致；PG/MySQL 同时连接及两 PG 连接交错场景可单独拆分，负例、错误责任与证据状态明确；局部测试阅读不升级为全链路支持。当前仅文档契约交付；parser 内部接线、共享名称绑定、规划/缓存、表达式上下文、错误位置及真实双方言隔离行为全部未验证，实施后须按第 1 章采集有效证据。

[类型转换语法边界](#类型转换语法边界)的文档验收为：上述输入形态、优先级/嵌套、参数位置、缺类型反例及词法保护均有双方言目标、诊断责任和证据状态；AST 候选与保真缺口明确。现有测试阅读不替代新增树形/位置断言及实际双方言对照，所有行为仍未验证。

[大小写匹配语法边界](#大小写匹配语法边界)的文档验收为：操作符类别、左右操作数、否定、ESCAPE 显式性与保真、优先级及不完整输入均有双方言目标和诊断责任；当前源码、测试意图与运行证据区分清楚。全部运行行为仍未验证，不能以共享语法或同值结果代替 PG 树形与双方言隔离证据。

[返回子句语法边界](#返回子句语法边界)的文档验收为：三种 DML、表达式/星号/别名、空投影与错位反例、零行和词法保护均有双方言目标与诊断责任；解析树、返回列元数据、真实副作用分开验收。现有非空 Returning 的 0A000 门禁不得被当作功能成功，全部运行行为未验证。

[冲突子句语法边界](#冲突子句语法边界)的文档验收为：目标的三种形态、两种动作、两类谓词、excluded 与目标别名、缺动作/缺目标反例均有保真和诊断责任；MySQL 对照、参数/词法保护及执行交接明确。所有运行行为未验证，不能将普通 INSERT 测试或文本改写当作冲突行为通过。

[数组构造语法边界](#数组构造语法边界)的文档验收为：基本构造、两种嵌套写法、空数组定型、混合类型、非矩形与未闭合括号分别有树形、语义交接和诊断责任；PG/MySQL 正反例、参数/词法保护及证据限制齐全。文档完成不表示数组可解析、可绑定或可编码，所有运行行为未验证。

[行构造语法边界](#行构造语法边界)的文档验收为：ROW(1,2)、(1,2)、(1)、ROW(1, 四个核心输入分别区分语法合法、类型可判定与可执行；单字段 ROW 对照、顺序/嵌套、参数、关系括号及词法保护均有双方言责任和证据限制。当前全部运行行为未验证，不以解析或名称扫描成功替代语义兼容。

### 后续细分

[方言身份传递](#方言身份传递)后续先做连接身份拥有权及关闭/重连边界的局部取证，再分别拆简单查询与扩展查询的身份承接、共享解析/名称绑定/规划/表达式上下文的必要局部接线及隔离回归；实施前确认实际落点，不将本责任链自动扩张为完整子系统重建。各任务明确单一交接点、所需状态、现有可复用实现和验收断言；DIALECT-IDENTITY 三类场景是待新增验收入口，不是既有测试或已登记通过的能力。

[类型转换语法边界](#类型转换语法边界)后续先拆最小 `::` 解析承载取证，比较现有 Cast 与受控入口能否保留上述字段；再按优先级/嵌套、参数位置与词法保护、原 SQL 错误定位分别细分。每项复用现有 parser 与独立测试布局，只补必要局部接线；将转换语义及参数类型推断交工作流 3，不在此建立完整类型系统。

[大小写匹配语法边界](#大小写匹配语法边界)后续先核对 Rust parser 到现有载体的最小接线及 MySQL 扩展基线，比较共享 AST 与受控 PG 入口的保真成本；再分别拆谓词/NOT/比较组合、ESCAPE 信息与词法保护、原 SQL 诊断及隔离验收。沿现有独立测试布局补专项树形/位置断言，不递归修建执行子系统；匹配算法和转义求值另交工作流 3、4。

[返回子句语法边界](#返回子句语法边界)后续先按三种 DML 核对 Rust 解析动作到共享载体的最小保真，再拆表达式/星号/别名与原 SQL 位置、planner/Describe 列交接、双方言拒绝隔离。功能开放须等待工作流 5 的真实副作用/回滚和工作流 7 的结果描述证据，不递归补建执行或协议子系统。

[冲突子句语法边界](#冲突子句语法边界)后续先比较共享 AST 与受控 PG 入口保真，再分别拆目标/动作与双谓词、excluded 绑定、原 SQL 跨度与参数映射、双方言隔离的独立验收。先补解析树和拒绝证据；真实唯一性、并发及更新条件效果由工作流 5 单独细分，不递归扩建执行子系统。

[数组构造语法边界](#数组构造语法边界)后续先比较共享 AST 与受控入口保留构造类别、嵌套及跨度的最小方案，再拆基本/嵌套/空列表树形、原 SQL 定位、参数身份与双方言隔离验收；共同类型、形状语义及数组 OID 交工作流 3，Bind/Describe/结果编码交工作流 7 单独细分，不递归补建数组子系统。

[行构造语法边界](#行构造语法边界)后续先比较共享 Row 载体与受控 PG 入口保留显式性、嵌套和原 SQL 跨度的最小接线，再分别拆行/分组/子查询树形、名称扫描交互、错误位置及双方言隔离断言。沿独立测试布局取证，不递归扩建表达式或关系子系统；行类型与 NULL/比较按工作流 3、4 单独细分。

## 3. 共享类型与转换契约

## 4. 方言函数与运算符契约

## 5. SQL会话与事务语义

## 6. 系统目录与内省契约

## 7. 线协议认证与数据通道

## 8. 客户端与双方言共存验收
